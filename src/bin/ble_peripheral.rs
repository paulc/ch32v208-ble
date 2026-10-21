//! Connectable BLE peripheral with a simple GATT service (based on the
//! CH32V20x EVT `BLE/Peripheral` example).
//!
//! Service 0xFFE0:
//! - 0xFFE1 "Data": read/write, up to 20 bytes
//! - 0xFFE2 "Counter": read/notify, u32 LE, incremented and notified every second
//!   while notifications are enabled
//!
//! GAP and GATT callbacks run inside TMOS (in the `ble` task). They only record
//! state and post an [`AppEvent`] to the `app` task, which logs events and sends
//! the notifications.

#![no_std]
#![no_main]

use core::ffi::c_void;

use ch32_hal::println;
use ch32_hal::rcc::*;
use ch32_hal::time::Hertz;
use ch32v208_ble::ble;
use ch32v208_ble::ble::ffi::*;
use ch32v208_ble::ble::gap::*;
use ch32v208_ble::ble::gatt::*;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Ticker};

/* Advertising */

/// Advertising interval in 0.625ms units (50ms)
const ADVERTISING_INTERVAL: u16 = 80;
/// Preferred connection interval range in 1.25ms units (7.5ms - 125ms)
const MIN_CONN_INTERVAL: u16 = 6;
const MAX_CONN_INTERVAL: u16 = 100;

const SERVICE_UUID: u16 = 0xFFE0;

static ADVERT_DATA: [u8; 7] = [
    0x02, // length of this data
    GAP_ADTYPE_FLAGS,
    GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED,
    0x03, // length of this data
    GAP_ADTYPE_16BIT_COMPLETE,
    SERVICE_UUID.to_le_bytes()[0],
    SERVICE_UUID.to_le_bytes()[1],
];

static SCAN_RSP_DATA: [u8; 27] = [
    0x14, // length of this data
    GAP_ADTYPE_LOCAL_NAME_COMPLETE,
    b'c', b'h', b'3', b'2', b'v', b'2', b'0', b'8', b' ', b'P', b'e', b'r', b'i', b'p', b'h', b'e', b'r', b'a', b'l',
    0x05, // length of this data
    GAP_ADTYPE_SLAVE_CONN_INTERVAL_RANGE,
    MIN_CONN_INTERVAL.to_le_bytes()[0],
    MIN_CONN_INTERVAL.to_le_bytes()[1],
    MAX_CONN_INTERVAL.to_le_bytes()[0],
    MAX_CONN_INTERVAL.to_le_bytes()[1],
];

/// GAP device name characteristic
static DEVICE_NAME: &[u8] = b"ch32v208 Peripheral";

/* GATT service */

const DATA_MAX_LEN: usize = 20;

static SERVICE_UUID_BYTES: [u8; 2] = SERVICE_UUID.to_le_bytes();
static DATA_UUID: [u8; 2] = 0xFFE1u16.to_le_bytes();
static COUNTER_UUID: [u8; 2] = 0xFFE2u16.to_le_bytes();

static SERVICE: gattAttrType_t = gattAttrType_t::uuid16(&SERVICE_UUID_BYTES);
static DATA_PROPS: u8 = GATT_PROP_READ | GATT_PROP_WRITE;
static DATA_DESC: [u8; 5] = *b"Data\0";
static COUNTER_PROPS: u8 = GATT_PROP_READ | GATT_PROP_NOTIFY;
static COUNTER_DESC: [u8; 8] = *b"Counter\0";

// Attribute values. Accessed from GATT callbacks (in TMOS) and the app task;
// both run on the same embassy executor, so never concurrently.
static mut DATA: [u8; DATA_MAX_LEN] = [0; DATA_MAX_LEN];
static mut DATA_LEN: usize = 0;
static mut COUNTER: [u8; 4] = [0; 4];
static mut COUNTER_CCC: [gattCharCfg_t; GATT_MAX_NUM_CONN] = [gattCharCfg_t::INIT; GATT_MAX_NUM_CONN];

// Attribute table indices
const DATA_VALUE: usize = 2;
const DATA_USER_DESC: usize = 3;
const COUNTER_VALUE: usize = 5;
const COUNTER_CCC_ATTR: usize = 6;
const COUNTER_USER_DESC: usize = 7;

/// Attribute table, registered with the library (which fills in the handles)
static mut ATTRS: [gattAttribute_t; 8] = [
    // Service declaration
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&PRIMARY_SERVICE_UUID),
        GATT_PERMIT_READ,
        &raw const SERVICE as *mut u8,
    ),
    // Data: declaration, value, user description
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&CHARACTER_UUID),
        GATT_PERMIT_READ,
        &raw const DATA_PROPS as *mut u8,
    ),
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&DATA_UUID),
        GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        &raw mut DATA as *mut u8,
    ),
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&CHAR_USER_DESC_UUID),
        GATT_PERMIT_READ,
        &raw const DATA_DESC as *mut u8,
    ),
    // Counter: declaration, value, client characteristic configuration, user description
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&CHARACTER_UUID),
        GATT_PERMIT_READ,
        &raw const COUNTER_PROPS as *mut u8,
    ),
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&COUNTER_UUID),
        GATT_PERMIT_READ,
        &raw mut COUNTER as *mut u8,
    ),
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&CLIENT_CHAR_CFG_UUID),
        GATT_PERMIT_READ | GATT_PERMIT_WRITE,
        &raw mut COUNTER_CCC as *mut u8,
    ),
    gattAttribute_t::new(
        gattAttrType_t::uuid16(&CHAR_USER_DESC_UUID),
        GATT_PERMIT_READ,
        &raw const COUNTER_DESC as *mut u8,
    ),
];

static SERVICE_CBS: gattServiceCBs_t = gattServiceCBs_t {
    pfnReadAttrCB: Some(read_attr),
    pfnWriteAttrCB: Some(write_attr),
    pfnAuthorizeAttrCB: None,
};

/// Index of `attr` in [`ATTRS`]
fn attr_index(attr: *const gattAttribute_t) -> usize {
    (attr as usize - &raw const ATTRS as usize) / size_of::<gattAttribute_t>()
}

/// Copy `value` to a read response
unsafe fn read_value(value: &[u8], dest: *mut u8, dest_len: *mut u16, max_len: u16) -> u8 {
    let len = value.len().min(max_len as usize);
    unsafe {
        core::ptr::copy_nonoverlapping(value.as_ptr(), dest, len);
        *dest_len = len as u16;
    }
    SUCCESS
}

unsafe extern "C" fn read_attr(
    conn_handle: u16,
    attr: *mut gattAttribute_t,
    value: *mut u8,
    len: *mut u16,
    offset: u16,
    max_len: u16,
    _method: u8,
) -> u8 {
    if offset > 0 {
        return ATT_ERR_ATTR_NOT_LONG;
    }
    unsafe {
        match attr_index(attr) {
            DATA_VALUE => {
                let data = &*&raw const DATA;
                read_value(&data[..DATA_LEN], value, len, max_len)
            }
            DATA_USER_DESC => read_value(&DATA_DESC[..DATA_DESC.len() - 1], value, len, max_len),
            COUNTER_VALUE => read_value(&*&raw const COUNTER, value, len, max_len),
            COUNTER_USER_DESC => read_value(&COUNTER_DESC[..COUNTER_DESC.len() - 1], value, len, max_len),
            COUNTER_CCC_ATTR => {
                let cfg = GATTServApp_ReadCharCfg(conn_handle, &raw mut COUNTER_CCC as _);
                read_value(&cfg.to_le_bytes(), value, len, max_len)
            }
            _ => ATT_ERR_ATTR_NOT_FOUND,
        }
    }
}

unsafe extern "C" fn write_attr(
    conn_handle: u16,
    attr: *mut gattAttribute_t,
    value: *mut u8,
    len: u16,
    offset: u16,
    _method: u8,
) -> u8 {
    unsafe {
        match attr_index(attr) {
            DATA_VALUE => {
                if offset > 0 {
                    return ATT_ERR_ATTR_NOT_LONG;
                }
                let new = core::slice::from_raw_parts(value, len as usize);
                if new.len() > DATA_MAX_LEN {
                    return ATT_ERR_INVALID_VALUE_SIZE;
                }
                let data = &mut *&raw mut DATA;
                data[..new.len()].copy_from_slice(new);
                DATA_LEN = new.len();
                send_event(AppEvent::DataWritten(heapless::Vec::from_slice(new).unwrap()));
                SUCCESS
            }
            COUNTER_CCC_ATTR => {
                if let Err(e) =
                    GATTServApp_ProcessCCCWriteReq(conn_handle, attr, value, len, offset, GATT_CLIENT_CFG_NOTIFY)
                {
                    return e.get();
                }
                let cfg = GATTServApp_ReadCharCfg(conn_handle, &raw mut COUNTER_CCC as _);
                send_event(AppEvent::Notify(cfg & GATT_CLIENT_CFG_NOTIFY != 0));
                SUCCESS
            }
            _ => ATT_ERR_ATTR_NOT_FOUND,
        }
    }
}

/// Link DB callback: reset the client configuration when a connection goes away
unsafe extern "C" fn link_status(conn_handle: u16, change_type: u8) {
    if conn_handle == LOOPBACK_CONNHANDLE {
        return;
    }
    unsafe {
        if change_type == LINKDB_STATUS_UPDATE_REMOVED
            || (change_type == LINKDB_STATUS_UPDATE_STATEFLAGS && linkDB_State(conn_handle, LINK_CONNECTED) == 0)
        {
            GATTServApp_InitCharCfg(conn_handle, &raw mut COUNTER_CCC as _);
        }
    }
}

/// Register the service (SimpleProfile_AddService in the EVT)
fn add_service() -> bStatus_t {
    unsafe {
        GATTServApp_InitCharCfg(INVALID_CONNHANDLE, &raw mut COUNTER_CCC as _);
        linkDB_Register(Some(link_status));
        let attrs = &raw mut ATTRS;
        GATTServApp_RegisterService(attrs as _, (*attrs).len() as u16, GATT_MAX_ENCRYPT_KEY_SIZE, &SERVICE_CBS)
    }
}

/* Events from TMOS callbacks to the app task */

enum AppEvent {
    Connected { handle: u16, interval: u16, peer: [u8; B_ADDR_LEN] },
    Disconnected { handle: u16, reason: u8 },
    ParamUpdate { handle: u16, interval: u16 },
    Mtu(u16),
    DataWritten(heapless::Vec<u8, DATA_MAX_LEN>),
    Notify(bool),
}

static EVENTS: Channel<CriticalSectionRawMutex, AppEvent, 8> = Channel::new();

fn send_event(event: AppEvent) {
    if EVENTS.try_send(event).is_err() {
        println!("event dropped");
    }
}

/* GAP callbacks */

unsafe extern "C" fn state_change(new_state: gapRole_States_t, event: *mut gapRoleEvent_t) {
    let event = unsafe { &*event };
    let opcode = unsafe { event.gap.opcode };
    match (new_state & GAPROLE_STATE_ADV_MASK, opcode) {
        (GAPROLE_STARTED, _) => println!("Initialized.."),
        (GAPROLE_ADVERTISING, GAP_MAKE_DISCOVERABLE_DONE_EVENT) => println!("Advertising.."),
        (GAPROLE_CONNECTED, GAP_LINK_ESTABLISHED_EVENT) => {
            let link = unsafe { &event.linkCmpl };
            send_event(AppEvent::Connected {
                handle: link.connectionHandle,
                interval: link.connInterval,
                peer: link.devAddr,
            });
        }
        (GAPROLE_ADVERTISING | GAPROLE_WAITING, GAP_LINK_TERMINATED_EVENT) => {
            let link = unsafe { &event.linkTerminate };
            send_event(AppEvent::Disconnected {
                handle: link.connectionHandle,
                reason: link.reason,
            });
            // Advertise again
            let enable: u8 = 1;
            let _ = unsafe { GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &raw const enable as _) };
        }
        (GAPROLE_WAITING, GAP_END_DISCOVERABLE_DONE_EVENT) => println!("Waiting for advertising.."),
        (GAPROLE_ERROR, _) => println!("Error.."),
        (state, opcode) => println!("GAP state {:#x} opcode {:#x}", state, opcode),
    }
}

unsafe extern "C" fn param_update(handle: u16, interval: u16, _latency: u16, _timeout: u16) {
    send_event(AppEvent::ParamUpdate { handle, interval });
}

static BOND_CBS: gapBondCBs_t = gapBondCBs_t {
    passcodeCB: None,
    pairStateCB: None,
    oobCB: None,
};

static ROLE_CBS: gapRolesCBs_t = gapRolesCBs_t {
    pfnStateChange: Some(state_change),
    pfnRssiRead: None,
    pfnParamUpdate: Some(param_update),
};

/// TMOS task receiving GAP/GATT messages for the application
unsafe extern "C" fn app_tmos_task(task_id: tmosTaskID, events: tmosEvents) -> tmosEvents {
    if events & SYS_EVENT_MSG != 0 {
        unsafe {
            let msg = tmos_msg_receive(task_id);
            if !msg.is_null() {
                let hdr = &*(msg as *const tmos_event_hdr_t);
                if hdr.event == GATT_MSG_EVENT {
                    let gatt = &*(msg as *const gattMsgEvent_t);
                    if gatt.method == ATT_MTU_UPDATED_EVENT {
                        send_event(AppEvent::Mtu(gatt.msg.mtuEvt.MTU));
                    }
                }
                let _ = tmos_msg_deallocate(msg);
            }
        }
        return events ^ SYS_EVENT_MSG;
    }
    0
}

/// Peripheral_Init in the EVT
fn peripheral_init() -> bStatus_t {
    unsafe {
        GAPRole_PeripheralInit()?;

        let enable: u8 = 1;
        GAPRole_SetParameter(GAPROLE_ADVERT_ENABLED, 1, &raw const enable as _)?;
        GAPRole_SetParameter(GAPROLE_ADVERT_DATA, ADVERT_DATA.len() as u16, ADVERT_DATA.as_ptr() as _)?;
        GAPRole_SetParameter(GAPROLE_SCAN_RSP_DATA, SCAN_RSP_DATA.len() as u16, SCAN_RSP_DATA.as_ptr() as _)?;
        let (min_interval, max_interval) = (MIN_CONN_INTERVAL, MAX_CONN_INTERVAL);
        GAPRole_SetParameter(GAPROLE_MIN_CONN_INTERVAL, 2, &raw const min_interval as _)?;
        GAPRole_SetParameter(GAPROLE_MAX_CONN_INTERVAL, 2, &raw const max_interval as _)?;

        GAP_SetParamValue(TGAP_DISC_ADV_INT_MIN, ADVERTISING_INTERVAL)?;
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MAX, ADVERTISING_INTERVAL)?;

        GGS_AddService(GATT_ALL_SERVICES)?; // GAP service
        GATTServApp_AddService(GATT_ALL_SERVICES)?; // GATT service
        add_service()?;
        GGS_SetParameter(GGS_DEVICE_NAME_ATT, DEVICE_NAME.len() as u8, DEVICE_NAME.as_ptr() as *const c_void)?;

        let task_id = TMOS_ProcessEventRegister(Some(app_tmos_task));
        GAPRole_PeripheralStartDevice(task_id, &BOND_CBS, &ROLE_CBS)
    }
}

/// Send the counter as a notification (peripheralChar4Notify in the EVT)
fn notify_counter(conn_handle: u16) -> bStatus_t {
    unsafe {
        let value = &*&raw const COUNTER;
        let mut noti = attHandleValueNoti_t {
            handle: (*&raw const ATTRS)[COUNTER_VALUE].handle,
            len: value.len() as u16,
            pValue: core::ptr::null_mut(),
        };
        noti.pValue = GATT_bm_alloc(conn_handle, ATT_HANDLE_VALUE_NOTI, noti.len, core::ptr::null_mut(), 0) as _;
        if noti.pValue.is_null() {
            return Err(core::num::NonZeroU8::new(bleMemAllocError).unwrap());
        }
        core::ptr::copy_nonoverlapping(value.as_ptr(), noti.pValue, value.len());
        let result = GATT_Notification(conn_handle, &mut noti, 0);
        if result.is_err() {
            GATT_bm_free(&raw mut noti as _, ATT_HANDLE_VALUE_NOTI);
        }
        result
    }
}

#[embassy_executor::task]
async fn app_task() -> ! {
    let mut conn: Option<u16> = None;
    let mut notify = false;
    let mut counter = 0u32;
    let mut ticker = Ticker::every(Duration::from_secs(1));

    loop {
        match select(EVENTS.receive(), ticker.next()).await {
            Either::First(event) => match event {
                AppEvent::Connected { handle, interval, peer } => {
                    println!(
                        "Connected: handle {:#x}, interval {} x 1.25ms, peer {}",
                        handle,
                        interval,
                        ble::MacAddress(peer)
                    );
                    conn = Some(handle);
                }
                AppEvent::Disconnected { handle, reason } => {
                    println!("Disconnected: handle {:#x}, reason {:#x}", handle, reason);
                    conn = None;
                    notify = false;
                }
                AppEvent::ParamUpdate { handle, interval } => {
                    println!("Param update: handle {:#x}, interval {} x 1.25ms", handle, interval);
                }
                AppEvent::Mtu(mtu) => println!("MTU: {}", mtu),
                AppEvent::DataWritten(data) => println!("Data written: {:02x?}", data.as_slice()),
                AppEvent::Notify(enabled) => {
                    println!("Counter notifications {}", if enabled { "on" } else { "off" });
                    notify = enabled;
                }
            },
            Either::Second(()) => {
                counter = counter.wrapping_add(1);
                unsafe { *&raw mut COUNTER = counter.to_le_bytes() };
                if let (Some(handle), true) = (conn, notify) {
                    if let Err(e) = notify_counter(handle) {
                        println!("notify failed: {:#x}", e.get());
                    }
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn ble_task() -> ! {
    ble::run().await
}

#[embassy_executor::main(entry = "ch32_hal::entry")]
async fn main(spawner: Spawner) -> ! {
    ch32_hal::debug::SDIPrint::enable();

    let mut config = ch32_hal::Config::default();
    config.rcc = Config {
        // 32MHz crystal (required by the radio). As in the WCH BLE EVT:
        // SYSCLK = HSE/4 * 12 = 96MHz, PCLK1 = HCLK/2
        hse: Some(Hse {
            freq: Hertz(32_000_000),
            mode: HseMode::Oscillator,
        }),
        sys: Sysclk::PLL,
        pll_src: PllSource::HSE,
        pll: Some(Pll {
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL12,
        }),
        pllx: None,
        ahb_pre: AHBPrescaler::DIV1,
        apb1_pre: APBPrescaler::DIV2,
        apb2_pre: APBPrescaler::DIV1,
        ls: LsConfig::default_lsi(), // unused by ch32-hal; ble::init starts the LSE
        hspll_src: HsPllSource::HSI,
        hspll: None,
    };
    let _p = ch32_hal::init(config);

    println!("BLE peripheral on ch32v208");
    println!("BLE lib: {}", ble::lib_version());

    let ble_config = ble::Config::default();
    println!("MAC: {}", ble_config.mac_addr);

    if let Err(e) = ble::init(ble_config) {
        panic!("ble::init: {:?}", e);
    }
    if let Err(e) = peripheral_init() {
        panic!("peripheral_init: {:?}", e);
    }

    spawner.spawn(ble_task().unwrap());
    spawner.spawn(app_task().unwrap());

    let mut ticker = Ticker::every(Duration::from_secs(10));
    loop {
        ticker.next().await;
        println!("{}", ble::STATS);
    }
}

ch32v208_ble::fault_handlers!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let _ = ch32_hal::println!("\n\n\n{}", info);
    loop {}
}
