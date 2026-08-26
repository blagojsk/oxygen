/*
 * QEMU's `virt` board places RAM at 0x4000_0000 and, given an ELF via -kernel, honours the
 * addresses below and jumps to ENTRY. Keeping .text.boot first guarantees _start sits at the
 * load address, so the entry point is where the hardware expects it regardless of how the
 * linker orders the rest.
 */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) { *(.rodata .rodata.*) }
    .data   : ALIGN(8) { *(.data .data.*) }

    /*
     * .bss is not present in the ELF image, so the boot code zeroes it by hand.
     * 16-byte alignment keeps the AArch64 stack-pointer alignment requirement satisfiable.
     */
    .bss : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    . = ALIGN(16);
    . += 64K;
    __stack_top = .;

    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame*) }
}
