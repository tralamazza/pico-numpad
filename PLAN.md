# Firmware review fixes

Baseline: `893b405` (host slots and Clippy gate). Keep all work Clippy-clean.

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
- [ ] On a disposable/backed-up device, inject invalid journal/read/write faults:
  confirm USB/menu responsiveness, retry, explicit reset, and failure handling.
- [ ] Interrupt a recovery reset between flash operations and verify that legacy
  bonds never reappear and key/LED configuration remains intact.


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
