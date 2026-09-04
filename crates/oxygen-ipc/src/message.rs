//! The unit IPC moves: a header naming an interface and a method, plus a small inline payload.
//!
//! A message never allocates and never outlives the call that sent it — the payload lives inline
//! in the [`Message`] value itself, not behind a pointer into memory the sender might reuse or
//! the receiver might outlive. That is what keeps IPC usable on a machine with tens of megabytes:
//! no per-message allocation, no lifetime to track across a task boundary, and a queue of them
//! (see [`crate::queue`]) is just an array.
//!
//! ## Why the payload is capped at 64 bytes
//!
//! A fixed inline size is a deliberate trade against a variable-length one: a variable payload
//! needs either an allocator (which the kernel does not always have on the path a syscall takes)
//! or a shared buffer with its own lifetime and capability of its own. 64 bytes is enough for a
//! method call's arguments or a small reply — a handle, a status, a short name — and anything
//! larger is a bulk transfer that belongs behind a capability of its own, not stuffed through the
//! control path.
//!
//! ## Why interface `0` is reserved
//!
//! [`Message::new`] refuses interface `0`. Reserving one value out of the whole `u32` space costs
//! nothing and buys a real diagnostic: a caller that forgot to set the interface field — zeroed
//! memory being the default — gets `UntypedMessage` back instead of a message that is silently
//! addressed to nothing in particular. Without the reservation, "forgot to set it" and "meant
//! interface zero" are indistinguishable to everything downstream.

use crate::IpcError;

/// Largest body a [`Message`] can carry, in bytes. See the module docs for why this is fixed
/// rather than variable.
pub const MAX_PAYLOAD: usize = 64;

/// What a message is *about*, independent of its bytes.
///
/// Naming an interface and a method (rather than handing over an opaque blob) is what "typed
/// IPC" means here: the kernel can check `interface != 0` and route or reject on that alone,
/// where an untyped blob's meaning is a private convention between two programs and invisible to
/// everything else — including the audit journal that has to describe what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Which interface `method` is a member of. Never `0` on a message built through
    /// [`Message::new`] — see the module docs.
    pub interface: u32,
    /// Which operation on `interface` this message invokes.
    pub method: u32,
    /// How many bytes of `Message::payload` are meaningful. See [`Message::body`].
    pub len: u32,
}

/// A header plus its inline payload.
///
/// `Copy`, like [`Header`]: a message is a value, not a resource, so sending one is a copy into
/// the receiver's queue rather than a move that could leave the sender holding a handle to
/// nothing. `PartialEq` compares the whole 64-byte payload array, padding included — callers who
/// only care about the meaningful prefix should compare `body()`, not the message itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub header: Header,
    pub payload: [u8; MAX_PAYLOAD],
}

impl Message {
    /// Builds a message addressed to `interface`/`method`, copying `body` into the inline
    /// payload.
    ///
    /// Fails `PayloadTooLarge` if `body` is longer than [`MAX_PAYLOAD`], and `UntypedMessage` if
    /// `interface` is `0` — see the module docs for both.
    pub fn new(interface: u32, method: u32, body: &[u8]) -> Result<Message, IpcError> {
        if interface == 0 {
            return Err(IpcError::UntypedMessage);
        }
        if body.len() > MAX_PAYLOAD {
            return Err(IpcError::PayloadTooLarge);
        }
        let mut payload = [0u8; MAX_PAYLOAD];
        payload[..body.len()].copy_from_slice(body);
        Ok(Message {
            header: Header {
                interface,
                method,
                len: body.len() as u32,
            },
            payload,
        })
    }

    /// A message with no body — a call or reply that carries only its header.
    ///
    /// Built through [`Message::new`] rather than assembled directly, so it is refused the same
    /// way for interface `0` instead of offering a second, more permissive path to the same
    /// invalid state.
    pub fn empty(interface: u32, method: u32) -> Result<Message, IpcError> {
        Self::new(interface, method, &[])
    }

    /// The message body: the first `header.len` bytes of the payload, never the zero-padded rest.
    pub fn body(&self) -> &[u8] {
        &self.payload[..self.header.len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_round_trips_and_excludes_padding() {
        let msg = Message::new(1, 2, b"hello").unwrap();
        assert_eq!(msg.body(), b"hello");
        assert_eq!(msg.header.len, 5);
        // The padding past `len` must never leak out of `body()`.
        assert_eq!(msg.payload[5], 0);
    }

    #[test]
    fn max_payload_is_accepted_one_more_is_rejected() {
        let max_body = [0xAAu8; MAX_PAYLOAD];
        assert!(Message::new(1, 0, &max_body).is_ok());

        let over = [0xAAu8; MAX_PAYLOAD + 1];
        assert_eq!(Message::new(1, 0, &over), Err(IpcError::PayloadTooLarge));
    }

    #[test]
    fn interface_zero_is_untyped() {
        assert_eq!(Message::new(0, 1, &[]), Err(IpcError::UntypedMessage));
        assert_eq!(Message::empty(0, 1), Err(IpcError::UntypedMessage));
    }

    #[test]
    fn empty_carries_no_body() {
        let msg = Message::empty(7, 9).unwrap();
        assert_eq!(msg.header.interface, 7);
        assert_eq!(msg.header.method, 9);
        assert_eq!(msg.header.len, 0);
        assert!(msg.body().is_empty());
    }
}
