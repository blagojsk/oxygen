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
//!
//! Two more push that same idea past a single handle. [`SYS_DESCRIBE`] answers what a handle's
//! *methods* are — arguments, return, and the syscall number that invokes each one — so a caller
//! that has never seen this source can still drive it correctly. [`SYS_AUDIT`] answers what has
//! *happened*: every grant, delegation, revocation and refusal this kernel has recorded, in
//! order. Between them, a holder can discover what it may call and read back what already was
//! called, which is the whole of "one surface, two audiences" applied to a capability's own
//! behaviour rather than just its existence.

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
/// Read typed characters. `(handle, ptr, cap) -> bytes read`. Requires `READ`; blocks.
pub const SYS_READ: u64 = 10;
/// Name the service published at an index. `(registry, index, ptr, cap) -> len`.
pub const SYS_SERVICES: u64 = 11;
/// Timer ticks since boot. `() -> ticks`.
pub const SYS_UPTIME: u64 = 12;
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
/// Describe the `index`th method of whatever a handle names. `(handle, index, ptr, cap) -> bytes
/// written`. Needs no particular right — see [`describe`].
pub const SYS_DESCRIBE: u64 = 13;
/// Read one retained audit event by index, oldest first. `(journal, index, ptr, cap) -> bytes
/// written`. Requires `READ` on a [`Object::Journal`].
pub const SYS_AUDIT: u64 = 14;

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
        SYS_READ => read(Handle::from_raw(args[0]), args[1], args[2]),
        SYS_SERVICES => services(Handle::from_raw(args[0]), args[1], args[2], args[3]),
        SYS_UPTIME => crate::arch::target::gic::ticks(),
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
        SYS_DESCRIBE => describe(Handle::from_raw(args[0]), args[1], args[2], args[3]),
        SYS_AUDIT => audit(Handle::from_raw(args[0]), args[1], args[2], args[3]),
        _ => E_NOSYS,
    }
}

/// The kernel's stable per-kind tag for an object capability.
///
/// Shared between what [`identify`] reports and what the audit journal records for the same
/// object, so the two can never drift apart — a reader comparing an [`oxygen_audit::Event`]'s
/// `kind` against what `SYS_IDENTIFY` says about the object it names is exactly the kind of
/// cross-check the journal exists to support.
fn object_kind(object: Object) -> u8 {
    match object {
        Object::Null => 0,
        Object::Console => 1,
        Object::Task(_) => 2,
        Object::Memory { .. } => 3,
        Object::Endpoint(_) => 4,
        Object::Registry => 5,
        Object::Journal => 6,
    }
}

/// Records a syscall's refusal for lack of a right.
///
/// Every call site reaches this only after whatever `with_caps` closure produced `object` and
/// `rights` has already returned — see the lock-ordering rule in `crate::audit`'s module docs.
/// This is the path that makes the journal an audit trail rather than a log of successes: an
/// agent that never sees what it was refused cannot tell "not permitted" from "never asked".
fn journal_denied(object: Object, rights: Rights, syscall_number: u64) {
    crate::audit::record(
        crate::audit::Action::Denied,
        object_kind(object),
        rights.bits(),
        syscall_number,
    );
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
        journal_denied(object, rights, SYS_WRITE);
        return E_RIGHTS;
    }

    // SAFETY: the range was just checked to lie inside a page mapped for this user thread, and
    // user pages are never unmapped while the thread that owns them is running.
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
    crate::arch::target::uart::write_bytes(bytes);
    len
}

/// Waits for typed input and copies it into the caller's buffer.
///
/// Blocks until at least one byte exists, then takes everything available up to `capacity`.
/// Returning as soon as there is *something* rather than waiting for a full buffer is what makes a
/// shell feel like a shell: a reader that waits for `capacity` bytes would hold the first
/// character until the last one arrived.
fn read(handle: Handle, ptr: u64, capacity: u64) -> u64 {
    let (object, rights) = match crate::sched::with_caps(|caps| caps.get(handle)) {
        Ok(pair) => pair,
        Err(e) => return code_for(e),
    };
    if !matches!(object, Object::Console) {
        return E_WRONGTYPE;
    }
    if !rights.contains(Rights::READ) {
        journal_denied(object, rights, SYS_READ);
        return E_RIGHTS;
    }
    if capacity == 0 || !crate::arch::target::user::is_user_writable(ptr, capacity) {
        return E_FAULT;
    }

    use crate::arch::target::uart;

    let mut written = 0usize;
    loop {
        while written < capacity as usize {
            let Some(byte) = uart::read_byte() else { break };
            // SAFETY: the range was checked to be a writable user page, and `written` is bounded
            // by `capacity` on every iteration.
            unsafe { core::ptr::write_volatile((ptr as *mut u8).add(written), byte) };
            written += 1;
        }
        if written > 0 {
            return written as u64;
        }
        crate::sched::block_current_on(crate::sched::WAIT_CONSOLE);
    }
}

/// Describes a capability to its holder: what it is, and what it permits.
///
/// This is the smallest possible version of the discoverability the whole system is aimed at. A
/// process — or an agent — should never have to be told out of band what a handle it was given
/// actually is. The answer packs the object kind into the low byte and the rights above it.
fn identify(handle: Handle) -> u64 {
    match crate::sched::with_caps(|caps| caps.get(handle)) {
        Ok((object, rights)) => (u64::from(rights.bits()) << 8) | u64::from(object_kind(object)),
        Err(e) => code_for(e),
    }
}

/// Hands on a narrower version of a capability the caller holds.
fn delegate(handle: Handle, rights: Rights) -> u64 {
    match crate::sched::with_caps(|caps| caps.delegate(handle, rights)) {
        Ok(new) => {
            // What the child actually ended up with, not merely what was asked for: `delegate`
            // narrows to the intersection with what the parent held, so the grant this records
            // must be read back off the new handle rather than assumed from the request.
            if let Ok((object, granted)) = crate::sched::with_caps(|caps| caps.get(new)) {
                crate::audit::record(
                    crate::audit::Action::Delegate,
                    object_kind(object),
                    granted.bits(),
                    new.raw(),
                );
            }
            new.raw()
        }
        Err(e) => {
            let code = code_for(e);
            if code == E_RIGHTS
                && let Ok((object, held)) = crate::sched::with_caps(|caps| caps.get(handle))
            {
                journal_denied(object, held, SYS_DELEGATE);
            }
            code
        }
    }
}

/// Withdraws everything derived from a capability, leaving the caller's own intact.
fn revoke(handle: Handle) -> u64 {
    match crate::sched::with_caps(|caps| caps.revoke(handle)) {
        Ok(count) => {
            if let Ok((object, rights)) = crate::sched::with_caps(|caps| caps.get(handle)) {
                crate::audit::record(
                    crate::audit::Action::Revoke,
                    object_kind(object),
                    rights.bits(),
                    count as u64,
                );
            }
            count as u64
        }
        Err(e) => {
            let code = code_for(e);
            if code == E_RIGHTS
                && let Ok((object, held)) = crate::sched::with_caps(|caps| caps.get(handle))
            {
                journal_denied(object, held, SYS_REVOKE);
            }
            code
        }
    }
}

/// Retires the calling thread. Never returns, so the trap it arrived on never completes.
fn exit(code: u64) -> ! {
    crate::sched::retire_current(code)
}

/// Resolves a handle to the endpoint it names, checking the rights the caller needs for it.
///
/// `syscall_number` is only for the journal: it names which call this refusal belongs to, since
/// this one helper backs `SYS_SEND`, `SYS_RECV` and half of `SYS_REGISTER`.
fn endpoint_of(handle: Handle, needed: Rights, syscall_number: u64) -> Result<u64, u64> {
    let (object, rights) = crate::sched::with_caps(|caps| caps.get(handle)).map_err(code_for)?;
    let Object::Endpoint(id) = object else {
        return Err(E_WRONGTYPE);
    };
    if !rights.contains(needed) {
        journal_denied(object, rights, syscall_number);
        return Err(E_RIGHTS);
    }
    Ok(id)
}

/// Checks that a handle names the registry with the rights this call needs.
fn registry_of(handle: Handle, needed: Rights, syscall_number: u64) -> Result<(), u64> {
    let (object, rights) = crate::sched::with_caps(|caps| caps.get(handle)).map_err(code_for)?;
    if !matches!(object, Object::Registry) {
        return Err(E_WRONGTYPE);
    }
    if !rights.contains(needed) {
        journal_denied(object, rights, syscall_number);
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
        Ok(handle) => {
            crate::audit::record(
                crate::audit::Action::Insert,
                object_kind(Object::Endpoint(id)),
                Rights::ALL.bits(),
                id,
            );
            handle.raw()
        }
        Err(e) => code_for(e),
    }
}

/// Sends a typed message to the endpoint a capability names.
fn send(handle: Handle, interface: u64, method: u64, ptr: u64, len: u64) -> u64 {
    let id = match endpoint_of(handle, Rights::WRITE, SYS_SEND) {
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
    let id = match endpoint_of(handle, Rights::READ, SYS_RECV) {
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
    if let Err(code) = registry_of(registry, Rights::READ, SYS_LOOKUP) {
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
        Ok(handle) => {
            crate::audit::record(
                crate::audit::Action::Lookup,
                object_kind(Object::Endpoint(id)),
                rights.bits(),
                id,
            );
            handle.raw()
        }
        Err(e) => code_for(e),
    }
}

/// Names the service published at an index, so userspace can enumerate what exists.
///
/// Enumeration is a separate right from lookup in spirit but not yet in the rights bitmask; both
/// need `READ` on the registry. What matters is that neither is ambient: a task that was granted
/// no registry capability cannot discover anything, which is what stops discoverability from
/// quietly becoming a way around the capability system.
fn services(registry: Handle, index: u64, ptr: u64, capacity: u64) -> u64 {
    if let Err(code) = registry_of(registry, Rights::READ, SYS_SERVICES) {
        return code;
    }
    if capacity == 0 || capacity > MAX_NAME as u64 {
        return E_INVAL;
    }
    if !crate::arch::target::user::is_user_writable(ptr, capacity) {
        return E_FAULT;
    }

    let mut scratch = [0u8; MAX_NAME];
    let Some(len) = crate::ipc::service_at(index as usize, &mut scratch[..capacity as usize])
    else {
        return E_NOENT;
    };
    // SAFETY: the destination was checked to be a writable user page with room for `capacity`
    // bytes, and `len` is bounded by that.
    unsafe { core::ptr::copy_nonoverlapping(scratch.as_ptr(), ptr as *mut u8, len) };
    len as u64
}

/// Publishes an endpoint the caller holds under a name.
///
/// `GRANT` on the endpoint is required because publishing it is handing it to strangers. A task
/// that holds a send-only capability to somebody else's service must not be able to advertise it
/// as its own.
fn register(registry: Handle, ptr: u64, len: u64, endpoint: Handle) -> u64 {
    if let Err(code) = registry_of(registry, Rights::WRITE, SYS_REGISTER) {
        return code;
    }
    let id = match endpoint_of(endpoint, Rights::GRANT, SYS_REGISTER) {
        Ok(id) => id,
        Err(code) => return code,
    };
    let name = match name_from_user(ptr, len) {
        Ok(n) => n,
        Err(code) => return code,
    };
    match crate::ipc::register(name, id) {
        Ok(()) => {
            crate::audit::record(
                crate::audit::Action::Register,
                object_kind(Object::Endpoint(id)),
                0,
                id,
            );
            0
        }
        Err(f) => code_for_ipc(f),
    }
}

/// Describes the `index`th method of whatever `handle` currently names, encoded on the wire —
/// see `oxygen_schema::Method` for the exact layout.
///
/// Resolved with any rights at all, on purpose: describing what a capability can do must never
/// itself depend on already holding one of the rights it describes, or a capability narrowed
/// down to nothing would also be one whose own methods could no longer be listed — exactly
/// backwards from what discoverability is for.
fn describe(handle: Handle, index: u64, ptr: u64, capacity: u64) -> u64 {
    let object = match crate::sched::with_caps(|caps| caps.object(handle)) {
        Ok(object) => object,
        Err(e) => return code_for(e),
    };
    let Some(interface) = crate::schema::interface_for(object) else {
        return E_WRONGTYPE;
    };
    let Some(method) = crate::schema::method_at(interface, index as usize) else {
        return E_NOENT;
    };

    let mut scratch = [0u8; oxygen_schema::ENCODED_METHOD_BYTES];
    // `scratch` is exactly `ENCODED_METHOD_BYTES` wide, which is all `encode` ever requires, so
    // this cannot fail.
    let written = method
        .encode(&mut scratch)
        .expect("scratch is exactly ENCODED_METHOD_BYTES wide");

    if !crate::arch::target::user::is_user_writable(ptr, capacity) || capacity < written as u64 {
        return E_FAULT;
    }
    // SAFETY: the destination was just checked to be a writable user page with room for
    // `written` bytes.
    unsafe { core::ptr::copy_nonoverlapping(scratch.as_ptr(), ptr as *mut u8, written) };
    written as u64
}

/// Reads one retained audit event by index, oldest first, encoded on the wire — see
/// `oxygen_audit::Event` for the exact layout.
///
/// Requires `READ` on a [`Object::Journal`]: unlike `describe`, this reveals what the whole
/// system has done rather than what one handle can do, so it is gated exactly like any other
/// read of something that is not ambiently visible.
fn audit(handle: Handle, index: u64, ptr: u64, capacity: u64) -> u64 {
    let (object, rights) = match crate::sched::with_caps(|caps| caps.get(handle)) {
        Ok(pair) => pair,
        Err(e) => return code_for(e),
    };
    if !matches!(object, Object::Journal) {
        return E_WRONGTYPE;
    }
    if !rights.contains(Rights::READ) {
        journal_denied(object, rights, SYS_AUDIT);
        return E_RIGHTS;
    }

    let Some(event) = crate::audit::event_at(index as usize) else {
        return E_NOENT;
    };

    let mut scratch = [0u8; oxygen_audit::ENCODED_EVENT_BYTES];
    // `scratch` is exactly `ENCODED_EVENT_BYTES` wide, which is all `encode` ever requires, so
    // this cannot fail.
    let written = event
        .encode(&mut scratch)
        .expect("scratch is exactly ENCODED_EVENT_BYTES wide");

    if !crate::arch::target::user::is_user_writable(ptr, capacity) || capacity < written as u64 {
        return E_FAULT;
    }
    // SAFETY: the destination was just checked to be a writable user page with room for
    // `written` bytes.
    unsafe { core::ptr::copy_nonoverlapping(scratch.as_ptr(), ptr as *mut u8, written) };
    written as u64
}
