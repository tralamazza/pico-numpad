# Pairing with the numpad — passkey entry

## The gap this closes

Before this, `ble.rs` never called `set_io_capabilities` and never handled any
`PassKey*` event. Pairing ran as **Just Works**, which is the only association
model that requires no user interaction. Just Works encrypts the link but
provides no MITM protection: an attacker positioned between the pad and the
laptop can complete pairing as the pad's peer and keep the session.

The pad now declares `IoCapabilities::KeyboardOnly` and answers
`PassKeyInput` by typing the code on its own digits.

## Why `KeyboardOnly` is what buys the protection

Two things in trouble 0.8 make the capability choice load-bearing rather than
cosmetic:

- `peripheral.rs` sets the MITM bit in `AuthReq` for every capability except
  `NoInputNoOutput`. Declaring `KeyboardOnly` is what asks for MITM at all.
- `choose_pairing_method` maps every `(central_capabilities, KeyboardOnly)`
  pair to `PassKeyEntry`, resolving to `EncryptedAuthenticated`. There is no
  branch in that table that falls back to Just Works for a keypad peripheral.

The one hole is a central that itself declares `NoInputNoOutput`. That pair
resolves to Just Works and it is not ours to prevent — the peer declined
protection. `PairingComplete { security_level }` distinguishes the two, and
only `EncryptedAuthenticated` gets the "it worked" indication.

## The pad is the input device

A numpad has ten digits and sixteen LEDs. Passkey entry is: one device shows
six digits, the other types them in. The pad types.

Rejected: **numeric comparison** (`KeyboardDisplay`), which would need the pad
to *render* a six-digit number on 16 LEDs. The laptop screen is a better
output device and the pad's strength is input, not display. Accordingly the
pad has no way to read a code back, so `PassKeyDisplay`/`PassKeyConfirm` are
cancelled on arrival rather than left to hang until the peer gives up.

## Entry uses the factory layout, not the user's keymap

`passkey.rs` derives the digit keys from `DEFAULT_KEYMAP` at compile time by
matching keypad HID usages (`0x59..=0x61` for 1–9, `0x62` for 0), so the
digits sit where the pad is printed regardless of how the user has remapped
it. Deriving rather than hardcoding the bit positions means a remap of the
default layout cannot silently move the passkeys.

This is the whole reason for the special-casing: during entry the pad is not a
keyboard yet. If entry used the user's keymap, nobody could tell which physical
keys were digits, so the silkscreen has to be the contract.

Those ten keys are lit for the duration so "which keys are digits" is
answerable by looking at the pad. Non-digit keys go dark.

## Bypassing the normal input path

Passkey entry sits above `Controls`, not inside it. While active, `poll` reads
the debounced mask directly and returns early, which buys three things:

- the user keymap is not consulted (above);
- `+` does not open the slot menu mid-entry, so the cancel gesture cannot be
  mistaken for a hold-to-select;
- no HID report goes out, so passkey digits never leak to the host as typing.

On exit, `Controls` is rebuilt rather than resumed. The entry bypassed it for
its whole duration, so a key resting on the pad at that moment would otherwise
arrive at the host as a fresh press. Stuck-key release reports also go out
before entry begins, for the same reason.

## Mistakes, timeouts, and giving up

- Six digits commit automatically. There is no early commit: a passkey is six
  digits, so a shorter answer is not a valid answer.
- `Enter` clears the entry and starts over. This is the backspace the pad does
  not have; a wrong digit costs a re-entry, not a re-pair.
- `+` cancels the pairing attempt via `pass_key_cancel()`.
- Two digit keys pressed together are ambiguous and are consumed with no
  effect. Requiring a release means a mistyped digit can only come from a key
  the user actually meant.
- A key already held when entry begins is seeded as already-down, so it is not
  read as the first digit.
- Entry times out at 30 s, matching the SMP signalling timeout. An abandoned
  entry fails closed instead of leaving the pad in a mode the user cannot exit.

The passkey value itself is never logged. It is the MITM secret, and a log line
outlives the pairing it was protecting.

## Feedback

The digit keys stay marked so the live keys are visible, brighten as the passkey
fills, and the key last pressed stands out — which is how the user tracks
position without a display of the digits themselves.

At the end, a green flash means `EncryptedAuthenticated`; red means failed,
cancelled or timed out. An unauthenticated pairing is deliberately not
celebrated, so a silent fallback is visible as a red flash rather than
passing as success.

## Config stays on USB

A wireless config channel was evaluated and dropped. A browser cannot reach
the HID service at all — Human Interface Device (`0x1812`) is on the Web
Bluetooth GATT blocklist — so config would need a second vendor service,
MTU chunking for the 32-byte record, and its own bond and trust model, for a
device you configure with a cable to hand. Configuration is USB-only by
decision; the WebUSB bulk interface in `usb.rs` is the only config path.

There is therefore no second link on this device. Passkey entry concerns only
pad to laptop HID pairing, and nothing else has to be kept clear of it.
