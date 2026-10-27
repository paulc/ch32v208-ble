//! Connectable BLE peripheral with a simple GATT service, using the
//! `ble::gatt_server` and `ble::peripheral` helpers. (`ble_peripheral_raw` is
//! the same application wired up by hand with the raw bindings.)
//!
//! Service 0xFFE0:
//! - 0xFFE1 "Data": read/write, up to 20 bytes
//! - 0xFFE2 "Counter": read/notify, u32 LE, incremented and notified every second
//!   while a client has notifications enabled (paused otherwise)
//!
//! The characteristic callbacks run inside TMOS (in the `ble` task): they only
//! update state and signal the main task, which does the logging. The counter
//! has its own task, which notifies all subscribed connections (`notify_all`)
//! without tracking connections itself.

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use ch32_hal::try_println;
use ch32_hal::rcc::*;
use ch32_hal::time::Hertz;
use ch32v208_ble::ble;
use ch32v208_ble::ble::MacAddress;
use ch32v208_ble::ble::gap::*;
use ch32v208_ble::ble::gatt::*;
use ch32v208_ble::ble::gatt_server::{self, Characteristic, Service, Uuid};
use ch32v208_ble::ble::peripheral::{self, AdvData, Event};

use embassy_executor::Spawner;
use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use heapless::Vec;

const SHORT_NAME: &str = "ch32v208-per";
const DEVICE_NAME: &str = "ch32v208-peripheral";
const SERVICE_UUID: u16 = 0xFFE0;
const DATA_MAX_LEN: usize = 20;

/* Application state, shared between the GATT callbacks and the main task */

static DATA: Mutex<CriticalSectionRawMutex, RefCell<Vec<u8, DATA_MAX_LEN>>> = Mutex::new(RefCell::new(Vec::new()));
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Signals from the GATT callbacks to the main task
static DATA_WRITTEN: Signal<CriticalSectionRawMutex, Vec<u8, DATA_MAX_LEN>> = Signal::new();
static SUBSCRIBED: Signal<CriticalSectionRawMutex, bool> = Signal::new();

/* GATT service */

static DATA_CHAR: Characteristic = Characteristic::new(Uuid::u16(0xFFE1), GATT_PROP_READ | GATT_PROP_WRITE)
    .description("Data")
    .on_read(&|_conn, buf| {
        DATA.lock(|data| {
            let data = data.borrow();
            let len = data.len().min(buf.len());
            buf[..len].copy_from_slice(&data[..len]);
            Ok(len)
        })
    })
    .on_write(&|_conn, value| {
        let value = Vec::from_slice(value).map_err(|_| ATT_ERR_INVALID_VALUE_SIZE)?;
        DATA.lock(|data| *data.borrow_mut() = value.clone());
        DATA_WRITTEN.signal(value);
        Ok(())
    });

static COUNTER_CHAR: Characteristic = Characteristic::new(Uuid::u16(0xFFE2), GATT_PROP_READ | GATT_PROP_NOTIFY)
    .description("Counter")
    .on_read(&|_conn, buf| {
        let value = COUNTER.load(Ordering::Relaxed).to_le_bytes();
        buf[..value.len()].copy_from_slice(&value);
        Ok(value.len())
    })
    .on_subscribe(&|_conn, enabled| SUBSCRIBED.signal(enabled));

static SERVICE: Service = Service::new(Uuid::u16(SERVICE_UUID), &[&DATA_CHAR, &COUNTER_CHAR]);

#[embassy_executor::task]
async fn ble_task() -> ! {
    ble::run().await
}

/// Increment the counter every second while a client is subscribed, and notify
/// all subscribers
#[embassy_executor::task]
async fn counter_task() -> ! {
    loop {
        // Pause while nobody is listening
        if let Err(e) = gatt_server::wait_subscribed(&COUNTER_CHAR).await {
            panic!("wait_subscribed: {:?}", e);
        }
        Timer::after_secs(1).await;
        // The client may have unsubscribed or disconnected during the sleep:
        // don't produce a value nobody will be notified of
        if !gatt_server::is_subscribed_any(&COUNTER_CHAR) {
            continue;
        }

        let counter = COUNTER.load(Ordering::Relaxed).wrapping_add(1);
        COUNTER.store(counter, Ordering::Relaxed);
        if let Err(e) = gatt_server::notify_all(&COUNTER_CHAR, &counter.to_le_bytes()) {
            try_println!("notify failed: {:?}", e);
        }
    }
}

#[embassy_executor::task]
async fn stats_task() -> ! {
    loop {
        Timer::after_secs(10).await;
        try_println!("{}", ble::STATS);
    }
}

#[embassy_executor::main(entry = "ch32_hal::entry")]
async fn main(spawner: Spawner) -> ! {
    ch32_hal::debug::SDIPrint::enable();

    let config = ch32_hal::Config {
        rcc: Config {
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
        },
        ..Default::default()
    };
    let _p = ch32_hal::init(config);

    try_println!("BLE peripheral on ch32v208");
    try_println!("BLE lib: {}", ble::lib_version());

    // Use random MAC address to avoid MacOS caching
    let ble_config = ble::Config {
        mac_addr: MacAddress::random(),
        ..Default::default()
    };

    try_println!("MAC: {}", ble_config.mac_addr);
    if let Err(e) = ble::init(ble_config) {
        panic!("ble::init: {:?}", e);
    }

    let adv = AdvData::new()
        .flags(GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED)
        .uuid16_list(&[SERVICE_UUID])
        .short_name(SHORT_NAME);
    let scan_rsp = AdvData::new().name(DEVICE_NAME).conn_interval_range(6, 100);
    let peripheral_config = peripheral::Config::new(DEVICE_NAME, &adv, &scan_rsp);
    // peripheral_config.scan_request_events = true; // log scan requests (diagnostics)
    if let Err(e) = peripheral::start(&peripheral_config, &[&SERVICE]) {
        panic!("peripheral::start: {:?}", e);
    }
    try_println!("Advertising..");

    spawner.spawn(ble_task().unwrap());
    spawner.spawn(counter_task().unwrap());
    spawner.spawn(stats_task().unwrap());

    loop {
        match select3(peripheral::next_event(), DATA_WRITTEN.wait(), SUBSCRIBED.wait()).await {
            Either3::First(event) => match event {
                Event::Connected { handle, interval, peer } => {
                    try_println!("Connected: handle {:#x}, interval {} x 1.25ms, peer {}", handle, interval, peer);
                }
                Event::Disconnected { handle, reason } => {
                    try_println!("Disconnected: handle {:#x}, reason {:#x}", handle, reason);
                }
                Event::ParamUpdate { handle, interval, .. } => {
                    try_println!("Param update: handle {:#x}, interval {} x 1.25ms", handle, interval);
                }
                Event::Mtu { mtu, .. } => try_println!("MTU: {}", mtu),
                Event::ScanRequest { peer } => try_println!("Scan request from {}", peer),
            },
            Either3::Second(data) => try_println!("Data written: {:02x?}", data.as_slice()),
            Either3::Third(enabled) => try_println!("Counter notifications {}", if enabled { "on" } else { "off" }),
        }
    }
}

ch32v208_ble::fault_handlers!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ch32_hal::try_println!("\n\n\n{}", info);
    loop {}
}
