# Amity

[![crates](https://img.shields.io/crates/v/amity.svg?style=for-the-badge&label=amity)](https://crates.io/crates/amity)
[![docs](https://img.shields.io/badge/docs.rs-amity-66c2a5?style=for-the-badge&labelColor=555555&logoColor=white)](https://docs.rs/amity)
[![actions](https://img.shields.io/github/actions/workflow/status/zakarumych/amity/badge.yml?branch=master&style=for-the-badge)](https://github.com/zakarumych/amity/actions/workflows/badge.yml)
[![MIT/Apache](https://img.shields.io/badge/license-MIT%2FApache-blue.svg?style=for-the-badge)](COPYING)
![loc](https://img.shields.io/tokei/lines/github/zakarumych/amity?style=for-the-badge)

Collection of concurrency algorithms.
The collection is not fixed and more algorithms will be added.
Algorithm may be removed from crate only if its implementation is unsound and cannot be fixed.
In this case it will be deprecated first and removed in later version.

Most algorithms require its own feature flag to be enabled.

## Available Algorithms

### 🔄 Backoff
Provides utilities for implementing exponential backoff strategies, useful in retry mechanisms.

### 🌀 Cache
Implements utilities for working with cache lines, optimizing memory access patterns in concurrent programming.

### 📐 State Pointer
Combines state and pointer into a single atomic value, enabling efficient state management in concurrent programming.

### 🔗 Ring Buffer
Implements simple ring-buffer that can be used to build concurrent data structures.
**Feature:** `ring-buffer`

### 🔃 Flip Queue
Queue implementation that allows concurrent writes, but only exclusive reads.
This is useful for scenarios where multiple threads need to write data concurrently, and single thread swaps inner and own `RingBuffer` to read it.

`FlipBuffer` is lockless version and requires mutable access to read and to expand internal buffer when full.

`FlipQueue` uses read-write lock to allow concurrent pushes until buffer is full,
and locks it for exclusive access to grow buffer and to swap it with reader.
**Feature:** `flip-queue`

### 🔺 Triple
Implements triple-buffering for wait-free data transfer between single producer single and consumer threads.
This allows for efficient data exchange without the need for locks.

Both consumer and producer has exclusive access to its own slot, allowing taking a mutable reference to it, which grants the ability to modify data in place, unlike channel-based approaches.

**Feature:** `triple`

#### Examples

Here is an example of using the `TripleBuffer`:

```rust
use amity::triple::TripleBuffer;

fn main() {
    // Create a new triple buffer with initial values
    let mut buffer = TripleBuffer::<u32>::default();

    // Split the buffer into producer and consumer
    let (mut producer, mut consumer) = buffer.split_mut();

    // Producer updates its element
    *producer.get_mut() = 42;

    // Publish the updated element
    producer.publish();

    // Consumer consumes the element
    if consumer.consume() {
        println!("Consumed value: {}", consumer.get());
    }
}
```

### 📡 Broad
A broadcast mechanism to notify multiple listeners of events concurrently.  
**Feature:** `broad`

#### Examples

Here is an example of using the `broad` module:

```rust
use amity::broad::{broadcast, BroadGet, BroadSet};

fn main() {
    // Create a new broadcast channel with an initial value
    let (mut sender, mut receiver) = broadcast(0u32);

    // Sender sends a new value
    sender.send(42);

    // Receiver receives the new value
    if let Some(value) = receiver.recv() {
        println!("Received value: {}", value);
    }
}
```

### 🔁 Spin
Provides a low-latency spinlock implementation for mutual exclusion in critical sections. Also includes a not-totally-unfair read-write spin lock for efficient concurrent read and write operations.  
**Feature:** `spin`

## `no-std` support

If algorithm requires `std` its feature name will have `-std` suffix and enable `std` feature.

## License

Licensed under either of

* Apache License, Version 2.0, ([license/APACHE](license/APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
* MIT license ([license/MIT](license/MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contributions

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
