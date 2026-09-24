MEMORY {
    /*
     * Pico 2 W has 4 MiB of flash. We cap the code region here and reserve the
     * top 64 KiB (0x103F0000..) for the config store in a later phase.
     */
    FLASH : ORIGIN = 0x10000000, LENGTH = 4032K

    /* RAM: 8 striped banks (SRAM0-7) for performance. */
    RAM : ORIGIN = 0x20000000, LENGTH = 512K

    /* Direct-mapped banks for dedicated use (e.g. per-core stacks). */
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    /* Boot ROM info: keep in the first 4K of flash where the Boot ROM / picotool look. */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH

} INSERT AFTER .vector_table;

/* Keep .text aligned after the variable-size boot metadata. */
_stext = ALIGN(ADDR(.start_block) + SIZEOF(.start_block), 8);

SECTIONS {
    /* Picotool 'Binary Info' entries. */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* Boot ROM extra info / signature. */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
