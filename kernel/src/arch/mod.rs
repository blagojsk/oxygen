//! Architecture abstraction.
//!
//! Everything that knows about a specific CPU or interrupt controller lives under here, so the
//! rest of the kernel can be written once. AArch64 is the primary target — the cheap ARM boards
//! this OS is meant to revive — and x86_64 is expected to arrive as a sibling module rather than
//! as a rewrite.

#[cfg(target_arch = "aarch64")]
pub mod aarch64;

#[cfg(target_arch = "aarch64")]
pub use aarch64 as target;
