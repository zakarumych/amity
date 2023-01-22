# Amity

[![crates](https://img.shields.io/crates/v/amity.svg?style=for-the-badge&label=amity)](https://crates.io/crates/amity)
[![docs](https://img.shields.io/badge/docs.rs-amity-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white)](https://docs.rs/amity)
[![actions](https://img.shields.io/github/actions/workflow/status/zakarumych/amity/badge.yml?branch=master&style=for-the-badge)](https://github.com/zakarumych/amity/actions/workflows/badge.yml)
[![MIT/Apache](https://img.shields.io/badge/license-MIT%2FApache-blue.svg?style=for-the-badge)](COPYING)
![loc](https://img.shields.io/tokei/lines/github/zakarumych/amity?style=for-the-badge)

Collection of concurrency algorithms.
Includes blocking, lock-free and wait-free algorithms
for different scenarios and purposes.

The collection is not fixed and more algorithms will be added.
Algorithm may be removed from crate only if its implementation is unsound and cannot be fixed.
In this case it will be deprecated first and removed in later version.

Most algorithms require its own feature flag to be enabled.

## `no-std` support

If algorithm requires `std` its feature name will have `-std` suffix and enable `std` feature.

## License

Licensed under either of

* Apache License, Version 2.0, ([license/APACHE](license/APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([license/MIT](license/MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
