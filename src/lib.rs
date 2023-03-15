//!
//! Amity crate.
//!

#![cfg_attr(not(feature = "std"), no_std)]

pub mod backoff;
pub mod condvar;
pub mod mutex;
pub mod park;
pub mod spin;
pub mod state_ptr;

pub type Spin<T> = lock_api::Mutex<spin::RawSpin, T>;

#[cfg(feature = "std")]
pub type Mutex<T> = lock_api::Mutex<mutex::StdRawMutex, T>;

#[cfg(not(feature = "std"))]
pub type Mutex<T> = Spin<T>;
