//! APA102 (`DotStar`) backlight driver for the Pico RGB Keypad Base.
//!
//! 16 APA102 LEDs on SPI0 (DATA=MOSI=GP19, CLK=GP18) with an active-low chip
//! select on GP17 that is asserted only for the duration of a transfer, matching
//! the behaviour of the Pimoroni C++ library.

use embassy_rp::gpio::{Level, Output};
use embassy_rp::spi::{Async, Error, Spi};

/// Number of backlight LEDs.
pub const NUM_LEDS: usize = 16;

/// Frame: 4-byte start + 4 bytes/LED + 4-byte end.
const FRAME_LEN: usize = 4 + NUM_LEDS * 4 + 4;

/// A single LED colour (R, G, B).
pub type Rgb = [u8; 3];

pub struct Backlight<'d> {
    spi: Spi<'d, Async>,
    cs: Output<'d>,
    frame: [u8; FRAME_LEN],
    brightness: u8, // 0..=31 (APA102 5-bit global brightness)
}

impl<'d> Backlight<'d> {
    pub fn new(spi: Spi<'d, Async>, cs: Output<'d>) -> Self {
        let mut bl = Self {
            spi,
            cs,
            frame: [0u8; FRAME_LEN],
            brightness: 0x10,
        };
        // End frame is all 1s; set once, never changes.
        for b in bl.frame.iter_mut().skip(4 + NUM_LEDS * 4) {
            *b = 0xFF;
        }
        bl
    }

    /// Set the global brightness (0..=31).
    pub fn set_brightness(&mut self, brightness: u8) {
        self.brightness = brightness & 0x1F;
    }

    /// Push a full frame of 16 colours to the strip.
    pub async fn write(&mut self, leds: &[Rgb; NUM_LEDS]) -> Result<(), Error> {
        let mut i = 4; // skip start frame (already zeros)
        for led in leds {
            self.frame[i] = 0xE0 | self.brightness;
            self.frame[i + 1] = led[2]; // B
            self.frame[i + 2] = led[1]; // G
            self.frame[i + 3] = led[0]; // R
            i += 4;
        }
        self.cs.set_level(Level::Low);
        let r = self.spi.write(&self.frame).await;
        self.cs.set_level(Level::High);
        r
    }
}
