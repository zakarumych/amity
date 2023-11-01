use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::backoff::BackOff;

/// A simple spin-lock.
pub struct RawSpin {
    lock: AtomicBool,
}

impl RawSpin {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    pub fn try_lock(&self) -> bool {
        !self.lock.fetch_or(true, Ordering::Acquire)
    }

    pub fn lock(&self) {
        let mut backoff = BackOff::new();

        while self.lock.fetch_or(true, Ordering::Acquire) {
            while self.is_locked() {
                backoff.blocking_wait();
            }
        }
    }

    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

unsafe impl lock_api::RawMutex for RawSpin {
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
    unsafe fn unlock(&self) {
        self.unlock()
    }
}

pub struct RawRwSpin {
    lock: AtomicUsize,
}

const SHARED_LOCK_TRESHOLD: usize = usize::MAX / 4;
const EXCLUSIVE_LOCK: usize = usize::MAX / 2 + 1;

impl RawRwSpin {
    pub const fn new() -> Self {
        RawRwSpin {
            lock: AtomicUsize::new(0),
        }
    }

    pub fn is_locked_exclusive(&self) -> bool {
        self.lock.load(Ordering::Relaxed) >= EXCLUSIVE_LOCK
    }

    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed) > 0
    }

    pub fn try_lock_shared(&self) -> bool {
        let count = self.lock.fetch_add(1, Ordering::Acquire);

        if count < SHARED_LOCK_TRESHOLD {
            true
        } else {
            self.lock.fetch_sub(1, Ordering::Relaxed);
            false
        }
    }

    pub fn lock_shared(&self) {
        let mut backoff = BackOff::new();

        while !self.try_lock_shared() {
            while self.is_locked_exclusive() {
                backoff.blocking_wait();
            }
        }
    }

    pub fn unlock_shared(&self) {
        self.lock.fetch_sub(1, Ordering::Release);
    }

    pub fn try_lock_exclusive(&self) -> bool {
        self.lock
            .compare_exchange(0, EXCLUSIVE_LOCK, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    pub fn lock_exclusive(&self) {
        let mut backoff = BackOff::new();

        while self
            .lock
            .compare_exchange_weak(0, EXCLUSIVE_LOCK, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.is_locked() {
                backoff.blocking_wait();
            }
        }
    }

    pub fn unlock_exclusive(&self) {
        self.lock.fetch_sub(EXCLUSIVE_LOCK, Ordering::Release);
    }
}

unsafe impl lock_api::RawRwLock for RawRwSpin {
    type GuardMarker = lock_api::GuardSend;

    const INIT: Self = Self::new();

    #[inline(always)]
    fn lock_shared(&self) {
        self.lock_shared()
    }

    fn try_lock_shared(&self) -> bool {
        self.try_lock_shared()
    }

    #[inline(always)]
    unsafe fn unlock_shared(&self) {
        self.unlock_shared()
    }

    #[inline(always)]
    fn lock_exclusive(&self) {
        self.lock_exclusive()
    }

    fn try_lock_exclusive(&self) -> bool {
        self.try_lock_exclusive()
    }

    #[inline(always)]
    unsafe fn unlock_exclusive(&self) {
        self.unlock_exclusive()
    }

    #[inline(always)]
    fn is_locked(&self) -> bool {
        self.is_locked()
    }

    #[inline(always)]
    fn is_locked_exclusive(&self) -> bool {
        self.is_locked_exclusive()
    }
}
