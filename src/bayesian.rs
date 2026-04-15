use crate::signal::SignalSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    BuyYes,
    BuyNo,
    NoTrade,
}

#[derive(Debug, Clone, Copy)]
pub struct ProbEstimate {
    pub prob_yes: f64,
    pub edge_bps: f64,
    pub direction: Direction,
}

/// Bayesian estimator: updates prior probability using signal likelihoods.
/// Uses log-odds form for numerical stability.
#[derive(Debug, Clone, Copy)]
pub struct BayesianEstimator {
    pub cvd_weight: f64,
    pub delay_weight: f64,
    pub premium_weight: f64,
}

impl BayesianEstimator {
    pub fn new(cvd_weight: f64, delay_weight: f64, premium_weight: f64) -> Self {
        Self {
            cvd_weight,
            delay_weight,
            premium_weight,
        }
    }

    /// Update prior with signal-derived likelihoods, returning probability estimate.
    ///
    /// Each signal contributes a log-likelihood ratio scaled by its weight:
    /// - CVD: buy pressure → higher prob_yes
    /// - Oracle delay + CEX move: stale oracle + CEX move → directional signal
    /// - Spot-perp premium: positive premium → bullish signal
    pub fn estimate(
        &self,
        prior: f64,
        signal: &SignalSnapshot,
        market_prob: f64,
    ) -> ProbEstimate {
        let prior_clamped = prior.clamp(0.01, 0.99);
        let log_odds = (prior_clamped / (1.0 - prior_clamped)).ln();

        // CVD contribution: sign indicates direction, magnitude clamped
        let cvd_contrib =
            self.cvd_weight * signal.spot_cvd.signum() * signal.spot_cvd.abs().min(1.0);

        // Delay contribution: oracle lag * CEX move direction
        let delay_contrib =
            self.delay_weight * signal.cex_move_bps * (signal.oracle_delay_ms / 1000.0).min(1.0);

        // Premium contribution: spot > perp → bullish
        let premium_contrib = self.premium_weight * signal.spot_perp_premium;

        let posterior_log_odds = log_odds + cvd_contrib + delay_contrib + premium_contrib;
        let prob_yes = (1.0 / (1.0 + (-posterior_log_odds).exp())).clamp(0.01, 0.99);

        let edge_bps = (prob_yes - market_prob) * 10_000.0;

        let direction = if edge_bps > 0.0 {
            Direction::BuyYes
        } else if edge_bps < 0.0 {
            Direction::BuyNo
        } else {
            Direction::NoTrade
        };

        ProbEstimate {
            prob_yes,
            edge_bps,
            direction,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn zero_signal() -> SignalSnapshot {
        SignalSnapshot {
            spot_cvd: 0.0,
            perp_cvd: 0.0,
            spot_perp_premium: 0.0,
            oracle_delay_ms: 0.0,
            oracle_price: 100.0,
            cex_mid_price: 100.0,
            cex_move_bps: 0.0,
            ts: Instant::now(),
        }
    }

    #[test]
    fn no_signal_returns_prior() {
        let est = BayesianEstimator::new(1.0, 1.0, 1.0);
        let result = est.estimate(0.5, &zero_signal(), 0.5);
        assert!((result.prob_yes - 0.5).abs() < 1e-6);
        assert!(result.edge_bps.abs() < 1e-6);
    }

    #[test]
    fn positive_cvd_raises_prob() {
        let est = BayesianEstimator::new(1.0, 0.0, 0.0);
        let mut sig = zero_signal();
        sig.spot_cvd = 0.8;

        let result = est.estimate(0.5, &sig, 0.5);
        assert!(result.prob_yes > 0.5);
        assert!(result.edge_bps > 0.0);
        assert_eq!(result.direction, Direction::BuyYes);
    }

    #[test]
    fn negative_cvd_lowers_prob() {
        let est = BayesianEstimator::new(1.0, 0.0, 0.0);
        let mut sig = zero_signal();
        sig.spot_cvd = -0.8;

        let result = est.estimate(0.5, &sig, 0.5);
        assert!(result.prob_yes < 0.5);
        assert!(result.edge_bps < 0.0);
        assert_eq!(result.direction, Direction::BuyNo);
    }

    #[test]
    fn delay_signal() {
        let est = BayesianEstimator::new(0.0, 1.0, 0.0);
        let mut sig = zero_signal();
        sig.oracle_delay_ms = 500.0;
        sig.cex_move_bps = 50.0;

        let result = est.estimate(0.5, &sig, 0.5);
        assert!(result.prob_yes > 0.5);
    }

    #[test]
    fn clamped_output() {
        let est = BayesianEstimator::new(100.0, 0.0, 0.0);
        let mut sig = zero_signal();
        sig.spot_cvd = 1.0;

        let result = est.estimate(0.5, &sig, 0.5);
        assert!(result.prob_yes <= 0.99);
        assert!(result.prob_yes >= 0.01);
    }
}
