//! TCA9555 I2C IO-expander driver for the Pico RGB Keypad Base buttons. The 16
//! silicone keys sit on the expander's 16 GPIOs (not a scanned matrix) and each
//! pulls its line low when pressed, so the pressed mask is the NOT of the input.

use embassy_futures::select::{Either, select};
use embassy_rp::gpio::Input;
use embassy_rp::i2c::{Async, Error, I2c};
use embassy_time::{Instant, Timer};

use crate::debounce::Debouncer;

/// 7-bit I2C address of the TCA9555 on the base (address pins tied to GND).
pub const ADDR: u8 = 0x20;

/// Upper bound on how long the input loop may sleep without a guaranteed wake.
/// Only bounds a *lost* interrupt, whose failure mode is a dead keyboard. At 1 s
/// the idle wake rate is 1/s and a dead INT line costs up to 1 s of latency --
/// degraded and logged as an all-`Timeout` trace, not silent.
pub const SAFETY_POLL_MS: u64 = 1_000;

/// Why `wait_change` returned. A dead INT line is otherwise invisible: the
/// safety timer covers for it and keys still work, so `Timeout` dominating an
/// idle period is the signature of an interrupt that is not arriving.
#[derive(Debug, defmt::Format, PartialEq, Eq, Clone, Copy)]
pub enum Wake {
    /// INT fell: a key moved.
    Interrupt,
    /// INT was already low on entry: a change landed during the previous read.
    AlreadyAsserted,
    /// The safety timer expired with no interrupt.
    Timeout,
}

const REG_INPUT0: u8 = 0x00;
const REG_CONFIG0: u8 = 0x06;

pub struct Keypad<'d> {
    i2c: I2c<'d, Async>,
    int: Input<'d>,
    debounce: Debouncer,
}

impl<'d> Keypad<'d> {
    /// `int` is the TCA9555 INT line: GP3 on the base, open drain, active low,
    /// pulled high on-board by `RM1-7` (10k).
    pub fn new(i2c: I2c<'d, Async>, int: Input<'d>) -> Self {
        Self {
            i2c,
            int,
            debounce: Debouncer::default(),
        }
    }

    /// Sleep until the expander reports an input change, or `max_ms` elapses.
    /// INT is open drain and is released only by reading the input registers, so
    /// a `read_pressed` must follow every wake. Checking the level first closes
    /// the race where the falling edge passed but the line is still low.
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

    /// Configure all 16 expander pins as inputs (config port 1 = input).
    pub async fn init(&mut self) -> Result<(), Error> {
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
    /// transition, or `None` when every key is stable. A sleeping reader must
    /// wake on this or presses are lost -- see `Debouncer::next_settle`.
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
