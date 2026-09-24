# Pico numpad firmware

## Prerequisites

- A **Pico 2 W** with the RGB keypad attached. This firmware targets RP2350 with
  4 MiB flash, not the original Pico/Pico W.
- An SWD debug probe supported by `probe-rs` (the tested setup uses a Raspberry Pi
  Debug Probe). Connect the probe's SWDIO, SWCLK, and GND to the matching Pico
  debug connections, connect the probe to your computer, and power the Pico.
  The Pico's own USB connector is used for keyboard/configuration access; the
  flash command below uses the separate SWD probe.
- Rust/Cargo managed by `rustup`. The toolchain is pinned in
  [`../rust-toolchain.toml`](../rust-toolchain.toml) (currently 1.98.1) and rustup
  installs the pinned compiler, the `thumbv8m.main-none-eabihf` target, and the
  `rustfmt`/`clippy` components automatically on the first build, so there is
  nothing to add by hand. The pin is load-bearing: the firmware builds against a
  git rev of embassy, and it also holds back a `proc-macro-error2` future
  incompatibility that has no upstream fix yet. Bump it deliberately.

- `probe-rs` on `PATH` for flashing and RTT, and `just` on `PATH` for the
  repository's build/check recipes. The tested probe-rs version is 0.32.0.
- For the optional web configurator: Python 3 to serve the editor locally and
  a WebUSB-capable browser such as Chrome or Edge.

## Build and flash

There are two release-grade profiles. They land in separate target directories, so
switching between them does not recompile the dependency tree.

| Profile | `DEFMT_LOG` | Use for | Artifact |
| --- | --- | --- | --- |
| `release` | `info` | the image you ship | `target/thumbv8m.main-none-eabihf/release/pico-numpad` |
| `diagnostic` | `debug` | bench work with full RTT tracing | `target/thumbv8m.main-none-eabihf/diagnostic/pico-numpad` |

From the repository root, the `just` recipes change into `firmware/` for you:

```sh
just build   # quiet ship image: release, DEFMT_LOG=info
just diag    # bench image: diagnostic, DEFMT_LOG=debug
just flash   # flash the bench image over SWD and tail RTT
just ship    # flash the quiet ship image over SWD
```

The ship profile drops the `debug!` flood (cyw43 HCI `rx`/`tx`, embassy internals)
but keeps `info` and above, so a shipped device still reports config load, active
slot, BLE connect and pairing on RTT. Measured flash for this app: `error`
708,200 / `warn` 717,364 / `info` 719,904 / `debug` 726,788 bytes. Adjust
`LOG_SHIP` in the [`../justfile`](../justfile) if you want a different cut.

Run Cargo commands from **`firmware/`**, so Cargo picks up its `.cargo/config.toml`
with the embedded target and probe runner. Note that a bare Cargo invocation picks
up `DEFMT_LOG = debug` from that config for **every** profile, so a plain
`cargo build --release` is debug-logged and is *not* the same artifact as
`just build`:

```sh
cd firmware
cargo build --profile diagnostic   # bench image, full tracing
DEFMT_LOG=info cargo build --release   # ship image
```

With the powered Pico and SWD probe connected, flash and verify from `firmware/`:

```sh
cargo run --profile diagnostic -- --verify
```

The Cargo runner flashes the Pico 2 W over SWD using `probe-rs` and reads RTT
logs. Allow roughly 35 seconds for flashing/verification. Stop it with Ctrl-C;
the keyboard continues running without the probe. To attach without flashing,
point at the ELF for the profile that is actually on the board:

```sh
probe-rs attach --chip RP235x --no-catch-reset target/thumbv8m.main-none-eabihf/diagnostic/pico-numpad
```

Use the exact ELF that is on the board for RTT decoding; defmt indexes its string
table per build, so the wrong ELF decodes log lines as unrelated strings. Run
only one probe-rs session at a time; stop an existing RTT reader before starting
another flash or attach command.

## USB keyboard and Bluetooth routing

The USB data connection exposes a standard report-protocol HID keyboard alongside
WebUSB configuration. No keyboard driver or browser connection is required.
The editor keeps its existing vendor interfaces and bulk endpoints.

- **USB takes priority** while a computer has configured the USB device and the
  USB bus is not suspended. BLE may remain connected, but receives released-key
  reports rather than duplicate typing.
- Without an active USB connection (including a power-only cable/charger), input
  goes to the selected BLE host. USB suspend also permits BLE input; USB remote
  wake is not implemented.
- The Pico cannot determine whether USB and BLE lead to the same laptop. Thus a
  USB connection to laptop A takes priority over a BLE connection to laptop B.
- Held keys must be released before they can type on a newly selected transport.
  Keymaps, debounce, the short `+` tap, and host-menu controls are shared.
- Switching BLE slots still reboots the board, briefly reconnecting USB too.
- Storage-recovery mode retains USB configuration, but does not type its recovery
  gestures as keyboard input.

## Configure keys and LEDs over WebUSB

1. Connect the Pico's own USB port to the computer with a data-capable cable.
   The debug probe's USB connection alone does not expose the configurator.
2. From the repository root, serve the editor:

   ```sh
   python3 -m http.server 8080 --bind 127.0.0.1 --directory web
   ```

3. Open <http://localhost:8080> in Chrome or Edge. Use localhost (or HTTPS when
   hosting elsewhere), not a `file://` URL; the editor requires a secure context.
4. Click **Connect** and select **pico-numpad** in the browser's USB picker.
5. Edit the key mappings, LED mode, or brightness, then click **Save to flash**.
   Wait for **Saved to flash.** before unplugging or restarting the Pico.

Editing the page alone does not update the device. **Save to flash** first applies
those settings to RAM, then persists them. A protocol-level apply (`SET_CONFIG`)
alone is temporary; if the save fails after applying, the running settings may
have changed without being persisted.

**Reset defaults** immediately resets the running key/LED configuration but does
not save it. Click **Save to flash** afterward to keep those defaults across a
restart. **Reload** reads the device's current running configuration, not a
separate copy from flash. These controls never clear Bluetooth pairings.

The editor is optional: USB keyboard input works without a browser. Closing the
editor does not disable USB keyboard priority.

## Three Bluetooth host slots

Each slot remembers one laptop. Only the selected slot can connect, and only
one laptop is connected at a time. Slots have distinct BLE identities:

| Slot | Bluetooth name |
| --- | --- |
| 1 | `pico-numpad` |
| 2 | `pico-numpad-2` |
| 3 | `pico-numpad-3` |

The existing single-host pairing is migrated to slot 1 on the first boot of this
firmware. Bonds and the selected slot survive power cycles and normal reflashes.

### Open the host menu

Hold the **physical bottom-right `+` key for three seconds**, then release it.
The physical `1`, `2`, and `3` keys (third row, first three columns) show the slots:

- **Green:** a saved pairing exists.
- **Blue:** empty; ready to pair when selected.
- **Pulsing green/blue:** currently selected slot, retaining its bond-status color.
- **Amber:** a slot key is being held; release to select, or keep holding to clear.

The menu is visible even with normal backlighting disabled. These controls use
physical key positions, regardless of custom key mappings.

### Switch laptops

In the menu, **tap and release `1`, `2`, or `3`**. Changing slots saves the
selection and briefly restarts the Pico, including USB, to cleanly switch BLE
identity. Release all keys; held keys are ignored until released after startup.

- For an **empty** slot, select its Bluetooth name on the desired laptop and pair.
- For a **bonded** slot, reconnect from its paired laptop if it does not reconnect
  automatically. A different laptop cannot replace that slot's bond silently.

### Force re-pairing

In the menu, **hold `1`, `2`, or `3` continuously for three seconds**. This clears
only that slot's bond, selects it, and restarts into pairing mode. Release the key.
On the laptop, forget the corresponding Bluetooth device and connect it again.
The other two bonds and the key/LED configuration are unchanged.

### Storage-fault recovery

If the saved host journal cannot be read or has an unsupported format, the
firmware starts a separate recovery mode instead of panicking:

- The whole keypad pulses **red**, or amber while keys are pressed.
- BLE stays off; USB configuration remains available, even if the radio has not
  been initialised. No guessed/empty pairing record is used.
- Release all keys, then **hold `+` for three seconds** to retry loading the
  existing data. This does not request a bond reset.
- To deliberately discard unreadable pairing data, release all keys, then hold
  the physical **`1` + `2` + `3` keys together for five seconds**. This recovery-only
  gesture resets **all three bonds** and selects slot 1. It does not erase the
  keymap or LED configuration. It differs from the normal menu's single-slot clear.
- Successful recovery briefly shows green and restarts. A failed retry/reset
  stays in recovery; release all keys before another attempt.

Recovery does not automatically erase storage. Explicit reset erases the legacy
single-bond region before the new journal, preventing the old bond from being
imported again if power is interrupted between those operations.

The web configurator's reset button resets key/LED settings, not Bluetooth slots.
Power-cycling does not clear bonds. Avoid chip-wide erase if you want to keep them.

### Cancel / normal typing

Press another key in the menu to cancel, or leave the menu idle for ten seconds.
Menu keys are not sent to the laptop. A **short `+` tap** still emits that key's
configured usage, but on release; holding `+` alone is reserved for the menu.
Pressing `+` together with another key before entering the menu types normally.

All physical keys are independently debounced for 20 ms before typing or menu
handling. Disabled mappings do not consume HID report entries, and duplicate
mappings share one entry until all physical keys mapped to that usage are released.

### HID compatibility / upgrading

The keyboard supports **Report Protocol Mode only** and no longer exposes the
optional Protocol Mode characteristic for unsupported boot-mode switching.
Notifications remain encrypted eight-byte keyboard reports without an ID prefix.
Removing the characteristic changes GATT handles for the subsequent Device
Information service. If an already paired laptop uses stale cached services after
this update, clear its slot using the normal host menu, forget the matching
Bluetooth device on that laptop, and pair it again. Other slots can remain paired.

If saving a slot action fails, the LEDs flash red and the old selection/bonds
remain in use; the Pico does not reboot into unsaved state.

## Storage

All storage lies in the reserved top 64 KiB of the 4 MiB flash:

| Flash offset | Use |
| --- | --- |
| `0x3f0000..0x3f1000` | Key/LED configuration |
| `0x3f1000..0x3f3000` | Legacy single bond (migration source; erased by explicit recovery reset) |
| `0x3f3000..0x3f5000` | Versioned three-slot journal and selected slot |

The entire slot record is saved together using `sequential-storage`. Migration
writes the new journal before using it, leaving the old bond intact. Once a new
record exists, clearing slot 1 cannot resurrect the legacy bond on next boot.
Downgrading to the old firmware will still use the old single-bond region.

## Lint and tests

From the repository root, `just check` runs everything that can be verified
without hardware in the current gate: `cargo fmt --check`, clippy, and the unit tests.

```sh
just check          # fmt-check + clippy + unit tests
just lint           # firmware clippy (embedded target) + standalone modules
just test           # unit tests for the hardware-free logic
```

Clippy levels are configured in `Cargo.toml` (`[lints.rust] unsafe_code = "forbid"`,
`[lints.clippy] all = "deny", pedantic = "deny"`), so a plain `cargo clippy`
fails on enabled Clippy all/pedantic lints. Cast lints are not blanket-allowed: the two modules that
narrow deliberately (`config_store.rs` flash offsets, `host_slots.rs` slot ids)
carry a scoped `#![allow(clippy::cast_possible_truncation)]` with the reason.

`just test` compiles the hardware-free modules directly with `rustc`: host-slot
controls (`host_slots.rs`), HID reports (`hid.rs`), debounce (`debounce.rs`),
recovery gestures (`recovery.rs`), and USB/BLE routing (`routing.rs`). Routing
coverage includes USB priority, held-key handover, and disconnected-input handling.

`just clippy-host` lints these modules through `clippy-driver`, explicitly passing
the lint levels because standalone compilation does not read `Cargo.toml`.
Use the recipes in [`../justfile`](../justfile) as the authoritative command list.

Hardware checks: migrate/reconnect slot 1; pair slots 2 and 3; switch between
laptops; power-cycle and reconnect; clear one slot and verify the others remain
green and reconnect; verify normal numeric keys, short `+`, and menu cancellation.
Recovery fault injection (invalid journal, read/write failure, and interrupted
reset) still requires a disposable test device or a backed-up flash image; the
host tests exercise gesture logic, not actual flash failure behavior.
