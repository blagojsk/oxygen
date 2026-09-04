//! How a capability describes itself: which methods its interface exposes, what each one takes,
//! what it hands back — and a fixed byte layout for carrying that description across a syscall
//! boundary.
//!
//! The capability system already tells a holder *what kind* of thing a handle names — console,
//! task, memory. That answers "what is this?" and leaves "what can I do with it?" unanswered:
//! which methods exist, what arguments each takes, what it returns. This crate is that second
//! answer, structured the same way [`oxygen_ipc::Message`] structures a call itself — data a
//! holder can read, not prose it has to already know or go find in source it may not have.
//!
//! The reason to build it as data rather than documentation: the identical [`Interface`] value
//! renders as a help listing for a person and as a tool definition for an agent driving the
//! system programmatically. SPECS.md names this invariant "one surface, two audiences" — this
//! crate is what makes it hold for a capability's methods specifically, the way
//! [`oxygen_ipc::Registry`] already makes it hold for service names. A system whose operations
//! can only be called by someone who already read the source is not discoverable, whichever
//! audience is asking.
//!
//! Pure logic, like the other portable crates: `#![no_std]`, no allocator, no hardware, fully
//! exercised on the host under `#[cfg(test)]`. The only dependency is `oxygen-ipc`, for
//! [`Name`](oxygen_ipc::Name) — the same short printable label the registry uses, kept as one
//! name type per concept rather than a second one defined here for the same purpose.

#![no_std]

pub mod interface;
pub mod method;
pub mod table;

pub use interface::{Interface, MAX_METHODS};
pub use method::{ArgKind, ENCODED_METHOD_BYTES, MAX_ARGS, Method};
pub use table::SchemaTable;

/// Why a schema operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaError {
    /// [`Method::new`] given more than [`MAX_ARGS`] arguments.
    TooManyArgs,
    /// [`Interface::add`] called past [`MAX_METHODS`].
    TooManyMethods,
    /// [`Interface::new`] given id `0` — reserved the same way `oxygen_ipc` reserves interface
    /// `0` on a message as untyped, so a capability with no schema and a message addressed to
    /// nothing in particular are the same caught mistake in both crates, not two different ones.
    UntypedInterface,
    /// [`Method::encode`] given a buffer shorter than [`ENCODED_METHOD_BYTES`].
    BufferTooSmall,
    /// [`Method::decode`] given a buffer that is not one of this crate's own encodings: too
    /// short, an unrecognised kind byte, or a name field that is not valid UTF-8/ASCII once its
    /// padding is trimmed.
    Malformed,
    /// [`SchemaTable::register`] given an id that is already registered. The existing entry is
    /// left in place.
    DuplicateInterface,
    /// [`SchemaTable::register`] with no free slot for another interface.
    TableFull,
}
