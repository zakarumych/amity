//! Spin-based mutexes and reader-writer locks.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::backoff::BackOff;

/// A very simple spin-lock.
///
/// Consists of a single atomic boolean that is set to true when the lock is acquired and false when it is released.
/// Locks are not fair and not ordered.
#[derive(Default)]
pub struct RawSpin {
    lock: AtomicBool,
}

impl RawSpin {
    /// Creates an unlocked mutex.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    /// Returns whether the mutex is currently locked.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    /// Attempts to acquire the mutex, returning true on success.
    #[must_use]
    pub fn try_lock(&self) -> bool {
        !self.lock.fetch_or(true, Ordering::Acquire)
    }

    /// Waits until the mutex is acquired.
    pub fn lock(&self) {
        let mut backoff = BackOff::new();

        while self.lock.fetch_or(true, Ordering::Acquire) {
            while self.is_locked() {
                backoff.wait();
            }
        }
    }

    /// Releases the held mutex.
    pub fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }
}

unsafe impl lock_api::RawMutex for RawSpin {
    type Guard = lock_api::GuardSend;

    const INIT: Self = Self::new();

    #[inline]
    fn is_locked(&self) -> bool {
        self.is_locked()
    }

    #[inline]
    fn try_lock(&self) -> Option<lock_api::GuardSend> {
        self.try_lock().then(lock_api::GuardSend::new)
    }

    #[inline]
    fn lock(&self) -> lock_api::GuardSend {
        self.lock();
        lock_api::GuardSend::new()
    }

    #[inline]
    unsafe fn unlock(&self, _: lock_api::GuardSend) {
        self.unlock();
    }
}

/// A very simple read-write spin-lock.
///
/// It consists of a single atomic integer that is used to track the number of shared locks and the exclusive lock.
/// A pending writer blocks new readers. Readers waiting on an active writer can reserve
/// their place before another writer acquires the lock. Acquisition order is unspecified.
#[derive(Default)]
pub struct RawRwSpin {
    lock: AtomicUsize,
}

// Keep the reader count below the flag bits in every build.
const SHARED_LOCK_THRESHOLD: usize = usize::MAX / 32 + 1;

// This value is used by exclusive lock to enqueue itself and makes new shared locks wait for it.
const EXCLUSIVE_PENDING: usize = usize::MAX / 4 + 1;

// This value is used by exclusive lock.
const EXCLUSIVE_LOCKED: usize = usize::MAX / 2 + 1;

// A waiting reader owns one count. An enqueued writer owns the pending bit.
// Transfer the reservation on acquisition or release it when the wait ends.
struct Reservation<'a> {
    lock: &'a AtomicUsize,
    amount: usize,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.lock.fetch_sub(self.amount, Ordering::Relaxed);
    }
}

impl RawRwSpin {
    /// Creates an unlocked reader-writer lock.
    #[must_use]
    pub const fn new() -> Self {
        RawRwSpin {
            lock: AtomicUsize::new(0),
        }
    }

    /// Returns true if currently locked exclusively.
    ///
    /// Use this only as a hint since state may change even before the method returns.
    #[must_use]
    pub fn is_locked_exclusive(&self) -> bool {
        self.lock.load(Ordering::Relaxed) >= EXCLUSIVE_LOCKED
    }

    /// Returns true if currently locked exclusively or shared.
    ///
    /// Use this only as a hint since state may change even before the method returns.
    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed) > 0
    }

    /// Attempts to acquire a shared lock.
    ///
    /// Returns true if acquired successfully, false otherwise.
    /// If acquired, caller is responsible for calling [`unlock_shared`](Self::unlock_shared) to release the shared lock.
    #[must_use]
    pub fn try_lock_shared(&self) -> bool {
        self.reserve_reader(false).is_some()
    }

    fn reserve_reader(&self, wait_for_writer: bool) -> Option<usize> {
        let mut state = self.lock.load(Ordering::Relaxed);
        loop {
            if state & EXCLUSIVE_PENDING != 0 || (!wait_for_writer && state & EXCLUSIVE_LOCKED != 0)
            {
                return None;
            }
            let readers = state & (EXCLUSIVE_PENDING - 1);
            assert!(readers < SHARED_LOCK_THRESHOLD, "too many shared locks");
            match self.lock.compare_exchange_weak(
                state,
                state + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(state),
                Err(observed) => state = observed,
            }
        }
    }

    /// Attempts to acquire a shared lock repeatedly until it succeeds or the provided closure returns false.
    ///
    /// Returns true if acquired successfully, false otherwise.
    /// If acquired, caller is responsible for calling [`unlock_shared`](Self::unlock_shared) to release the shared lock.
    #[must_use]
    pub fn lock_shared_while(&self, mut cond: impl FnMut() -> bool) -> bool {
        let mut backoff = BackOff::new();
        let state = loop {
            if let Some(state) = self.reserve_reader(true) {
                break state;
            }
            if !cond() {
                return false;
            }
            backoff.wait();
        };

        if state & EXCLUSIVE_LOCKED != 0 {
            let reservation = Reservation {
                lock: &self.lock,
                amount: 1,
            };
            loop {
                if !cond() {
                    return false;
                }
                backoff.wait();
                if self.lock.load(Ordering::Acquire) & EXCLUSIVE_LOCKED == 0 {
                    break;
                }
            }
            core::mem::forget(reservation);
        }
        true
    }

    /// Performs shared locking, blocking the thread until the lock is acquired.
    ///
    /// This method is not guaranteed to be fair, so it may block indefinitely if exclusive locks are constantly being acquired.
    pub fn lock_shared(&self) {
        let acquired = self.lock_shared_while(|| true);
        debug_assert!(acquired);
    }

    /// Releases the shared lock previously acquired by
    /// [`lock_shared`](Self::lock_shared), [`lock_shared_while`](Self::lock_shared_while) or [`try_lock_shared`](Self::try_lock_shared).
    pub fn unlock_shared(&self) {
        self.lock.fetch_sub(1, Ordering::Release);
    }

    /// Performs a single attempt to acquire an exclusive lock.
    #[must_use]
    pub fn try_lock_exclusive(&self) -> bool {
        self.lock
            .compare_exchange(0, EXCLUSIVE_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Attempts to acquire an exclusive lock repeatedly until it succeeds or the provided closure returns false.
    ///
    /// Returns true if acquired successfully, false otherwise.
    /// If acquired, caller is responsible for calling [`unlock_exclusive`](Self::unlock_exclusive) to release the exclusive lock.
    #[must_use]
    pub fn lock_exclusive_while(&self, cond: impl FnMut() -> bool) -> bool {
        let mut cond = cond;
        let mut backoff = BackOff::new();
        let mut reservation = None;

        let mut r = self.lock.compare_exchange_weak(
            0,
            EXCLUSIVE_LOCKED,
            Ordering::Acquire,
            Ordering::Relaxed,
        );

        loop {
            match r {
                Ok(_) => {
                    // The exchange consumes the pending bit when this writer owns it.
                    core::mem::forget(reservation);
                    return true;
                }
                Err(mut count) => {
                    if !cond() {
                        return false;
                    }

                    if reservation.is_none() && count > 0 && count < EXCLUSIVE_PENDING {
                        // Try to enqueue itself, to prevent new shared locks from being acquired,
                        // and only wait for existing shared locks to be released.
                        let r = self.lock.compare_exchange_weak(
                            count,
                            count + EXCLUSIVE_PENDING,
                            Ordering::Relaxed,
                            Ordering::Relaxed,
                        );

                        match r {
                            Ok(_) => {
                                reservation = Some(Reservation {
                                    lock: &self.lock,
                                    amount: EXCLUSIVE_PENDING,
                                });
                                count += EXCLUSIVE_PENDING;
                            }
                            Err(c) => count = c,
                        }
                    }

                    let expected = if reservation.is_some() {
                        EXCLUSIVE_PENDING
                    } else {
                        0
                    };

                    if count != expected {
                        // Wait only if this is not spurious fail.
                        backoff.wait();
                    }

                    // Try again.
                    r = self.lock.compare_exchange_weak(
                        expected,
                        EXCLUSIVE_LOCKED,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    );
                }
            }
        }
    }

    /// Performs exclusive locking, blocking the thread until the lock is acquired.
    ///
    /// This method is not guaranteed to be fair, so it may block indefinitely if exclusive locks are constantly being acquired.
    /// Once a writer reserves the pending bit, incoming readers wait for the writer to acquire the lock.
    pub fn lock_exclusive(&self) {
        let acquired = self.lock_exclusive_while(|| true);
        debug_assert!(acquired);
    }

    /// Releases the exclusive lock previously acquired by
    /// [`lock_exclusive`](Self::lock_exclusive), [`lock_exclusive_while`](Self::lock_exclusive_while) or [`try_lock_exclusive`](Self::try_lock_exclusive).
    pub fn unlock_exclusive(&self) {
        self.lock.fetch_sub(EXCLUSIVE_LOCKED, Ordering::Release);
    }
}

unsafe impl lock_api::RawRwLock for RawRwSpin {
    type SharedGuard = lock_api::GuardSend;
    type ExclusiveGuard = lock_api::GuardSend;

    const INIT: Self = Self::new();

    #[inline]
    fn lock_shared(&self) -> lock_api::GuardSend {
        self.lock_shared();
        lock_api::GuardSend::new()
    }

    fn try_lock_shared(&self) -> Option<lock_api::GuardSend> {
        self.try_lock_shared().then(lock_api::GuardSend::new)
    }

    #[inline]
    unsafe fn unlock_shared(&self, _: lock_api::GuardSend) {
        self.unlock_shared();
    }

    #[inline]
    fn lock_exclusive(&self) -> lock_api::GuardSend {
        self.lock_exclusive();
        lock_api::GuardSend::new()
    }

    fn try_lock_exclusive(&self) -> Option<lock_api::GuardSend> {
        self.try_lock_exclusive().then(lock_api::GuardSend::new)
    }

    #[inline]
    unsafe fn unlock_exclusive(&self, _: lock_api::GuardSend) {
        self.unlock_exclusive();
    }

    #[inline]
    fn is_locked(&self) -> bool {
        self.is_locked()
    }

    #[inline]
    fn is_locked_exclusive(&self) -> bool {
        self.is_locked_exclusive()
    }
}

/// Mutex type that uses [`RawSpin`].
pub type Spin<T> = lock_api::Mutex<RawSpin, T>;

/// Read-write mutex type that uses [`RawRwSpin`].
pub type RwSpin<T> = lock_api::RwLock<RawRwSpin, T>;

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::panic::catch_unwind;

    #[test]
    fn reader_predicate_unwind_releases_reservation() {
        let lock = RawRwSpin::new();
        lock.lock_exclusive();
        assert!(catch_unwind(|| lock.lock_shared_while(|| panic!())).is_err());
        lock.unlock_exclusive();
        assert!(lock.try_lock_exclusive());
        lock.unlock_exclusive();
    }

    #[test]
    fn writer_predicate_unwind_releases_reservation() {
        let lock = RawRwSpin::new();
        lock.lock_shared();
        assert!(
            catch_unwind(|| lock.lock_exclusive_while(|| {
                assert_eq!(lock.lock.load(Ordering::Relaxed) & EXCLUSIVE_PENDING, 0);
                true
            }))
            .is_err()
        );
        lock.unlock_shared();
        assert!(lock.try_lock_exclusive());
        lock.unlock_exclusive();
    }

    #[test]
    fn reader_limit_preserves_lock_state() {
        let lock = RawRwSpin::new();
        lock.lock.store(SHARED_LOCK_THRESHOLD, Ordering::Relaxed);
        assert!(catch_unwind(|| lock.try_lock_shared()).is_err());
        assert_eq!(lock.lock.load(Ordering::Relaxed), SHARED_LOCK_THRESHOLD);
    }
}
