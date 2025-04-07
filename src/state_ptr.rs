//! This module provides a way to combine state with pointer into
//! single value.
//!
//! Size of the state is limited by the pointer alignment.
//! `PtrState` combines pointer and state.
//! It preserves pointer provenance and allows to use restored pointer safely
//! under strict-provenance rules.

use core::{
    marker::PhantomData,
    mem::align_of,
    ptr::{from_mut, from_ref, null_mut},
    sync::atomic::{AtomicPtr, Ordering},
};

/// State wrapper for `usize` that ensures that
/// address bits for pointer to `T` are not set.
#[repr(transparent)]
pub struct State<T>(usize, PhantomData<T>);

impl<T> Clone for State<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for State<T> {}

impl<T> PartialEq for State<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for State<T> {}

impl<T> State<T> {
    /// Number of bits available to store the state.
    pub const STATE_BITS: u32 = <PtrState<T>>::STATE_BITS;

    /// Mask for state bits.
    pub const STATE_MASK: usize = <PtrState<T>>::STATE_MASK;

    /// Zero state.
    #[must_use]
    pub const fn zero() -> Self {
        State(0, PhantomData)
    }

    /// Returns state value.
    #[inline]
    #[must_use]
    pub const fn value(&self) -> usize {
        self.0
    }

    /// Creates new `State` from `usize`.
    /// If any of address bits are set then `None` is returned.
    #[inline]
    #[must_use]
    pub const fn new(value: usize) -> Option<Self> {
        if value & Self::STATE_MASK == value {
            Some(State(value, PhantomData))
        } else {
            None
        }
    }

    /// Creates new `State` from `usize`.
    /// Value is truncated to fit into `STATE_BITS`.
    #[inline]
    #[must_use]
    pub const fn new_truncated(value: usize) -> Self {
        State(value & Self::STATE_MASK, PhantomData)
    }
}

impl<T> From<State<T>> for usize {
    #[inline]
    fn from(state: State<T>) -> Self {
        state.value()
    }
}

/// Stores pointer and state in lower bits of single pointer value.
/// State size is limited by pointer alignment.
///
/// `PtrState` keeps provenance of the pointer.
#[repr(transparent)]
pub struct PtrState<T>(*mut T);

impl<T> Clone for PtrState<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for PtrState<T> {}

impl<T> PartialEq for PtrState<T> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<T> Eq for PtrState<T> {}

impl<T> PtrState<T> {
    /// Number of bits available to store the state.
    pub const STATE_BITS: u32 = align_of::<T>().trailing_zeros();

    /// Mask for state bits.
    pub const STATE_MASK: usize = align_of::<T>() - 1;

    /// Mask for pointer bits.
    pub const ADDR_MASK: usize = !Self::STATE_MASK;

    /// Null-pointer with zero state.
    #[must_use]
    pub const fn null_zero() -> Self {
        PtrState(null_mut())
    }

    /// Creates new `PtrState` with null pointer and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    #[must_use]
    pub fn null_state(state: State<T>) -> Self {
        PtrState::new(null_mut(), state)
    }

    /// Creates new `PtrState` from pointer and state.
    /// State is wrapped to ensure that only lower bits may be set.
    ///
    /// # Panics
    ///
    /// When debug assertions are enabled pointer is checked to not contain any state bits.
    #[inline]
    pub fn new(ptr: *mut T, state: State<T>) -> Self {
        PtrState(ptr.cast::<u8>().wrapping_add(state.0).cast())
    }

    /// Creates new `PtrState` from reference and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    pub fn new_ref(ptr: &T, state: State<T>) -> Self {
        PtrState::new(from_ref(ptr).cast_mut(), state)
    }

    /// Creates new `PtrState` from mutable reference and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    pub fn new_mut(ptr: &mut T, state: State<T>) -> Self {
        PtrState::new(from_mut(ptr), state)
    }

    /// Creates new `PtrState` with pointer and zero state.
    ///
    /// # Panics
    ///
    /// When debug assertions are enabled pointer is checked to not contain any state bits.
    #[inline]
    pub const fn new_zero(ptr: *mut T) -> Self {
        PtrState(ptr)
    }

    /// Constructs `PtrState` from raw pointer.
    /// Any existing state bits from raw pointer are preserved.
    #[inline]
    pub const fn from_raw(ptr: *mut T) -> Self {
        PtrState(ptr)
    }

    /// Returns raw pointer value with both address and state bits.
    #[inline]
    #[must_use]
    pub const fn raw(&self) -> *mut T {
        self.0
    }

    /// Creates new `PtrState` with pointer from this value and new state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    #[must_use]
    pub fn with_state(&self, state: State<T>) -> Self {
        PtrState::new(self.ptr(), state)
    }

    /// Creates new `PtrState` with state from this value and new pointer.
    ///
    /// # Panics
    ///
    /// When debug assertions are enabled pointer is checked to not contain any state bits.
    #[inline]
    #[must_use]
    pub fn with_ptr(&self, ptr: *mut T) -> Self {
        PtrState::new(ptr, self.state())
    }

    /// Returns pointer from this value.
    #[must_use]
    pub fn ptr(&self) -> *mut T {
        let state = (self.0 as usize) & Self::STATE_MASK;
        self.0.cast::<u8>().wrapping_sub(state).cast()
    }

    /// Returns state from this value.
    #[must_use]
    pub fn state(&self) -> State<T> {
        let state = (self.0 as usize) & Self::STATE_MASK;
        State(state, PhantomData)
    }
}

/// Stores pointer and state in lower bits of single pointer value.
/// State size is limited by pointer alignment.
///
/// `AtomicPtrState` keeps provenance of the pointer.
#[repr(transparent)]
pub struct AtomicPtrState<T>(AtomicPtr<T>);

impl<T> AtomicPtrState<T> {
    /// Number of bits available to store the state.
    pub const STATE_BITS: u32 = align_of::<T>().trailing_zeros();

    /// Mask for state bits.
    pub const STATE_MASK: usize = align_of::<T>() - 1;

    /// Mask for pointer bits.
    pub const ADDR_MASK: usize = !Self::STATE_MASK;

    /// Null-pointer with zero state.
    #[must_use]
    pub const fn null_zero() -> Self {
        AtomicPtrState(AtomicPtr::new(null_mut()))
    }

    /// Creates new `AtomicPtrState` with null pointer and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    #[must_use]
    pub fn null_state(state: State<T>) -> Self {
        AtomicPtrState::new(null_mut(), state)
    }

    /// Creates new `AtomicPtrState` from pointer and state.
    /// State is wrapped to ensure that only lower bits may be set.
    ///
    /// # Panics
    ///
    /// When debug assertions are enabled pointer is checked to not contain any state bits.
    #[inline]
    pub fn new(ptr: *mut T, state: State<T>) -> Self {
        AtomicPtrState(AtomicPtr::new(
            ptr.cast::<u8>().wrapping_add(state.0).cast(),
        ))
    }

    /// Creates new `AtomicPtrState` from reference and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    pub fn new_ref(ptr: &T, state: State<T>) -> Self {
        AtomicPtrState::new(from_ref(ptr).cast_mut(), state)
    }

    /// Creates new `AtomicPtrState` from mutable reference and state.
    /// State is wrapped to ensure that only lower bits may be set.
    #[inline]
    pub fn new_mut(ptr: &mut T, state: State<T>) -> Self {
        AtomicPtrState::new(from_mut(ptr), state)
    }

    /// Creates new `AtomicPtrState` with pointer and zero state.
    ///
    /// # Panics
    ///
    /// When debug assertions are enabled pointer is checked to not contain any state bits.
    #[inline]
    pub fn new_zero(ptr: *mut T) -> Self {
        AtomicPtrState(AtomicPtr::new(ptr))
    }

    /// Constructs `AtomicPtrState` from raw pointer.
    /// Any existing state bits from raw pointer are preserved.
    #[inline]
    pub const fn from_raw(ptr: *mut T) -> Self {
        AtomicPtrState(AtomicPtr::new(ptr))
    }

    /// Creates new `AtomicPtrState` from merged pointer-state value.
    #[inline]
    #[must_use]
    pub fn from_ptr_state(ptr_state: PtrState<T>) -> Self {
        AtomicPtrState(AtomicPtr::new(ptr_state.raw()))
    }

    #[inline]
    pub fn load(&self, order: Ordering) -> PtrState<T> {
        PtrState(self.0.load(order))
    }

    #[inline]
    pub fn store(&self, value: PtrState<T>, order: Ordering) {
        self.0.store(value.0, order);
    }

    #[inline]
    pub fn swap(&self, value: PtrState<T>, order: Ordering) -> PtrState<T> {
        PtrState(self.0.swap(value.0, order))
    }

    /// Stores the `new` value into the pointer-state if the current value is the same as the `current` value.
    ///
    /// The return value is a result indicating whether the new value was written and containing
    /// the previous value. On success this value is guaranteed to be equal to `current`.
    ///
    /// `compare_exchange` takes two [`Ordering`] arguments to describe the memory
    /// ordering of this operation. `success` describes the required ordering for the
    /// read-modify-write operation that takes place if the comparison with `current` succeeds.
    /// `failure` describes the required ordering for the load operation that takes place when
    /// the comparison fails. Using [`Acquire`] as success ordering makes the store part
    /// of this operation [`Relaxed`], and using [`Release`] makes the successful load
    /// [`Relaxed`]. The failure ordering can only be [`SeqCst`], [`Acquire`] or [`Relaxed`].
    ///
    /// [`Ordering`]: core::sync::atomic::Ordering
    /// [`Acquire`]: core::sync::atomic::Ordering::Acquire
    /// [`Release`]: core::sync::atomic::Ordering::Release
    /// [`Relaxed`]: core::sync::atomic::Ordering::Relaxed
    /// [`SeqCst`]: core::sync::atomic::Ordering::SeqCst
    #[allow(clippy::missing_errors_doc)]
    #[inline]
    pub fn compare_exchange(
        &self,
        current: PtrState<T>,
        new: PtrState<T>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<PtrState<T>, PtrState<T>> {
        self.0
            .compare_exchange(current.0, new.0, success, failure)
            .map(PtrState)
            .map_err(PtrState)
    }

    /// Stores the `new` value into the pointer-state if the current value is the same as the `current` value.
    ///
    /// Unlike [`AtomicPtrState::compare_exchange`], this function is allowed to spuriously fail even when the
    /// comparison succeeds, which can result in more efficient code on some platforms. The
    /// return value is a result indicating whether the new value was written and containing the
    /// previous value.
    ///
    /// `compare_exchange_weak` takes two [`Ordering`] arguments to describe the memory
    /// ordering of this operation. `success` describes the required ordering for the
    /// read-modify-write operation that takes place if the comparison with `current` succeeds.
    /// `failure` describes the required ordering for the load operation that takes place when
    /// the comparison fails. Using [`Acquire`] as success ordering makes the store part
    /// of this operation [`Relaxed`], and using [`Release`] makes the successful load
    /// [`Relaxed`]. The failure ordering can only be [`SeqCst`], [`Acquire`] or [`Relaxed`].
    ///
    /// [`Ordering`]: core::sync::atomic::Ordering
    /// [`Acquire`]: core::sync::atomic::Ordering::Acquire
    /// [`Release`]: core::sync::atomic::Ordering::Release
    /// [`Relaxed`]: core::sync::atomic::Ordering::Relaxed
    /// [`SeqCst`]: core::sync::atomic::Ordering::SeqCst
    #[allow(clippy::missing_errors_doc)]
    #[inline]
    pub fn compare_exchange_weak(
        &self,
        current: PtrState<T>,
        new: PtrState<T>,
        success: Ordering,
        failure: Ordering,
    ) -> Result<PtrState<T>, PtrState<T>> {
        self.0
            .compare_exchange_weak(current.0, new.0, success, failure)
            .map(PtrState)
            .map_err(PtrState)
    }
}
