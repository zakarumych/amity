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

const FLAGS_INIT: u8 = 0b0000_0000;
const INDEX_MASK: u8 = 0b0000_0011;
const PENDING_BIT: u8 = 0b0000_0100;

impl<T> TripleBuffer<T> {
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
    pub fn consumed(&self) -> bool {
        let flags = self.flags.load(Ordering::Relaxed);
        let consumed = flags & PENDING_BIT == 0;
        consumed
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
    pub fn consumed_mut(&mut self) -> bool {
        *self.flags.get_mut() & PENDING_BIT == 0
    }

    /// Returns current pending index.
    /// Producer and consumer indices are two other values in set {0, 1, 2}.
    ///
    /// Note that result may be outdated by the time it is returned.
    pub fn pending(&self) -> u8 {
        let flags = self.flags.load(Ordering::Relaxed);
        flags & INDEX_MASK
    }

    /// Returns current pending index.
    /// Producer and consumer indices are two other values in set {0, 1, 2}.
    ///
    /// Uses mutable borrow to ensure that the value may not be outdated before method returns.
    pub fn pending_mut(&mut self) -> u8 {
        *self.flags.get_mut() & INDEX_MASK
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
    /// This method is unsafe because it allows to publish element to the consumer.
    /// The producer index must be correct.
    pub unsafe fn publish(&self, producer: u8) -> (u8, bool) {
        // We Release the element to the consumer
        // and Acquire current pending element for producer.
        // Thus AcqRel ordering is used.
        let flags = self.flags.swap(producer | PENDING_BIT, Ordering::AcqRel);

        let new_producer = flags & INDEX_MASK;
        let consumed = flags & PENDING_BIT == 0;

        (new_producer, consumed)
    }

    /// Acquires current pending element for the consumer and marks consumer's element as pending.
    /// Returns new consumer index and whether element was published after last consumption.
    ///
    /// This method must be called BEFORE reading from new current consumer's element.
    /// It will mark last published element as consumed.
    ///
    /// If false is returned, it means that element was published since last consumption,
    /// and consumer may wish to not read it.
    ///
    /// # Safety
    ///
    /// This method is unsafe because it allows to consume element from the producer.
    /// The consumer index must be correct.
    pub unsafe fn consume(&self, consumer: u8) -> (u8, bool) {
        let flags = self.flags.swap(consumer, Ordering::AcqRel);

        let new_consumer = flags & INDEX_MASK;
        let published = flags & PENDING_BIT != 0;

        (new_consumer, published)
    }

    /// Returns reference to the specified element.
    ///
    /// Although it may be currently borrowed mutably via `get_mut` method,
    /// this method alone may not cause mutable aliasing and thus safe.
    pub fn get(&self, index: usize) -> &T {
        unsafe { &*self.elements[index].0.get() }
    }

    /// Returns mutable reference to the specified element.
    ///
    /// It borrows the triple buffer mutably, so only one element can be accessed mutably.
    pub fn get_mut(&mut self, index: usize) -> &mut T {
        self.elements[index].0.get_mut()
    }

    /// Returns mutable reference to the specified element.
    ///
    /// # Safety
    ///
    /// This method does not check if the index is in bounds.
    /// Triple-check the index before calling this method.
    pub unsafe fn get_unchecked(&self, index: usize) -> &T {
        unsafe { &*self.elements.get_unchecked(index).get() }
    }

    /// Returns mutable reference to the specified element.
    ///
    /// # Safety
    ///
    /// This method does not check if the index is in bounds.
    /// Triple-check the index before calling this method.
    ///
    /// This method returns mutable reference to element.
    /// while only borrows the triple buffer immutably.
    /// Caller must ensure that one element is not accessed elsewhere while this reference is alive.
    pub unsafe fn get_unchecked_mut(&self, index: usize) -> &mut T {
        unsafe { &mut *self.elements.get_unchecked(index).get() }
    }

    /// Sends the value to the triple buffer.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    pub unsafe fn send(&self, producer: u8, value: T) -> (u8, bool) {
        unsafe {
            *self.get_unchecked_mut(producer as usize) = value;
        }
        unsafe { self.publish(producer) }
    }

    /// Sends the value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    pub unsafe fn send_from(&self, producer: u8, value: &T) -> (u8, bool)
    where
        T: Clone,
    {
        unsafe {
            self.get_unchecked_mut(producer as usize).clone_from(value);
        }
        unsafe { self.publish(producer) }
    }

    /// Sends the value to the triple buffer.
    /// Clones the value from `value`.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    #[cfg(feature = "alloc")]
    pub unsafe fn send_from_borrow<U>(&self, producer: u8, value: &U) -> (u8, bool)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        unsafe {
            value.clone_into(self.get_unchecked_mut(producer as usize));
        }
        unsafe { self.publish(producer) }
    }

    /// Receives the value from the triple buffer.
    ///
    /// Triple buffer acts as a queue with 1 element capacity.
    pub unsafe fn recv(&self, consumer: u8) -> (u8, Option<T>)
    where
        T: Default,
    {
        let (new_consumer, produced) = unsafe { self.consume(consumer) };

        if produced {
            unsafe {
                let value = core::mem::take(self.get_unchecked_mut(consumer as usize));
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
    pub unsafe fn recv_into(&self, consumer: u8, value: &mut T) -> (u8, bool)
    where
        T: Clone,
    {
        let (new_consumer, produced) = unsafe { self.consume(consumer) };

        if produced {
            unsafe {
                value.clone_from(self.get_unchecked_mut(consumer as usize));
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

pub struct TripleBufferProducer<T, B> {
    buffer: B,
    producer: u8,
    last_consumed: bool,
    marker: PhantomData<fn(T) -> T>,
}

impl<T, B> TripleBufferProducer<T, B>
where
    B: Deref<Target = TripleBuffer<T>>,
{
    /// Returns reference to the producer's element.
    /// This method can be used to inspect the value of the element
    /// before publishing it.
    pub fn get(&self) -> &T {
        // Safety: producer index is kept correct.
        unsafe { self.buffer.get_unchecked(self.producer as usize) }
    }

    /// Returns mutable reference to the producer's element.
    /// This method can be used to update the value of the element
    /// before publishing it.
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: producer index is kept correct.
        unsafe { self.buffer.get_unchecked_mut(self.producer as usize) }
    }

    /// Clones `value` into producer's element.
    pub fn clone_from<U>(&mut self, value: &T)
    where
        T: Clone,
    {
        self.get_mut().clone_from(value);
    }

    /// Clones `value` into producer's element.
    #[cfg(feature = "alloc")]
    pub fn clone_from_borrow<U>(&mut self, value: &U)
    where
        U: alloc::borrow::ToOwned<Owned = T> + ?Sized,
    {
        value.clone_into(self.get_mut());
    }

    /// Publishes producer's element and marks it as pending.
    /// Returns whether consumer consumed last published element.
    pub fn publish(&mut self) -> bool {
        // Safety: producer index is kept correct.
        let (new_producer, consumed) = unsafe { self.buffer.publish(self.producer) };
        self.last_consumed = consumed;
        self.producer = new_producer;
        consumed
    }

    /// Returns whether last published element was consumed.
    /// This will return true if no element was published yet.
    pub fn consumed(&self) -> bool {
        self.buffer.consumed()
    }

    /// Returns true if current producer's element was previously consumed.
    /// This will return true if this element was not yet published.
    ///
    /// Producer may wish to not overwrite this element or update differently.
    ///
    /// This is the same value that was returned by last call to `publish` method.
    pub fn last_consumed(&self) -> bool {
        self.last_consumed
    }
}

pub struct TripleBufferConsumer<T, B> {
    buffer: B,
    consumer: u8,
    marker: PhantomData<fn(T) -> T>,
}

impl<T, B> TripleBufferConsumer<T, B>
where
    B: Deref<Target = TripleBuffer<T>>,
{
    /// Returns reference to the consumer's element.
    /// This method can be used to inspect the value of the element after consuming it.
    pub fn get(&self) -> &T {
        // Safety: consumer index is kept correct.
        unsafe { self.buffer.get_unchecked(self.consumer as usize) }
    }

    /// Returns mutable reference to the consumer's element.
    /// This method can be used to take the value of the element after consuming it.
    pub fn get_mut(&mut self) -> &mut T {
        // Safety: consumer index is kept correct.
        unsafe { self.buffer.get_unchecked_mut(self.consumer as usize) }
    }

    /// Consumes next element and marks previous as pending.
    pub fn consume(&mut self) -> bool {
        // Safety: consumer index is kept correct.
        let (new_consumer, published) = unsafe { self.buffer.consume(self.consumer) };
        self.consumer = new_consumer;
        published
    }

    /// Returns whether element was published since last consumption.
    pub fn published(&self) -> bool {
        !self.buffer.consumed()
    }
}

#[cfg(feature = "alloc")]
impl<T> TripleBuffer<T> {
    /// Split the triple buffer into producer and consumer.
    /// This method consumes buffer, wraps it into `Arc` and returns producer and consumer that share it.
    pub fn split_arc(
        mut self,
    ) -> (
        TripleBufferProducer<T, alloc::sync::Arc<Self>>,
        TripleBufferConsumer<T, alloc::sync::Arc<Self>>,
    ) {
        let pending = self.pending_mut();

        let arc = alloc::sync::Arc::new(self);
        let producer = TripleBufferProducer {
            buffer: arc.clone(),
            producer: (pending + 1) % 3,
            last_consumed: true,
            marker: PhantomData,
        };
        let consumer = TripleBufferConsumer {
            buffer: arc,
            consumer: (pending + 2) % 3,
            marker: PhantomData,
        };
        (producer, consumer)
    }
}

impl<T> TripleBuffer<T> {
    /// Split the triple buffer into producer and consumer.
    /// This method takes mutable reference to buffer and returns
    /// producer and consumer that share immutable reference with same lifetime.
    pub fn split_mut<'a>(
        &'a mut self,
    ) -> (
        TripleBufferProducer<T, &'a Self>,
        TripleBufferConsumer<T, &'a Self>,
    ) {
        let pending = self.pending_mut();

        let me = &*self;
        let producer = TripleBufferProducer {
            buffer: me,
            producer: (pending + 1) % 3,
            last_consumed: true,
            marker: PhantomData,
        };
        let consumer = TripleBufferConsumer {
            buffer: me,
            consumer: (pending + 2) % 3,
            marker: PhantomData,
        };
        (producer, consumer)
    }
}
