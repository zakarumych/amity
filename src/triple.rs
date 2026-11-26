//! Contains wait-free cheap triple-buffer implementation.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::MaybeUninit,
    ops::Deref,
    sync::atomic::{AtomicU8, Ordering},
};

#[cfg(feature = "alloc")]
use alloc::borrow::ToOwned;

use crate::cache::CachePadded;

/// Triple buffer is synchronization primitive that allows to pass values
/// from single producer to single consumer with minimal synchronization overhead.
///
/// It can be viewed as a queue with fixed capacity.
///
/// One element is always belong to producer, one to consumer and one is pending.
///
/// When producer publishes new element, producer's element becomes pending and
/// pending element becomes producer's element.
///
/// When consumer consumes element, consumer's element becomes pending and pending
/// element becomes consumer's element.
/// It also learns whether producer published element for consumption.
///
/// Unlike queues, triple buffer allows mutable access to existing elements, allowing to
/// update their value instead of posting new instances.
/// This can be beneficial for performance when constructing new instances is expensive
/// and old values can be retrofitted or salvaged.
pub struct TripleBuffer<T> {
    /// Elements of the triple buffer.
    elements: [CachePadded<UnsafeCell<T>>; 3],
    flags: AtomicU8,
}

// SAFETY: Each endpoint owns a distinct slot. AcqRel exchanges transfer slot
// ownership between threads. Sharing a reference to an endpoint's payload
// requires T: Sync, enforced by its marker or the raw access contract.
unsafe impl<T: Send> Sync for TripleBuffer<T> {}

const FLAGS_INIT: u8 = 0b0000_0000;

const INDEX_MASK: u8 = 0b0000_0011;
const PENDING_BIT: u8 = 0b0000_0100;

/// Index of a slot in a triple buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Idx {
    /// First slot.
    #[default]
    Zero = 0,
    /// Second slot.
    One = 1,
    /// Third slot.
    Two = 2,
}

impl Idx {
    /// Create triple index from `u8` value.
    ///
    /// # Panics
    ///
    /// Panics if value is not 0, 1 or 2.
    #[inline]
    #[must_use]
    pub fn from_bits(value: u8) -> Self {
        match value {
            0 => Idx::Zero,
            1 => Idx::One,
            2 => Idx::Two,
            _ => panic!("Invalid index for triple buffer"),
        }
    }

    /// Create triple index from `u8` value without checking for bounds.
    ///
    /// # Safety
    ///
    /// The caller must ensure that value is 0, 1 or 2.
    #[inline]
    #[must_use]
    pub unsafe fn from_bits_unchecked(value: u8) -> Self {
        match value {
            0 => Idx::Zero,
            1 => Idx::One,
            2 => Idx::Two,
            _ => unsafe { core::hint::unreachable_unchecked() },
        }
    }

    /// Returns bits representation of the index.
    #[inline]
    #[must_use]
    pub fn bits(self) -> u8 {
        self as u8
    }

    /// Returns index as `usize`.
    #[inline]
    #[must_use]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Return other index.
    /// Call it again to get one another index.
    /// Calling it 3rd time will return the same index.
    #[inline]
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Idx::Zero => Idx::One,
            Idx::One => Idx::Two,
            Idx::Two => Idx::Zero,
        }
    }
}

impl<T> TripleBuffer<T> {
    /// Initializes slots zero, one, and two with `a`, `b`, and `c`, respectively.
    pub const fn new(a: T, b: T, c: T) -> Self {
        TripleBuffer {
            elements: [
                CachePadded(UnsafeCell::new(a)),
                CachePadded(UnsafeCell::new(b)),
                CachePadded(UnsafeCell::new(c)),
            ],
            flags: AtomicU8::new(FLAGS_INIT),
        }
    }
}

impl<T> Default for TripleBuffer<T>
where
    T: Default,
{
    fn default() -> Self {
        TripleBuffer {
            elements: [
                CachePadded(UnsafeCell::new(T::default())),
                CachePadded(UnsafeCell::new(T::default())),
                CachePadded(UnsafeCell::new(T::default())),
            ],
            flags: AtomicU8::new(FLAGS_INIT),
        }
    }
}

impl<T> TripleBuffer<MaybeUninit<T>> {
    /// Creates triple buffer with uninitialized elements.
    #[must_use]
    pub const fn uninit() -> Self {
        TripleBuffer {
            elements: [
                CachePadded(UnsafeCell::new(MaybeUninit::uninit())),
                CachePadded(UnsafeCell::new(MaybeUninit::uninit())),
                CachePadded(UnsafeCell::new(MaybeUninit::uninit())),
            ],
            flags: AtomicU8::new(FLAGS_INIT),
        }
    }
}

impl<T> TripleBuffer<T> {
    /// Checks if last published element was consumed.
    ///
    /// This will return true if no element was published yet.
    ///
    /// Producer may wish to not publish new element if previous one was not yet consumed.
    /// However this should not be used to "wait" for consumption
    /// as it will turn triple-buffer into bad mutex.
    /// Instead producer may simply skip publishing new element if previous one was not consumed.
    ///
    /// Note that result may be outdated by the time it is returned.
    #[must_use]
    pub fn consumed(&self) -> bool {
        let flags = self.flags.load(Ordering::Relaxed);
        flags & PENDING_BIT == 0
    }

    /// Checks if last published element was consumed.
    ///
    /// This will return true if no element was published yet.
    ///
    /// Producer may wish to not publish new element if previous one was not yet consumed.
    /// However this should not be used to "wait" for consumption
    /// as it will turn triple-buffer into bad mutex.
    /// Instead producer may simply skip publishing new element if previous one was not consumed.
    ///
    /// Uses mutable borrow to ensure that the value may not be outdated before method returns.
    #[must_use]
    pub fn consumed_mut(&mut self) -> bool {
        *self.flags.get_mut() & PENDING_BIT == 0
    }

    /// Returns current pending index.
    /// Producer and consumer indices are two other values in set {0, 1, 2}.
    ///
    /// Note that result may be outdated by the time it is returned.
    #[must_use]
    pub fn pending(&self) -> Idx {
        let flags = self.flags.load(Ordering::Relaxed);

        // SAFETY: Both lower bits are never set at the same time,
        // Making `flags & INDEX_MASK` to always be 0, 1 or 2, and never 3.
        unsafe { Idx::from_bits_unchecked(flags & INDEX_MASK) }
    }

    /// Returns current pending index.
    /// Producer and consumer indices are two other values in set {0, 1, 2}.
    ///
    /// Uses mutable borrow to ensure that the value may not be outdated before method returns.
    #[must_use]
    pub fn pending_mut(&mut self) -> Idx {
        let flags = *self.flags.get_mut();

        // SAFETY: Both lower bits are never set at the same time,
        // Making `flags & INDEX_MASK` to always be 0, 1 or 2, and never 3.
        unsafe { Idx::from_bits_unchecked(flags & INDEX_MASK) }
    }

    /// Release current producer's element and mark it as pending.
    /// Returns new producer index and previous published element was consumed.
    ///
    /// This method must be called AFTER writing to current publisher's element.
    /// It will make it available for consumer to read.
    ///
    /// If false is returned, it means that element was not consumed (and won't be until it is republished),
    /// and producer may wish to not overwrite it.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot and end all
    /// borrows of it before the exchange. The returned index becomes the
    /// caller's producer slot.
    #[must_use]
    pub unsafe fn publish(&self, producer: Idx) -> (Idx, bool) {
        // We Release the element to the consumer
        // and Acquire current pending element for producer.
        // Thus AcqRel ordering is used.
        let flags = self
            .flags
            .swap(producer.bits() | PENDING_BIT, Ordering::AcqRel);

        let new_producer = unsafe { Idx::from_bits_unchecked(flags & INDEX_MASK) };
        let consumed = flags & PENDING_BIT == 0;

        (new_producer, consumed)
    }

    /// Acquires current pending element for the consumer and marks consumer's element as pending.
    /// Returns new consumer index and whether element was published after last consumption.
    ///
    /// This method must be called BEFORE reading from new current consumer's element.
    /// It will mark last published element as consumed.
    ///
    /// A false result means there was no new publication. The slots are
    /// exchanged in either case.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's consumer slot and end all
    /// borrows of it before the exchange. The returned index becomes the
    /// caller's consumer slot.
    #[must_use]
    pub unsafe fn consume(&self, consumer: Idx) -> (Idx, bool) {
        let flags = self.flags.swap(consumer.bits(), Ordering::AcqRel);

        let new_consumer = unsafe { Idx::from_bits_unchecked(flags & INDEX_MASK) };
        let published = flags & PENDING_BIT != 0;

        (new_consumer, published)
    }

    /// Returns mutable reference to the specified element.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own the slot and retain exclusive access
    /// for the lifetime of the returned reference.
    #[must_use]
    #[allow(
        clippy::mut_from_ref,
        reason = "the caller owns the slot inside UnsafeCell"
    )]
    pub unsafe fn get_unchecked_mut(&self, index: Idx) -> &mut T {
        unsafe { &mut *self.elements.get_unchecked(index.index()).get() }
    }

    /// Returns shared reference to the specified element.
    ///
    /// # Safety
    ///
    /// The caller must keep the slot borrowed for the reference's lifetime.
    /// Concurrent shared access from multiple threads requires `T: Sync`.
    #[must_use]
    pub unsafe fn get_unchecked(&self, index: Idx) -> &T {
        unsafe { &*self.elements.get_unchecked(index.index()).get() }
    }

    /// Sets the producer value to the triple buffer.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    pub unsafe fn set(&self, producer: Idx, value: T) {
        unsafe {
            *self.get_unchecked_mut(producer) = value;
        }
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    pub unsafe fn set_from(&self, producer: Idx, value: &T)
    where
        T: Clone,
    {
        unsafe {
            self.get_unchecked_mut(producer).clone_from(value);
        }
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    #[cfg(feature = "alloc")]
    pub unsafe fn set_from_borrow<U>(&self, producer: Idx, value: &U)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        unsafe {
            value.clone_into(self.get_unchecked_mut(producer));
        }
    }

    /// Sends the value to the triple buffer.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    /// On return the producer owns the returned index.
    #[must_use]
    pub unsafe fn send(&self, producer: Idx, value: T) -> (Idx, bool) {
        unsafe {
            *self.get_unchecked_mut(producer) = value;
        }
        unsafe { self.publish(producer) }
    }

    /// Sends the value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    /// On return the producer owns the returned index.
    #[must_use]
    pub unsafe fn send_from(&self, producer: Idx, value: &T) -> (Idx, bool)
    where
        T: Clone,
    {
        unsafe {
            self.get_unchecked_mut(producer).clone_from(value);
        }
        unsafe { self.publish(producer) }
    }

    /// Sends the value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// The caller must exclusively own this buffer's producer slot with no
    /// outstanding borrows and retain exclusive producer access during the call.
    /// On return the producer owns the returned index.
    #[cfg(feature = "alloc")]
    pub unsafe fn send_from_borrow<U>(&self, producer: Idx, value: &U) -> (Idx, bool)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        unsafe {
            value.clone_into(self.get_unchecked_mut(producer));
        }
        unsafe { self.publish(producer) }
    }

    /// Reads the consumer value from the triple buffer.
    ///
    /// # Safety
    ///
    /// The caller must hold a shared borrow of the consumer slot throughout
    /// the call. Concurrent shared access from multiple threads requires `T: Sync`.
    #[must_use]
    pub unsafe fn read(&self, consumer: Idx) -> T
    where
        T: Clone,
    {
        unsafe { self.get_unchecked(consumer).clone() }
    }

    /// Reads the consumer value from the triple buffer into `value`.
    ///
    /// # Safety
    ///
    /// The caller must hold a shared borrow of the consumer slot throughout
    /// the call. Concurrent shared access from multiple threads requires `T: Sync`.
    pub unsafe fn read_into(&self, consumer: Idx, value: &mut T)
    where
        T: Clone,
    {
        unsafe { value.clone_from(self.get_unchecked(consumer)) }
    }

    /// Receives the value from the triple buffer.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// The caller must meet the slot ownership requirements of [`Self::consume`].
    /// The exchange releases the input slot even if payload handling panics.
    /// On return the consumer owns the returned index.
    #[must_use]
    pub unsafe fn recv(&self, consumer: Idx) -> (Idx, Option<T>)
    where
        T: Default,
    {
        let (new_consumer, produced) = unsafe { self.consume(consumer) };

        if produced {
            unsafe {
                let value = core::mem::take(self.get_unchecked_mut(new_consumer));
                (new_consumer, Some(value))
            }
        } else {
            (new_consumer, None)
        }
    }

    /// Receives the value from the triple buffer.
    /// Clones the value into `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// The caller must meet the slot ownership requirements of [`Self::consume`].
    /// The exchange releases the input slot even if payload handling panics.
    /// On return the consumer owns the returned index.
    #[must_use]
    pub unsafe fn recv_into(&self, consumer: Idx, value: &mut T) -> (Idx, bool)
    where
        T: Clone,
    {
        let (new_consumer, produced) = unsafe { self.consume(consumer) };

        if produced {
            unsafe {
                value.clone_from(self.get_unchecked(new_consumer));
                (new_consumer, true)
            }
        } else {
            (new_consumer, false)
        }
    }
}

impl<T> TripleBuffer<Option<T>> {
    /// Creates triple buffer with `None`-initialized elements.
    pub const fn new_none() -> Self {
        TripleBuffer {
            elements: [
                CachePadded(UnsafeCell::new(None)),
                CachePadded(UnsafeCell::new(None)),
                CachePadded(UnsafeCell::new(None)),
            ],
            flags: AtomicU8::new(FLAGS_INIT),
        }
    }
}

/// Producer handle. Sharing a handle between threads requires a shareable payload.
///
/// ```compile_fail
/// use amity::triple::TripleBuffer;
/// use std::cell::Cell;
/// fn require_sync<T: Sync>(_: &T) {}
/// let mut buffer = TripleBuffer::<Cell<u32>>::default();
/// let (producer, _) = buffer.split_mut();
/// require_sync(&producer);
/// ```
pub struct TripleBufferProducer<T, B> {
    buffer: B,
    producer: Idx,
    last_consumed: bool,
    marker: PhantomData<(T, fn(T))>,
}

impl<T, B> TripleBufferProducer<T, B>
where
    B: Deref<Target = TripleBuffer<T>>,
{
    /// Returns reference to the producer's element.
    /// This method can be used to inspect the value of the element
    /// before publishing it.
    #[must_use]
    pub fn get(&self) -> &T {
        // SAFETY: Only this instance owns the producer index.
        unsafe { self.buffer.get_unchecked(self.producer) }
    }

    /// Returns mutable reference to the producer's element.
    /// This method can be used to update the value of the element
    /// before publishing it.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: Only this instance owns the producer index.
        unsafe { self.buffer.get_unchecked_mut(self.producer) }
    }

    /// Sets the producer value to the triple buffer.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    pub unsafe fn set(&mut self, value: T) {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            self.buffer.set(self.producer, value);
        }
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    pub unsafe fn set_from(&mut self, value: &T)
    where
        T: Clone,
    {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            self.buffer.set_from(self.producer, value);
        }
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    #[cfg(feature = "alloc")]
    pub unsafe fn set_from_borrow<U>(&mut self, value: &U)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            self.buffer.set_from_borrow(self.producer, value);
        }
    }

    /// Publishes producer's element and marks it as pending.
    /// Returns whether consumer consumed last published element.
    pub fn publish(&mut self) -> bool {
        // SAFETY: Only this instance owns the producer index.
        let (new_producer, consumed) = unsafe { self.buffer.publish(self.producer) };
        self.last_consumed = consumed;
        self.producer = new_producer;
        consumed
    }

    /// Sets the producer value to the triple buffer.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    pub unsafe fn send(&mut self, value: T) -> bool {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            self.buffer.set(self.producer, value);
        }
        self.publish()
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    pub unsafe fn send_from(&mut self, value: &T) -> bool
    where
        T: Clone,
    {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            self.buffer.set_from(self.producer, value);
        }
        self.publish()
    }

    /// Sets the producer value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// # Safety
    ///
    /// This handle owns the producer slot. Any raw borrows of that slot must
    /// end before this call.
    #[cfg(feature = "alloc")]
    pub unsafe fn send_from_borrow<U>(&mut self, value: &U) -> bool
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        // SAFETY: Only this instance owns the producer index.
        unsafe {
            let (producer, consumed) = self.buffer.send_from_borrow(self.producer, value);
            self.last_consumed = consumed;
            self.producer = producer;
            consumed
        }
    }

    /// Returns whether last published element was consumed.
    /// This will return true if no element was published yet.
    #[must_use]
    pub fn consumed(&self) -> bool {
        self.buffer.consumed()
    }

    /// Returns true if current producer's element was previously consumed.
    /// This will return true if this element was not yet published.
    ///
    /// Producer may wish to not overwrite this element or update differently.
    ///
    /// This is the same value that was returned by last call to `publish` method.
    #[must_use]
    pub fn last_consumed(&self) -> bool {
        self.last_consumed
    }
}

/// Consumer handle. Sharing a handle between threads requires a shareable payload.
///
/// ```compile_fail
/// use amity::triple::TripleBuffer;
/// use std::cell::Cell;
/// fn require_sync<T: Sync>(_: &T) {}
/// let mut buffer = TripleBuffer::<Cell<u32>>::default();
/// let (_, consumer) = buffer.split_mut();
/// require_sync(&consumer);
/// ```
pub struct TripleBufferConsumer<T, B> {
    buffer: B,
    consumer: Idx,
    marker: PhantomData<(T, fn(T))>,
}

impl<T, B> TripleBufferConsumer<T, B>
where
    B: Deref<Target = TripleBuffer<T>>,
{
    /// Returns reference to the consumer's element.
    /// This method can be used to inspect the value of the element after consuming it.
    #[must_use]
    pub fn get(&self) -> &T {
        // Safety: consumer index is kept correct.
        unsafe { self.buffer.get_unchecked(self.consumer) }
    }

    /// Returns mutable reference to the consumer's element.
    /// This method can be used to take the value of the element after consuming it.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: consumer index is kept correct.
        unsafe { self.buffer.get_unchecked_mut(self.consumer) }
    }

    /// Reads the consumer value from the triple buffer.
    ///
    /// # Safety
    ///
    /// This method accesses consumer element
    /// while only borrows the buffer immutably.
    /// Caller must ensure that one element is not accessed mutably elsewhere during this call.
    #[must_use]
    pub unsafe fn read(&self) -> T
    where
        T: Clone,
    {
        unsafe { self.buffer.read(self.consumer) }
    }

    /// Reads the consumer value from the triple buffer into `value`.
    ///
    /// # Safety
    ///
    /// This method accesses consumer element
    /// while only borrows the buffer immutably.
    /// Caller must ensure that one element is not accessed mutably elsewhere during this call.
    pub unsafe fn read_into(&self, value: &mut T)
    where
        T: Clone,
    {
        unsafe { self.buffer.read_into(self.consumer, value) }
    }

    /// Receives the value from the triple buffer.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// This method mutably accesses consumer element
    /// while only borrows the buffer immutably.
    /// Caller must ensure that one element is not accessed elsewhere during this call.
    #[must_use]
    pub unsafe fn recv(&mut self) -> Option<T>
    where
        T: Default,
    {
        self.consume().then(|| core::mem::take(self.get_mut()))
    }

    /// Receives the value from the triple buffer.
    /// Clones the value into `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    ///
    /// # Safety
    ///
    /// This method mutably accesses consumer element
    /// while only borrows the buffer immutably.
    /// Caller must ensure that one element is not accessed elsewhere during this call.
    #[must_use]
    pub unsafe fn recv_into(&mut self, value: &mut T) -> bool
    where
        T: Clone,
    {
        let published = self.consume();
        if published {
            value.clone_from(self.get());
        }
        published
    }

    /// Consumes next element and marks previous as pending.
    pub fn consume(&mut self) -> bool {
        // Safety: consumer index is kept correct.
        let (new_consumer, published) = unsafe { self.buffer.consume(self.consumer) };
        self.consumer = new_consumer;
        published
    }

    /// Returns whether element was published since last consumption.
    #[must_use]
    pub fn published(&self) -> bool {
        !self.buffer.consumed()
    }
}

#[cfg(feature = "alloc")]
impl<T> TripleBuffer<T> {
    /// Split the triple buffer into producer and consumer.
    /// This method consumes buffer, wraps it into `Arc` and returns producer and consumer that share it.
    #[must_use]
    pub fn split_arc(
        mut self,
    ) -> (
        TripleBufferProducer<T, alloc::sync::Arc<Self>>,
        TripleBufferConsumer<T, alloc::sync::Arc<Self>>,
    ) {
        let pending = self.pending_mut();
        let producer = pending.other();
        let consumer = producer.other();

        let arc = alloc::sync::Arc::new(self);
        let producer = TripleBufferProducer {
            buffer: arc.clone(),
            producer,
            last_consumed: true,
            marker: PhantomData,
        };
        let consumer = TripleBufferConsumer {
            buffer: arc,
            consumer,
            marker: PhantomData,
        };
        (producer, consumer)
    }
}

impl<T> TripleBuffer<T> {
    /// Split the triple buffer into producer and consumer.
    /// This method takes mutable reference to buffer and returns
    /// producer and consumer that share immutable reference with same lifetime.
    #[must_use]
    pub fn split_mut(
        &mut self,
    ) -> (
        TripleBufferProducer<T, &Self>,
        TripleBufferConsumer<T, &Self>,
    ) {
        let pending = self.pending_mut();
        let producer = pending.other();
        let consumer = producer.other();

        debug_assert_ne!(pending, producer);
        debug_assert_ne!(pending, consumer);
        debug_assert_ne!(producer, consumer);

        let me = &*self;
        let producer = TripleBufferProducer {
            buffer: me,
            producer,
            last_consumed: true,
            marker: PhantomData,
        };
        let consumer = TripleBufferConsumer {
            buffer: me,
            consumer,
            marker: PhantomData,
        };
        (producer, consumer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_receive_uses_published_slot() {
        let buffer = TripleBuffer::new(10, 11, 12);
        let (mut producer, mut consumer) = (Idx::One, Idx::Two);
        unsafe {
            (producer, _) = buffer.publish(producer);
            let (next, value) = buffer.recv(consumer);
            consumer = next;
            assert_eq!(value, Some(11));
            let _ = buffer.send(producer, 24);
            let mut value = 0;
            assert!(buffer.recv_into(consumer, &mut value).1);
            assert_eq!(value, 24);
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn handle_receive_commits_slot_before_user_code() {
        use std::panic::{AssertUnwindSafe, catch_unwind};

        struct Value(u32);

        impl Default for Value {
            fn default() -> Self {
                panic!("default")
            }
        }
        impl Clone for Value {
            fn clone(&self) -> Self {
                panic!("clone")
            }
        }

        let mut buffer = TripleBuffer::new(Value(0), Value(1), Value(2));
        let (mut tx, mut rx) = buffer.split_mut();
        tx.publish();
        let result = catch_unwind(AssertUnwindSafe(|| unsafe { rx.recv() }));
        assert!(result.is_err());
        assert_eq!(rx.get().0, 1);
        *tx.get_mut() = Value(3);
        tx.publish();
        let result = catch_unwind(AssertUnwindSafe(|| unsafe { rx.recv_into(&mut Value(4)) }));
        assert!(result.is_err());
        assert_eq!(rx.get().0, 3);
    }

    #[cfg(feature = "std")]
    #[test]
    fn send_only_slots_cross_threads() {
        use core::cell::Cell;
        use std::thread;

        let mut buffer = TripleBuffer::<Cell<u32>>::default();
        let (mut tx, mut rx) = buffer.split_mut();
        thread::scope(|scope| {
            scope
                .spawn(move || {
                    tx.get().set(31);
                    tx.publish();
                })
                .join()
                .unwrap();
            assert!(rx.consume());
            assert_eq!(rx.get().get(), 31);
        });

        let (mut tx, mut rx) = buffer.split_arc();
        tx.get().set(41);
        tx.publish();
        let value = thread::spawn(move || {
            assert!(rx.consume());
            rx.get().get()
        })
        .join()
        .unwrap();
        assert_eq!(value, 41);
    }
}
