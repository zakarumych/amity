//! Provides flipping queue type for message passing between multiple producers and single consumer.
//!
//! The twist is that message sending cannot happen concurrently with message receiving.
//! Therefore queue is flipped to either side - producer or consumer.
//!
//! This implementation uses borrowing for this purpose.
//! Shared reference allows to send messages from multiple threads.
//! Exclusive reference allows to receive messages from single thread.

use core::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, replace, swap},
    sync::atomic::{AtomicU64, Ordering},
};

use lock_api::RawRwLock;

use crate::ring_buffer::{self, RingBuffer, ring_index};

/// Implements ring-buffer data structure.
///
/// It allows pushing values from multiple threads in parallel until capacity is reached.
/// After that it requires exclusive borrow to push more values.
///
/// Reading values always requires exclusive borrow.
///
/// Thus it doesn't require any internal synchronization primitives.
pub struct FlipBuffer<T> {
    buffer: RingBuffer<UnsafeCell<T>>,
    // u64 is used to allow incrementing without overflow.
    // Contains number of elements pushed via `try_push`.
    tail: AtomicU64,
}

unsafe impl<T> Sync for FlipBuffer<T> where T: Send {}
unsafe impl<T> Send for FlipBuffer<T> where T: Send {}

impl<T> FlipBuffer<T> {
    /// Create new ring buffer.
    #[must_use]
    pub fn new() -> Self {
        FlipBuffer {
            buffer: RingBuffer::new(),
            tail: AtomicU64::new(0),
        }
    }

    /// Create new ring buffer.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        FlipBuffer {
            buffer: RingBuffer::with_capacity(cap),
            tail: AtomicU64::new(0),
        }
    }

    /// Flushes tail written by `try_push` to the ring-buffer.
    fn flush_tail(&mut self) {
        let tail = replace(self.tail.get_mut(), 0);

        match usize::try_from(tail) {
            Ok(tail) if tail <= self.buffer.capacity() - self.buffer.len() => unsafe {
                self.buffer.set_len(self.buffer.len() + tail);
            },
            _ => unsafe {
                self.buffer.set_len(self.buffer.capacity());
            },
        }
    }

    /// Clears the buffer, removing all values.
    pub fn clear(&mut self) {
        self.flush_tail();
        self.buffer.clear();
    }

    /// Attempts to push value to the queue.
    ///
    /// If at full capacity this will fail and return the value back.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        // Acquire index to write to.
        let tail = self.tail.fetch_add(1, Ordering::Acquire);
        match usize::try_from(tail) {
            Ok(tail) if tail < self.buffer.capacity() - self.buffer.len() => {
                // Queue is not full.
                let idx = ring_index(
                    self.buffer.head() + self.buffer.len(),
                    tail,
                    self.buffer.capacity(),
                );

                // Safety: We have exclusive access to the queue at this index.
                unsafe {
                    let slot = self.buffer.as_ptr().add(idx);
                    UnsafeCell::raw_get(slot).write(value);
                }
                Ok(())
            }
            _ => {
                // Queue is full.
                Err(value)
            }
        }
    }

    /// Push value to the queue.
    ///
    /// If at full capacity this will grow the queue.
    pub fn push(&mut self, value: T) {
        self.flush_tail();
        self.buffer.push(UnsafeCell::new(value));
    }

    /// Pop value from the queue.
    ///
    /// This requires exclusive borrow.
    pub fn pop(&mut self) -> Option<T> {
        self.flush_tail();
        self.buffer.pop().map(UnsafeCell::into_inner)
    }

    /// Returns iterator over values in the queue.
    ///
    /// This requires exclusive borrow.
    ///
    /// This iterator will consume all values in the queue even if dropped before it is finished.
    pub fn drain(&mut self) -> Drain<'_, T> {
        self.flush_tail();
        Drain {
            inner: self.buffer.drain(),
        }
    }

    pub fn swap_buffer(&mut self, ring: &mut RingBuffer<T>) {
        self.flush_tail();
        swap(ring.as_unsafe_cell_mut(), &mut self.buffer);
    }
}

#[must_use = "iterator does nothing unless consumed"]
pub struct Drain<'a, T> {
    inner: ring_buffer::Drain<'a, UnsafeCell<T>>,
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    #[inline(always)]
    fn next(&mut self) -> Option<T> {
        self.inner.next().map(UnsafeCell::into_inner)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    #[inline(always)]
    fn count(self) -> usize {
        self.inner.count()
    }

    #[inline(always)]
    fn nth(&mut self, n: usize) -> Option<T> {
        self.inner.nth(n).map(UnsafeCell::into_inner)
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {
    #[inline(always)]
    fn len(&self) -> usize {
        self.inner.len()
    }
}

/// Implements flexible flip-queue data structure.
/// This data structure allows either push values into the queue or pop them from it
/// like a regular queue.
/// The advantage `FlipQueue` is that it allows to push values from multiple threads in parallel.
/// But only when no values are popped from the queue.
///
/// Pushing requires shared borrow and popping requires exclusive borrow. This guarantees that above
/// requirement is met.
///
/// When queue is at full capacity it is grown by doubling its size.
/// Pushing thread will have to lock the queue to grow it.
///
/// When capacity is enough to push all values, then pushing is guaranteed to be wait-free.
/// Popping and draining is wait-free since it uses exclusive borrow.
///
/// Typical use case of flip-queue is to broadcast `&FlipQueue` to multiple tasks and collect values,
/// and then after all tasks are finished drain the queue to process collected values.
pub struct FlipQueue<T, L = crate::DefaultRawRwLock> {
    flip_buffer: UnsafeCell<FlipBuffer<T>>,
    rw_lock: L,
}

unsafe impl<T, L> Send for FlipQueue<T, L>
where
    T: Send,
    L: Send,
{
}

unsafe impl<T, L> Sync for FlipQueue<T, L>
where
    T: Send,
    L: Sync,
{
}

impl<T, L> Default for FlipQueue<T, L>
where
    L: RawRwLock,
{
    fn default() -> Self {
        FlipQueue::new()
    }
}

impl<T, L> FlipQueue<T, L> {
    /// Clears the queue, removing all values.
    pub fn clear(&mut self) {
        self.flip_buffer.get_mut().clear();
    }
}

impl<T, L> FlipQueue<T, L>
where
    L: RawRwLock,
{
    /// Create new flip queue.
    #[must_use]
    pub fn new() -> Self {
        FlipQueue {
            flip_buffer: UnsafeCell::new(FlipBuffer::new()),
            rw_lock: L::INIT,
        }
    }

    /// Create new flip queue.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        FlipQueue {
            flip_buffer: UnsafeCell::new(FlipBuffer::with_capacity(cap)),
            rw_lock: L::INIT,
        }
    }

    /// Tries to push value to the queue.
    ///
    /// If at full capacity this will return the value back.
    ///
    /// This function may block if call to `push` grows the queue.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        self.rw_lock.lock_shared();
        let _defer = defer(|| unsafe { self.rw_lock.unlock_shared() });
        // Safety: Shared lock acquired.
        unsafe { (&*self.flip_buffer.get()).try_push(value) }
    }

    /// Push value to the queue.
    ///
    /// If at full capacity this will lock the queue and grow it.
    /// All pushes that need not to grow the queue are executed in parallel.
    pub fn push(&self, value: T) {
        if let Err(value) = self.try_push(value) {
            self.push_slow(value);
        }
    }

    // Do not inline this cold function.
    #[inline(never)]
    #[cold]
    fn push_slow(&self, value: T) {
        self.rw_lock.lock_exclusive();
        let _defer = defer(|| unsafe { self.rw_lock.unlock_exclusive() });
        // Safety: Exclusive lock acquired.
        unsafe {
            let flip_buffer = &mut *self.flip_buffer.get();
            flip_buffer.push(value);
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.flip_buffer.get_mut().pop()
    }

    pub fn drain(&mut self) -> Drain<'_, T> {
        self.flip_buffer.get_mut().drain()
    }

    pub fn drain_locking<R>(&self, f: impl FnOnce(Drain<T>) -> R) -> R {
        self.rw_lock.lock_exclusive();
        let _defer = defer(|| unsafe { self.rw_lock.unlock_exclusive() });
        unsafe {
            let flip_buffer = &mut *self.flip_buffer.get();
            f(flip_buffer.drain())
        }
    }

    /// Lock the queue and swap its buffer with provided one.
    ///
    /// This is preferred to draining it with locking if iteration can take significant time.
    pub fn swap_buffer(&self, ring: &mut RingBuffer<T>) {
        self.rw_lock.lock_exclusive();
        let _defer = defer(|| unsafe { self.rw_lock.unlock_exclusive() });

        unsafe {
            let flip_buffer = &mut *self.flip_buffer.get();
            flip_buffer.swap_buffer(ring);
        }
    }
}

#[test]
#[cfg(feature = "std")]
fn test_flib_buffer() {
    let mut ring_buffer = FlipBuffer::with_capacity(256);

    std::thread::scope(|scope| {
        let ring_buffer = &ring_buffer;
        for i in 0..10 {
            scope.spawn(move || {
                for j in 0..10 {
                    ring_buffer.try_push(i * 10 + j).unwrap();
                }
            });
        }
    });

    let mut idx = ring_buffer.drain().collect::<Vec<_>>();
    idx.sort();

    assert_eq!(idx, (0..100).collect::<Vec<_>>());
}

#[test]
#[cfg(feature = "std")]
fn test_flib_queue() {
    let mut queue = FlipQueue::<_>::with_capacity(1);

    std::thread::scope(|scope| {
        let queue = &queue;
        for i in 0..10 {
            scope.spawn(move || {
                for j in 0..10 {
                    queue.push(i * 10 + j);
                }
            });
        }
    });

    let mut idx = queue.drain().collect::<Vec<_>>();
    idx.sort();

    assert_eq!(idx, (0..100).collect::<Vec<_>>());
}

struct Defer<F: FnOnce()>(ManuallyDrop<F>);

impl<F> Drop for Defer<F>
where
    F: FnOnce(),
{
    fn drop(&mut self) {
        unsafe {
            let f = ManuallyDrop::take(&mut self.0);
            f();
        }
    }
}

fn defer<F>(f: F) -> Defer<F>
where
    F: FnOnce(),
{
    Defer(ManuallyDrop::new(f))
}
