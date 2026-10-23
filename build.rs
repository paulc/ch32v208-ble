use std::env;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    // Linker script: qingke-rt's link.x adapted for the BLE library (see the
    // comments in ble-link.x). It INCLUDEs memory.x, device.x and assert-align.x
    // from ch32-hal/qingke-rt.
    println!("cargo:rustc-link-search={manifest_dir}");
    println!("cargo:rustc-link-arg-bins=-Tble-link.x");
    println!("cargo:rerun-if-changed=ble-link.x");

    // WCH BLE library (CH32V20x_BLE_LIB, from openwch/ch32v20x EVT/EXAM/BLE/LIB)
    println!("cargo:rustc-link-search=native={manifest_dir}/vendor");
    println!("cargo:rerun-if-changed=vendor/libwchble.a");

    println!("cargo:rerun-if-changed=build.rs");
}
