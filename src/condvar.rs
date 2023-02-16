//!
//! Provides `CondVar` type that can be used to wait for state change
//!

use core::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "std")]
use std::thread::{current, Thread};

use crate::{
    backoff::BackOff,
    state_ptr::{AtomicPtrState, PtrState, State},
};

#[repr(align(256))]
struct CondVarNode {
    #[cfg(feature = "std")]
    thread: Thread,
    next: *mut Self,
    ready: AtomicBool,
}

/// Atomic condition variable.
/// It supports updating state value and waiting for an update.
/// It is implemented using lock-free algorithm
/// with exponential back-off and
/// optional thread parking when "std" feature is enabled.
#[repr(transparent)]
pub struct CondVar {
    atomic: AtomicPtrState<CondVarNode>,
}

pub enum CondVarUpdateOrWait {
    Update(u8),
    Wait,
    Break,
}

pub enum CondVarWake {
    None,
    One,
    All,
}

impl CondVar {
    /// Number of bits available to store the state.
    pub const STATE_BITS: u32 = <PtrState<CondVarNode>>::STATE_BITS;

    /// Mask for state bits.
    pub const STATE_MASK: usize = <PtrState<CondVarNode>>::STATE_MASK;

    /// Constant-initialized `CondVar` with zero state.
    pub const ZERO: Self = CondVar {
        atomic: AtomicPtrState::NULL_ZERO,
    };

    #[inline(always)]
    pub const fn new(state: u8) -> Self {
        CondVar {
            atomic: AtomicPtrState::new_null(State::new_truncated(state as usize)),
        }
    }

    /// Atomically loads current state,
    /// calls `f` with the state value and
    /// depending on the result of `f` either
    /// updates the state,
    /// waits for the state to change
    /// or breaks returning last read state.
    ///
    /// The `f` function is possibly called multiple times.
    /// When `f` returns `CondVarUpdateOrWait::Update` the state is updated if not yet changed.
    /// If successful `Ok` is returned with previous state.
    /// If unsuccessful `f` is called again with new state.
    /// When `f` returns `CondVarUpdateOrWait::Wait` it waits for the state to change.
    /// And then `f` is called again with new state.
    /// When `f` returns `CondVarUpdateOrWait::Break` it breaks returning `Err` with last read state.
    ///
    /// This function uses two atomic orderings.
    /// `load` ordering is used for loading the state.
    /// The state observable by `f` is always loaded with `load` ordering.
    ///
    /// `update` ordering is used for updating the state.
    /// Successful update is always done with `update` ordering.
    ///
    /// When state is updated may wake other threads that wait for the state to change.
    /// This is controlled by `wake` parameter.
    /// When `wake` is `CondVarWake::None` no threads are woken.
    /// When `wake` is `CondVarWake::One` only one thread is woken. // Due to ABA hazard this is currently acts as `CondVarWake::All`.
    /// When `wake` is `CondVarWake::All` all threads are woken.
    #[inline]
    pub fn update_wait_break(
        &self,
        wake: CondVarWake,
        load: Ordering,
        update: Ordering,
        mut f: impl FnMut(u8) -> CondVarUpdateOrWait,
    ) -> Result<u8, u8> {
        let mut cur = self.atomic.load(load);
        let mut backoff = BackOff::new();

        loop {
            match f(cur.state().value() as u8) {
                CondVarUpdateOrWait::Update(new_state) => {
                    // Update the state

                    let new = match wake {
                        CondVarWake::None => {
                            cur.with_state(State::new_truncated(new_state as usize))
                        }
                        // TODO: Fix ABA problem.
                        // CondVarWake::One => {
                        //     let cur_ptr = cur.ptr();
                        //     let next =
                        //         unsafe { cur_ptr.as_ref() }.map_or(null_mut(), |node| node.next);
                        //     PtrState::new(next, State::new_truncated(new_state as usize))
                        // }
                        CondVarWake::One | CondVarWake::All => {
                            PtrState::new_null(State::new_truncated(new_state as usize))
                        }
                    };

                    let result = self.atomic.compare_exchange_weak(cur, new, update, load);

                    match result {
                        Ok(_) => {
                            let mut node = cur.ptr();

                            match wake {
                                CondVarWake::None => {}
                                // TODO: Fix ABA problem.
                                // CondVarWake::One => {
                                //     if let Some(node_ref) = unsafe { node.as_ref() } {
                                //         node_ref.ready.store(true, update);
                                //         node_ref.thread.unpark();
                                //     }
                                // }
                                CondVarWake::One | CondVarWake::All => {
                                    while let Some(node_ref) = unsafe { node.as_ref() } {
                                        node_ref.ready.store(true, Ordering::Release);
                                        node_ref.thread.unpark();
                                        node = node_ref.next;
                                    }
                                }
                            }
                            return Ok(cur.state().value() as u8);
                        }
                        Err(new) => {
                            backoff.lock_free_wait(); // State changed. Retry after lock-free back-off.
                            cur = new;
                        }
                    }
                }
                CondVarUpdateOrWait::Wait => {
                    // Wait for the state to change.
                    // This will always cause new loop to be executed.

                    #[cfg(feature = "std")]
                    if backoff.should_block() {
                        // After few loops we should park the thread.
                        // This is only possible with "std" feature.
                        let mut node = CondVarNode {
                            thread: current(),
                            next: null_mut(),
                            ready: AtomicBool::new(false),
                        };

                        let root = cur.ptr();
                        node.next = root;

                        {
                            let node = &node; // Sharing is valid now.
                            let new = PtrState::new_ref(node, cur.state());
                            match self.atomic.compare_exchange_weak(
                                cur,
                                new,
                                Ordering::Relaxed,
                                load,
                            ) {
                                Ok(_) => {
                                    while !node.ready.load(Ordering::Acquire) {
                                        std::thread::park();
                                    }
                                    backoff.reset();

                                    // Load the state again.
                                    cur = self.atomic.load(load);
                                }
                                Err(new) => {
                                    // State changed. Retry.
                                    cur = new;
                                }
                            }
                        }
                        continue;
                    }

                    // Perform blocking back-off.
                    backoff.blocking_wait();

                    // Load the state again.
                    cur = self.atomic.load(load);
                }
                CondVarUpdateOrWait::Break => {
                    // Break the loop immediately.
                    return Err(cur.state().value() as u8);
                }
            }
        }
    }

    /// Simplified version of `update_wait_break` that
    /// never breaks.
    /// It either updates the state when `f` returns `Some` or
    /// waits for the state to change when `f` returns `None`.
    #[inline(always)]
    pub fn update_wait(
        &self,
        wake: CondVarWake,
        load: Ordering,
        update: Ordering,
        mut f: impl FnMut(u8) -> Option<u8>,
    ) -> u8 {
        let result = self.update_wait_break(wake, load, update, |state| match f(state) {
            Some(state) => CondVarUpdateOrWait::Update(state),
            None => CondVarUpdateOrWait::Wait,
        });
        match result {
            Ok(state) => state,
            Err(_) => unreachable!("Break variant is not used"),
        }
    }

    /// Simplified version of `update_wait_break` that
    /// never waits.
    /// It either updates the state when `f` returns `Some` or
    /// breaks when `f` returns `None`.
    #[inline(always)]
    pub fn update_break(
        &self,
        wake: CondVarWake,
        load: Ordering,
        update: Ordering,
        mut f: impl FnMut(u8) -> Option<u8>,
    ) -> Result<u8, u8> {
        self.update_wait_break(wake, load, update, |state| match f(state) {
            Some(state) => CondVarUpdateOrWait::Update(state),
            None => CondVarUpdateOrWait::Break,
        })
    }

    /// Simplified version of `update_wait_break` that
    /// always updates the state.
    #[inline(always)]
    pub fn update(
        &self,
        wake: CondVarWake,
        load: Ordering,
        update: Ordering,
        mut f: impl FnMut(u8) -> u8,
    ) -> u8 {
        let result = self.update_wait_break(wake, load, update, |state| {
            CondVarUpdateOrWait::Update(f(state))
        });
        match result {
            Ok(state) => state,
            Err(_) => unreachable!("Break variant is not used"),
        }
    }

    /// Simplified version of `update_wait_break` that
    /// always set pre-defined `new_state`.
    #[inline]
    pub fn set(&self, wake: CondVarWake, update: Ordering, new_state: u8) -> u8 {
        match wake {
            CondVarWake::None => {
                self.update(CondVarWake::None, Ordering::Relaxed, update, |_| new_state)
            }
            // TODO: Fix ABA problem.
            // CondVarWake::One => self.update(CondVarWake::One, update, |_| new_state),
            CondVarWake::One | CondVarWake::All => {
                let cur = self.atomic.swap(
                    PtrState::new_null(State::new_truncated(new_state as usize)),
                    update,
                );

                let mut node = cur.ptr();

                while let Some(node_ref) = unsafe { node.as_ref() } {
                    node_ref.ready.store(true, update);
                    node_ref.thread.unpark();
                    node = node_ref.next;
                }

                cur.state().value() as u8
            }
        }
    }

    /// Simplified version of `update_wait_break` that
    /// never updates the state.
    /// It waits for the state to change,
    /// until `stop` returns `true` for current state.
    #[inline]
    pub fn wait(&self, load: Ordering, mut stop: impl FnMut(u8) -> bool) -> u8 {
        let result = self.update_wait_break(CondVarWake::None, load, Ordering::Relaxed, |state| {
            if stop(state) {
                CondVarUpdateOrWait::Break
            } else {
                CondVarUpdateOrWait::Wait
            }
        });

        match result {
            Ok(_) => unreachable!("Update variant is not used"),
            Err(state) => state,
        }
    }

    /// Waits until the state is equal to `target`.
    #[inline]
    pub fn wait_for(&self, load: Ordering, target: u8) {
        self.wait(load, |state| state == target);
    }
}
