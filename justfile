# Dev tasks for pico-numpad.
#
# `cargo` must run inside firmware/ so that firmware/.cargo/config.toml selects the
# thumbv8m target and the probe-rs runner.
#
# Lint levels live in firmware/Cargo.toml ([lints.rust] / [lints.clippy]). The
# hardware-free modules are also compiled standalone with clippy-driver/rustc,
# which does not read Cargo.toml, so LINT_FLAGS mirrors those levels there.

LINT_FLAGS := "-Dwarnings -Wclippy::all -Wclippy::pedantic"

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
    && clippy-driver --edition=2021 --test {{LINT_FLAGS}} src/recovery.rs -o /tmp/pico-numpad-recovery-clippy

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
    && /tmp/pico-numpad-recovery-tests

# Build the firmware image.
build:
    cd firmware && cargo build --release

# Flash over SWD and tail RTT logs (~35s to verify).
flash:
    cd firmware && cargo run --release -- --verify

# fmt-check + clippy + tests.
check: fmt-check lint test
