use crate::bayesian::Direction;

/// Kelly criterion position sizer for binary prediction markets.
#[derive(Debug, Clone, Copy)]
pub struct KellySizer {
    pub max_fraction: f64,
    pub bankroll: f64,
    pub min_edge_bps: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct SizeDecision {
    pub stake_usd: f64,
    pub kelly_fraction: f64,
    pub capped: bool,
}

impl KellySizer {
    pub fn new(max_fraction: f64, bankroll: f64, min_edge_bps: f64) -> Self {
        Self {
            max_fraction,
            bankroll,
            min_edge_bps,
        }
    }

    /// Compute Kelly optimal stake for a binary market.
    ///
    /// Full Kelly: f* = (p*b - q) / b
    /// where b = (1 - market_price) / market_price (payout odds),
    ///       p = estimated win probability,
    ///       q = 1 - p.
    pub fn size(&self, prob: f64, market_price: f64, direction: Direction) -> SizeDecision {
        let zero = SizeDecision {
            stake_usd: 0.0,
            kelly_fraction: 0.0,
            capped: false,
        };

        if direction == Direction::NoTrade {
            return zero;
        }

        let (p, price) = match direction {
            Direction::BuyYes => (prob, market_price),
            Direction::BuyNo => (1.0 - prob, 1.0 - market_price),
            Direction::NoTrade => unreachable!(),
        };

        // Guard against degenerate prices
        if price <= 0.0 || price >= 1.0 {
            return zero;
        }

        let b = (1.0 - price) / price; // payout odds
        let q = 1.0 - p;
        let f_star = (p * b - q) / b;

        // Check minimum edge
        if f_star <= 0.0 || (f_star * 10_000.0) < self.min_edge_bps {
            return SizeDecision {
                stake_usd: 0.0,
                kelly_fraction: f_star,
                capped: false,
            };
        }

        let capped = f_star > self.max_fraction;
        let fraction = f_star.min(self.max_fraction);
        let stake = fraction * self.bankroll;

        SizeDecision {
            stake_usd: stake,
            kelly_fraction: f_star,
            capped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_edge_no_trade() {
        let sizer = KellySizer::new(0.25, 10_000.0, 50.0);
        // Market price = true prob → zero edge
        let result = sizer.size(0.5, 0.5, Direction::BuyYes);
        assert!(result.stake_usd == 0.0);
    }

    #[test]
    fn positive_edge_produces_stake() {
        let sizer = KellySizer::new(0.25, 10_000.0, 0.0);
        // prob=0.6, price=0.5 → positive edge
        let result = sizer.size(0.6, 0.5, Direction::BuyYes);
        assert!(result.stake_usd > 0.0);
        assert!(result.kelly_fraction > 0.0);
    }

    #[test]
    fn fraction_capped() {
        let sizer = KellySizer::new(0.1, 10_000.0, 0.0);
        // Large edge → f* > 0.1 → should be capped
        let result = sizer.size(0.9, 0.5, Direction::BuyYes);
        assert!(result.capped);
        assert!(result.stake_usd <= 0.1 * 10_000.0 + 1e-10);
    }

    #[test]
    fn buy_no_direction() {
        let sizer = KellySizer::new(0.25, 10_000.0, 0.0);
        // prob_yes=0.3 → edge on NO side
        let result = sizer.size(0.3, 0.6, Direction::BuyNo);
        // p(no)=0.7, price(no)=0.4 → positive edge
        assert!(result.stake_usd > 0.0);
    }

    #[test]
    fn no_trade_direction() {
        let sizer = KellySizer::new(0.25, 10_000.0, 0.0);
        let result = sizer.size(0.5, 0.5, Direction::NoTrade);
        assert_eq!(result.stake_usd, 0.0);
    }

    #[test]
    fn degenerate_price() {
        let sizer = KellySizer::new(0.25, 10_000.0, 0.0);
        let result = sizer.size(0.6, 0.0, Direction::BuyYes);
        assert_eq!(result.stake_usd, 0.0);

        let result = sizer.size(0.6, 1.0, Direction::BuyYes);
        assert_eq!(result.stake_usd, 0.0);
    }

    #[test]
    fn kelly_formula_correctness() {
        let sizer = KellySizer::new(1.0, 10_000.0, 0.0);
        // prob=0.6, price=0.5 → b=1.0, f* = (0.6*1 - 0.4)/1 = 0.2
        let result = sizer.size(0.6, 0.5, Direction::BuyYes);
        assert!((result.kelly_fraction - 0.2).abs() < 1e-10);
        assert!((result.stake_usd - 2_000.0).abs() < 1e-6);
    }
}
