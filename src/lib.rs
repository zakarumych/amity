//!
//! Amity crate.
//!

#![cfg_attr(not(feature = "std"), no_std)]

use core::sync::atomic::Ordering;

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod backoff;
pub mod condvar;
pub mod flip_queue;
pub mod mutex;
pub mod park;
pub mod rwlock;
pub mod spin;
pub mod state_ptr;

mod sync;

pub type Spin<T> = lock_api::Mutex<spin::RawSpin, T>;

/// Mutex that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
#[cfg(feature = "std")]
pub type Mutex<T> = lock_api::Mutex<mutex::StdRawMutex, T>;

/// Mutex that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldMutex<T> = lock_api::Mutex<mutex::YieldRawMutex, T>;

#[cfg(not(feature = "std"))]
pub type Mutex<T> = Spin<T>;

/// RwLock that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
#[cfg(feature = "std")]
pub type RwLock<T> = lock_api::RwLock<rwlock::StdRawRwLock, T>;

/// RwLock that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldRwLock<T> = lock_api::RwLock<rwlock::YieldRawRwLock, T>;

#[cfg(not(feature = "std"))]
pub type RwLock<T> = Spin<T>;

// One central function responsible for reporting capacity overflows. This'll
// ensure that the code generation related to these panics is minimal as there's
// only one location which panics rather than a bunch throughout the module.
fn capacity_overflow() -> ! {
    panic!("capacity overflow");
}

// pub enum Ordering {
//     Relaxed,
//     Release,
//     Acquire,
//     AcqRel,
//     SeqCst,
// }

#[inline]
fn merge_ordering(lhs: Ordering, rhs: Ordering) -> Ordering {
    match (lhs, rhs) {
        (Ordering::SeqCst, _) => Ordering::SeqCst,
        (_, Ordering::SeqCst) => Ordering::SeqCst,
        (Ordering::AcqRel, _) => Ordering::AcqRel,
        (_, Ordering::AcqRel) => Ordering::AcqRel,
        (Ordering::Acquire, Ordering::Release) => Ordering::AcqRel,
        (Ordering::Release, Ordering::Acquire) => Ordering::AcqRel,
        (Ordering::Acquire, _) => Ordering::Acquire,
        (_, Ordering::Acquire) => Ordering::Acquire,
        (Ordering::Release, _) => Ordering::Release,
        (_, Ordering::Release) => Ordering::Release,
        (Ordering::Relaxed, Ordering::Relaxed) => Ordering::Relaxed,
        _ => unreachable!("amity does not use any other ordering"),
    }
}
