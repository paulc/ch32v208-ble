/* Linker script for ch32v208-ble: qingke-rt 0.8 `link-no-highcode.x` (used as
 * `link.x` without qingke-rt's `highcode` feature) with two changes, marked
 * "ch32v208-ble", for the WCH BLE library (vendor/libwchble.a):
 *
 * 1. .highcode is placed first in flash, before .text, as in WCH's Link.ld.
 *    The library marks its time-critical code .highcode (BB/LLE interrupt
 *    paths, RX/TX waits, connection events). Only the first 128KB of flash
 *    (with 64KB RAM) is zero-wait; qingke-rt puts .highcode at the end of
 *    .text, which in a larger image is slow flash, where the controller
 *    missed the 150us reply window (connectable advertising sent no scan
 *    responses).
 *
 * 2. COMMON symbols are placed in .bss. The library keeps its state in C
 *    common symbols; qingke-rt does not list COMMON, so lld put them after
 *    _ebss, where they were not zeroed at reset and kept stale values across a
 *    warm reset.
 *
 * Used via build.rs (`-Tble-link.x` instead of `-Tlink.x`). Keep in sync with
 * qingke-rt when updating it.
 */

INCLUDE memory.x
/* Provides weak aliases (cf. PROVIDED) for device specific interrupt handlers */
/* This will usually be provided by a device crate generated using svd2rust (see `device.x`) */
INCLUDE device.x

PROVIDE(_stext = ORIGIN(REGION_TEXT));
PROVIDE(_stack_start = ORIGIN(REGION_STACK) + LENGTH(REGION_STACK));
PROVIDE(_max_hart_id = 0);
PROVIDE(_hart_stack_size = 2K);
PROVIDE(_heap_size = 0);

/* fault handlers */
PROVIDE(InstructionMisaligned = ExceptionHandler);
PROVIDE(InstructionFault = ExceptionHandler);
PROVIDE(IllegalInstruction = ExceptionHandler);
PROVIDE(Breakpoint = ExceptionHandler);
PROVIDE(LoadMisaligned = ExceptionHandler);
PROVIDE(LoadFault = ExceptionHandler);
PROVIDE(StoreMisaligned = ExceptionHandler);
PROVIDE(StoreFault = ExceptionHandler);;
PROVIDE(UserEnvCall = ExceptionHandler);
PROVIDE(MachineEnvCall = ExceptionHandler);

/* core interrupt handlers */
PROVIDE(NonMaskableInt = DefaultHandler);
PROVIDE(SysTick = DefaultHandler);
PROVIDE(Software = DefaultHandler);

PROVIDE(Exception = _exception_handler);

PROVIDE(DefaultHandler = DefaultInterruptHandler);
PROVIDE(ExceptionHandler = DefaultExceptionHandler);

/* # Interrupt vectors */
EXTERN(__CORE_INTERRUPTS);
EXTERN(__EXTERNAL_INTERRUPTS); /* `static` variable similar to `__EXCEPTIONS` */

ENTRY(_start)

SECTIONS
{
    .vector_table ORIGIN(FLASH) :
    {
        KEEP(*(SORT_NONE(.init)))
        . = ALIGN(4);
        /* core interrupts table's first entry is omitted, occupied by the init jump instruction */
        KEEP(*(.vector_table.core_interrupts));
        KEEP(*(.vector_table.external_interrupts));
        KEEP(*(.vector_table.exceptions));
        *(.trap .trap.rust)
    } >FLASH AT>FLASH

    /* ch32v208-ble: .highcode first, in zero-wait flash (was at the end of .text) */
    .highcode : ALIGN(4)
    {
        *(.highcode .highcode.*);
    } >FLASH AT>FLASH

    .text : ALIGN(4)
    {
        . = ALIGN(4);
        KEEP(*(SORT_NONE(.handle_reset)))
        *(.init.rust)
        *(.text .text.*)
    } >FLASH AT>FLASH

    .rodata : ALIGN(4)
    {
        *(.srodata .srodata.*);
        *(.rodata .rodata.*);
        . = ALIGN(4);
    } >FLASH AT>FLASH

    .data : ALIGN(4)
    {
        _data_lma = LOADADDR(.data);
        PROVIDE(_data_vma = .);
        PROVIDE( __global_pointer$ = . + 0x800 );
        *(.sdata .sdata.* .sdata2 .sdata2.*);
        *(.data .data.*);
        . = ALIGN(4);
        PROVIDE( _edata = .);
    } >RAM AT>FLASH

    .bss (NOLOAD) : ALIGN(4)
    {
        PROVIDE( _sbss = .);
        *(.sbss .sbss.* .bss .bss.*);
        /* ch32v208-ble: library C common symbols, zeroed at reset with .bss */
        *(COMMON);
        PROVIDE( _ebss = .);
    } >RAM

    .uninit (NOLOAD) : ALIGN(4)
    {
        *(.uninit .uninit.*);
    } >RAM

    /* Align `__sheap` and `_stack_end` pointers to 4 bytes */
    . = ALIGN(4);

    /* Place the heap start and stack end at the end of allocated RAM */
    PROVIDE(__sheap = .);
    PROVIDE(_stack_end = .);

    .got (INFO) :
    {
        KEEP(*(.got .got.*));
    }

    .eh_frame (INFO) : { KEEP(*(.eh_frame)) }
    .eh_frame_hdr (INFO) : { *(.eh_frame_hdr) }
}

INCLUDE assert-align.x
