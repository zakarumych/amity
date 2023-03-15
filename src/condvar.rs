//!
//! Provides `CondVar` type that can be used to wait for state change
//!

use core::{
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{
    backoff::BackOff,
    park::{DefaultPark, Park, Unpark},
    state_ptr::{AtomicPtrState, PtrState, State},
};

#[cfg(feature = "std")]
pub type StdCondVar = CondVar<std::thread::Thread>;

#[repr(align(256))]
struct CondVarNode<T> {
    unpark: T,
    next: *mut Self,
    ready: AtomicBool,
}

/// Atomic condition variable.
/// It supports updating state value and waiting for an update.
/// It is implemented using lock-free algorithm
/// with exponential back-off and
/// optional thread parking when "std" feature is enabled.
#[repr(transparent)]
pub struct CondVar<T> {
    atomic: AtomicPtrState<CondVarNode<T>>,
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

impl<T> CondVar<T> {
    /// Number of bits available to store the state.
    pub const STATE_BITS: u32 = <PtrState<CondVarNode<T>>>::STATE_BITS;

    /// Mask for state bits.
    pub const STATE_MASK: usize = <PtrState<CondVarNode<T>>>::STATE_MASK;

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

    /// Loads the current state.
    #[inline]
    pub fn load(&self, load: Ordering) -> u8 {
        self.atomic.load(load).state().value() as u8
    }

    /// Atomically updates current state with optimistic assumption.
    ///
    /// It may fail even if the state is equal semantically to `old`
    /// if there are threads waiting for the state to change.
    ///
    /// `update` ordering is used for updating the state.
    /// Successful update is always done with `update` ordering.
    #[inline]
    pub fn optimistic_update(&self, update: Ordering, old_state: u8, new_state: u8) -> bool {
        let old_state = State::new_truncated(old_state as usize);
        let new_state = State::new_truncated(new_state as usize);
        self.atomic
            .compare_exchange(
                PtrState::new_null(old_state),
                PtrState::new_null(new_state),
                update,
                Ordering::Relaxed,
            )
            .is_ok()
    }
}

impl<T> CondVar<T>
where
    T: Unpark,
{
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
    /// When state is updated this function may wake other threads that wait for the state to change.
    /// This is controlled by `wake` parameter.
    /// When `wake` is `CondVarWake::None` no threads are woken.
    /// When `wake` is `CondVarWake::One` only one thread is woken. // Due to ABA hazard this is currently acts as `CondVarWake::All`.
    /// When `wake` is `CondVarWake::All` all threads are woken.
    #[inline]
    pub fn update_wait_break_park(
        &self,
        park: impl Park<T>,
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
                                        node_ref.unpark.unpark();
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

                    if backoff.should_block() {
                        // After few loops we should park the thread.
                        let mut node = CondVarNode {
                            unpark: park.unpark_token(),
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
                                        park.park();
                                    }

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
        let mut cur = self.atomic.load(load);
        let mut backoff = BackOff::new();

        loop {
            match f(cur.state().value() as u8) {
                Some(new_state) => {
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
                                        node_ref.unpark.unpark();
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
                None => {
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
    pub fn update_wait_park(
        &self,
        park: impl Park<T>,
        wake: CondVarWake,
        load: Ordering,
        update: Ordering,
        mut f: impl FnMut(u8) -> Option<u8>,
    ) -> u8 {
        let result =
            self.update_wait_break_park(park, wake, load, update, |state| match f(state) {
                Some(state) => CondVarUpdateOrWait::Update(state),
                None => CondVarUpdateOrWait::Wait,
            });
        match result {
            Ok(state) => state,
            Err(_) => unreachable!("Break variant is not used"),
        }
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
        let result = self.update_break(wake, load, update, |state| Some(f(state)));
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
                    node_ref.ready.store(true, Ordering::Release);

                    // Unpark strictly after setting the `ready` flag.
                    node_ref.unpark.unpark();

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
    pub fn wait_park(
        &self,
        park: impl Park<T>,
        load: Ordering,
        mut stop: impl FnMut(u8) -> bool,
    ) -> u8 {
        let result = self.update_wait_break_park(
            park,
            CondVarWake::None,
            load,
            Ordering::Relaxed,
            |state| {
                if stop(state) {
                    CondVarUpdateOrWait::Break
                } else {
                    CondVarUpdateOrWait::Wait
                }
            },
        );

        match result {
            Ok(_) => unreachable!("Update variant is not used"),
            Err(state) => state,
        }
    }

    /// Waits until the state is equal to `target`.
    #[inline]
    pub fn wait_for_park(&self, park: impl Park<T>, load: Ordering, target: u8) {
        self.wait_park(park, load, |state| state == target);
    }
}

impl<T> CondVar<T>
where
    T: DefaultPark,
{
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
    /// When state is updated this function may wake other threads that wait for the state to change.
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
        f: impl FnMut(u8) -> CondVarUpdateOrWait,
    ) -> Result<u8, u8> {
        self.update_wait_break_park(T::default_park(), wake, load, update, f)
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
        f: impl FnMut(u8) -> Option<u8>,
    ) -> u8 {
        self.update_wait_park(T::default_park(), wake, load, update, f)
    }

    /// Simplified version of `update_wait_break` that
    /// never updates the state.
    /// It waits for the state to change,
    /// until `stop` returns `true` for current state.
    #[inline]
    pub fn wait(&self, load: Ordering, stop: impl FnMut(u8) -> bool) -> u8 {
        self.wait_park(T::default_park(), load, stop)
    }

    /// Waits until the state is equal to `target`.
    #[inline]
    pub fn wait_for(&self, load: Ordering, target: u8) {
        self.wait_for_park(T::default_park(), load, target)
    }
}
