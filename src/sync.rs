pub use core::{
    cell::UnsafeCell,
    hint::spin_loop,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering},
};

#[cfg(feature = "std")]
pub use std::thread::{current, park, yield_now, Thread};

#[inline(always)]
pub fn with_atomic<T>(a: &mut AtomicU64, f: impl FnOnce(&mut u64) -> T) -> T {
    f(a.get_mut())
}

#[inline(always)]
pub fn read_atomic(a: &mut AtomicU64) -> u64 {
    *a.get_mut()
}

#[inline(always)]
pub fn write_atomic(a: &mut AtomicU64, v: u64) {
    *a.get_mut() = v;
}

#[inline(always)]
pub unsafe fn write_cell<T>(a: &UnsafeCell<T>, value: T) {
    *a.get() = value;
}

#[inline(always)]
pub unsafe fn read_init_cell<T>(a: &mut UnsafeCell<MaybeUninit<T>>) -> T {
    a.get_mut().assume_init_read()
}

pub use crate::{Mutex, RwLock};
