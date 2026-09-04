//! The capability space: a task's flat table of capability slots, plus a derivation tree so a
//! granted capability can be withdrawn.
//!
//! Pure logic, no hardware, no allocator — the table lives inline in whatever resident memory the
//! kernel gives a task, which is what makes it cheap enough to hold one per task rather than a
//! single system-wide table gated by a lock. Everything here is host-testable; the kernel side is
//! only ever "call these methods, then journal what happened" — the audit journal is what
//! tracking reads, and this crate does not invent a second bookkeeping mechanism alongside it.
//!
//! Authority is unforgeable by construction, not by convention: a [`Handle`] is an opaque `u64`
//! that userspace cannot turn into access to someone else's object, because [`CapSpace::get`]
//! checks both that the slot is occupied and that its generation matches the one the handle was
//! issued with.

#![no_std]

pub mod handle;
pub mod object;
pub mod rights;
pub mod space;

pub use handle::Handle;
pub use object::Object;
pub use rights::Rights;
pub use space::{CapError, CapSpace};
