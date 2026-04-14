use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct SpscInner<T> {
    buf: Box<[UnsafeCell<MaybeUninit<T>>]>,
    cap: usize,
    mask: usize,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Safety: SPSC discipline — only producer writes head, only consumer writes tail.
// The AtomicUsize fences ensure visibility of buf writes across threads.
unsafe impl<T: Send> Sync for SpscInner<T> {}

pub struct SpscProducer<T> {
    inner: Arc<SpscInner<T>>,
    cached_tail: usize,
}

pub struct SpscConsumer<T> {
    inner: Arc<SpscInner<T>>,
    cached_head: usize,
}

// Safety: each half is used by exactly one thread.
unsafe impl<T: Send> Send for SpscProducer<T> {}
unsafe impl<T: Send> Send for SpscConsumer<T> {}

/// Create an SPSC channel with at least `min_cap` slots.
/// Actual capacity is rounded up to the next power of 2.
pub fn spsc_channel<T>(min_cap: usize) -> (SpscProducer<T>, SpscConsumer<T>) {
    let cap = min_cap.next_power_of_two().max(2);
    let mut buf = Vec::with_capacity(cap);
    for _ in 0..cap {
        buf.push(UnsafeCell::new(MaybeUninit::uninit()));
    }
    let inner = Arc::new(SpscInner {
        buf: buf.into_boxed_slice(),
        cap,
        mask: cap - 1,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    });
    (
        SpscProducer {
            inner: Arc::clone(&inner),
            cached_tail: 0,
        },
        SpscConsumer {
            inner,
            cached_head: 0,
        },
    )
}

impl<T> SpscProducer<T> {
    /// Try to push a value. Returns `Err(val)` if the buffer is full.
    pub fn try_push(&mut self, val: T) -> Result<(), T> {
        let head = self.inner.head.load(Ordering::Relaxed);
        let next_head = head.wrapping_add(1);

        // Check if full: refresh cached tail if needed
        if next_head.wrapping_sub(self.cached_tail) > self.inner.cap {
            self.cached_tail = self.inner.tail.load(Ordering::Acquire);
            if next_head.wrapping_sub(self.cached_tail) > self.inner.cap {
                return Err(val);
            }
        }

        let idx = head & self.inner.mask;
        unsafe {
            (*self.inner.buf[idx].get()).write(val);
        }
        self.inner.head.store(next_head, Ordering::Release);
        Ok(())
    }

    /// Push a value, overwriting the oldest entry if full.
    /// The producer simply writes and advances head. The consumer detects
    /// when it has been lapped and skips stale entries.
    /// Use for signal feeds where freshness matters more than completeness.
    pub fn push_overwrite(&mut self, val: T)
    where
        T: Copy,
    {
        let head = self.inner.head.load(Ordering::Relaxed);
        let idx = head & self.inner.mask;
        unsafe {
            (*self.inner.buf[idx].get()).write(val);
        }
        self.inner
            .head
            .store(head.wrapping_add(1), Ordering::Release);
    }
}

impl<T> SpscConsumer<T> {
    /// Try to pop a single value.
    pub fn try_pop(&mut self) -> Option<T> {
        let tail = self.inner.tail.load(Ordering::Relaxed);
        let head = self.inner.head.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        // If producer has lapped us (overwrite mode), skip to oldest valid entry
        let available = head.wrapping_sub(tail);
        let actual_tail = if available > self.inner.cap {
            let new_tail = head.wrapping_sub(self.inner.cap);
            self.inner.tail.store(new_tail, Ordering::Release);
            new_tail
        } else {
            tail
        };

        let idx = actual_tail & self.inner.mask;
        let val = unsafe { (*self.inner.buf[idx].get()).assume_init_read() };
        self.inner
            .tail
            .store(actual_tail.wrapping_add(1), Ordering::Release);
        self.cached_head = head;
        Some(val)
    }

    /// Drain all available items, returning only the most recent one.
    /// Efficient for signal consumers that only care about the latest value.
    pub fn drain_last(&mut self) -> Option<T>
    where
        T: Copy,
    {
        let head = self.inner.head.load(Ordering::Acquire);
        let tail = self.inner.tail.load(Ordering::Relaxed);

        if tail == head {
            return None;
        }

        // Read the most recent item (head - 1)
        let last_idx = head.wrapping_sub(1) & self.inner.mask;
        let val = unsafe { (*self.inner.buf[last_idx].get()).assume_init_read() };

        // Advance tail to head (skip all intermediate items)
        self.inner.tail.store(head, Ordering::Release);
        self.cached_head = head;

        Some(val)
    }
}

impl<T> Drop for SpscInner<T> {
    fn drop(&mut self) {
        let tail = *self.tail.get_mut();
        let head = *self.head.get_mut();
        let available = head.wrapping_sub(tail);
        // Only drop up to cap items (in overwrite mode, head may have lapped)
        let to_drop = available.min(self.cap);
        let start = head.wrapping_sub(to_drop);
        let mut idx = start;
        while idx != head {
            let buf_idx = idx & self.mask;
            unsafe {
                (*self.buf[buf_idx].get()).assume_init_drop();
            }
            idx = idx.wrapping_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_pop_basic() {
        let (mut tx, mut rx) = spsc_channel::<u64>(4);
        assert!(rx.try_pop().is_none());

        tx.try_push(1).unwrap();
        tx.try_push(2).unwrap();
        tx.try_push(3).unwrap();

        assert_eq!(rx.try_pop(), Some(1));
        assert_eq!(rx.try_pop(), Some(2));
        assert_eq!(rx.try_pop(), Some(3));
        assert!(rx.try_pop().is_none());
    }

    #[test]
    fn full_ring_returns_err() {
        let (mut tx, mut _rx) = spsc_channel::<u64>(2);
        assert!(tx.try_push(1).is_ok());
        assert!(tx.try_push(2).is_ok());
        assert!(tx.try_push(3).is_err());
    }

    #[test]
    fn overwrite_drops_oldest() {
        let (mut tx, mut rx) = spsc_channel::<u64>(2);
        tx.push_overwrite(1);
        tx.push_overwrite(2);
        tx.push_overwrite(3); // overwrites slot 0 (was 1, now 3)

        // Buffer: slot[0]=3, slot[1]=2. head=3, tail=0.
        // available = 3, cap = 2, so consumer skips to head-cap=1.
        // Reads slot[1]=2, then slot[0]=3.
        assert_eq!(rx.try_pop(), Some(2));
        assert_eq!(rx.try_pop(), Some(3));
        assert!(rx.try_pop().is_none());
    }

    #[test]
    fn drain_last_returns_most_recent() {
        let (mut tx, mut rx) = spsc_channel::<u64>(8);
        tx.try_push(10).unwrap();
        tx.try_push(20).unwrap();
        tx.try_push(30).unwrap();

        assert_eq!(rx.drain_last(), Some(30));
        assert!(rx.try_pop().is_none()); // all consumed
    }

    #[test]
    fn drain_last_empty() {
        let (_tx, mut rx) = spsc_channel::<u64>(4);
        assert!(rx.drain_last().is_none());
    }

    #[test]
    fn drain_last_after_overwrite() {
        let (mut tx, mut rx) = spsc_channel::<u64>(2);
        tx.push_overwrite(1);
        tx.push_overwrite(2);
        tx.push_overwrite(3);
        tx.push_overwrite(4);

        // Should get the most recent value
        assert_eq!(rx.drain_last(), Some(4));
        assert!(rx.try_pop().is_none());
    }

    #[test]
    fn wrap_around() {
        let (mut tx, mut rx) = spsc_channel::<u64>(2);
        for i in 0..10 {
            tx.try_push(i).unwrap();
            assert_eq!(rx.try_pop(), Some(i));
        }
    }

    #[test]
    fn concurrent_push_pop() {
        let (mut tx, mut rx) = spsc_channel::<u64>(1024);
        let n = 100_000u64;

        let producer = std::thread::spawn(move || {
            for i in 0..n {
                while tx.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });

        let consumer = std::thread::spawn(move || {
            let mut next = 0u64;
            while next < n {
                if let Some(val) = rx.try_pop() {
                    assert_eq!(val, next);
                    next += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
    }

    #[test]
    fn power_of_two_rounding() {
        let (mut tx, mut _rx) = spsc_channel::<u64>(3);
        // capacity should be 4 (next power of 2)
        assert!(tx.try_push(1).is_ok());
        assert!(tx.try_push(2).is_ok());
        assert!(tx.try_push(3).is_ok());
        assert!(tx.try_push(4).is_ok());
        assert!(tx.try_push(5).is_err());
    }
}
