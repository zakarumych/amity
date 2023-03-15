//! This module abstracts over the park-unpark mechanism.

#[cfg(feature = "std")]
use std::thread::{current, Thread};

/// Generic unpark mechanism.
/// With `std` enabled `std::thread::Thread` implements this trait.
pub trait Unpark {
    fn unpark(&self);
}

/// Generic unpark mechanism.
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

pub trait Park<T: Unpark> {
    fn park(&self);
    fn unpark_token(&self) -> T;
}

#[cfg(feature = "std")]
pub struct CurrentThread;

#[cfg(feature = "std")]
impl Park<Thread> for CurrentThread {
    #[inline(always)]
    fn park(&self) {
        std::thread::park();
    }

    #[inline(always)]
    fn unpark_token(&self) -> Thread {
        current()
    }
}

pub struct UnparkNoop;

impl Unpark for UnparkNoop {
    #[inline(always)]
    fn unpark(&self) {}
}

pub struct ParkUnparkYield;

impl Park<UnparkNoop> for ParkUnparkYield {
    #[inline(always)]
    fn park(&self) {
        #[cfg(feature = "std")]
        std::thread::yield_now();

        #[cfg(not(feature = "std"))]
        core::hint::spin_loop();
    }

    #[inline(always)]
    fn unpark_token(&self) -> UnparkNoop {
        UnparkNoop
    }
}
