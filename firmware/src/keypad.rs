//! TCA9555 I2C IO-expander driver for the Pico RGB Keypad Base buttons.
//!
//! The 16 silicone keys are wired to the expander's 16 GPIOs (not a scanned
//! matrix). Each key pulls its line low when pressed, so the pressed mask is the
//! bitwise NOT of the input registers.

use embassy_futures::select::{select, Either};
use embassy_rp::gpio::Input;
use embassy_rp::i2c::{Async, Error, I2c};
use embassy_time::{Instant, Timer};

use crate::debounce::Debouncer;

/// 7-bit I2C address of the TCA9555 on the base (address pins tied to GND).
pub const ADDR: u8 = 0x20;

/// Upper bound on how long the input loop may sleep without a guaranteed wake.
///
/// The INT line plus the control-state deadline cover every wake the firmware
/// actually needs; this exists only to bound a *lost* interrupt, whose failure
/// mode is a silently dead keyboard.
///
/// 1 s is a measured choice. With INT verified working on GP3 the interrupt
/// always wins this race while typing, so this value only sets the *idle* wake
/// rate: 100 ms measured 10 wakes/s idle, 1 s gives 1/s, and the original 5 ms
/// poll was 200/s. If the interrupt ever stopped arriving the symptom would be
/// up to 1 s of input latency -- degraded and obvious, and logged as an
/// all-`Timeout` trace, rather than a dead pad.
pub const SAFETY_POLL_MS: u64 = 1_000;

/// Why `wait_change` returned.
///
/// Worth distinguishing because a dead INT line is otherwise invisible: the
/// safety timer covers for it, keys still work, and the only symptom is that
/// idle power is ~10x higher than intended. `Timeout` dominating an idle period
/// is the signature of an interrupt that is not arriving.
#[derive(Debug, defmt::Format, PartialEq, Eq, Clone, Copy)]
pub enum Wake {
    /// INT fell: a key moved.
    Interrupt,
    /// INT was already low on entry -- a change landed during the previous read.
    AlreadyAsserted,
    /// The safety timer expired with no interrupt.
    Timeout,
}

const REG_INPUT0: u8 = 0x00;
const REG_CONFIG0: u8 = 0x06;

/// Number of keys on the pad.
#[allow(dead_code)]
pub const NUM_KEYS: usize = 16;

pub struct Keypad<'d> {
    i2c: I2c<'d, Async>,
    int: Input<'d>,
    debounce: Debouncer,
}

impl<'d> Keypad<'d> {
    /// `int` is the TCA9555 INT line: GP3 on the Pico RGB Keypad Base, pulled
    /// high on-board by `RM1-7` (10k) to 3V3. Open drain, active low.
    pub fn new(i2c: I2c<'d, Async>, int: Input<'d>) -> Self {
        Self {
            i2c,
            int,
            debounce: Debouncer::default(),
        }
    }

    /// Sleep until the expander reports an input change, or `max_ms` elapses.
    ///
    /// INT is open drain and is released *only* by reading the input registers,
    /// so a `read_pressed` must follow every wake or the line stays asserted.
    ///
    /// Checking the level before sleeping closes the obvious race: if a key moved
    /// during or after the previous read, the falling edge has already passed but
    /// the line is still low. Sleeping on the edge there would drop the event until
    /// the safety timer fired.
    pub async fn wait_change(&mut self, max_ms: u64) -> Wake {
        if self.int.is_low() {
            return Wake::AlreadyAsserted;
        }
        match select(
            self.int.wait_for_falling_edge(),
            Timer::after_millis(max_ms),
        )
        .await
        {
            Either::First(()) => Wake::Interrupt,
            Either::Second(()) => Wake::Timeout,
        }
    }

    /// Configure all 16 expander pins as inputs.
    pub async fn init(&mut self) -> Result<(), Error> {
        // Configuration ports: 1 = input.
        self.i2c.write_async(ADDR, [REG_CONFIG0, 0xFF, 0xFF]).await
    }

    /// Raw 16-bit input register value (bit set = line high).
    pub async fn read_raw(&mut self) -> Result<u16, Error> {
        let mut buf = [0u8; 2];
        self.i2c
            .write_read_async(ADDR, [REG_INPUT0], &mut buf)
            .await?;
        Ok(u16::from(buf[0]) | (u16::from(buf[1]) << 8))
    }

    /// Millis until the debouncer needs another sample to settle a pending
    /// transition, or `None` when every key is stable.
    ///
    /// This is not optional for a sleeping reader. The read that clears INT is
    /// taken too early to settle the transition it was woken by, so without a
    /// follow-up sample at `since + DEBOUNCE_MS` presses are lost or keys are
    /// left stuck. See `Debouncer::next_settle`.
    #[must_use]
    pub fn next_settle(&self, now: u64) -> Option<u64> {
        self.debounce.next_settle(now)
    }

    /// Debounced pressed mask (bit set = pressed), shared by every input consumer.
    pub async fn read_pressed(&mut self) -> Result<u16, Error> {
        let raw = !self.read_raw().await?;
        Ok(self.debounce.update(Instant::now().as_millis(), raw))
    }
}
