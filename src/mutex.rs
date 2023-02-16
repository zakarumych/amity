use core::sync::atomic::Ordering;

use crate::condvar::{CondVar, CondVarUpdateOrWait, CondVarWake};

const UNLOCKED: u8 = 0;
const LOCKED: u8 = 1;

pub struct MutexRaw {
    condvar: CondVar,
}

impl MutexRaw {
    pub const fn new() -> Self {
        Self {
            condvar: CondVar::new(0),
        }
    }

    /// Attempts to acquire the lock without blocking.
    /// Returns true if the lock was acquired, false otherwise.
    pub fn try_lock(&self) -> bool {
        self.condvar
            .update_break(
                CondVarWake::None,
                Ordering::Relaxed,
                Ordering::Acquire,
                |state| match state {
                    UNLOCKED => Some(LOCKED),
                    LOCKED | _ => None,
                },
            )
            .is_ok()
    }

    /// Blocking lock that returns when the lock is acquired.
    pub fn lock(&self) {
        self.condvar.update_wait(
            CondVarWake::None,
            Ordering::Relaxed,
            Ordering::Acquire,
            |state| match state {
                UNLOCKED => Some(LOCKED),
                LOCKED | _ => None,
            },
        );
    }

    /// Attempts to acquire the lock.
    /// Waits for the lock state change if the lock is already acquired.
    /// Returns true if the lock was acquired, false otherwise.
    pub fn try_lock_wait(&self) -> bool {
        let mut waited = false;
        self.condvar
            .update_wait_break(
                CondVarWake::None,
                Ordering::Relaxed,
                Ordering::Acquire,
                |state| {
                    if waited {
                        return CondVarUpdateOrWait::Break;
                    }
                    match state {
                        UNLOCKED => CondVarUpdateOrWait::Update(LOCKED),
                        LOCKED | _ => {
                            waited = true;
                            CondVarUpdateOrWait::Wait
                        }
                    }
                },
            )
            .is_ok()
    }

    pub fn unlock(&self) {
        self.condvar.set(CondVarWake::One, Ordering::Release, 0);
    }
}
