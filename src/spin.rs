use core::sync::atomic::{AtomicBool, Ordering};

use crate::backoff::BackOff;

pub struct SpinRaw {
    lock: AtomicBool,
}

impl SpinRaw {
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
