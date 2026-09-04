//! `Handle`: the token userspace holds for a capability.
//!
//! Packed as one `u64` — `index` in the low 32 bits, `generation` in the high 32 bits — so it
//! crosses a syscall boundary as a single register, and a userspace program can only ever hold
//! the opaque number, never a pointer into kernel memory. The generation half is what makes it
//! unforgeable in practice: guessing a live index is easy (they are small, dense integers), but
//! guessing the exact generation currently occupying that index is not, and [`crate::CapSpace`]
//! rejects anything else in either half.

/// An opaque capability reference. Userspace never sees `index` or `generation` separately, only
/// the packed `u64` — the split is an implementation detail of `CapSpace`, not part of the
/// contract a caller depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(u64);

impl Handle {
    /// All-ones, and never valid. A real table can never grow to `u32::MAX` slots — that alone
    /// would be 64 GiB of `Slot`s — so this exact index can never collide with one a `CapSpace`
    /// actually issued, whatever generation ends up in the high half.
    pub const NULL: Handle = Handle(u64::MAX);

    /// Packs a slot index and the generation it was issued with into one handle.
    pub const fn new(index: u32, generation: u32) -> Self {
        Handle(((generation as u64) << 32) | index as u64)
    }

    /// The low half: which slot this handle names.
    pub const fn index(self) -> u32 {
        self.0 as u32
    }

    /// The high half: which occupant of that slot this handle was issued for.
    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    /// The packed representation, for carrying a handle across a syscall boundary.
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// Reconstructs a handle from a raw value received from userspace.
    ///
    /// Does not validate it — a raw `u64` from userspace is untrusted by definition, and the only
    /// place that can tell a live handle from a forged one is the table it names, so validation
    /// is [`crate::CapSpace::get`]'s job, not this constructor's.
    pub const fn from_raw(raw: u64) -> Self {
        Handle(raw)
    }
}
