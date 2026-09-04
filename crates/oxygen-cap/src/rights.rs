//! `Rights`: what a capability lets its holder do to the object it names.
//!
//! Hand-rolled rather than pulled from the `bitflags` crate: the set is four bits wide and the
//! operations it needs are `contains`, `intersection` and a couple of small helpers, so a
//! proc-macro dependency for that is a worse trade than writing it out by hand.

/// A set of rights, stored as a bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rights(u32);

impl Rights {
    /// May read the object's data.
    pub const READ: Rights = Rights(1 << 0);
    /// May write the object's data.
    pub const WRITE: Rights = Rights(1 << 1);
    /// May delegate a subset of these rights onward as a new, derived capability.
    pub const GRANT: Rights = Rights(1 << 2);
    /// May withdraw what was delegated *from* this capability. Distinct from holding authority
    /// over the object itself — see `CapSpace::revoke`, which is what this right gates.
    pub const REVOKE: Rights = Rights(1 << 3);

    /// No rights at all.
    pub const NONE: Rights = Rights(0);
    /// Every right this crate defines.
    pub const ALL: Rights = Rights(Self::READ.0 | Self::WRITE.0 | Self::GRANT.0 | Self::REVOKE.0);

    /// Builds a set from raw bits, for decoding a value received across a syscall boundary.
    pub const fn from_bits(bits: u32) -> Self {
        Rights(bits)
    }

    /// The raw bitmask, for encoding a set across a syscall boundary.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// True if every bit set in `other` is also set in `self`.
    pub const fn contains(self, other: Rights) -> bool {
        self.0 & other.0 == other.0
    }

    /// The rights `self` and `other` agree on. This is the narrowing operator delegation is built
    /// from: a derived capability can only ever end up with a subset of what its source held.
    pub const fn intersection(self, other: Rights) -> Rights {
        Rights(self.0 & other.0)
    }

    /// The rights `self` and `other` together grant. Needed to build a multi-bit set (`GRANT`
    /// plus `REVOKE`, say) without spelling out bit positions at every call site.
    pub const fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}
