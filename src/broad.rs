//! Provides [`Sender`] and [`Receiver`] - broadcasting channel.
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
    /// Use `Sender` for safe usage.
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
    /// Use `Sender` for safe usage.
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
    /// Use `Sender` for safe usage.
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
    /// Use `Sender` for safe usage.
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

    /// Converts the channel into [`Sender`].
    #[inline]
    pub fn into_sender(self, producer: u8, version: u64) -> Sender<T, L> {
        Sender {
            broadcast: Arc::new(self),
            producer,
            version,
        }
    }
}

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

pub struct Sender<T, L = crate::DefaultRawRwLock> {
    broadcast: Arc<Broadcast<T, L>>,
    producer: u8,
    version: u64,
}

impl<T> Sender<T> {
    #[inline]
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
    #[inline]
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
    pub fn receiver(&self) -> Receiver<T, L> {
        Receiver {
            broadcast: self.broadcast.clone(),
            version: 0,
        }
    }
}

/// `Receiver` paired with cached value.
///
/// This allows reading the value from cache and update when needed.
pub struct Cache<T, L = crate::DefaultRawRwLock> {
    get: Receiver<T, L>,
    local: T,
}

impl<T, L> Clone for Cache<T, L>
where
    T: Clone,
{
    #[inline]
    fn clone(&self) -> Self {
        Cache {
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

impl<T, L> Cache<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    /// Create a new broadcasting cache with the given initial value.
    #[inline]
    pub fn new(mut get: Receiver<T, L>) -> Self {
        let local = get.last();
        Cache { get, local }
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

impl<T, L> From<Receiver<T, L>> for Cache<T, L>
where
    L: RawRwLock,
    T: Clone,
{
    #[inline]
    fn from(get: Receiver<T, L>) -> Self {
        Cache::new(get)
    }
}
