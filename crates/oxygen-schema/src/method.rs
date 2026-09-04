//! A single callable operation on an interface — its id, name, argument shapes and return shape
//! — plus the fixed byte layout that carries all of that across a syscall boundary.
//!
//! ## Why a fixed wire layout
//!
//! A [`Method`] is built and read on one side of a syscall in ordinary Rust, but it is also
//! handed to a caller-supplied buffer that a *different* compilation of a *different* program
//! reads back — an agent's tool-calling loop, say, that never linked against this crate.
//! Reinterpreting the value's in-memory representation across that boundary would work today,
//! by coincidence, because both sides happen to be built by the same compiler for the same
//! target; nothing would keep it working once either side changes. [`Method::encode`] and
//! [`Method::decode`] fix the layout explicitly instead — a byte offset and width per field,
//! little-endian, documented on [`ENCODED_METHOD_BYTES`] — so what crosses the boundary is a
//! contract, not an accident of compiler version.

use crate::SchemaError;
use oxygen_ipc::Name;

/// Longest argument list a [`Method`] can declare.
pub const MAX_ARGS: usize = 4;

/// What kind of value occupies an argument or return slot.
///
/// `Copy`, and each variant maps to a stable `u8` via [`ArgKind::as_u8`] / [`ArgKind::from_u8`].
/// The mapping has to stay fixed release over release: it crosses the same syscall boundary a
/// [`Method`] does and is read by a program compiled separately from this crate, so changing
/// what `2` means would silently reinterpret every schema already out there instead of failing
/// to compile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// No argument occupies this slot, or a method returns nothing.
    None,
    /// A capability handle.
    Handle,
    /// A byte string of bounded length.
    Bytes,
    /// A plain integer.
    Number,
    /// A capability's rights bitmask.
    Rights,
}

impl ArgKind {
    /// The stable wire value for this kind. See the type's docs for why it must not change.
    pub const fn as_u8(self) -> u8 {
        match self {
            ArgKind::None => 0,
            ArgKind::Handle => 1,
            ArgKind::Bytes => 2,
            ArgKind::Number => 3,
            ArgKind::Rights => 4,
        }
    }

    /// The kind `byte` names, or `None` if it names none of them. A byte a future version wrote
    /// that this one does not yet know, and simple corruption, are the same case from here: this
    /// crate cannot tell them apart, so both are refused rather than guessed at.
    pub const fn from_u8(byte: u8) -> Option<ArgKind> {
        match byte {
            0 => Some(ArgKind::None),
            1 => Some(ArgKind::Handle),
            2 => Some(ArgKind::Bytes),
            3 => Some(ArgKind::Number),
            4 => Some(ArgKind::Rights),
            _ => None,
        }
    }
}

/// Byte width of an encoded [`Method`] — [`Method::encode`]'s output length and the minimum
/// length [`Method::decode`] accepts.
///
/// The wire layout, every multi-byte field little-endian:
///
/// | Offset | Bytes | Field |
/// | --- | --- | --- |
/// | 0 | 4 | method id (`u32`) |
/// | 4 | 1 | argument count |
/// | 5 | 1 | return kind ([`ArgKind::as_u8`]) |
/// | 6 | [`MAX_ARGS`] | argument kinds, padded with `ArgKind::None` |
/// | 6 + `MAX_ARGS` | `oxygen_ipc::MAX_NAME` | name, zero-padded |
///
/// Fixed-width and little-endian because the two sides of this boundary are the same machine
/// today and are not guaranteed to stay that way — a layout nothing states explicitly is one a
/// caller can only get right by guessing, and a guess that happens to work is exactly the bug
/// that waits for the day it stops working.
pub const ENCODED_METHOD_BYTES: usize = 4 + 1 + 1 + MAX_ARGS + oxygen_ipc::MAX_NAME;

/// One callable operation: an id stable across the wire, a human-readable name, up to
/// [`MAX_ARGS`] argument kinds, and one return kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Method {
    id: u32,
    name: Name,
    args: [ArgKind; MAX_ARGS],
    arg_count: u8,
    returns: ArgKind,
}

impl Method {
    /// Builds a method. Fails `TooManyArgs` if `args` is longer than [`MAX_ARGS`]; the unused
    /// tail of the fixed argument array is padded with `ArgKind::None`, never left to whatever
    /// bits happened to be there.
    pub fn new(
        id: u32,
        name: Name,
        args: &[ArgKind],
        returns: ArgKind,
    ) -> Result<Method, SchemaError> {
        if args.len() > MAX_ARGS {
            return Err(SchemaError::TooManyArgs);
        }
        let mut slots = [ArgKind::None; MAX_ARGS];
        slots[..args.len()].copy_from_slice(args);
        Ok(Method {
            id,
            name,
            args: slots,
            arg_count: args.len() as u8,
            returns,
        })
    }

    pub const fn id(&self) -> u32 {
        self.id
    }

    pub const fn name(&self) -> Name {
        self.name
    }

    /// The declared arguments, in order — never the zero-padded tail past however many were
    /// actually given to [`Method::new`].
    pub fn args(&self) -> &[ArgKind] {
        &self.args[..self.arg_count as usize]
    }

    pub const fn returns(&self) -> ArgKind {
        self.returns
    }

    /// Fills an unused slot in [`crate::interface::Interface`]'s fixed method array before
    /// anything has been added to it.
    ///
    /// Never observable from outside this crate: `Interface::methods` always slices to its live
    /// count, which starts at zero, so this value is overwritten before it could be read. It
    /// exists only because the array itself needs *some* valid `Method` to be built from —
    /// `Option`-wrapping every slot instead (as [`oxygen_ipc::Registry`] does for names) would
    /// avoid the need, at the cost of `Interface::methods()` no longer being a plain slice.
    pub(crate) fn padding() -> Method {
        Method {
            id: 0,
            // Any single printable-ASCII byte is a valid `Name`; "_" carries no meaning of its
            // own and was picked only because it cannot be mistaken for a real method name.
            name: Name::new("_").expect("a single printable ASCII byte is always a valid Name"),
            args: [ArgKind::None; MAX_ARGS],
            arg_count: 0,
            returns: ArgKind::None,
        }
    }

    /// Writes this method's wire encoding into `out`. See [`ENCODED_METHOD_BYTES`] for the exact
    /// layout.
    ///
    /// Fails `BufferTooSmall` without writing anything if `out` is shorter than
    /// [`ENCODED_METHOD_BYTES`] — checked up front, so a caller can never observe a partial
    /// write. Returns the number of bytes written on success.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, SchemaError> {
        if out.len() < ENCODED_METHOD_BYTES {
            return Err(SchemaError::BufferTooSmall);
        }

        out[0..4].copy_from_slice(&self.id.to_le_bytes());
        out[4] = self.arg_count;
        out[5] = self.returns.as_u8();
        for (slot, kind) in out[6..6 + MAX_ARGS].iter_mut().zip(self.args.iter()) {
            *slot = kind.as_u8();
        }

        // The name field is fixed-width but the name itself is shorter than that almost always,
        // so the tail must be zeroed explicitly — `out` is caller-supplied and may hold anything.
        let name_off = 6 + MAX_ARGS;
        let name_field = &mut out[name_off..ENCODED_METHOD_BYTES];
        name_field.fill(0);
        let name_bytes = self.name.as_str().as_bytes();
        name_field[..name_bytes.len()].copy_from_slice(name_bytes);

        Ok(ENCODED_METHOD_BYTES)
    }

    /// The inverse of [`Method::encode`].
    ///
    /// `Malformed` covers every way `bytes` can fail to be one of this crate's own encodings: a
    /// buffer shorter than [`ENCODED_METHOD_BYTES`], an argument count past [`MAX_ARGS`], an
    /// argument or return byte [`ArgKind::from_u8`] does not recognise, or a name field that is
    /// not a valid [`Name`] once its zero padding is trimmed. None of these get a default
    /// substituted in their place — a schema silently reinterpreted as something else is worse
    /// than one that is visibly rejected.
    pub fn decode(bytes: &[u8]) -> Result<Method, SchemaError> {
        if bytes.len() < ENCODED_METHOD_BYTES {
            return Err(SchemaError::Malformed);
        }

        let id = u32::from_le_bytes(bytes[0..4].try_into().expect("4-byte slice"));
        let arg_count = bytes[4];
        if arg_count as usize > MAX_ARGS {
            return Err(SchemaError::Malformed);
        }
        let returns = ArgKind::from_u8(bytes[5]).ok_or(SchemaError::Malformed)?;

        let mut args = [ArgKind::None; MAX_ARGS];
        for (slot, &byte) in args.iter_mut().zip(&bytes[6..6 + MAX_ARGS]) {
            *slot = ArgKind::from_u8(byte).ok_or(SchemaError::Malformed)?;
        }

        let name_off = 6 + MAX_ARGS;
        let name_field = &bytes[name_off..ENCODED_METHOD_BYTES];
        let name_len = name_field
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_field.len());
        let name_str =
            core::str::from_utf8(&name_field[..name_len]).map_err(|_| SchemaError::Malformed)?;
        let name = Name::new(name_str).map_err(|_| SchemaError::Malformed)?;

        Ok(Method {
            id,
            name,
            args,
            arg_count,
            returns,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> Name {
        Name::new(s).unwrap()
    }

    #[test]
    fn args_reports_only_the_live_slots() {
        let method = Method::new(
            1,
            name("read"),
            &[ArgKind::Handle, ArgKind::Number],
            ArgKind::Bytes,
        )
        .unwrap();
        assert_eq!(method.args(), &[ArgKind::Handle, ArgKind::Number]);
    }

    #[test]
    fn max_args_is_accepted_one_more_is_refused() {
        let full = [ArgKind::Handle; MAX_ARGS];
        assert!(Method::new(1, name("m"), &full, ArgKind::None).is_ok());

        let mut over = [ArgKind::Handle; MAX_ARGS + 1];
        over[MAX_ARGS] = ArgKind::Number;
        assert_eq!(
            Method::new(1, name("m"), &over, ArgKind::None),
            Err(SchemaError::TooManyArgs)
        );
    }

    #[test]
    fn encode_decode_round_trips_with_no_args_and_with_max_args() {
        let empty = Method::new(1, name("ping"), &[], ArgKind::None).unwrap();
        let mut buf = [0u8; ENCODED_METHOD_BYTES];
        let written = empty.encode(&mut buf).unwrap();
        assert_eq!(written, ENCODED_METHOD_BYTES);
        assert_eq!(Method::decode(&buf).unwrap(), empty);

        let full_args = [
            ArgKind::Handle,
            ArgKind::Bytes,
            ArgKind::Number,
            ArgKind::Rights,
        ];
        let full = Method::new(7, name("call"), &full_args, ArgKind::Rights).unwrap();
        let mut buf = [0u8; ENCODED_METHOD_BYTES];
        full.encode(&mut buf).unwrap();
        assert_eq!(Method::decode(&buf).unwrap(), full);
    }

    #[test]
    fn encode_into_a_too_small_buffer_is_refused_and_writes_nothing() {
        let method = Method::new(1, name("m"), &[], ArgKind::None).unwrap();
        let mut buf = [0xAAu8; ENCODED_METHOD_BYTES - 1];

        assert_eq!(method.encode(&mut buf), Err(SchemaError::BufferTooSmall));
        assert!(
            buf.iter().all(|&b| b == 0xAA),
            "a failed encode must leave the buffer untouched"
        );
    }

    #[test]
    fn decode_rejects_an_unrecognised_kind_byte() {
        let method = Method::new(1, name("m"), &[ArgKind::Handle], ArgKind::Number).unwrap();
        let mut buf = [0u8; ENCODED_METHOD_BYTES];
        method.encode(&mut buf).unwrap();

        // Byte 5 is the return kind; 0xFF names no `ArgKind`.
        buf[5] = 0xFF;
        assert_eq!(Method::decode(&buf), Err(SchemaError::Malformed));
    }

    #[test]
    fn decode_rejects_a_short_buffer() {
        let buf = [0u8; ENCODED_METHOD_BYTES - 1];
        assert_eq!(Method::decode(&buf), Err(SchemaError::Malformed));
    }

    #[test]
    fn arg_kind_round_trips_and_rejects_unknown_bytes() {
        let kinds = [
            ArgKind::None,
            ArgKind::Handle,
            ArgKind::Bytes,
            ArgKind::Number,
            ArgKind::Rights,
        ];
        for kind in kinds {
            assert_eq!(ArgKind::from_u8(kind.as_u8()), Some(kind));
        }
        assert_eq!(ArgKind::from_u8(5), None);
        assert_eq!(ArgKind::from_u8(255), None);
    }
}
