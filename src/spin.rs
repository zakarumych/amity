use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::backoff::BackOff;

/// A very simple spin-lock.
///
/// Consists of a single atomic boolean that is set to true when the lock is acquired and false when it is released.
/// Locks are not fair and not ordered.
pub struct RawSpin {
    lock: AtomicBool,
}

impl RawSpin {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn is_locked(&self) -> bool {
        self.lock.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn try_lock(&self) -> bool {
        !self.lock.fetch_or(true, Ordering::Acquire)
    }

    pub fn lock(&self) {
        let mut backoff = BackOff::new();

        while self.lock.fetch_or(true, Ordering::Acquire) {
            while self.is_locked() {
                backoff.wait();
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

/// A very simple read-write spin-lock.
///
/// It consists of a single atomic integer that is used to track the number of shared locks and the exclusive lock.
/// The lock is not totally fair.
/// Pending exclusive lock block new shared locks from being acquired.
/// But in case of constant stream of exclusive locks, some of them may block indefinitely, so are shared locks.
pub struct RawRwSpin {
    lock: AtomicUsize,
}

// There maximum number of shared locks that can be held at a time.
// When this limit is exceeded, panic occurs, preventing the program to break the lock algorithm.
// This can only happen if the shared lock is acquired repeatedly without being released properly.
// Because there can't be enough threads to hold the lock at the same time.
const SHARED_LOCK_THRESHOLD: usize = usize::MAX / 32 + 1;

// This value is used by exclusive lock to enqueue itself and makes new shared locks wait for it.
const EXCLUSIVE_PENDING: usize = usize::MAX / 4 + 1;

// This value is used by exclusive lock.
const EXCLUSIVE_LOCKED: usize = usize::MAX / 2 + 1;

impl RawRwSpin {
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

    /// Performs a single attempt to acquire a shared lock.
    ///
    /// Returns true if acquired successfully, false otherwise.
    /// If acquired, caller is responsible for calling [`unlock_shared`](Self::unlock_shared) to release the shared lock.
    #[must_use]
    pub fn try_lock_shared(&self) -> bool {
        let count = self.lock.fetch_add(1, Ordering::Acquire);

        #[cfg(debug_assertions)]
        locks_count_check(count, || {
            // Unlock if shared locks count is too high.
            self.lock.fetch_sub(1, Ordering::Relaxed);
        });

        if count < EXCLUSIVE_PENDING {
            true
        } else {
            // Give up.
            self.lock.fetch_sub(1, Ordering::Relaxed);
            false
        }
    }

    /// Attempts to acquire a shared lock repeatedly until it succeeds or the provided closure returns false.
    ///
    /// Returns true if acquired successfully, false otherwise.
    /// If acquired, caller is responsible for calling [`unlock_shared`](Self::unlock_shared) to release the shared lock.
    #[must_use]
    pub fn lock_shared_while(&self, cond: impl FnMut() -> bool) -> bool {
        let mut count = self.lock.fetch_add(1, Ordering::Acquire);
        let mut backoff = BackOff::new();
        let mut cond = cond;

        #[cfg(debug_assertions)]
        locks_count_check(count, || {
            // Unlock if shared locks count is too high.
            self.lock.fetch_sub(1, Ordering::Relaxed);
        });

        if count < EXCLUSIVE_PENDING {
            // Successfully acquired the lock.
            return true;
        }

        // Only once, give a room for exclusive lock to be acquired.
        if count < EXCLUSIVE_LOCKED {
            // Exclusive lock was enqueued, so give a room for it to be acquired.
            count = self.lock.fetch_sub(1, Ordering::Relaxed);

            loop {
                #[cfg(debug_assertions)]
                locks_count_check(count, || {});

                if !cond() {
                    // The closure returned false, so we won't wait for the lock to be released.
                    return false;
                }

                // No exclusive lock waiters, so we can acquire the lock.
                if count >= EXCLUSIVE_LOCKED || count < EXCLUSIVE_PENDING {
                    // Get back to queue.
                    count = self.lock.fetch_add(1, Ordering::Acquire);
                    break;
                }

                // Wait a bit.
                backoff.wait();

                count = self.lock.load(Ordering::Relaxed);
            }
        }

        loop {
            #[cfg(debug_assertions)]
            locks_count_check(count, || {
                // Unlock if shared locks count is too high.
                self.lock.fetch_sub(1, Ordering::Relaxed);
            });

            // Here we can go ahead of exclusive lock waiter.
            // If one was present before call to this method
            // it was let ahead of us in branch before this loop.
            if count < EXCLUSIVE_LOCKED {
                // Successfully acquired the lock.
                return true;
            }

            if !cond() {
                // The closure returned false, so we won't wait for the lock to be released.
                self.lock.fetch_sub(1, Ordering::Relaxed);
                return false;
            }

            // Wait a bit.
            backoff.wait();

            count = self.lock.load(Ordering::Acquire);
        }
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
        let mut in_queue = false;

        let mut r = self.lock.compare_exchange_weak(
            0,
            EXCLUSIVE_LOCKED,
            Ordering::Acquire,
            Ordering::Relaxed,
        );

        loop {
            match r {
                Ok(_) => {
                    // Successfully acquired the lock.
                    return true;
                }
                Err(mut count) => {
                    if !cond() {
                        if in_queue {
                            // Remove itself from the queue.
                            self.lock.fetch_sub(EXCLUSIVE_PENDING, Ordering::Relaxed);
                        }

                        // The closure returned false, so we won't wait for the lock to be released.
                        return false;
                    }

                    if !in_queue && count > 0 && count < EXCLUSIVE_PENDING {
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
                                in_queue = true;
                                count += EXCLUSIVE_PENDING
                            }
                            Err(c) => count = c,
                        }
                    }

                    let expected = if in_queue { EXCLUSIVE_PENDING } else { 0 };

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
    /// Constant stream of shared locks will not block the exclusive lock indefinitely.
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

/// Mutex type that uses [`RawSpin`](RawSpin).
pub type Spin<T> = lock_api::Mutex<RawSpin, T>;

/// Read-write mutex type that uses [`RawRwSpin`](RawRwSpin).
pub type RwSpin<T> = lock_api::RwLock<RawRwSpin, T>;

#[cfg(debug_assertions)]
#[inline(always)]
fn locks_count_check(count: usize, failure: impl FnOnce()) {
    if count < EXCLUSIVE_PENDING {
        if count > SHARED_LOCK_THRESHOLD {
            failure();

            // Terminate well before shared lock counter would overflow into exclusive lock range.
            too_many_shared_locks(count);
        }
    } else if count < EXCLUSIVE_LOCKED {
        if count > EXCLUSIVE_PENDING + SHARED_LOCK_THRESHOLD {
            failure();

            // Terminate well before lock counter would overflow.
            too_many_shared_locks(count - EXCLUSIVE_LOCKED);
        }
    } else {
        if count > EXCLUSIVE_LOCKED + SHARED_LOCK_THRESHOLD {
            failure();

            // Terminate well before lock counter would overflow.
            too_many_shared_locks(count - EXCLUSIVE_LOCKED);
        }
    }
}

#[cfg(debug_assertions)]
#[cold]
#[inline(never)]
fn too_many_shared_locks(count: usize) -> ! {
    panic!(
        "There can only be {SHARED_LOCK_THRESHOLD} shared locks at a time. Current count: {count}. This can only happen if the lock is not released properly.",
    );
}
