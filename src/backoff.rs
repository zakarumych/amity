//! Backoff for retry loops under contention.

/// Waiting hint.
/// Each wait issues a spin hint until the spin threshold. Subsequent waits
/// yield the thread with the `std` feature and continue spinning otherwise.
/// After enough waits, [`should_block`](Self::should_block) advises the caller to block.
#[derive(Default)]
pub struct BackOff {
    spin_count: usize,
}

impl BackOff {
    const SPIN_THRESHOLD: usize = 7;
    const YIELD_THRESHOLD: usize = 15;

    /// Starts a backoff in the spin phase.
    #[inline]
    #[must_use]
    pub fn new() -> Self {
        BackOff { spin_count: 0 }
    }

    /// Resets the wait count to zero.
    #[inline]
    pub fn reset(&mut self) {
        self.spin_count = 0;
    }

    /// Performs one backoff step.
    #[inline]
    pub fn wait(&mut self) {
        if self.spin_count < Self::SPIN_THRESHOLD {
            core::hint::spin_loop();
        } else {
            #[cfg(feature = "std")]
            std::thread::yield_now();

            #[cfg(not(feature = "std"))]
            core::hint::spin_loop();
        }

        if self.spin_count < Self::YIELD_THRESHOLD {
            self.spin_count += 1;
        }
    }

    /// Returns whether enough waits have elapsed to recommend blocking.
    #[must_use]
    pub fn should_block(&self) -> bool {
        self.spin_count >= Self::YIELD_THRESHOLD
    }
}
