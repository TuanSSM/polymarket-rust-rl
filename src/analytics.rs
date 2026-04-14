use num_traits::ToPrimitive;
use rust_decimal::Decimal;

/// Market analytics computed from trade data.
/// All Decimal→f64 conversions use `ToPrimitive::to_f64()`
/// instead of `to_string().parse::<f64>()` for correctness and performance.

#[derive(Debug, Clone)]
pub struct MarketAnalytics {
    pub vwap: f64,
    pub total_volume: f64,
    pub trade_count: u64,
    pub high: f64,
    pub low: f64,
    pub volatility: f64,
}

impl Default for MarketAnalytics {
    fn default() -> Self {
        Self {
            vwap: 0.0,
            total_volume: 0.0,
            trade_count: 0,
            high: f64::MIN,
            low: f64::MAX,
            volatility: 0.0,
        }
    }
}

/// Accumulator for incremental VWAP computation.
pub struct VwapAccumulator {
    cumulative_pv: f64,
    cumulative_volume: f64,
}

impl VwapAccumulator {
    pub fn new() -> Self {
        Self {
            cumulative_pv: 0.0,
            cumulative_volume: 0.0,
        }
    }

    /// Add a trade. Uses `to_f64()` for Decimal conversion.
    pub fn add_trade(&mut self, price: Decimal, volume: Decimal) {
        let p = price.to_f64().unwrap_or(0.0);
        let v = volume.to_f64().unwrap_or(0.0);
        self.cumulative_pv += p * v;
        self.cumulative_volume += v;
    }

    pub fn vwap(&self) -> f64 {
        if self.cumulative_volume == 0.0 {
            0.0
        } else {
            self.cumulative_pv / self.cumulative_volume
        }
    }

    pub fn total_volume(&self) -> f64 {
        self.cumulative_volume
    }
}

impl Default for VwapAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute realized volatility from a series of Decimal prices.
pub fn realized_volatility(prices: &[Decimal]) -> f64 {
    if prices.len() < 2 {
        return 0.0;
    }
    let f64_prices: Vec<f64> = prices.iter().filter_map(|p| p.to_f64()).collect();
    if f64_prices.len() < 2 {
        return 0.0;
    }
    let returns: Vec<f64> = f64_prices
        .windows(2)
        .map(|w| (w[1] / w[0]).ln())
        .collect();
    let mean = returns.iter().sum::<f64>() / returns.len() as f64;
    let variance =
        returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
    variance.sqrt()
}

/// Convert Decimal spread to f64.
pub fn spread_to_f64(bid: Decimal, ask: Decimal) -> f64 {
    let b = bid.to_f64().unwrap_or(0.0);
    let a = ask.to_f64().unwrap_or(0.0);
    a - b
}

/// Compute mid price from Decimal bid/ask.
pub fn mid_price(bid: Decimal, ask: Decimal) -> f64 {
    let b = bid.to_f64().unwrap_or(0.0);
    let a = ask.to_f64().unwrap_or(0.0);
    (a + b) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::prelude::*;

    #[test]
    fn vwap_accumulator_new() {
        let acc = VwapAccumulator::new();
        assert_eq!(acc.vwap(), 0.0);
        assert_eq!(acc.total_volume(), 0.0);
    }

    #[test]
    fn vwap_single_trade() {
        let mut acc = VwapAccumulator::new();
        acc.add_trade(dec!(100.0), dec!(10.0));
        assert!((acc.vwap() - 100.0).abs() < f64::EPSILON);
        assert!((acc.total_volume() - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn vwap_multiple_trades() {
        let mut acc = VwapAccumulator::new();
        acc.add_trade(dec!(100.0), dec!(10.0)); // 1000
        acc.add_trade(dec!(110.0), dec!(20.0)); // 2200
        // VWAP = 3200 / 30 = 106.666...
        let expected = 3200.0 / 30.0;
        assert!((acc.vwap() - expected).abs() < 1e-10);
    }

    #[test]
    fn vwap_empty() {
        let acc = VwapAccumulator::default();
        assert_eq!(acc.vwap(), 0.0);
    }

    #[test]
    fn realized_volatility_basic() {
        let prices = vec![dec!(100.0), dec!(102.0), dec!(101.0), dec!(103.0)];
        let vol = realized_volatility(&prices);
        assert!(vol > 0.0);
        assert!(vol < 1.0);
    }

    #[test]
    fn realized_volatility_insufficient() {
        assert_eq!(realized_volatility(&[]), 0.0);
        assert_eq!(realized_volatility(&[dec!(100.0)]), 0.0);
    }

    #[test]
    fn spread_to_f64_basic() {
        let spread = spread_to_f64(dec!(99.5), dec!(100.5));
        assert!((spread - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mid_price_basic() {
        let mid = mid_price(dec!(99.0), dec!(101.0));
        assert!((mid - 100.0).abs() < f64::EPSILON);
    }
}
