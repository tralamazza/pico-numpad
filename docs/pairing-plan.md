# Pairing with the numpad — passkey entry (not built)

## Why this is worth doing: current pairing has no MITM protection

`ble.rs` never calls `set_io_capabilities` and never handles any `PassKey*`
event. It handles only `PairingComplete` and `PairingFailed`.

That means pairing runs as **Just Works**. The tell is that it works today with
no user interaction at all — Just Works is the only association model that
requires nothing. The cost is that Just Works provides no MITM protection: an
attacker positioned between the pad and the laptop can complete pairing as the
pad's peer and keep the session.

This is not a regression and not unique to this device, but it is a real gap,
and the hardware to fix it is already bolted on.

## The stack supports it

trouble 0.8 exposes the full set on `GattConnectionEvent`:

| event | meaning | response |
|---|---|---|
| `PassKeyDisplay(PassKey)` | this device shows the passkey | render it |
| `PassKeyConfirm(PassKey)` | numeric comparison | `pass_key_confirm()` / `pass_key_cancel()` |
| `PassKeyInput` | this device must enter the passkey | `pass_key_input(u32)` |
| `PairingComplete { security_level, bond }` | done | — |
| `PairingFailed(Error)` | failed | — |

IO capabilities are set with `set_io_capabilities(IoCapabilities::…)`:
`DisplayOnly`, `DisplayYesNo`, `KeyboardOnly`, `NoInputNoOutput`,
`KeyboardDisplay`.

## Passkey entry is the right model for a numpad

A numpad has ten digits and sixteen LEDs. Passkey entry is: one device displays
six digits, the other *types them in*. The pad is the input device.

Flow:

1. Pairing starts, pad receives `PassKeyInput`.
2. LEDs enter an input pattern — six slots, waiting.
3. User types the six digits the laptop is showing, on the numpad.
4. Each accepted digit fills the next slot.
5. `Enter` commits → `pass_key_input(n)`.
6. A cancel gesture (`+`) → `pass_key_cancel()`.

This gives MITM protection using hardware that already exists, and it is a
better interaction than "click Pair and hope".

Rejected alternative: **numeric comparison** (`KeyboardDisplay`), where the pad
would have to *render* a six-digit number on 16 LEDs. Doable as BCD columns or
a rolling display, but it is a worse output device than a laptop screen and it
wastes the fact that the pad's strength is input, not display.

## Interaction design questions

- Which keys are digits during pairing, and what happens to the other ten keys?
  The pad is not yet a keyboard at this point, so all sixteen are free.
- Timeout. A passkey entry that is abandoned should fail closed, not hang.
- Mistake handling: a wrong digit means failed pairing and a retry. Backspace is
  worth having; a mis-entered passkey otherwise costs a full re-pair.
- The existing long-press-to-clear-bond gesture must not collide with digit
  entry.

## Config stays on USB

A wireless config channel was evaluated and dropped. A browser cannot reach
the HID service at all — Human Interface Device (`0x1812`) is on the Web
Bluetooth GATT blocklist — so config would need a second vendor service,
MTU chunking for the 32-byte record, and its own bond and trust model, for a
device you configure with a cable to hand. Configuration is USB-only by
decision; the WebUSB bulk interface in `usb.rs` is the only config path.

There is therefore no second link on this device. Passkey entry concerns only
pad <-> laptop HID pairing, and nothing else has to be kept clear of it.
