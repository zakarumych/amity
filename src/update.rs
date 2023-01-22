//! Provides implementation of synchronized update algorithm.
//!
//! # Overview

use core::{
    ptr::NonNull,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicUsize, Ordering},
};

pub struct SyncUpdatePtr<T> {
    ptr: AtomicPtr<T>,
    state: AtomicBool,
}

impl<T> SyncUpdatePtr<T> {
    pub fn new(ptr: *mut T) -> Self {
        SyncUpdatePtr {
            ptr: AtomicPtr::new(ptr),
        }
    }

    pub fn get(&self) -> Option<NonNull<T>> {
        let ptr = self.ptr.load(Ordering::Acquire);
        NonNull::new(ptr)
    }

    pub fn update<F>(&self, f: F, yeet: impl Fn()) -> Option<NonNull<T>> {
        let old = self.state.swap(true, Ordering::Acquire);
        if old {
            loop {
                let ptr = self.ptr.load(Ordering::Acquire);
                if !ptr.is_null() {
                    yeet();
                    return NonNull::new(ptr);
                }
            }
        }
    }
}
