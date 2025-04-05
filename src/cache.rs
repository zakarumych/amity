use core::ops::{Deref, DerefMut};

#[cfg_attr(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64"
    ),
    repr(align(128))
)]
#[cfg_attr(
    any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon"
    ),
    repr(align(32))
)]
#[cfg_attr(target_arch = "m68k", repr(align(16)))]
#[cfg_attr(target_arch = "s390x", repr(align(256)))]
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon",
        target_arch = "m68k",
        target_arch = "s390x",
    )),
    repr(align(64))
)]
/// In concurrent programming, false sharing is a performance-degrading situation that can arise
/// when two or more cores are modifying different variables that reside on the same cache line.
///
/// Cache lines are assumed to be N bytes long, where N depends on the architecture:
/// - `x86_64`, aarch64 and powerpc: 64 bytes, but N is 128 as prefetching pulls pairs of cache lines on some CPUs.
/// - arm, mips, mips32r6, mips64, mips64r6, sparc and hexagon: 32 bytes
/// - m68k: 16 bytes
/// - s390x: 256 bytes
/// - others: 64 bytes. Just some default for other architectures.
///
/// Note that alignment MAY be different than cache line size of the actual CPU the program runs on,
/// as cache line size vary between CPUs with the same architecture.
///
/// Adding this structure as a field to another structure will ensure that it will be aligned to cache lines,
/// removing false sharing.
/// It derives common traits for convenience.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CacheAlign;
#[cfg_attr(
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64"
    ),
    repr(align(128))
)]
#[cfg_attr(
    any(
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon"
    ),
    repr(align(32))
)]
#[cfg_attr(target_arch = "m68k", repr(align(16)))]
#[cfg_attr(target_arch = "s390x", repr(align(256)))]
#[cfg_attr(
    not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "powerpc64",
        target_arch = "arm",
        target_arch = "mips",
        target_arch = "mips32r6",
        target_arch = "mips64",
        target_arch = "mips64r6",
        target_arch = "sparc",
        target_arch = "hexagon",
        target_arch = "m68k",
        target_arch = "s390x",
    )),
    repr(align(64))
)]
/// In concurrent programming, false sharing is a performance-degrading situation that can arise
/// when two or more cores are modifying different variables that reside on the same cache line.
///
/// Cache lines are assumed to be N bytes long, where N depends on the architecture:
/// - `x86_64`, aarch64 and powerpc: 64 bytes, but N is 128 as prefetching pulls pairs of cache lines on some CPUs.
/// - arm, mips, mips32r6, mips64, mips64r6, sparc and hexagon: 32 bytes
/// - m68k: 16 bytes
/// - s390x: 256 bytes
/// - others: 64 bytes. Just some default for other architectures.
///
/// Note that alignment MAY be different than cache line size of the actual CPU the program runs on,
/// as cache line size vary between CPUs with the same architecture.
///
/// This structure contains a single value of type `T` that is aligned to cache lines, removing false sharing.
/// It derives common traits for convenience.
#[derive(Clone, Copy, Debug, Default, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct CachePadded<T>(pub T);

impl<T> Deref for CachePadded<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for CachePadded<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
