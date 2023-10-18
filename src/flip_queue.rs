//! Provides flipping queue type for message passing between multiple producers and single consumer.
//!
//! The twist is that message sending cannot happen concurrently with message receiving.
//! Therefore queue is flipped to either side - producer or consumer.
//!
//! This implementation uses borrowing for this purpose.
//! Shared reference allows to send messages from multiple threads.
//! Exclusive reference allows to receive messages from single thread.

use core::{
    mem::{replace, size_of, MaybeUninit},
    ptr::{copy_nonoverlapping, drop_in_place},
    slice::from_raw_parts_mut,
};

use crate::{
    capacity_overflow,
    sync::{
        read_atomic, read_init_cell, with_atomic, write_atomic, write_cell, AtomicU64, Ordering,
        RwLock, UnsafeCell,
    },
};

/// Implements ring-buffer data structure.
///
/// It allows pushing values from multiple threads in parallel until capacity is reached.
/// After that it requires exclusive borrow to push more values.
///
/// Reading values always requires exclusive borrow.
///
/// Thus it doesn't require any internal synchronization primitives.
pub struct FlipRingBuffer<T> {
    array: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: usize,
    // u64 is used to allow incrementing without overflow.
    tail: AtomicU64,
}

unsafe impl<T> Sync for FlipRingBuffer<T> where T: Send {}
unsafe impl<T> Send for FlipRingBuffer<T> where T: Send {}

impl<T> FlipRingBuffer<T> {
    /// Create new ring buffer.
    pub fn new() -> Self {
        // This causes box len to be usize::MAX if T size is 0.
        let array = Vec::new().into_boxed_slice();

        FlipRingBuffer {
            array,
            head: 0,
            tail: AtomicU64::new(0),
        }
    }

    /// Create new ring buffer.
    pub fn with_capacity(cap: usize) -> Self {
        let array = (0..cap)
            .map(|_| UnsafeCell::new(MaybeUninit::<T>::uninit()))
            .collect::<Vec<_>>()
            .into_boxed_slice();

        FlipRingBuffer {
            array,
            head: 0,
            tail: AtomicU64::new(0),
        }
    }

    /// Returns content of the buffer as pair of slices.
    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let tail = self.clamp_tail();

        let front_len = tail.min(self.array.len() - self.head);
        let back_len = tail - front_len;
        unsafe {
            // Cast `*mut UnsafeCell<MaybeUninit<T>>` to `*mut T` is valid.
            let ptr: *mut T = self.array.as_mut_ptr().cast::<T>();
            let front_ptr = ptr.add(self.head);

            let front = from_raw_parts_mut(front_ptr, front_len);
            let back = from_raw_parts_mut(ptr, back_len);

            (front, back)
        }
    }

    /// Clears the buffer, removing all values.
    pub fn clear(&mut self) {
        let (front, back) = self.as_mut_slices();
        let front = front as *mut [T];
        let back = back as *mut [T];

        self.head = 0;
        self.tail = AtomicU64::new(0);

        unsafe {
            let _back_dropped = Dropper(&mut *back);
            drop_in_place(front);
        }
    }

    /// Clamps the tail to the capacity.
    /// When tail grows larget then capacity no more values are pushed.
    /// All operations with mulable access clamp tail to capacity before using it.
    fn clamp_tail(&mut self) -> usize {
        with_atomic(&mut self.tail, |tail| {
            match usize::try_from(*tail) {
                Ok(tail) if tail <= self.array.len() => tail,
                _ => {
                    // Cap the tail at capacity.
                    *tail = self.array.len() as u64;
                    self.array.len()
                }
            }
        })
    }

    /// Attempts to push value to the queue.
    ///
    /// If at full capacity this will fail and return the value back.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        // Acquire index to write to.
        let tail = self.tail.fetch_add(1, Ordering::Acquire);
        match usize::try_from(tail) {
            Ok(tail) if tail < self.array.len() => {
                // Queue is not full.
                let idx = deque_index(self.head, tail, self.array.len());

                // Safety: We have exclusive access to the queue at this index.
                unsafe {
                    write_cell(self.array.get_unchecked(idx), MaybeUninit::new(value));
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
        match usize::try_from(read_atomic(&mut self.tail)) {
            Ok(tail) if tail < self.array.len() => {
                // Queue is not full.
                let idx = deque_index(self.head, tail, self.array.len());

                // Safety: We have exclusive access to the queue at this index.
                unsafe {
                    write_cell(self.array.get_unchecked(idx), MaybeUninit::new(value));
                }
            }
            _ => {
                // Queue is full.
                // Grow the queue.
                let new_cap = new_cap::<T>(self.array.len());

                // Create new array.
                let mut new_array = (0..new_cap)
                    .map(|_| UnsafeCell::new(MaybeUninit::<T>::uninit()))
                    .collect::<Vec<_>>()
                    .into_boxed_slice();

                unsafe {
                    copy_nonoverlapping(
                        self.array.as_ptr().add(self.head),
                        new_array.as_mut_ptr(),
                        self.array.len() - self.head,
                    );
                    copy_nonoverlapping(
                        self.array.as_ptr(),
                        new_array.as_mut_ptr().add(self.array.len() - self.head),
                        self.head,
                    );

                    write_cell(
                        new_array.get_unchecked(self.array.len()),
                        MaybeUninit::new(value),
                    );
                }

                self.head = 0;
                write_atomic(&mut self.tail, self.array.len() as u64 + 1);
                let _ = replace(&mut self.array, new_array);
            }
        }
    }

    /// Pop value from the queue.
    ///
    /// This requires exclusive borrow.
    pub fn pop(&mut self) -> Option<T> {
        let tail = self.clamp_tail();
        if tail == 0 {
            // Queue is empty.
            return None;
        }

        // Safety: head index never gets greater than or equal to array length.
        // If tail > 0 the head index was initialized with value.
        let value = unsafe {
            debug_assert!(self.head < self.array.len());
            read_init_cell(self.array.get_unchecked_mut(self.head))
        };

        self.head = (self.head + 1) % self.array.len();
        with_atomic(&mut self.tail, |tail| *tail -= 1);

        Some(value)
    }

    /// Returns iterator over values in the queue.
    ///
    /// This requires exclusive borrow.
    ///
    /// This iterator will consume all values in the queue even if dropped before it is finished.
    pub fn drain(&mut self) -> Drain<'_, T> {
        let tail = self.clamp_tail();
        Drain {
            ring_buffer: self,
            tail,
        }
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
#[repr(transparent)]
pub struct FlipQueue<T> {
    ring_buffer: RwLock<FlipRingBuffer<T>>,
}

impl<T> Drop for FlipQueue<T> {
    fn drop(&mut self) {
        let (front, back) = self.as_mut_slices();
        unsafe {
            let _back_dropped = Dropper(back);
            drop_in_place(front);
        }
    }
}

impl<T> FlipQueue<T> {
    /// Create new flip queue.
    pub fn new() -> Self {
        FlipQueue {
            ring_buffer: RwLock::new(FlipRingBuffer::new()),
        }
    }

    /// Create new flip queue.
    pub fn with_capacity(cap: usize) -> Self {
        FlipQueue {
            ring_buffer: RwLock::new(FlipRingBuffer::with_capacity(cap)),
        }
    }

    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        self.ring_buffer.get_mut().as_mut_slices()
    }

    /// Clears the queue, removing all values.
    pub fn clear(&mut self) {
        self.ring_buffer.get_mut().clear();
    }

    /// Tries to push value to the queue.
    ///
    /// If at full capacity this will return the value back.
    ///
    /// This function may block if call to `push` grows the queue.
    pub fn try_push(&self, value: T) -> Result<(), T> {
        self.ring_buffer.read().try_push(value)
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
        let mut ring_buffer = self.ring_buffer.write();
        ring_buffer.push(value);
    }

    pub fn pop(&mut self) -> Option<T> {
        self.ring_buffer.get_mut().pop()
    }

    pub fn drain(&mut self) -> Drain<'_, T> {
        self.ring_buffer.get_mut().drain()
    }
}

pub struct Drain<'a, T> {
    ring_buffer: &'a mut FlipRingBuffer<T>,
    tail: usize,
}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        self.ring_buffer.clear();
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.tail == 0 {
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;

        self.ring_buffer.head = (self.ring_buffer.head + 1) % self.ring_buffer.array.len();
        with_atomic(&mut self.ring_buffer.tail, |tail| *tail -= 1);
        self.tail -= 1;

        // Safety: head index never gets greater than or equal to array length.
        // If tail > 0 the head index was initialized with value.
        let value = unsafe {
            debug_assert!(head < self.ring_buffer.array.len());
            read_init_cell(self.ring_buffer.array.get_unchecked_mut(head))
        };

        Some(value)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len();
        (len, Some(len))
    }

    fn count(self) -> usize {
        self.len()
    }

    fn nth(&mut self, n: usize) -> Option<T> {
        if n == 0 {
            return self.next();
        }

        if self.tail <= n {
            self.ring_buffer.clear();
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;
        let new_head = deque_index(head, n, self.ring_buffer.array.len());

        self.ring_buffer.head = new_head;
        with_atomic(&mut self.ring_buffer.tail, |tail| *tail -= n as u64);
        self.tail -= n;

        let front_len = n.min(self.ring_buffer.array.len() - head);
        let back_len = n - front_len;
        let (front, back) = unsafe {
            // Cast `*mut UnsafeCell<MaybeUninit<T>>` to `*mut T` is valid.
            let ptr: *mut T = self.ring_buffer.array.as_mut_ptr().cast::<T>();
            let front_ptr = ptr.add(head);

            let front = from_raw_parts_mut(front_ptr, front_len);
            let back = from_raw_parts_mut(ptr, back_len);

            (front, back)
        };

        debug_assert_eq!(back.len() + front.len(), n);
        let _back_dropper = Dropper(&mut back[..new_head]);
        unsafe {
            drop_in_place(front);
        }

        self.next()
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {
    fn len(&self) -> usize {
        self.tail
    }
}

#[inline(always)]
fn deque_index(head: usize, tail: usize, cap: usize) -> usize {
    let (sum, wrap) = head.overflowing_add(tail);
    let sum = if wrap { dewrap(sum, cap) } else { sum };
    sum % cap
}

#[inline(always)]
#[cold]
fn dewrap(sum: usize, cap: usize) -> usize {
    (sum % cap).wrapping_sub(cap)
}

#[inline(always)]
fn new_cap<T>(cap: usize) -> usize {
    match cap.checked_add(cap) {
        Some(new_cap) => new_cap.max(min_non_zero_cap::<T>()),
        None => capacity_overflow(),
    }
}

const fn min_non_zero_cap<T>() -> usize {
    if size_of::<T>() == 1 {
        8
    } else if size_of::<T>() <= 1024 {
        4
    } else {
        1
    }
}

struct Dropper<'a, T>(&'a mut [T]);

impl<'a, T> Drop for Dropper<'a, T> {
    fn drop(&mut self) {
        unsafe {
            drop_in_place(self.0);
        }
    }
}

#[test]
#[cfg(feature = "std")]
fn test_flib_buffer() {
    let mut ring_buffer = FlipRingBuffer::with_capacity(256);

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
