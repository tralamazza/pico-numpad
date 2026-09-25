# pico-numpad

Firmware and config tooling for a 16-key numpad built on a Raspberry Pi Pico 2 W
(RP2350).

The device presents itself as:

- a Bluetooth HID keyboard with three host slots
- a USB HID keyboard
- a WebUSB device exposing a config interface

Written in Rust using [Embassy](https://embassy.dev/) and
[Trouble](https://github.com/trouble-rs/trouble) for BLE.

## Behaviour

- Three Bluetooth host slots. A long-press on a slot key selects that slot and
  connects to it. Only one slot is connected at a time.
- When a USB cable is connected, key input is sent over USB regardless of the
  Bluetooth slot state.
- Pairing bonds, the active slot, and the keymap are stored in flash and survive a
  reboot.
- The keymap, brightness and LED mode are edited over WebUSB. Changes are written
  to the config region of flash and do not affect the firmware image or the bonds.
- Holding the `+` key for three seconds opens the slot menu. The `+` LED fills
  amber during the hold. In the menu, holding a slot key fills it amber, then red;
  releasing while amber selects the slot, releasing at red clears that bond.
- The backlight turns off after 60 seconds with no input, and comes back on the
  next keypress.
- Bluetooth advertising runs at 160 ms for the first 30 cycles after boot or a
  disconnect, then 800 ms.
- The key matrix is read on the I/O expander's interrupt line rather than polled.

## Hardware

| Item | |
|---|---|
| Board | Raspberry Pi Pico 2 W (RP2350, Cortex-M33) |
| Bluetooth | cyw43 (onboard), used for Bluetooth only |
| Keys | 4×4 matrix on a TCA9555 I²C I/O expander, address `0x20` |
| Backlight | 16 APA102 LEDs on SPI0 |
| Expander interrupt | GP3, pulled up |
| Debug | SWD + RTT |

Pin map and traced connections: [`firmware/README.md`](firmware/README.md).

## Requirements

- `rustup` — the toolchain is pinned in `rust-toolchain.toml` and installed on
  first build
- [`just`](https://github.com/casey/just)
- [`probe-rs`](https://probe.rs) on `PATH`, for flashing and RTT only.
  `just build` works without it.

## Flashing without probe-rs

`just uf2` produces a UF2 image, which the Pico 2 W's BOOTSEL bootloader takes
directly -- no debug probe and no `probe-rs` needed:

1. Hold `BOOTSEL` while plugging the USB in.
2. The board appears as a `RP2350` drive.
3. Drop `firmware/target/pico-numpad.uf2` onto it. The board reboots into the
   new firmware.

Prebuilt images are attached to each [tagged
release](https://github.com/tralamazza/pico-numpad/releases), and every CI run
uploads one as a workflow artifact. Each release also carries the matching
`.elf`: `defmt` indexes its log string table per build, so decoding RTT output
from a device needs the exact ELF that produced it.

The UF2 is tagged with the `rp2350-arm-s` family ID, which is what the BOOTSEL
bootloader matches on. `elf2uf2-rs` cannot produce this -- see the note in
[`.github/actions/setup-picotool/action.yml`](.github/actions/setup-picotool/action.yml).

## Commands

```sh
just build     # release (ship) image
just check     # fmt check + clippy + host tests
just size      # flash and RAM footprint of the ship image
just flash     # flash the diagnostic image over SWD and tail RTT
just serve     # WebUSB config editor at http://localhost:8080
```

`just --list` shows all recipes.

## Editing the keymap

The editor is hosted at <https://tralamazza.github.io/pico-numpad/>. Open it in
Chrome or Brave, click **Connect**, select the device.

To run it locally instead — offline, or while iterating on the editor:

```sh
just serve
```

then open `http://localhost:8080`. WebUSB requires a secure context; both HTTPS
and `localhost` qualify. USB access is granted per origin, so the Pages site and
localhost each need their own one-time grant.

## Media keys

Any key can be a media key. In the editor, switch the assignment panel from
**Keyboard** to **Media** and pick a control: play/pause, next and previous
track, stop, eject, mute, volume up and down, repeat, the menu controls, power
and sleep. Media keys are reported on the Consumer page (`0x0C`) instead of the
keyboard page, so they reach the operating system's media handling rather than
typing a character.

Sixteen controls are available, not any consumer usage. The set is fixed by the
HID descriptor, which declares each control individually — macOS will not route
a control it cannot see declared at parse time. The AL application-launch
usages (`0x18A` Calculator, `0x196` Internet Browser) are excluded for a
second reason: they are 16-bit and the config stores one byte per key.

Volume, mute and playback controls work on macOS and Linux. Windows is untested.
`firmware/README.md` explains why the descriptor is shaped the way it is and why
there is no per-host switch for it.

## Layout

```
firmware/     Rust firmware
  src/        embedded application
  tests/      host-side tests for the hardware-free logic
web/        single-file WebUSB config editor, no build step
tools/      macOS-side helpers: duplicate-input check, RTT capture, favicon
            generator, WebUSB editor selftest, hardware config round-trip
justfile    commands
```

`firmware/README.md` covers the flash storage layout and its failure modes, the
BLE link parameters observed on macOS, the routing rules, the hardware checks
that automation cannot cover, and measurements.

## Tests

The hardware-free logic — debouncing, slot state, menu transitions, routing,
config serialisation, checksums — is unit-tested on the host under `cargo test`.
Run everything with `just check`.

Not covered by automated tests: pairing with real hosts, the long-press gestures,
and battery life.

## License

Apache License 2.0 — see [LICENSE](LICENSE).

One exception, noted so it is not missed: `firmware/cyw43-firmware/*.bin` are
prebuilt Broadcom/Cypress binaries for the Pico 2 W's wireless module, shipped
under the Broadcom Permissive Binary License 1.0
([`firmware/cyw43-firmware/LICENSE-permissive-binary-license-1.0.txt`](firmware/cyw43-firmware/LICENSE-permissive-binary-license-1.0.txt)).
They are not Apache-2.0 and are not our code.
