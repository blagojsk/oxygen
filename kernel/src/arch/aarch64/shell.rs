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
user_str!(
    HELP_TEXT,
    b"  help      this list\n  services  what is published in the registry\n  uptime    timer ticks since boot\n  echo ...  say something back\n  describe  a capability's methods; 'describe <name>' for a looked-up one\n  audit     what has happened, from the journal\n"
);
user_str!(CMD_HELP, b"help");
user_str!(CMD_SERVICES, b"services");
user_str!(CMD_UPTIME, b"uptime");
user_str!(CMD_ECHO, b"echo");
user_str!(CMD_DESCRIBE, b"describe");
user_str!(CMD_AUDIT, b"audit");
user_str!(NO_SERVICES, b"(nothing published)\n");
user_str!(NO_SUCH_SERVICE, b"no such service\n");
user_str!(INDENT, b"  ");

// `describe`'s rendering: `  <name>(<kind>, <kind>) -> <kind>`.
user_str!(OPEN_PAREN, b"(");
user_str!(COMMA_SPACE, b", ");
user_str!(CLOSE_PAREN_ARROW, b") -> ");
user_str!(KIND_NONE, b"none");
user_str!(KIND_HANDLE, b"handle");
user_str!(KIND_BYTES, b"bytes");
user_str!(KIND_NUMBER, b"number");
user_str!(KIND_RIGHTS, b"rights");

// `audit`'s rendering: `  #<seq> thread <actor> <action> kind <kind> rights <rights> detail
// <detail>`.
user_str!(EVENT_HASH, b"  #");
user_str!(EVENT_THREAD, b" thread ");
user_str!(SPACE, b" ");
user_str!(EVENT_KIND, b" kind ");
user_str!(EVENT_RIGHTS, b" rights ");
user_str!(EVENT_DETAIL, b" detail ");
user_str!(ACTION_INSERT, b"insert");
user_str!(ACTION_DELEGATE, b"delegate");
user_str!(ACTION_REVOKE, b"revoke");
user_str!(ACTION_DELETE, b"delete");
user_str!(ACTION_DENIED, b"denied");
user_str!(ACTION_REGISTER, b"register");
user_str!(ACTION_LOOKUP, b"lookup");
user_str!(ACTION_UNKNOWN, b"?");

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

/// Reads a little-endian `u32` from 4 readable bytes at `ptr`.
///
/// Built from `wrapping_mul`/`wrapping_add` rather than shifts: the byte at the highest offset is
/// the most significant, so folding from that end down — multiply what has been accumulated by
/// 256, add the next byte — reconstructs the value with no operation this file has to avoid.
///
/// # Safety
/// `ptr` must point to at least 4 readable bytes.
#[unsafe(link_section = ".user_text")]
unsafe fn read_u32_le(ptr: *const u8) -> u32 {
    let mut value = 0u32;
    let mut i = 4usize;
    loop {
        if i == 0 {
            break;
        }
        i = i.wrapping_sub(1);
        // SAFETY: `i` is in `0..4`, and `ptr` has at least 4 readable bytes.
        let byte = unsafe { core::ptr::read(ptr.add(i)) };
        value = value.wrapping_mul(256).wrapping_add(byte as u32);
    }
    value
}

/// As [`read_u32_le`], for 8 bytes.
///
/// # Safety
/// `ptr` must point to at least 8 readable bytes.
#[unsafe(link_section = ".user_text")]
unsafe fn read_u64_le(ptr: *const u8) -> u64 {
    let mut value = 0u64;
    let mut i = 8usize;
    loop {
        if i == 0 {
            break;
        }
        i = i.wrapping_sub(1);
        // SAFETY: `i` is in `0..8`, and `ptr` has at least 8 readable bytes.
        let byte = unsafe { core::ptr::read(ptr.add(i)) };
        value = value.wrapping_mul(256).wrapping_add(byte as u64);
    }
    value
}

/// Writes a zero-padded name field, up to its first zero byte or `max`, whichever comes first.
///
/// # Safety
/// `field` must point to at least `max` readable bytes, and `console` must carry `WRITE`.
#[unsafe(link_section = ".user_text")]
unsafe fn put_name_field(console: u64, field: *const u8, max: usize) {
    let mut len = 0usize;
    loop {
        if len >= max {
            break;
        }
        // SAFETY: `len` is bounded by `max`, and `field` has `max` readable bytes.
        let byte = unsafe { core::ptr::read(field.add(len)) };
        if byte == 0 {
            break;
        }
        len = len.wrapping_add(1);
    }
    // SAFETY: `field` is `len` readable bytes, `len` bounded by `max` above.
    unsafe { syscall(sys::SYS_WRITE, console, field as u64, len as u64, 0) };
}

/// Writes the name an [`oxygen_schema::ArgKind`] wire byte stands for.
///
/// # Safety
/// As [`put`].
#[unsafe(link_section = ".user_text")]
unsafe fn put_kind(console: u64, kind: u8) {
    unsafe {
        if kind == 1 {
            put(console, &KIND_HANDLE);
        } else if kind == 2 {
            put(console, &KIND_BYTES);
        } else if kind == 3 {
            put(console, &KIND_NUMBER);
        } else if kind == 4 {
            put(console, &KIND_RIGHTS);
        } else {
            put(console, &KIND_NONE);
        }
    }
}

/// Writes the name an [`oxygen_audit::Action`] wire byte stands for.
///
/// # Safety
/// As [`put`].
#[unsafe(link_section = ".user_text")]
unsafe fn put_action(console: u64, action: u8) {
    unsafe {
        if action == 0 {
            put(console, &ACTION_INSERT);
        } else if action == 1 {
            put(console, &ACTION_DELEGATE);
        } else if action == 2 {
            put(console, &ACTION_REVOKE);
        } else if action == 3 {
            put(console, &ACTION_DELETE);
        } else if action == 4 {
            put(console, &ACTION_DENIED);
        } else if action == 5 {
            put(console, &ACTION_REGISTER);
        } else if action == 6 {
            put(console, &ACTION_LOOKUP);
        } else {
            put(console, &ACTION_UNKNOWN);
        }
    }
}

/// Renders one `SYS_DESCRIBE` method encoding as `  <name>(<kind>, <kind>) -> <kind>`.
///
/// Decoded by hand from the offsets `crates/oxygen-schema/src/method.rs` documents — id `u32` at
/// 0, argument count at 4, return kind at 5, up to four argument kinds from 6, a 16-byte
/// zero-padded name from 10 — rather than calling `Method::decode`, which lives in kernel `.text`
/// and is unreachable from here.
///
/// # Safety
/// `buf` must hold at least 26 (`ENCODED_METHOD_BYTES`) readable bytes, as `SYS_DESCRIBE` wrote
/// them, and `console` must carry `WRITE`.
#[unsafe(link_section = ".user_text")]
unsafe fn put_method_line(console: u64, buf: *const u8) {
    // SAFETY: `buf` has at least 26 readable bytes.
    let arg_count = unsafe { core::ptr::read(buf.add(4)) } as usize;
    // SAFETY: as above.
    let returns = unsafe { core::ptr::read(buf.add(5)) };

    unsafe {
        put(console, &INDENT);
        put_name_field(console, buf.add(10), 16);
        put(console, &OPEN_PAREN);

        let mut i = 0usize;
        loop {
            if i >= arg_count || i >= 4 {
                break;
            }
            if i > 0 {
                put(console, &COMMA_SPACE);
            }
            // SAFETY: `i` is bounded above by 4, and `buf` has at least 10 readable bytes.
            let kind = core::ptr::read(buf.add(6).add(i));
            put_kind(console, kind);
            i = i.wrapping_add(1);
        }

        put(console, &CLOSE_PAREN_ARROW);
        put_kind(console, returns);
        put(console, &NEWLINE);
    }
}

/// Renders one `SYS_AUDIT` event encoding as
/// `  #<seq> thread <actor> <action> kind <kind> rights <rights> detail <detail>`.
///
/// Decoded by hand from the offsets `crates/oxygen-audit/src/event.rs` documents — `seq` at 0,
/// `actor` at 8, `detail` at 16, `rights` at 24, `action` at 28, `kind` at 29 — rather than calling
/// `Event::decode`, which lives in kernel `.text` and is unreachable from here.
///
/// # Safety
/// `buf` must hold at least 32 (`ENCODED_EVENT_BYTES`) readable bytes, as `SYS_AUDIT` wrote them,
/// and `console` must carry `WRITE`.
#[unsafe(link_section = ".user_text")]
unsafe fn put_event_line(console: u64, buf: *const u8) {
    // SAFETY: `buf` has at least 32 readable bytes.
    let seq = unsafe { read_u64_le(buf) };
    // SAFETY: as above.
    let actor = unsafe { read_u64_le(buf.add(8)) };
    // SAFETY: as above.
    let detail = unsafe { read_u64_le(buf.add(16)) };
    // SAFETY: as above.
    let rights = unsafe { read_u32_le(buf.add(24)) };
    // SAFETY: as above.
    let action = unsafe { core::ptr::read(buf.add(28)) };
    // SAFETY: as above.
    let kind = unsafe { core::ptr::read(buf.add(29)) };

    unsafe {
        put(console, &EVENT_HASH);
        put_number(console, seq);
        put(console, &EVENT_THREAD);
        put_number(console, actor);
        put(console, &SPACE);
        put_action(console, action);
        put(console, &EVENT_KIND);
        put_number(console, kind as u64);
        put(console, &EVENT_RIGHTS);
        put_number(console, rights as u64);
        put(console, &EVENT_DETAIL);
        put_number(console, detail);
        put(console, &NEWLINE);
    }
}

/// `describe` with no argument describes the console; `describe <name>` looks the name up first
/// and describes whatever it resolves to. Either way, every method comes back as one
/// `SYS_DESCRIBE` call per index, stopping at the first negative return.
///
/// # Safety
/// `registry` must carry `READ`; `name` must point to `name_len` readable bytes on this thread's
/// own memory (`name_len == 0` means "no argument").
#[unsafe(link_section = ".user_text")]
unsafe fn describe(console: u64, registry: u64, name: *const u8, name_len: usize) {
    let handle = if name_len == 0 {
        console
    } else {
        // SAFETY: `name` is `name_len` readable bytes on this thread's own stack.
        let looked_up =
            unsafe { syscall(sys::SYS_LOOKUP, registry, name as u64, name_len as u64, 0) };
        if (looked_up as i64) < 0 {
            // SAFETY: writing a `.user_text` static.
            unsafe { put(console, &NO_SUCH_SERVICE) };
            return;
        }
        looked_up
    };

    let mut method = MaybeUninit::<[u8; 26]>::uninit();
    let ptr = method.as_mut_ptr().cast::<u8>();
    let mut index = 0u64;
    loop {
        // SAFETY: `ptr` is 26 writable bytes on this thread's stack.
        let written = unsafe { syscall(sys::SYS_DESCRIBE, handle, index, ptr as u64, 26) };
        if (written as i64) < 0 {
            break;
        }
        // SAFETY: the kernel wrote a full method encoding there on success.
        unsafe { put_method_line(console, ptr) };
        index = index.wrapping_add(1);
    }
}

/// `audit` lists every retained journal event, oldest first.
///
/// # Safety
/// `journal` must carry `READ`.
#[unsafe(link_section = ".user_text")]
unsafe fn print_audit(console: u64, journal: u64) {
    let mut event = MaybeUninit::<[u8; 32]>::uninit();
    let ptr = event.as_mut_ptr().cast::<u8>();
    let mut index = 0u64;
    loop {
        // SAFETY: `ptr` is 32 writable bytes on this thread's stack.
        let written = unsafe { syscall(sys::SYS_AUDIT, journal, index, ptr as u64, 32) };
        if (written as i64) < 0 {
            break;
        }
        // SAFETY: the kernel wrote a full event encoding there on success.
        unsafe { put_event_line(console, ptr) };
        index = index.wrapping_add(1);
    }
}

/// Where the kernel drops us. Never returns: a shell that exits leaves nobody to talk to.
///
/// # Safety
/// Entered by `user::enter` with a console capability in `x0`, a registry capability in `x1` and
/// a journal capability in `x2`, on a stack mapped for EL0.
#[unsafe(link_section = ".user_text")]
pub extern "C" fn main(console: u64, registry: u64, journal: u64) -> ! {
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
            } else if is(buf, len, &CMD_AUDIT) {
                print_audit(console, journal);
            } else if len >= CMD_DESCRIBE.len() && is(buf, CMD_DESCRIBE.len(), &CMD_DESCRIBE) {
                // Everything after "describe ", if anything, is the name to look up first.
                let rest = CMD_DESCRIBE.len();
                if len > rest {
                    describe(
                        console,
                        registry,
                        buf.add(rest.wrapping_add(1)),
                        len.wrapping_sub(rest).wrapping_sub(1),
                    );
                } else {
                    describe(console, registry, buf, 0);
                }
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
