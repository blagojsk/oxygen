//! Crossing into EL0, and the loader that makes a page safe to run there.
//!
//! Everything the kernel has done so far it has done to itself. This is the first boundary the
//! hardware enforces on the kernel's behalf: below EL1 a thread cannot reach kernel memory, cannot
//! touch a system register, and cannot reach a device except by asking. Asking is a syscall, and
//! what it is allowed to ask for is a capability.
//!
//! The loader exists because code cannot simply be declared. A program arrives as bytes, and bytes
//! live in writable memory; making them executable while they are still writable would hand
//! userspace exactly the write-then-execute primitive the whole page-table design refuses the
//! kernel. So the sequence is fixed: copy while writable, publish to the instruction stream, then
//! narrow the page to read-execute and never widen it again.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};

use oxygen_aarch64::paging::{Access, L3_PAGE_SIZE};

use super::mmu;
use crate::syscall::{self, Rights as R};

const PAGE: usize = L3_PAGE_SIZE as usize;

/// A page of memory the MMU can retarget on its own.
///
/// Aligned and sized to exactly one 4 KiB page because permissions have page granularity: a
/// user-visible buffer sharing a page with kernel data would make that kernel data user-visible
/// too, which is the sort of mistake that never announces itself.
#[repr(C, align(4096))]
struct Page([u8; PAGE]);

/// Where the user program is loaded. Zeroed, so it costs image space only in `.bss`.
static mut USER_TEXT: Page = Page([0; PAGE]);
/// How many user threads can be running at once.
///
/// Each needs its own stack page, and a stack page is the smallest unit of memory the MMU can give
/// distinct permissions to. Four because that is what the current programs need; a real process
/// model allocates these from the frame allocator instead of reserving them.
pub const USER_SLOTS: usize = 4;

/// The user threads' stacks. Separate pages from the code, never executable.
static mut USER_STACKS: [Page; USER_SLOTS] = [const { Page([0; PAGE]) }; USER_SLOTS];

/// Where the loaded program starts and where its stack ends, published once by [`load`].
///
/// Statics rather than return values because the thread that enters EL0 is spawned with a single
/// `usize` argument, and that argument is already spoken for: it carries the capability the
/// program is born holding.
static ENTRY: AtomicU64 = AtomicU64::new(0);
static TRESPASS_ENTRY: AtomicU64 = AtomicU64::new(0);
static SERVER_ENTRY: AtomicU64 = AtomicU64::new(0);
static CLIENT_ENTRY: AtomicU64 = AtomicU64::new(0);

global_asm!(
    r#"
// The first program to run outside the kernel.
//
// It lives in .rodata because until the loader copies it, it is data — the kernel never executes
// these bytes in place. Everything here is position-independent: it is assembled at one address
// and runs at another, so every reference to its own strings goes through ADR.
//
// What it proves, in order: EL0 can reach the console only by presenting a capability; authority
// can be narrowed and handed on; and a withdrawn grant stops working while the grantor's own
// keeps working. The last one is the entire argument for carrying a derivation tree.
.section .rodata
.balign 4
.global __user_program_start
__user_program_start:
    mov     x19, x0                     // the console capability the kernel handed over

    // Speak, using the capability we were given.
    mov     x0, x19
    adr     x1, 8f
    adr     x2, 9f
    sub     x2, x2, x1
    mov     x8, #{sys_write}
    svc     #0

    // Narrow it: a child capability that may write and nothing else.
    mov     x0, x19
    mov     x1, #{rights_write}
    mov     x8, #{sys_delegate}
    svc     #0
    mov     x20, x0

    // The delegate works.
    mov     x0, x20
    adr     x1, 10f
    adr     x2, 11f
    sub     x2, x2, x1
    mov     x8, #{sys_write}
    svc     #0

    // Withdraw everything derived from the original.
    mov     x0, x19
    mov     x8, #{sys_revoke}
    svc     #0

    // The same call, the same handle — and now it must fail.
    mov     x0, x20
    adr     x1, 10f
    adr     x2, 11f
    sub     x2, x2, x1
    mov     x8, #{sys_write}
    svc     #0

    // Hand the refusal back so the kernel can assert on which refusal it was.
    mov     x8, #{sys_exit}
    svc     #0
1:  b       1b

8:  .ascii  "  [user] hello from EL0, through a console capability\n"
9:
10: .ascii  "  [user] and again, through a capability derived from it\n"
11:
.balign 4
.global __user_program_end
__user_program_end:

// A program that tries to read kernel memory, to prove EL0 cannot.
//
// The address arrives in x0 and is a kernel page the kernel itself is using, so a fault here is a
// *permission* fault and not merely an unmapped one. Nothing follows the load: if it returns, the
// exit below reports success, and success is the failure this asserts against.
.balign 4
.global __user_trespass_start
__user_trespass_start:
    ldr     x1, [x0]
    mov     x0, xzr
    mov     x8, #{sys_exit}
    svc     #0
2:  b       2b
.balign 4
.global __user_trespass_end
__user_trespass_end:

// A server: publishes an endpoint under a name and blocks until somebody sends to it.
//
// It never learns who its caller is and never needs to. The name is the whole introduction, which
// is what lets two programs that cannot pass each other a capability still find each other.
.balign 4
.global __user_server_start
__user_server_start:
    mov     x19, x0                     // console capability
    mov     x21, x1                     // registry capability

    mov     x0, x19
    adr     x1, 20f
    adr     x2, 21f
    sub     x2, x2, x1
    mov     x8, #{sys_write}
    svc     #0

    mov     x8, #{sys_endpoint}
    svc     #0
    mov     x20, x0

    mov     x0, x21
    adr     x1, 24f
    mov     x2, #4
    mov     x3, x20
    mov     x8, #{sys_register}
    svc     #0

    // Receive into a buffer on our own stack. This is the call that blocks: nothing has been sent
    // yet, so the thread leaves the run queue entirely until the client wakes it.
    mov     x22, sp
    sub     x22, x22, #128
    mov     x0, x20
    mov     x1, x22
    mov     x2, #128
    mov     x8, #{sys_recv}
    svc     #0

    // Hand back the first payload byte, twelve bytes past the header, so the kernel can assert
    // that what arrived is what was sent rather than merely that something arrived.
    ldrb    w0, [x22, #12]
    mov     x8, #{sys_exit}
    svc     #0
3:  b       3b
20: .ascii  "  [user] server: publishing \"echo\", then waiting for a message\n"
21:
24: .ascii  "echo"

// A client: finds that endpoint by name and sends one typed message to it.
.balign 4
.global __user_client_start
__user_client_start:
    mov     x19, x0                     // console capability
    mov     x21, x1                     // registry capability

    mov     x0, x19
    adr     x1, 30f
    adr     x2, 31f
    sub     x2, x2, x1
    mov     x8, #{sys_write}
    svc     #0

    mov     x0, x21
    adr     x1, 34f
    mov     x2, #4
    mov     x8, #{sys_lookup}
    svc     #0
    mov     x20, x0

    mov     x0, x20
    mov     x1, #7                      // interface
    mov     x2, #3                      // method
    adr     x3, 36f
    mov     x4, #1
    mov     x8, #{sys_send}
    svc     #0

    mov     x0, xzr
    mov     x8, #{sys_exit}
    svc     #0
4:  b       4b
30: .ascii  "  [user] client: looking \"echo\" up and sending to it\n"
31:
34: .ascii  "echo"
36: .byte   42
.balign 4
.global __user_client_end
__user_client_end:
"#,
    sys_write = const syscall::SYS_WRITE,
    sys_delegate = const syscall::SYS_DELEGATE,
    sys_revoke = const syscall::SYS_REVOKE,
    sys_exit = const syscall::SYS_EXIT,
    rights_write = const R::WRITE.bits(),
    sys_endpoint = const syscall::SYS_ENDPOINT,
    sys_send = const syscall::SYS_SEND,
    sys_recv = const syscall::SYS_RECV,
    sys_lookup = const syscall::SYS_LOOKUP,
    sys_register = const syscall::SYS_REGISTER,
);

unsafe extern "C" {
    static __user_program_start: u8;
    static __user_program_end: u8;
    static __user_trespass_start: u8;
    static __user_server_start: u8;
    static __user_client_start: u8;
    static __user_client_end: u8;
}

/// Copies the program into its page, publishes it to the instruction stream, and narrows the
/// permissions. Returns the entry point and the top of the user stack.
///
/// # Safety
/// Runs once, before any user thread exists. Retargeting a live page's permissions underneath
/// something already using it would fault it at an arbitrary instruction.
pub unsafe fn load() {
    // Both programs are copied as one span. They are emitted contiguously into the same section,
    // so a single copy preserves the distance between them and each one's entry point is found by
    // the offset it had at assembly time.
    let src = &raw const __user_program_start;
    let end = &raw const __user_client_end;
    let len = end as usize - src as usize;
    assert!(len <= PAGE, "user programs do not fit in one page");

    let text = (&raw mut USER_TEXT).cast::<u8>();

    // SAFETY: `src..end` is the program the assembler emitted, `text` is a whole page we own, and
    // the length is checked above to fit. The regions cannot overlap: one is in .rodata, the other
    // in .bss.
    unsafe { core::ptr::copy_nonoverlapping(src, text, len) };

    // SAFETY: the bytes are in place; this makes them visible to instruction fetch.
    unsafe { publish_as_code(text as u64, len) };

    // SAFETY: nothing is executing from or writing to any of these pages yet — no user thread has
    // been entered.
    unsafe {
        mmu::remap_page(text as u64, Access::UserCode);
        for slot in 0..USER_SLOTS {
            mmu::remap_page(stack_base(slot), Access::UserData);
        }
    }

    // Each program's entry point is where it sat relative to the start of the copied span.
    let base = text as u64;
    // SAFETY: taking the addresses of linker-provided symbols, all inside the copied span.
    let offset = |s: *const u8| base + ((s as usize - src as usize) as u64);
    ENTRY.store(base, Ordering::SeqCst);
    TRESPASS_ENTRY.store(offset(&raw const __user_trespass_start), Ordering::SeqCst);
    SERVER_ENTRY.store(offset(&raw const __user_server_start), Ordering::SeqCst);
    CLIENT_ENTRY.store(offset(&raw const __user_client_start), Ordering::SeqCst);
}

/// Base address of one user stack page.
fn stack_base(slot: usize) -> u64 {
    assert!(slot < USER_SLOTS, "no such user stack");
    (&raw const USER_STACKS) as u64 + (slot * PAGE) as u64
}

/// Top of one user stack, which is where that thread's `sp` starts.
pub fn stack_top(slot: usize) -> u64 {
    stack_base(slot) + PAGE as u64
}

/// Entry point of the program that exercises capabilities.
pub fn program() -> u64 {
    ENTRY.load(Ordering::SeqCst)
}

/// Entry point of the program that deliberately reads where it may not.
pub fn trespasser() -> u64 {
    TRESPASS_ENTRY.load(Ordering::SeqCst)
}

/// Entry point of the program that publishes an endpoint and waits on it.
pub fn server() -> u64 {
    SERVER_ENTRY.load(Ordering::SeqCst)
}

/// Entry point of the program that looks that endpoint up and sends to it.
pub fn client() -> u64 {
    CLIENT_ENTRY.load(Ordering::SeqCst)
}

/// Makes freshly written bytes executable, as far as the caches are concerned.
///
/// Instruction and data caches are not coherent with each other on AArch64. Code written through
/// the data side sits in the data cache while the instruction side still holds — or fetches —
/// whatever was there before. Skipping this does not fail predictably: it works until a cache line
/// happens to be stale, and then executes something that was never written.
///
/// # Safety
/// `addr..addr + len` must be memory the caller owns.
unsafe fn publish_as_code(addr: u64, len: usize) {
    // CTR_EL0 reports cache line sizes as log2 of the number of 4-byte words.
    let ctr: u64;
    // SAFETY: CTR_EL0 is readable at EL1 and reading it has no side effects.
    unsafe { core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack)) };
    let dcache_line = 4u64 << ((ctr >> 16) & 0xF);
    let icache_line = 4u64 << (ctr & 0xF);
    let end = addr + len as u64;

    // Clean the data cache to the point of unification, so the bytes reach memory the instruction
    // side will look at.
    let mut p = addr & !(dcache_line - 1);
    while p < end {
        // SAFETY: maintenance by virtual address on memory the caller owns.
        unsafe { core::arch::asm!("dc cvau, {}", in(reg) p, options(nostack)) };
        p += dcache_line;
    }
    // SAFETY: ordering barrier; the cleans must complete before the invalidates begin.
    unsafe { core::arch::asm!("dsb ish", options(nostack)) };

    // Then discard any stale instruction-cache lines covering the same addresses.
    let mut p = addr & !(icache_line - 1);
    while p < end {
        // SAFETY: maintenance by virtual address on memory the caller owns.
        unsafe { core::arch::asm!("ic ivau, {}", in(reg) p, options(nostack)) };
        p += icache_line;
    }
    // SAFETY: the ISB is what makes the invalidation visible to this core's own fetch.
    unsafe { core::arch::asm!("dsb ish", "isb", options(nostack)) };
}

/// Whether a user-supplied pointer names memory the kernel is willing to read on its behalf.
///
/// A syscall argument is a number chosen by userspace, and the kernel dereferences it. Without
/// this check `write(handle, 0xffff_0000, 4096)` would print kernel memory to the console — the
/// capability system would be intact and the secret would be gone anyway.
pub fn is_user_readable(ptr: u64, len: u64) -> bool {
    let text = (&raw const USER_TEXT) as u64;
    within(ptr, len, text) || is_user_writable(ptr, len)
}

/// Whether a user-supplied pointer names memory the kernel may write results into.
///
/// Narrower than [`is_user_readable`], and deliberately so: the program text is readable and must
/// never be writable, or a syscall that returns data into a caller-chosen buffer becomes a way to
/// rewrite the caller's own code.
pub fn is_user_writable(ptr: u64, len: u64) -> bool {
    (0..USER_SLOTS).any(|slot| within(ptr, len, stack_base(slot)))
}

/// Whether `ptr..ptr + len` lies entirely inside the page starting at `page_base`.
fn within(ptr: u64, len: u64, page_base: u64) -> bool {
    match ptr.checked_add(len) {
        Some(last) => ptr >= page_base && last <= page_base + PAGE as u64,
        // An overflowing range names no memory at all, and treating it as in-bounds is exactly
        // how a length check gets walked past.
        None => false,
    }
}

/// Leaves EL1 for EL0 and does not come back — the way back is a trap.
///
/// # Safety
/// `entry` must be a mapped, EL0-executable address and `stack_top` a mapped, EL0-writable one.
/// Both are what [`load`] returns.
pub unsafe fn enter(entry: u64, stack_top: u64, arg0: u64, arg1: u64) -> ! {
    // SAFETY: SPSR of zero selects EL0t with DAIF clear. Clear on purpose: masking interrupts on
    // the way down would make a user thread unpreemptable, and a single spinning user program
    // would take the machine with it.
    unsafe {
        core::arch::asm!(
            "msr sp_el0, {stack}",
            "msr elr_el1, {entry}",
            "msr spsr_el1, xzr",
            "isb",
            "eret",
            stack = in(reg) stack_top,
            entry = in(reg) entry,
            in("x0") arg0,
            in("x1") arg1,
            options(noreturn),
        )
    }
}
