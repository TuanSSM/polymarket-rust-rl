use rand::Rng;

/// State vector for the RL policy (all f64, no allocations).
#[derive(Debug, Clone, Copy)]
pub struct StateVec {
    pub edge_bps: f64,
    pub cvd_norm: f64,
    pub delay_norm: f64,
    pub premium_norm: f64,
    pub time_in_episode_frac: f64,
    pub position_frac: f64,
    pub spread_bps: f64,
}

impl StateVec {
    pub const DIM: usize = 7;

    #[inline]
    pub fn as_array(&self) -> [f64; Self::DIM] {
        [
            self.edge_bps,
            self.cvd_norm,
            self.delay_norm,
            self.premium_norm,
            self.time_in_episode_frac,
            self.position_frac,
            self.spread_bps,
        ]
    }
}

impl Default for StateVec {
    fn default() -> Self {
        Self {
            edge_bps: 0.0,
            cvd_norm: 0.0,
            delay_norm: 0.0,
            premium_norm: 0.0,
            time_in_episode_frac: 0.0,
            position_frac: 0.0,
            spread_bps: 0.0,
        }
    }
}

/// Discrete action space for limit-order management.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Action {
    Hold = 0,
    PostBid = 1,
    PostAsk = 2,
    CancelAll = 3,
    MarketBuy = 4,
    MarketSell = 5,
}

impl Action {
    pub const COUNT: usize = 6;

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Action::Hold,
            1 => Action::PostBid,
            2 => Action::PostAsk,
            3 => Action::CancelAll,
            4 => Action::MarketBuy,
            5 => Action::MarketSell,
            _ => Action::Hold,
        }
    }
}

/// Outcome of a completed episode, sent back to Controller.
#[derive(Debug, Clone, Copy)]
pub struct EpisodeOutcome {
    pub market_id: u64,
    pub pnl_usd: f64,
    pub fills: u32,
    pub cancels: u32,
    pub final_state: StateVec,
    pub last_action: Action,
    pub duration_ms: u64,
}

/// Published by Controller, read by CoreEngines via SegLock.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub weights: [[f64; StateVec::DIM]; Action::COUNT],
    pub biases: [f64; Action::COUNT],
    pub kelly_fraction_cap: f64,
    pub bankroll: f64,
    pub min_edge_bps: f64,
    pub max_position_usd: f64,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            weights: [[0.0; StateVec::DIM]; Action::COUNT],
            biases: [0.0; Action::COUNT],
            kelly_fraction_cap: 0.25,
            bankroll: 10_000.0,
            min_edge_bps: 50.0,
            max_position_usd: 1_000.0,
        }
    }
}

/// Linear Q-function: Q(s,a) = w_a . s + b_a
/// One weight vector per action.
#[derive(Debug, Clone)]
pub struct LinearPolicy {
    pub weights: [[f64; StateVec::DIM]; Action::COUNT],
    pub biases: [f64; Action::COUNT],
    pub alpha: f64,
    pub gamma: f64,
    pub epsilon: f64,
}

impl LinearPolicy {
    pub fn new(alpha: f64, gamma: f64, epsilon: f64) -> Self {
        Self {
            weights: [[0.0; StateVec::DIM]; Action::COUNT],
            biases: [0.0; Action::COUNT],
            alpha,
            gamma,
            epsilon,
        }
    }

    /// Q(s, a) = w_a . s + b_a
    #[inline]
    pub fn q_value(&self, state: &StateVec, action: Action) -> f64 {
        let s = state.as_array();
        let a = action as usize;
        let mut q = self.biases[a];
        for i in 0..StateVec::DIM {
            q += self.weights[a][i] * s[i];
        }
        q
    }

    /// Epsilon-greedy action selection.
    pub fn select_action(&self, state: &StateVec, rng: &mut impl Rng) -> Action {
        if rng.gen::<f64>() < self.epsilon {
            Action::from_index(rng.gen_range(0..Action::COUNT))
        } else {
            self.greedy_action(state)
        }
    }

    /// Greedy (argmax Q) action selection.
    pub fn greedy_action(&self, state: &StateVec) -> Action {
        let mut best_action = 0;
        let mut best_q = f64::NEG_INFINITY;
        for a in 0..Action::COUNT {
            let q = self.q_value(state, Action::from_index(a));
            if q > best_q {
                best_q = q;
                best_action = a;
            }
        }
        Action::from_index(best_action)
    }

    /// TD(0) update: w_a += alpha * delta * s
    /// delta = reward + gamma * max_a' Q(s', a') - Q(s, a)
    pub fn td0_update(
        &mut self,
        state: &StateVec,
        action: Action,
        reward: f64,
        next_state: &StateVec,
    ) {
        let q_sa = self.q_value(state, action);
        let max_q_next = (0..Action::COUNT)
            .map(|a| self.q_value(next_state, Action::from_index(a)))
            .fold(f64::NEG_INFINITY, f64::max);

        let delta = reward + self.gamma * max_q_next - q_sa;

        let s = state.as_array();
        let a = action as usize;
        for i in 0..StateVec::DIM {
            self.weights[a][i] += self.alpha * delta * s[i];
        }
        self.biases[a] += self.alpha * delta;
    }

    /// TD(0) terminal update (no next state).
    pub fn td0_terminal(&mut self, state: &StateVec, action: Action, reward: f64) {
        let q_sa = self.q_value(state, action);
        let delta = reward - q_sa;

        let s = state.as_array();
        let a = action as usize;
        for i in 0..StateVec::DIM {
            self.weights[a][i] += self.alpha * delta * s[i];
        }
        self.biases[a] += self.alpha * delta;
    }

    /// Load weights from Params (received via SegLock).
    pub fn load_params(&mut self, params: &Params) {
        self.weights = params.weights;
        self.biases = params.biases;
    }

    /// Export weights to Params struct.
    pub fn export_params(&self, risk: &crate::config::RiskConfig) -> Params {
        Params {
            weights: self.weights,
            biases: self.biases,
            kelly_fraction_cap: risk.max_kelly_fraction,
            bankroll: risk.bankroll_usd,
            min_edge_bps: risk.min_edge_bps,
            max_position_usd: risk.max_position_usd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::SmallRng;

    fn test_state() -> StateVec {
        StateVec {
            edge_bps: 1.0,
            cvd_norm: 0.5,
            delay_norm: 0.3,
            premium_norm: 0.1,
            time_in_episode_frac: 0.5,
            position_frac: 0.2,
            spread_bps: 0.8,
        }
    }

    #[test]
    fn q_value_zero_weights() {
        let policy = LinearPolicy::new(0.01, 0.99, 0.1);
        let state = test_state();
        // All weights zero → Q = 0 for all actions
        for a in 0..Action::COUNT {
            assert_eq!(policy.q_value(&state, Action::from_index(a)), 0.0);
        }
    }

    #[test]
    fn q_value_with_weights() {
        let mut policy = LinearPolicy::new(0.01, 0.99, 0.1);
        // Set weight[Hold][0] = 2.0, bias[Hold] = 1.0
        policy.weights[0][0] = 2.0;
        policy.biases[0] = 1.0;

        let state = test_state();
        // Q(s, Hold) = 1.0 + 2.0 * 1.0 = 3.0
        assert!((policy.q_value(&state, Action::Hold) - 3.0).abs() < 1e-10);
    }

    #[test]
    fn greedy_selects_highest_q() {
        let mut policy = LinearPolicy::new(0.01, 0.99, 0.0); // epsilon=0
        policy.biases[Action::MarketBuy as usize] = 10.0;

        let state = test_state();
        let action = policy.greedy_action(&state);
        assert_eq!(action, Action::MarketBuy);
    }

    #[test]
    fn epsilon_greedy_explores() {
        let policy = LinearPolicy::new(0.01, 0.99, 1.0); // epsilon=1 → always random
        let state = test_state();
        let mut rng = SmallRng::seed_from_u64(42);

        let mut action_counts = [0u32; Action::COUNT];
        for _ in 0..6000 {
            let a = policy.select_action(&state, &mut rng);
            action_counts[a as usize] += 1;
        }
        // With epsilon=1.0, all actions should be roughly equally likely
        for count in &action_counts {
            assert!(*count > 500, "action count too low: {count}");
        }
    }

    #[test]
    fn td0_update_moves_q_toward_target() {
        let mut policy = LinearPolicy::new(0.1, 0.99, 0.1);
        let state = test_state();
        let next_state = StateVec::default();

        let q_before = policy.q_value(&state, Action::Hold);
        policy.td0_update(&state, Action::Hold, 1.0, &next_state);
        let q_after = policy.q_value(&state, Action::Hold);

        // Positive reward should increase Q
        assert!(q_after > q_before);
    }

    #[test]
    fn td0_terminal() {
        let mut policy = LinearPolicy::new(0.1, 0.99, 0.1);
        let state = test_state();

        policy.td0_terminal(&state, Action::PostBid, 5.0);
        let q = policy.q_value(&state, Action::PostBid);
        // Should move toward reward of 5.0
        assert!(q > 0.0);
    }

    #[test]
    fn params_roundtrip() {
        let mut policy = LinearPolicy::new(0.01, 0.99, 0.1);
        policy.weights[1][2] = 3.14;
        policy.biases[3] = -1.5;

        let risk = crate::config::RiskConfig {
            max_position_usd: 1000.0,
            max_kelly_fraction: 0.25,
            bankroll_usd: 10000.0,
            min_edge_bps: 50.0,
        };

        let params = policy.export_params(&risk);
        let mut policy2 = LinearPolicy::new(0.01, 0.99, 0.1);
        policy2.load_params(&params);

        assert_eq!(policy2.weights[1][2], 3.14);
        assert_eq!(policy2.biases[3], -1.5);
    }

    #[test]
    fn action_from_index_roundtrip() {
        for i in 0..Action::COUNT {
            let a = Action::from_index(i);
            assert_eq!(a as usize, i);
        }
    }
}
