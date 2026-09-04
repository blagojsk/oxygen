//! PL011 UART — the kernel's only way to say anything before there are drivers.
//!
//! On QEMU's `virt` board the PL011 sits at 0x0900_0000. There is no MMU yet, so this is a
//! raw physical MMIO write; once paging exists the base address becomes a mapping rather
//! than a constant.

use core::fmt::{self, Write};

use crate::sync::SpinLock;

const UART_BASE: usize = 0x0900_0000;
const UART0: *mut u8 = UART_BASE as *mut u8;
/// Flag register: bit 4 (RXFE) is set while the receive FIFO is empty, bit 5 (TXFF) while the
/// transmit FIFO is full.
const UART_FR: *const u32 = (UART_BASE + 0x18) as *const u32;
const RXFE: u32 = 1 << 4;
const TXFF: u32 = 1 << 5;
/// Interrupt mask set/clear. Bit 4 is receive, bit 6 receive-timeout.
const UART_IMSC: *mut u32 = (UART_BASE + 0x38) as *mut u32;
/// Interrupt clear register — write-1-to-clear.
const UART_ICR: *mut u32 = (UART_BASE + 0x44) as *mut u32;
const RXIM: u32 = 1 << 4;
const RTIM: u32 = 1 << 6;

/// The PL011's line on QEMU's `virt` board: SPI 1, and the GIC numbers SPIs from 32.
pub const UART_INTID: u32 = 33;

/// Bytes typed but not yet read.
///
/// A ring rather than a single byte because a person can type faster than a thread is scheduled,
/// and because paste exists: dropping everything but the newest character turns a pasted line into
/// one character, which looks like a broken keyboard rather than a full buffer.
const INPUT_CAPACITY: usize = 256;

struct Input {
    bytes: [u8; INPUT_CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    /// Set when a byte arrives with nowhere to go, so the loss can be reported rather than
    /// silently changing what the user typed.
    overruns: u32,
}

static INPUT: SpinLock<Input> = SpinLock::new(Input {
    bytes: [0; INPUT_CAPACITY],
    head: 0,
    tail: 0,
    len: 0,
    overruns: 0,
});

pub struct Uart;

impl Uart {
    fn put(&mut self, byte: u8) {
        // SAFETY: fixed MMIO address for the board we boot on, and the kernel is
        // single-threaded at this point, so there is no concurrent writer to race.
        unsafe {
            while core::ptr::read_volatile(UART_FR) & TXFF != 0 {
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(UART0, byte);
        }
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            // A bare LF leaves the cursor mid-line on a real terminal.
            if byte == b'\n' {
                self.put(b'\r');
            }
            self.put(byte);
        }
        Ok(())
    }
}

/// Writes raw bytes, without assuming they are text.
///
/// What arrives from a syscall is whatever userspace put in the buffer. Refusing a byte for not
/// being valid UTF-8 would be the kernel imposing an encoding on a device that has none, so this
/// deliberately takes bytes where the rest of the kernel takes `str`.
pub fn write_bytes(bytes: &[u8]) {
    let mut uart = Uart;
    for &byte in bytes {
        if byte == b'\n' {
            uart.put(b'\r');
        }
        uart.put(byte);
    }
}

/// Enables receive interrupts.
///
/// Interrupt-driven rather than polled: polling the flag register costs the CPU whether or not
/// anybody is typing, and on a single-core board aimed at weak hardware that is the whole machine
/// spent waiting for a keystroke.
///
/// # Safety
/// Touches board MMIO and enables a GIC line; the caller must have brought up the GIC.
pub unsafe fn init_rx() {
    // SAFETY: fixed MMIO for this board. RTIM matters as much as RXIM: without the timeout, a few
    // characters that never fill the FIFO would sit there until somebody typed enough more.
    unsafe {
        core::ptr::write_volatile(UART_ICR, 0x7FF);
        core::ptr::write_volatile(UART_IMSC, RXIM | RTIM);
        super::gic::enable(UART_INTID);
    }
}

/// Drains the receive FIFO into the input ring, then wakes anyone waiting for a keystroke.
///
/// Called from the IRQ handler. The lock is released before the wake, because waking takes the
/// scheduler's lock and no path may hold two.
pub fn handle_irq() {
    let mut arrived = false;
    let mut first_overrun = false;
    {
        let mut input = INPUT.lock();
        loop {
            // SAFETY: reading the flag and data registers of the board's UART.
            let empty = unsafe { core::ptr::read_volatile(UART_FR) } & RXFE != 0;
            if empty {
                break;
            }
            // SAFETY: the FIFO is non-empty, so there is a byte to take.
            let byte = unsafe { core::ptr::read_volatile(UART0 as *const u8) };
            first_overrun |= input.push(byte);
            arrived = true;
        }
        // SAFETY: write-1-to-clear on the interrupt clear register.
        unsafe { core::ptr::write_volatile(UART_ICR, RXIM | RTIM) };
    }
    // Said once, outside the lock, and never again. Input that vanished without a word is a
    // keyboard that looks broken; the same warning on every dropped byte is worse than silence.
    if first_overrun {
        crate::println!("  [uart] input buffer full — characters are being dropped");
    }
    if arrived {
        crate::sched::wake_all_on(crate::sched::WAIT_CONSOLE);
    }
}

impl Input {
    /// Returns whether this is the first byte to be dropped, so the loss can be reported once.
    fn push(&mut self, byte: u8) -> bool {
        if self.len == INPUT_CAPACITY {
            self.overruns = self.overruns.saturating_add(1);
            return self.overruns == 1;
        }
        self.bytes[self.tail] = byte;
        self.tail = (self.tail + 1) % INPUT_CAPACITY;
        self.len += 1;
        false
    }

    fn pop(&mut self) -> Option<u8> {
        if self.len == 0 {
            return None;
        }
        let byte = self.bytes[self.head];
        self.head = (self.head + 1) % INPUT_CAPACITY;
        self.len -= 1;
        Some(byte)
    }
}

/// Takes one typed byte if there is one. Never blocks.
pub fn read_byte() -> Option<u8> {
    INPUT.lock().pop()
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = Uart.write_fmt(args);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::arch::target::uart::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
