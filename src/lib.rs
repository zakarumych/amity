//! # Amity
//!
//! Collection of concurrency algorithms.
//! The collection is not fixed and more algorithms will be added.
//! Algorithm may be removed from crate only if its implementation is unsound and cannot be fixed.
//! In this case it will be deprecated first and removed in later version.
//!
//! Most algorithms require its own feature flag to be enabled.
//!
//! ## Available Algorithms
//!
//! ### 🔄 Backoff
//! Provides utilities for implementing exponential backoff strategies, useful in retry mechanisms.
//!
//! #### Examples
//!
//! Here is an example of using the `BackOff` struct to implement exponential backoff when waiting for a resource:
//!
//! ```rust
//! use amity::backoff::BackOff;
//!
//! fn try_acquire_resource(attempt: u64) -> Option<Resource> {
//!     // Simulate trying to acquire a shared resource
//!     if attempt.wrapping_mul(0xfedcba9876543210) < u64::MAX / 10 {
//!         Some(Resource {})
//!     } else {
//!         None
//!     }
//! }
//!
//! struct Resource {}
//!
//! fn main() {
//!     let mut backoff = BackOff::new();
//!     let mut attempt = 0;
//!
//!     loop {
//!         // Try to acquire the resource
//!         attempt += 1;
//!         if let Some(resource) = try_acquire_resource(attempt) {
//!             // Resource acquired, use it
//!             println!("Resource acquired!");
//!             break;
//!         }
//!
//!         // Failed to acquire, check if we should block
//!         if backoff.should_block() {
//!             println!("Backoff limit reached, blocking thread");
//!             std::thread::sleep(std::time::Duration::from_millis(10));
//!             backoff.reset(); // Reset the backoff counter after blocking
//!         } else {
//!             // Wait with exponential backoff
//!             backoff.wait();
//!         }
//!     }
//! }
//! ```
//!
//! If blocking is not an option, you can use `BackOff::wait()`.
//!
//! ```rust
//! use amity::backoff::BackOff;
//!
//! fn try_acquire_resource(attempt: u64) -> Option<Resource> {
//!     // Simulate trying to acquire a shared resource
//!     if attempt.wrapping_mul(0xfedcba9876543210) < u64::MAX / 10 {
//!         Some(Resource {})
//!     } else {
//!         None
//!     }
//! }
//!
//! struct Resource {}
//!
//! fn main() {
//!     let mut backoff = BackOff::new();
//!     let mut attempt = 0;
//!     
//!     loop {
//!         // Try to acquire the resource
//!         attempt += 1;
//!         if let Some(resource) = try_acquire_resource(attempt) {
//!             // Resource acquired, use it
//!             println!("Resource acquired!");
//!             break;
//!         }
//!
//!         // Wait with exponential backoff
//!         backoff.wait();
//!     }
//! }
//! ```
//!
//! The `BackOff` struct helps manage contention by implementing an efficient waiting strategy - first spinning, then yielding, and finally suggesting when the thread should block completely.
//!
//! ### 🌀 Cache
//! Implements utilities for working with cache lines, optimizing memory access patterns in concurrent programming.
//!
//! #### Examples
//!
//! Here is an example of using the `CachePadded` struct to prevent false sharing in a concurrent scenario:
//!
//! ```rust
//! use amity::cache::CachePadded;
//! use std::{cell::UnsafeCell, sync::Arc, thread};
//!
//! struct SharedCounters {
//!     // Each counter is padded to avoid false sharing when accessed by different threads
//!     counter1: CachePadded<UnsafeCell<usize>>,
//!     counter2: CachePadded<UnsafeCell<usize>>,
//! }
//!
//! unsafe impl Sync for SharedCounters {}
//!
//! impl SharedCounters {
//!     fn new() -> Self {
//!         Self {
//!             counter1: CachePadded(UnsafeCell::new(0)),
//!             counter2: CachePadded(UnsafeCell::new(0)),
//!         }
//!     }
//! }
//!
//! let counters = Arc::new(SharedCounters::new());
//! let counters_clone = Arc::clone(&counters);
//!
//! // Thread 1 updates counter1
//! let thread1 = thread::spawn(move || {
//!     for _ in 0..1_000_000 {
//!         unsafe {
//!             // Need to use unsafe to get mutable access in this example
//!             let counter1 = &mut *counters_clone.counter1.get();
//!             *counter1 += 1;
//!         }
//!     }
//! });
//!
//! let counters_clone = Arc::clone(&counters);
//!
//! // Thread 2 updates counter2 without false sharing
//! let thread2 = thread::spawn(move || {
//!     for _ in 0..1_000_000 {
//!         unsafe {
//!             let counter2 = &mut *counters_clone.counter1.get();
//!             *counter2 += 1;
//!         }
//!     }
//! });
//!
//! thread1.join().unwrap();
//! thread2.join().unwrap();
//! ```
//!
//! This example demonstrates how `CachePadded` can be used to prevent false sharing when multiple threads need to update different data simultaneously.
//!
//! ### 📐 State Pointer
//! Combines state and pointer into a single atomic value, enabling efficient state management in concurrent programming.
//!
//! #### Examples
//!
//! Here is an example of using the `PtrState` and `AtomicPtrState` to efficiently combine state information with pointers:
//!
//! ```rust
//! use amity::state_ptr::{PtrState, State, AtomicPtrState};
//! use std::sync::atomic::Ordering;
//!
//! struct Node {
//!     data: u64,
//! }
//!
//! // Create a sample node
//! let mut node = Node { data: 42 };
//!
//! // Create a state value (limited by pointer alignment)
//! let state = State::<Node>::new(3).unwrap();
//!
//! // Combine pointer and state into a single value
//! let ptr_state = PtrState::new_mut(&mut node, state);
//!
//! // Extract pointer and state separately
//! let ptr = ptr_state.ptr();
//! let extracted_state = ptr_state.state();
//!
//! // Use the extracted pointer safely
//! unsafe {
//!     assert_eq!((*ptr).data, 42);
//! }
//!
//! println!("State value: {}", extracted_state.value());
//!
//! // Atomic version for thread-safe operations
//! let atomic_ptr_state = AtomicPtrState::new_mut(&mut node, state);
//!
//! // Load the combined value atomically
//! let loaded_ptr_state = atomic_ptr_state.load(Ordering::Acquire);
//!
//! // Update state while preserving the pointer
//! let new_state = State::<Node>::new(5).unwrap();
//! let updated_ptr_state = loaded_ptr_state.with_state(new_state);
//!
//! // Store the updated value atomically
//! atomic_ptr_state.store(updated_ptr_state, Ordering::Release);
//!
//! println!("New state value: {}",
//!     atomic_ptr_state.load(Ordering::Relaxed).state().value());
//! ```
//!
//! This pattern is useful in concurrent data structures where you need to pack flags or other state information with pointers without additional memory overhead. For example, it can be used in lock-free algorithms to mark nodes as "deleted" or to store version counters for ABA problem prevention.
//!
//! ### 🔗 Ring Buffer
//! Implements simple ring-buffer that can be used to build concurrent data structures.
//! **Feature:** `ring-buffer`
//!
//! #### Examples
//!
//! Here is an example of using the `RingBuffer` struct for efficient FIFO operations:
//!
//! ```rust
//! # #[cfg(feature = "ring-buffer")]
//! # {
//! use amity::ring_buffer::RingBuffer;
//!
//! // Create a new ring buffer
//! let mut buffer = RingBuffer::<i32>::new();
//!
//! // Push some elements
//! buffer.push(10);
//! buffer.push(20);
//! buffer.push(30);
//!
//! // Check the buffer status
//! println!("Buffer length: {}", buffer.len());
//! println!("Buffer capacity: {}", buffer.capacity());
//!
//! // Pop elements (FIFO order)
//! while let Some(value) = buffer.pop() {
//!     println!("Popped value: {}", value);
//! }
//!
//! // Buffer is now empty
//! assert!(buffer.is_empty());
//!
//! // Using with_capacity for performance
//! let mut buffer = RingBuffer::<String>::with_capacity(10);
//!
//! // Fill the buffer
//! for i in 0..5 {
//!     buffer.push(format!("Item {}", i));
//! }
//!
//! // Use drain to consume all elements
//! for item in buffer.drain() {
//!     println!("Drained: {}", item);
//! }
//!
//! // Buffer is automatically cleared after drain
//! assert!(buffer.is_empty());
//! # }
//! ```
//!
//! Ring buffers are particularly useful in scenarios requiring fixed-size queues or when implementing producer-consumer patterns where elements need to be processed in order.
//!
//! ### 🔃 Flip Queue
//! Queue implementation that allows concurrent writes, but only exclusive reads.
//! This is useful for scenarios where multiple threads need to write data concurrently, and single thread swaps inner and own `RingBuffer` to read it.
//!
//! `FlipBuffer` is lockless version and requires mutable access to read and to expand internal buffer when full.
//!
//! `FlipQueue` uses read-write lock to allow concurrent pushes until buffer is full,
//! and locks it for exclusive access to grow buffer and to swap it with reader.
//! **Feature:** `flip-queue`
//!
//! #### Examples
//!
//! Here is an example of using the `FlipQueue` for concurrent writes from multiple threads and exclusive reads:
//!
//! ```rust
//! # #[cfg(feature = "flip-queue")]
//! # {
//! use amity::flip_queue::FlipQueue;
//! use amity::ring_buffer::RingBuffer;
//! use std::sync::Arc;
//! use std::thread;
//!
//! // Create a shared flip queue with initial capacity
//! let queue = Arc::new(FlipQueue::<usize>::with_capacity(16));
//!
//! // Spawn multiple producer threads
//! let mut handles = vec![];
//! for thread_id in 0..4 {
//!     let queue_clone = Arc::clone(&queue);
//!     let handle = thread::spawn(move || {
//!         for i in 0..25 {
//!             let value = thread_id * 100 + i;
//!             queue_clone.push_sync(value);
//!             println!("Thread {} pushed {}", thread_id, value);
//!         }
//!     });
//!     handles.push(handle);
//! }
//!
//! // Wait for producers to finish
//! for handle in handles {
//!     handle.join().unwrap();
//! }
//!
//! // Using drain_locking for exclusive access
//! let items: Vec<_> = queue.drain_locking(|drain| drain.collect());
//! println!("Collected {} items", items.len());
//!
//! // Alternative approach: swap with an empty buffer for bulk processing
//! let mut queue = FlipQueue::<String>::new();
//!
//! // Add some items
//! for i in 0..10 {
//!     queue.push(format!("Item {}", i));
//! }
//!
//! // Prepare an empty buffer to swap with
//! let mut buffer = RingBuffer::new();
//!
//! // Swap buffers - very efficient for bulk processing
//! queue.swap_buffer(&mut buffer);
//!
//! // Process items from the swapped buffer
//! while let Some(item) = buffer.pop() {
//!     println!("Processing: {}", item);
//! }
//! # }
//! ```
//!
//! This pattern is especially useful when you have multiple producers that need to add data concurrently without blocking each other, but processing happens on a single consumer thread. The `swap_buffer` method provides a very efficient way to batch process items without holding locks for extended periods.
//!
//! ### 🔺 Triple
//! Implements triple-buffering for wait-free data transfer between single producer single and consumer threads.
//! This allows for efficient data exchange without the need for locks.
//!
//! Both consumer and producer has exclusive access to its own slot, allowing taking a mutable reference to it, which grants the ability to modify data in place, unlike channel-based approaches.
//!
//! **Feature:** `triple`
//!
//! #### Examples
//!
//! Here is an example of using the `TripleBuffer`:
//!
//! ```rust
//! # #[cfg(feature = "triple")]
//! # {
//! use amity::triple::TripleBuffer;
//!
//! // Create a new triple buffer with initial values
//! let mut buffer = TripleBuffer::<u32>::default();
//!
//! // Split the buffer into producer and consumer
//! let (mut producer, mut consumer) = buffer.split_mut();
//!
//! // Producer updates its element
//! *producer.get_mut() = 42;
//!
//! // Publish the updated element
//! producer.publish();
//!
//! // Consumer consumes the element
//! if consumer.consume() {
//!     println!("Consumed value: {}", consumer.get());
//! }
//! # }
//! ```
//!
//! ### 📡 Broad
//! A broadcast mechanism to notify multiple listeners of events concurrently.  
//! **Feature:** `broad`
//!
//! #### Examples
//!
//! Here is an example of using the `broad` module:
//!
//! ```rust
//! # #[cfg(feature = "broad")]
//! # {
//! use amity::broad::{Receiver, Sender};
//!
//! // Create a new broadcast channel with an initial value
//! let mut tx = Sender::new(0u32);
//! let mut rx = tx.receiver();
//!
//! // Sender sends a new value
//! tx.send(42);
//!
//! // Receiver receives the new value
//! if let Some(value) = rx.recv() {
//!     println!("Received value: {}", value);
//! }
//! # }
//! ```
//!
//! ### 🔁 Spin
//! Provides a low-latency spinlock implementation for mutual exclusion in critical sections. Also includes a not-totally-unfair read-write spin lock for efficient concurrent read and write operations.  
//! **Feature:** `spin`
//!
//! #### Examples
//!
//! Here is an example of using the `Spin` mutex for mutual exclusion:
//!
//! ```rust
//! # #[cfg(feature = "spin")]
//! # {
//! use amity::spin::Spin;
//! use std::sync::Arc;
//! use std::thread;
//!
//! let counter = Arc::new(Spin::new(0));
//! let mut handles = vec![];
//!
//! // Spawn multiple threads that increment the counter
//! for _ in 0..10 {
//!     let counter_clone = Arc::clone(&counter);
//!     let handle = thread::spawn(move || {
//!         for _ in 0..100 {
//!             // Lock the mutex and update the counter
//!             let mut count = counter_clone.lock();
//!             *count += 1;
//!             // Mutex automatically unlocks when `count` goes out of scope
//!         }
//!     });
//!     handles.push(handle);
//! }
//!
//! // Wait for all threads to complete
//! for handle in handles {
//!     handle.join().unwrap();
//! }
//!
//! println!("Final count: {}", *counter.lock());
//! assert_eq!(*counter.lock(), 1000);
//! # }
//! ```
//!
//! Here is an example of using the `RwSpin` read-write lock for concurrent read access and exclusive write access:
//!
//! ```rust
//! # #[cfg(feature = "spin")]
//! # {
//! use amity::spin::RwSpin;
//! use std::sync::Arc;
//! use std::thread;
//! use std::time::Duration;
//!
//! let data = Arc::new(RwSpin::new(vec![1, 2, 3, 4]));
//! let mut handles = vec![];
//!
//! // Spawn reader threads
//! for i in 0..3 {
//!     let data_clone = Arc::clone(&data);
//!     let handle = thread::spawn(move || {
//!         for _ in 0..5 {
//!             // Acquire a read lock - multiple readers can access simultaneously
//!             let values = data_clone.read();
//!             println!("Reader {}: Current values: {:?}", i, *values);
//!             
//!             // Simulate some processing time
//!             thread::sleep(Duration::from_millis(10));
//!             
//!             // Read lock automatically released when `values` goes out of scope
//!         }
//!     });
//!     handles.push(handle);
//! }
//!
//! // Spawn writer threads
//! for i in 0..2 {
//!     let data_clone = Arc::clone(&data);
//!     let handle = thread::spawn(move || {
//!         for j in 0..3 {
//!             // Acquire a write lock - exclusive access
//!             let mut values = data_clone.write();
//!             values.push(i * 10 + j);
//!             println!("Writer {}: Added value {}", i, i * 10 + j);
//!             
//!             // Simulate some processing time
//!             thread::sleep(Duration::from_millis(50));
//!             
//!             // Write lock automatically released when `values` goes out of scope
//!         }
//!     });
//!     handles.push(handle);
//! }
//!
//! // Wait for all threads to complete
//! for handle in handles {
//!     handle.join().unwrap();
//! }
//!
//! // Print the final state
//! let final_data = data.read();
//! println!("Final values: {:?}", *final_data);
//! # }
//! ```
//!
//! ## `no-std` support
//!
//! If algorithm requires `std` its feature name will have `-std` suffix and enable `std` feature.
//!

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(clippy::pedantic)]
#![allow(clippy::inline_always)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod backoff;
pub mod cache;
pub mod state_ptr;

#[cfg(feature = "broad")]
pub mod broad;

#[cfg(feature = "flip-queue")]
pub mod flip_queue;

#[cfg(feature = "ring-buffer")]
pub mod ring_buffer;

#[cfg(feature = "spin")]
pub mod spin;

#[cfg(feature = "triple")]
pub mod triple;

#[cfg(all(feature = "spin", not(feature = "parking_lot")))]
#[allow(dead_code)]
type DefaultRawRwLock = spin::RawRwSpin;

#[cfg(feature = "parking_lot")]
#[allow(dead_code)]
type DefaultRawRwLock = parking_lot::RawRwLock;

// One central function responsible for reporting capacity overflows. This'll
// ensure that the code generation related to these panics is minimal as there's
// only one location which panics rather than a bunch throughout the module.
#[cfg(feature = "alloc")]
#[allow(dead_code)]
#[cold]
#[inline(never)]
fn capacity_overflow() -> ! {
    panic!("capacity overflow");
}

// pub enum Ordering {
//     Relaxed,
//     Release,
//     Acquire,
//     AcqRel,
//     SeqCst,
// }

// #[inline]
// fn merge_ordering(lhs: Ordering, rhs: Ordering) -> Ordering {
//     match (lhs, rhs) {
//         (Ordering::SeqCst, _) => Ordering::SeqCst,
//         (_, Ordering::SeqCst) => Ordering::SeqCst,
//         (Ordering::AcqRel, _) => Ordering::AcqRel,
//         (_, Ordering::AcqRel) => Ordering::AcqRel,
//         (Ordering::Acquire, Ordering::Release) => Ordering::AcqRel,
//         (Ordering::Release, Ordering::Acquire) => Ordering::AcqRel,
//         (Ordering::Acquire, _) => Ordering::Acquire,
//         (_, Ordering::Acquire) => Ordering::Acquire,
//         (Ordering::Release, _) => Ordering::Release,
//         (_, Ordering::Release) => Ordering::Release,
//         (Ordering::Relaxed, Ordering::Relaxed) => Ordering::Relaxed,
//         _ => unreachable!("amity does not use any other ordering"),
//     }
// }
