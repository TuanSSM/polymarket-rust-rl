use num_traits::ToPrimitive;
use rust_decimal::Decimal;

/// Cumulative Volume Delta (CVD) momentum indicator.
/// Tracks buy vs sell volume pressure for momentum signals.
///
/// All Decimal→f64 conversions use `ToPrimitive::to_f64()`
/// instead of `to_string().parse::<f64>()` round-trips.

#[derive(Debug, Clone)]
pub struct CvdMomentum {
    cvd: f64,
    ema_fast: f64,
    ema_slow: f64,
    alpha_fast: f64,
    alpha_slow: f64,
    initialized: bool,
    tick_count: u64,
}

impl CvdMomentum {
    /// Create a new CVD momentum tracker.
    pub fn new(fast_period: u32, slow_period: u32) -> Self {
        Self {
            cvd: 0.0,
            ema_fast: 0.0,
            ema_slow: 0.0,
            alpha_fast: 2.0 / (fast_period as f64 + 1.0),
            alpha_slow: 2.0 / (slow_period as f64 + 1.0),
            initialized: false,
            tick_count: 0,
        }
    }

    /// Update with a new trade using Decimal volume.
    pub fn update(&mut self, volume: Decimal, is_buy: bool) {
        let vol = volume.to_f64().unwrap_or(0.0);
        self.update_f64(vol, is_buy);
    }

    /// Update with f64 volume directly.
    pub fn update_f64(&mut self, volume: f64, is_buy: bool) {
        let delta = if is_buy { volume } else { -volume };
        self.cvd += delta;
        self.tick_count += 1;

        if !self.initialized {
            self.ema_fast = self.cvd;
            self.ema_slow = self.cvd;
            self.initialized = true;
        } else {
            self.ema_fast = self.alpha_fast * self.cvd + (1.0 - self.alpha_fast) * self.ema_fast;
            self.ema_slow = self.alpha_slow * self.cvd + (1.0 - self.alpha_slow) * self.ema_slow;
        }
    }

    /// Update with Decimal price and volume (volume-weighted CVD).
    pub fn update_weighted(&mut self, price: Decimal, volume: Decimal, is_buy: bool) {
        let p = price.to_f64().unwrap_or(0.0);
        let v = volume.to_f64().unwrap_or(0.0);
        let weighted_vol = p * v;
        let delta = if is_buy { weighted_vol } else { -weighted_vol };
        self.cvd += delta;
        self.tick_count += 1;

        if !self.initialized {
            self.ema_fast = self.cvd;
            self.ema_slow = self.cvd;
            self.initialized = true;
        } else {
            self.ema_fast = self.alpha_fast * self.cvd + (1.0 - self.alpha_fast) * self.ema_fast;
            self.ema_slow = self.alpha_slow * self.cvd + (1.0 - self.alpha_slow) * self.ema_slow;
        }
    }

    /// Momentum signal: EMA crossover value.
    pub fn momentum(&self) -> f64 {
        self.ema_fast - self.ema_slow
    }

    /// Normalized momentum in [-1.0, 1.0] range via sigmoid.
    pub fn normalized_momentum(&self) -> f64 {
        let m = self.momentum();
        if m.abs() < f64::EPSILON {
            0.0
        } else {
            m / (m.abs() + 1.0)
        }
    }

    pub fn cvd(&self) -> f64 {
        self.cvd
    }

    pub fn ema_fast(&self) -> f64 {
        self.ema_fast
    }

    pub fn ema_slow(&self) -> f64 {
        self.ema_slow
    }

    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }
}

/// Batch CVD computation from Decimal price/volume arrays.
pub fn batch_cvd(prices: &[Decimal], volumes: &[Decimal], sides: &[bool]) -> f64 {
    prices
        .iter()
        .zip(volumes.iter())
        .zip(sides.iter())
        .fold(0.0f64, |acc, ((p, v), &is_buy)| {
            let vol = p.to_f64().unwrap_or(0.0) * v.to_f64().unwrap_or(0.0);
            if is_buy {
                acc + vol
            } else {
                acc - vol
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::*;

    #[test]
    fn new_defaults() {
        let cvd = CvdMomentum::new(5, 20);
        assert_eq!(cvd.cvd(), 0.0);
        assert_eq!(cvd.momentum(), 0.0);
        assert_eq!(cvd.tick_count(), 0);
    }

    #[test]
    fn update_single_buy() {
        let mut cvd = CvdMomentum::new(5, 20);
        cvd.update(dec!(100.0), true);
        assert_eq!(cvd.cvd(), 100.0);
        assert_eq!(cvd.tick_count(), 1);
        assert!(cvd.momentum().abs() < f64::EPSILON); // first tick: fast == slow
    }

    #[test]
    fn update_single_sell() {
        let mut cvd = CvdMomentum::new(5, 20);
        cvd.update(dec!(100.0), false);
        assert_eq!(cvd.cvd(), -100.0);
        assert_eq!(cvd.tick_count(), 1);
    }

    #[test]
    fn momentum_crossover() {
        let mut cvd = CvdMomentum::new(3, 10);
        // Push consistent buys — fast EMA should lead slow EMA
        for _ in 0..20 {
            cvd.update_f64(10.0, true);
        }
        assert!(cvd.momentum() > 0.0, "fast should lead slow on buys");
    }

    #[test]
    fn normalized_momentum_range() {
        let mut cvd = CvdMomentum::new(3, 10);
        for _ in 0..50 {
            cvd.update_f64(100.0, true);
        }
        let nm = cvd.normalized_momentum();
        assert!(nm >= -1.0 && nm <= 1.0, "normalized out of range: {nm}");
        assert!(nm > 0.0); // consistent buys should be positive
    }

    #[test]
    fn batch_cvd_basic() {
        let prices = vec![dec!(100.0), dec!(101.0), dec!(99.0)];
        let volumes = vec![dec!(10.0), dec!(20.0), dec!(15.0)];
        let sides = vec![true, true, false];
        let result = batch_cvd(&prices, &volumes, &sides);
        // 100*10 + 101*20 - 99*15 = 1000 + 2020 - 1485 = 1535
        assert!((result - 1535.0).abs() < f64::EPSILON);
    }

    #[test]
    fn update_weighted_basic() {
        let mut cvd = CvdMomentum::new(5, 20);
        cvd.update_weighted(dec!(50.0), dec!(10.0), true);
        assert_eq!(cvd.cvd(), 500.0); // price * volume
        assert_eq!(cvd.tick_count(), 1);
    }
}
