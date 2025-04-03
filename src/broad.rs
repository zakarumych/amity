//! Provides [`BroadSet`] and [`BroadGet`] - broadcasting channel.
//!

use alloc::{borrow::ToOwned, sync::Arc};

use lock_api::{RawRwLock, RwLock};

use crate::triple::TripleBuffer;

pub struct Broadcast<T, L = crate::DefaultRawRwLock> {
    buffer: TripleBuffer<(T, u64)>,
    consumer: RwLock<L, u8>,
}

impl<T, L> Broadcast<T, L>
where
    L: RawRwLock,
{
    /// Update the consumer, fetch consumer value reference,
    /// call provided function with the value and whether it was updated.
    /// Returns the result of the function.
    pub fn read<R>(&self, current: &mut u64, f: impl FnOnce(&T, bool) -> R) -> R {
        if !self.buffer.consumed() {
            let mut write = self.consumer.write();
            if !self.buffer.consumed() {
                let (new_consumer, published) = unsafe { self.buffer.consume(*write) };
                assert!(published);
                *write = new_consumer;
            }

            let (value, version) = unsafe { self.buffer.get_unchecked(*write as usize) };

            if *version > *current {
                *current = *version;
                f(value, true)
            } else {
                f(value, false)
            }
        } else {
            let read = self.consumer.read();
            let (value, version) = unsafe { self.buffer.get_unchecked(*read as usize) };

            if *version > *current {
                *current = *version;
                f(value, true)
            } else {
                f(value, false)
            }
        }
    }

    /// Receive new value if it was set since last receive.
    #[inline]
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
        self.read(current, |buffer, _| value.clone_from(buffer))
    }

    /// Calls provided function with mutable reference to the value.
    /// Bumps version and publishes the value.
    ///
    /// # Safety
    ///
    /// This function is unsafe because only single producer is allowed to call this function at a time.
    ///
    /// Use `BroadSet` for safe usage.
    pub unsafe fn write<R>(
        &self,
        producer: &mut u8,
        current: &mut u64,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        let (buffer, version) = unsafe { self.buffer.get_unchecked_mut(*producer as usize) };

        let r = f(buffer);

        *current += 1;
        *version += *current;

        (*producer, _) = unsafe { self.buffer.publish(*producer) };
        r
    }

    /// Send new value to all receivers.
    /// If you need to clone the value to use this method, consider using [`send_from`](Self::send_from) instead.
    ///
    /// # Safety
    ///
    /// This function is unsafe because only single producer is allowed to call this function at a time.
    ///
    /// Use `BroadSet` for safe usage.
    #[inline]
    pub unsafe fn send(&self, producer: &mut u8, current: &mut u64, value: T) {
        unsafe {
            self.write(producer, current, move |buffer| {
                *buffer = value;
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
    /// This function is unsafe because only single producer is allowed to call this function at a time.
    ///
    /// Use `BroadSet` for safe usage.
    #[inline]
    pub unsafe fn send_from(&self, producer: &mut u8, current: &mut u64, value: &T)
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
    /// This function is unsafe because only single producer is allowed to call this function at a time.
    ///
    /// Use `BroadSet` for safe usage.
    #[inline]
    pub unsafe fn send_from_borrow<U>(&self, producer: &mut u8, current: &mut u64, value: &U)
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
    pub fn new(initial: T) -> (Self, u64, u8)
    where
        T: Clone,
    {
        let producer = 0;
        let consumer = 1;
        let version = 0;

        let broadcast = Broadcast {
            buffer: TripleBuffer::new(
                (initial.clone(), version),
                (initial.clone(), version),
                (initial.clone(), version),
            ),
            consumer: RwLock::new(consumer),
        };

        (broadcast, version, producer)
    }

    /// Splits the broadcasting channel into a sender and receiver.
    ///
    /// # Safety
    ///
    /// This function is unsafe because it requires the caller to ensure that producer index is correct.
    /// To safely create a sender and receiver, use [`new_split`](Self::new_split) instead.
    pub unsafe fn split(self, version: u64, producer: u8) -> (BroadSet<T, L>, BroadGet<T, L>) {
        let broadcast = Arc::new(self);
        (
            BroadSet {
                broadcast: broadcast.clone(),
                producer,
                version,
            },
            BroadGet {
                broadcast,
                version: 0,
            },
        )
    }

    /// Creates a new broadcasting channel with the given initial value and splits it into a sender and receiver.
    #[inline]
    pub fn new_split(initial: T) -> (BroadSet<T, L>, BroadGet<T, L>)
    where
        T: Clone,
    {
        let (broadcast, version, producer) = Self::new(initial);
        unsafe { broadcast.split(version, producer) }
    }
}

pub struct BroadGet<T, L = crate::DefaultRawRwLock> {
    broadcast: Arc<Broadcast<T, L>>,
    version: u64,
}

impl<T, L> Clone for BroadGet<T, L> {
    #[inline]
    fn clone(&self) -> Self {
        BroadGet {
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

impl<T, L> BroadGet<T, L>
where
    L: RawRwLock,
{
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
        self.broadcast.last_into(&mut self.version, value)
    }
}

pub struct BroadSet<T, L = crate::DefaultRawRwLock> {
    broadcast: Arc<Broadcast<T, L>>,
    producer: u8,
    version: u64,
}

impl<T, L> BroadSet<T, L>
where
    L: RawRwLock,
{
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
}

/// Creates a new broadcasting channel with the given initial value.
///
/// # Example
///
/// ```rust
/// use amity::broad::{broadcast, BroadGet, BroadSet};
///
/// fn main() {
///     // Create a new broadcast channel with an initial value
///     let (mut sender, mut receiver) = broadcast(0u32);
///
///     // Sender sends a new value
///     sender.send(42);
///
///     // Receiver receives the new value
///     assert_eq!(receiver.recv(), Some(42), "Receiver should receive 42");
///
///
///     assert_eq!(receiver.recv(), None, "Receiver should not receive anything else");
/// }
/// ```
#[inline]
pub fn broadcast<T>(initial: T) -> (BroadSet<T>, BroadGet<T>)
where
    T: Clone,
{
    Broadcast::new_split(initial)
}

/// Creates a new broadcasting channel with the given initial value and a custom lock type.
#[inline]
pub fn broadcast_with_lock<T, L>(initial: T) -> (BroadSet<T, L>, BroadGet<T, L>)
where
    T: Clone,
    L: RawRwLock,
{
    Broadcast::new_split(initial)
}

/// `BroadGet` paired with cached value.
///
/// This allows reading the value from cache and update when needed.
pub struct BroadCache<T, L = crate::DefaultRawRwLock> {
    get: BroadGet<T, L>,
    local: T,
}

impl<T, L> Clone for BroadCache<T, L>
where
    T: Clone,
{
    #[inline]
    fn clone(&self) -> Self {
        BroadCache {
            get: self.get.clone(),
            local: self.local.clone(),
        }
    }

    #[inline]
    fn clone_from(&mut self, source: &Self) {
        self.get.clone_from(&source.get);
        self.local.clone_from(&source.local);
    }
}

impl<T, L> BroadCache<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    /// Create a new broadcasting cache with the given initial value.
    #[inline]
    pub fn new(mut get: BroadGet<T, L>) -> Self {
        let local = get.last();
        BroadCache { get, local }
    }

    /// Get the current value from the cache.
    #[inline]
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

impl<T, L> From<BroadGet<T, L>> for BroadCache<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    #[inline]
    fn from(get: BroadGet<T, L>) -> Self {
        BroadCache::new(get)
    }
}
