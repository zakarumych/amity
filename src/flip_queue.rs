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
    mem::{replace, swap},
    sync::atomic::{AtomicU64, Ordering},
};

use lock_api::{RawRwLock, RwLock};

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

    /// Number of elements popped via `pop_sync`.
    /// u64 is used to allow incrementing without overflow.
    popped: AtomicU64,

    /// Number of elements pushed via `push_sync`.
    /// u64 is used to allow incrementing without overflow.
    pushed: AtomicU64,
}

unsafe impl<T> Sync for FlipBuffer<T> where T: Send {}
unsafe impl<T> Send for FlipBuffer<T> where T: Send {}

impl<T> FlipBuffer<T> {
    /// Create new ring buffer.
    #[must_use]
    pub fn new() -> Self {
        FlipBuffer {
            buffer: RingBuffer::new(),
            popped: AtomicU64::new(0),
            pushed: AtomicU64::new(0),
        }
    }

    /// Create new ring buffer.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        FlipBuffer {
            buffer: RingBuffer::with_capacity(cap),
            popped: AtomicU64::new(0),
            pushed: AtomicU64::new(0),
        }
    }

    /// Flushes atomic pops and pushes.
    fn flush(&mut self) {
        // Number of elements that could be popped
        let len = self.buffer.len();

        // and pushed
        let vacant = self.buffer.capacity() - len;

        let popped = replace(self.popped.get_mut(), 0);
        let pushed = replace(self.pushed.get_mut(), 0);

        if popped == 0 && pushed == 0 {
            return;
        }

        let popped = match usize::try_from(popped) {
            Ok(popped) if popped <= len => popped,
            _ => len,
        };

        let pushed = match usize::try_from(pushed) {
            Ok(pushed) if pushed <= vacant => pushed,
            _ => vacant,
        };

        let new_len = len - popped + pushed;
        let new_head = ring_index(self.buffer.head(), popped, self.buffer.capacity());

        unsafe {
            self.buffer.set_head(new_head);
            self.buffer.set_len(new_len);
        }
    }

    /// Clears the buffer, removing all values.
    pub fn clear(&mut self) {
        self.flush();
        self.buffer.clear();
    }

    /// Attempts to push value to the queue.
    ///
    /// If at full capacity this will fail and return the value back.
    pub fn push_sync(&self, value: T) -> Result<(), T> {
        // Acquire index to write to.
        let pushed = self.pushed.fetch_add(1, Ordering::Acquire);
        match usize::try_from(pushed) {
            Ok(pushed) if pushed < self.buffer.capacity() - self.buffer.len() => {
                // Queue is not full.
                let idx = ring_index(
                    self.buffer.head(),
                    self.buffer.len() + pushed,
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
        self.flush();
        self.buffer.push(UnsafeCell::new(value));
    }

    /// Attempts to pop value from the queue.
    ///
    /// If empty this will fail and return `None`.
    ///
    /// Note that values pushed via `push_sync` and not yet flushed do not affect emptiness of the queue.
    pub fn pop_sync(&self) -> Option<T> {
        // Acquire index to write to.
        let popped: u64 = self.popped.fetch_add(1, Ordering::Acquire);
        match usize::try_from(popped) {
            Ok(popped) if popped < self.buffer.len() => {
                // Queue is not full.
                let idx = ring_index(self.buffer.head(), popped, self.buffer.capacity());

                // Safety: We have exclusive access to the queue at this index.
                let value = unsafe {
                    let slot = self.buffer.as_ptr().add(idx);
                    UnsafeCell::raw_get(slot).read()
                };
                Some(value)
            }
            _ => {
                // Queue is empty.
                None
            }
        }
    }

    /// Pop value from the queue.
    ///
    /// This requires exclusive borrow.
    pub fn pop(&mut self) -> Option<T> {
        self.flush();
        self.buffer.pop().map(UnsafeCell::into_inner)
    }

    /// Returns iterator over values in the queue.
    ///
    /// This requires exclusive borrow.
    ///
    /// This iterator will consume all values in the queue even if dropped before it is finished.
    pub fn drain(&mut self) -> Drain<'_, T> {
        self.flush();
        Drain {
            inner: self.buffer.drain(),
        }
    }

    pub fn swap_buffer(&mut self, ring: &mut RingBuffer<T>) {
        self.flush();
        swap(ring.as_unsafe_cell_mut(), &mut self.buffer);
    }
}

#[must_use = "iterator does nothing unless consumed"]
pub struct Drain<'a, T> {
    inner: ring_buffer::Drain<'a, UnsafeCell<T>>,
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    #[inline]
    fn next(&mut self) -> Option<T> {
        self.inner.next().map(UnsafeCell::into_inner)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }

    #[inline]
    fn count(self) -> usize {
        self.inner.count()
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<T> {
        self.inner.nth(n).map(UnsafeCell::into_inner)
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {
    #[inline]
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
    buffer: lock_api::RwLock<L, FlipBuffer<T>>,
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

impl<T, L> FlipQueue<T, L>
where
    L: RawRwLock,
{
    /// Clears the queue, removing all values.
    pub fn clear(&mut self) {
        self.buffer.get_mut().clear();
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
            buffer: RwLock::new(FlipBuffer::new()),
        }
    }

    /// Create new flip queue.
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        FlipQueue {
            buffer: RwLock::new(FlipBuffer::with_capacity(cap)),
        }
    }

    /// Tries to push value to the queue.
    ///
    /// If at full capacity this will return the value back.
    ///
    /// This function may block if concurrent call to `push_sync` grows the queue.
    pub fn try_push_sync(&self, value: T) -> Result<(), T> {
        let read = self.buffer.read();
        // Safety: Shared lock acquired.
        read.push_sync(value)
    }

    /// Push value to the queue.
    ///
    /// If at full capacity this will lock the queue and grow it.
    /// Pushes that need not to grow the queue are executed in parallel.
    pub fn push_sync(&self, value: T) {
        if let Err(value) = self.try_push_sync(value) {
            self.push_slow(value);
        }
    }

    // Do not inline this cold function.
    #[inline(never)]
    #[cold]
    fn push_slow(&self, value: T) {
        let mut write = self.buffer.write();
        write.push(value);
    }

    pub fn push(&mut self, value: T) {
        self.buffer.get_mut().push(value);
    }

    /// Tries to pop value from the queue.
    pub fn try_pop_sync(&self) -> Option<T> {
        let read = self.buffer.read();
        read.pop_sync()
    }

    pub fn pop_sync(&self) -> Option<T> {
        {
            let read = self.buffer.read();

            if let Some(value) = read.pop_sync() {
                return Some(value);
            }

            // This is just an optimization to avoid locking the queue if there were no pushed values.
            if read.pushed.load(Ordering::Relaxed) == 0 {
                return None;
            }
        }
        self.pop_slow()
    }

    fn pop_slow(&self) -> Option<T> {
        let mut write = self.buffer.write();
        write.pop()
    }

    pub fn pop(&mut self) -> Option<T> {
        self.buffer.get_mut().pop()
    }

    pub fn drain(&mut self) -> Drain<'_, T> {
        self.buffer.get_mut().drain()
    }

    pub fn drain_locking<R>(&self, f: impl FnOnce(Drain<T>) -> R) -> R {
        let mut write = self.buffer.write();
        f(write.drain())
    }

    /// Lock the queue and swap its buffer with provided one.
    ///
    /// This is preferred to draining it with locking if iteration can take significant time.
    pub fn swap_buffer(&self, ring: &mut RingBuffer<T>) {
        let mut write = self.buffer.write();
        write.swap_buffer(ring);
    }
}

#[test]
#[cfg(feature = "std")]
fn test_flib_buffer() {
    let mut flip_buffer = FlipBuffer::with_capacity(256);

    std::thread::scope(|scope| {
        let flip_buffer = &flip_buffer;
        for i in 0..10 {
            scope.spawn(move || {
                for j in 0..10 {
                    flip_buffer.push_sync(i * 10 + j).unwrap();
                }
            });
        }
    });

    let mut idx = flip_buffer.drain().collect::<Vec<_>>();
    idx.sort();

    assert_eq!(idx, (0..100).collect::<Vec<_>>());
}

#[test]
#[cfg(feature = "std")]
fn test_flib_queue() {
    let mut flip_queue = FlipQueue::<_>::with_capacity(1);

    std::thread::scope(|scope| {
        let flip_queue = &flip_queue;
        for i in 0..10 {
            scope.spawn(move || {
                for j in 0..10 {
                    flip_queue.push_sync(i * 10 + j);
                }
            });
        }
    });

    let mut idx = flip_queue.drain().collect::<Vec<_>>();
    idx.sort();

    assert_eq!(idx, (0..100).collect::<Vec<_>>());
}

#[test]
#[cfg(feature = "std")]
fn test_flib_queue_push_pop() {
    let mut flip_queue = FlipQueue::<_>::with_capacity(1);

    std::thread::scope(|scope| {
        let flip_queue = &flip_queue;

        for _ in 0..10 {
            scope.spawn(move || {
                for _ in 0..10 {
                    while let None = flip_queue.pop_sync() {
                        std::thread::yield_now();
                    }
                }
            });
        }

        for i in 0..10 {
            scope.spawn(move || {
                for j in 0..10 {
                    flip_queue.push_sync(i * 10 + j);
                }
            });
        }
    });

    assert_eq!(flip_queue.pop(), None);
}
