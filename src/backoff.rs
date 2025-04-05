/// Waiting hint.
/// Provides common exponential spin logic.
/// When spin count exceeds spin limit it switches to yield when "std" feature is enabled.
/// When spin count exceeds yield limit it advises caller to block thread.
#[derive(Default)]
pub struct BackOff {
    spin_count: usize,
}

impl BackOff {
    const SPIN_THRESHOLD: usize = 7;
    const YIELD_THRESHOLD: usize = 15;

    #[inline(always)]
    #[must_use]
    pub fn new() -> Self {
        BackOff { spin_count: 0 }
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.spin_count = 0;
    }

    #[inline(always)]
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

    #[must_use]
    pub fn should_block(&self) -> bool {
        self.spin_count >= Self::YIELD_THRESHOLD
    }
}
