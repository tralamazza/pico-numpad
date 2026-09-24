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
slot, BLE connect and pairing on RTT. Measured flash for this app (text+data):
`error` 668,196 / `warn` 677,344 / `info` 679,884 / `debug` 686,808 bytes, out
of a 4032 K region. Adjust `LOG_SHIP` in the
[`../justfile`](../justfile) if you want a different cut.

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
On the laptop, **forget the corresponding Bluetooth device** and connect it again.
The other two bonds and the key/LED configuration are unchanged.

**Forgetting it on the laptop is not optional.** A host cannot be *told* its bond
is gone: the central owns its bond store, and BLE gives a peripheral no way to
invalidate a pairing it does not hold. When a stale host-side pairing reconnects,
the numpad can only refuse, and the host sees a generic authentication failure
(trouble-host disconnects with `AuthenticationFailure`). macOS keeps the dead
pairing and retries it silently rather than prompting you to re-pair. SMP has no
"your stored bond is stale" reason code either — its reject reasons are all
pairing-time failures (confirm value, unsupported method, passkey, OOB, auth
requirements) — so there is nothing more informative to send. The spec's precise
answer, link-layer `LL_REJECT_IND_EXT` with `LL_ERROR_LTK_MISSING`, still leaves
invalidation to the host's policy.

To make that visible from the laptop rather than only from a serial log, **an
unbonded slot says so in its advertised name**: `pico-numpad-2-pairing` instead of
`pico-numpad-2`. If you see the `-pairing` suffix in the Bluetooth list, the
device is waiting to pair and the older entry under the unsuffixed name is the
stale one to forget. The suffix disappears once the slot holds a bond, and the
name is rebuilt on every advertisement so it tracks the current state. The GATT
Generic Access device name stays the stable identity; only the advertised name
varies.

A full bond reset (the recovery gesture below) clears all three slots at once, so
it can mean forgetting and re-pairing up to three laptops.

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
- A successful **retry** shows green and restarts. A successful **bond reset**
  pulses **blue** four times before restarting, so the two outcomes are
  distinguishable: blue means every previously paired host must now forget this
  device. A failed retry/reset stays in recovery; release all keys before another
  attempt.

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

All non-volatile state lives in the **reserved top 64 KiB** of the 4 MiB QSPI
flash. Every offset is derived in `src/config_store.rs`, so this table is the
arithmetic rather than a hand-maintained guess:

```
FLASH_SIZE    = 4 MiB
CONFIG_OFFSET = FLASH_SIZE - 64 KiB            = 0x3F0000
SECTOR        = ERASE_SIZE (embassy-rp 0.10)   = 4096
BOND_OFFSET   = CONFIG_OFFSET + SECTOR         = 0x3F1000   BOND_LEN  = 8 KiB
HOSTS_OFFSET  = BOND_OFFSET + BOND_LEN        = 0x3F3000   HOSTS_LEN = 8 KiB
```

| Region | Relative | Absolute | Size | Use |
| --- | --- | --- | --- | --- |
| Config | `0x3F0000` | `0x103F0000` | 4 KiB | Key/LED configuration (exactly one erase sector) |
| Legacy bond | `0x3F1000` | `0x103F1000` | 8 KiB | Pre-three-slot single bond. Migration source; erased by an explicit recovery reset |
| Slot journal | `0x3F3000` | `0x103F3000` | 8 KiB | Versioned three-slot record + selected slot |
| *(slack)* | `0x3F5000`–`0x400000` | `0x103F5000`–`0x10400000` | 44 KiB | Unused |

### The invariant that keeps code off your pairing data

`memory.x` caps the firmware at `FLASH : ORIGIN = 0x10000000, LENGTH = 4032K`,
which ends at `0x103F0000` — **exactly where the config sector begins**. That is
deliberate, and it is the only thing standing between a large link and your
pairing material.

**Never raise that `LENGTH` to 4096K.** The linker would then place code over the
storage region and flashing would overwrite the config, the legacy bond, and the
slot journal. The 64 KiB reservation is a deliberate cost: the ship image is
~665 KiB against a 4032 KiB budget, so there is no pressure to reclaim it.

### What actually gets written

Dumping a live backup and scanning for non-erased (non-`0xFF`) bytes confirms the
firmware writes nothing outside the declared regions:

```
0x103F0000..0x103F0020     32 bytes   config
0x103F1000..0x103F1074    115 bytes   legacy single bond
0x103F3000..0x103F31C4    452 bytes   slot journal
```

The regions look sparse because `sequential-storage` appends instead of rewriting
in place; the free space absorbs wear and lets records grow.

### Migration and clearing

The whole slot record is saved together via `sequential-storage`. Migration writes
the new journal *before* using it, leaving the old bond intact, so an interrupted
migration cannot lose both. Once a journal record exists, clearing slot 1 cannot
resurrect the legacy bond on next boot. Downgrading to the old firmware still
reads the old single-bond region.

### Back up before destructive storage tests

The relative offsets above are from the flash base at `0x10000000`; the whole
reserved region is `0x103F0000..0x10400000` in absolute terms. The `just` recipes
dump and restore **all 64 KiB** in one shot over SWD — the entire reservation, not
just the ~600 bytes in use, so a restore cannot miss a region someone forgot to
name:

```sh
just backup-storage    # dump 64 KiB from 0x103F0000 -> pico-numpad-config-backup.bin
just verify-storage    # re-read the region and diff it against the file
just restore-storage   # write the file back and verify (refuses if no backup exists)
```

The backup contains your **real pairing material**. It is gitignored; keep it off
shared drives and treat it like a key. `restore-storage` deliberately refuses to
run when the backup file is missing, so you cannot restore over a state you never
captured.

**Stop every other `probe-rs` session first.** Only one session can hold the debug
probe, so a lingering `probe-rs run` — an RTT capture from `just flash`, for
example — makes the download fail or hang.

`probe-rs write` **cannot** restore flash; it only accepts RAM addresses. Use
`probe-rs download --binary-format bin --base-address 0x103F0000`, which is what
the recipe runs.

Verified on hardware: a read → download → re-read round-trip of the whole 64 KiB
came back byte-identical, and the device afterwards reported
`config loaded from flash` / `host slots loaded; active=1` and re-established its
encrypted slot-1 bond. Restoring is also the quickest recovery when a bond reset
has left hosts holding bonds the device no longer has — see
[*Force re-pairing*](#force-re-pairing).

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

CI runs `just check` plus a ship build on every push and pull request
([`../.github/workflows/ci.yml`](../.github/workflows/ci.yml)), using the pinned
toolchain. It cannot cover the flash path, so the hardware checks in
[`../PLAN.md`](../PLAN.md) are still on you.

### Duplicate-input check (macOS)

`tools/dupcheck.swift` taps the HID input reports of every pico-numpad device on
the host (`vendor 0x2e8a`) through `IOHIDManager`, tags each report with its
transport, and fails if the same key set arrives on **both** USB and BLE. Reports
are compared by key signature rather than raw bytes, so a differing report ID or
report length between the two transports cannot hide a real duplicate.

```sh
just dupcheck-selftest   # verify the detection logic on synthetic events
just dupcheck 30         # capture 30s; type each key once, then hold a few
```

`dupcheck-selftest` is portable to any machine with `swiftc` and needs no device.
`dupcheck` needs macOS **Input Monitoring** permission for your terminal app
(System Settings > Privacy & Security > Input Monitoring); without it no reports
arrive.

**A PASS requires both transports to have been observed.** The tool returns
`INCONCLUSIVE`, never `PASS`, when: nothing was captured; only one transport
produced reports; or both transports reported but no key press was observed,
only empty/release reports.

**How to actually run it.** With USB connected the firmware routes every key to
USB and sends **none** to BLE (`routing.rs`: `Destination::Usb => (keys, 0)`), so
a USB-only capture is the *expected correct* result -- but the host cannot tell
that apart from a dead BLE link or a blind capture. Verify across two sources:

1. Attach RTT and confirm `connected on host slot N`, so the BLE link is
   genuinely up while you type.
2. `just dupcheck 30` with USB plugged in, and type. Expect reports on `USB` only.
3. **Unplug USB and type again.** Reports should now appear on
   `Bluetooth Low Energy`. This is the step that proves the harness can see BLE
   at all, which is what makes step 2's silence meaningful rather than
   ambiguous.
4. Replug USB and type: back to `USB` only, with no key on both.

A `PASS` from the tool means both transports carried reports in the same window
and none matched -- a state the firmware should never produce. Steps 2-4 are the
real check; the tool's own verdict cannot reach PASS in the normal USB-priority
state, by design.

Hardware checks: migrate/reconnect slot 1; pair slots 2 and 3; switch between
laptops; power-cycle and reconnect; clear one slot and verify the others remain
green and reconnect; verify normal numeric keys, short `+`, and menu cancellation.
Recovery fault injection (invalid journal, read/write failure, and interrupted
reset) still requires a disposable test device or a backed-up flash image; the
host tests exercise gesture logic, not actual flash failure behavior.
