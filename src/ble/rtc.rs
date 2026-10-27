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
    /// The RTC enable bit (`RCC_BDCTLR.RTCEN`) did not stay set
    EnableTimeout,
    /// RTC register synchronisation timed out (RTC not clocked?)
    SyncTimeout,
    /// RTC write did not complete (RTC not clocked?)
    WriteTimeout,
}

/// Start the LSE and run the RTC counter from it.
///
/// The RTC lives in the backup domain, which keeps its state across resets. If
/// it does not respond (e.g. left mid-configuration), the backup domain is reset
/// and the start-up retried once.
pub fn init() -> Result<(), Error> {
    let rcc = pac::RCC;

    // Enable access to the backup domain
    rcc.apb1pcenr().modify(|w| {
        w.set_pwren(true);
        w.set_bkpen(true);
    });
    unsafe { PWR_CTLR.write_volatile(PWR_CTLR_DBP | PWR_CTLR.read_volatile()) };

    // The RTC clock source can only be changed by resetting the backup domain
    let rtcsel = rcc.bdctlr().read().rtcsel();
    if rtcsel != Rtcsel::LSE && rtcsel != Rtcsel::NO_CLK {
        reset_backup_domain();
    }

    match start() {
        Err(Error::SyncTimeout | Error::WriteTimeout) => {
            reset_backup_domain();
            start()
        }
        result => result,
    }
}

fn reset_backup_domain() {
    pac::RCC.bdctlr().modify(|w| w.set_bdrst(true));
    pac::RCC.bdctlr().modify(|w| w.set_bdrst(false));
}

fn start() -> Result<(), Error> {
    let rcc = pac::RCC;
    let rtc = pac::RTC;

    rcc.bdctlr().modify(|w| w.set_lseon(true));
    wait_for(|| rcc.bdctlr().read().lserdy(), Error::LseNotReady)?;
    // Select the clock and enable the RTC in separate writes, as the EVT does
    // (`RCC_RTCCLKConfig`, then `RCC_RTCCLKCmd`). After a power-on reset, setting
    // both in one write left RTCEN clear (RTC unclocked, so the sync below never
    // completed). Rewrite RTCEN until it reads back set.
    rcc.bdctlr().modify(|w| w.set_rtcsel(Rtcsel::LSE));
    wait_for(
        || {
            rcc.bdctlr().modify(|w| w.set_rtcen(true));
            rcc.bdctlr().read().rtcen()
        },
        Error::EnableTimeout,
    )?;

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

/// EXTI line connected to the RTC alarm
const EXTI_LINE_RTC_ALARM: usize = 17;

/// Route the RTC alarm to the `RTC_ALARM` interrupt (via EXTI line 17, rising edge)
pub fn enable_alarm_interrupt() {
    let exti = pac::EXTI;
    exti.rtenr().modify(|w| w.set_tr(EXTI_LINE_RTC_ALARM, true));
    exti.intenr().modify(|w| w.set_mr(EXTI_LINE_RTC_ALARM, true));
    pac::RTC.ctlrh().modify(|w| w.set_alrie(true));
}

/// Set the alarm to fire when the counter reaches `time`.
///
/// Called from the alarm interrupt: waits (a few RTC clocks) for the RTC
/// write to complete rather than timing out.
pub fn set_alarm(time: u32) {
    let rtc = pac::RTC;
    while !rtc.ctlrl().read().rtoff() {}
    rtc.ctlrl().modify(|w| w.set_cnf(true));
    rtc.alrmh().write(|w| w.set_alrh((time >> 16) as u16));
    rtc.alrml().write(|w| w.set_alrl(time as u16));
    rtc.ctlrl().modify(|w| w.set_cnf(false));
    while !rtc.ctlrl().read().rtoff() {}
}

/// Clear the alarm flag and its EXTI pending bit
pub fn clear_alarm() {
    pac::RTC.ctlrl().modify(|w| w.set_alrf(false));
    pac::EXTI.intfr().write(|w| w.set_if_(EXTI_LINE_RTC_ALARM, true));
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
pub extern "C" fn get_clock_value() -> u32 {
    now()
}
