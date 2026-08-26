//! The first instructions the machine executes.
//!
//! QEMU's `virt` board enters at the ELF entry point with the MMU off, caches cold and no
//! stack. Rust cannot run in that state, so this is the minimum assembly needed to make the
//! environment legal for it: park every core but one, install a stack, and zero `.bss`
//! (which occupies no space in the image, so its contents are whatever the RAM held).

use core::arch::global_asm;

global_asm!(
    r#"
.section .text.boot
.global _start
_start:
    // Only the boot core continues. The rest sleep rather than racing us through
    // initialisation with no stack of their own; they get woken deliberately once
    // there is an SMP story to wake them into.
    mrs     x0, mpidr_el1
    and     x0, x0, #0xFF
    cbnz    x0, .Lpark

    // Install the stack before touching anything that could spill to it.
    adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    // Zero .bss. Rust assumes statics start zeroed; nothing else does this for us.
    adrp    x0, __bss_start
    add     x0, x0, :lo12:__bss_start
    adrp    x1, __bss_end
    add     x1, x1, :lo12:__bss_end
.Lzero:
    cmp     x0, x1
    b.hs    .Lrust
    str     xzr, [x0], #8
    b       .Lzero

.Lrust:
    bl      kernel_main

    // kernel_main is `!`, so reaching here means something went badly wrong.
.Lpark:
    wfe
    b       .Lpark
"#
);
