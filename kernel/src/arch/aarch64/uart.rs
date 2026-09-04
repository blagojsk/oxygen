//! PL011 UART — the kernel's only way to say anything before there are drivers.
//!
//! On QEMU's `virt` board the PL011 sits at 0x0900_0000. There is no MMU yet, so this is a
//! raw physical MMIO write; once paging exists the base address becomes a mapping rather
//! than a constant.

use core::fmt::{self, Write};

const UART0: *mut u8 = 0x0900_0000 as *mut u8;
/// Flag register: bit 5 (TXFF) is set while the transmit FIFO is full.
const UART_FR: *const u32 = 0x0900_0018 as *const u32;
const TXFF: u32 = 1 << 5;

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
