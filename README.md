# ch32v208-ble

BLE on the CH32V208 with ch32-hal/embassy, using WCH's BLE library directly.

## Library

`vendor/libwchble.a` (`CH32V20x_BLE_LIB_V1.4`) and `vendor/wchble.h` come from
[openwch/ch32v20x](https://github.com/openwch/ch32v20x) `EVT/EXAM/BLE/LIB`.
The CH58x library (`LIBCH58xBLE.a`) used by ch58x-hal cannot be used: it targets
the CH58x register map (radio at `0x4000_C000`, RTC in SYS), whereas the V208
radio is at `0x4002_4100`. The two libraries share the same API, so the
ch58x-hal bindings and wrappers are a good template, but some structs and
constants differ (`bleConfig_t`, `bleClockConfig_t`, `LL_TX_POWEER_*`).

## Structure

| File | Purpose |
|------|---------|
| `src/ble/ffi.rs` | Hand-written bindings for the subset of `wchble.h` used so far |
| `src/ble/irq.rs` | `BB` handler; `LLE` + ecall handlers ported from `vendor/ble_task_scheduler.S` |
| `src/ble/rtc.rs` | RTC on LSE (16384Hz counter) as the TMOS timebase |
| `src/ble/mod.rs` | `init()` (= EVT `WCHBLE_Init` + `HAL_Init`) and `run()` (TMOS loop) |
| `src/bin/ble_adv.rs` | Non-connectable advertiser (EVT `Broadcaster`) |

## How TMOS is driven from embassy

`ble::run()` runs in an embassy task and calls `TMOS_SystemProcess()` in a loop:

- When TMOS is idle it calls the library's `idleCB` with the RTC count of the
  next timed event. The callback records it and returns "not slept"; `run()`
  then awaits either that time (embassy `Timer`) or a wake signal from the
  BB/LLE interrupts.
- Otherwise `run()` yields and calls TMOS again.

The library is not re-entrant, so call it only before `run()` starts or from
TMOS callbacks. Other tasks that call into it should call `ble::wake()` after.

### LLE interrupt / ecall

The library's LLE handler can defer controller work to thread level: it
`mret`s into an internal function that ends with `ecall`, and the ecall handler
restores the interrupted context from the LLE handler's stack frame. So:

- `LLE` and `MachineEnvCall` (core vector 5) are assembly, ported from WCH's
  `ble_task_scheduler.S`, and override qingke-rt's defaults.
- `LLE_IRQLibHandler` must be called straight from that assembly (no Rust
  wrapper), because the library relies on `sp` pointing at the saved frame.
- The only addition is a call to `ble_irq_wake` to wake `run()`.

## Hardware assumptions

- 32MHz HSE crystal. On the V208 the PLL "/4" divides the full 32MHz, so `hse.freq` must be
  32MHz (the example uses /4 × 12 = 96MHz, as in the WCH EVT)
- 32.768kHz LSE crystal (`ble::init` starts it; fails with `Error::Rtc(LseNotReady)` otherwise)

## Running

```
cargo run --release --bin ble_adv
```

Expected SDI output: lib version, MAC, `Initialized..`, `Advertising..`, then
`tick: n` with `ble::STATS` every 5s. The device should appear in a scanner (e.g. nRF Connect)
as `ch32v208` with WCH manufacturer data, scan response `Broadcaster`.

### Tested (stage 1)

Advertising seen by a scanner at -56dBm. Typical stats per 5s: ~550 TMOS passes,
~280 idle waits, ~336 LLE interrupts (100ms interval), RTC +82190 (16384Hz).
`bb` stays 0 for the advertiser.

One early run never went idle (`idle=0`, ~150k TMOS passes/s) and was not
reproduced. BLE still worked, since `run()` falls back to yielding. If `idle`
stops increasing again, the library skips its idle step unless: LL state
`gBleIPPara[7]` <= 4, `rfInfo[0xC]` bit 2 clear, `tmosSign` == 0, and no TMOS
task has pending events.

## Not done yet

- SNV (bond storage in flash), temperature-triggered RF calibration (`tsCB`)
- LSI-only operation (needs `Calibration_LSI` porting)
- Low-power sleep
- Peripheral/central roles, GATT (ch58x-hal `gap.rs`/`gatt*.rs` to port)
