use alloc::{boxed::Box, vec::Vec};

use core::{
    cell::UnsafeCell,
    mem::{replace, size_of, MaybeUninit},
    ptr::{copy_nonoverlapping, drop_in_place},
    slice::{from_raw_parts, from_raw_parts_mut},
};

use crate::capacity_overflow;

/// Ring-buffer structure.
///
/// This buffer allocates memory on heap and allows to push and pop values from it.
/// Values are pushed to the end of the buffer and popped from the beginning.
///
/// It is reduced version of `std::collections::VecDeque`.
/// The advantage of this implementation is that it guaranteed to be ring-buffer
/// and provides unchecked access to its internals.
pub struct RingBuffer<T> {
    head: usize,
    len: usize,
    array: Box<[MaybeUninit<T>]>,
}

unsafe impl<T> Send for RingBuffer<T> where T: Send {}
unsafe impl<T> Sync for RingBuffer<T> where T: Sync {}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        let (front, back) = self.as_mut_slices();
        unsafe {
            let _back_dropped = Dropper(back);
            drop_in_place(front);
        }
    }
}

impl<T> RingBuffer<T> {
    const MIN_NON_ZERO_CAP: usize = if size_of::<T>() == 1 {
        8
    } else if size_of::<T>() <= 1024 {
        4
    } else {
        1
    };

    #[inline]
    pub fn new() -> Self {
        if size_of::<T>() == 0 {
            // This causes box len to be usize::MAX if T size is 0.
            // The array is still 0 bytes long.
            Self::with_capacity(usize::MAX)
        } else {
            Self::with_capacity(0)
        }
    }

    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        let mut vec = Vec::with_capacity(cap);
        // Safety: no bytes require initialization.
        unsafe {
            vec.set_len(vec.capacity());
        }
        let array = vec.into_boxed_slice();

        RingBuffer {
            array,
            head: 0,
            len: 0,
        }
    }

    #[inline]
    pub fn as_slices(&self) -> (&[T], &[T]) {
        let front_len = self.len.min(self.array.len() - self.head);
        let back_len = self.len - front_len;
        unsafe {
            // Cast `*constUnsafeCell<MaybeUninit<T>>` to `*constT` is valid.
            let ptr: *const T = self.array.as_ptr().cast::<T>();
            let front_ptr = ptr.add(self.head);

            let front = from_raw_parts(front_ptr, front_len);
            let back = from_raw_parts(ptr, back_len);

            (front, back)
        }
    }

    #[inline]
    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let front_len = self.len.min(self.array.len() - self.head);
        let back_len = self.len - front_len;
        unsafe {
            // Cast `*mut MaybeUninit<T>` to `*mut T` is valid.
            let ptr: *mut T = self.array.as_mut_ptr().cast::<T>();
            let front_ptr = ptr.add(self.head);

            let front = from_raw_parts_mut(front_ptr, front_len);
            let back = from_raw_parts_mut(ptr, back_len);

            (front, back)
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        let (front, back) = self.as_mut_slices();
        let front = front as *mut [T];
        let back = back as *mut [T];

        self.head = 0;
        self.len = 0;

        unsafe {
            let _back_dropped = Dropper(&mut *back);
            drop_in_place(front);
        }
    }

    pub fn push(&mut self, value: T) {
        if self.len < self.array.len() {
            // Queue is not full.
            let idx = ring_index(self.head, self.len, self.array.len());

            // Safety: We have exclusive access to the queue at this index.
            unsafe {
                *self.array.get_unchecked_mut(idx) = MaybeUninit::new(value);
            }
            self.len += 1;
        } else {
            // Queue is full.
            // Grow the queue.

            // Create new array.
            let mut new_vec = Vec::with_capacity(new_cap::<T>(self.array.len()));
            // Safety: no bytes require initialization.
            unsafe {
                new_vec.set_len(new_vec.capacity());
            }
            let mut new_array = new_vec.into_boxed_slice();

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

                *new_array.get_unchecked_mut(self.array.len()) = MaybeUninit::new(value);
            }

            self.head = 0;
            self.len = self.array.len() + 1;
            let _ = replace(&mut self.array, new_array);
        }
    }

    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            // Queue is empty.
            return None;
        }

        // Safety: head index never gets greater than or equal to array length.
        // If tail > 0 the head index was initialized with value.
        let value = unsafe {
            debug_assert!(self.head < self.array.len());
            self.array.get_unchecked(self.head).assume_init_read()
        };

        self.head = (self.head + 1) % self.array.len();
        self.len -= 1;

        Some(value)
    }

    #[inline(always)]
    pub fn drain(&mut self) -> Drain<'_, T> {
        Drain {
            len: self.len,
            ring_buffer: self,
        }
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn capacity(&self) -> usize {
        self.array.len()
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline(always)]
    pub unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    #[inline(always)]
    pub fn head(&self) -> usize {
        self.head
    }

    #[inline(always)]
    pub fn as_ptr(&self) -> *const T {
        // Cast `*const MaybeUninit<T>` to `*const T` is valid.
        self.array.as_ptr().cast::<T>()
    }

    #[inline(always)]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        // Cast `*const MaybeUninit<T>` to `*const T` is valid.
        self.array.as_mut_ptr().cast::<T>()
    }

    #[inline(always)]
    pub(crate) fn as_unsafe_cell_mut(&mut self) -> &mut RingBuffer<UnsafeCell<T>> {
        // Safety: UnsafeCell<MaybeUninit<T>> is layout compatible with UnsafeCell<MaybeUninit<T>>.
        // Note that this function has mutable reference to self.
        // It temporary allows to mutate elements of the buffer with shared borrow.
        unsafe { &mut *(self as *mut Self as *mut RingBuffer<UnsafeCell<T>>) }
    }
}

pub struct Drain<'a, T> {
    ring_buffer: &'a mut RingBuffer<T>,
    len: usize,
}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        self.ring_buffer.clear();
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        debug_assert_eq!(self.len, self.ring_buffer.len);
        if self.len == 0 {
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;

        self.ring_buffer.head = (self.ring_buffer.head + 1) % self.ring_buffer.array.len();
        self.ring_buffer.len -= 1;
        self.len -= 1;

        // Safety: head index never gets greater than or equal to array length.
        // If tail > 0 the head index was initialized with value.
        let value = unsafe {
            debug_assert!(head < self.ring_buffer.array.len());
            self.ring_buffer
                .array
                .get_unchecked(head)
                .assume_init_read()
        };

        Some(value)
    }

    #[inline(always)]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.len;
        (len, Some(len))
    }

    #[inline(always)]
    fn count(self) -> usize {
        self.len
    }

    fn nth(&mut self, n: usize) -> Option<T> {
        debug_assert_eq!(self.len, self.ring_buffer.len);
        if n == 0 {
            return self.next();
        }

        if self.len <= n {
            self.ring_buffer.clear();
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;
        let new_head = ring_index(head, n, self.ring_buffer.array.len());

        self.ring_buffer.head = new_head;
        self.ring_buffer.len -= n;
        self.len -= n;

        let front_len = n.min(self.ring_buffer.array.len() - head);
        let back_len = n - front_len;
        let (front, back) = unsafe {
            // Cast `*mut MaybeUninit<T>` to `*mut T` is valid.
            let ptr = self.ring_buffer.array.as_mut_ptr().cast::<T>();
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
    #[inline(always)]
    fn len(&self) -> usize {
        self.len
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

#[inline(always)]
pub(crate) fn ring_index(head: usize, len: usize, cap: usize) -> usize {
    let (sum, wrap) = head.overflowing_add(len);
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
        Some(new_cap) => new_cap.max(RingBuffer::<T>::MIN_NON_ZERO_CAP),
        None => capacity_overflow(),
    }
}
