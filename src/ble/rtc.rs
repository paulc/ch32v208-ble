//! RTC as the TMOS timebase.
//!
//! The library has no timer of its own on the CH32V20x: TMOS reads a free-running
//! counter through `bleClockConfig_t::getClockValue`. As in the WCH EVT
//! (`HAL/RTC.c`) the RTC counter is used, clocked from the 32.768kHz LSE with a
//! prescaler of 2, giving a 16384Hz, 32-bit counter.
//!
//! ch32-hal has no RTC driver yet, so this uses the PAC registers directly.

use ch32_hal::pac;
use ch32_hal::pac::rcc::vals::Rtcsel;
use embassy_time::{Duration, Instant};

/// RTC counter frequency (LSE / 2)
pub const RTC_FREQ_HZ: u32 = 32_768 / 2;

/// PWR control register (not in the ch32-metapac CH32V208 PAC)
const PWR_CTLR: *mut u32 = 0x4000_7000 as *mut u32;
const PWR_CTLR_DBP: u32 = 1 << 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The 32.768kHz LSE did not start
    LseNotReady,
    /// RTC register synchronisation timed out (RTC not clocked?)
    SyncTimeout,
    /// RTC write did not complete (RTC not clocked?)
    WriteTimeout,
}

/// Start the LSE and run the RTC counter from it
pub fn init() -> Result<(), Error> {
    let rcc = pac::RCC;
    let rtc = pac::RTC;

    // Enable access to the backup domain
    rcc.apb1pcenr().modify(|w| {
        w.set_pwren(true);
        w.set_bkpen(true);
    });
    unsafe { PWR_CTLR.write_volatile(PWR_CTLR_DBP | PWR_CTLR.read_volatile()) };

    // The RTC clock source can only be changed by resetting the backup domain
    let rtcsel = rcc.bdctlr().read().rtcsel();
    if rtcsel != Rtcsel::LSE && rtcsel != Rtcsel::NO_CLK {
        rcc.bdctlr().modify(|w| w.set_bdrst(true));
        rcc.bdctlr().modify(|w| w.set_bdrst(false));
    }

    rcc.bdctlr().modify(|w| w.set_lseon(true));
    wait_for(|| rcc.bdctlr().read().lserdy(), Error::LseNotReady)?;
    rcc.bdctlr().modify(|w| {
        w.set_rtcsel(Rtcsel::LSE);
        w.set_rtcen(true);
    });

    // Wait for the APB1 registers to sync with the RTC domain
    rtc.ctlrl().modify(|w| w.set_rsf(false));
    wait_for(|| rtc.ctlrl().read().rsf(), Error::SyncTimeout)?;

    // Configuration mode: prescaler = 1 (divide by 2), counter = 0
    wait_for_last_task()?;
    rtc.ctlrl().modify(|w| w.set_cnf(true));
    rtc.pscrh().write(|w| w.set_prlh(0));
    rtc.pscrl().write(|w| w.set_prll(1));
    rtc.cnth().write(|w| w.set_cnth(0));
    rtc.cntl().write(|w| w.set_cntl(0));
    rtc.ctlrl().modify(|w| w.set_cnf(false));
    wait_for_last_task()?;

    Ok(())
}

fn wait_for_last_task() -> Result<(), Error> {
    wait_for(|| pac::RTC.ctlrl().read().rtoff(), Error::WriteTimeout)
}

/// Busy-wait up to 2.5s (the EVT's LSE start-up limit) for `ready`
fn wait_for(ready: impl Fn() -> bool, error: Error) -> Result<(), Error> {
    let start = Instant::now();
    while !ready() {
        if start.elapsed() > Duration::from_millis(2500) {
            return Err(error);
        }
    }
    Ok(())
}

/// Current RTC count
pub fn now() -> u32 {
    let rtc = pac::RTC;
    loop {
        let high = rtc.cnth().read().cnth();
        let low = rtc.cntl().read().cntl();
        // Re-read the high half in case the low half rolled over in between
        if rtc.cnth().read().cnth() == high {
            return (high as u32) << 16 | low as u32;
        }
    }
}

/// `getClockValue` callback for `TMOS_TimerInit`
pub unsafe extern "C" fn get_clock_value() -> u32 {
    now()
}

/// Convert RTC ticks to microseconds
pub fn ticks_to_us(ticks: u32) -> u64 {
    ticks as u64 * 1_000_000 / RTC_FREQ_HZ as u64
}
