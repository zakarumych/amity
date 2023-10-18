use core::mem::MaybeUninit;

pub use loom::{
    cell::UnsafeCell,
    hint::spin_loop,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering},
    thread::{current, park, yield_now, Thread},
};

pub fn with_atomic<T>(a: &mut AtomicU64, f: impl FnOnce(&mut u64) -> T) -> T {
    a.with_mut(f)
}

pub fn read_atomic(a: &mut AtomicU64) -> u64 {
    a.with_mut(|a| *a)
}

pub fn write_atomic(a: &mut AtomicU64, v: u64) {
    a.with_mut(|a| *a = v)
}

#[inline(always)]
pub unsafe fn write_cell<T>(a: &UnsafeCell<T>, value: T) {
    *a.get_mut().deref() = value;
}

#[inline(always)]
pub unsafe fn read_init_cell<T>(a: &mut UnsafeCell<MaybeUninit<T>>) -> T {
    a.get_mut().deref().assume_init_read()
}

pub use parking_lot::{Mutex, RwLock};
