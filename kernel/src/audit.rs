//! The audit journal: every capability grant, delegation, revocation and refusal, in order, with
//! who did it.
//!
//! This is the kernel-side half of `oxygen_audit`: that crate is the ring buffer and the wire
//! encoding, pure logic with no notion of a thread or a lock; this module is the one static
//! instance of it and the two things only the kernel can supply — who the calling thread is, and
//! mutual exclusion against every core... well, the one core there currently is, and every
//! interrupt on it.
//!
//! ## Sizing
//!
//! 64 events at [`oxygen_audit::ENCODED_EVENT_BYTES`] (32) bytes each is 2 KiB of permanently
//! resident memory — against the 2 MiB kernel heap SPECS.md's memory-management section already
//! budgets, that is a tenth of a percent, cheap enough to keep around always rather than something
//! worth trimming further. What it buys is a record deep enough to survive a shell session's
//! worth of `describe` and `audit` calls without wrapping mid-demonstration, while staying far
//! short of "unbounded", which this machine cannot afford for the same reason it cannot afford an
//! unbounded capability table. [`dropped`] is what keeps that bound honest: once the ring wraps, a
//! reader can still tell it has, rather than being quietly handed a trail that looks complete and
//! is not.
//!
//! ## Lock ordering
//!
//! [`record`] reads the actor's thread id from `sched::current_id` *before* it takes the
//! journal's own lock, and never the other way around. `sched::with_caps` and `sched::current_id`
//! both lock the scheduler, which masks interrupts and is not reentrant; every syscall handler
//! that journals an event has already let its `with_caps` closure return by the time it calls
//! [`record`], so the scheduler's lock and the journal's lock are always taken one at a time, in
//! this fixed order, never nested. Getting that backwards — taking the journal lock first and
//! then asking the scheduler who is running — would deadlock the first time a syscall on the
//! interrupted thread's behalf needed the same journal lock to record its own event.

pub use oxygen_audit::Action;
use oxygen_audit::{Event, Journal};

use crate::sync::SpinLock;

/// Number of events the journal retains before the oldest is evicted. See the module docs for
/// why this size and not a larger or smaller one.
const CAPACITY: usize = 64;

static JOURNAL: SpinLock<Journal<CAPACITY>> = SpinLock::new(Journal::new());

/// Records one audit event and returns the sequence number assigned to it.
///
/// Takes no lock of its own until the actor id has already been read — see the module docs on
/// why that order is load-bearing rather than stylistic.
pub fn record(action: Action, kind: u8, rights: u32, detail: u64) -> u64 {
    let actor = crate::sched::current_id();
    JOURNAL.lock().record(actor, action, kind, rights, detail)
}

/// The event at `index`, oldest retained first. `None` once `index` runs past how many are
/// currently retained.
pub fn event_at(index: usize) -> Option<Event> {
    JOURNAL.lock().get(index)
}

/// How many events are currently retained.
pub fn len() -> usize {
    JOURNAL.lock().len()
}

/// How many events have been evicted over the journal's entire lifetime. See the module docs on
/// why every eviction is counted rather than happening silently.
pub fn dropped() -> u64 {
    JOURNAL.lock().dropped()
}

/// Prints every retained event, oldest first — the kernel-side view used by the selftest's boot
/// output, as distinct from `SYS_AUDIT`, which is how a user thread reads the same record.
pub fn dump() {
    let guard = JOURNAL.lock();
    for event in guard.iter() {
        crate::println!(
            "  [audit] #{} thread {} {:?} kind {} rights {:#x} detail {:#x}",
            event.seq,
            event.actor,
            event.action,
            event.kind,
            event.rights,
            event.detail,
        );
    }
}
