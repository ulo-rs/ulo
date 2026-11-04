//! Runtime-free primitives for the authors of transport links and hosts: [`ordered`], the bounded
//! queue a link's writer is fed through so frames leave in the order their sends were called, and
//! [`Watch`], a value changed from synchronous code that tasks wait on under a predicate. Neither
//! needs an executor: each is a `std` mutex and an `event-listener` event, so a link or host on any
//! runtime uses them, and the first-party links and hosts use these same types.
//!
//! Semver applies to this module as to the rest of the crate's public API: from 0.1.0, a change
//! that breaks a caller is released under a new breaking version (`0.x` to `0.(x+1)` before 1.0).
//! A pre-release of 0.1.0 may break it, and Cargo takes a later pre-release of the same version
//! for a requirement on an earlier one, so a crate depending on a pre-release pins it with `=`.

pub mod ordered;
mod watch;

pub use watch::{Changed, Watch};
