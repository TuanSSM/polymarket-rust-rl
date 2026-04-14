use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::types::CachePadded;

/// Lock-free Single-Producer Single-Consumer ring buffer.
///
/// Zero-allocation after construction. Power-of-2 capacity for
/// branchless index wrapping via bitmask.
pub struct SpscRingBuffer<T> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    head: CachePadded<AtomicUsize>,
    tail: CachePadded<AtomicUsize>,
}

unsafe impl<T: Send> Send for SpscRingBuffer<T> {}
unsafe impl<T: Send> Sync for SpscRingBuffer<T> {}

/// Producer handle. Only one may exist per ring buffer.
pub struct Producer<'a, T> {
    rb: &'a SpscRingBuffer<T>,
}

/// Consumer handle. Only one may exist per ring buffer.
pub struct Consumer<'a, T> {
    rb: &'a SpscRingBuffer<T>,
}

// Producer/Consumer are !Sync — only one thread may use each handle.
unsafe impl<'a, T: Send> Send for Producer<'a, T> {}
unsafe impl<'a, T: Send> Send for Consumer<'a, T> {}

impl<T: Copy> SpscRingBuffer<T> {
    /// Create a new ring buffer. `capacity` is rounded up to next power of 2.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2).next_power_of_two();
        let buffer: Vec<UnsafeCell<MaybeUninit<T>>> = (0..capacity)
            .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
            .collect();
        Self {
            buffer: buffer.into_boxed_slice(),
            mask: capacity - 1,
            head: CachePadded(AtomicUsize::new(0)),
            tail: CachePadded(AtomicUsize::new(0)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// Split into producer/consumer pair.
    pub fn split(&self) -> (Producer<'_, T>, Consumer<'_, T>) {
        (Producer { rb: self }, Consumer { rb: self })
    }
}

impl<'a, T: Copy> Producer<'a, T> {
    /// Try to push a value. Returns `Err(value)` if full.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        let head = self.rb.head.load(Ordering::Relaxed);
        let tail = self.rb.tail.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= self.rb.capacity() {
            return Err(value);
        }
        let slot = head & self.rb.mask;
        unsafe {
            (*self.rb.buffer[slot].get()).write(value);
        }
        self.rb.head.store(head.wrapping_add(1), Ordering::Release);
        Ok(())
    }

    pub fn len(&self) -> usize {
        let head = self.rb.head.load(Ordering::Relaxed);
        let tail = self.rb.tail.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn is_full(&self) -> bool {
        self.len() >= self.rb.capacity()
    }
}

impl<'a, T: Copy> Consumer<'a, T> {
    /// Try to pop a value. Returns `None` if empty.
    pub fn try_pop(&self) -> Option<T> {
        let tail = self.rb.tail.load(Ordering::Relaxed);
        let head = self.rb.head.load(Ordering::Acquire);
        if tail == head {
            return None;
        }
        let slot = tail & self.rb.mask;
        let value = unsafe { (*self.rb.buffer[slot].get()).assume_init_read() };
        self.rb.tail.store(tail.wrapping_add(1), Ordering::Release);
        Some(value)
    }

    /// Drain up to `max` items into the provided vec.
    pub fn drain_into(&self, dst: &mut Vec<T>, max: usize) -> usize {
        let mut count = 0;
        while count < max {
            match self.try_pop() {
                Some(v) => {
                    dst.push(v);
                    count += 1;
                }
                None => break,
            }
        }
        count
    }

    pub fn len(&self) -> usize {
        let tail = self.rb.tail.load(Ordering::Relaxed);
        let head = self.rb.head.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn new_creates_power_of_two() {
        let rb = SpscRingBuffer::<u64>::new(16);
        assert_eq!(rb.capacity(), 16);
    }

    #[test]
    fn new_rounds_up_capacity() {
        let rb = SpscRingBuffer::<u64>::new(13);
        assert_eq!(rb.capacity(), 16);
    }

    #[test]
    fn new_minimum_capacity() {
        let rb = SpscRingBuffer::<u64>::new(1);
        assert_eq!(rb.capacity(), 2);
    }

    #[test]
    fn push_pop_single() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, cons) = rb.split();
        prod.try_push(42).unwrap();
        assert_eq!(cons.try_pop(), Some(42));
    }

    #[test]
    fn push_pop_multiple() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, cons) = rb.split();
        for i in 0..5 {
            prod.try_push(i).unwrap();
        }
        for i in 0..5 {
            assert_eq!(cons.try_pop(), Some(i));
        }
    }

    #[test]
    fn empty_pop_returns_none() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (_prod, cons) = rb.split();
        assert_eq!(cons.try_pop(), None);
    }

    #[test]
    fn full_push_returns_err() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, _cons) = rb.split();
        for i in 0..4 {
            prod.try_push(i).unwrap();
        }
        assert_eq!(prod.try_push(99), Err(99));
    }

    #[test]
    fn fill_and_drain() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, cons) = rb.split();
        for i in 0..8 {
            prod.try_push(i).unwrap();
        }
        assert!(prod.is_full());
        for i in 0..8 {
            assert_eq!(cons.try_pop(), Some(i));
        }
        assert!(cons.is_empty());
    }

    #[test]
    fn alternating_push_pop() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, cons) = rb.split();
        for i in 0..100 {
            prod.try_push(i).unwrap();
            assert_eq!(cons.try_pop(), Some(i));
        }
    }

    #[test]
    fn fifo_ordering() {
        let rb = SpscRingBuffer::<u64>::new(16);
        let (prod, cons) = rb.split();
        let values: Vec<u64> = (0..16).collect();
        for &v in &values {
            prod.try_push(v).unwrap();
        }
        for &expected in &values {
            assert_eq!(cons.try_pop(), Some(expected));
        }
    }

    #[test]
    fn capacity_boundary() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, cons) = rb.split();
        // Fill to capacity
        for i in 0..4 {
            prod.try_push(i).unwrap();
        }
        assert!(prod.try_push(99).is_err());
        // Drain one, push one
        cons.try_pop().unwrap();
        prod.try_push(99).unwrap();
    }

    #[test]
    fn wrapping_behavior() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, cons) = rb.split();
        // Push and pop many times to force index wrapping
        for round in 0..50 {
            for j in 0..3 {
                prod.try_push(round * 3 + j).unwrap();
            }
            for j in 0..3 {
                assert_eq!(cons.try_pop(), Some(round * 3 + j));
            }
        }
    }

    #[test]
    fn drain_into_basic() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, cons) = rb.split();
        for i in 0..5 {
            prod.try_push(i).unwrap();
        }
        let mut buf = Vec::new();
        let count = cons.drain_into(&mut buf, 10);
        assert_eq!(count, 5);
        assert_eq!(buf, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn drain_into_max_limit() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, cons) = rb.split();
        for i in 0..5 {
            prod.try_push(i).unwrap();
        }
        let mut buf = Vec::new();
        let count = cons.drain_into(&mut buf, 3);
        assert_eq!(count, 3);
        assert_eq!(buf, vec![0, 1, 2]);
    }

    #[test]
    fn drain_into_empty() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (_prod, cons) = rb.split();
        let mut buf = Vec::new();
        let count = cons.drain_into(&mut buf, 10);
        assert_eq!(count, 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn producer_len() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, _cons) = rb.split();
        assert_eq!(prod.len(), 0);
        prod.try_push(1).unwrap();
        assert_eq!(prod.len(), 1);
        prod.try_push(2).unwrap();
        assert_eq!(prod.len(), 2);
    }

    #[test]
    fn consumer_len() {
        let rb = SpscRingBuffer::<u64>::new(8);
        let (prod, cons) = rb.split();
        prod.try_push(1).unwrap();
        prod.try_push(2).unwrap();
        assert_eq!(cons.len(), 2);
        cons.try_pop();
        assert_eq!(cons.len(), 1);
    }

    #[test]
    fn producer_is_empty() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, _cons) = rb.split();
        assert!(prod.is_empty());
        prod.try_push(1).unwrap();
        assert!(!prod.is_empty());
    }

    #[test]
    fn producer_is_full() {
        let rb = SpscRingBuffer::<u64>::new(4);
        let (prod, _cons) = rb.split();
        assert!(!prod.is_full());
        for i in 0..4 {
            prod.try_push(i).unwrap();
        }
        assert!(prod.is_full());
    }

    #[test]
    fn cross_thread_basic() {
        let rb = Arc::new(SpscRingBuffer::<u64>::new(64));
        let rb2 = rb.clone();

        // SAFETY: we guarantee single-producer, single-consumer
        let (prod, cons) = rb.split();
        let prod_ptr = &prod as *const Producer<'_, u64> as usize;
        let cons_ptr = &cons as *const Consumer<'_, u64> as usize;

        let handle = thread::spawn(move || {
            let prod = unsafe { &*(prod_ptr as *const Producer<'_, u64>) };
            for i in 0..32u64 {
                while prod.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });

        let mut received = Vec::new();
        while received.len() < 32 {
            let cons = unsafe { &*(cons_ptr as *const Consumer<'_, u64>) };
            if let Some(v) = cons.try_pop() {
                received.push(v);
            }
        }

        handle.join().unwrap();
        let _ = (rb2, prod, cons); // prevent drops before threads finish
        assert_eq!(received, (0..32).collect::<Vec<_>>());
    }

    #[test]
    fn cross_thread_fifo_ordering() {
        let rb = Arc::new(SpscRingBuffer::<u64>::new(128));
        let rb2 = rb.clone();

        let (prod, cons) = rb.split();
        let prod_ptr = &prod as *const Producer<'_, u64> as usize;
        let cons_ptr = &cons as *const Consumer<'_, u64> as usize;

        let n = 1000u64;

        let writer = thread::spawn(move || {
            let prod = unsafe { &*(prod_ptr as *const Producer<'_, u64>) };
            for i in 0..n {
                while prod.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });

        let reader = thread::spawn(move || {
            let cons = unsafe { &*(cons_ptr as *const Consumer<'_, u64>) };
            let mut received = Vec::with_capacity(n as usize);
            while received.len() < n as usize {
                if let Some(v) = cons.try_pop() {
                    received.push(v);
                }
            }
            received
        });

        writer.join().unwrap();
        let received = reader.join().unwrap();
        let _ = (rb2, prod, cons);

        // Verify strict FIFO ordering
        for (i, &v) in received.iter().enumerate() {
            assert_eq!(v, i as u64, "FIFO violation at index {i}");
        }
    }

    #[test]
    fn cross_thread_high_throughput() {
        let rb = Arc::new(SpscRingBuffer::<u64>::new(256));
        let rb2 = rb.clone();

        let (prod, cons) = rb.split();
        let prod_ptr = &prod as *const Producer<'_, u64> as usize;
        let cons_ptr = &cons as *const Consumer<'_, u64> as usize;

        let n = 10_000u64;

        let writer = thread::spawn(move || {
            let prod = unsafe { &*(prod_ptr as *const Producer<'_, u64>) };
            for i in 0..n {
                while prod.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });

        let reader = thread::spawn(move || {
            let cons = unsafe { &*(cons_ptr as *const Consumer<'_, u64>) };
            let mut count = 0u64;
            let mut last = None;
            while count < n {
                if let Some(v) = cons.try_pop() {
                    if let Some(prev) = last {
                        assert!(v > prev, "ordering violation");
                    }
                    last = Some(v);
                    count += 1;
                }
            }
            count
        });

        writer.join().unwrap();
        let count = reader.join().unwrap();
        let _ = (rb2, prod, cons);
        assert_eq!(count, n);
    }

    #[test]
    fn cross_thread_concurrent_consistency() {
        let rb = Arc::new(SpscRingBuffer::<u64>::new(64));
        let rb2 = rb.clone();

        let (prod, cons) = rb.split();
        let prod_ptr = &prod as *const Producer<'_, u64> as usize;
        let cons_ptr = &cons as *const Consumer<'_, u64> as usize;

        let n = 5_000u64;

        let writer = thread::spawn(move || {
            let prod = unsafe { &*(prod_ptr as *const Producer<'_, u64>) };
            let mut pushed = 0u64;
            for i in 0..n {
                while prod.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
                pushed += 1;
            }
            pushed
        });

        let reader = thread::spawn(move || {
            let cons = unsafe { &*(cons_ptr as *const Consumer<'_, u64>) };
            let mut sum = 0u64;
            let mut count = 0u64;
            while count < n {
                if let Some(v) = cons.try_pop() {
                    sum += v;
                    count += 1;
                }
            }
            (count, sum)
        });

        let pushed = writer.join().unwrap();
        let (popped, sum) = reader.join().unwrap();
        let _ = (rb2, prod, cons);
        assert_eq!(pushed, n);
        assert_eq!(popped, n);
        assert_eq!(sum, n * (n - 1) / 2);
    }
}
