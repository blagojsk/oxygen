//! Threads and a round-robin scheduler.
//!
//! Round-robin because it is the policy whose behaviour you can predict by reading it, and on the
//! machines this OS targets a scheduler that is cheap and fair beats one that is clever. Priorities
//! and sleeping belong here eventually; what must not creep in is a policy whose cost grows with
//! the number of threads, since these machines have little enough CPU to spare.
//!
//! Preemption is real: the timer interrupt marks the running thread for replacement, and the
//! switch happens on the way out of the interrupt handler. Because each thread has its own kernel
//! stack, a preempted thread's full register state is sitting in the trap frame on that stack, and
//! it is restored when its handler eventually returns.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use oxygen_cap::CapSpace;

use crate::arch::target::context::{self, Context};
use crate::sync::SpinLock;

/// Per-thread kernel stack. 16 KiB is chosen against the target hardware: enough for the interrupt
/// nesting this kernel permits, small enough that a hundred threads cost under two megabytes.
const STACK_SIZE: usize = 16 * 1024;

/// Capability slots per thread.
///
/// Fixed rather than growable, and small on purpose: a capability space that can grow without
/// bound is a way for one task to consume the kernel's memory, and a task that genuinely needs
/// hundreds of capabilities is describing a design problem rather than a sizing one.
pub const CAP_SLOTS: usize = 16;

/// Exit code reported for a thread the hardware stopped, as opposed to one that chose to exit.
/// Distinct from any syscall error so the two can never be confused for one another.
pub const EXIT_FAULTED: u64 = u64::MAX - 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Runnable or running — the scheduler makes no distinction while there is one core.
    Ready,
    /// Returned from its entry point. Kept in the table so its id stays meaningful.
    Finished,
}

pub struct Thread {
    pub id: u64,
    pub name: &'static str,
    pub state: State,
    context: Context,
    /// Everything this thread is allowed to do. Empty at birth: a thread is born with no
    /// authority at all and receives it explicitly, which is the only arrangement in which
    /// "what can this agent do?" has an answer you can actually read.
    caps: CapSpace<CAP_SLOTS>,
    /// Owned so it is freed when the thread is reaped. Never read directly — the context's stack
    /// pointer is what actually matters — but dropping it while the thread lives would hand its
    /// stack to somebody else.
    _stack: Option<Box<[u8]>>,
}

struct Scheduler {
    threads: Vec<Thread>,
    current: usize,
}

static SCHEDULER: SpinLock<Option<Scheduler>> = SpinLock::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
/// Set by the timer, cleared by the switch. Separate from the scheduler lock on purpose: the timer
/// interrupt must never block on a lock the thread it interrupted is holding.
static NEED_RESCHED: AtomicBool = AtomicBool::new(false);
static SWITCHES: AtomicU64 = AtomicU64::new(0);
/// The code the most recently retired thread stopped with, and whether one has retired at all.
/// Two values rather than a sentinel because every possible `u64` is a legitimate exit code.
static EXIT_CODE: AtomicU64 = AtomicU64::new(0);
static HAS_EXITED: AtomicBool = AtomicBool::new(false);

/// Adopts the currently executing code as thread 0.
///
/// Something has to be running before anything can be switched away from, and that something is
/// the boot path. Its context is filled in by the first switch rather than constructed here — the
/// values only become meaningful at the moment it is suspended.
pub fn init() {
    let boot = Thread {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        name: "boot",
        state: State::Ready,
        context: Context::default(),
        caps: CapSpace::new(),
        _stack: None,
    };
    *SCHEDULER.lock() = Some(Scheduler {
        threads: alloc::vec![boot],
        current: 0,
    });
}

/// Creates a runnable thread. Returns its id.
pub fn spawn(name: &'static str, entry: extern "C" fn(usize), arg: usize) -> u64 {
    let stack = alloc::vec![0u8; STACK_SIZE].into_boxed_slice();
    // The stack grows downward, so the context starts at the top of the allocation.
    let stack_top = stack.as_ptr() as u64 + STACK_SIZE as u64;

    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let thread = Thread {
        id,
        name,
        state: State::Ready,
        context: Context::for_new_thread(entry, arg, stack_top),
        caps: CapSpace::new(),
        _stack: Some(stack),
    };

    let mut guard = SCHEDULER.lock();
    if let Some(s) = guard.as_mut() {
        s.threads.push(thread);
    }
    id
}

/// Marks the running thread for replacement. Called from the timer interrupt.
pub fn request_reschedule() {
    NEED_RESCHED.store(true, Ordering::Relaxed);
}

pub fn reschedule_requested() -> bool {
    NEED_RESCHED.load(Ordering::Relaxed)
}

pub fn switches() -> u64 {
    SWITCHES.load(Ordering::Relaxed)
}

pub fn current_id() -> u64 {
    SCHEDULER
        .lock()
        .as_ref()
        .map_or(0, |s| s.threads[s.current].id)
}

/// Hands the CPU to the next ready thread.
///
/// Safe to call with nothing to switch to: if this is the only runnable thread it returns
/// immediately, which is what makes it usable from both an idle loop and an interrupt.
pub fn yield_now() {
    NEED_RESCHED.store(false, Ordering::Relaxed);

    // The two raw pointers are taken while the lock is held, but the switch happens after it is
    // released. Holding a spin lock across a context switch deadlocks the moment the thread we
    // switch to tries to take it — and it always does, on its way back through here.
    let (from, to) = {
        let mut guard = SCHEDULER.lock();
        let Some(s) = guard.as_mut() else { return };

        let Some(next) = next_ready(s) else { return };
        if next == s.current {
            return;
        }

        let from: *mut Context = &mut s.threads[s.current].context;
        let to: *const Context = &s.threads[next].context;
        s.current = next;
        (from, to)
    };

    SWITCHES.fetch_add(1, Ordering::Relaxed);
    // SAFETY: both contexts belong to threads in the table, which outlive the switch — threads are
    // never removed. `to` is either a thread suspended inside this function, which resumes here,
    // or a new thread whose context points at the trampoline and whose stack we allocated.
    unsafe { context::__switch_context(from, to) };
}

/// Round-robin: start after the current thread and take the first ready one, wrapping. Scanning
/// from `current + 1` rather than from zero is what makes it round-robin rather than a strict
/// priority for whoever sits earliest in the table.
fn next_ready(s: &Scheduler) -> Option<usize> {
    let n = s.threads.len();
    (1..=n)
        .map(|offset| (s.current + offset) % n)
        .find(|&i| s.threads[i].state == State::Ready)
}

/// Retires the running thread. Reached when a thread's entry point returns.
#[unsafe(no_mangle)]
extern "C" fn thread_exit() -> ! {
    retire_current(0)
}

/// Stops the running thread for good and records why.
///
/// Does not return, and cannot: it is called from inside a syscall or a fault, where the trap
/// frame below belongs to a thread that must never resume. The thread stays in the table rather
/// than being removed — its id keeps meaning something, and its stack is not handed to anyone
/// while a frame on it is still live.
pub fn retire_current(code: u64) -> ! {
    EXIT_CODE.store(code, Ordering::SeqCst);
    HAS_EXITED.store(true, Ordering::SeqCst);
    {
        let mut guard = SCHEDULER.lock();
        if let Some(s) = guard.as_mut() {
            s.threads[s.current].state = State::Finished;
        }
    }
    loop {
        yield_now();
        core::hint::spin_loop();
    }
}

/// The code the last thread to retire stopped with, if any has.
pub fn exit_code() -> Option<u64> {
    HAS_EXITED
        .load(Ordering::SeqCst)
        .then(|| EXIT_CODE.load(Ordering::SeqCst))
}

/// Forgets the last exit code, so a later retirement can be waited for distinctly.
pub fn clear_exit() {
    HAS_EXITED.store(false, Ordering::SeqCst);
}

/// Runs `f` against the capability space of the thread currently on the CPU.
///
/// Scoped rather than handing out a reference because the space lives inside the thread table,
/// behind the scheduler's lock. The closure must not switch threads — nothing it is given the
/// means to do can.
pub fn with_caps<R>(f: impl FnOnce(&mut CapSpace<CAP_SLOTS>) -> R) -> R {
    let mut guard = SCHEDULER.lock();
    let s = guard
        .as_mut()
        .expect("capability access before the scheduler was initialised");
    let current = s.current;
    f(&mut s.threads[current].caps)
}

/// How many threads exist, and how many of those are still runnable.
pub fn census() -> (usize, usize) {
    let guard = SCHEDULER.lock();
    guard.as_ref().map_or((0, 0), |s| {
        (
            s.threads.len(),
            s.threads.iter().filter(|t| t.state == State::Ready).count(),
        )
    })
}

/// Prints the thread table.
///
/// The first thing anyone debugging a scheduler wants, and the reason threads carry a name at all:
/// an id alone tells you something is stuck, a name tells you what.
pub fn dump() {
    let guard = SCHEDULER.lock();
    let Some(s) = guard.as_ref() else { return };
    for (i, t) in s.threads.iter().enumerate() {
        let marker = if i == s.current { '>' } else { ' ' };
        let state = match t.state {
            State::Ready => "ready",
            State::Finished => "finished",
        };
        crate::println!("  [sched] {marker} #{} {:<10} {}", t.id, t.name, state);
    }
}
