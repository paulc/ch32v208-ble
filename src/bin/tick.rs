#![no_std]
#![no_main]

use ch32_hal::println;
use ch32_hal::rcc::*;
use ch32_hal::time::Hertz;

use embassy_executor::Spawner;
use embassy_time::Timer;

#[embassy_executor::main(entry = "ch32_hal::entry")]
async fn main(_spawner: Spawner) -> ! {
    ch32_hal::debug::SDIPrint::enable();

    let mut config = ch32_hal::Config::default();
    config.rcc = Config {
        hse: Some(Hse {
            freq: Hertz(16_000_000),
            mode: HseMode::Oscillator,
        }),
        sys: Sysclk::PLL,
        pll_src: PllSource::HSE,
        pll: Some(Pll {
            prediv: PllPreDiv::DIV4,
            mul: PllMul::MUL15,
        }),
        pllx: None,
        ahb_pre: AHBPrescaler::DIV1,
        apb1_pre: APBPrescaler::DIV1,
        apb2_pre: APBPrescaler::DIV1,
        ls: LsConfig::default_lsi(),
        hspll_src: HsPllSource::HSI,
        hspll: None,
    };
    let _p = ch32_hal::init(config);

    println!("Hello from ch32v208!");
    println!("CHIP: {}", ch32_hal::signature::chip_id().name());
    println!("Clocks: {:?}", ch32_hal::rcc::clocks());

    let mut tick = 0u32;
    loop {
        Timer::after_millis(1000).await;
        tick += 1;
        println!("tick: {}", tick);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    let _ = ch32_hal::println!("\n\n\n{}", info);
    loop {}
}
