use crate::{
    backoff::BackOff,
    sync::{AtomicBool, Ordering},
};

/// A simple spin-lock.
pub struct RawSpin {
    lock: AtomicBool,
}

impl RawSpin {
    #[cfg(loom)]
    pub fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    #[cfg(not(loom))]
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    pub fn try_lock(&self) -> bool {
        self.lock.swap(true, Ordering::Acquire) == false
    }

    pub fn lock(&self) {
        let mut backoff = BackOff::new();

        while !self.try_lock() {
            while self.is_locked() {
                backoff.blocking_wait();
            }
        }
    }

    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

#[cfg(not(loom))]
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
    unsafe fn unlock(&self) {}
}
