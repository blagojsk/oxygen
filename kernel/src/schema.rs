//! The schema table: what each object kind's capability lets its holder call.
//!
//! This is the kernel-side half of `oxygen_schema`: that crate is the pure data — an
//! [`oxygen_schema::Method`] and its fixed wire encoding — with no notion of what a syscall or an
//! object kind is; this module is where the kernel's actual objects get described. The same
//! [`oxygen_schema::Method`] value this module hands `SYS_DESCRIBE` renders as a help line for a
//! person and as a callable tool definition for an agent — see `oxygen_schema`'s crate docs for
//! why that is the point of building it as data rather than as prose.
//!
//! ## Method ids are syscall numbers
//!
//! A method's id is not an index this module assigns — it is the exact number a caller already
//! has to put in `x8` to invoke it. Giving it a second, schema-local numbering would mean an
//! agent that just read a description still has to look the id up in a different table before it
//! could make the call; using the syscall number directly means the description *is* the call to
//! make.
//!
//! ## Every interface gets the four generic methods
//!
//! `identify`, `delegate`, `revoke` and `describe` apply to any capability, whatever it names.
//! [`add_generic_methods`] appends them to every interface built here so that a holder can
//! discover them from the handle alone, rather than needing to already know that these universal,
//! kind-independent operations exist somewhere off to the side.
//!
//! ## A different namespace from message interfaces
//!
//! `oxygen_ipc::Message` carries its own `interface` field — the M5 echo test sends interface
//! `7` — and that number is chosen freely by whatever service is listening, naming what a
//! *message sent to an endpoint* means. The ids registered here (1–4) name something else
//! entirely: what an *object kind's capability* supports. `2` here is "the endpoint object
//! supports send/recv", not "messages tagged 2" — the two numbering schemes share no relationship
//! and a collision between them (interface `7` meaning one thing over IPC and another thing here)
//! would be coincidence, not conflict. Letting a service describe what its own message interface
//! speaks, the way this module lets an object kind describe its methods, is recorded as FUTURE
//! work in SPECS.md; nothing here registers or reads a message interface id.

use oxygen_cap::Object;
use oxygen_ipc::Name;
use oxygen_schema::{ArgKind, Interface, Method, SchemaTable};

use crate::sync::SpinLock;
use crate::syscall;

/// Interface slots in the table. Four are used today (console, endpoint, registry, journal);
/// room for a few more before this needs revisiting.
const INTERFACES: usize = 8;

static TABLE: SpinLock<Option<SchemaTable<INTERFACES>>> = SpinLock::new(None);

/// Every name here is a short literal known to be valid printable ASCII, so unwrapping is just
/// discharging a proof rather than handling a real failure mode.
fn name(text: &str) -> Name {
    Name::new(text).expect("schema names in this module are short, printable-ASCII literals")
}

/// A method whose id is a syscall number. Every argument list declared in this module is well
/// under `MAX_ARGS`, so building one can only fail if this module itself has a bug.
fn method(id: u64, text: &str, args: &[ArgKind], returns: ArgKind) -> Method {
    Method::new(id as u32, name(text), args, returns)
        .expect("method argument lists declared here never exceed MAX_ARGS")
}

/// Appends `identify`, `delegate`, `revoke` and `describe` — see the module docs on why every
/// interface gets these four regardless of what is kind-specific.
fn add_generic_methods(iface: &mut Interface) {
    iface
        .add(method(
            syscall::SYS_IDENTIFY,
            "identify",
            &[],
            ArgKind::Number,
        ))
        .expect("every interface here has room for the four generic methods");
    iface
        .add(method(
            syscall::SYS_DELEGATE,
            "delegate",
            &[ArgKind::Rights],
            ArgKind::Handle,
        ))
        .expect("every interface here has room for the four generic methods");
    iface
        .add(method(syscall::SYS_REVOKE, "revoke", &[], ArgKind::Number))
        .expect("every interface here has room for the four generic methods");
    iface
        .add(method(
            syscall::SYS_DESCRIBE,
            "describe",
            &[ArgKind::Number, ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("every interface here has room for the four generic methods");
}

/// Builds the schema table.
///
/// Needs nothing but the table itself, so it could run anywhere in boot — it is placed after
/// `ipc::init` for clarity, since what it describes (the endpoint and registry object kinds) is
/// the same thing IPC's own init step brings up, even though this module never touches IPC state.
pub fn init() {
    let mut table = SchemaTable::new();

    let mut console = Interface::new(1, name("console")).expect("id 1 is not the reserved 0");
    console
        .add(method(
            syscall::SYS_WRITE,
            "write",
            &[ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("console has room for six methods");
    console
        .add(method(
            syscall::SYS_READ,
            "read",
            &[ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("console has room for six methods");
    add_generic_methods(&mut console);
    table
        .register(console)
        .expect("interface id 1 is registered exactly once, here");

    let mut endpoint = Interface::new(2, name("endpoint")).expect("id 2 is not the reserved 0");
    endpoint
        .add(method(
            syscall::SYS_SEND,
            "send",
            &[ArgKind::Number, ArgKind::Number, ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("endpoint has room for six methods");
    endpoint
        .add(method(
            syscall::SYS_RECV,
            "recv",
            &[ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("endpoint has room for six methods");
    add_generic_methods(&mut endpoint);
    table
        .register(endpoint)
        .expect("interface id 2 is registered exactly once, here");

    let mut registry = Interface::new(3, name("registry")).expect("id 3 is not the reserved 0");
    registry
        .add(method(
            syscall::SYS_LOOKUP,
            "lookup",
            &[ArgKind::Bytes],
            ArgKind::Handle,
        ))
        .expect("registry has room for seven methods");
    registry
        .add(method(
            syscall::SYS_REGISTER,
            "register",
            &[ArgKind::Bytes, ArgKind::Handle],
            ArgKind::Number,
        ))
        .expect("registry has room for seven methods");
    registry
        .add(method(
            syscall::SYS_SERVICES,
            "services",
            &[ArgKind::Number, ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("registry has room for seven methods");
    add_generic_methods(&mut registry);
    table
        .register(registry)
        .expect("interface id 3 is registered exactly once, here");

    let mut journal = Interface::new(4, name("journal")).expect("id 4 is not the reserved 0");
    journal
        .add(method(
            syscall::SYS_AUDIT,
            "audit",
            &[ArgKind::Number, ArgKind::Bytes],
            ArgKind::Number,
        ))
        .expect("journal has room for five methods");
    add_generic_methods(&mut journal);
    table
        .register(journal)
        .expect("interface id 4 is registered exactly once, here");

    *TABLE.lock() = Some(table);
}

/// Which schema interface describes `object`'s capability, if any.
///
/// A separate mapping from the kind byte `SYS_IDENTIFY` reports: see the module docs on why an
/// object's kind tag and its schema interface id are different, unrelated numberings that happen
/// to agree for `Console` only because both started counting at the same object.
pub fn interface_for(object: Object) -> Option<u32> {
    match object {
        Object::Console => Some(1),
        Object::Endpoint(_) => Some(2),
        Object::Registry => Some(3),
        Object::Journal => Some(4),
        Object::Null | Object::Task(_) | Object::Memory { .. } => None,
    }
}

/// The method at `index` on `interface`, in declaration order.
///
/// `None` both when `interface` was never registered and when it was but `index` runs past its
/// last method — the two look identical to `SYS_DESCRIBE`'s caller, and both are exactly the
/// "nothing more here" that `E_NOENT` means.
pub fn method_at(interface: u32, index: usize) -> Option<Method> {
    let guard = TABLE.lock();
    let table = guard.as_ref()?;
    table.get(interface)?.method(index).copied()
}
