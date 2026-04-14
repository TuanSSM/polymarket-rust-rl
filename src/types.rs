use serde::{Deserialize, Serialize};

/// Cache-line aligned wrapper to prevent false sharing between cores.
#[repr(align(64))]
pub struct CachePadded<T>(pub T);

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> std::ops::DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Long,
    Short,
    Flat,
}

/// Trade event passed through SPSC ring buffer.
/// Kept small for cache efficiency.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TradeEvent {
    pub timestamp_ns: u64,
    pub price: f64,
    pub quantity: f64,
    pub side: Side,
    pub market_id: u32,
}

/// Signal emitted by core engine after gate evaluation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Signal {
    pub timestamp_ns: u64,
    pub direction: Direction,
    pub strength: f64,
    pub market_id: u32,
}

/// Parameters published by controller via SeqLock.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Parameters {
    pub max_position: f64,
    pub risk_limit: f64,
    pub spread_threshold: f64,
    pub momentum_window: u32,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            max_position: 100.0,
            risk_limit: 0.05,
            spread_threshold: 0.002,
            momentum_window: 20,
        }
    }
}

/// Per-core position state. No shared access on hot path.
#[derive(Debug, Clone, Copy, Default)]
pub struct Position {
    pub quantity: f64,
    pub avg_entry_price: f64,
    pub unrealized_pnl: f64,
    pub market_id: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_padded_alignment() {
        let padded = CachePadded(42u64);
        let addr = &padded as *const _ as usize;
        assert_eq!(addr % 64, 0);
        assert_eq!(*padded, 42);
    }

    #[test]
    fn cache_padded_deref_mut() {
        let mut padded = CachePadded(10u64);
        *padded = 20;
        assert_eq!(*padded, 20);
    }

    #[test]
    fn parameters_default() {
        let p = Parameters::default();
        assert_eq!(p.max_position, 100.0);
        assert_eq!(p.risk_limit, 0.05);
        assert_eq!(p.spread_threshold, 0.002);
        assert_eq!(p.momentum_window, 20);
    }

    #[test]
    fn position_default() {
        let pos = Position::default();
        assert_eq!(pos.quantity, 0.0);
        assert_eq!(pos.avg_entry_price, 0.0);
        assert_eq!(pos.unrealized_pnl, 0.0);
        assert_eq!(pos.market_id, 0);
    }

    #[test]
    fn trade_event_serde_roundtrip() {
        let event = TradeEvent {
            timestamp_ns: 1_000_000,
            price: 0.55,
            quantity: 100.0,
            side: Side::Buy,
            market_id: 42,
        };
        let json = serde_json::to_string(&event).unwrap();
        let decoded: TradeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.timestamp_ns, event.timestamp_ns);
        assert_eq!(decoded.price, event.price);
        assert_eq!(decoded.side, event.side);
    }
}
