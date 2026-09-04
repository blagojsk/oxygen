//! The shell — the first thing here a person can actually talk to.
//!
//! Written in Rust rather than assembly, and running at EL0 like anything else, which imposes two
//! rules that the rest of the kernel does not have to think about.
//!
//! First, every function and every byte of data it touches must live in `.user_text`. That is the
//! only section of the image EL0 may fetch from or read; a string literal left in the kernel's
//! `.rodata` is unreachable from here, and the symptom is a permission fault on a line that looks
//! like a `print`.
//!
//! Second, nothing may call *out* of that section. A slice index that can panic compiles to a call
//! into the kernel's panic machinery, and `let buf = [0u8; 128]` can compile to a call to `memset`
//! — both of which live in kernel `.text` and fault the instant they are reached. So: raw pointer
//! reads instead of indexing, `MaybeUninit` instead of zeroed arrays, and lengths tracked by hand.
//! It reads as though written by somebody who does not trust the compiler, and that is exactly
//! right: the boundary is real, and the compiler has not been told about it.
//!
//! The same rule is why the arithmetic below is all `wrapping_*`. A debug build inserts overflow
//! checks, and an overflow check that fires calls `core::panicking` — in kernel `.text`. Wrapping
//! arithmetic emits no check at all, so the question never arises. `scripts/check-userspace.sh`
//! enforces the whole rule mechanically: it disassembles `.user` and fails if any branch leaves
//! it, because this is not a class of mistake anyone should be relying on review to catch.
//!
//! It lives under `arch/aarch64` because it is machine code that has to run at EL0 on this
//! architecture. A second architecture needs the syscall stubs split from the logic.

use core::mem::MaybeUninit;

use crate::syscall as sys;

/// Declares a string that lives where EL0 can read it.
///
/// The indirection exists because `static S: &[u8] = b"..."` puts the *pointer* in this section
/// and leaves the bytes in the kernel's `.rodata`, which reads correctly and faults at run time.
macro_rules! user_str {
    ($name:ident, $text:literal) => {
        #[unsafe(link_section = ".user_text")]
        static $name: [u8; $text.len()] = *$text;
    };
}

user_str!(BANNER, b"\n  oxygen shell. type 'help'.\n");
user_str!(PROMPT, b"oxygen$ ");
user_str!(NEWLINE, b"\n");
user_str!(ERASE, b"\x08 \x08");
user_str!(UNKNOWN, b"unknown command. try 'help'.\n");
user_str!(HELP_TEXT, b"  help      this list\n  services  what is published in the registry\n  uptime    timer ticks since boot\n  echo ...  say something back\n");
user_str!(CMD_HELP, b"help");
user_str!(CMD_SERVICES, b"services");
user_str!(CMD_UPTIME, b"uptime");
user_str!(CMD_ECHO, b"echo");
user_str!(NO_SERVICES, b"(nothing published)\n");
user_str!(INDENT, b"  ");

/// The longest line the shell will accept before it stops echoing.
const LINE: usize = 128;

/// One system call.
///
/// # Safety
/// The kernel validates every argument it dereferences, but a wrong number here still asks for the
/// wrong operation, and pointers must name this thread's own memory.
#[unsafe(link_section = ".user_text")]
unsafe fn syscall(number: u64, a0: u64, a1: u64, a2: u64, a3: u64) -> u64 {
    let out: u64;
    // SAFETY: `svc` traps to the kernel, which reads x8 and x0–x3 and returns in x0. Nothing else
    // is clobbered, and the kernel restores every register it saved.
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") a0 => out,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            options(nostack),
        );
    }
    out
}

/// Writes bytes that live in this section.
///
/// # Safety
/// `console` must be a console capability carrying `WRITE`.
#[unsafe(link_section = ".user_text")]
unsafe fn put(console: u64, text: &[u8]) {
    // SAFETY: the pointer and length describe a `.user_text` static, which EL0 may read.
    unsafe {
        syscall(
            sys::SYS_WRITE,
            console,
            text.as_ptr() as u64,
            text.len() as u64,
            0,
        )
    };
}

/// Writes a decimal number, most significant digit first.
///
/// # Safety
/// As [`put`].
#[unsafe(link_section = ".user_text")]
unsafe fn put_number(console: u64, mut value: u64) {
    // Twenty digits is the most a u64 can need. Uninitialised rather than zeroed: zeroing twenty
    // bytes is where the compiler reaches for `memset`, which is not reachable from here.
    let mut digits = MaybeUninit::<[u8; 20]>::uninit();
    let start = digits.as_mut_ptr().cast::<u8>();
    let mut written = 0usize;

    loop {
        let digit = b'0'.wrapping_add((value % 10) as u8);
        // SAFETY: at most twenty iterations, so the write stays inside the array.
        unsafe { core::ptr::write(start.add(written), digit) };
        written = written.wrapping_add(1);
        value /= 10;
        if value == 0 {
            break;
        }
    }

    // Produced backwards, so emit it backwards. One byte per call keeps this allocation-free.
    while written > 0 {
        written = written.wrapping_sub(1);
        // SAFETY: `written` is an index we just wrote, and the byte lives on this thread's stack,
        // which the kernel accepts as readable user memory.
        unsafe {
            syscall(sys::SYS_WRITE, console, start.add(written) as u64, 1, 0);
        }
    }
}

/// Whether the first `len` bytes at `buf` are exactly `word`.
///
/// # Safety
/// `buf` must point to at least `len` readable bytes.
#[unsafe(link_section = ".user_text")]
unsafe fn is(buf: *const u8, len: usize, word: &[u8]) -> bool {
    if len != word.len() {
        return false;
    }
    let mut i = 0;
    while i < word.len() {
        // SAFETY: `i` is bounded by `word.len()`, which equals `len`. Both sides are read through
        // raw pointers: indexing inserts a bounds check that calls into the kernel's panic path,
        // and even `get_unchecked` is a real call in a debug build.
        unsafe {
            if core::ptr::read(buf.add(i)) != core::ptr::read(word.as_ptr().add(i)) {
                return false;
            }
        }
        i = i.wrapping_add(1);
    }
    true
}

/// Reads one line, echoing as it goes, and returns how long it is.
///
/// Echo is the shell's job rather than the kernel's: the kernel has no idea whether the thing on
/// the other end wants to see what it typed. An agent driving this over a pipe does not.
///
/// # Safety
/// `buf` must point to at least `LINE` writable bytes on this thread's stack.
#[unsafe(link_section = ".user_text")]
unsafe fn read_line(console: u64, buf: *mut u8) -> usize {
    let mut len = 0usize;
    loop {
        let mut byte = 0u8;
        // SAFETY: `byte` is on this thread's stack, which the kernel accepts as writable.
        let read = unsafe { syscall(sys::SYS_READ, console, (&raw mut byte) as u64, 1, 0) };
        if (read as i64) <= 0 {
            continue;
        }

        if byte == b'\r' || byte == b'\n' {
            // SAFETY: writing a `.user_text` static.
            unsafe { put(console, &NEWLINE) };
            return len;
        }
        if byte == 0x08 || byte == 0x7F {
            if len > 0 {
                len = len.wrapping_sub(1);
                // Back up, paint over, back up again — the only way to erase on a dumb terminal.
                // SAFETY: writing a `.user_text` static.
                unsafe { put(console, &ERASE) };
            }
            continue;
        }
        // Written out rather than as a range. Clippy prefers `(0x20..0x7F).contains(&byte)` and
        // is wrong here for a reason it cannot see: that is a call into `core`, which lives in
        // kernel `.text`, which EL0 cannot fetch. It faults. `check-userspace.sh` catches it.
        #[allow(clippy::manual_range_contains)]
        let printable = byte >= 0x20 && byte < 0x7F;
        if printable && len < LINE {
            // SAFETY: `len` is checked against LINE, and `buf` has that much room.
            unsafe {
                core::ptr::write(buf.add(len), byte);
                syscall(sys::SYS_WRITE, console, (&raw const byte) as u64, 1, 0);
            }
            len = len.wrapping_add(1);
        }
    }
}

/// Lists what the registry currently holds.
///
/// # Safety
/// `registry` must be a registry capability carrying `READ`.
#[unsafe(link_section = ".user_text")]
unsafe fn list_services(console: u64, registry: u64) {
    let mut name = MaybeUninit::<[u8; 16]>::uninit();
    let start = name.as_mut_ptr().cast::<u8>();
    let mut index = 0u64;

    loop {
        // SAFETY: `start` is sixteen writable bytes on this thread's stack.
        let len = unsafe { syscall(sys::SYS_SERVICES, registry, index, start as u64, 16) };
        if (len as i64) <= 0 {
            break;
        }
        // SAFETY: the kernel wrote `len` bytes there, and `len` is bounded by the sixteen we gave.
        unsafe {
            put(console, &INDENT);
            syscall(sys::SYS_WRITE, console, start as u64, len, 0);
            put(console, &NEWLINE);
        }
        index = index.wrapping_add(1);
    }

    if index == 0 {
        // SAFETY: writing a `.user_text` static.
        unsafe { put(console, &NO_SERVICES) };
    }
}

/// Where the kernel drops us. Never returns: a shell that exits leaves nobody to talk to.
///
/// # Safety
/// Entered by `user::enter` with a console capability in `x0` and a registry capability in `x1`,
/// on a stack mapped for EL0.
#[unsafe(link_section = ".user_text")]
pub extern "C" fn main(console: u64, registry: u64) -> ! {
    // SAFETY: writing a `.user_text` static through the capability we were handed.
    unsafe { put(console, &BANNER) };

    let mut line = MaybeUninit::<[u8; LINE]>::uninit();
    let buf = line.as_mut_ptr().cast::<u8>();

    loop {
        // SAFETY: every pointer below is either a `.user_text` static or this thread's own stack.
        unsafe {
            put(console, &PROMPT);
            let len = read_line(console, buf);

            if len == 0 {
                continue;
            }
            if is(buf, len, &CMD_HELP) {
                put(console, &HELP_TEXT);
            } else if is(buf, len, &CMD_SERVICES) {
                list_services(console, registry);
            } else if is(buf, len, &CMD_UPTIME) {
                put_number(console, syscall(sys::SYS_UPTIME, 0, 0, 0, 0));
                put(console, &NEWLINE);
            } else if len >= CMD_ECHO.len() && is(buf, CMD_ECHO.len(), &CMD_ECHO) {
                // Everything after "echo", space included, handed straight back.
                let rest = CMD_ECHO.len();
                if len > rest {
                    syscall(
                        sys::SYS_WRITE,
                        console,
                        buf.add(rest.wrapping_add(1)) as u64,
                        len.wrapping_sub(rest).wrapping_sub(1) as u64,
                        0,
                    );
                }
                put(console, &NEWLINE);
            } else {
                put(console, &UNKNOWN);
            }
        }
    }
}
