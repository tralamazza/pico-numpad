# Pico numpad firmware

## Build and flash

From this directory:

```sh
cargo run --release -- --verify
```

The Cargo runner flashes the Pico 2 W over SWD using `probe-rs` and reads RTT
logs. Allow roughly 35 seconds for flashing/verification. Stop it with Ctrl-C;
the keyboard continues running without the probe. To attach without flashing:

```sh
probe-rs attach --chip RP235x --no-catch-reset target/thumbv8m.main-none-eabihf/release/pico-numpad
```

Use the exact ELF that is on the board for RTT decoding.

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

The web configurator's reset button resets key/LED settings, not Bluetooth slots.
Power-cycling does not clear bonds. Avoid chip-wide erase if you want to keep them.

### Cancel / normal typing

Press another key in the menu to cancel, or leave the menu idle for ten seconds.
Menu keys are not sent to the laptop. A **short `+` tap** still emits that key's
configured usage, but on release; holding `+` alone is reserved for the menu.
Pressing `+` together with another key before entering the menu types normally.

If saving a slot action fails, the LEDs flash red and the old selection/bonds
remain in use; the Pico does not reboot into unsaved state.

## Storage

All storage lies in the reserved top 64 KiB of the 4 MiB flash:

| Flash offset | Use |
| --- | --- |
| `0x3f0000..0x3f1000` | Key/LED configuration |
| `0x3f1000..0x3f3000` | Legacy single bond (read only during migration) |
| `0x3f3000..0x3f5000` | Versioned three-slot journal and selected slot |

The entire slot record is saved together using `sequential-storage`. Migration
writes the new journal before using it, leaving the old bond intact. Once a new
record exists, clearing slot 1 cannot resurrect the legacy bond on next boot.
Downgrading to the old firmware will still use the old single-bond region.

## Host-side logic tests

The key controls and report builder can be tested without hardware:

```sh
rustc --edition=2021 --test src/host_slots.rs -o /tmp/pico-numpad-slot-tests
/tmp/pico-numpad-slot-tests
rustc --edition=2021 --test src/hid.rs -o /tmp/pico-numpad-hid-tests
/tmp/pico-numpad-hid-tests
```

Hardware checks: migrate/reconnect slot 1; pair slots 2 and 3; switch between
laptops; power-cycle and reconnect; clear one slot and verify the others remain
green and reconnect; verify normal numeric keys, short `+`, and menu cancellation.
