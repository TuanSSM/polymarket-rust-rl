/// Branchless gate: multiplies signal by 0.0 or 1.0 based on threshold.
/// Avoids branch misprediction in the hot path.
#[derive(Debug, Clone, Copy)]
pub struct BranchlessGate {
    pub threshold: f64,
}

impl BranchlessGate {
    pub fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// Returns `value * mask` where mask is 1.0 if `|value| >= threshold`, else 0.0.
    /// Uses sign-bit manipulation to avoid branching.
    #[inline(always)]
    pub fn apply(&self, value: f64) -> f64 {
        let exceeds = value.abs() - self.threshold;
        // Sign bit is 0 when exceeds >= 0 (pass), 1 when exceeds < 0 (block)
        let sign_bit = (exceeds.to_bits() >> 63) as f64;
        // mask = 1.0 when pass, 0.0 when block
        let mask = 1.0 - sign_bit;
        value * mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn above_threshold_passes() {
        let gate = BranchlessGate::new(0.5);
        assert_eq!(gate.apply(1.0), 1.0);
        assert_eq!(gate.apply(-1.0), -1.0);
        assert_eq!(gate.apply(0.5), 0.5); // exactly at threshold
    }

    #[test]
    fn below_threshold_blocks() {
        let gate = BranchlessGate::new(0.5);
        assert_eq!(gate.apply(0.3), 0.0);
        assert_eq!(gate.apply(-0.3), 0.0);
        assert_eq!(gate.apply(0.0), 0.0);
    }

    #[test]
    fn zero_threshold_passes_all_nonzero() {
        let gate = BranchlessGate::new(0.0);
        assert_eq!(gate.apply(0.001), 0.001);
        assert_eq!(gate.apply(-0.001), -0.001);
        // Zero itself: abs(0) - 0 = 0, sign bit is 0, so mask=1, result=0*1=0
        assert_eq!(gate.apply(0.0), 0.0);
    }

    #[test]
    fn large_values() {
        let gate = BranchlessGate::new(100.0);
        assert_eq!(gate.apply(200.0), 200.0);
        assert_eq!(gate.apply(50.0), 0.0);
        assert_eq!(gate.apply(-150.0), -150.0);
    }

    #[test]
    fn nan_handling() {
        let gate = BranchlessGate::new(0.5);
        // NaN propagates: NaN * anything = NaN
        let result = gate.apply(f64::NAN);
        assert!(result.is_nan() || result == 0.0);
    }
}
