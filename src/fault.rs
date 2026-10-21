//! Fault reporting for bring-up: replaces qingke-rt's silent exception and
//! default interrupt loops with handlers that print over SDI.
//!
//! Use [`fault_handlers!`](crate::fault_handlers) once in the application.

use core::sync::atomic::{AtomicU32, Ordering};

/// `ra`/`sp` at the time of the last exception (saved by the `Exception` vector)
#[unsafe(no_mangle)]
pub static FAULT_RA: AtomicU32 = AtomicU32::new(0);
#[unsafe(no_mangle)]
pub static FAULT_SP: AtomicU32 = AtomicU32::new(0);

/// Print the exception state and BLE stats
pub fn report_exception() {
    let (mcause, mepc, mtval): (usize, usize, usize);
    unsafe {
        core::arch::asm!("csrr {}, mcause", "csrr {}, mepc", "csrr {}, mtval",
            out(reg) mcause, out(reg) mepc, out(reg) mtval);
    }
    ch32_hal::println!(
        "\n!! exception mcause={:#x} mepc={:#x} mtval={:#x} ra={:#x} sp={:#x}",
        mcause,
        mepc,
        mtval,
        FAULT_RA.load(Ordering::Relaxed),
        FAULT_SP.load(Ordering::Relaxed)
    );
    ch32_hal::println!("!! {}", crate::ble::STATS);
}

/// Print the unexpected interrupt
pub fn report_interrupt() {
    let mcause: usize;
    unsafe { core::arch::asm!("csrr {}, mcause", out(reg) mcause) };
    ch32_hal::println!("\n!! unhandled interrupt mcause={:#x}", mcause);
}

/// Define `Exception` (core vector 3, saving `ra`/`sp` first), `ExceptionHandler`
/// and `DefaultHandler`, overriding qingke-rt's defaults.
#[macro_export]
macro_rules! fault_handlers {
    () => {
        core::arch::global_asm!(
            r#"
            .section .text.fault_exception,"ax",@progbits
            .global Exception
            .align 2
        Exception:
            la t0, FAULT_RA
            sw ra, 0(t0)
            la t0, FAULT_SP
            sw sp, 0(t0)
            j ExceptionHandler
            "#
        );

        #[unsafe(no_mangle)]
        extern "C" fn ExceptionHandler() -> ! {
            $crate::fault::report_exception();
            loop {}
        }

        #[unsafe(no_mangle)]
        extern "C" fn DefaultHandler() -> ! {
            $crate::fault::report_interrupt();
            loop {}
        }
    };
}
