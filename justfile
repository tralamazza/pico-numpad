# Dev tasks for pico-numpad. `cargo` must run inside firmware/ so that
# firmware/.cargo/config.toml selects the thumbv8m target and probe-rs runner.
# Lint levels live in firmware/Cargo.toml; LINT_FLAGS mirrors them for the
# standalone host-module compiles, which do not read Cargo.toml.

# Mirrors [lints.rust] unsafe_code = "forbid" from Cargo.toml.
LINT_FLAGS := "-Dwarnings -F unsafe_code -Wclippy::all -Wclippy::pedantic"

# Read out of Cargo.toml so the standalone compiles follow [package] edition.
EDITION := `sed -n 's/^edition = "\([^"]*\)"/\1/p' firmware/Cargo.toml`

# Everything worth running before pushing.
default: check

# Format the firmware crate.
fmt:
    cd firmware && cargo fmt

# Fail if the firmware crate is not formatted.
fmt-check:
    cd firmware && cargo fmt --check

# Clippy for the firmware binary (embedded target).
clippy:
    cd firmware && cargo clippy --release

# Clippy for the hardware-free modules, compiled standalone like `just test`.
clippy-host:
    cd firmware \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/host_slots.rs -o /tmp/pico-numpad-slot-clippy \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/hid.rs -o /tmp/pico-numpad-hid-clippy \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/debounce.rs -o /tmp/pico-numpad-debounce-clippy \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/recovery.rs -o /tmp/pico-numpad-recovery-clippy \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/routing.rs -o /tmp/pico-numpad-routing-clippy \
    && clippy-driver --edition={{EDITION}} --test {{LINT_FLAGS}} src/config.rs -o /tmp/pico-numpad-config-clippy

# All clippy checks.
lint: clippy clippy-host

# Unit tests for the hardware-free logic (slot controls, HID report builder).
test:
    cd firmware \
    && rustc --edition={{EDITION}} --test src/host_slots.rs -o /tmp/pico-numpad-slot-tests \
    && /tmp/pico-numpad-slot-tests \
    && rustc --edition={{EDITION}} --test src/hid.rs -o /tmp/pico-numpad-hid-tests \
    && /tmp/pico-numpad-hid-tests \
    && rustc --edition={{EDITION}} --test src/debounce.rs -o /tmp/pico-numpad-debounce-tests \
    && /tmp/pico-numpad-debounce-tests \
    && rustc --edition={{EDITION}} --test src/recovery.rs -o /tmp/pico-numpad-recovery-tests \
    && /tmp/pico-numpad-recovery-tests \
    && rustc --edition={{EDITION}} --test src/routing.rs -o /tmp/pico-numpad-routing-tests \
    && /tmp/pico-numpad-routing-tests \
    && rustc --edition={{EDITION}} --test src/config.rs -o /tmp/pico-numpad-config-tests \
    && /tmp/pico-numpad-config-tests

# defmt log levels: `release` is the ship image, `diagnostic` the bench image.
# `info` keeps the field-diagnosis transitions (config loaded, slot active,
# connected, pairing complete) without the cyw43 HCI and embassy internals that
# `debug` pulls in. Measure with `just size` rather than writing figures down.
LOG_SHIP := "info"
LOG_BENCH := "debug"

# Build the quiet ship image (release profile, no flashing).
build:
    cd firmware && DEFMT_LOG={{LOG_SHIP}} cargo build --release

# Build the bench image with full defmt tracing.
diag:
    cd firmware && DEFMT_LOG={{LOG_BENCH}} cargo build --profile diagnostic

# Flash the bench image over SWD and tail RTT logs (~35s to verify).
flash:
    cd firmware && DEFMT_LOG={{LOG_BENCH}} cargo run --profile diagnostic -- --verify

# Flash the quiet ship image over SWD; info, warn and error still reach RTT.
ship:
    cd firmware && DEFMT_LOG={{LOG_SHIP}} cargo run --release -- --verify

# picotool, not elf2uf2-rs: the latter stamps RP2040 and the RP2350 bootloader
# refuses it. The .elf path is needed because picotool picks the format by suffix.

# Convert the ship image to UF2 for BOOTSEL drag-and-drop flashing.
uf2:
    @just build
    @ELF=firmware/target/pico-numpad.elf; \
     UF2=firmware/target/pico-numpad.uf2; \
     cp firmware/target/thumbv8m.main-none-eabihf/release/pico-numpad "$ELF"; \
     picotool uf2 convert "$ELF" "$UF2" --family rp2350-arm-s; \
     picotool info "$UF2"

# Report the ship image size (uses arm-none-eabi-size if present; Apple's size cannot read ARM ELF).
size:
    @BIN=firmware/target/thumbv8m.main-none-eabihf/release/pico-numpad; \
    if command -v arm-none-eabi-size >/dev/null 2>&1; then SZ=arm-none-eabi-size; else SZ=size; fi; \
    "$SZ" "$BIN" | awk 'NR==2 {printf "ship image: %d bytes flash (text+data) of the 4032K region, %d bytes ram (bss)\n", $1+$2, $3}'

# The editor is a single static file in web/ with no build step. WebUSB treats
# http://localhost as a secure context and grants access per origin, so the
# origin you open is the origin that stays authorised.

# Serve the WebUSB config editor at http://localhost:8080.
serve:
    python3 -m http.server 8080 --bind 127.0.0.1 --directory web

# --------------------------------------------------------------------------
# Storage safety net. Dumps the whole reserved top 64 KiB (config, legacy bond,
# slot journal and the slack between them) so a restore cannot miss a region.
# Take a backup before any destructive storage test. The backup holds real
# pairing material -- it is gitignored, keep it off shared drives. Only one
# process can hold the probe, so stop any lingering `just flash` capture first.
BACKUP := "pico-numpad-config-backup.bin"
STORAGE_BASE := "0x103F0000"
# 16384 x 32-bit words = 64 KiB, the entire reserved region.
STORAGE_WORDS := "16384"

# Dump the reserved region over SWD (read-only). Stop other probe-rs sessions first.
backup-storage:
    probe-rs read --chip RP235x -f binary -o {{BACKUP}} b32 {{STORAGE_BASE}} {{STORAGE_WORDS}}
    @echo "backed up 64 KiB from {{STORAGE_BASE}} to {{BACKUP}}"

# Re-read the region and diff it against the backup file.
verify-storage:
    @test -f {{BACKUP}} || { echo "no {{BACKUP}}; run 'just backup-storage' first"; exit 1; }
    probe-rs read --chip RP235x -f binary -o /tmp/pico-numpad-reread.bin b32 {{STORAGE_BASE}} {{STORAGE_WORDS}}
    cmp {{BACKUP}} /tmp/pico-numpad-reread.bin && echo "round-trip: byte-identical"

# Restore the reserved region from the backup, then verify the write.
restore-storage:
    @test -f {{BACKUP}} || { echo "no {{BACKUP}}; refusing to restore. Run 'just backup-storage' first."; exit 1; }
    probe-rs download --chip RP235x --binary-format bin --base-address {{STORAGE_BASE}} --verify {{BACKUP}}
    @echo "restored {{BACKUP}} to {{STORAGE_BASE}}; power-cycle and check RTT for 'config loaded from flash'"

# --------------------------------------------------------------------------
# Host-side duplicate-input check (macOS only; needs Input Monitoring permission).

# Verify the duplicate-detection logic on synthetic events (no device needed).
dupcheck-selftest:
    swiftc -O tools/dupcheck.swift -o /tmp/pico-numpad-dupcheck
    /tmp/pico-numpad-dupcheck --selftest

# Capture from the connected device(s). Usage: just dupcheck 30
dupcheck *args:
    swiftc -O tools/dupcheck.swift -o /tmp/pico-numpad-dupcheck
    /tmp/pico-numpad-dupcheck {{args}}

# Drive the web editor's WebUSB paths in headless Chromium with a faithful
# navigator.usb stub. No hardware needed; needs a Chromium-family browser.
webtest:
    python3 tools/webusb-selftest.py

# fmt-check + clippy + tests.
check: fmt-check lint test

# Everything portable plus the macOS-only host harness selftest and the browser test.
check-macos: check dupcheck-selftest webtest
