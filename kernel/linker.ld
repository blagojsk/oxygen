/*
 * QEMU's `virt` board places RAM at 0x4000_0000 and, given an ELF via -kernel, honours the
 * addresses below and jumps to ENTRY. Keeping .text.boot first guarantees _start sits at the
 * load address, so the entry point is where the hardware expects it.
 *
 * Every section is aligned to 4 KiB and bracketed by symbols. That is not tidiness: it is what
 * lets the MMU map each one with the permissions it actually needs. A page is the smallest unit
 * of protection the hardware offers, so two sections sharing a page must share permissions —
 * which in practice means the stricter one loses.
 */
ENTRY(_start)

SECTIONS
{
    . = 0x40000000;
    __kernel_start = .;

    /* Executable, never writable. */
    .text : ALIGN(4096) {
        __text_start = .;
        KEEP(*(.text.boot))
        *(.text .text.*)
        . = ALIGN(4096);
        __text_end = .;
    }

    /* Readable only: constants and the exception vector table's data, if any. */
    .rodata : ALIGN(4096) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        . = ALIGN(4096);
        __rodata_end = .;
    }

    /* Writable, never executable: initialised data, zeroed data, page tables and the stack. */
    .data : ALIGN(4096) {
        __data_start = .;
        *(.data .data.*)
        . = ALIGN(4096);
    }

    .bss : ALIGN(4096) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    }

    /*
     * 64 KiB of stack. Modest on purpose: this OS is meant for machines where a megabyte matters,
     * and a kernel that needs more stack than this usually has a recursion bug rather than a
     * genuine requirement.
     */
    . = ALIGN(16);
    . += 64K;
    __stack_top = .;

    . = ALIGN(4096);
    __data_end = .;
    __kernel_end = .;

    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame*) }
}
