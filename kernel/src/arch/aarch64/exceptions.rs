//! Exception vectors.
//!
//! AArch64 dispatches every exception through a 2 KiB table whose base sits in `VBAR_EL1`. The
//! table has sixteen slots of 128 bytes each: four kinds of exception (synchronous, IRQ, FIQ,
//! SError) for each of four sources (current EL on SP0, current EL on SPx, lower EL in 64-bit,
//! lower EL in 32-bit). The layout is fixed by hardware — the CPU computes an offset into the
//! table, so a slot in the wrong place sends the wrong exception to the wrong handler.
//!
//! Each slot has room for only a few instructions, so every one of them jumps to shared code that
//! saves state and calls into Rust.

use core::arch::global_asm;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::println;

/// Set by the selftest immediately before it writes to `.text` on purpose.
///
/// Without this the write would be reported as a fatal kernel fault, which is exactly what it
/// would be at any other moment. Announcing the expectation is what turns a crash into an
/// assertion.
static EXPECT_WRITE_FAULT: AtomicBool = AtomicBool::new(false);

/// Arms the check above.
pub fn expect_write_fault() {
    EXPECT_WRITE_FAULT.store(true, Ordering::SeqCst);
}

/// Whether an ESR describes a *permission* fault on a write, as opposed to a translation fault
/// (nothing mapped) or an alignment fault. The distinction is the whole point: an unmapped page
/// would also stop the write, and would prove nothing about permissions.
fn is_permission_write_fault(esr: u64) -> bool {
    let ec = esr >> 26;
    let iss = esr & 0x1FF_FFFF;
    // DFSC 0b0011LL is a permission fault, where LL is the level that refused it.
    let permission_fault = matches!(iss & 0x3F, 0b001100..=0b001111);
    let write = (iss >> 6) & 1 == 1;
    // Data abort, taken from the current EL (0b100101) or a lower one (0b100100).
    matches!(ec, 0b100100 | 0b100101) && permission_fault && write
}

/// Registers saved on exception entry.
///
/// `repr(C)` because the assembly below writes this layout by hand; reordering the fields here
/// without changing the assembly would silently misinterpret every field.
#[repr(C)]
#[derive(Debug)]
pub struct TrapFrame {
    pub x: [u64; 31],
    /// Where execution resumes — the address of the faulting or interrupted instruction.
    pub elr: u64,
    /// Saved processor state.
    pub spsr: u64,
    /// Exception Syndrome Register: what happened and why.
    pub esr: u64,
    /// Fault Address Register: which address, for the faults where that is meaningful.
    pub far: u64,
}

global_asm!(
    r#"
// Saves the full general-purpose set plus what `eret` needs, calls into Rust, restores.
//
// This lives OUTSIDE the vector table on purpose. A vector slot is exactly 128 bytes — 32
// instructions — and this sequence is longer than that, so inlining it into the table would push
// every later entry past its slot and the CPU, which indexes the table by arithmetic, would
// dispatch into the middle of the previous entry.
//
// The frame is 36 slots for 35 fields: 31 GPRs, then ELR, SPSR, ESR and FAR. It must cover every
// field TrapFrame declares — an undersized frame writes the last one past the allocation and
// corrupts the caller's stack, which shows up much later as a return to a nonsense address. The
// round up to an even count keeps SP 16-byte aligned, which AArch64 requires.
.macro TRAP_BODY handler
    sub     sp, sp, #(36 * 8)
    stp     x0,  x1,  [sp, #(0 * 8)]
    stp     x2,  x3,  [sp, #(2 * 8)]
    stp     x4,  x5,  [sp, #(4 * 8)]
    stp     x6,  x7,  [sp, #(6 * 8)]
    stp     x8,  x9,  [sp, #(8 * 8)]
    stp     x10, x11, [sp, #(10 * 8)]
    stp     x12, x13, [sp, #(12 * 8)]
    stp     x14, x15, [sp, #(14 * 8)]
    stp     x16, x17, [sp, #(16 * 8)]
    stp     x18, x19, [sp, #(18 * 8)]
    stp     x20, x21, [sp, #(20 * 8)]
    stp     x22, x23, [sp, #(22 * 8)]
    stp     x24, x25, [sp, #(24 * 8)]
    stp     x26, x27, [sp, #(26 * 8)]
    stp     x28, x29, [sp, #(28 * 8)]
    str     x30,      [sp, #(30 * 8)]

    mrs     x0, elr_el1
    mrs     x1, spsr_el1
    stp     x0, x1, [sp, #(31 * 8)]
    mrs     x0, esr_el1
    mrs     x1, far_el1
    stp     x0, x1, [sp, #(33 * 8)]

    mov     x0, sp
    bl      \handler

    // ELR/SPSR first: eret consumes them.
    ldp     x0, x1, [sp, #(31 * 8)]
    msr     elr_el1, x0
    msr     spsr_el1, x1

    ldp     x0,  x1,  [sp, #(0 * 8)]
    ldp     x2,  x3,  [sp, #(2 * 8)]
    ldp     x4,  x5,  [sp, #(4 * 8)]
    ldp     x6,  x7,  [sp, #(6 * 8)]
    ldp     x8,  x9,  [sp, #(8 * 8)]
    ldp     x10, x11, [sp, #(10 * 8)]
    ldp     x12, x13, [sp, #(12 * 8)]
    ldp     x14, x15, [sp, #(14 * 8)]
    ldp     x16, x17, [sp, #(16 * 8)]
    ldp     x18, x19, [sp, #(18 * 8)]
    ldp     x20, x21, [sp, #(20 * 8)]
    ldp     x22, x23, [sp, #(22 * 8)]
    ldp     x24, x25, [sp, #(24 * 8)]
    ldp     x26, x27, [sp, #(26 * 8)]
    ldp     x28, x29, [sp, #(28 * 8)]
    ldr     x30,      [sp, #(30 * 8)]
    add     sp, sp, #(36 * 8)
    eret
.endm

// One branch per slot, then pad to the next 128-byte boundary. The order is fixed by hardware:
// four exception kinds (sync, IRQ, FIQ, SError) for each of four sources.
.macro VECTOR target
    b       \target
    .align 7
.endm

.section .text
.align 11
.global __exception_vectors
__exception_vectors:
    // Current EL, SP_EL0 — we run on SP_EL1, so these mean something is very wrong.
    VECTOR __vec_sync_sp0
    VECTOR __vec_irq_sp0
    VECTOR __vec_fiq_sp0
    VECTOR __vec_serror_sp0
    // Current EL, SP_ELx — kernel faults and kernel interrupts.
    VECTOR __vec_sync
    VECTOR __vec_irq
    VECTOR __vec_fiq
    VECTOR __vec_serror
    // Lower EL, AArch64 — syscalls and user faults, once there is user mode.
    VECTOR __vec_sync_lower
    VECTOR __vec_irq_lower
    VECTOR __vec_fiq_lower
    VECTOR __vec_serror_lower
    // Lower EL, AArch32 — 32-bit userspace, which this OS does not support.
    VECTOR __vec_sync_lower32
    VECTOR __vec_irq_lower32
    VECTOR __vec_fiq_lower32
    VECTOR __vec_serror_lower32

__vec_sync_sp0:       TRAP_BODY trap_sync_sp0
__vec_irq_sp0:        TRAP_BODY trap_irq_sp0
__vec_fiq_sp0:        TRAP_BODY trap_fiq_sp0
__vec_serror_sp0:     TRAP_BODY trap_serror_sp0
__vec_sync:           TRAP_BODY trap_sync
__vec_irq:            TRAP_BODY trap_irq
__vec_fiq:            TRAP_BODY trap_fiq
__vec_serror:         TRAP_BODY trap_serror
__vec_sync_lower:     TRAP_BODY trap_sync_lower
__vec_irq_lower:      TRAP_BODY trap_irq_lower
__vec_fiq_lower:      TRAP_BODY trap_fiq_lower
__vec_serror_lower:   TRAP_BODY trap_serror_lower
__vec_sync_lower32:   TRAP_BODY trap_sync_lower32
__vec_irq_lower32:    TRAP_BODY trap_irq_lower32
__vec_fiq_lower32:    TRAP_BODY trap_fiq_lower32
__vec_serror_lower32: TRAP_BODY trap_serror_lower32
"#
);

/// Installs the vector table.
///
/// # Safety
/// Must run before interrupts are unmasked. Until `VBAR_EL1` points here, any exception is
/// dispatched through whatever the firmware left in place.
pub unsafe fn init() {
    unsafe extern "C" {
        static __exception_vectors: u8;
    }
    // SAFETY: writing VBAR_EL1 with the address of a correctly aligned table we define above.
    // The ISB ensures the write has taken effect before any later instruction can fault.
    unsafe {
        let base = &raw const __exception_vectors;
        core::arch::asm!(
            "msr vbar_el1, {}",
            "isb",
            in(reg) base as u64,
            options(nomem, nostack),
        );
    }
}

/// Decoded exception class — the top six bits of ESR.
fn describe(esr: u64) -> &'static str {
    match esr >> 26 {
        0b000000 => "unknown",
        0b000111 => "SIMD/FP access trapped",
        0b010101 => "SVC (system call)",
        0b100000 | 0b100001 => "instruction abort",
        0b100010 => "PC alignment fault",
        0b100100 | 0b100101 => "data abort",
        0b100110 => "SP alignment fault",
        0b111100 => "BRK (breakpoint)",
        _ => "unclassified",
    }
}

/// Which kind of abort, for the aborts where the distinction is the whole point.
///
/// A permission fault means the mapping exists and refused the access; a translation fault means
/// nothing was mapped there at all. Both stop the access, and only the first proves anything about
/// privilege — so an assertion that a user thread was refused has to be able to tell them apart.
fn fault_class(esr: u64) -> &'static str {
    if !matches!(esr >> 26, 0b100000 | 0b100001 | 0b100100 | 0b100101) {
        return "";
    }
    // DFSC/IFSC, the low six bits of ISS. The top four select the class, the low two the level
    // of the walk that raised it.
    let status = esr & 0x3F;
    match status >> 2 {
        0b0000 => " (address size fault)",
        0b0001 => " (translation fault — nothing was mapped)",
        0b0010 => " (access flag fault)",
        0b0011 => " (permission fault — the mapping refused it)",
        _ => "",
    }
}

/// Reports a fault we have no policy for yet, then stops.
///
/// Returning would resume the faulting instruction and fault again forever, so this deliberately
/// does not return. Once there are processes, a fault from userspace kills the process instead.
fn fatal(kind: &str, frame: &TrapFrame) -> ! {
    println!();
    println!("  [trap] {kind}");
    println!("  [trap] {} (esr {:#018x})", describe(frame.esr), frame.esr);
    println!("  [trap] elr {:#018x}  far {:#018x}", frame.elr, frame.far);
    println!("  [trap] spsr {:#018x}", frame.spsr);
    panic!("unhandled exception: {kind}");
}

macro_rules! fatal_trap {
    ($($name:ident => $label:literal),* $(,)?) => {
        $(
            #[unsafe(no_mangle)]
            extern "C" fn $name(frame: &mut TrapFrame) -> ! {
                fatal($label, frame)
            }
        )*
    };
}

fatal_trap! {
    trap_sync_sp0      => "synchronous (SP0)",
    trap_irq_sp0       => "irq (SP0)",
    trap_fiq_sp0       => "fiq (SP0)",
    trap_serror_sp0    => "serror (SP0)",
    trap_fiq           => "fiq (kernel)",
    trap_serror        => "serror (kernel)",
    trap_fiq_lower     => "fiq (user)",
    trap_serror_lower  => "serror (user)",
    trap_sync_lower32  => "synchronous (aarch32)",
    trap_irq_lower32   => "irq (aarch32)",
    trap_fiq_lower32   => "fiq (aarch32)",
    trap_serror_lower32=> "serror (aarch32)",
}

/// Synchronous kernel exception.
///
/// Fatal, with one exception: the W^X selftest deliberately writes to `.text` and needs the fault
/// it provokes to be treated as a pass. Anything else is a genuine kernel fault.
#[unsafe(no_mangle)]
extern "C" fn trap_sync(frame: &mut TrapFrame) -> ! {
    if EXPECT_WRITE_FAULT.load(Ordering::SeqCst) && is_permission_write_fault(frame.esr) {
        println!("  [selftest] write to .text refused by the MMU — W^X is enforced");
        super::semihosting::exit(0);
    }
    fatal("synchronous (kernel)", frame)
}

/// Kernel IRQ. This one returns: an interrupt is a normal event, not a fault.
///
/// Preemption happens here rather than inside the GIC handler, once the interrupt has been
/// acknowledged and ended. Switching threads with an interrupt still active would stall every
/// other interrupt at that priority for as long as the next thread runs.
///
/// The switch is safe at this point precisely because each thread has its own kernel stack: this
/// thread's full register state is already saved in the trap frame below us, and stays there until
/// it is scheduled again and this handler returns.
#[unsafe(no_mangle)]
extern "C" fn trap_irq(_frame: &mut TrapFrame) {
    super::gic::handle_irq();
    if crate::sched::reschedule_requested() {
        crate::sched::yield_now();
    }
}

/// Synchronous exception from EL0 — a system call, or a user thread doing something it may not.
///
/// This is the only trap in the kernel that is routinely *expected*. It returns rather than
/// panicking, and the value it leaves in the frame's `x0` is what userspace sees come back out of
/// its `svc`.
#[unsafe(no_mangle)]
extern "C" fn trap_sync_lower(frame: &mut TrapFrame) {
    /// Exception class for an `SVC` executed in AArch64 state.
    const EC_SVC64: u64 = 0b010101;

    if frame.esr >> 26 == EC_SVC64 {
        let args = [
            frame.x[0], frame.x[1], frame.x[2], frame.x[3], frame.x[4], frame.x[5],
        ];
        // The number is in x8, following the AArch64 Linux convention. ELR already points at the
        // instruction after the SVC — the hardware advances it — so there is nothing to adjust.
        frame.x[0] = crate::syscall::dispatch(frame.x[8], args);
        return;
    }

    user_fault(frame)
}

/// A user thread did something the hardware refused.
///
/// It kills the thread and nothing else. That is the entire point of having spent M4 on a
/// privilege boundary: before it, every fault was fatal to the machine, because every fault was
/// the kernel's. Now a user program can be wrong on its own.
fn user_fault(frame: &TrapFrame) -> ! {
    println!();
    println!(
        "  [trap] user fault: {}{}",
        describe(frame.esr),
        fault_class(frame.esr)
    );
    println!("  [trap] elr {:#018x}  far {:#018x}", frame.elr, frame.far);
    println!("  [trap] the thread is retired; the kernel is not affected");
    crate::sched::retire_current(crate::sched::EXIT_FAULTED)
}

/// IRQ taken while userspace was running. Same handling for now; it diverges once there are
/// processes to reschedule.
#[unsafe(no_mangle)]
extern "C" fn trap_irq_lower(_frame: &mut TrapFrame) {
    super::gic::handle_irq();
}
