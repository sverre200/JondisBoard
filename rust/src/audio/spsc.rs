//! A fixed-size single-producer / single-consumer queue for handing values to and from the audio
//! threads without locks or allocation. The real-time side only ever does wait-free pushes and
//! pops.
//!
//! This is the one place in the audio code with `unsafe`: each slot is written by the producer
//! and read by the consumer, and the head/tail atomics (Release on publish, Acquire on observe)
//! make sure a slot is never touched by both at once.

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Ring<T> {
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
    mask: usize,
    /// next slot the consumer reads (only the consumer stores it)
    head: AtomicUsize,
    /// next slot the producer writes (only the producer stores it)
    tail: AtomicUsize,
}

// SAFETY: a slot is accessed by exactly one side at a time, as handed over by head/tail with
// Release/Acquire ordering, so sharing the ring between the two threads is sound when T: Send.
unsafe impl<T: Send> Sync for Ring<T> {}
unsafe impl<T: Send> Send for Ring<T> {}

impl<T> Drop for Ring<T> {
    fn drop(&mut self) {
        let mut h = *self.head.get_mut();
        let t = *self.tail.get_mut();
        while h != t {
            // SAFETY: slots between head and tail hold initialised values nobody else can reach
            // (we have &mut self).
            unsafe { (*self.slots[h & self.mask].get()).assume_init_drop() };
            h = h.wrapping_add(1);
        }
    }
}

pub struct Producer<T> {
    ring: Arc<Ring<T>>,
}

pub struct Consumer<T> {
    ring: Arc<Ring<T>>,
}

/// A queue holding up to `capacity` (rounded up to a power of two) values.
pub fn channel<T: Send>(capacity: usize) -> (Producer<T>, Consumer<T>) {
    let cap = capacity.max(2).next_power_of_two();
    let slots = (0..cap).map(|_| UnsafeCell::new(MaybeUninit::uninit())).collect();
    let ring = Arc::new(Ring { slots, mask: cap - 1, head: AtomicUsize::new(0), tail: AtomicUsize::new(0) });
    (Producer { ring: ring.clone() }, Consumer { ring })
}

impl<T: Send> Producer<T> {
    /// Add a value; gives it back if the queue is full.
    pub fn push(&mut self, v: T) -> Result<(), T> {
        let r = &*self.ring;
        let t = r.tail.load(Ordering::Relaxed);
        if t.wrapping_sub(r.head.load(Ordering::Acquire)) > r.mask {
            return Err(v);
        }
        // SAFETY: slot t is outside head..tail, so the consumer isn't reading it, and only this
        // (unique, non-Clone) producer writes slots.
        unsafe { (*r.slots[t & r.mask].get()).write(v) };
        r.tail.store(t.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T: Send> Consumer<T> {
    pub fn pop(&mut self) -> Option<T> {
        let r = &*self.ring;
        let h = r.head.load(Ordering::Relaxed);
        if h == r.tail.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: slot h is inside head..tail, written and published by the producer (Acquire
        // above), and only this unique consumer reads it before moving head past it.
        let v = unsafe { (*r.slots[h & r.mask].get()).assume_init_read() };
        r.head.store(h.wrapping_add(1), Ordering::Release);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_and_full() {
        let (mut p, mut c) = channel::<u32>(4);
        for i in 0..4 {
            p.push(i).unwrap();
        }
        assert_eq!(p.push(9), Err(9));
        assert_eq!(c.pop(), Some(0));
        p.push(4).unwrap();
        assert_eq!((1..=4).map(|_| c.pop().unwrap()).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert_eq!(c.pop(), None);
    }

    #[test]
    fn drops_leftovers_and_works_across_threads() {
        let probe = Arc::new(());
        {
            let (mut p, _c) = channel::<Arc<()>>(8);
            p.push(probe.clone()).unwrap();
            assert_eq!(Arc::strong_count(&probe), 2);
        }
        assert_eq!(Arc::strong_count(&probe), 1);

        let (mut p, mut c) = channel::<u64>(64);
        let t = std::thread::spawn(move || {
            let mut sum = 0u64;
            let mut got = 0;
            while got < 100_000 {
                if let Some(v) = c.pop() {
                    sum += v;
                    got += 1;
                }
            }
            sum
        });
        let mut i = 0u64;
        while i < 100_000 {
            if p.push(i).is_ok() {
                i += 1;
            }
        }
        assert_eq!(t.join().unwrap(), (0..100_000u64).sum::<u64>());
    }
}
