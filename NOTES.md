# Notes: SSH server + Ethernet + serial + BLE on CH32V208

Goal: run the [sunset](https://github.com/mkj/sunset) SSH server over
Ethernet (embassy-net, built-in 10M MAC+PHY), plus a serial port and BLE
(this crate), on one CH32V208WBU6.

Conclusion: **feasible**. Flash is comfortable, RAM is tight but workable for
one SSH session, and the system clock has to move from 96MHz to 120MHz (not
yet tested with BLE). Figures marked "measured" come from this repo's release
build; the rest are estimates.

## Flash (480KB total, first 128KB zero-wait)

| Component | Size |
|---|---|
| Current `ble_peripheral` (libwchble ≈ 144KB + Rust/embassy/HAL ≈ 21KB + vectors/data) | ≈ 173KB (measured) |
| embassy-net + smoltcp + 10M ETH driver | ≈ 30–40KB extra (ch32-hal `eth_echo` is ≈ 48KB including embassy/HAL/core already counted) |
| sunset (SSH + x25519, ed25519, chacha20-poly1305, sha256) | ≈ 80–110KB (sunset's Pico W demo is ≈ 150KB *including* embassy-net and the cyw43 driver, excluding wifi firmware) |
| UART | a few KB |
| **Total** | **≈ 300–330KB** |

Background:
- `cargo size` reports "text" as `.vector_table` + `.highcode` + `.text` +
  `.rodata`; the flashed image also includes `.data`'s load copy.
- The size is dominated by `libwchble.a`: `GAP_ProcessEvent` and other
  dispatch tables reference extended/periodic advertising, scanning/central,
  SMP/bonding and SNV, so none of it can be garbage collected. WCH's own C
  peripheral example is similar in size.
- Only timing-critical code must be in zero-wait flash: BB/LLE IRQ handlers,
  the ecall handler and the library's `.highcode`. `ble-link.x` places
  `.highcode` first (0x514–0x33E0); with `--features highcode` it runs from RAM
  instead. Everything else (including SSH crypto) can run from the wait-state
  region: handshakes cost tens of ms, and chacha20 is far faster than 10Mbit.

## RAM (64KB, cannot be increased)

| Component | Size |
|---|---|
| BLE (7KB library heap + other `.bss`/`.data`) | ≈ 11.5KB (measured) |
| ETH packet buffers, 1514B each (`State<2,2>` ≈ 6KB, `State<3,2>` ≈ 7.5KB) | 6–8KB |
| embassy-net `StackResources` + TCP socket buffers (2KB rx + 2KB tx) | ≈ 5KB |
| sunset, per concurrent session (upstream figure, likely incl. async state) | ≈ 13KB |
| Serial buffers | ≈ 1KB |
| Stack (single shared stack; ed25519 sign / x25519 peaks at a few KB) | 6–8KB |
| **Total** | **≈ 43–47KB, leaving ≈ 17–20KB** |

Constraints this implies:
- **One SSH session.** Two only after shrinking the BLE heap and ETH buffers.
- **Disable sunset's ML-KEM hybrid key exchange**; use plain curve25519.
  ML-KEM-768 needs several KB of extra buffers and stack.
- **Watch async task sizes.** embassy stores each task's future in static RAM;
  check `.bss` and task storage sizes (`cargo bloat`) after each change.
- **Don't trade RAM for zero-wait flash** via the V20x option bytes: it reduces
  RAM, which is the scarce resource here.

## Clock

The ch32-hal 10M ETH driver only accepts a 60MHz or 120MHz system clock
(`src/eth/v10m/mod.rs` returns `CoreFrequencyInvalid` otherwise). The BLE
examples run at 96MHz (EVT setting). With the 32MHz crystal, PLL `/4 × 15`
gives 120MHz (as in `src/bin/tick.rs`).

The radio is clocked directly from the HSE and the TMOS timebase comes from the
RTC (LSE), so BLE is expected to work at 120MHz, but this is **untested** and
is the first thing to verify.

## Timing / scheduling

- **Interrupt priorities:** BB/LLE/RTC_ALARM are at P8 and timing-critical
  (missed reply windows break scanning/connections). The ETH interrupt must be
  lower priority (numerically higher than P8) so it cannot delay them.
- **TMOS latency during crypto:** `ble::run()` is a cooperative embassy task.
  An ed25519/x25519 operation doesn't yield, so TMOS stalls for tens of ms
  during an SSH handshake. The link layer runs in interrupts, so connections
  survive; GATT responses are just delayed. If it matters, run the BLE task on
  an `InterruptExecutor` at higher priority.
- **No idle time:** `ble::run()` loops on `yield_now`, so other tasks only run
  between TMOS passes. Network/crypto throughput depends on passes being cheap;
  check here first if Ethernet throughput is poor.

## Toolchain

sunset requires Rust ≥ 1.95; this repo uses nightly (1.101), so no issue.

## Plan

1. **ETH + BLE at 120MHz.** Combine ch32-hal's `examples/ch32v208/src/bin/eth.rs`
   with `ble_peripheral`. Verify advertising, connect/read/write/notify, and
   DHCP + UDP echo, together. Set the ETH IRQ priority below P8.
2. **Add sunset**, single session, curve25519 + ed25519 + chacha20-poly1305,
   ML-KEM disabled. Verify login and an interactive channel; check handshake
   time and BLE behaviour during handshakes.
3. **Add serial (UART)**, exposed as an SSH channel and/or a BLE characteristic.

After each stage, measure:
- `cargo size` (`.bss`/`.data`) and `cargo bloat` for task storage sizes
- stack high-water mark (paint the stack with a pattern at startup, scan for
  the lowest overwritten address)
- TMOS pass rate (`ble::STATS`) under SSH load
