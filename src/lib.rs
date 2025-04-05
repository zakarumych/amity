#![doc = include_str!("../README.md")]
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::pedantic)]
#![allow(clippy::inline_always)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod backoff;
pub mod cache;
pub mod state_ptr;

#[cfg(feature = "broad")]
pub mod broad;

#[cfg(feature = "flip-queue")]
pub mod flip_queue;

#[cfg(feature = "ring-buffer")]
pub mod ring_buffer;

#[cfg(feature = "spin")]
pub mod spin;

#[cfg(feature = "triple")]
pub mod triple;

#[cfg(all(feature = "spin", not(feature = "parking_lot")))]
#[allow(dead_code)]
type DefaultRawRwLock = spin::RawRwSpin;

#[cfg(feature = "parking_lot")]
#[allow(dead_code)]
type DefaultRawRwLock = parking_lot::RawRwLock;

// One central function responsible for reporting capacity overflows. This'll
// ensure that the code generation related to these panics is minimal as there's
// only one location which panics rather than a bunch throughout the module.
#[cfg(feature = "alloc")]
#[allow(dead_code)]
#[cold]
#[inline(never)]
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

// #[inline]
// fn merge_ordering(lhs: Ordering, rhs: Ordering) -> Ordering {
//     match (lhs, rhs) {
//         (Ordering::SeqCst, _) => Ordering::SeqCst,
//         (_, Ordering::SeqCst) => Ordering::SeqCst,
//         (Ordering::AcqRel, _) => Ordering::AcqRel,
//         (_, Ordering::AcqRel) => Ordering::AcqRel,
//         (Ordering::Acquire, Ordering::Release) => Ordering::AcqRel,
//         (Ordering::Release, Ordering::Acquire) => Ordering::AcqRel,
//         (Ordering::Acquire, _) => Ordering::Acquire,
//         (_, Ordering::Acquire) => Ordering::Acquire,
//         (Ordering::Release, _) => Ordering::Release,
//         (_, Ordering::Release) => Ordering::Release,
//         (Ordering::Relaxed, Ordering::Relaxed) => Ordering::Relaxed,
//         _ => unreachable!("amity does not use any other ordering"),
//     }
// }
