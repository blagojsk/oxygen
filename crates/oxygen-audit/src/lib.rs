//! The audit journal: the single record of every grant, delegation, revocation and refusal in
//! the system, in order, with who did it.
//!
//! This crate exists because of one of the system's invariants: an approval *is* a capability
//! grant, and the audit journal *is* what task tracking reads — there is deliberately no second
//! mechanism for either. A human approving what an agent may do and a human auditing what an
//! agent already did are the same act of oversight looked at from two directions, and this is the
//! one surface built to serve both: a human reads it to see what an agent did, an agent reads it
//! to see what it has been allowed. A separate approval log and a separate task-tracking store
//! would let those two views drift apart, which is exactly what the "one surface for two
//! audiences" invariant rules out.
//!
//! Pure logic, like the other portable crates: no allocator, no hardware, fully exercised on the
//! host under `#[cfg(test)]`. The kernel's job is only ever "call [`Journal::record`], then hand
//! a reader whatever [`Journal::since`] returns" — this crate does not know what a syscall or a
//! task is.
//!
//! ## Truncation is visible by design
//!
//! [`Journal`] is a ring, not an unbounded log — the machine this runs on cannot afford an
//! unbounded audit trail any more than it can an unbounded capability table (see the workspace
//! rule on justifying resident memory). A ring that silently discarded its oldest entries once
//! full would be an audit trail that lies about being complete, and a trail that lies is worse
//! than no trail at all, because it is the one that gets trusted. So every eviction is counted
//! in [`Journal::dropped`], which never resets — a reader comparing the count it last saw
//! against the current one can always tell whether it missed something, even though the missed
//! events themselves are gone for good.
//!
//! ## Sequence numbers, not read cursors
//!
//! Every recorded [`Event`] gets a `seq`: assigned once, strictly increasing, and never reused —
//! climbing forever even after the ring has wrapped a thousand times over. A reader on the other
//! side of the privilege boundary this journal sits behind cannot hold a borrow or a cursor into
//! kernel state between calls, so [`Journal::since`] gives it a stateless way to catch up instead:
//! remember the highest `seq` you have seen, ask for everything strictly newer, and use `dropped`
//! to know whether the answer has a gap in it.
//!
//! ## Wire encoding
//!
//! [`Event::encode`]/[`Event::decode`] give an event a fixed, documented byte layout
//! ([`ENCODED_EVENT_BYTES`] wide), and [`Action`] a stable `u8` encoding, because both cross a
//! syscall boundary into a program this crate was not compiled with — a `#[repr(Rust)]` type
//! carries no such promise on its own.

#![no_std]

pub mod action;
pub mod event;
pub mod journal;

pub use action::Action;
pub use event::{AuditError, ENCODED_EVENT_BYTES, Event};
pub use journal::Journal;
