//! BLE for the CH32V208 using the WCH BLE library (`vendor/libwchble.a`).
//!
//! The library contains the BLE controller and host, scheduled by WCH's TMOS
//! (a cooperative event/task scheduler). The application drives it by calling
//! `TMOS_SystemProcess()` repeatedly. Here that is done by [`run`], spawned as an
//! embassy task: it calls TMOS and yields, in a loop, so other tasks run between
//! passes. (The EVT without sleep does the same from `main`.)
//!
//! The library's idle callback (`idleCB`) is not used: around it the library
//! powers down part of the RF (RFEND), with interrupts enabled, which races with
//! the interrupt-driven controller below (connectable advertising stalled).
//! The executor spins anyway; low power needs a proper sleep integration later.
//!
//! The controller (link layer) runs interrupt-driven: `bleClockConfig_t::irqEnable`
//! is set (the header marks it reserved, and the WCH EVT leaves it 0). With 0 the
//! link layer is polled from `TMOS_SystemProcess`, so replies such as scan
//! responses and connection-event data depend on TMOS being called back-to-back,
//! as the EVT main loop does. With 1 the LLE interrupt defers the link-layer work
//! to thread level straight away (the ecall mechanism in `irq`), independent of
//! how often [`run`] gets to call TMOS. In this mode connection events are
//! timed from the RTC alarm interrupt (see `irq`).
//!
//! The library is not re-entrant: call it from thread level only, i.e. from
//! embassy tasks or TMOS callbacks. All tasks share one cooperative executor,
//! so calls never overlap (the controller's interrupt-deferred work is designed
//! to preempt thread-level library code).
//!
//! Setup mirrors `HAL/MCU.c` (`WCHBLE_Init`, `HAL_Init`) and `HAL/RTC.c` from the
//! CH32V20x EVT (openwch/ch32v20x `EVT/EXAM/BLE`).

pub mod adv;
pub mod ffi;
pub mod gap;
pub mod gatt;
pub mod gatt_server;
pub mod peripheral;
mod irq;
pub mod rtc;

use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::num::NonZeroU8;
use core::sync::atomic::{AtomicU32, Ordering};

use ch32_hal::pac;
use embassy_futures::yield_now;
use embassy_time::Instant;

use self::ffi::*;

/// Library memory (BLE_MEMHEAP_SIZE in the EVT)
const HEAP_SIZE: usize = 7 * 1024;

#[repr(C, align(4))]
struct BleHeap([MaybeUninit<u8>; HEAP_SIZE]);
static mut BLE_HEAP: BleHeap = BleHeap([MaybeUninit::uninit(); HEAP_SIZE]);

// Library buffer configuration (EVT defaults)
const BLE_BUFF_MAX_LEN: u16 = 27;
const BLE_BUFF_NUM: u8 = 5;
const BLE_TX_NUM_EVENT: u8 = 1;

/// HAL task event: periodic RF calibration
const HAL_REG_INIT_EVENT: tmosEvents = 0x2000;
const BLE_CALIBRATION_PERIOD_MS: u32 = 120_000;

/// OSC registers (not in the ch32-metapac CH32V208 PAC)
const OSC_HSE_CAL_CTRL: *mut u32 = 0x4002_202C as *mut u32;


/// Event counters, for diagnostics
pub struct Stats {
    /// `TMOS_SystemProcess` calls
    pub tmos_passes: AtomicU32,
    pub bb_irqs: AtomicU32,
    /// RTC alarm interrupts (connection event timing)
    pub rtc_irqs: AtomicU32,
    // LLE interrupts and deferred (ecall) returns are counted in assembly:
    // see `irq::LLE_IRQS` and `irq::LLE_DEFERRED`
}

pub static STATS: Stats = Stats {
    tmos_passes: AtomicU32::new(0),
    bb_irqs: AtomicU32::new(0),
    rtc_irqs: AtomicU32::new(0),
};

/// Increment a counter. No atomic RMW on riscv32imc, but each counter has a single writer.
fn count(counter: &AtomicU32) {
    counter.store(counter.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
}

impl core::fmt::Display for Stats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "tmos={} bb={} lle={} deferred={} alarm={} rtc={}",
            self.tmos_passes.load(Ordering::Relaxed),
            self.bb_irqs.load(Ordering::Relaxed),
            irq::LLE_IRQS.load(Ordering::Relaxed),
            irq::LLE_DEFERRED.load(Ordering::Relaxed),
            self.rtc_irqs.load(Ordering::Relaxed),
            rtc::now()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// `VER_LIB` does not match the version the bindings were written against
    LibVersion,
    /// LSE/RTC start-up failed
    Rtc(rtc::Error),
    /// `BLE_LibInit` failed, see `ffi::ERR_*`
    LibInit(NonZeroU8),
    /// `TMOS_TimerInit` failed
    TimerInit(NonZeroU8),
    /// `TMOS_ProcessEventRegister` failed
    TaskRegister,
}

pub struct Config {
    pub mac_addr: MacAddress,
    /// `ffi::LL_TX_POWEER_*`
    pub tx_power: u8,
    /// Maximum connections as peripheral (0-3)
    pub peripheral_connections: u8,
    /// Maximum connections as central. With the interrupt-driven controller the
    /// library allows at most 2 connections in total.
    pub central_connections: u8,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mac_addr: MacAddress::factory(),
            tx_power: LL_TX_POWEER_0_DBM,
            peripheral_connections: 1,
            central_connections: 1,
        }
    }
}

/// Initialise the library, the TMOS timebase (RTC/LSE) and the BLE interrupts.
///
/// Must be called after `ch32_hal::init` (which starts the embassy time driver
/// used for the LSE start-up timeout). Call the GAP role init functions after
/// this, then spawn [`run`].
pub fn init(config: Config) -> Result<(), Error> {
    #[cfg(feature = "highcode")]
    zero_common_symbols();

    if !lib_version().starts_with(VER_FILE) {
        return Err(Error::LibVersion);
    }

    // The library uses the CRC peripheral
    pac::RCC.ahbpcenr().modify(|w| w.set_crcen(true));

    // 32MHz crystal capacitance and current (from WCHBLE_Init)
    unsafe {
        let mut v = OSC_HSE_CAL_CTRL.read_volatile();
        v &= !(0x07 << 28);
        v |= 0x03 << 28;
        v |= 3 << 24;
        OSC_HSE_CAL_CTRL.write_volatile(v);
    }

    rtc::init().map_err(Error::Rtc)?;
    rtc::enable_alarm_interrupt();

    let mut cfg: bleConfig_t = unsafe { core::mem::zeroed() };
    cfg.MEMAddr = &raw mut BLE_HEAP as u32;
    cfg.MEMLen = HEAP_SIZE as u16;
    cfg.BufMaxLen = BLE_BUFF_MAX_LEN;
    cfg.BufNumber = BLE_BUFF_NUM;
    cfg.TxNumEvent = BLE_TX_NUM_EVENT;
    cfg.TxPower = config.tx_power;
    // No SNV (SNVAddr, SNVNum, readFlashCB, writeFlashCB): bonding info is not saved
    cfg.ClockFrequency = rtc::RTC_FREQ_HZ as u16;
    cfg.ClockAccuracy = 50; // LSE
    cfg.ConnectNumber = (config.peripheral_connections & 3) | (config.central_connections << 2);
    cfg.MacAddr = config.mac_addr.0;
    cfg.srandCB = Some(srand);
    // No idleCB: see the module docs
    // No temperature sampling (tsCB) or LSI calibration (rcCB) - using LSE

    unsafe {
        irq::set_lle_handler();
        BLE_LibInit(&cfg).map_err(Error::LibInit)?;
        irq::enable();

        // HAL_Init: HAL task + TMOS timer + periodic RF calibration
        let hal_task = TMOS_ProcessEventRegister(Some(hal_process_event));
        if hal_task == INVALID_TASK_ID {
            return Err(Error::TaskRegister);
        }

        let clock = bleClockConfig_t {
            getClockValue: Some(rtc::get_clock_value),
            ClockMaxCount: u32::MAX,
            ClockFrequency: rtc::RTC_FREQ_HZ as u16,
            ClockAccuracy: 100, // LSE
            // Interrupt-driven controller: see the module docs
            irqEnable: 1,
        };
        // Fails if irqEnable is set and there are more than 2 connections in total
        TMOS_TimerInit(&clock).map_err(Error::TimerInit)?;

        tmos_start_task(
            hal_task,
            HAL_REG_INIT_EVENT,
            MS1_TO_SYSTEM_TIME(BLE_CALIBRATION_PERIOD_MS),
        );
    }

    Ok(())
}

/// With the `highcode` feature, zero the library's C common symbols (see
/// build.rs, `ble_common.x`): they are outside the range zeroed at reset, so
/// would otherwise keep values from before a (warm) reset. (By default
/// ble-link.x places them in .bss.)
#[cfg(feature = "highcode")]
fn zero_common_symbols() {
    unsafe extern "C" {
        static mut __ble_common_start: u8;
        static mut __ble_common_end: u8;
    }
    unsafe {
        let start = &raw mut __ble_common_start;
        let len = (&raw mut __ble_common_end as usize) - (start as usize);
        core::ptr::write_bytes(start, 0, len);
    }
}

/// Run TMOS forever, yielding to other tasks between passes. Spawn this in an
/// embassy task after [`init`].
pub async fn run() -> ! {
    loop {
        unsafe { TMOS_SystemProcess() };
        count(&STATS.tmos_passes);
        yield_now().await;
    }
}

/// Library version, e.g. "CH32V20x_BLE_LIB_V1.4"
pub fn lib_version() -> &'static str {
    unsafe { CStr::from_ptr(VER_LIB.as_ptr().cast()) }.to_str().unwrap_or("?")
}

/// `srandCB`: random seed
unsafe extern "C" fn srand() -> u32 {
    Instant::now().as_ticks() as u32
}

/// HAL TMOS task (HAL_ProcessEvent in the EVT)
unsafe extern "C" fn hal_process_event(task_id: tmosTaskID, events: tmosEvents) -> tmosEvents {
    unsafe {
        if events & SYS_EVENT_MSG != 0 {
            let msg = tmos_msg_receive(task_id);
            if !msg.is_null() {
                let _ = tmos_msg_deallocate(msg);
            }
            return events ^ SYS_EVENT_MSG;
        }
        if events & HAL_REG_INIT_EVENT != 0 {
            // Calibrate RF
            BLE_RegInit();
            tmos_start_task(task_id, HAL_REG_INIT_EVENT, MS1_TO_SYSTEM_TIME(BLE_CALIBRATION_PERIOD_MS));
            return events ^ HAL_REG_INIT_EVENT;
        }
    }
    0
}

/// Library format MAC address, LSB first
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddress(pub [u8; 6]);

impl MacAddress {
    /// Factory MAC address from the chip's ESIG area (FLASH_GetMACAddress)
    pub fn factory() -> Self {
        let lo = unsafe { (0x1FFF_F7E8 as *const u32).read_volatile() }.to_le_bytes();
        let hi = unsafe { (0x1FFF_F7EC as *const u32).read_volatile() }.to_le_bytes();
        MacAddress([lo[0], lo[1], lo[2], lo[3], hi[0], hi[1]])
    }

    /// Pseudo-random address, for testing (e.g. to avoid a scanner's cached name).
    ///
    /// Mixes the factory MAC with the RTC counter (which keeps running across
    /// resets until [`init`] restarts it) and SysTick. Not cryptographic. The top
    /// two bits are set as for a static random address, but the library still
    /// uses it as its (public) device address.
    pub fn random() -> Self {
        let factory = Self::factory().0;
        let mut seed = u32::from_le_bytes([factory[0], factory[1], factory[2], factory[3]])
            ^ (u32::from(factory[4]) << 8 | u32::from(factory[5]))
            ^ rtc::now()
            ^ (pac::SYSTICK.cnt().read() as u32);
        let mut next = || {
            // splitmix32
            seed = seed.wrapping_add(0x9E37_79B9);
            let mut z = seed;
            z = (z ^ (z >> 16)).wrapping_mul(0x85EB_CA6B);
            z = (z ^ (z >> 13)).wrapping_mul(0xC2B2_AE35);
            z ^ (z >> 16)
        };
        let low = next().to_le_bytes();
        let high = next().to_le_bytes();
        MacAddress([low[0], low[1], low[2], low[3], high[0], high[1] | 0xC0])
    }

    /// Convert from human readable format, MSB first
    pub fn from_msb(addr: [u8; 6]) -> Self {
        MacAddress([addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]])
    }
}

impl core::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let MacAddress(addr) = self;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            addr[5], addr[4], addr[3], addr[2], addr[1], addr[0]
        )
    }
}

