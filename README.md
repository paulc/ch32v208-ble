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
| `src/ble/gatt_server.rs` | Helper: services/characteristics as statics with read/write/subscribe callbacks; builds the attribute table, routes callbacks; `notify()`, `notify_all()`, `wait_subscribed()`. Module docs explain the manual process |
| `src/ble/adv.rs` | `AdvData` builder for advertising / scan response data |
| `src/ble/peripheral.rs` | Helper: Peripheral role setup (`start()`), connection events (`next_event()`, optionally scan requests); re-exports `AdvData` |
| `src/ble/irq.rs` | `BB` and `RTC_ALARM` handlers; `LLE` + ecall handlers ported from `vendor/ble_task_scheduler.S` |
| `src/ble/rtc.rs` | RTC on LSE (16384Hz counter) as the TMOS timebase, and its alarm |
| `src/fault.rs` | `fault_handlers!()`: print exceptions (`mcause`/`mepc`/`ra`) and unexpected interrupts over SDI |
| `src/bin/ble_adv.rs` | Scannable advertiser (EVT `Broadcaster`) |
| `src/bin/ble_peripheral.rs` | Connectable peripheral with a GATT service (EVT `Peripheral`), using the helpers |
| `src/bin/ble_peripheral_raw.rs` | The same peripheral wired up by hand with the raw bindings |
| `src/bin/tick.rs` | Minimal embassy test application (no BLE) |
| `build.rs` | Links the library; selects `ble-link.x`, or with the `highcode` feature qingke-rt's `link.x` plus a `ble_common.x` fragment |
| `ble-link.x` | Default linker script: qingke-rt's `link.x` with `.highcode` first and `COMMON` in `.bss` (see below) |
| `util/wlink-remote` | Cargo runner: flashes via `wlink` on a remote host over ssh |

## Usage

```rust
let mut config = ble::Config::default(); // factory MAC, 0dBm, 1 peripheral + 1 central
config.mac_addr = ble::MacAddress::random(); // optional, e.g. to avoid cached names
ble::init(config)?;
```

A peripheral with a GATT service, using `ble::gatt_server` and `ble::peripheral`
(see `src/bin/ble_peripheral.rs`):

```rust
static DATA: Characteristic = Characteristic::new(Uuid::u16(0xFFE1), GATT_PROP_READ | GATT_PROP_WRITE)
    .description("Data")
    .on_read(&|_conn, buf| { /* fill buf */ Ok(len) })
    .on_write(&|_conn, data| { /* use data */ Ok(()) });
static COUNTER: Characteristic = Characteristic::new(Uuid::u16(0xFFE2), GATT_PROP_READ | GATT_PROP_NOTIFY)
    .on_read(&read_counter)
    .on_subscribe(&|_conn, enabled| { /* ... */ });
static SERVICE: Service = Service::new(Uuid::u16(0xFFE0), &[&DATA, &COUNTER]);

let adv = AdvData::new()
    .flags(GAP_ADTYPE_FLAGS_GENERAL | GAP_ADTYPE_FLAGS_BREDR_NOT_SUPPORTED)
    .uuid16_list(&[0xFFE0])
    .short_name("ch32v208-per");
let scan_rsp = AdvData::new().name("ch32v208-peripheral");
peripheral::start(&peripheral::Config::new("ch32v208-peripheral", &adv, &scan_rsp), &[&SERVICE])?;
spawner.spawn(ble_task().unwrap()); // async task calling ble::run().await

loop {
    match peripheral::next_event().await {
        Event::Connected { handle, .. } => { /* ... */ }
        Event::Disconnected { .. } => { /* advertising restarts automatically */ }
        _ => {}
    }
}

// Notifications, e.g. from a task per data source (no connection tracking):
loop {
    gatt_server::wait_subscribed(&COUNTER).await?; // optional: pause while nobody listens
    Timer::after_secs(1).await;
    if !gatt_server::is_subscribed_any(&COUNTER) {
        continue;                                  // unsubscribed during the sleep
    }
    gatt_server::notify_all(&COUNTER, &value)?;    // every subscribed connection
}
```

`gatt_server::notify(conn, &ch, data)` notifies one connection;
`is_subscribed(conn, &ch)` / `is_subscribed_any(&ch)` check subscriptions.

Callbacks can be closures or `fn`s, and run inside TMOS (keep them short).
Limits: single-packet reads and writes, at most `MAX_ATTRIBUTES` (48) rows,
`MAX_SERVICES` (4) services and `MAX_NOTIFY` (8) notifying characteristics.
For the manual process (attribute table layout, callbacks, notifications),
see the `ble::gatt_server` module docs and `ble_peripheral_raw`.

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
In `ble_peripheral` the callbacks only update state and signal the main task,
which logs; `ble::peripheral` delivers GAP events through an embassy `Channel`
(`next_event()`), and a separate `counter_task` produces the notifications
(`wait_subscribed` / `notify_all`), independent of connections.

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

### Memory layout: `.highcode` and the library's common symbols

The library marks its time-critical code `.highcode` (BB/LLE interrupt paths,
RX/TX waits, connection events). Only the first 128KB of flash is zero-wait
(with 64KB RAM), and qingke-rt's plain `link.x` puts `.highcode` at the end of
`.text`, which in a ~170KB image is slow flash. The BB interrupt then missed
the 150us reply window: connectable advertising sent no scan responses (no name
in scanners).

`libwchble.a` also keeps 86 variables as C common symbols. qingke-rt's `link.x`
does not list `COMMON`, so lld put them after `_ebss`, where they were not
zeroed at reset: after a warm reset (e.g. wlink flash) the library started with
stale state from the previous firmware (seen as a jump through a stale function
pointer).

Two layouts handle both, selected by the `highcode` cargo feature:

| | Default | `--features highcode` |
|---|---|---|
| `.highcode` runs from | zero-wait flash, placed first (as WCH's `Link.ld`) | RAM, copied at startup (with the vector table and interrupt handlers) |
| Linker script | `ble-link.x`: a copy of qingke-rt's `link.x` | qingke-rt's `link.x` (ch32-hal/qingke-rt `highcode` feature) |
| Common symbols | `COMMON` in `.bss`, zeroed at reset | `ble_common.x` fragment (`INSERT AFTER .bss`), zeroed by `ble::init` |
| RAM | no extra | ~13KB extra |
| Zero-wait flash | ~12KB used by `.highcode` | freed |
| Maintenance | keep `ble-link.x` in sync with qingke-rt | follows qingke-rt |

Both are tested on hardware (scan responses, connections, GATT). TMOS ran
~1.8M passes/10s with the default and ~1.61M with `highcode`.

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
cargo run --release --bin ble_peripheral      # or ble_peripheral_raw
cargo run --release --bin ble_peripheral --features highcode   # .highcode in RAM
```

### ble_adv

Advertises scannable (not connectable) every 100ms: WCH manufacturer data and
short name `ch32v208-adv`, scan response with the complete name
`ch32v208-advertiser`. Prints `tick: n` with
`ble::STATS` every 5s.

Tested: advertising and scan response seen in nRF Connect and `blescan`.

### ble_peripheral

Advertises as `ch32v208-per` (short name) / `ch32v208-peripheral` (complete
name, in the scan response) with service `0xFFE0`:

- `0xFFE1` "Data": read/write, up to 20 bytes (writes are logged)
- `0xFFE2` "Counter": read/notify, u32 LE, incremented and notified every
  second while a client has notifications enabled (a `counter_task` using
  `wait_subscribed` / `notify_all`)

Logs connections (with peer address), disconnections, MTU and parameter
updates, writes and notification state; prints `ble::STATS` every 10s.
Advertising restarts after a disconnect.

`ble_peripheral_raw` provides the same service wired up by hand, advertising as
`ch32v208 Peripheral` (complete name in the scan response only); its counter
runs continuously and is notified to the connected client while subscribed.

Tested with `blescan` (macOS, both versions): scan response (name), enumerate,
read, write and notify work;
connections run at a 30ms interval with ~33 RTC alarm interrupts/s.
macOS caches device names per address, so a scanner may show an old name
(`MacAddress::random()` avoids this). CoreBluetooth reports a device on its
first advertising packet, before the scan response, then updates it: the
examples put a short name in the advertising packet (shown first) and the
complete name in the scan response (shown after the update).

## Not done yet

- SNV (bond storage in flash), temperature-triggered RF calibration (`tsCB`)
- LSI-only operation (needs `Calibration_LSI` porting)
- Low-power sleep
- Pairing/bonding (needs SNV), connection parameter update requests
- Central/observer roles, GATT client
