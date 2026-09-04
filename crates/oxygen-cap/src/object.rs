//! What a capability slot refers to.

/// The kernel object a capability grants some level of access to.
///
/// `Null` marks an unoccupied slot rather than describing a real object. `CapSpace` uses it as
/// the free/occupied discriminant instead of a separate `bool` alongside it, so a slot's
/// occupancy and its contents can never disagree with each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Object {
    /// A free slot.
    Null,
    /// The kernel console/serial device.
    Console,
    /// Another task, named by its kernel-assigned id.
    Task(u64),
    /// A range of physical memory this capability grants access to.
    Memory { base: u64, len: u64 },
}
