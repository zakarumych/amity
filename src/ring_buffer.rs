//! A growable FIFO ring buffer.

use alloc::{boxed::Box, vec::Vec};

use core::{
    cell::UnsafeCell,
    mem::{MaybeUninit, replace, size_of},
    ops::{Deref, DerefMut},
    ptr::{
        NonNull, copy_nonoverlapping, drop_in_place, slice_from_raw_parts, slice_from_raw_parts_mut,
    },
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
#[repr(C)]
pub struct RingBuffer<T> {
    head: usize,
    len: usize,
    array: Allocation<T>,
}

// A thin pointer and a length give the allocation a fixed layout for as_unsafe_cell_mut.
#[repr(C)]
struct Allocation<T> {
    ptr: NonNull<MaybeUninit<T>>,
    len: usize,
}

impl<T> From<Box<[MaybeUninit<T>]>> for Allocation<T> {
    fn from(array: Box<[MaybeUninit<T>]>) -> Self {
        let len = array.len();
        let ptr = NonNull::from(Box::leak(array)).cast();
        Self { ptr, len }
    }
}

impl<T> Deref for Allocation<T> {
    type Target = [MaybeUninit<T>];

    fn deref(&self) -> &Self::Target {
        // Safety: The allocation owns len slots for the lifetime of the borrow.
        unsafe { core::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> DerefMut for Allocation<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // Safety: The exclusive borrow covers the entire allocation.
        unsafe { core::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl<T> Drop for Allocation<T> {
    fn drop(&mut self) {
        // Safety: Both the pointer and length come from the owned boxed slice.
        let array = slice_from_raw_parts_mut(self.ptr.as_ptr(), self.len);
        unsafe { drop(Box::from_raw(array)) }
    }
}

unsafe impl<T> Send for RingBuffer<T> where T: Send {}
unsafe impl<T> Sync for RingBuffer<T> where T: Sync {}

impl<T> Default for RingBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Drop for RingBuffer<T> {
    fn drop(&mut self) {
        let (front, back) = self.as_mut_slice_pointers();
        unsafe {
            drop_two_slices(front, back);
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

    /// Creates an empty buffer.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        if size_of::<T>() == 0 {
            // This causes box len to be usize::MAX if T size is 0.
            // The array is still 0 bytes long.
            Self::with_capacity(usize::MAX)
        } else {
            Self::with_capacity(0)
        }
    }

    /// Creates an empty buffer with room for at least `cap` elements.
    #[inline]
    #[must_use]
    pub fn with_capacity(cap: usize) -> Self {
        let mut vec = Vec::with_capacity(cap);
        // Safety: no bytes require initialization.
        unsafe {
            vec.set_len(vec.capacity());
        }
        let array = vec.into_boxed_slice().into();

        RingBuffer {
            array,
            head: 0,
            len: 0,
        }
    }

    /// Returns the elements in queue order as two slices.
    #[inline]
    #[must_use]
    pub fn as_slices(&self) -> (&[T], &[T]) {
        let (front, back) = self.as_slice_pointers();
        unsafe { (&*front, &*back) }
    }

    /// Returns the elements in queue order as two mutable slices.
    #[inline]
    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let (front, back) = self.as_mut_slice_pointers();
        unsafe { (&mut *front, &mut *back) }
    }

    #[inline]
    fn as_mut_slice_pointers(&mut self) -> (*mut [T], *mut [T]) {
        let front_len = self.len.min(self.array.len() - self.head);
        let back_len = self.len - front_len;
        unsafe {
            // Cast `*mut MaybeUninit<T>` to `*mut T` is valid.
            let ptr: *mut T = self.array.as_mut_ptr().cast::<T>();
            let front_ptr = ptr.add(self.head);

            let front = slice_from_raw_parts_mut(front_ptr, front_len);
            let back = slice_from_raw_parts_mut(ptr, back_len);

            (front, back)
        }
    }

    #[inline]
    fn as_slice_pointers(&self) -> (*const [T], *const [T]) {
        let front_len = self.len.min(self.array.len() - self.head);
        let back_len = self.len - front_len;
        unsafe {
            // Cast `*mut MaybeUninit<T>` to `*mut T` is valid.
            let ptr: *const T = self.array.as_ptr().cast::<T>();
            let front_ptr = ptr.add(self.head);

            let front = slice_from_raw_parts(front_ptr, front_len);
            let back = slice_from_raw_parts(ptr, back_len);

            (front, back)
        }
    }

    /// Drops all elements and retains the capacity.
    #[inline]
    pub fn clear(&mut self) {
        let (front, back) = self.as_mut_slice_pointers();

        self.head = 0;
        self.len = 0;

        unsafe {
            drop_two_slices(front, back);
        }
    }

    /// Appends a value, growing the buffer if full.
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
            let _ = replace(&mut self.array, new_array.into());
        }
    }

    /// Removes the front element, returning `None` if the buffer is empty.
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

    /// Borrows the elements in queue order.
    #[inline]
    #[must_use]
    pub fn iter(&self) -> Iter<'_, T> {
        let (front, back) = self.as_slices();
        Iter {
            front: front.iter(),
            back: back.iter(),
        }
    }

    /// Mutably borrows the elements in queue order.
    #[inline]
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        let (front, back) = self.as_mut_slices();
        IterMut {
            front: front.iter_mut(),
            back: back.iter_mut(),
        }
    }

    /// Drains elements in queue order. Dropping the iterator clears the remainder.
    #[inline]
    pub fn drain(&mut self) -> Drain<'_, T> {
        Drain { ring_buffer: self }
    }

    /// Returns whether the buffer contains no elements.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the number of elements the buffer can hold without growing.
    #[inline]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.array.len()
    }

    /// Returns the number of elements in the buffer.
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Sets the number of live elements starting at the head, wrapping at capacity.
    ///
    /// # Safety
    ///
    /// `len` must fit within the capacity. Before safe access or drop, all slots
    /// in the resulting range must hold initialized values owned by the buffer.
    #[inline]
    pub unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    /// Returns the physical index of the queue head.
    #[inline]
    #[must_use]
    pub fn head(&self) -> usize {
        self.head
    }

    /// Sets the physical index of the first element.
    ///
    /// # Safety
    ///
    /// `head` must be below capacity, or zero for a zero-capacity buffer. Before
    /// safe access or drop, the logical range from this head must contain
    /// initialized values owned by the buffer.
    #[inline]
    pub unsafe fn set_head(&mut self, head: usize) {
        self.head = head;
    }

    /// Returns a pointer to the start of the backing storage.
    #[inline]
    #[must_use]
    pub fn as_ptr(&self) -> *const T {
        // Cast `*const MaybeUninit<T>` to `*const T` is valid.
        self.array.as_ptr().cast::<T>()
    }

    /// Returns a mutable pointer to the start of the backing storage.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut T {
        // Cast `*const MaybeUninit<T>` to `*const T` is valid.
        self.array.as_mut_ptr().cast::<T>()
    }

    #[inline]
    #[doc(hidden)]
    pub fn as_unsafe_cell_mut(&mut self) -> &mut RingBuffer<UnsafeCell<T>> {
        // Safety: Both repr(C) structures have identical field offsets and thin pointer layout.
        // UnsafeCell<T> has T's layout and drop behavior. The exclusive borrow covers the conversion.
        unsafe { &mut *core::ptr::from_mut(self).cast::<RingBuffer<UnsafeCell<T>>>() }
    }
}

impl<'a, T> IntoIterator for &'a RingBuffer<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut RingBuffer<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Iterates over shared references in queue order.
pub struct Iter<'a, T> {
    front: core::slice::Iter<'a, T>,
    back: core::slice::Iter<'a, T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        self.front.next().or_else(|| self.back.next())
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.front.len() + self.back.len(), None)
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<&'a T> {
        let front_len = self.front.len();
        if n >= front_len {
            self.front.nth(front_len);
            self.back.nth(n - front_len)
        } else {
            self.front.nth(n)
        }
    }

    #[inline]
    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, &'a T) -> B,
    {
        let mut state = init;
        for item in self.front {
            state = f(state, item);
        }
        for item in self.back {
            state = f(state, item);
        }
        state
    }

    #[inline]
    fn for_each<F>(self, mut f: F)
    where
        F: FnMut(&'a T),
    {
        for item in self.front {
            f(item);
        }
        for item in self.back {
            f(item);
        }
    }

    #[inline]
    fn count(self) -> usize {
        self.front.len() + self.back.len()
    }

    #[inline]
    fn last(self) -> Option<&'a T> {
        self.back.last().or(self.front.last())
    }

    #[inline]
    fn all<F>(&mut self, mut f: F) -> bool
    where
        F: FnMut(&'a T) -> bool,
    {
        for item in &mut self.front {
            if !f(item) {
                return false;
            }
        }

        for item in &mut self.back {
            if !f(item) {
                return false;
            }
        }

        true
    }

    #[inline]
    fn any<F>(&mut self, mut f: F) -> bool
    where
        F: FnMut(&'a T) -> bool,
    {
        for item in &mut self.front {
            if f(item) {
                return true;
            }
        }

        for item in &mut self.back {
            if f(item) {
                return true;
            }
        }

        false
    }

    #[inline]
    fn find<P>(&mut self, mut predicate: P) -> Option<&'a T>
    where
        P: FnMut(&&'a T) -> bool,
    {
        self.front
            .find(&mut predicate)
            .or_else(|| self.back.find(predicate))
    }

    #[inline]
    fn position<P>(&mut self, mut predicate: P) -> Option<usize>
    where
        P: FnMut(&'a T) -> bool,
    {
        let mut pos = 0;
        for item in &mut self.front {
            if predicate(item) {
                return Some(pos);
            }
            pos += 1;
        }

        for item in &mut self.back {
            if predicate(item) {
                return Some(pos);
            }
            pos += 1;
        }

        None
    }

    #[inline]
    fn rposition<P>(&mut self, mut predicate: P) -> Option<usize>
    where
        P: FnMut(Self::Item) -> bool,
    {
        let mut pos = self.back.len() + self.front.len();
        for item in self.back.by_ref().rev() {
            pos -= 1;
            if predicate(item) {
                return Some(pos);
            }
        }

        for item in self.front.by_ref().rev() {
            pos -= 1;
            if predicate(item) {
                return Some(pos);
            }
        }

        None
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a T> {
        self.back.next_back().or_else(|| self.front.next_back())
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<&'a T> {
        let back_len = self.back.len();
        if n >= back_len {
            self.back.nth_back(back_len);
            self.front.nth_back(n - back_len)
        } else {
            self.back.nth_back(n)
        }
    }

    #[inline]
    fn rfold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, &'a T) -> B,
    {
        let mut state = init;
        for item in self.back.rev() {
            state = f(state, item);
        }
        for item in self.front.rev() {
            state = f(state, item);
        }
        state
    }

    #[inline]
    fn rfind<P>(&mut self, mut predicate: P) -> Option<&'a T>
    where
        P: FnMut(&&'a T) -> bool,
    {
        self.back
            .rfind(&mut predicate)
            .or_else(|| self.front.rfind(predicate))
    }
}

/// Iterates over mutable references in queue order.
pub struct IterMut<'a, T> {
    front: core::slice::IterMut<'a, T>,
    back: core::slice::IterMut<'a, T>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    #[inline]
    fn next(&mut self) -> Option<&'a mut T> {
        self.front.next().or_else(|| self.back.next())
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.front.len() + self.back.len(), None)
    }

    #[inline]
    fn nth(&mut self, n: usize) -> Option<&'a mut T> {
        let front_len = self.front.len();
        if n >= front_len {
            self.front.nth(front_len);
            self.back.nth(n - front_len)
        } else {
            self.front.nth(n)
        }
    }

    #[inline]
    fn fold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, &'a mut T) -> B,
    {
        let mut state = init;
        for item in self.front {
            state = f(state, item);
        }
        for item in self.back {
            state = f(state, item);
        }
        state
    }

    #[inline]
    fn for_each<F>(self, mut f: F)
    where
        F: FnMut(&'a mut T),
    {
        for item in self.front {
            f(item);
        }
        for item in self.back {
            f(item);
        }
    }

    #[inline]
    fn count(self) -> usize {
        self.front.len() + self.back.len()
    }

    #[inline]
    fn last(self) -> Option<&'a mut T> {
        self.back.last().or(self.front.last())
    }

    #[inline]
    fn all<F>(&mut self, mut f: F) -> bool
    where
        F: FnMut(&'a mut T) -> bool,
    {
        for item in &mut self.front {
            if !f(item) {
                return false;
            }
        }

        for item in &mut self.back {
            if !f(item) {
                return false;
            }
        }

        true
    }

    #[inline]
    fn any<F>(&mut self, mut f: F) -> bool
    where
        F: FnMut(&'a mut T) -> bool,
    {
        for item in &mut self.front {
            if f(item) {
                return true;
            }
        }

        for item in &mut self.back {
            if f(item) {
                return true;
            }
        }

        false
    }

    #[inline]
    fn find<P>(&mut self, mut predicate: P) -> Option<&'a mut T>
    where
        P: FnMut(&&'a mut T) -> bool,
    {
        self.front
            .find(&mut predicate)
            .or_else(|| self.back.find(predicate))
    }

    #[inline]
    fn position<P>(&mut self, mut predicate: P) -> Option<usize>
    where
        P: FnMut(&'a mut T) -> bool,
    {
        let mut pos = 0;
        for item in &mut self.front {
            if predicate(item) {
                return Some(pos);
            }
            pos += 1;
        }

        for item in &mut self.back {
            if predicate(item) {
                return Some(pos);
            }
            pos += 1;
        }

        None
    }

    #[inline]
    fn rposition<P>(&mut self, mut predicate: P) -> Option<usize>
    where
        P: FnMut(Self::Item) -> bool,
    {
        let mut pos = self.back.len() + self.front.len();
        for item in self.back.by_ref().rev() {
            pos -= 1;
            if predicate(item) {
                return Some(pos);
            }
        }

        for item in self.front.by_ref().rev() {
            pos -= 1;
            if predicate(item) {
                return Some(pos);
            }
        }

        None
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a mut T> {
        self.back.next_back().or_else(|| self.front.next_back())
    }

    #[inline]
    fn nth_back(&mut self, n: usize) -> Option<&'a mut T> {
        let back_len = self.back.len();
        if n >= back_len {
            self.back.nth_back(back_len);
            self.front.nth_back(n - back_len)
        } else {
            self.back.nth_back(n)
        }
    }

    #[inline]
    fn rfold<B, F>(self, init: B, mut f: F) -> B
    where
        F: FnMut(B, &'a mut T) -> B,
    {
        let mut state = init;
        for item in self.back.rev() {
            state = f(state, item);
        }
        for item in self.front.rev() {
            state = f(state, item);
        }
        state
    }

    #[inline]
    fn rfind<P>(&mut self, mut predicate: P) -> Option<&'a mut T>
    where
        P: FnMut(&&'a mut T) -> bool,
    {
        self.back
            .rfind(&mut predicate)
            .or_else(|| self.front.rfind(predicate))
    }
}

/// Yields removed elements in queue order and drops the remainder on drop.
pub struct Drain<'a, T> {
    ring_buffer: &'a mut RingBuffer<T>,
}

impl<T> Drop for Drain<'_, T> {
    fn drop(&mut self) {
        self.ring_buffer.clear();
    }
}

impl<T> Iterator for Drain<'_, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.ring_buffer.len == 0 {
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;

        self.ring_buffer.head = (self.ring_buffer.head + 1) % self.ring_buffer.array.len();
        self.ring_buffer.len -= 1;

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

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.ring_buffer.len;
        (len, Some(len))
    }

    #[inline]
    fn count(self) -> usize {
        self.ring_buffer.len
    }

    fn nth(&mut self, n: usize) -> Option<T> {
        if n == 0 {
            return self.next();
        }

        if self.ring_buffer.len <= n {
            self.ring_buffer.clear();
            // Queue is empty.
            return None;
        }

        let head = self.ring_buffer.head;
        let new_head = ring_index(head, n, self.ring_buffer.array.len());

        self.ring_buffer.head = new_head;
        self.ring_buffer.len -= n;

        let front_n = n.min(self.ring_buffer.array.len() - head);
        let back_n = n - front_n;
        let (front, back) = unsafe {
            // Cast `*mut MaybeUninit<T>` to `*mut T` is valid.
            let ptr = self.ring_buffer.array.as_mut_ptr().cast::<T>();
            let front_ptr = ptr.add(head);

            let front = slice_from_raw_parts_mut(front_ptr, front_n);
            let back = slice_from_raw_parts_mut(ptr, back_n);

            (front, back)
        };

        debug_assert_eq!(back.len() + front.len(), n);

        unsafe {
            drop_two_slices(front, back);
        }

        self.next()
    }
}

impl<T> ExactSizeIterator for Drain<'_, T> {
    #[inline]
    fn len(&self) -> usize {
        self.ring_buffer.len
    }
}

/// Drops two slices.
/// First drops `front`, then `back`.
///
/// Drops both even in case of panic.
unsafe fn drop_two_slices<T>(front: *mut [T], back: *mut [T]) {
    struct Dropper<T>(*mut [T]);

    impl<T> Drop for Dropper<T> {
        fn drop(&mut self) {
            unsafe {
                drop_in_place(self.0);
            }
        }
    }

    unsafe {
        let _back_dropped = Dropper(back);
        drop_in_place(front);
    }
}

#[inline]
pub(crate) fn ring_index(head: usize, index: usize, cap: usize) -> usize {
    debug_assert!(head < cap, "head must fit into the capacity");
    debug_assert!(index <= cap, "index must be less than or equal to capacity");

    let tail = cap - head;

    if tail > index {
        head + index
    } else {
        index - tail
    }
}

#[inline]
fn new_cap<T>(cap: usize) -> usize {
    match cap.checked_add(cap) {
        Some(new_cap) => new_cap.max(RingBuffer::<T>::MIN_NON_ZERO_CAP),
        None => capacity_overflow(),
    }
}

#[test]
fn iterate_wrapped_buffer() {
    let mut ring = RingBuffer::with_capacity(4);
    for value in 0..4 {
        ring.push(value);
    }
    assert_eq!(ring.pop(), Some(0));
    assert_eq!(ring.pop(), Some(1));
    ring.push(4);
    ring.push(5);

    let mut iter = ring.iter();
    assert_eq!(iter.next(), Some(&2));
    assert_eq!(iter.next_back(), Some(&5));
    assert_eq!(iter.next_back(), Some(&4));
    assert_eq!(iter.next(), Some(&3));
    assert_eq!(iter.next(), None);
    assert_eq!(ring.iter().nth(2), Some(&4));
    assert_eq!(ring.iter().nth_back(2), Some(&3));

    let mut iter = ring.iter_mut();
    assert_eq!(iter.next().as_deref(), Some(&2));
    assert_eq!(iter.next_back().as_deref(), Some(&5));
    assert_eq!(iter.next_back().as_deref(), Some(&4));
    assert_eq!(iter.next().as_deref(), Some(&3));
    assert_eq!(iter.next(), None);
    assert_eq!(ring.iter_mut().nth(2).as_deref(), Some(&4));
    assert_eq!(ring.iter_mut().nth_back(2).as_deref(), Some(&3));

    let mut iter = (&ring).into_iter();
    assert_eq!(iter.find(|&&value| value == 3), Some(&3));
    assert_eq!(iter.next(), Some(&4));
    let mut iter = ring.iter();
    assert_eq!(iter.rfind(|&&value| value == 3), Some(&3));
    assert_eq!(iter.next_back(), Some(&2));

    let mut iter = (&mut ring).into_iter();
    assert_eq!(iter.find(|value| **value == 4).as_deref(), Some(&4));
    assert_eq!(iter.next().as_deref(), Some(&5));
    let mut iter = ring.iter_mut();
    assert_eq!(iter.rfind(|value| **value == 4).as_deref(), Some(&4));
    assert_eq!(iter.next_back().as_deref(), Some(&3));
}
