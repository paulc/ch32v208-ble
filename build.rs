use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg-bins=-Tlink.x");

    // WCH BLE library (CH32V20x_BLE_LIB, from openwch/ch32v20x EVT/EXAM/BLE/LIB)
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    println!("cargo:rustc-link-search=native={manifest_dir}/vendor");
    println!("cargo:rerun-if-changed=vendor/libwchble.a");

    // The library uses C common symbols, which qingke-rt's link.x leaves outside
    // the zeroed .bss range (_sbss.._ebss). Collect them in their own section;
    // ble::init zeroes it before initialising the library.
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
    println!("cargo:rustc-link-arg-bins=-Tble_common.x");

    println!("cargo:rerun-if-changed=build.rs");
}
