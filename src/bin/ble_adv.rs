//! Simple BLE advertiser (port of the CH32V20x EVT `BLE/Broadcaster` example).
//!
//! Advertises scannable (not connectable) every 100ms: the advertising packet
//! carries WCH manufacturer data and the short name "ch32v208-adv", the scan
//! response the complete name "ch32v208-advertiser". The EVT uses
//! non-connectable advertising; scannable also exercises the scan
//! request/response path. TMOS runs in the `ble` embassy task alongside a `tick` task.

#![no_std]
#![no_main]

use ch32_hal::try_println;
use ch32_hal::rcc::*;
use ch32_hal::time::Hertz;
use ch32v208_ble::ble;
use ch32v208_ble::ble::ffi::*;
use ch32v208_ble::ble::adv::AdvData;
use ch32v208_ble::ble::gap::*;

use embassy_executor::Spawner;
use embassy_time::Timer;

const SHORT_NAME: &str = "ch32v208-adv";
const NAME: &str = "ch32v208-advertiser";
/// Nanjing Qinheng Microelectronics Co., Ltd. (WCH)
const WCH_COMPANY_ID: u16 = 0x07D7;

/// Advertising interval in 0.625ms units (100ms)
const ADVERTISING_INTERVAL: u16 = 160;

static BROADCASTER_CBS: gapRolesBroadcasterCBs_t = gapRolesBroadcasterCBs_t {
    pfnStateChange: Some(broadcaster_state),
    pfnScanRecv: None,
};

unsafe extern "C" fn broadcaster_state(new_state: gapRole_States_t) {
    match new_state & GAPROLE_STATE_ADV_MASK {
        GAPROLE_STARTED => try_println!("Initialized.."),
        GAPROLE_ADVERTISING => try_println!("Advertising.."),
        GAPROLE_WAITING => try_println!("Waiting for advertising.."),
        GAPROLE_ERROR => try_println!("Error.."),
        _ => try_println!("Broadcaster state: {:#x}", new_state),
    }
}

/// Broadcaster_Init in the EVT
fn broadcaster_init() -> bStatus_t {
    unsafe {
        GAPRole_BroadcasterInit()?;

        let advertising_enable: u8 = 1;
        let adv_event_type: u8 = GAP_ADTYPE_ADV_SCAN_IND;
        GAPRole_SetParameter(
            GAPROLE_ADVERT_ENABLED,
            1,
            &raw const advertising_enable as _,
        )?;
        GAPRole_SetParameter(GAPROLE_ADV_EVENT_TYPE, 1, &raw const adv_event_type as _)?;
        let adv = AdvData::new()
            .flags(GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED)
            .manufacturer(WCH_COMPANY_ID, &[0x01])
            .short_name(SHORT_NAME);
        let scan_rsp = AdvData::new().name(NAME).tx_power(0);
        // The library copies the data
        let (adv, scan_rsp) = (adv.as_slice(), scan_rsp.as_slice());
        GAPRole_SetParameter(GAPROLE_ADVERT_DATA, adv.len() as u16, adv.as_ptr() as _)?;
        GAPRole_SetParameter(GAPROLE_SCAN_RSP_DATA, scan_rsp.len() as u16, scan_rsp.as_ptr() as _)?;

        GAP_SetParamValue(TGAP_DISC_ADV_INT_MIN, ADVERTISING_INTERVAL)?;
        GAP_SetParamValue(TGAP_DISC_ADV_INT_MAX, ADVERTISING_INTERVAL)?;

        GAPRole_BroadcasterStartDevice(&BROADCASTER_CBS)
    }
}

#[embassy_executor::task]
async fn ble_task() -> ! {
    ble::run().await
}

#[embassy_executor::task]
async fn tick_task() -> ! {
    let mut tick = 0u32;
    loop {
        Timer::after_millis(5000).await;
        tick += 1;
        try_println!("tick: {} {}", tick, ble::STATS);
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

    try_println!("BLE advertiser on ch32v208");
    try_println!("Clocks: {:?}", ch32_hal::rcc::clocks());
    try_println!("BLE lib: {}", ble::lib_version());

    let ble_config = ble::Config::default();
    try_println!("MAC: {}", ble_config.mac_addr);

    if let Err(e) = ble::init(ble_config) {
        panic!("ble::init: {:?}", e);
    }
    try_println!("ble::init done");
    if let Err(e) = broadcaster_init() {
        panic!("broadcaster_init: {:?}", e);
    }
    try_println!("broadcaster_init done");

    spawner.spawn(ble_task().unwrap());
    spawner.spawn(tick_task().unwrap());

    loop {
        Timer::after_secs(3600).await;
    }
}

ch32v208_ble::fault_handlers!();

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    ch32_hal::try_println!("\n\n\n{}", info);
    loop {}
}
