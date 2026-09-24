//! TCA9555 I2C IO-expander driver for the Pico RGB Keypad Base buttons.
//!
//! The 16 silicone keys are wired to the expander's 16 GPIOs (not a scanned
//! matrix). Each key pulls its line low when pressed, so the pressed mask is the
//! bitwise NOT of the input registers.

use embassy_rp::i2c::{Async, Error, I2c};
use embassy_time::Instant;

use crate::debounce::Debouncer;

/// 7-bit I2C address of the TCA9555 on the base (address pins tied to GND).
pub const ADDR: u8 = 0x20;

const REG_INPUT0: u8 = 0x00;
const REG_CONFIG0: u8 = 0x06;

/// Number of keys on the pad.
#[allow(dead_code)]
pub const NUM_KEYS: usize = 16;

pub struct Keypad<'d> {
    i2c: I2c<'d, Async>,
    debounce: Debouncer,
}

impl<'d> Keypad<'d> {
    pub fn new(i2c: I2c<'d, Async>) -> Self {
        Self {
            i2c,
            debounce: Debouncer::default(),
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

    /// Debounced pressed mask (bit set = pressed), shared by every input consumer.
    pub async fn read_pressed(&mut self) -> Result<u16, Error> {
        let raw = !self.read_raw().await?;
        Ok(self.debounce.update(Instant::now().as_millis(), raw))
    }
}
