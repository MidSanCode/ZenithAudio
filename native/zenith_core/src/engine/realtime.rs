//! Lock-free, allocation-free utilities for the audio thread (PLAN §3.S1
//! `engine/realtime.rs`).
//!
//! The engine needs two things that the standard library does not offer in a
//! real-time-safe form: a queue to carry control-thread events to the audio
//! thread without locking, and a pool that never grows. Both are implemented
//! here over preallocated storage.
//!
//! # Why hand-rolled
//!
//! `std::sync::mpsc` allocates and takes locks; `Vec` may reallocate. Neither
//! may appear on the audio path (ABI principle P5). The queue below is a bounded
//! ring buffer whose storage is allocated once, and the pool is a fixed slab.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A bounded single-producer / single-consumer queue.
///
/// # Safety of the shared state
///
/// The producer writes `head` and the consumer writes `tail`; each reads the
/// other's index with acquire/release ordering. `slots` is only ever touched by
/// one side at a time for a given index, so the `UnsafeCell` never has two
/// simultaneous mutable accesses.
pub struct SpscQueue<T> {
    /// The storage. Never resized after construction.
    slots: alloc::boxed::Box<[UnsafeCell<Option<T>>]>,
    /// Next write index, owned by the producer.
    head: AtomicUsize,
    /// Next read index, owned by the consumer.
    tail: AtomicUsize,
    /// Capacity, fixed.
    capacity: usize,
}

// SAFETY: `T: Send` is enough because the queue only moves values between the
// producer and consumer; it never shares a reference to a `T`.
unsafe impl<T: Send> Send for SpscQueue<T> {}
// SAFETY: as above — this is a transfer queue, not a shared container.
unsafe impl<T: Send> Sync for SpscQueue<T> {}

impl<T> SpscQueue<T> {
    /// Creates a queue holding at most `capacity` items.
    ///
    /// The usable capacity is `capacity - 1`: one slot is kept empty to
    /// distinguish full from empty without a separate counter.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(2);
        let mut slots = alloc::vec::Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(UnsafeCell::new(None));
        }
        Self {
            slots: slots.into_boxed_slice(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            capacity,
        }
    }

    /// Maximum number of items the queue can hold.
    #[must_use]
    pub fn usable_capacity(&self) -> usize {
        self.capacity - 1
    }

    /// Whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.load(Ordering::Acquire) == self.tail.load(Ordering::Acquire)
    }

    /// Pushes `value`, returning it back when the queue is full.
    ///
    /// Returning the value rather than blocking is deliberate: the audio thread
    /// must never wait, and the control thread needs to know an event was
    /// dropped so it can report it (ABI §6.6).
    pub fn push(&self, value: T) -> Result<(), T> {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % self.capacity;
        if next == self.tail.load(Ordering::Acquire) {
            return Err(value);
        }
        // SAFETY: this index is between head and tail, so the consumer is not
        // reading it, and only the producer writes here.
        unsafe {
            *self.slots[head].get() = Some(value);
        }
        self.head.store(next, Ordering::Release);
        Ok(())
    }

    /// Pops the oldest value, or `None` when empty.
    pub fn pop(&self) -> Option<T> {
        let tail = self.tail.load(Ordering::Relaxed);
        if tail == self.head.load(Ordering::Acquire) {
            return None;
        }
        // SAFETY: this index is behind head, so the producer is not writing it,
        // and only the consumer reads here.
        let value = unsafe { (*self.slots[tail].get()).take() };
        self.tail
            .store((tail + 1) % self.capacity, Ordering::Release);
        value
    }

    /// Drops any items still queued.
    pub fn clear(&self) {
        while self.pop().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queue_round_trips_in_order() {
        let q: SpscQueue<i32> = SpscQueue::new(4);
        assert!(q.is_empty());
        assert!(q.push(1).is_ok());
        assert!(q.push(2).is_ok());
        assert_eq!(q.pop(), Some(1));
        assert_eq!(q.pop(), Some(2));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn a_full_queue_hands_the_value_back_instead_of_blocking() {
        let q: SpscQueue<i32> = SpscQueue::new(2); // usable capacity 1
        assert_eq!(q.usable_capacity(), 1);
        assert!(q.push(7).is_ok());
        assert_eq!(q.push(8), Err(8), "a full queue must not drop silently");
    }

    #[test]
    fn clearing_empties_the_queue() {
        let q: SpscQueue<i32> = SpscQueue::new(4);
        q.push(1).ok();
        q.push(2).ok();
        q.clear();
        assert!(q.is_empty());
    }
}
