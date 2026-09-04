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
use oxygen_ipc::{MAX_NAME, MAX_PAYLOAD, Message, Name};

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
/// Create an IPC endpoint. `() -> handle`.
pub const SYS_ENDPOINT: u64 = 5;
/// Send a typed message. `(handle, interface, method, ptr, len) -> 0`. Requires `WRITE`.
pub const SYS_SEND: u64 = 6;
/// Receive one, blocking until it arrives. `(handle, ptr, cap) -> bytes written`. Requires `READ`.
pub const SYS_RECV: u64 = 7;
/// Find an endpoint by name. `(registry, name_ptr, name_len) -> handle`. Requires `READ`.
pub const SYS_LOOKUP: u64 = 8;
/// Publish an endpoint under a name. `(registry, name_ptr, name_len, endpoint) -> 0`.
/// Requires `WRITE` on the registry and `GRANT` on the endpoint.
pub const SYS_REGISTER: u64 = 9;

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
/// Nothing is published under that name, or the endpoint is gone.
pub const E_NOENT: u64 = -8i64 as u64;
/// A queue or the registry is full.
pub const E_FULL: u64 = -9i64 as u64;
/// The message or the name was malformed — an interface of zero, an unprintable name, a body
/// longer than a message can carry.
pub const E_INVAL: u64 = -10i64 as u64;

/// The longest single write accepted. A user program can ask for any length; without a bound, one
/// call would occupy the console for as long as it liked, which is a denial of service dressed up
/// as a print statement.
const MAX_WRITE: u64 = 4096;

/// Bytes of header the receiver gets before the body: interface, method and length, each `u32`.
const HEADER_BYTES: usize = 12;

fn code_for_ipc(fault: crate::ipc::IpcFault) -> u64 {
    use crate::ipc::IpcFault;
    match fault {
        IpcFault::NoSuchEndpoint | IpcFault::NotFound => E_NOENT,
        IpcFault::Full => E_FULL,
        IpcFault::Malformed(_) => E_INVAL,
    }
}

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
        SYS_ENDPOINT => endpoint(),
        SYS_SEND => send(
            Handle::from_raw(args[0]),
            args[1],
            args[2],
            args[3],
            args[4],
        ),
        SYS_RECV => recv(Handle::from_raw(args[0]), args[1], args[2]),
        SYS_LOOKUP => lookup(Handle::from_raw(args[0]), args[1], args[2]),
        SYS_REGISTER => register(
            Handle::from_raw(args[0]),
            args[1],
            args[2],
            Handle::from_raw(args[3]),
        ),
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
                Object::Endpoint(_) => 4,
                Object::Registry => 5,
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

/// Resolves a handle to the endpoint it names, checking the rights the caller needs for it.
fn endpoint_of(handle: Handle, needed: Rights) -> Result<u64, u64> {
    let (object, rights) = crate::sched::with_caps(|caps| caps.get(handle)).map_err(code_for)?;
    let Object::Endpoint(id) = object else {
        return Err(E_WRONGTYPE);
    };
    if !rights.contains(needed) {
        return Err(E_RIGHTS);
    }
    Ok(id)
}

/// Checks that a handle names the registry with the rights this call needs.
fn registry_of(handle: Handle, needed: Rights) -> Result<(), u64> {
    let (object, rights) = crate::sched::with_caps(|caps| caps.get(handle)).map_err(code_for)?;
    if !matches!(object, Object::Registry) {
        return Err(E_WRONGTYPE);
    }
    if !rights.contains(needed) {
        return Err(E_RIGHTS);
    }
    Ok(())
}

/// Copies a name out of user memory, rejecting anything that is not a valid one.
fn name_from_user(ptr: u64, len: u64) -> Result<Name, u64> {
    if len == 0 || len > MAX_NAME as u64 {
        return Err(E_INVAL);
    }
    if !crate::arch::target::user::is_user_readable(ptr, len) {
        return Err(E_FAULT);
    }
    // SAFETY: the range was just checked to lie inside a page mapped for this user thread.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let text = core::str::from_utf8(bytes).map_err(|_| E_INVAL)?;
    Name::new(text).map_err(|_| E_INVAL)
}

/// Creates an endpoint and hands the caller a capability to it, with every right.
///
/// Full rights because the caller made it. Narrowing it for somebody else is what `SYS_DELEGATE`
/// is for, and doing that narrowing here would be the kernel guessing at a policy.
fn endpoint() -> u64 {
    let id = crate::ipc::create_endpoint();
    match crate::sched::with_caps(|caps| caps.insert(Object::Endpoint(id), Rights::ALL)) {
        Ok(handle) => handle.raw(),
        Err(e) => code_for(e),
    }
}

/// Sends a typed message to the endpoint a capability names.
fn send(handle: Handle, interface: u64, method: u64, ptr: u64, len: u64) -> u64 {
    let id = match endpoint_of(handle, Rights::WRITE) {
        Ok(id) => id,
        Err(code) => return code,
    };
    if len > MAX_PAYLOAD as u64 {
        return E_INVAL;
    }
    if len > 0 && !crate::arch::target::user::is_user_readable(ptr, len) {
        return E_FAULT;
    }
    // SAFETY: checked above to lie inside a page mapped for this user thread; a zero length
    // dereferences nothing.
    let body = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };

    let message = match Message::new(interface as u32, method as u32, body) {
        Ok(m) => m,
        Err(_) => return E_INVAL,
    };
    match crate::ipc::send(id, message) {
        Ok(()) => 0,
        Err(f) => code_for_ipc(f),
    }
}

/// Waits for a message and writes it into the caller's buffer.
///
/// The header goes first, then the body, so a caller reads a fixed twelve bytes and then knows how
/// much follows. Returning only the body would drop the interface and method on the floor, and a
/// message whose type the receiver cannot see is exactly the untyped IPC this milestone exists to
/// avoid.
fn recv(handle: Handle, ptr: u64, capacity: u64) -> u64 {
    let id = match endpoint_of(handle, Rights::READ) {
        Ok(id) => id,
        Err(code) => return code,
    };
    if !crate::arch::target::user::is_user_writable(ptr, capacity) {
        return E_FAULT;
    }

    let message = match crate::ipc::recv(id) {
        Ok(m) => m,
        Err(f) => return code_for_ipc(f),
    };
    let body = message.body();
    let total = HEADER_BYTES + body.len();
    if total as u64 > capacity {
        return E_FAULT;
    }

    let header = [
        message.header.interface.to_le_bytes(),
        message.header.method.to_le_bytes(),
        message.header.len.to_le_bytes(),
    ];
    // SAFETY: the destination was checked to be a writable user page with room for `total` bytes.
    unsafe {
        let out = core::slice::from_raw_parts_mut(ptr as *mut u8, total);
        for (i, word) in header.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(word);
        }
        out[HEADER_BYTES..].copy_from_slice(body);
    }
    total as u64
}

/// Finds an endpoint by name and hands back a capability to it.
///
/// The capability carries `READ | WRITE` but not `GRANT`: a task that found a service by name may
/// talk to it, and may not pass that authority on to somebody who did not. Anyone who should have
/// it can look the name up themselves, which is the registry doing its job.
fn lookup(registry: Handle, ptr: u64, len: u64) -> u64 {
    if let Err(code) = registry_of(registry, Rights::READ) {
        return code;
    }
    let name = match name_from_user(ptr, len) {
        Ok(n) => n,
        Err(code) => return code,
    };
    let id = match crate::ipc::lookup(&name) {
        Ok(id) => id,
        Err(f) => return code_for_ipc(f),
    };
    let rights = Rights::READ.union(Rights::WRITE);
    match crate::sched::with_caps(|caps| caps.insert(Object::Endpoint(id), rights)) {
        Ok(handle) => handle.raw(),
        Err(e) => code_for(e),
    }
}

/// Publishes an endpoint the caller holds under a name.
///
/// `GRANT` on the endpoint is required because publishing it is handing it to strangers. A task
/// that holds a send-only capability to somebody else's service must not be able to advertise it
/// as its own.
fn register(registry: Handle, ptr: u64, len: u64, endpoint: Handle) -> u64 {
    if let Err(code) = registry_of(registry, Rights::WRITE) {
        return code;
    }
    let id = match endpoint_of(endpoint, Rights::GRANT) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let name = match name_from_user(ptr, len) {
        Ok(n) => n,
        Err(code) => return code,
    };
    match crate::ipc::register(name, id) {
        Ok(()) => 0,
        Err(f) => code_for_ipc(f),
    }
}
