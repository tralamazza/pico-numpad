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
# Measured flash (text+data) for this app, which has 26 warn!/17 info!/1 error!:
#   error 708,200   warn 717,364   info 719,904   debug 726,788
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

# fmt-check + clippy + tests.
check: fmt-check lint test
