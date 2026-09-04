//! Portable IPC logic for Oxygen: typed messages, the queue they wait in, and the registry a
//! program uses to find another program's object by name.
//!
//! This is M5's thesis in code: M4 made authority a capability handle the kernel checks rather
//! than a permission a caller happens to have. That only matters once two tasks can name the same
//! object across the boundary between them, which is what this crate gives them a shared,
//! typed way to do — a [`message::Message`] names an interface and a method instead of being an
//! opaque blob, and [`registry::Registry`] lets a name resolve to an object instead of that
//! object's identity being a number passed out of band. Both properties serve the same design
//! invariant: the surface is discoverable and structured, not just functional.
//!
//! Pure logic, like the other portable crates: no allocator, no hardware, fully exercised on the
//! host under `#[cfg(test)]`. The kernel side is only ever "call these methods, then journal what
//! happened" — this crate does not know what a syscall or a task is, and does not need to.

#![no_std]

pub mod message;
pub mod queue;
pub mod registry;

pub use message::{Header, MAX_PAYLOAD, Message};
pub use queue::MessageQueue;
pub use registry::{MAX_NAME, Name, Registry};

/// Why an IPC operation failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcError {
    /// A message body longer than [`MAX_PAYLOAD`].
    PayloadTooLarge,
    /// Interface `0`, which [`Message::new`] reserves as invalid — see the `message` module docs.
    ///
    /// [`Message::new`]: message::Message::new
    UntypedMessage,
    /// A [`MessageQueue`] has no free slot for another message.
    QueueFull,
    /// A name that is empty, longer than [`MAX_NAME`], or contains a byte outside printable ASCII
    /// — see the `registry` module docs.
    InvalidName,
    /// A [`Registry::register`] naming an already-bound [`Name`].
    NameTaken,
    /// A [`Registry`] has no free slot for another binding.
    RegistryFull,
}
