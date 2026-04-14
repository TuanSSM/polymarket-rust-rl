use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Exchange identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exchange {
    BinanceSpot,
    BinanceFutures,
    Bybit,
}

/// Raw trade event from any exchange.
#[derive(Debug, Clone, Copy)]
pub struct TradeEvent {
    pub exchange: Exchange,
    pub symbol_hash: u64,
    pub price: f64,
    pub qty: f64,
    pub is_buy: bool,
    pub local_ts: Instant,
    pub exchange_ts_us: u64,
}

/// Chainlink oracle tick.
#[derive(Debug, Clone, Copy)]
pub struct OracleTick {
    pub feed_id: u64,
    pub price: f64,
    pub oracle_ts_us: u64,
    pub local_ts: Instant,
}

/// Computed signals broadcast to all CoreEngines.
#[derive(Debug, Clone, Copy)]
pub struct SignalSnapshot {
    pub spot_cvd: f64,
    pub perp_cvd: f64,
    pub spot_perp_premium: f64,
    pub oracle_delay_ms: f64,
    pub oracle_price: f64,
    pub cex_mid_price: f64,
    pub cex_move_bps: f64,
    pub ts: Instant,
}

/// Aggregates raw feeds into SignalSnapshot.
pub struct SignalEngine {
    spot_cvd: f64,
    perp_cvd: f64,
    last_spot_price: f64,
    last_perp_price: f64,
    last_oracle: Option<OracleTick>,
    oracle_price_at_update: f64,
    spot_prices: VecDeque<(Instant, f64)>,
    perp_prices: VecDeque<(Instant, f64)>,
    window: Duration,
}

impl SignalEngine {
    pub fn new(window: Duration) -> Self {
        Self {
            spot_cvd: 0.0,
            perp_cvd: 0.0,
            last_spot_price: 0.0,
            last_perp_price: 0.0,
            last_oracle: None,
            oracle_price_at_update: 0.0,
            spot_prices: VecDeque::with_capacity(1024),
            perp_prices: VecDeque::with_capacity(1024),
            window,
        }
    }

    /// Process a trade event. Updates CVD and price tracking.
    pub fn on_trade(&mut self, event: TradeEvent) {
        let signed_qty = if event.is_buy {
            event.qty
        } else {
            -event.qty
        };

        match event.exchange {
            Exchange::BinanceSpot | Exchange::Bybit => {
                self.spot_cvd += signed_qty;
                self.last_spot_price = event.price;
                self.spot_prices.push_back((event.local_ts, event.price));
                self.prune_window(&event.local_ts);
            }
            Exchange::BinanceFutures => {
                self.perp_cvd += signed_qty;
                self.last_perp_price = event.price;
                self.perp_prices.push_back((event.local_ts, event.price));
                self.prune_window(&event.local_ts);
            }
        }
    }

    /// Process an oracle tick. Updates delay tracking.
    pub fn on_oracle(&mut self, tick: OracleTick) {
        self.oracle_price_at_update = tick.price;
        self.last_oracle = Some(tick);
    }

    /// Build a snapshot of all current signals.
    pub fn snapshot(&self) -> SignalSnapshot {
        let now = Instant::now();

        let oracle_delay_ms = self
            .last_oracle
            .map(|o| o.local_ts.elapsed().as_secs_f64() * 1000.0)
            .unwrap_or(0.0);

        let oracle_price = self
            .last_oracle
            .map(|o| o.price)
            .unwrap_or(0.0);

        let cex_mid = if self.last_spot_price > 0.0 && self.last_perp_price > 0.0 {
            (self.last_spot_price + self.last_perp_price) / 2.0
        } else if self.last_spot_price > 0.0 {
            self.last_spot_price
        } else {
            self.last_perp_price
        };

        // CEX move since last oracle update, in basis points
        let cex_move_bps = if self.oracle_price_at_update > 0.0 && cex_mid > 0.0 {
            (cex_mid - self.oracle_price_at_update) / self.oracle_price_at_update * 10_000.0
        } else {
            0.0
        };

        // Spot-perp premium: positive means spot > perp
        let spot_perp_premium = if self.last_spot_price > 0.0 && self.last_perp_price > 0.0 {
            (self.last_spot_price - self.last_perp_price) / self.last_perp_price * 10_000.0
        } else {
            0.0
        };

        SignalSnapshot {
            spot_cvd: self.spot_cvd,
            perp_cvd: self.perp_cvd,
            spot_perp_premium,
            oracle_delay_ms,
            oracle_price,
            cex_mid_price: cex_mid,
            cex_move_bps,
            ts: now,
        }
    }

    /// Reset CVD accumulators (typically at episode boundaries).
    pub fn reset_cvd(&mut self) {
        self.spot_cvd = 0.0;
        self.perp_cvd = 0.0;
    }

    fn prune_window(&mut self, now: &Instant) {
        let cutoff = *now - self.window;
        while self
            .spot_prices
            .front()
            .is_some_and(|(ts, _)| *ts < cutoff)
        {
            self.spot_prices.pop_front();
        }
        while self
            .perp_prices
            .front()
            .is_some_and(|(ts, _)| *ts < cutoff)
        {
            self.perp_prices.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trade(exchange: Exchange, price: f64, qty: f64, is_buy: bool) -> TradeEvent {
        TradeEvent {
            exchange,
            symbol_hash: 0,
            price,
            qty,
            is_buy,
            local_ts: Instant::now(),
            exchange_ts_us: 0,
        }
    }

    #[test]
    fn cvd_accumulation() {
        let mut engine = SignalEngine::new(Duration::from_secs(60));

        engine.on_trade(make_trade(Exchange::BinanceSpot, 100.0, 1.0, true));
        engine.on_trade(make_trade(Exchange::BinanceSpot, 100.0, 0.5, false));
        engine.on_trade(make_trade(Exchange::BinanceFutures, 100.0, 2.0, true));

        let snap = engine.snapshot();
        assert!((snap.spot_cvd - 0.5).abs() < 1e-10);
        assert!((snap.perp_cvd - 2.0).abs() < 1e-10);
    }

    #[test]
    fn oracle_delay() {
        let mut engine = SignalEngine::new(Duration::from_secs(60));

        let tick = OracleTick {
            feed_id: 0,
            price: 100.0,
            oracle_ts_us: 0,
            local_ts: Instant::now(),
        };
        engine.on_oracle(tick);

        // Small sleep to create measurable delay
        std::thread::sleep(Duration::from_millis(5));

        let snap = engine.snapshot();
        assert!(snap.oracle_delay_ms >= 4.0);
        assert!((snap.oracle_price - 100.0).abs() < 1e-10);
    }

    #[test]
    fn spot_perp_premium() {
        let mut engine = SignalEngine::new(Duration::from_secs(60));

        engine.on_trade(make_trade(Exchange::BinanceSpot, 101.0, 1.0, true));
        engine.on_trade(make_trade(Exchange::BinanceFutures, 100.0, 1.0, true));

        let snap = engine.snapshot();
        // premium = (101 - 100) / 100 * 10000 = 100 bps
        assert!((snap.spot_perp_premium - 100.0).abs() < 1e-10);
    }

    #[test]
    fn cex_move_bps() {
        let mut engine = SignalEngine::new(Duration::from_secs(60));

        let tick = OracleTick {
            feed_id: 0,
            price: 100.0,
            oracle_ts_us: 0,
            local_ts: Instant::now(),
        };
        engine.on_oracle(tick);

        // CEX moves to 101
        engine.on_trade(make_trade(Exchange::BinanceSpot, 101.0, 1.0, true));

        let snap = engine.snapshot();
        // move = (101 - 100) / 100 * 10000 = 100 bps
        assert!((snap.cex_move_bps - 100.0).abs() < 1e-10);
    }

    #[test]
    fn reset_cvd() {
        let mut engine = SignalEngine::new(Duration::from_secs(60));
        engine.on_trade(make_trade(Exchange::BinanceSpot, 100.0, 5.0, true));
        engine.reset_cvd();

        let snap = engine.snapshot();
        assert!((snap.spot_cvd).abs() < 1e-10);
    }
}
