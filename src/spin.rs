//!
//! Configurable spin-lock.
//!

use core::sync::atomic::{AtomicBool, Ordering};

/// Basic spin-lock.
/// Works by spinning on atomic boolean flag.
pub struct Spin {
    locked: AtomicBool,
}

impl Spin {
    /// Returns new spin-lock.
    pub const fn new() -> Self {
        Spin {
            locked: AtomicBool::new(false),
        }
    }

    /// Returns `true` if spin-lock is locked.
    /// Returns `false` if spin-lock is unlocked.
    ///
    /// The state may change in any moment,
    /// after or even before this method returns.
    ///
    /// Use only as hint to the actual state.
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// Locks spin-lock.
    ///
    /// On success returns `true`.
    /// Spin lock should be unlocked by calling `unlock` method.
    ///
    /// On failure returns `false`.
    /// Lock is not acquired.
    pub fn try_lock(&self) -> bool {
        !self.locked.swap(true, Ordering::Acquire)
    }

    /// Locks spin-lock.
    /// Spins until lock is acquired.
    /// On each iteration calls `yeet` function that can perform some
    /// other work or yield CPU.
    pub fn lock(&self, yeet: impl Fn()) {
        while !self.try_lock() {
            while self.is_locked() {
                yeet();
            }
        }
    }
}
