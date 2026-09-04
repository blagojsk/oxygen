//! Endpoints, and the registry that lets a program find one by name.
//!
//! Two tasks that cannot already name the same thing cannot talk to each other, and until there is
//! a way to pass a capability across a task boundary — which there is not yet — the only thing
//! they can both name is a name. That is what the registry is for: a task publishes an endpoint
//! under a printable name, another task asks for that name and gets a capability back. Discovery
//! is itself an authority, gated on holding a registry capability, so "what exists here?" is a
//! question somebody was granted the right to ask.
//!
//! Blocking lives here rather than in the scheduler because the condition is an IPC condition. A
//! receiver with an empty queue parks; a sender wakes everyone parked on that endpoint and lets
//! them race for it. Picking a winner inside the send path would be a scheduling policy smuggled
//! into a message queue.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use oxygen_ipc::{IpcError, Message, MessageQueue, Name, Registry};

use crate::println;
use crate::sync::SpinLock;

/// Messages an endpoint holds before a send is refused.
///
/// Shallow on purpose. A deep queue turns a receiver that has stopped keeping up into memory the
/// kernel is holding on its behalf, and the sender learns about it far too late to do anything
/// sensible; a shallow one tells the sender immediately.
const QUEUE_DEPTH: usize = 8;

/// How many names the registry can hold at once.
const REGISTRY_SLOTS: usize = 16;

struct Endpoint {
    id: u64,
    queue: MessageQueue<QUEUE_DEPTH>,
}

static ENDPOINTS: SpinLock<Option<Vec<Endpoint>>> = SpinLock::new(None);
static REGISTRY: SpinLock<Option<Registry<REGISTRY_SLOTS>>> = SpinLock::new(None);
/// Endpoint ids start at 1, so that zero is never a valid endpoint and a zeroed field cannot
/// accidentally name one.
static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(1);

/// Errors that are about endpoints rather than about message or name encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcFault {
    /// No endpoint with that id — it never existed, or the capability outlived it.
    NoSuchEndpoint,
    /// The endpoint's queue is full and the sender should try again or give up.
    Full,
    /// The registry holds no entry under that name.
    NotFound,
    /// The message or the name was malformed. Carries what was wrong with it.
    Malformed(IpcError),
}

impl From<IpcError> for IpcFault {
    fn from(e: IpcError) -> Self {
        match e {
            IpcError::QueueFull | IpcError::RegistryFull => IpcFault::Full,
            other => IpcFault::Malformed(other),
        }
    }
}

/// Creates the endpoint table and the registry. Needs the heap, so it runs after `mm::init`.
pub fn init() {
    *ENDPOINTS.lock() = Some(Vec::new());
    *REGISTRY.lock() = Some(Registry::new());
}

/// Creates an endpoint and returns its id.
pub fn create_endpoint() -> u64 {
    let id = NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed);
    let mut guard = ENDPOINTS.lock();
    if let Some(list) = guard.as_mut() {
        list.push(Endpoint {
            id,
            queue: MessageQueue::new(),
        });
    }
    id
}

/// Queues a message and wakes whoever was waiting for it.
///
/// The lock is released before the wake on purpose: waking takes the scheduler's lock, and a path
/// that holds two locks at once is a deadlock waiting for the other order to appear somewhere
/// else.
pub fn send(endpoint: u64, message: Message) -> Result<(), IpcFault> {
    {
        let mut guard = ENDPOINTS.lock();
        let list = guard.as_mut().ok_or(IpcFault::NoSuchEndpoint)?;
        let target = list
            .iter_mut()
            .find(|e| e.id == endpoint)
            .ok_or(IpcFault::NoSuchEndpoint)?;
        target.queue.push(message)?;
    }
    crate::sched::wake_all_on(endpoint);
    Ok(())
}

/// Takes a message if one is waiting, without blocking.
pub fn try_recv(endpoint: u64) -> Result<Option<Message>, IpcFault> {
    let mut guard = ENDPOINTS.lock();
    let list = guard.as_mut().ok_or(IpcFault::NoSuchEndpoint)?;
    let target = list
        .iter_mut()
        .find(|e| e.id == endpoint)
        .ok_or(IpcFault::NoSuchEndpoint)?;
    Ok(target.queue.pop())
}

/// Waits until a message arrives on the endpoint.
///
/// The re-check after waking is not optional: every thread blocked on this endpoint is woken by a
/// send, and only one of them can have the message. The others find the queue empty and park
/// again, which is exactly right and needs no coordination between them.
pub fn recv(endpoint: u64) -> Result<Message, IpcFault> {
    loop {
        if let Some(message) = try_recv(endpoint)? {
            return Ok(message);
        }
        crate::sched::block_current_on(endpoint);
    }
}

/// Publishes a value under a name.
pub fn register(name: Name, value: u64) -> Result<(), IpcFault> {
    let mut guard = REGISTRY.lock();
    let registry = guard.as_mut().ok_or(IpcFault::NotFound)?;
    registry.register(name, value)?;
    Ok(())
}

/// Looks a name up.
pub fn lookup(name: &Name) -> Result<u64, IpcFault> {
    let guard = REGISTRY.lock();
    let registry = guard.as_ref().ok_or(IpcFault::NotFound)?;
    registry.lookup(name).ok_or(IpcFault::NotFound)
}

/// Prints what is currently published.
///
/// The registry being enumerable is half the point of having one. A system whose services can only
/// be found by already knowing their names is discoverable in name only.
pub fn dump() {
    let guard = REGISTRY.lock();
    let Some(registry) = guard.as_ref() else {
        return;
    };
    for (name, value) in registry.iter() {
        println!("  [ipc]  {name} -> endpoint {value}");
    }
}
