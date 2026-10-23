use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();

    if env::var("CARGO_FEATURE_HIGHCODE").is_ok() {
        // `highcode` feature: qingke-rt's link.x (link-highcode.x) runs .highcode
        // from RAM. The library's C common symbols are still left outside its
        // zeroed .bss range (_sbss.._ebss): collect them in their own section,
        // which ble::init zeroes.
        let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
        fs::write(
            out_dir.join("ble_common.x"),
            r#"SECTIONS
{
    .ble_common (NOLOAD) : ALIGN(4)
    {
        __ble_common_start = .;
        *(COMMON);
        . = ALIGN(4);
        __ble_common_end = .;
    } > RAM
}
INSERT AFTER .bss;
"#,
        )
        .unwrap();
        println!("cargo:rustc-link-search={}", out_dir.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        println!("cargo:rustc-link-arg-bins=-Tble_common.x");
    } else {
        // Default: qingke-rt's link.x adapted for the BLE library, with .highcode
        // first in zero-wait flash and COMMON in .bss (see the comments in
        // ble-link.x). It INCLUDEs memory.x, device.x and assert-align.x from
        // ch32-hal/qingke-rt.
        println!("cargo:rustc-link-search={manifest_dir}");
        println!("cargo:rustc-link-arg-bins=-Tble-link.x");
    }
    println!("cargo:rerun-if-changed=ble-link.x");

    // WCH BLE library (CH32V20x_BLE_LIB, from openwch/ch32v20x EVT/EXAM/BLE/LIB)
    println!("cargo:rustc-link-search=native={manifest_dir}/vendor");
    println!("cargo:rerun-if-changed=vendor/libwchble.a");

    println!("cargo:rerun-if-changed=build.rs");
}
