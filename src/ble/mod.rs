//! BLE for the CH32V208 using the WCH BLE library (`vendor/libwchble.a`).
//!
//! The library contains the BLE controller and host, scheduled by WCH's TMOS
//! (a cooperative event/task scheduler). The application drives it by calling
//! `TMOS_SystemProcess()` repeatedly. Here that is done by [`run`], which should be
//! spawned as an embassy task:
//!
//! - After each pass TMOS calls our idle callback with the RTC count of its next
//!   timed event; [`run`] then sleeps (async) until that time, or until a BB/LLE
//!   interrupt signals that the radio has produced work.
//! - If TMOS was not idle, [`run`] just yields and calls it again.
//!
//! The library is not re-entrant: call TMOS/GAP functions either before spawning
//! [`run`], or from TMOS callbacks/tasks. If another embassy task calls into the
//! library it should call [`wake`] afterwards so TMOS processes any new events.
//!
//! Setup mirrors `HAL/MCU.c` (`WCHBLE_Init`, `HAL_Init`) and `HAL/RTC.c` from the
//! CH32V20x EVT (openwch/ch32v20x `EVT/EXAM/BLE`).

pub mod ffi;
mod irq;
pub mod rtc;

use core::ffi::CStr;
use core::mem::MaybeUninit;
use core::num::NonZeroU8;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use ch32_hal::pac;
use embassy_futures::select::select;
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};

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
const PERIPHERAL_MAX_CONNECTION: u8 = 1;
const CENTRAL_MAX_CONNECTION: u8 = 3;

/// HAL task event: periodic RF calibration
const HAL_REG_INIT_EVENT: tmosEvents = 0x2000;
const BLE_CALIBRATION_PERIOD_MS: u32 = 120_000;

/// OSC registers (not in the ch32-metapac CH32V208 PAC)
const OSC_HSE_CAL_CTRL: *mut u32 = 0x4002_202C as *mut u32;

/// Signalled by the BB/LLE interrupts (and [`wake`]) to run TMOS
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Set by the idle callback during `TMOS_SystemProcess`
static IDLE: AtomicBool = AtomicBool::new(false);
/// RTC count of the next TMOS timed event, valid when IDLE is set
static IDLE_UNTIL: AtomicU32 = AtomicU32::new(0);

/// Event counters, for diagnostics
pub struct Stats {
    /// `TMOS_SystemProcess` calls
    pub tmos_passes: AtomicU32,
    /// Idle callbacks
    pub idle: AtomicU32,
    pub bb_irqs: AtomicU32,
    pub lle_irqs: AtomicU32,
}

pub static STATS: Stats = Stats {
    tmos_passes: AtomicU32::new(0),
    idle: AtomicU32::new(0),
    bb_irqs: AtomicU32::new(0),
    lle_irqs: AtomicU32::new(0),
};

/// Increment a counter. No atomic RMW on riscv32imc, but each counter has a single writer.
fn count(counter: &AtomicU32) {
    counter.store(counter.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
}

impl core::fmt::Display for Stats {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "tmos={} idle={} bb={} lle={} rtc={}",
            self.tmos_passes.load(Ordering::Relaxed),
            self.idle.load(Ordering::Relaxed),
            self.bb_irqs.load(Ordering::Relaxed),
            self.lle_irqs.load(Ordering::Relaxed),
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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            mac_addr: MacAddress::factory(),
            tx_power: LL_TX_POWEER_0_DBM,
        }
    }
}

/// Initialise the library, the TMOS timebase (RTC/LSE) and the BLE interrupts.
///
/// Must be called after `ch32_hal::init` (which starts the embassy time driver
/// used for the LSE start-up timeout). Call the GAP role init functions after
/// this, then spawn [`run`].
pub fn init(config: Config) -> Result<(), Error> {
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
    cfg.ConnectNumber = (PERIPHERAL_MAX_CONNECTION & 3) | (CENTRAL_MAX_CONNECTION << 2);
    cfg.MacAddr = config.mac_addr.0;
    cfg.srandCB = Some(srand);
    cfg.idleCB = Some(idle);
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
            irqEnable: 0,
        };
        TMOS_TimerInit(&clock).map_err(Error::TimerInit)?;

        tmos_start_task(
            hal_task,
            HAL_REG_INIT_EVENT,
            MS1_TO_SYSTEM_TIME(BLE_CALIBRATION_PERIOD_MS),
        );
    }

    Ok(())
}

/// Run TMOS forever. Spawn this in an embassy task after [`init`].
pub async fn run() -> ! {
    loop {
        WAKE.reset();
        IDLE.store(false, Ordering::Relaxed);

        unsafe { TMOS_SystemProcess() };
        count(&STATS.tmos_passes);

        if IDLE.load(Ordering::Relaxed) {
            let ticks = IDLE_UNTIL.load(Ordering::Relaxed).wrapping_sub(rtc::now()) as i32;
            if ticks > 0 {
                let timeout = Timer::after_micros(rtc::ticks_to_us(ticks as u32));
                select(WAKE.wait(), timeout).await;
                continue;
            }
        }
        yield_now().await;
    }
}

/// Wake [`run`] so TMOS processes newly set events
pub fn wake() {
    WAKE.signal(());
}

/// Library version, e.g. "CH32V20x_BLE_LIB_V1.4"
pub fn lib_version() -> &'static str {
    unsafe { CStr::from_ptr(VER_LIB.as_ptr().cast()) }.to_str().unwrap_or("?")
}

/// `idleCB`: TMOS has nothing to do until RTC count `time`.
///
/// Returns non-zero ("did not sleep"); the waiting is done in [`run`].
unsafe extern "C" fn idle(time: u32) -> u32 {
    IDLE_UNTIL.store(time, Ordering::Relaxed);
    IDLE.store(true, Ordering::Relaxed);
    count(&STATS.idle);
    1
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

