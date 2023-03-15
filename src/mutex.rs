use core::sync::atomic::Ordering;

use crate::{
    condvar::{CondVar, CondVarWake},
    park::{DefaultPark, Park, Unpark},
};

const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;

#[cfg(feature = "std")]
pub type StdRawMutex = RawMutex<std::thread::Thread>;

pub struct RawMutex<T> {
    condvar: CondVar<T>,
}

impl<T> RawMutex<T> {
    #[inline(always)]
    pub const fn new() -> Self {
        Self {
            condvar: CondVar::new(0),
        }
    }

    /// Returns true if the lock is acquired, false otherwise.
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        self.condvar.load(Ordering::Relaxed) == LOCKED
    }

    /// Attempts to acquire the lock without blocking.
    /// Returns true if the lock was acquired, false otherwise.
    #[inline(always)]
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
    #[inline(always)]
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

    #[inline(always)]
    pub unsafe fn unlock(&self) {
        self.condvar.set(CondVarWake::One, Ordering::Release, 0);
    }
}

impl<T> RawMutex<T>
where
    T: DefaultPark,
{
    /// Blocking lock that returns when the lock is acquired.
    #[inline(always)]
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

    #[inline(always)]
    fn is_locked(&self) -> bool {
        self.is_locked()
    }

    #[inline(always)]
    fn try_lock(&self) -> bool {
        self.try_lock()
    }

    #[inline(always)]
    fn lock(&self) {
        self.lock()
    }

    #[inline(always)]
    unsafe fn unlock(&self) {}
}
