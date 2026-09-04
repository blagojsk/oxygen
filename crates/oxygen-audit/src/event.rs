//! [`Event`]: one journal entry, and the fixed byte layout it takes across the syscall boundary.

use crate::action::Action;

/// The width of an [`Event`]'s encoded form. See [`Event::encode`] for the field layout this
/// counts.
pub const ENCODED_EVENT_BYTES: usize = 32;

/// Why encoding or decoding an [`Event`] failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditError {
    /// The buffer passed to [`Event::encode`] is smaller than [`ENCODED_EVENT_BYTES`].
    BufferTooSmall,
    /// The input to [`Event::decode`] is shorter than [`ENCODED_EVENT_BYTES`], or its action byte
    /// names no [`Action`]. Both mean the same thing to a reader: these bytes did not come from a
    /// matching [`Event::encode`].
    Malformed,
}

/// One recorded happening: who did what, to which kind of object, with which rights, and
/// whatever extra detail the action needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    /// Assigned by [`crate::Journal::record`]: strictly increasing, never reused, never zero.
    /// Zero means "no event" — see the crate docs on sequence numbers.
    pub seq: u64,
    /// The thread id that performed the action.
    pub actor: u64,
    pub action: Action,
    /// The kernel's object-kind tag for the object this action concerns: `0` Null, `1` Console,
    /// `2` Task, `3` Memory, `4` Endpoint, `5` Registry, `6` Journal. A raw tag rather than an
    /// enum of its own, so this crate does not need to know about every object kind the kernel
    /// defines just to record what happened to one.
    pub kind: u8,
    /// The rights bitmask this action granted, checked, or was refused.
    pub rights: u32,
    /// Whatever the action needs beyond `actor`, `kind` and `rights` — a handle, an endpoint id,
    /// an error code.
    pub detail: u64,
}

impl Event {
    /// Encodes this event into `out`'s first [`ENCODED_EVENT_BYTES`] bytes and returns that
    /// count.
    ///
    /// The layout is fixed and little-endian — a contract a reader on the other side of the
    /// syscall boundary depends on, not an implementation detail free to change:
    ///
    /// ```text
    /// offset 0  : u64 seq
    /// offset 8  : u64 actor
    /// offset 16 : u64 detail
    /// offset 24 : u32 rights
    /// offset 28 : u8  action   (Action::as_u8)
    /// offset 29 : u8  kind
    /// offset 30 : [u8; 2] zero padding
    /// ```
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, AuditError> {
        if out.len() < ENCODED_EVENT_BYTES {
            return Err(AuditError::BufferTooSmall);
        }
        out[0..8].copy_from_slice(&self.seq.to_le_bytes());
        out[8..16].copy_from_slice(&self.actor.to_le_bytes());
        out[16..24].copy_from_slice(&self.detail.to_le_bytes());
        out[24..28].copy_from_slice(&self.rights.to_le_bytes());
        out[28] = self.action.as_u8();
        out[29] = self.kind;
        out[30] = 0;
        out[31] = 0;
        Ok(ENCODED_EVENT_BYTES)
    }

    /// Decodes an event from `bytes`' first [`ENCODED_EVENT_BYTES`] bytes. See [`AuditError`] for
    /// why a short input and an unrecognised action byte are both `Malformed`.
    pub fn decode(bytes: &[u8]) -> Result<Event, AuditError> {
        if bytes.len() < ENCODED_EVENT_BYTES {
            return Err(AuditError::Malformed);
        }
        // Each subslice is a fixed, in-range width taken from a slice already checked above, so
        // `try_into` can never actually fail here — the `unwrap`s just discharge that proof.
        let seq = u64::from_le_bytes(bytes[0..8].try_into().unwrap());
        let actor = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
        let detail = u64::from_le_bytes(bytes[16..24].try_into().unwrap());
        let rights = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
        let action = Action::from_u8(bytes[28]).ok_or(AuditError::Malformed)?;
        let kind = bytes[29];

        Ok(Event {
            seq,
            actor,
            action,
            kind,
            rights,
            detail,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_then_decode_round_trips_every_field() {
        let original = Event {
            seq: 0x0102_0304_0506_0708,
            actor: 0x1112_1314_1516_1718,
            action: Action::Delegate,
            kind: 4,
            rights: 0xAABB_CCDD,
            detail: 0x2122_2324_2526_2728,
        };
        let mut buf = [0u8; ENCODED_EVENT_BYTES];

        let written = original.encode(&mut buf).unwrap();
        assert_eq!(written, ENCODED_EVENT_BYTES);

        assert_eq!(Event::decode(&buf).unwrap(), original);
    }

    #[test]
    fn decode_rejects_an_unrecognised_action_byte() {
        let event = Event {
            seq: 1,
            actor: 1,
            action: Action::Insert,
            kind: 1,
            rights: 0,
            detail: 0,
        };
        let mut buf = [0u8; ENCODED_EVENT_BYTES];
        event.encode(&mut buf).unwrap();
        buf[28] = 200; // no Action maps to this byte

        assert_eq!(Event::decode(&buf), Err(AuditError::Malformed));
    }

    #[test]
    fn encode_into_a_too_small_buffer_is_refused() {
        let event = Event {
            seq: 1,
            actor: 1,
            action: Action::Insert,
            kind: 1,
            rights: 0,
            detail: 0,
        };
        let mut buf = [0u8; ENCODED_EVENT_BYTES - 1];

        assert_eq!(event.encode(&mut buf), Err(AuditError::BufferTooSmall));
    }
}
