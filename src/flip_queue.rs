//! Queues with concurrent pushes and pops and exclusive access for batch processing.

use core::{
    cell::UnsafeCell,
    mem::{swap, take},
    sync::atomic::{AtomicU64, Ordering},
};

use lock_api::{RawRwLock, RwLock};

use crate::ring_buffer::{self, RingBuffer, ring_index};

/// Implements ring-buffer data structure.
///
/// It allows pushing values from multiple threads in parallel until capacity is reached.
/// After that it requires exclusive borrow to push more values.
///
/// Shared pops consume elements present at the last exclusive operation.
/// Exclusive access makes pushed elements available to pop and freed slots available to push.
/// The process aborts if either shared operation is attempted 2^63 times between flushes.
pub struct FlipBuffer<T> {
    buffer: RingBuffer<UnsafeCell<T>>,

    /// Number of `pop_sync` attempts since the last flush.
    popped: AtomicU64,

    /// Number of `push_sync` attempts since the last flush.
    pushed: AtomicU64,
}

unsafe impl<T> Sync for FlipBuffer<T> where T: Send {}
unsafe impl<T> Send for FlipBuffer<T> where T: Send {}

impl<T> Drop for FlipBuffer<T> {
    fn drop(&mut self) {
        self.flush();
    }
}

impl<T> Default for FlipBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

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
        let len = self.buffer.len();
        let popped = usize::try_from(take(self.popped.get_mut()))
            .unwrap_or(usize::MAX)
            .min(len);
        let pushed = usize::try_from(take(self.pushed.get_mut()))
            .unwrap_or(usize::MAX)
            .min(self.buffer.capacity() - len);

        if popped == 0 && pushed == 0 {
            return;
        }

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
    ///
    /// # Errors
    ///
    /// Function will fail if queue is full, returning the value back.
    pub fn push_sync(&self, value: T) -> Result<(), T> {
        let len = self.buffer.len();
        let cap = self.buffer.capacity();
        let Some(pushed) = reserve(&self.pushed, cap - len) else {
            return Err(value);
        };
        let idx = ring_index(self.buffer.head(), len + pushed, cap);

        // Safety: This reservation owns a vacant slot until the next exclusive borrow.
        unsafe {
            let slot = self.buffer.as_ptr().add(idx);
            UnsafeCell::raw_get(slot).write(value);
        }
        Ok(())
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
        let popped = reserve(&self.popped, self.buffer.len())?;
        let idx = ring_index(self.buffer.head(), popped, self.buffer.capacity());

        // Safety: This reservation owns an initialized slot until the next exclusive borrow.
        Some(unsafe {
            let slot = self.buffer.as_ptr().add(idx);
            UnsafeCell::raw_get(slot).read()
        })
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

    /// Flushes pending operations and exchanges the backing buffer with `ring`.
    pub fn swap_buffer(&mut self, ring: &mut RingBuffer<T>) {
        self.flush();
        swap(ring.as_unsafe_cell_mut(), &mut self.buffer);
    }
}

const ATTEMPT_LIMIT: u64 = 1 << 63;

#[inline]
fn reserve(counter: &AtomicU64, limit: usize) -> Option<usize> {
    let ticket = counter.fetch_add(1, Ordering::Acquire);
    // Leave headroom for concurrent increments while the process is aborting.
    if ticket >= ATTEMPT_LIMIT - 1 {
        abort_attempt_overflow();
    }
    let index = usize::try_from(ticket).ok()?;
    (index < limit).then_some(index)
}

#[cold]
#[inline(never)]
extern "C" fn abort_attempt_overflow() -> ! {
    #[cfg(feature = "std")]
    std::process::abort();

    // The C ABI prevents the panic from unwinding into the queue operation.
    #[cfg(not(feature = "std"))]
    panic!("flip buffer attempt limit reached");
}

/// A FIFO drain that drops remaining elements when dropped.
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

/// A growable queue with concurrent pushes and pops.
/// Shared operations take a read lock to reserve available slots and may wait for a writer.
/// Growing, flushing shared reservations, and swapping buffers require exclusive access.
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
    /// This function may block while another operation holds or waits for the write lock.
    ///
    /// # Errors
    ///
    /// Returns the value if there is no vacant slot available for reservation.
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

    /// Appends a value, growing the queue if needed.
    pub fn push(&mut self, value: T) {
        self.buffer.get_mut().push(value);
    }

    /// Pops the next available element under a shared lock.
    /// Returns `None` when the readable slots are exhausted, even if a shared
    /// push has since succeeded. Use [`pop_sync`](Self::pop_sync) to flush
    /// pending pushes when the readable slots run out.
    ///
    /// Acquiring the shared lock may block behind a writer.
    pub fn try_pop_sync(&self) -> Option<T> {
        let read = self.buffer.read();
        read.pop_sync()
    }

    /// Pops the oldest value, taking the write lock if pending pushes need flushing.
    pub fn pop_sync(&self) -> Option<T> {
        let read = self.buffer.read();

        if let Some(value) = read.pop_sync() {
            return Some(value);
        }

        // This is just an optimization to avoid locking the queue if there were no pushed values.
        if read.pushed.load(Ordering::Relaxed) == 0 {
            return None;
        }

        drop(read);
        self.pop_slow()
    }

    fn pop_slow(&self) -> Option<T> {
        let mut write = self.buffer.write();
        write.pop()
    }

    /// Removes the oldest value, or returns `None` if the queue is empty.
    pub fn pop(&mut self) -> Option<T> {
        self.buffer.get_mut().pop()
    }

    /// Drains the queue in FIFO order.
    pub fn drain(&mut self) -> Drain<'_, T> {
        self.buffer.get_mut().drain()
    }

    /// Calls `f` with a drain while holding the write lock.
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
fn swap_wrapped_owned_values() {
    let mut ring = RingBuffer::with_capacity(2);
    ring.push(alloc::boxed::Box::new(1));
    ring.push(alloc::boxed::Box::new(2));
    drop(ring.pop());
    ring.push(alloc::boxed::Box::new(3));

    let mut buffer = FlipBuffer::with_capacity(1);
    buffer.push_sync(alloc::boxed::Box::new(4)).unwrap();
    buffer.swap_buffer(&mut ring);
    assert_eq!(ring.pop().as_deref(), Some(&4));
    assert_eq!(buffer.pop_sync().as_deref(), Some(&2));
    assert_eq!(buffer.pop().as_deref(), Some(&3));
}

#[test]
fn flush_clamps_unsuccessful_attempts() {
    let mut buffer = FlipBuffer::with_capacity(1);
    buffer.push_sync(3).unwrap();
    assert_eq!(buffer.push_sync(4), Err(4));
    assert_eq!(*buffer.pushed.get_mut(), 2);
    assert_eq!(buffer.pop_sync(), None);
    assert_eq!(*buffer.popped.get_mut(), 1);

    buffer.flush();
    assert_eq!(buffer.pop_sync(), Some(3));
    assert_eq!(buffer.pop_sync(), None);
    assert_eq!(*buffer.popped.get_mut(), 2);
    assert_eq!(buffer.pop(), None);
}

#[test]
#[cfg(not(miri))]
fn abort_on_attempt_limit() {
    extern crate std;
    use std::{env, process::Command, string::String};

    const CHILD_OPERATION: &str = "AMITY_FLIP_QUEUE_TEST_ABORT_OPERATION";
    const MARKER: &str = "attempt below limit returned";
    if let Ok(operation) = env::var(CHILD_OPERATION) {
        let mut buffer = FlipBuffer::<u8>::new();
        match operation.as_str() {
            "push" => {
                *buffer.pushed.get_mut() = ATTEMPT_LIMIT - 2;
                assert_eq!(buffer.push_sync(1), Err(1));
                std::eprintln!("{MARKER}");
                let _ = buffer.push_sync(1);
            }
            "pop" => {
                *buffer.popped.get_mut() = ATTEMPT_LIMIT - 2;
                assert_eq!(buffer.pop_sync(), None);
                std::eprintln!("{MARKER}");
                let _ = buffer.pop_sync();
            }
            _ => panic!("unknown operation"),
        }
        return;
    }

    for operation in ["push", "pop"] {
        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "flip_queue::abort_on_attempt_limit",
                "--nocapture",
            ])
            .env(CHILD_OPERATION, operation)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(MARKER), "{operation}: {stderr}");
        assert!(!output.status.success(), "{operation} returned");
        assert_ne!(output.status.code(), Some(101), "{operation} unwound");
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
    idx.sort_unstable();

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
    idx.sort_unstable();

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
                    while flip_queue.pop_sync().is_none() {
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
