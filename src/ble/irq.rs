//! BLE interrupt glue.
//!
//! `BB`: an ordinary interrupt handler calling into the library.
//!
//! `LLE`: ported from `vendor/ble_task_scheduler.S`. The library's LLE handler
//! may defer work to thread level: it stores the interrupted `mepc` in slot 0 of
//! the register frame saved here, sets `mepc` to an internal
//! `ble_controller_scheduler` and `mret`s into it. That function runs the
//! controller work with interrupts enabled and then executes `ecall`. The ecall
//! handler (`MachineEnvCall`, core vector 5) restores the original `mepc` and
//! registers from the same frame and returns to the interrupted code.
//!
//! Because the library relies on `sp` pointing at this frame, `LLE_IRQLibHandler`
//! must be called directly from the assembly below (not through a Rust wrapper).
//!
//! The only change from the WCH assembly is a call to `ble_irq_wake` before the
//! library handler, so the TMOS task is woken (see `super::run`).

use super::ffi;

/// Address of `LLE_IRQLibHandler`, used by the `LLE` vector (required by the library)
#[unsafe(no_mangle)]
pub static mut g_LLE_IRQLibHandlerLocation: u32 = 0;

core::arch::global_asm!(
    r#"
    .section .text.ble_ecall_handler,"ax",@progbits
    .global MachineEnvCall
    .align 2
MachineEnvCall:
    addi a1, x0, 0x20
    csrs 0x804, a1

    lw a1, 0 * 4( sp )
    csrw mepc, a1

    lw x1, 1 * 4( sp )
    lw x4, 2 * 4( sp )
    lw x5, 3 * 4( sp )
    lw x6, 4 * 4( sp )
    lw x7, 5 * 4( sp )
    lw x8, 6 * 4( sp )
    lw x9, 7 * 4( sp )
    lw x10, 8 * 4( sp )
    lw x11, 9 * 4( sp )
    lw x12, 10 * 4( sp )
    lw x13, 11 * 4( sp )
    lw x14, 12 * 4( sp )
    lw x15, 13 * 4( sp )
    lw x16, 14 * 4( sp )
    lw x17, 15 * 4( sp )
    lw x18, 16 * 4( sp )
    lw x19, 17 * 4( sp )
    lw x20, 18 * 4( sp )
    lw x21, 19 * 4( sp )
    lw x22, 20 * 4( sp )
    lw x23, 21 * 4( sp )
    lw x24, 22 * 4( sp )
    lw x25, 23 * 4( sp )
    lw x26, 24 * 4( sp )
    lw x27, 25 * 4( sp )
    lw x28, 26 * 4( sp )
    lw x29, 27 * 4( sp )
    lw x30, 28 * 4( sp )
    lw x31, 29 * 4( sp )

    addi sp, sp, 32*4

    mret

    .section .text.ble_lle_handler,"ax",@progbits
    .global LLE
    .align 2
LLE:
    addi sp, sp, -32*4

    sw x1, 1 * 4( sp )
    sw x4, 2 * 4( sp )
    sw x5, 3 * 4( sp )
    sw x6, 4 * 4( sp )
    sw x7, 5 * 4( sp )
    sw x8, 6 * 4( sp )
    sw x9, 7 * 4( sp )
    sw x10, 8 * 4( sp )
    sw x11, 9 * 4( sp )
    sw x12, 10 * 4( sp )
    sw x13, 11 * 4( sp )
    sw x14, 12 * 4( sp )
    sw x15, 13 * 4( sp )
    sw x16, 14 * 4( sp )
    sw x17, 15 * 4( sp )
    sw x18, 16 * 4( sp )
    sw x19, 17 * 4( sp )
    sw x20, 18 * 4( sp )
    sw x21, 19 * 4( sp )
    sw x22, 20 * 4( sp )
    sw x23, 21 * 4( sp )
    sw x24, 22 * 4( sp )
    sw x25, 23 * 4( sp )
    sw x26, 24 * 4( sp )
    sw x27, 25 * 4( sp )
    sw x28, 26 * 4( sp )
    sw x29, 27 * 4( sp )
    sw x30, 28 * 4( sp )
    sw x31, 29 * 4( sp )

    call ble_irq_wake

    la a1, g_LLE_IRQLibHandlerLocation
    lw a0, 0(a1)
    jalr x1, 0(a0)

    lw x1, 1 * 4( sp )
    lw x4, 2 * 4( sp )
    lw x5, 3 * 4( sp )
    lw x6, 4 * 4( sp )
    lw x7, 5 * 4( sp )
    lw x8, 6 * 4( sp )
    lw x9, 7 * 4( sp )
    lw x10, 8 * 4( sp )
    lw x11, 9 * 4( sp )
    lw x12, 10 * 4( sp )
    lw x13, 11 * 4( sp )
    lw x14, 12 * 4( sp )
    lw x15, 13 * 4( sp )
    lw x16, 14 * 4( sp )
    lw x17, 15 * 4( sp )
    lw x18, 16 * 4( sp )
    lw x19, 17 * 4( sp )
    lw x20, 18 * 4( sp )
    lw x21, 19 * 4( sp )
    lw x22, 20 * 4( sp )
    lw x23, 21 * 4( sp )
    lw x24, 22 * 4( sp )
    lw x25, 23 * 4( sp )
    lw x26, 24 * 4( sp )
    lw x27, 25 * 4( sp )
    lw x28, 26 * 4( sp )
    lw x29, 27 * 4( sp )
    lw x30, 28 * 4( sp )
    lw x31, 29 * 4( sp )

    addi sp, sp, 32*4

    mret
    "#
);

/// Called from the `LLE` vector before the library handler
#[unsafe(no_mangle)]
extern "C" fn ble_irq_wake() {
    super::count(&super::STATS.lle_irqs);
    super::wake();
}

#[qingke_rt::interrupt]
fn BB() {
    super::count(&super::STATS.bb_irqs);
    unsafe { ffi::BB_IRQLibHandler() };
    super::wake();
}

/// Point the `LLE` vector at the library handler (checked by `BLE_LibInit`)
pub(super) unsafe fn set_lle_handler() {
    unsafe { g_LLE_IRQLibHandlerLocation = ffi::LLE_IRQLibHandler as *const () as u32 };
}

/// Enable the BB/LLE interrupts
pub(super) unsafe fn enable() {
    use ch32_hal::interrupt::{Interrupt, InterruptExt};

    unsafe {
        Interrupt::BB.enable();
        Interrupt::LLE.enable();
    }
}
