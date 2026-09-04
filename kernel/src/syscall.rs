//! The system call surface — everything EL0 is able to ask for.
//!
//! Five calls, and four of them are about capabilities. That ratio is the design, not an accident
//! of what has been built so far: a syscall here is never "do this thing", it is "do this thing
//! with this authority", and the authority is the argument the kernel actually checks. A user
//! thread holds no ambient permission to reach the console, or anything else; it holds handles.
//!
//! Two of the calls exist purely so the surface can be examined at runtime rather than read out of
//! a header — [`SYS_IDENTIFY`] answers what a handle is and what it permits, and [`SYS_REVOKE`]
//! withdraws what was handed on. An agent that can ask what it holds, and a human who can take it
//! back, are the same mechanism seen from two sides.

use oxygen_cap::{CapError, Handle, Object};

pub use oxygen_cap::Rights;

/// Write bytes to whatever the capability names. `(handle, ptr, len) -> bytes written`.
pub const SYS_WRITE: u64 = 0;
/// Ask what a capability is and what it permits. `(handle) -> packed description`.
pub const SYS_IDENTIFY: u64 = 1;
/// Derive a narrower capability from one already held. `(handle, rights) -> handle`.
pub const SYS_DELEGATE: u64 = 2;
/// Withdraw everything derived from a capability. `(handle) -> how many were withdrawn`.
pub const SYS_REVOKE: u64 = 3;
/// Stop the calling thread. `(code)`, and does not return.
pub const SYS_EXIT: u64 = 4;

// Errors come back as small negative numbers in the return register, the way every kernel with a
// single return value has done it. They are distinct values rather than one failure code because
// the difference between "you never had this" and "you had it and it was withdrawn" is exactly
// what a caller needs in order to react sensibly.
/// No such call.
pub const E_NOSYS: u64 = -1i64 as u64;
/// The handle names no slot at all.
pub const E_BADHANDLE: u64 = -2i64 as u64;
/// The slot exists but this handle is a stale reference to a capability that is gone.
pub const E_STALE: u64 = -3i64 as u64;
/// The capability is real but does not permit this.
pub const E_RIGHTS: u64 = -4i64 as u64;
/// A pointer argument does not name memory the caller may hand to the kernel.
pub const E_FAULT: u64 = -5i64 as u64;
/// The capability space is full.
pub const E_NOSLOT: u64 = -6i64 as u64;
/// The capability is valid but names the wrong kind of object for this call.
pub const E_WRONGTYPE: u64 = -7i64 as u64;

/// The longest single write accepted. A user program can ask for any length; without a bound, one
/// call would occupy the console for as long as it liked, which is a denial of service dressed up
/// as a print statement.
const MAX_WRITE: u64 = 4096;

fn code_for(error: CapError) -> u64 {
    match error {
        CapError::OutOfSlots => E_NOSLOT,
        CapError::BadHandle => E_BADHANDLE,
        CapError::StaleHandle => E_STALE,
        CapError::InsufficientRights => E_RIGHTS,
    }
}

/// Routes a trap from EL0 to the call it named.
///
/// The number is in `x8` and the arguments in `x0`–`x5`, which is the AArch64 Linux convention.
/// Following it costs nothing and means anyone who has written a syscall on this architecture
/// already knows where to look.
pub fn dispatch(number: u64, args: [u64; 6]) -> u64 {
    match number {
        SYS_WRITE => write(Handle::from_raw(args[0]), args[1], args[2]),
        SYS_IDENTIFY => identify(Handle::from_raw(args[0])),
        SYS_DELEGATE => delegate(Handle::from_raw(args[0]), Rights::from_bits(args[1] as u32)),
        SYS_REVOKE => revoke(Handle::from_raw(args[0])),
        SYS_EXIT => exit(args[0]),
        _ => E_NOSYS,
    }
}

/// Writes user bytes to the console named by a capability.
///
/// Two checks and neither is optional. The pointer must name memory the caller owns, or a handle
/// with perfectly good rights becomes a way to print kernel memory. The capability must actually
/// be a console and must carry `WRITE`, or rights are decoration.
fn write(handle: Handle, ptr: u64, len: u64) -> u64 {
    if len > MAX_WRITE {
        return E_FAULT;
    }
    if !super::arch::target::user::is_user_readable(ptr, len) {
        return E_FAULT;
    }

    let (object, rights) = match crate::sched::with_caps(|caps| caps.get(handle)) {
        Ok(pair) => pair,
        Err(e) => return code_for(e),
    };
    if !matches!(object, Object::Console) {
        return E_WRONGTYPE;
    }
    if !rights.contains(Rights::WRITE) {
        return E_RIGHTS;
    }

    // SAFETY: the range was just checked to lie inside a page mapped for this user thread, and
    // user pages are never unmapped while the thread that owns them is running.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::arch::target::uart::write_bytes(bytes);
    len
}

/// Describes a capability to its holder: what it is, and what it permits.
///
/// This is the smallest possible version of the discoverability the whole system is aimed at. A
/// process — or an agent — should never have to be told out of band what a handle it was given
/// actually is. The answer packs the object kind into the low byte and the rights above it.
fn identify(handle: Handle) -> u64 {
    match crate::sched::with_caps(|caps| caps.get(handle)) {
        Ok((object, rights)) => {
            let kind = match object {
                Object::Null => 0u64,
                Object::Console => 1,
                Object::Task(_) => 2,
                Object::Memory { .. } => 3,
            };
            (u64::from(rights.bits()) << 8) | kind
        }
        Err(e) => code_for(e),
    }
}

/// Hands on a narrower version of a capability the caller holds.
fn delegate(handle: Handle, rights: Rights) -> u64 {
    match crate::sched::with_caps(|caps| caps.delegate(handle, rights)) {
        Ok(new) => new.raw(),
        Err(e) => code_for(e),
    }
}

/// Withdraws everything derived from a capability, leaving the caller's own intact.
fn revoke(handle: Handle) -> u64 {
    match crate::sched::with_caps(|caps| caps.revoke(handle)) {
        Ok(count) => count as u64,
        Err(e) => code_for(e),
    }
}

/// Retires the calling thread. Never returns, so the trap it arrived on never completes.
fn exit(code: u64) -> ! {
    crate::sched::retire_current(code)
}
