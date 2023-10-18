use crate::{
    condvar::{CondVar, CondVarWake},
    park::{DefaultPark, Park, ParkYield, Unpark},
    sync::Ordering,
};

const UNLOCKED: u8 = 0;
const MAX_SHARED: u8 = 254;
const EXCLUSIVE_LOCK: u8 = 255;

/// Raw mutex that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
#[cfg(feature = "std")]
pub type StdRawRwLock = RawRwLock<crate::sync::Thread>;

/// Raw mutex that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldRawRwLock = RawRwLock<ParkYield>;

pub struct RawRwLock<T> {
    condvar: CondVar<T>,
}

impl<T> RawRwLock<T> {
    #[inline(always)]
    #[cfg(loom)]
    pub fn new() -> Self {
        RawRwLock {
            condvar: CondVar::zero(),
        }
    }

    #[inline(always)]
    #[cfg(not(loom))]
    pub const fn new() -> Self {
        RawRwLock {
            condvar: CondVar::zero(),
        }
    }

    /// Returns true if the lock is acquired in any way, false otherwise.
    #[inline(always)]
    pub fn is_locked(&self) -> bool {
        self.condvar.load(Ordering::Relaxed) != UNLOCKED
    }

    /// Attempts to acquire the shared lock without blocking.
    /// Returns true if the lock was acquired, false otherwise.
    #[inline(always)]
    pub fn try_lock_shared(&self) -> bool {
        // If this fails then either the lock is already acquired or
        // at least one thread is waiting for the lock.
        self.condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, 1)
    }

    /// Attempts to acquire the exclusive lock without blocking.
    /// Returns true if the lock was acquired, false otherwise.
    #[inline(always)]
    pub fn try_lock_exclusive(&self) -> bool {
        // If this fails then either the lock is already acquired or
        // at least one thread is waiting for the lock.
        self.condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, EXCLUSIVE_LOCK)
    }
}

impl<T> RawRwLock<T>
where
    T: Unpark,
{
    /// Blocking shared lock that returns when the lock is acquired.
    #[inline(always)]
    pub fn lock_shared_park(&self, park: impl Park<T>) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, 1)
        {
            return;
        }

        self.condvar.update_wait_park(
            park,
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                EXCLUSIVE_LOCK | MAX_SHARED => None,
                readers => Some(readers + 1),
            },
        );
    }

    /// Blocking exclusive lock that returns when the lock is acquired.
    #[inline(always)]
    pub fn lock_exclusive_park(&self, park: impl Park<T>) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, EXCLUSIVE_LOCK)
        {
            return;
        }

        self.condvar.update_wait_park(
            park,
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => Some(EXCLUSIVE_LOCK),
                _ => None,
            },
        );
    }

    #[inline(always)]
    pub unsafe fn unlock_shared(&self) {
        self.condvar.update(
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => unreachable!("unlock_shared called on unlocked RwLock"),
                readers => readers - 1,
            },
        );
    }

    #[inline(always)]
    pub unsafe fn unlock_exclusive(&self) {
        self.condvar.set(CondVarWake::One, Ordering::Release, 0);
    }
}

impl<T> RawRwLock<T>
where
    T: DefaultPark,
{
    /// Blocking lock that returns when the lock is acquired.
    #[inline(always)]
    pub fn lock_shared(&self) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, 1)
        {
            return;
        }

        self.condvar.update_wait(
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                EXCLUSIVE_LOCK | MAX_SHARED => None,
                readers => Some(readers + 1),
            },
        );
    }

    /// Blocking lock that returns when the lock is acquired.
    #[inline(always)]
    pub fn lock_exclusive(&self) {
        if self
            .condvar
            .optimistic_update(Ordering::Acquire, UNLOCKED, EXCLUSIVE_LOCK)
        {
            return;
        }

        self.condvar.update_wait(
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => Some(EXCLUSIVE_LOCK),
                _ => None,
            },
        );
    }
}

#[cfg(not(loom))]
unsafe impl<T> lock_api::RawRwLock for RawRwLock<T>
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
    fn try_lock_shared(&self) -> bool {
        self.try_lock_shared()
    }

    #[inline(always)]
    fn lock_shared(&self) {
        self.lock_shared()
    }

    #[inline(always)]
    unsafe fn unlock_shared(&self) {
        self.unlock_shared()
    }

    #[inline(always)]
    fn try_lock_exclusive(&self) -> bool {
        self.try_lock_exclusive()
    }

    #[inline(always)]
    fn lock_exclusive(&self) {
        self.lock_exclusive()
    }

    #[inline(always)]
    unsafe fn unlock_exclusive(&self) {
        self.unlock_exclusive()
    }
}
