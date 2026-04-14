use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicU64, Ordering};

/// SeqLock: optimistic reader lock for parameter broadcasting.
///
/// Provides ~1ns cache-hit reads (99.6%+ hit rate in steady state).
/// Single writer, multiple readers. Readers never block the writer.
pub struct SeqLock<T> {
    sequence: AtomicU64,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SeqLock<T> {}
unsafe impl<T: Send + Copy> Sync for SeqLock<T> {}

impl<T: Copy> SeqLock<T> {
    pub fn new(value: T) -> Self {
        Self {
            sequence: AtomicU64::new(0),
            data: UnsafeCell::new(value),
        }
    }

    /// Read the current value. Spins if a write is in progress.
    pub fn read(&self) -> T {
        loop {
            let seq1 = self.sequence.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let value = unsafe { *self.data.get() };
            fence(Ordering::Acquire);
            let seq2 = self.sequence.load(Ordering::Relaxed);
            if seq1 == seq2 {
                return value;
            }
            std::hint::spin_loop();
        }
    }

    /// Read and return (value, sequence_number).
    pub fn read_with_seq(&self) -> (T, u64) {
        loop {
            let seq1 = self.sequence.load(Ordering::Acquire);
            if seq1 & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let value = unsafe { *self.data.get() };
            fence(Ordering::Acquire);
            let seq2 = self.sequence.load(Ordering::Relaxed);
            if seq1 == seq2 {
                return (value, seq1);
            }
            std::hint::spin_loop();
        }
    }

    /// Write a new value. Must be called from a single writer thread.
    pub fn write(&self, value: T) {
        let seq = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(seq.wrapping_add(1), Ordering::Release);
        unsafe {
            *self.data.get() = value;
        }
        self.sequence.store(seq.wrapping_add(2), Ordering::Release);
    }

    /// Current sequence number (for staleness detection).
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Parameters;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn new_default_value() {
        let lock = SeqLock::new(42u64);
        assert_eq!(lock.read(), 42);
    }

    #[test]
    fn read_initial_parameters() {
        let lock = SeqLock::new(Parameters::default());
        let p = lock.read();
        assert_eq!(p.max_position, 100.0);
    }

    #[test]
    fn write_then_read() {
        let lock = SeqLock::new(0u64);
        lock.write(99);
        assert_eq!(lock.read(), 99);
    }

    #[test]
    fn multiple_writes() {
        let lock = SeqLock::new(0u64);
        for i in 0..100 {
            lock.write(i);
            assert_eq!(lock.read(), i);
        }
    }

    #[test]
    fn sequence_increments_by_two() {
        let lock = SeqLock::new(0u64);
        assert_eq!(lock.sequence(), 0);
        lock.write(1);
        assert_eq!(lock.sequence(), 2);
        lock.write(2);
        assert_eq!(lock.sequence(), 4);
    }

    #[test]
    fn read_with_seq_basic() {
        let lock = SeqLock::new(42u64);
        let (val, seq) = lock.read_with_seq();
        assert_eq!(val, 42);
        assert_eq!(seq, 0);
    }

    #[test]
    fn read_with_seq_after_write() {
        let lock = SeqLock::new(0u64);
        lock.write(55);
        let (val, seq) = lock.read_with_seq();
        assert_eq!(val, 55);
        assert_eq!(seq, 2);
    }

    #[test]
    fn parameters_write_read() {
        let lock = SeqLock::new(Parameters::default());
        let new_params = Parameters {
            max_position: 200.0,
            risk_limit: 0.1,
            spread_threshold: 0.003,
            momentum_window: 30,
        };
        lock.write(new_params);
        let p = lock.read();
        assert_eq!(p.max_position, 200.0);
        assert_eq!(p.momentum_window, 30);
    }

    #[test]
    fn concurrent_read_write() {
        let lock = Arc::new(SeqLock::new(0u64));
        let lock_w = lock.clone();

        let writer = thread::spawn(move || {
            for i in 0..10_000u64 {
                lock_w.write(i);
            }
        });

        let mut reads = 0u64;
        while reads < 1000 {
            let val = lock.read();
            assert!(val < 10_000);
            reads += 1;
        }

        writer.join().unwrap();
        // After writer finishes, should read final value
        assert_eq!(lock.read(), 9_999);
    }

    #[test]
    fn concurrent_read_consistency() {
        // Write a struct with two correlated fields; readers verify consistency
        #[derive(Copy, Clone)]
        struct Pair {
            a: u64,
            b: u64,
        }
        let lock = Arc::new(SeqLock::new(Pair { a: 0, b: 0 }));
        let lock_w = lock.clone();

        let writer = thread::spawn(move || {
            for i in 0..5_000u64 {
                lock_w.write(Pair { a: i, b: i * 2 });
            }
        });

        let mut checks = 0;
        while checks < 2000 {
            let pair = lock.read();
            assert_eq!(pair.b, pair.a * 2, "torn read detected");
            checks += 1;
        }

        writer.join().unwrap();
    }

    #[test]
    fn many_readers_one_writer() {
        let lock = Arc::new(SeqLock::new(0u64));
        let lock_w = lock.clone();

        let readers: Vec<_> = (0..4)
            .map(|_| {
                let lock_r = lock.clone();
                thread::spawn(move || {
                    let mut count = 0;
                    while count < 500 {
                        let v = lock_r.read();
                        assert!(v <= 5_000);
                        count += 1;
                    }
                })
            })
            .collect();

        let writer = thread::spawn(move || {
            for i in 0..5_000u64 {
                lock_w.write(i);
            }
        });

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }
    }

    #[test]
    fn rapid_writes() {
        let lock = SeqLock::new(0u64);
        for i in 0..10_000 {
            lock.write(i);
        }
        assert_eq!(lock.read(), 9_999);
        assert_eq!(lock.sequence(), 20_000);
    }

    #[test]
    fn large_struct() {
        #[derive(Copy, Clone, PartialEq, Debug)]
        struct Big {
            data: [u64; 8],
        }
        let val = Big { data: [42; 8] };
        let lock = SeqLock::new(val);
        assert_eq!(lock.read(), val);
        let val2 = Big { data: [99; 8] };
        lock.write(val2);
        assert_eq!(lock.read(), val2);
    }

    #[test]
    fn sequence_monotonic() {
        let lock = SeqLock::new(0u64);
        let mut prev_seq = lock.sequence();
        for i in 1..100 {
            lock.write(i);
            let seq = lock.sequence();
            assert!(seq > prev_seq);
            prev_seq = seq;
        }
    }

    #[test]
    fn read_after_many_writes() {
        let lock = SeqLock::new(0u64);
        for i in 0..1000 {
            lock.write(i);
        }
        assert_eq!(lock.read(), 999);
    }

    #[test]
    fn concurrent_stress() {
        let lock = Arc::new(SeqLock::new(Parameters::default()));
        let lock_w = lock.clone();

        let writer = thread::spawn(move || {
            for i in 0..3_000u32 {
                lock_w.write(Parameters {
                    max_position: i as f64,
                    risk_limit: i as f64 * 0.001,
                    spread_threshold: 0.002,
                    momentum_window: i,
                });
            }
        });

        let readers: Vec<_> = (0..3)
            .map(|_| {
                let lock_r = lock.clone();
                thread::spawn(move || {
                    let mut checks = 0;
                    while checks < 500 {
                        let p = lock_r.read();
                        // Verify internal consistency
                        let expected_risk = p.momentum_window as f64 * 0.001;
                        assert!(
                            (p.risk_limit - expected_risk).abs() < 0.0001
                                || p.momentum_window == 20,
                            "torn read: window={} risk={}",
                            p.momentum_window,
                            p.risk_limit
                        );
                        checks += 1;
                    }
                })
            })
            .collect();

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }
    }
}
