//! Provides [`Sender`] and [`Receiver`] - broadcasting channel.
//!
//! Readers observe the latest publication. Each read holds a shared snapshot
//! for its callback, which may publish and read again on the same channel.
//! An in-place write clones a recycled value if a reader still holds it.
//!
//! # Example
//!
//! ```rust
//! use amity::broad::{Receiver, Sender};
//!
//! fn main() {
//!     // Create a new broadcast channel with an initial value
//!     let mut tx = Sender::new(0u32);
//!     let mut rx = tx.receiver();
//!
//!     // Sender sends a new value
//!     tx.send(42);
//!
//!     // Receiver receives the new value
//!     if let Some(value) = rx.recv() {
//!         println!("Received value: {}", value);
//!     }
//! }
//! ```

use alloc::{borrow::ToOwned, sync::Arc};

use lock_api::{RawRwLock, RwLock};

use crate::triple::{Idx, TripleBuffer};

/// A single-writer channel that shares the latest value with multiple readers.
pub struct Broadcast<T, L = crate::DefaultRawRwLock> {
    buffer: TripleBuffer<(Arc<T>, u64)>,
    consumer: RwLock<L, Idx>,
    make_mut: fn(&mut Arc<T>) -> &mut T,
}

// Internal snapshots borrow the channel for their entire lifetime. Moving the
// channel therefore transfers exclusive ownership of every payload allocation.
unsafe impl<T: Send, L: Send> Send for Broadcast<T, L> {}

impl<T, L> Broadcast<T, L>
where
    L: RawRwLock,
{
    /// Update the consumer, fetch consumer value reference,
    /// call provided function with the value and whether it was updated.
    /// The callback holds a snapshot and may read this channel again.
    /// The receiver version advances after the callback returns successfully.
    pub fn read<R>(&self, current: &mut u64, f: impl FnOnce(&T, bool) -> R) -> R {
        let (value, version) = if self.buffer.consumed() {
            let read = self.consumer.read();
            // The guard pins the consumer slot until its snapshot is cloned.
            unsafe { self.buffer.get_unchecked(*read).clone() }
        } else {
            let mut write = self.consumer.write();
            if !self.buffer.consumed() {
                let (new_consumer, _) = unsafe { self.buffer.consume(*write) };
                *write = new_consumer;
            }
            unsafe { self.buffer.get_unchecked(*write).clone() }
        };
        let updated = version > *current;
        let result = f(&value, updated);
        if updated {
            *current = version;
        }
        result
    }

    /// Receive new value if it was set since last receive.
    #[inline]
    #[must_use]
    pub fn recv(&self, current: &mut u64) -> Option<T>
    where
        T: Clone,
    {
        self.read(
            current,
            |value, updated| {
                if updated { Some(value.clone()) } else { None }
            },
        )
    }

    /// Receive new value if it was set since last receive and clone it into `value`.
    /// Returns `true` if the value was updated, `false` otherwise.
    #[inline]
    pub fn recv_into(&self, current: &mut u64, value: &mut T) -> bool
    where
        T: Clone,
    {
        self.read(current, |buffer, updated| {
            if updated {
                value.clone_from(buffer);
                true
            } else {
                false
            }
        })
    }

    /// Returns last set value.
    #[inline]
    #[must_use]
    pub fn last(&self, current: &mut u64) -> T
    where
        T: Clone,
    {
        self.read(current, |value, _| value.clone())
    }

    /// Updates `value` with the last set value.
    #[inline]
    pub fn last_into(&self, current: &mut u64, value: &mut T)
    where
        T: Clone,
    {
        self.read(current, |buffer, _| value.clone_from(buffer));
    }

    /// Calls provided function with mutable reference to the value.
    /// Bumps version and publishes the value.
    /// If a reader still holds the recycled slot's value, clones it before mutation.
    ///
    /// # Panics
    ///
    /// Panics when the publication version is exhausted.
    ///
    /// # Safety
    ///
    /// The caller must hold this channel's unique producer index and provide its
    /// current publication version. All writes, including reentrant writes, must
    /// be serialized. On return, use the updated index and version for the next write.
    ///
    /// Use `Sender` for safe usage.
    pub unsafe fn write<R>(
        &self,
        producer: &mut Idx,
        current: &mut u64,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        unsafe { self.write_slot(producer, current, |slot| f((self.make_mut)(slot))) }
    }

    unsafe fn write_slot<R>(
        &self,
        producer: &mut Idx,
        current: &mut u64,
        f: impl FnOnce(&mut Arc<T>) -> R,
    ) -> R {
        let next = current.checked_add(1).expect("broadcast version exhausted");
        let (slot, version) = unsafe { self.buffer.get_unchecked_mut(*producer) };
        let result = f(slot);
        *version = next;
        *current = next;
        (*producer, _) = unsafe { self.buffer.publish(*producer) };
        result
    }

    /// Send new value to all receivers.
    /// If you need to clone the value to use this method, consider using [`send_from`](Self::send_from) instead.
    ///
    /// # Safety
    ///
    /// The caller must hold this channel's unique producer index and provide its
    /// current publication version. All writes, including reentrant writes, must
    /// be serialized. On return, use the updated index and version for the next write.
    ///
    /// Use `Sender` for safe usage.
    #[inline]
    pub unsafe fn send(&self, producer: &mut Idx, current: &mut u64, value: T) {
        unsafe {
            self.write_slot(producer, current, move |slot| {
                if let Some(buffer) = Arc::get_mut(slot) {
                    *buffer = value;
                } else {
                    *slot = Arc::new(value);
                }
            });
        }
    }

    /// Send new value to all receivers.
    ///
    /// Clones value from `value` into the buffer.
    /// This is especially useful for types that are expensive to clone, like `String` or `Vec`, or structs containing them.
    /// since they can reuse resources from previous values.
    ///
    /// # Safety
    ///
    /// The caller must hold this channel's unique producer index and provide its
    /// current publication version. All writes, including reentrant writes, must
    /// be serialized. On return, use the updated index and version for the next write.
    ///
    /// Use `Sender` for safe usage.
    #[inline]
    pub unsafe fn send_from(&self, producer: &mut Idx, current: &mut u64, value: &T)
    where
        T: Clone,
    {
        unsafe {
            self.write(producer, current, move |buffer| {
                buffer.clone_from(value);
            });
        }
    }

    /// Send new value to all receivers.
    ///
    /// Converts value to owned type using `ToOwned` trait.
    /// This is especially useful for types that are expensive to clone, like `String` or `Vec`, or structs containing them.
    /// since they can reuse resources from previous values.
    ///
    /// # Safety
    ///
    /// The caller must hold this channel's unique producer index and provide its
    /// current publication version. All writes, including reentrant writes, must
    /// be serialized. On return, use the updated index and version for the next write.
    ///
    /// Use `Sender` for safe usage.
    #[inline]
    pub unsafe fn send_from_borrow<U>(&self, producer: &mut Idx, current: &mut u64, value: &U)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        unsafe {
            self.write(producer, current, move |buffer| {
                value.clone_into(buffer);
            });
        }
    }

    /// Creates a new broadcasting channel with the given initial value.
    #[must_use]
    pub fn new(initial: T) -> (Self, u64, Idx)
    where
        T: Clone,
    {
        let version = 0;
        let mut buffer = TripleBuffer::new(
            (Arc::new(initial.clone()), version),
            (Arc::new(initial.clone()), version),
            (Arc::new(initial), version),
        );
        let producer = buffer.pending_mut().other();
        let consumer = producer.other();

        let broadcast = Broadcast {
            buffer,
            consumer: RwLock::new(consumer),
            make_mut: Arc::make_mut,
        };

        (broadcast, version, producer)
    }

    /// Converts the channel into [`Sender`].
    ///
    /// # Panics
    ///
    /// Panics if `producer` identifies the pending or consumer slot,
    /// or if `version` precedes the latest publication.
    #[inline]
    #[must_use]
    pub fn into_sender(mut self, producer: Idx, version: u64) -> Sender<T, L> {
        let pending = self.buffer.pending_mut();
        let consumer = *self.consumer.get_mut();
        assert_ne!(producer, pending, "pending slot belongs to the channel");
        assert_ne!(producer, consumer, "consumer slot belongs to the readers");
        let latest = if self.buffer.consumed_mut() {
            consumer
        } else {
            pending
        };
        // The channel is exclusively owned during conversion.
        let latest_version = unsafe { self.buffer.get_unchecked(latest) }.1;
        assert!(
            version >= latest_version,
            "version precedes the latest publication"
        );
        Sender {
            broadcast: Arc::new(self),
            producer,
            version,
        }
    }
}

/// A reader that tracks the last publication it observed.
pub struct Receiver<T, L = crate::DefaultRawRwLock> {
    broadcast: Arc<Broadcast<T, L>>,
    version: u64,
}

impl<T, L> Clone for Receiver<T, L> {
    #[inline]
    fn clone(&self) -> Self {
        Receiver {
            broadcast: self.broadcast.clone(),
            version: self.version,
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.broadcast.clone_from(&source.broadcast);
        self.version = source.version;
    }
}

impl<T, L> Receiver<T, L>
where
    L: RawRwLock,
{
    /// Read using provided function.
    #[inline]
    pub fn read<R>(&mut self, f: impl FnOnce(&T, bool) -> R) -> R {
        self.broadcast.read(&mut self.version, f)
    }

    /// Receive new value if it was set since last receive.
    #[inline]
    pub fn recv(&mut self) -> Option<T>
    where
        T: Clone,
    {
        self.broadcast.recv(&mut self.version)
    }

    /// Receive new value if it was set since last receive and clone it into `value`.
    /// Returns `true` if the value was updated, `false` otherwise.
    #[inline]
    pub fn recv_into(&mut self, value: &mut T) -> bool
    where
        T: Clone,
    {
        self.broadcast.recv_into(&mut self.version, value)
    }

    /// Returns last set value.
    #[inline]
    pub fn last(&mut self) -> T
    where
        T: Clone,
    {
        self.broadcast.last(&mut self.version)
    }

    /// Updates `value` with the last set value.
    #[inline]
    pub fn last_into(&mut self, value: &mut T)
    where
        T: Clone,
    {
        self.broadcast.last_into(&mut self.version, value);
    }
}

/// The sole publisher for a broadcast channel.
pub struct Sender<T, L = crate::DefaultRawRwLock> {
    broadcast: Arc<Broadcast<T, L>>,
    producer: Idx,
    version: u64,
}

impl<T> Sender<T> {
    /// Creates a channel with the given initial value and the default lock.
    #[inline]
    #[must_use]
    pub fn new(initial: T) -> Self
    where
        T: Clone,
    {
        Self::with_lock(initial)
    }
}

impl<T, L> Sender<T, L>
where
    L: RawRwLock,
{
    /// Creates a channel with the given initial value and lock type `L`.
    #[inline]
    #[must_use]
    pub fn with_lock(initial: T) -> Self
    where
        T: Clone,
    {
        let (broadcast, version, producer) = Broadcast::new(initial);
        Sender {
            broadcast: Arc::new(broadcast),
            producer,
            version,
        }
    }

    /// Write using provided function
    /// and publish the updated value.
    ///
    /// # Panics
    ///
    /// Panics when the publication version is exhausted.
    #[inline]
    pub fn write<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        unsafe {
            self.broadcast
                .write(&mut self.producer, &mut self.version, f)
        }
    }

    /// Send new value to all receivers.
    /// If you need to clone the value to use this method, consider using [`send_from`](Self::send_from) instead.
    #[inline]
    pub fn send(&mut self, value: T) {
        unsafe {
            self.broadcast
                .send(&mut self.producer, &mut self.version, value);
        }
    }

    /// Send new value to all receivers.
    ///
    /// Clones value from `value` into the buffer.
    /// This is especially useful for types that are expensive to clone, like `String` or `Vec`, or structs containing them.
    /// since they can reuse resources from previous values.
    #[inline]
    pub fn send_from(&mut self, value: &T)
    where
        T: Clone,
    {
        unsafe {
            self.broadcast
                .send_from(&mut self.producer, &mut self.version, value);
        }
    }

    /// Send new value to all receivers.
    ///
    /// Converts value to owned type using `ToOwned` trait.
    /// This is especially useful for types that are expensive to clone, like `String` or `Vec`, or structs containing them.
    /// since they can reuse resources from previous values.
    #[inline]
    pub fn send_from_borrow<U>(&mut self, value: &U)
    where
        U: ToOwned<Owned = T> + ?Sized,
    {
        unsafe {
            self.broadcast
                .send_from_borrow(&mut self.producer, &mut self.version, value);
        }
    }

    /// Creates a new receiver for this channel.
    #[inline]
    #[must_use]
    pub fn receiver(&self) -> Receiver<T, L> {
        Receiver {
            broadcast: self.broadcast.clone(),
            version: 0,
        }
    }

    /// Creates a new cached for this channel.
    #[inline]
    #[must_use]
    pub fn cached(&self) -> Cached<T, L>
    where
        T: Clone,
    {
        Cached::new(self.receiver())
    }
}

/// `Receiver` paired with cached value.
///
/// This allows reading the value from cache and update when needed.
pub struct Cached<T, L = crate::DefaultRawRwLock> {
    get: Receiver<T, L>,
    local: T,
}

impl<T, L> Clone for Cached<T, L>
where
    T: Clone,
{
    #[inline]
    fn clone(&self) -> Self {
        Cached {
            get: self.get.clone(),
            local: self.local.clone(),
        }
    }
}

impl<T, L> Cached<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    /// Create a new broadcasting cache with the given initial value.
    #[inline]
    #[must_use]
    pub fn new(mut get: Receiver<T, L>) -> Self {
        let local = get.last();
        Cached { get, local }
    }

    /// Get the current value from the cache.
    #[inline]
    #[must_use]
    pub fn get(&self) -> &T {
        &self.local
    }

    /// Update the cache with the latest value from the broadcasting channel.
    /// Returns `true` if the value was updated, `false` otherwise.
    #[inline]
    pub fn update(&mut self) -> bool {
        self.get.recv_into(&mut self.local)
    }
}

impl<T, L> From<Receiver<T, L>> for Cached<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    #[inline]
    fn from(get: Receiver<T, L>) -> Self {
        Cached::new(get)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receivers_track_each_publication() {
        let mut sender = Sender::new(0);
        let mut receiver = sender.receiver();
        let mut cached = sender.cached();
        for value in 1..=12 {
            sender.send(value);
            assert_eq!(receiver.recv(), Some(value));
            assert_eq!(receiver.recv(), None);
            assert!(cached.update());
            assert_eq!(*cached.get(), value);
        }
    }

    #[test]
    fn read_callback_retains_its_snapshot_across_publications() {
        let mut sender = Sender::new(0);
        let mut outer = sender.receiver();
        let mut inner = sender.receiver();
        sender.send(1);
        outer.read(|snapshot, updated| {
            assert!(updated);
            // Check before reentry so a lock held across the callback fails promptly.
            assert!(inner.broadcast.consumer.try_write().is_some());
            sender.send(2);
            assert_eq!(inner.recv(), Some(2));
            sender.send(3);
            sender.write(|recycled| {
                assert_eq!(*recycled, 1);
                *recycled = 4;
            });
            assert_eq!(inner.recv(), Some(4));
            assert_eq!(*snapshot, 1);
        });
    }

    #[test]
    #[should_panic(expected = "pending slot")]
    fn sender_rejects_pending_index() {
        let (broadcast, version, _) = Broadcast::<u32>::new(0);
        let pending = broadcast.buffer.pending();
        let _sender = broadcast.into_sender(pending, version);
    }

    #[test]
    #[should_panic(expected = "consumer slot")]
    fn sender_rejects_consumer_index() {
        let (mut broadcast, version, _) = Broadcast::<u32>::new(0);
        let consumer = *broadcast.consumer.get_mut();
        let _sender = broadcast.into_sender(consumer, version);
    }

    #[test]
    #[should_panic(expected = "version precedes")]
    fn sender_rejects_stale_version() {
        let (broadcast, mut version, mut producer) = Broadcast::<u32>::new(0);
        // This test holds the unique producer returned by the constructor.
        unsafe { broadcast.send(&mut producer, &mut version, 5) };
        let _sender = broadcast.into_sender(producer, 0);
    }

    #[cfg(feature = "std")]
    mod unwind {
        use super::Sender;
        use core::cell::Cell;
        use std::panic::{AssertUnwindSafe, catch_unwind};

        struct Payload<'a>(u32, &'a Cell<bool>);

        impl Clone for Payload<'_> {
            fn clone(&self) -> Self {
                assert!(!self.1.replace(false), "payload clone failed");
                Self(self.0, self.1)
            }

            fn clone_from(&mut self, source: &Self) {
                self.0 = u32::MAX;
                *self = source.clone();
            }
        }

        #[test]
        fn cache_retries_publication_after_clone_panics() {
            let fail = Cell::new(false);
            let mut sender = Sender::new(Payload(0, &fail));
            let mut cached = sender.cached();
            sender.send(Payload(7, &fail));

            fail.set(true);
            assert!(catch_unwind(AssertUnwindSafe(|| cached.update())).is_err());
            assert!(cached.update());
            assert_eq!(cached.get().0, 7);
            assert!(!cached.update());
        }

        #[test]
        fn failed_cache_clone_preserves_value_and_subscription() {
            let fail = Cell::new(false);
            let mut original = Sender::new(Payload(1, &fail));
            let mut cached = original.cached();
            let source = Sender::new(Payload(2, &fail)).cached();

            fail.set(true);
            assert!(catch_unwind(AssertUnwindSafe(|| cached.clone_from(&source))).is_err());
            assert_eq!(cached.get().0, 1);
            original.send(Payload(3, &fail));
            assert!(cached.update());
            assert_eq!(cached.get().0, 3);
        }
    }

    #[cfg(feature = "std")]
    #[test]
    fn exhausted_version_leaves_publication_intact() {
        let (broadcast, _, producer) = Broadcast::<u32>::new(0);
        let mut sender = broadcast.into_sender(producer, u64::MAX - 1);
        let mut receiver = sender.receiver();
        sender.send(7);
        let called = core::cell::Cell::new(false);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sender.write(|_| called.set(true));
        }));
        assert!(result.is_err());
        assert!(!called.get());
        assert_eq!(receiver.recv(), Some(7));
    }

    #[cfg(feature = "std")]
    #[test]
    fn channels_cross_threads() {
        fn assert_send<T: Send>() {}
        assert_send::<Broadcast<core::cell::Cell<u32>>>();

        let mut sender = Sender::new(0);
        let mut receiver = sender.receiver();
        std::thread::scope(|scope| {
            scope.spawn(move || sender.send(1)).join().unwrap();
            assert_eq!(
                scope.spawn(move || receiver.recv()).join().unwrap(),
                Some(1)
            );
        });
    }
}
