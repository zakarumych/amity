use core::sync::atomic::Ordering;

use crate::{
    condvar::{CondVar, CondVarWake},
    park::{DefaultPark, Park, ParkYield, Unpark},
};

const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;

/// Raw mutex that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
#[cfg(feature = "std")]
pub type StdRawMutex = RawMutex<std::thread::Thread>;

/// Raw mutex that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldRawMutex = RawMutex<ParkYield>;

pub struct RawMutex<T> {
    condvar: CondVar<T>,
}

impl<T> Default for RawMutex<T> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RawMutex<T> {
    #[inline]
    pub const fn new() -> Self {
        RawMutex {
            condvar: CondVar::zero(),
        }
    }

    /// Returns true if the lock is acquired, false otherwise.
    #[inline]
    pub fn is_locked(&self) -> bool {
        self.condvar.load(Ordering::Relaxed) == LOCKED
    }

    /// Attempts to acquire the lock without blocking.
    /// Returns true if the lock was acquired, false otherwise.
    #[inline]
    pub fn try_lock(&self) -> bool {
        // If this fails then either the lock is already acquired or
        // at least one thread is waiting for the lock.
        self.condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, LOCKED)
    }
}

impl<T> RawMutex<T>
where
    T: Unpark,
{
    /// Blocking lock that returns when the lock is acquired.
    #[inline]
    pub fn lock_park(&self, park: impl Park<T>) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, LOCKED)
        {
            return;
        }

        self.condvar.update_wait_park(
            park,
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => Some(LOCKED),
                LOCKED | _ => None,
            },
        );
    }

    #[inline]
    pub unsafe fn unlock(&self) {
        self.condvar.set(CondVarWake::One, Ordering::Release, 0);
    }
}

impl<T> RawMutex<T>
where
    T: DefaultPark,
{
    /// Blocking lock that returns when the lock is acquired.
    #[inline]
    pub fn lock(&self) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, LOCKED)
        {
            return;
        }

        self.condvar.update_wait(
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => Some(LOCKED),
                LOCKED | _ => None,
            },
        );
    }
}

unsafe impl<T> lock_api::RawMutex for RawMutex<T>
where
    T: DefaultPark,
{
    type GuardMarker = lock_api::GuardSend;

    const INIT: Self = Self::new();

    #[inline]
    fn is_locked(&self) -> bool {
        self.is_locked()
    }

    #[inline]
    fn try_lock(&self) -> bool {
        self.try_lock()
    }

    #[inline]
    fn lock(&self) {
        self.lock()
    }

    #[inline]
    unsafe fn unlock(&self) {}
}
