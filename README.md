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
| `src/ble/mod.rs` | `Config`, `init()` (= EVT `WCHBLE_Init` + `HAL_Init`), `run()` (TMOS loop), `STATS`, `MacAddress` |
| `src/ble/ffi.rs` | Hand-written bindings for `wchble.h`: library init, TMOS, status codes |
| `src/ble/gap.rs` | GAP bindings: parameters, events, Broadcaster/Peripheral roles, bond manager |
| `src/ble/gatt.rs` | GATT server bindings: attribute tables, services, CCC, notifications, GGS, linkDB |
| `src/ble/irq.rs` | `BB` and `RTC_ALARM` handlers; `LLE` + ecall handlers ported from `vendor/ble_task_scheduler.S` |
| `src/ble/rtc.rs` | RTC on LSE (16384Hz counter) as the TMOS timebase, and its alarm |
| `src/fault.rs` | `fault_handlers!()`: print exceptions (`mcause`/`mepc`/`ra`) and unexpected interrupts over SDI |
| `src/bin/ble_adv.rs` | Scannable advertiser (EVT `Broadcaster`) |
| `src/bin/ble_peripheral.rs` | Connectable peripheral with a GATT service (EVT `Peripheral`) |
| `src/bin/tick.rs` | Minimal embassy test application (no BLE) |
| `build.rs` | Links the library; adds the `ble_common.x` linker fragment |
| `util/wlink-remote` | Cargo runner: flashes via `wlink` on a remote host over ssh |

## Usage

```rust
let mut config = ble::Config::default(); // factory MAC, 0dBm, 1 peripheral + 1 central
config.mac_addr = ble::MacAddress::random(); // optional, e.g. to avoid cached names
ble::init(config)?;
// GAP role / GATT setup (see the examples), then:
spawner.spawn(ble_task().unwrap()); // async task calling ble::run().await
```

`ble::STATS` counts TMOS passes and BB/LLE/deferred/RTC alarm interrupts, and
prints with `{}`.

## Design

### TMOS driven from embassy

`ble::run()` runs in an embassy task: it calls `TMOS_SystemProcess()` and
yields, in a loop, so other tasks run between passes (the WCH EVT without
sleep does the same from `main`).

GAP/GATT callbacks run inside TMOS, in the `ble` task. The library is not
re-entrant, so call it from thread level only (embassy tasks or TMOS
callbacks); all tasks share one cooperative executor, so calls never overlap.
In `ble_peripheral` the callbacks only update attribute values and post events
to an embassy `Channel`; an `app` task handles them and sends notifications.

### Interrupt-driven controller (`irqEnable`)

`bleClockConfig_t::irqEnable` (marked reserved in the header, 0 in the EVT)
selects how the link layer runs:

- 0: polled from `TMOS_SystemProcess`. Replies within the 150us inter-frame
  space (scan responses, connection events) then depend on TMOS being called
  back-to-back; through the embassy executor they were missed.
- 1 (used here): the LLE interrupt defers link-layer work to thread level
  immediately (see below), and connection events are timed from the RTC alarm
  interrupt. The library pends `RTC_ALARM` itself; the handler calls
  `TMOS_TimerIRQHandler(&time)` and programs the alarm for `time`.
  `TMOS_TimerInit` then allows at most 2 connections in total.

### LLE interrupt / ecall

The library's LLE handler can defer controller work to thread level: it
`mret`s into an internal function that ends with `ecall`, and the ecall handler
restores the interrupted context from the LLE handler's stack frame. So:

- `LLE` and `MachineEnvCall` (core vector 5) are assembly, ported from WCH's
  `ble_task_scheduler.S`, and override qingke-rt's defaults.
- `LLE_IRQLibHandler` must be called straight from that assembly (no Rust
  wrapper), because the library relies on `sp` pointing at the saved frame.
- No Rust in the ecall handler: qingke's critical section clears MPIE when MIE
  is 0 (as in an exception) and does not restore it, so the `mret` would leave
  interrupts disabled. The only additions are counter increments.
- BB/LLE/RTC_ALARM run at priority P8, the same preemption level as the embassy
  SysTick (P15), so they never nest and the deferred return never happens
  inside another handler.

### Not used: the idle callback

The library's `idleCB` reports when TMOS is idle until a given time, which
could let `run()` sleep. But around the callback the library powers down part
of the RF (RFEND) with interrupts enabled, which races with the interrupt-driven
controller: connectable advertising stalled. The executor spins anyway, so it
gained nothing; low power needs a proper sleep integration.

### Library common symbols

`libwchble.a` keeps 86 variables as C common symbols. qingke-rt's `link.x`
only zeroes `.sbss`/`.bss` (`_sbss.._ebss`), and lld appends `COMMON` after
that, so after a warm reset (e.g. wlink flash) the library started with stale
state from the previous firmware (seen as a jump through a stale function
pointer). `build.rs` adds `ble_common.x`, which collects `COMMON` into
`.ble_common`, and `ble::init` zeroes it first.

## Hardware assumptions

- 32MHz HSE crystal. On the V208 the PLL "/4" divides the full 32MHz, so
  `hse.freq` must be 32MHz (the examples use /4 × 12 = 96MHz, as in the WCH EVT)
- 32.768kHz LSE crystal (`ble::init` starts it; fails with
  `Error::Rtc(LseNotReady)` otherwise)

## Running

The cargo runner (`.cargo/config.toml`) is `util/wlink-remote`, which copies
the ELF over ssh to `$WLINK_HOST` (default `192.168.64.1`) and runs
`wlink flash` there, showing SDI output (`ENABLE_SDI=0` to disable). For a
locally attached probe use the commented-out `wlink` runner instead.

```
cargo run --release --bin ble_adv
cargo run --release --bin ble_peripheral
```

### ble_adv

Advertises scannable (not connectable) as `ch32v208` with WCH manufacturer
data and scan response `Broadcaster`, every 100ms. Prints `tick: n` with
`ble::STATS` every 5s.

Tested: advertising and scan response seen in nRF Connect and `blescan`.

### ble_peripheral

Advertises as `ch32v208 Peripheral` with service `0xFFE0`:

- `0xFFE1` "Data": read/write, up to 20 bytes (writes are logged)
- `0xFFE2` "Counter": read/notify, u32 LE, incremented every second and
  notified while notifications are enabled

Logs connections (with peer address), disconnections, MTU and parameter
updates, writes and notification state; prints `ble::STATS` every 10s.
Advertising restarts after a disconnect.

Tested with `blescan` (macOS): enumerate, read, write and notify work;
connections run at a 30ms interval with ~33 RTC alarm interrupts/s.
macOS caches device names per address, so a scanner may show an old name
(`MacAddress::random()` avoids this).

## Not done yet

- SNV (bond storage in flash), temperature-triggered RF calibration (`tsCB`)
- LSI-only operation (needs `Calibration_LSI` porting)
- Low-power sleep
- Pairing/bonding (needs SNV), connection parameter update requests
- Central/observer roles, GATT client
