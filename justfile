# Dev tasks for pico-numpad.
#
# `cargo` must run inside firmware/ so that firmware/.cargo/config.toml selects the
# thumbv8m target and the probe-rs runner.
#
# Lint levels live in firmware/Cargo.toml ([lints.rust] / [lints.clippy]). The
# hardware-free modules are also compiled standalone with clippy-driver/rustc,
# which does not read Cargo.toml, so LINT_FLAGS mirrors those levels there.

# -F unsafe_code mirrors [lints.rust] unsafe_code = "forbid" from Cargo.toml,
# which a standalone clippy-driver invocation does not read.
LINT_FLAGS := "-Dwarnings -F unsafe_code -Wclippy::all -Wclippy::pedantic"

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
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/host_slots.rs -o /tmp/pico-numpad-slot-clippy \
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/hid.rs -o /tmp/pico-numpad-hid-clippy \
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/debounce.rs -o /tmp/pico-numpad-debounce-clippy \
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/recovery.rs -o /tmp/pico-numpad-recovery-clippy \
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/routing.rs -o /tmp/pico-numpad-routing-clippy

# All clippy checks.
lint: clippy clippy-host

# Unit tests for the hardware-free logic (slot controls, HID report builder).
test:
    cd firmware \
    && rustc --edition=2021 --test src/host_slots.rs -o /tmp/pico-numpad-slot-tests \
    && /tmp/pico-numpad-slot-tests \
    && rustc --edition=2021 --test src/hid.rs -o /tmp/pico-numpad-hid-tests \
    && /tmp/pico-numpad-hid-tests \
    && rustc --edition=2021 --test src/debounce.rs -o /tmp/pico-numpad-debounce-tests \
    && /tmp/pico-numpad-debounce-tests \
    && rustc --edition=2021 --test src/recovery.rs -o /tmp/pico-numpad-recovery-tests \
    && /tmp/pico-numpad-recovery-tests \
    && rustc --edition=2021 --test src/routing.rs -o /tmp/pico-numpad-routing-tests \
    && /tmp/pico-numpad-routing-tests

# defmt log levels. `release` is the image you ship; `diagnostic` is the bench
# image. Set here (not only in .cargo/config.toml) so the two profiles stay apart
# and a bare `cargo` call still defaults to full tracing.
#
# Measured flash for this app (text+data; bss is RAM and is 39,860 at every
# level), with its 26 warn!/17 info!/1 error!:
#   error 668,196   warn 677,344   info 679,884   debug 686,808
# `info` is the ship level: it keeps the app's state transitions (config loaded,
# slot active, connected, pairing complete) for 2.5 KiB over `warn`, and still
# drops the cyw43 HCI rx/tx and embassy debug spam that costs ~7 KiB at `debug`.
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

# Report the ship image size (uses arm-none-eabi-size when present; Apple's
# /usr/bin/size cannot read ARM ELF).
size:
    @BIN=firmware/target/thumbv8m.main-none-eabihf/release/pico-numpad; \
    if command -v arm-none-eabi-size >/dev/null 2>&1; then SZ=arm-none-eabi-size; else SZ=size; fi; \
    "$SZ" "$BIN" | awk 'NR==2 {printf "ship image: %d bytes flash (text+data) of the 4032K region, %d bytes ram (bss)\n", $1+$2, $3}'

# --------------------------------------------------------------------------
# Storage safety net.
#
# The whole reserved top 64 KiB of flash is dumped in one shot: config at
# 0x103F0000 (4 KiB), the legacy single bond at 0x103F1000 (8 KiB), and the
# three-slot journal at 0x103F3000 (8 KiB), plus 44 KiB of slack. All of it is
# captured rather than just the ~600 bytes actually in use, so a restore cannot
# miss a region someone forgot to name. Offsets derive from config_store.rs; see
# the Storage section in firmware/README.md for the derivation and the invariant
# that keeps linked code from ever reaching 0x103F0000.
#
# Take a backup before any destructive storage test. Round-trip verified
# byte-identical on the RP2350.
#
# The backup contains your real pairing material -- it is gitignored, keep it off
# shared drives.
#
# Only one process can hold the debug probe, so stop any lingering `just flash`
# RTT capture before running these. Note that `probe-rs write` cannot restore
# flash -- it only accepts RAM addresses -- which is why restore goes through
# `download --binary-format bin --base-address`.
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
# Captures HID reports from every pico-numpad device on this host and fails if the
# same key set arrives on both USB and BLE. See tools/dupcheck.swift.

# Verify the duplicate-detection logic on synthetic events (no device needed).
dupcheck-selftest:
    swiftc -O tools/dupcheck.swift -o /tmp/pico-numpad-dupcheck
    /tmp/pico-numpad-dupcheck --selftest

# Capture from the connected device(s). Usage: just dupcheck 30
dupcheck *args:
    swiftc -O tools/dupcheck.swift -o /tmp/pico-numpad-dupcheck
    /tmp/pico-numpad-dupcheck {{args}}

# fmt-check + clippy + tests.
check: fmt-check lint test

# Everything portable plus the macOS-only host harness selftest.
check-macos: check dupcheck-selftest
