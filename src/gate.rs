//! Branchless gate evaluation for risk checks.
//!
//! Gates compose multiplicatively: the product of all gate outputs
//! yields the final risk multiplier (0.0 = blocked, 1.0 = pass).
//! All operations use arithmetic instead of branches to avoid
//! branch misprediction penalties on the hot path.

/// Position limit gate: returns 1.0 if |position| < limit, else 0.0.
#[inline(always)]
pub fn position_limit_gate(position: f64, limit: f64) -> f64 {
    let headroom = limit - position.abs();
    let bits = headroom.to_bits();
    let sign = (bits >> 63) as f64;
    1.0 - sign
}

/// Signal strength gate: returns 1.0 if strength >= threshold, else 0.0.
#[inline(always)]
pub fn signal_strength_gate(strength: f64, threshold: f64) -> f64 {
    let diff = strength - threshold;
    let bits = diff.to_bits();
    let sign = (bits >> 63) as f64;
    1.0 - sign
}

/// Spread gate: returns 1.0 if spread <= max_spread, else 0.0.
#[inline(always)]
pub fn spread_gate(spread: f64, max_spread: f64) -> f64 {
    let diff = max_spread - spread;
    let bits = diff.to_bits();
    let sign = (bits >> 63) as f64;
    1.0 - sign
}

/// Directional agreement gate: returns 1.0 if signal and momentum agree.
#[inline(always)]
pub fn direction_gate(signal_dir: f64, momentum_dir: f64) -> f64 {
    let product = signal_dir * momentum_dir;
    let bits = product.to_bits();
    let sign = (bits >> 63) as f64;
    1.0 - sign
}

/// Compose position + signal + spread gates multiplicatively.
#[inline(always)]
pub fn evaluate_gates(
    position: f64,
    position_limit: f64,
    signal_strength: f64,
    strength_threshold: f64,
    spread: f64,
    max_spread: f64,
) -> f64 {
    position_limit_gate(position, position_limit)
        * signal_strength_gate(signal_strength, strength_threshold)
        * spread_gate(spread, max_spread)
}

/// Full gate evaluation with direction. Returns risk multiplier in [0.0, 1.0].
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn full_gate_eval(
    position: f64,
    position_limit: f64,
    signal_strength: f64,
    strength_threshold: f64,
    spread: f64,
    max_spread: f64,
    signal_dir: f64,
    momentum_dir: f64,
) -> f64 {
    evaluate_gates(
        position,
        position_limit,
        signal_strength,
        strength_threshold,
        spread,
        max_spread,
    ) * direction_gate(signal_dir, momentum_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_limit_under() {
        assert_eq!(position_limit_gate(50.0, 100.0), 1.0);
    }

    #[test]
    fn position_limit_at() {
        assert_eq!(position_limit_gate(100.0, 100.0), 1.0);
    }

    #[test]
    fn position_limit_over() {
        assert_eq!(position_limit_gate(101.0, 100.0), 0.0);
    }

    #[test]
    fn position_limit_negative_position() {
        assert_eq!(position_limit_gate(-50.0, 100.0), 1.0);
    }

    #[test]
    fn position_limit_negative_over() {
        assert_eq!(position_limit_gate(-101.0, 100.0), 0.0);
    }

    #[test]
    fn signal_strength_above() {
        assert_eq!(signal_strength_gate(0.05, 0.01), 1.0);
    }

    #[test]
    fn signal_strength_below() {
        assert_eq!(signal_strength_gate(0.005, 0.01), 0.0);
    }

    #[test]
    fn signal_strength_at_threshold() {
        assert_eq!(signal_strength_gate(0.01, 0.01), 1.0);
    }

    #[test]
    fn spread_under_max() {
        assert_eq!(spread_gate(0.001, 0.005), 1.0);
    }

    #[test]
    fn spread_over_max() {
        assert_eq!(spread_gate(0.01, 0.005), 0.0);
    }

    #[test]
    fn spread_at_max() {
        assert_eq!(spread_gate(0.005, 0.005), 1.0);
    }

    #[test]
    fn direction_agree() {
        assert_eq!(direction_gate(1.0, 1.0), 1.0);
        assert_eq!(direction_gate(-1.0, -1.0), 1.0);
    }

    #[test]
    fn direction_disagree() {
        assert_eq!(direction_gate(1.0, -1.0), 0.0);
        assert_eq!(direction_gate(-1.0, 1.0), 0.0);
    }

    #[test]
    fn evaluate_gates_all_pass() {
        let result = evaluate_gates(50.0, 100.0, 0.05, 0.01, 0.001, 0.005);
        assert_eq!(result, 1.0);
    }

    #[test]
    fn evaluate_gates_one_fails() {
        // position over limit
        let result = evaluate_gates(150.0, 100.0, 0.05, 0.01, 0.001, 0.005);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn full_gate_eval_all_pass() {
        let result = full_gate_eval(50.0, 100.0, 0.05, 0.01, 0.001, 0.005, 1.0, 1.0);
        assert_eq!(result, 1.0);
    }

    #[test]
    fn full_gate_eval_direction_blocks() {
        let result = full_gate_eval(50.0, 100.0, 0.05, 0.01, 0.001, 0.005, 1.0, -1.0);
        assert_eq!(result, 0.0);
    }
}
