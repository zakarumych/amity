//!
//! Amity crate.
//!

#![cfg_attr(not(feature = "std"), no_std)]

use core::{mem::ManuallyDrop, sync::atomic::Ordering};

#[cfg(feature = "alloc")]
extern crate alloc;

// pub mod arena;
pub mod backoff;
pub mod condvar;
pub mod flip_queue;
pub mod mutex;
pub mod park;
pub mod ring_buffer;
pub mod rwlock;
pub mod spin;
pub mod state_ptr;

// mod sync;

pub type Spin<T> = lock_api::Mutex<spin::RawSpin, T>;

#[cfg(feature = "std")]
pub type RawMutex = mutex::StdRawMutex;

#[cfg(not(feature = "std"))]
pub type RawMutex = spin::RawSpin;

/// Mutex that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;

/// Mutex that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldMutex<T> = lock_api::Mutex<mutex::YieldRawMutex, T>;

#[cfg(feature = "std")]
pub type RawRwLock = rwlock::StdRawRwLock;

#[cfg(not(feature = "std"))]
pub type RawRwLock = spin::RawRwSpin;

/// RwLock that uses thread parking when waiting for the lock.
///
/// It can only be used with `"std"`.
pub type RwLock<T> = lock_api::RwLock<RawRwLock, T>;

/// RwLock that uses thread yielding when waiting for the lock.
///
/// It causes busy waiting but can be used without `"std"`.
pub type YieldRwLock<T> = lock_api::RwLock<rwlock::YieldRawRwLock, T>;

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

struct Defer<F: FnOnce()>(ManuallyDrop<F>);

impl<F> Drop for Defer<F>
where
    F: FnOnce(),
{
    fn drop(&mut self) {
        unsafe {
            let f = ManuallyDrop::take(&mut self.0);
            f();
        }
    }
}

fn defer<F>(f: F) -> Defer<F>
where
    F: FnOnce(),
{
    Defer(ManuallyDrop::new(f))
}
