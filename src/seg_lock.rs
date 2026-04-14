use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

struct SegLockInner<T> {
    seq: AtomicU32,
    data: UnsafeCell<T>,
}

// Safety: single-writer, multi-reader discipline enforced by API.
// AtomicU32 fences ensure visibility of data writes.
unsafe impl<T: Copy + Send> Sync for SegLockInner<T> {}
unsafe impl<T: Copy + Send> Send for SegLockInner<T> {}

pub struct SegLockWriter<T: Copy> {
    inner: Arc<SegLockInner<T>>,
}

pub struct SegLockReader<T: Copy> {
    inner: Arc<SegLockInner<T>>,
}

unsafe impl<T: Copy + Send> Send for SegLockWriter<T> {}
unsafe impl<T: Copy + Send> Send for SegLockReader<T> {}

/// Create a seqlock with an initial value.
/// Returns (writer, reader). The reader can be cloned for multiple consumers.
pub fn seg_lock<T: Copy>(initial: T) -> (SegLockWriter<T>, SegLockReader<T>) {
    let inner = Arc::new(SegLockInner {
        seq: AtomicU32::new(0),
        data: UnsafeCell::new(initial),
    });
    (
        SegLockWriter {
            inner: Arc::clone(&inner),
        },
        SegLockReader { inner },
    )
}

impl<T: Copy> SegLockWriter<T> {
    /// Write a new value. Only one writer should exist.
    pub fn write(&self, val: T) {
        let s = self.inner.seq.load(Ordering::Relaxed);
        // Mark write in progress (odd sequence)
        self.inner.seq.store(s.wrapping_add(1), Ordering::Release);
        // Safety: we are the sole writer
        unsafe {
            self.inner.data.get().write(val);
        }
        // Mark write complete (even sequence)
        self.inner.seq.store(s.wrapping_add(2), Ordering::Release);
    }
}

impl<T: Copy> SegLockReader<T> {
    /// Read the current value. Retries if a write is in progress.
    pub fn read(&self) -> T {
        loop {
            let s1 = self.inner.seq.load(Ordering::Acquire);
            if s1 & 1 != 0 {
                // Write in progress, spin
                std::hint::spin_loop();
                continue;
            }
            // Safety: data is Copy and we check the sequence number for torn reads.
            let val = unsafe { *self.inner.data.get() };
            let s2 = self.inner.seq.load(Ordering::Acquire);
            if s1 == s2 {
                return val;
            }
            // Sequence changed during read, retry
            std::hint::spin_loop();
        }
    }
}

impl<T: Copy> Clone for SegLockReader<T> {
    fn clone(&self) -> Self {
        SegLockReader {
            inner: Arc::clone(&self.inner),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_read_write() {
        let (writer, reader) = seg_lock(0u64);
        assert_eq!(reader.read(), 0);

        writer.write(42);
        assert_eq!(reader.read(), 42);

        writer.write(100);
        assert_eq!(reader.read(), 100);
    }

    #[test]
    fn multiple_readers() {
        let (writer, reader1) = seg_lock(0u64);
        let reader2 = reader1.clone();

        writer.write(99);
        assert_eq!(reader1.read(), 99);
        assert_eq!(reader2.read(), 99);
    }

    #[test]
    fn concurrent_read_write() {
        #[derive(Copy, Clone, PartialEq, Debug)]
        struct Data {
            a: u64,
            b: u64,
            c: u64,
        }

        let initial = Data { a: 0, b: 0, c: 0 };
        let (writer, reader) = seg_lock(initial);

        let write_thread = std::thread::spawn(move || {
            for i in 0..100_000u64 {
                writer.write(Data { a: i, b: i, c: i });
            }
        });

        let read_thread = std::thread::spawn(move || {
            for _ in 0..200_000 {
                let data = reader.read();
                // All fields must be consistent (same write)
                assert_eq!(data.a, data.b, "torn read: a={} b={}", data.a, data.b);
                assert_eq!(data.b, data.c, "torn read: b={} c={}", data.b, data.c);
            }
        });

        write_thread.join().unwrap();
        read_thread.join().unwrap();
    }
}
