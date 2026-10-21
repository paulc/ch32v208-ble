use std::env;

fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlink.x");

    // WCH BLE library (CH32V20x_BLE_LIB, from openwch/ch32v20x EVT/EXAM/BLE/LIB)
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search=native={manifest_dir}/vendor");
    println!("cargo:rerun-if-changed=vendor/libwchble.a");
    println!("cargo:rerun-if-changed=build.rs");
}
