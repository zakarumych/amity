/// Waiting hint.
/// Provides common exponential spin logic.
/// When spin count exceeds spin limit it switches to yield when "std" feature is enabled.
/// When spin count exceeds yield limit it advises caller to block thread.
pub struct BackOff {
    spin_count: usize,
}

impl BackOff {
    const SPIN_THRESHOLD: usize = 7;
    const YIELD_THRESHOLD: usize = 15;

    #[inline(always)]
    pub fn new() -> Self {
        BackOff { spin_count: 0 }
    }

    #[inline(always)]
    pub fn reset(&mut self) {
        self.spin_count = 0;
    }

    #[inline(always)]
    fn spin_loop(&self) {
        #[cfg(not(loom))]
        {
            for _ in 0..1 << self.spin_count {
                crate::sync::spin_loop();
            }
        }
    }

    #[inline(always)]
    pub fn lock_free_wait(&mut self) {
        self.spin_loop();

        if self.spin_count < Self::SPIN_THRESHOLD {
            self.spin_count += 1;
        }
    }

    #[inline(always)]
    pub fn blocking_wait(&mut self) {
        if self.spin_count < Self::SPIN_THRESHOLD {
            self.spin_loop();
        } else {
            #[cfg(feature = "std")]
            crate::sync::yield_now();

            #[cfg(not(feature = "std"))]
            self.spin_loop();
        }

        if self.spin_count < Self::YIELD_THRESHOLD {
            self.spin_count += 1;
        }
    }

    pub fn should_block(&self) -> bool {
        self.spin_count >= Self::YIELD_THRESHOLD
    }
}
