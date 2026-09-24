# Firmware review fixes

Baseline: `893b405` (host slots and Clippy gate). Keep all work Clippy-clean.
This document is cumulative: the sections below record the original review fixes,
then the USB keyboard follow-up, then the build-hygiene and tooling pass that
brought the repo to its current state.

## Scope and acceptance criteria

- [x] **Storage recovery:** replace the boot-time panic on host-journal errors
  with an explicit recovery mode. Keep USB configuration and debounced keypad
  controls available; do not enable BLE with invented/empty bonds or silently
  overwrite unreadable storage. Provide a retry and a deliberate, separately
  documented reset gesture. A reset must preserve key/LED configuration and
  prevent migration from resurrecting a legacy bond. Failed recovery stays usable.
- [x] **Debounce:** filter each physical key before both menu controls and HID
  reporting. Test press/release bounce, independent keys, and a bounced slot hold.
  Preserve the three-second menu entry and clear gestures and bond-status colors.
- [x] **Report-only HID:** remove the unsupported Protocol Mode characteristic;
  retain encrypted eight-byte Report notifications. Document that existing hosts
  may need to forget/re-pair after this GATT layout change.
- [x] **Report construction:** skip disabled (`0x00`) mappings and deduplicate
  ordinary usages without consuming six-key capacity; preserve modifier handling.
  Add regression tests for disabled keys, duplicates, and releases.
- [x] **Validation/docs:** extend the standalone test/Clippy gate for new pure
  logic, run `just check` and `just build`, and document recovery and behavior.
  Do not erase real pairing data to inject a storage fault during validation.

## Implementation order

1. Add a tested debouncer and recovery-control state machine.
2. Connect storage-error boot recovery and explicit journal reset/retry.
3. Correct HID report building and remove unsupported protocol switching.
4. Run all gates, review changes, update this plan with results and any remaining
   hardware checks. Keep commits/flash deployment separate from code validation.

## Results

- Implemented all four review fixes; no storage-format change or automatic bond
  erase was introduced.
- Recovery pulses red, keeps USB working, and does not start BLE. Hold `+` for
  three seconds to retry. Only in recovery, hold physical `1`+`2`+`3` for five
  seconds to reset all bonds; release is required before repeating any action.
- Independent 20 ms press/release debounce feeds all physical controls.
- Regression tests cover disabled and duplicate usages, modifier preservation,
  bounce during clear-slot holds, boot-held keys, and interrupted/repeated
  recovery gestures. A repeated-gesture timer bug found by the tests was fixed.
- `just check`: PASS (formatting, firmware/host Clippy, all test suites).
- `just build`: PASS. Only the pre-existing dependency future-compatibility
  notice for `proc-macro-error2` remains.
- `git diff --check`: PASS.
- No live pairing data was erased during implementation or validation.
- Flashed with `just flash` (`probe-rs run --verify`); flash verification passed
  in 37.52 seconds. RTT confirmed configuration and active slot 1 loaded,
  encrypted reconnection, HID notification subscription and initial report,
  and USB configuration. Physical input/menu checks remain below.

## Remaining hardware validation

- [x] Flash, verify image, and check startup/encrypted BLE reconnection over RTT.
- [x] Check ordinary number input and short `+` (confirmed by user).
- [x] Check host-menu access and slot switching on the updated firmware
  (confirmed by user; exact hold timing not separately measured).
- [x] Check cached-service behavior on the existing Mac pairing: numbers, `+`,
  and slot switching work without re-pairing.
- [~] On a disposable/backed-up device, inject invalid journal/read/write faults:
  confirm USB/menu responsiveness, retry, explicit reset, and failure handling.
  **Boot-time failure handling verified** (see below). The retry and reset
  gestures still need physical key presses.
- [ ] Interrupt a recovery reset between flash operations and verify that legacy
  bonds never reappear and key/LED configuration remains intact. Needs a
  physical power-yank mid-gesture; the storage backup below makes it safe to try.

### Storage fault injection -- boot-time results (automated)

Backed up the full 64 KiB region (`just backup-storage`, round-trip verified,
three copies with matching SHA-256), then overwrote the 8 KiB slot journal at
`0x103F3000` with random data and observed boot over RTT:

```
0.000960 [INFO ] config loaded from flash                    key/LED config survived
0.001323 [ERROR] host storage recovery: host slot read failed  recovery, not panic
0.286442 [DEBUG] SET_CONFIGURATION: configured               USB still enumerated
```

- No panic and no hard fault.
- Zero cyw43/BLE log activity -- BLE was never started, so the device did not
  come up with invented or empty bonds.
- Key/LED config is in a separate sector and was unaffected.
- A second boot with the journal still corrupt behaved identically, so failed
  recovery is stable across reboots rather than degrading.
- `just restore-storage` returned the device to normal: `host slots loaded;
  active=1`, `connected on host slot 1`, `pairing complete: Encrypted`, and the
  region read back byte-identical to the backup.

- The `+` three-second retry and the `1`+`2`+`3` five-second bond reset were both
  exercised on hardware. The retry re-read the journal, failed while it was still
  corrupt, and left storage untouched. The bond reset wrote a fresh journal,
  restarted, and came back `bonded=false`. Diffing flash against the
  pre-corruption backup afterwards showed the **config sector byte-identical**,
  so the reset preserved key/LED config as required.
- The suspected "success logged as failure" in `ble.rs` -- the `warn!` sitting
  outside the `if restored` block -- did **not** occur: `restart()` diverges, so
  the failure line never follows a success. The block is now an explicit
  `if`/`else` so that ordering is obvious to a reader rather than load-bearing on
  a diverging call.

Still unverified: the interrupted-reset case in the item above, which needs the
power yanked mid-reset.

## Tell the user when hosts hold stale bonds

- [x] Advertise bond state: an unbonded slot advertises `pico-numpad-N-pairing`.
  The GATT Generic Access name stays the stable identity, and the advertised name
  is rebuilt on every advertisement so it tracks state within a session.
- [x] Distinguish the recovery outcomes on the LED: retry success pulses green,
  bond-reset success pulses blue, meaning "every host must now forget this
  device".
- [x] Document why the host cannot simply be told.

A host cannot be told its bond is gone. The central owns its bond store and a
peripheral cannot invalidate a pairing it does not hold; the numpad can only
refuse the reconnect, which trouble-host reports as `AuthenticationFailure`.
macOS keeps the dead pairing and retries it silently instead of prompting to
re-pair. SMP offers no stale-bond reason code -- its reject reasons are all
pairing-time failures -- and even the link layer's precise
`LL_REJECT_IND_EXT` / `LL_ERROR_LTK_MISSING` leaves invalidation to host policy.
The advertised name is therefore the only signal that reliably reaches a human.

Observed on hardware after the bond reset, when the old host tried to resume:

```
69.451017 [INFO]  connected on host slot 1
69.643217 [WARN]  [host] Long term key request reply failed, no long term key
69.703039 [ERROR] [security manager] Encryption event error Connection Terminated By Local Host
```

Covered by `unbonded_slots_advertise_that_they_need_pairing` and
`advertised_names_fit_the_advertising_payload` (22-byte name budget inside the
31-byte legacy advertising packet).


## USB keyboard follow-up

- [x] Add report-protocol USB HID alongside the existing WebUSB interfaces.
- [x] Share debounced input, mapping, and menu filtering between transports.
- [x] Use configured, non-suspended USB over BLE; never duplicate typing.
  USB priority confirmed by the user, including USB A / BLE B connections.
  The device cannot identify whether both transports terminate on the same host.
- [x] Release the old transport on handover and require held keys to be released
  before they type on the new transport. Discard stale USB reset/resume reports.
- [x] Add report framing/routing tests and include routing in host Clippy gates.
- [x] `just check`, `just build`, and `git diff --check` pass; composite USB
  firmware flashed with `--verify` (36.57 seconds).
- [x] macOS `hidutil` lists USB keyboard (usage page 1, usage 6) alongside BLE.
  Read-only GET_CONFIG succeeds on unchanged vendor interface 1 / EP1.
  Existing slot-1 bond reconnects with encryption; no pairing data erased.
- [x] Physically verify USB typing and BLE fallback after USB unplug
  (confirmed by user).
- [ ] Explicitly verify no duplicate input with USB and BLE both connected.
  `just dupcheck 30` (macOS, needs Input Monitoring) captures HID reports from
  both transports and fails if the same key set arrives on both. **A PASS only
  counts if the summary lists both `USB` and `Bluetooth Low Energy` with non-zero
  reports** -- a run that saw one transport alone is INCONCLUSIVE, not a pass.
  The first live run captured USB only (38 reports, report ID 1) with no BLE line.
  That is very likely correct firmware behaviour, not a failure: `routing.rs`
  sends `(keys, 0)` when USB is connected, so BLE gets nothing by design. The
  host cannot distinguish that from a dead link, so verify across two sources --
  confirm `connected on host slot N` in RTT, check USB-only typing, then unplug
  USB and confirm reports appear on `Bluetooth Low Energy` (which proves the
  harness can see BLE), then replug and confirm USB-only again. See the README
  duplicate-input section for the full sequence.

## Build hygiene and tooling pass

Follow-on work after the review items above. All of it keeps `just check` green.

### Dependency hygiene

- Added `embassy-futures` to `[patch.crates-io]`. The embassy crates depend on it
  by path inside the embassy repo, so the crates.io direct dep was linking a
  second copy (`cargo tree -d` showed `embassy-futures 0.1.2` twice). One copy now.
- Dropped the `critical-section` feature from `portable-atomic`. `embassy-rp`
  already provides the critical-section impl and the `CriticalSectionRawMutex`
  uses come from `embassy-sync`; Cargo feature unification was switching a second
  provider on for every `portable-atomic` consumer (`static_cell`, `usb-device`).
- Removed `heapless` (no references in `src/` or `build.rs`) and
  `executor-interrupt` from `embassy-executor` (nothing spawns at interrupt
  priority).
- Verified on hardware after each change: boots, config loads, BLE slot 1
  connects and encrypts, USB configures, no faults.

### Log-level split

`DEFMT_LOG=debug` in `.cargo/config.toml` had been applying to every profile, so
the shipped image carried the whole embassy/cyw43 debug flood. Cargo cannot scope
env vars to a profile, so the split is by profile: `release` (ship, `info`) and
`diagnostic` (bench, `debug`), in separate target dirs so switching levels does
not recompile the dependency tree. Recipes: `just build` / `just diag` /
`just flash` / `just ship`.

Measured flash (text+data, 4032 K region) at each level:

| `DEFMT_LOG` | flash | note |
| --- | --- | --- |
| `error` | 668,196 | strips all 17 `info!` state transitions too |
| `warn` | 677,344 | keeps failure paths only |
| `info` | 679,884 | **ship** — keeps config load / slot / connect / pairing |
| `debug` | 686,808 | bench — cyw43 HCI `rx`/`tx` and embassy internals |

`info` was chosen over `warn` for 2,540 bytes because a shipped device that cannot
report "connected on host slot 1" is hard to diagnose in the field.

### Toolchain pin and lint gate

- `rust-toolchain.toml` pins 1.98.1 plus the `thumbv8m.main-none-eabihf` target
  and `rustfmt`/`clippy`, auto-installed by rustup. Load-bearing twice over: the
  git-pinned embassy, and holding back the `proc-macro-error2 2.0.1` E0365
  future-incompatibility (transitive via `pio-proc <- pio <- embassy-rp`), which
  is at its latest published version with no upstream fix.
- `just clippy-host` now passes `-F unsafe_code`, mirroring
  `[lints.rust] unsafe_code = "forbid"` that a standalone `clippy-driver`
  invocation does not read. Confirmed with a negative test: an `unsafe` block in a
  host module is now caught by the gate.
- `unsafe_code` kept as `forbid` rather than `deny`, deliberately: no scoped
  escape hatch means any future `unsafe` has to change `Cargo.toml` visibly.

### CI

`.github/workflows/ci.yml` runs `just check` plus a ship build and image-size
report on every push and pull request, on the pinned toolchain. It cannot cover
the flash path, so the hardware checks above remain manual.

### Storage safety net

`just backup-storage` / `just verify-storage` / `just restore-storage` dump and
restore the whole 64 KiB config + bond region (`0x103F0000`) over SWD. The
backup is gitignored because it holds real pairing material, and `restore-storage`
refuses to run when no backup exists. Round-trip verified on hardware: read →
download → re-read came back byte-identical, and the device still loaded its
config and reconnected its encrypted slot-1 bond afterwards. Note `probe-rs
write` cannot do this restore; it only accepts RAM addresses.

### Duplicate-input harness

`tools/dupcheck.swift` taps HID input reports from every `vendor 0x2e8a` device
via `IOHIDManager`, tags each by transport, and fails if the same key set
arrives on both USB and BLE. Comparison is by key signature, not raw bytes, so a
different report ID or length between the transports cannot mask a duplicate.
`just dupcheck-selftest` verifies that logic against synthetic events and passes
(10 cases: cross-transport duplicate, cross-report-ID duplicate, outside window,
different keys, releases, USB-only, BLE-only, single report,
both-transports-but-only-releases, empty capture).

The first version of this harness could report PASS when only one transport had
been observed, because it only special-cased "zero events". A live run produced
`USB [id 1]: 38 report(s)` with no BLE line at all and still said PASS -- a
result that proved nothing. `evaluate` now requires at least two distinct
transports **and** at least one non-release report before it can return PASS;
anything short of that is INCONCLUSIVE. The USB-only, BLE-only and
releases-only selftest cases exist specifically to hold that behaviour.

### Gate status

- `just check` / `just check-macos`: PASS (28 unit tests, clippy on the embedded
  target and the five standalone host modules, dupcheck selftest).
- `just build` ship image: 679,884 bytes flash, 39,860 bytes RAM.
- `just flash` verified on the board after every change in this pass; last flash
  verification 37.24 s, 0 errors, BLE encrypted.

