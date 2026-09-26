# Per-host presets — design (not yet built)

Each of the three BLE host slots gets three presets of its own: keymap, media
keys, brightness, LED mode. Colour stays per host, not per preset — it identifies
the host, and making it vary by preset would undo that.

3 hosts x 3 presets = 9 configs.

## Why presets are not just the host slots

The first instinct was "a preset *is* a slot" — switching host switches layout,
zero new gestures. Rejected: layout then cannot vary independently of the
machine. You could not have two different layouts on the same laptop.

## Gesture

Reuses the existing slot keys. No new keys, no new hold timings.

| action | result | status |
|---|---|---|
| tap inactive slot | switch host | existing |
| **tap the active slot** | **cycle preset 1 -> 2 -> 3 -> 1** | **new; currently a no-op** |
| hold slot 3s | clear that host's bond | existing |
| hold `+` 3s | open menu | existing |

Tapping the already-active slot currently just re-selects it
(`Hosts::apply(Action::Select(slot))` sets `active` to what it already was),
so the gesture is free.

### Why cycle rather than direct-select

Tap counts for cycle at N=3:

| from -> to | taps |
|---|---|
| 1 -> 2, 2 -> 3, 3 -> 1 | 1 |
| 1 -> 3, 3 -> 2, 2 -> 1 | 2 |

Average 1.3, worst case 2.

Direct-select has no free gesture available. Every single-key gesture is taken or
conflicts: `+` tap types `+`; `+` hold opens the menu; slot tap selects host;
slot hold clears a bond; `+` + slot *is* host-select. The only shape left is a
two-stage menu — hold `+`, hold `+` again to enter a preset stage, then press a
slot key to pick preset 1/2/3. That is 4+ seconds of holding and a second menu
layer, against 1-2 taps.

The two-stage menu also has a real cost beyond UX: the slot state machine already
has nine states with timing-sensitive transitions and a destructive action
(bond clear) in the middle of it. A second stage is a new way to do the wrong
thing by accident.

Cycle loses only if many more presets are wanted. At N=6+ it degrades to ~3 taps
average and the two-stage menu starts earning its complexity.

Direction: forward only (1 -> 2 -> 3 -> 1). Reverse cycling would need another
gesture and is not worth one.

## Preset visibility

A **confirmation blink**: on switching, the active slot key blinks N times,
where N is the new preset number. Transient, costs nothing permanently, and
unambiguous at the moment it is needed. The editor also shows the current
preset persistently.

Rejected alternatives:

- **Tint brightness steps** (preset 1/2/3 at 100/60/30% of the slot tint) —
  permanent cue, but subtle, and it makes the brightness setting mean two
  things. Can be layered on later if the blink proves too easy to miss.
- **Hue shift per preset** — permanent and obvious, but then the colour the
  user configured is not what gets displayed. That breaks the per-slot colour
  feature rather than extending it.

## What it costs to build

**Storage.** 9 x 32 B = 288 B, against a 4 KiB config sector at
`0x103F0000` — ample. But `config_store` currently reads and writes a single
record and must be restructured to a 3x3 array, each record with its own
checksum. Erase is per-sector, so writing 288 B instead of 32 B does not
change erase count meaningfully.

The active preset per host must persist across reboot: 3 bytes, fits the
existing host-slots record.

**WebUSB protocol.** `CMD_GET` / `CMD_SET` address one config today. Needs an
index, `host * 3 + preset`. The editor grows a host selector as well as a
preset selector, and `tools/media-roundtrip.py` needs updating to match.

**Migration.** The existing single config becomes `active host / preset 1`.
Nothing is erased.

## Open questions

- Should a preset switch over USB (wired) also be per-host, or is the wired
  path a single config?
- Does the blink need to suppress the idle-blank timer so it is actually seen?
