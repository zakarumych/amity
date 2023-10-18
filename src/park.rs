//! This module abstracts over the park-unpark mechanism.

use crate::sync::{current, Thread};

/// Generic unpark mechanism.
/// With `std` enabled `std::thread::Thread` implements this trait.
pub trait Unpark: Clone {
    fn unpark(&self);
}

/// Generic unpark mechanism that can be default-constructed.
/// With `std` enabled `std::thread::Thread` implements this trait.
pub trait DefaultPark: Unpark + Sized {
    type Park: Park<Self>;
    fn default_park() -> Self::Park;
}

#[cfg(feature = "std")]
impl Unpark for Thread {
    #[inline(always)]
    fn unpark(&self) {
        self.unpark();
    }
}

#[cfg(feature = "std")]
impl DefaultPark for Thread {
    type Park = CurrentThread;

    #[inline(always)]
    fn default_park() -> Self::Park {
        CurrentThread
    }
}

/// Generic parking mechanism.
///
/// # Usage
///
/// Call `unpark_token` to get a token that can be used to unpark this thread.
/// Place where another token can find it to unpark this thread.
/// Call `park` to block the current thread until it is unparked.
///
/// With `"std"` feature enabled `CurrentThread` implement this trait with `Thread` as unpark token.
///
/// `ParkYield` implements this trait with `UnparkYield` as unpark token.
/// It yields the current thread when `park` is called instead of blocking it.
/// This behavior is valid since spurious wakeups are allowed.
pub trait Park<T: Unpark> {
    /// Returns a token that can be used to unpark this thread.
    fn unpark_token(&self) -> T;

    /// Blocks the current thread until it is unparked using
    /// unpark token returned by `unpark_token` previously called from this thread.
    fn park(&self);
}

#[cfg(feature = "std")]
pub struct CurrentThread;

#[cfg(feature = "std")]
impl Park<Thread> for CurrentThread {
    #[inline(always)]
    fn park(&self) {
        crate::sync::park();
    }

    #[inline(always)]
    fn unpark_token(&self) -> Thread {
        current()
    }
}

#[derive(Clone, Copy)]
pub struct UnparkYield;

impl Unpark for UnparkYield {
    #[inline(always)]
    fn unpark(&self) {}
}

pub struct ParkYield;

impl Park<UnparkYield> for ParkYield {
    #[inline(always)]
    fn park(&self) {
        #[cfg(feature = "std")]
        crate::sync::yield_now();

        #[cfg(not(feature = "std"))]
        crate::sync::spin_loop();
    }

    #[inline(always)]
    fn unpark_token(&self) -> UnparkYield {
        UnparkYield
    }
}
