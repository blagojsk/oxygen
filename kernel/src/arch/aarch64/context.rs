//! Switching one thread of execution for another.
//!
//! Only the callee-saved registers are here, and that is not an oversight. The AArch64 procedure
//! call standard says a function may destroy x0–x18 freely, so whoever called us has already
//! preserved anything of theirs worth keeping. Saving all thirty-one registers would be copying
//! values the compiler has already spilled — pure cost, on the hottest path in the kernel.
//!
//! The caller-saved half is not lost, incidentally: a thread preempted by an interrupt has its
//! full register set sitting in the trap frame on its own stack, and gets it back when that
//! handler eventually returns.

use core::arch::global_asm;

/// A suspended thread, as seen by the switcher: the registers whose values must survive.
///
/// `repr(C)` because the assembly below indexes this layout by hand. Reordering a field here
/// without changing the offsets there restores the wrong values into the wrong registers, and the
/// symptom is a thread resuming with a corrupted stack pointer.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct Context {
    /// x19–x28: callee-saved general purpose.
    pub x19: u64,
    pub x20: u64,
    pub x21: u64,
    pub x22: u64,
    pub x23: u64,
    pub x24: u64,
    pub x25: u64,
    pub x26: u64,
    pub x27: u64,
    pub x28: u64,
    /// Frame pointer.
    pub x29: u64,
    /// Link register — where `switch` returns to, and therefore where the thread resumes.
    pub x30: u64,
    pub sp: u64,
}

global_asm!(
    r#"
.section .text
.global __switch_context
// __switch_context(from: *mut Context, to: *const Context)
//
// Writes the current callee-saved state into `from`, loads `to`, and returns — but returns onto
// the *other* thread's stack, so the `ret` at the end lands wherever that thread last stopped.
// That is the whole trick: the switch is an ordinary function call that comes back as somebody
// else.
__switch_context:
    stp     x19, x20, [x0, #(0 * 8)]
    stp     x21, x22, [x0, #(2 * 8)]
    stp     x23, x24, [x0, #(4 * 8)]
    stp     x25, x26, [x0, #(6 * 8)]
    stp     x27, x28, [x0, #(8 * 8)]
    stp     x29, x30, [x0, #(10 * 8)]
    mov     x2, sp
    str     x2,       [x0, #(12 * 8)]

    ldp     x19, x20, [x1, #(0 * 8)]
    ldp     x21, x22, [x1, #(2 * 8)]
    ldp     x23, x24, [x1, #(4 * 8)]
    ldp     x25, x26, [x1, #(6 * 8)]
    ldp     x27, x28, [x1, #(8 * 8)]
    ldp     x29, x30, [x1, #(10 * 8)]
    ldr     x2,       [x1, #(12 * 8)]
    mov     sp, x2
    ret

.global __thread_trampoline
// Where a thread that has never run before begins.
//
// A brand-new thread has no return address to resume at, so its context is built with x30 pointing
// here and its entry point parked in a callee-saved register. Those survive the load above, so by
// the time this executes they hold what the spawner put there.
__thread_trampoline:
    mov     x0, x20          // argument
    blr     x19              // entry point
    bl      thread_exit      // if it ever returns, retire the thread rather than falling off
1:  b       1b
"#
);

unsafe extern "C" {
    /// # Safety
    /// Both pointers must be valid `Context`s, and `to` must describe a thread whose stack is
    /// mapped and not in use by anything else.
    pub unsafe fn __switch_context(from: *mut Context, to: *const Context);
    fn __thread_trampoline();
}

impl Context {
    /// Builds the context of a thread that has not started yet.
    ///
    /// `stack_top` must be 16-byte aligned: AArch64 faults on a stack pointer that is not, at
    /// every instruction that touches it.
    pub fn for_new_thread(entry: extern "C" fn(usize), arg: usize, stack_top: u64) -> Context {
        Context {
            x19: entry as *const () as u64,
            x20: arg as u64,
            x30: __thread_trampoline as *const () as u64,
            sp: stack_top & !0xF,
            ..Default::default()
        }
    }
}
