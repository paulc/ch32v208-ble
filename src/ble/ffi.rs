//! Bindings to `vendor/libwchble.a` (CH32V20x_BLE_LIB_V1.4).
//!
//! Hand-written from `vendor/wchble.h`, covering only what is used so far. Names
//! and values follow the C header so the WCH reference manual and EVT examples can
//! be read alongside. Adapted from `ch58x-hal/src/ble/ffi.rs` - note that several
//! structs and constants differ between the CH58x and CH32V20x libraries
//! (e.g. `bleConfig_t`, `bleClockConfig_t`, `LL_TX_POWEER_*`).

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]

use core::ffi::c_void;
use core::num::NonZeroU8;

/// `bStatus_t` is a `uint8_t` where 0 is SUCCESS. `Result<(), NonZeroU8>` has the
/// same ABI, so library calls can be used with `?`.
pub type bStatus_t = Result<(), NonZeroU8>;

pub type tmosTaskID = u8;
pub type tmosEvents = u16;
pub type tmosTimer = u32;
pub type BOOL = u8;

/// Header version these bindings were written against (checked against `VER_LIB` at init)
pub const VER_FILE: &str = "CH32V20x_BLE_LIB_V1.4";

/* Tx_POWER define (Accuracy: ±2dBm) */
pub const LL_TX_POWEER_MINUS_18_DBM: u8 = 0x01;
pub const LL_TX_POWEER_MINUS_10_DBM: u8 = 0x03;
pub const LL_TX_POWEER_MINUS_5_DBM: u8 = 0x05;
pub const LL_TX_POWEER_MINUS_3_DBM: u8 = 0x07;
pub const LL_TX_POWEER_0_DBM: u8 = 0x09;
pub const LL_TX_POWEER_1_DBM: u8 = 0x0B;
pub const LL_TX_POWEER_2_DBM: u8 = 0x0D;
pub const LL_TX_POWEER_3_DBM: u8 = 0x11;
pub const LL_TX_POWEER_4_DBM: u8 = 0x15;
pub const LL_TX_POWEER_5_DBM: u8 = 0x1B;
pub const LL_TX_POWEER_6_DBM: u8 = 0x25;
pub const LL_TX_POWEER_7_DBM: u8 = 0x3F;

/* ERR_LIB_INIT define (BLE_LibInit return codes) */
pub const ERR_LLE_IRQ_HANDLE: u8 = 0x01;
pub const ERR_MEM_ALLOCATE_SIZE: u8 = 0x02;
pub const ERR_SET_MAC_ADDR: u8 = 0x03;
pub const ERR_GAP_ROLE_CONFIG: u8 = 0x04;
pub const ERR_CONNECT_NUMBER_CONFIG: u8 = 0x05;
pub const ERR_SNV_ADDR_CONFIG: u8 = 0x06;
pub const ERR_CLOCK_SELECT_CONFIG: u8 = 0x07;

/// BLE library config struct, passed to `BLE_LibInit`
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct bleConfig_t {
    /// library memory start address
    pub MEMAddr: u32,
    /// library memory size
    pub MEMLen: u16,
    /// SNV flash start address (if NULL, bonding information will not be saved)
    pub SNVAddr: u32,
    /// SNV flash block size (default 256)
    pub SNVBlock: u16,
    /// SNV flash block number (default 1)
    pub SNVNum: u8,
    /// Maximum number of sent and received packages cached by the controller (default 5).
    /// Must be greater than the number of connections.
    pub BufNumber: u8,
    /// Maximum length (in octets) of the data portion of each HCI data packet (default 27)
    pub BufMaxLen: u16,
    /// Maximum number of TX data in a connection event (default 1)
    pub TxNumEvent: u8,
    /// Maximum number of RX data in a connection event (default equal to BufNumber)
    pub RxNumEvent: u8,
    /// Transmit power level (default LL_TX_POWEER_0_DBM)
    pub TxPower: u8,
    /// Connect number, lower two bits are peripheral number, followed by central number
    pub ConnectNumber: u8,
    /// Wait rf start window (us)
    pub WindowWidening: u8,
    /// Wait event arrive window in one system clock
    pub WaitWindow: u8,
    /// MAC address, little-endian
    pub MacAddr: [u8; 6],
    /// The timing clock frequency (Hz)
    pub ClockFrequency: u16,
    /// The timing clock accuracy (ppm)
    pub ClockAccuracy: u16,
    /// Generate a random seed
    pub srandCB: Option<unsafe extern "C" fn() -> u32>,
    /// Called when TMOS is idle with the clock count of the next scheduled event.
    /// Returns 0 if the device slept, non-zero otherwise.
    pub idleCB: Option<unsafe extern "C" fn(time: u32) -> u32>,
    /// Read the current temperature, to determine whether calibration is needed
    pub tsCB: Option<unsafe extern "C" fn() -> u16>,
    /// LSI clock calibration
    pub rcCB: Option<unsafe extern "C" fn()>,
    /// Library status callback
    pub staCB: Option<unsafe extern "C" fn(code: u8, status: u32)>,
    /// Read flash (SNV)
    pub readFlashCB: Option<unsafe extern "C" fn(addr: u32, num: u32, pBuf: *mut u32) -> u32>,
    /// Write flash (SNV)
    pub writeFlashCB: Option<unsafe extern "C" fn(addr: u32, num: u32, pBuf: *mut u32) -> u32>,
}

// Spot-check the layout against offsets used inside the library
const _: () = assert!(core::mem::offset_of!(bleConfig_t, idleCB) == 40);
const _: () = assert!(core::mem::size_of::<bleConfig_t>() == 64);

/// TMOS timebase config, passed to `TMOS_TimerInit`
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct bleClockConfig_t {
    /// Returns the current clock count
    pub getClockValue: Option<unsafe extern "C" fn() -> u32>,
    /// The maximum count value
    pub ClockMaxCount: u32,
    /// The timing clock frequency (Hz)
    pub ClockFrequency: u16,
    /// The timing clock accuracy (ppm)
    pub ClockAccuracy: u16,
    /// reserved
    pub irqEnable: u8,
}

/* TMOS */

/// A message is waiting event
pub const SYS_EVENT_MSG: tmosEvents = 0x8000;
/// Task ID isn't setup properly
pub const INVALID_TASK_ID: tmosTaskID = 0xFF;

/// Unit of the TMOS event timer is 625us
pub const SYSTEM_TIME_MICROSEN: u32 = 625;

/// Convert ms to TMOS timer units (625us)
pub const fn MS1_TO_SYSTEM_TIME(ms: u32) -> tmosTimer {
    ms * 1000 / SYSTEM_TIME_MICROSEN
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct tmos_event_hdr_t {
    pub event: u8,
    pub status: u8,
}

pub type pTaskEventHandlerFn = Option<unsafe extern "C" fn(taskID: tmosTaskID, events: tmosEvents) -> tmosEvents>;

/* GAP */

/// Minimum advertising interval (n * 0.625 mSec)
pub const TGAP_DISC_ADV_INT_MIN: u16 = 3;
/// Maximum advertising interval (n * 0.625 mSec)
pub const TGAP_DISC_ADV_INT_MAX: u16 = 4;

/* GAP_ADVERTISEMENT_TYPE_DEFINES */
pub const GAP_ADTYPE_ADV_IND: u8 = 0x00;
pub const GAP_ADTYPE_ADV_SCAN_IND: u8 = 0x02;
pub const GAP_ADTYPE_ADV_NONCONN_IND: u8 = 0x03;

/* GAP_ADTYPE_DEFINES */
pub const GAP_ADTYPE_FLAGS: u8 = 0x01;
pub const GAP_ADTYPE_LOCAL_NAME_SHORT: u8 = 0x08;
pub const GAP_ADTYPE_LOCAL_NAME_COMPLETE: u8 = 0x09;
pub const GAP_ADTYPE_POWER_LEVEL: u8 = 0x0A;
pub const GAP_ADTYPE_MANUFACTURER_SPECIFIC: u8 = 0xFF;

/* GAP_ADTYPE_FLAGS_MODES */
pub const GAP_ADTYPE_FLAGS_GENERAL: u8 = 0x02;
pub const GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED: u8 = 0x04;

/* GAPROLE_PROFILE_PARAMETERS */
pub const GAPROLE_PROFILEROLE: u16 = 0x300;
pub const GAPROLE_ADVERT_ENABLED: u16 = 0x305;
pub const GAPROLE_ADVERT_DATA: u16 = 0x306;
pub const GAPROLE_SCAN_RSP_DATA: u16 = 0x307;
pub const GAPROLE_ADV_EVENT_TYPE: u16 = 0x308;

/// GAP role state. Bits [3:0] are the advertising state.
pub type gapRole_States_t = u32;

pub const GAPROLE_STATE_ADV_MASK: gapRole_States_t = 0xF;
pub const GAPROLE_INIT: gapRole_States_t = 0;
pub const GAPROLE_STARTED: gapRole_States_t = 1;
pub const GAPROLE_ADVERTISING: gapRole_States_t = 2;
pub const GAPROLE_WAITING: gapRole_States_t = 3;
pub const GAPROLE_CONNECTED: gapRole_States_t = 4;
pub const GAPROLE_CONNECTED_ADV: gapRole_States_t = 5;
pub const GAPROLE_ERROR: gapRole_States_t = 6;

pub type gapRolesBroadcasterStateNotify_t = Option<unsafe extern "C" fn(newState: gapRole_States_t)>;
/// Scan request received (argument is a `gapScanRec_t *`, not bound yet)
pub type gapRolesScanReqRecv_t = Option<unsafe extern "C" fn(pEvent: *mut c_void)>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct gapRolesBroadcasterCBs_t {
    /// Whenever the device changes state
    pub pfnStateChange: gapRolesBroadcasterStateNotify_t,
    pub pfnScanRecv: gapRolesScanReqRecv_t,
}

#[link(name = "wchble", kind = "static")]
unsafe extern "C" {
    /// Library version string, e.g. "CH32V20x_BLE_LIB_V1.4"
    pub static VER_LIB: [u8; 0];

    /* BLE */

    /// Init BLE lib. Returns 0 on success, error defined @ ERR_LIB_INIT
    pub fn BLE_LibInit(pCfg: *const bleConfig_t) -> bStatus_t;
    /// BLE register reset and rf calibration
    pub fn BLE_RegInit();
    /// BB interrupt handler, called from the `BB` vector
    pub fn BB_IRQLibHandler();
    /// LLE interrupt handler, called via `g_LLE_IRQLibHandlerLocation` from the `LLE` vector
    pub fn LLE_IRQLibHandler();

    /* TMOS */

    /// TMOS system timer initialization. Must be called before using TMOS tasks.
    pub fn TMOS_TimerInit(pClockConfig: *const bleClockConfig_t) -> bStatus_t;
    /// Register a process event callback. Returns 0xFF on error, otherwise the task id.
    pub fn TMOS_ProcessEventRegister(eventCb: pTaskEventHandlerFn) -> tmosTaskID;
    /// Process system (one pass)
    pub fn TMOS_SystemProcess();
    /// Get current system clock (in 625us units)
    pub fn TMOS_GetSystemClock() -> u32;

    /// Set an event for a task (runs on the next TMOS_SystemProcess)
    pub fn tmos_set_event(taskID: tmosTaskID, event: tmosEvents) -> bStatus_t;
    /// Start an event after a period of time (625us units). Returns TRUE/FALSE.
    pub fn tmos_start_task(taskID: tmosTaskID, event: tmosEvents, time: tmosTimer) -> BOOL;
    /// Stop a timed event
    pub fn tmos_stop_task(taskID: tmosTaskID, event: tmosEvents) -> bStatus_t;
    /// Receive a message, or NULL if none
    pub fn tmos_msg_receive(taskID: tmosTaskID) -> *mut u8;
    /// Free a received message
    pub fn tmos_msg_deallocate(msg_ptr: *mut u8) -> bStatus_t;

    /* GAP */

    /// Set a GAP parameter (TGAP_*)
    pub fn GAP_SetParamValue(paramID: u16, paramValue: u16) -> bStatus_t;
    /// Set a GAP Role parameter (GAPROLE_*). The value is copied.
    pub fn GAPRole_SetParameter(param: u16, len: u16, pValue: *const c_void) -> bStatus_t;

    /* GAP Broadcaster role */

    /// Initialize the Broadcaster role
    pub fn GAPRole_BroadcasterInit() -> bStatus_t;
    /// Start the device in Broadcaster role
    pub fn GAPRole_BroadcasterStartDevice(pAppCallbacks: &'static gapRolesBroadcasterCBs_t) -> bStatus_t;
    /// Set the Broadcaster scan request callback
    pub fn GAPRole_BroadcasterSetCB(pAppCallbacks: &'static gapRolesBroadcasterCBs_t);
}
