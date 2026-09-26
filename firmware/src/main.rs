#![no_std]
#![no_main]

use cyw43::{Cyw43439, aligned_bytes};
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_rp::bind_interrupts;
use embassy_rp::dma::{self, Channel};
use embassy_rp::flash::Flash;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::i2c::{self, Config as I2cConfig, I2c};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH4, I2C0, PIO0, USB};
use embassy_rp::pio::Pio;
use embassy_rp::spi::{Config as SpiConfig, Spi};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbInterruptHandler};
use static_cell::StaticCell;
use trouble_host::prelude::ExternalController;
use {defmt_rtt as _, panic_probe as _};

mod backlight;
mod ble;
mod config;
mod config_bus;
mod config_store;
mod debounce;
mod hid;
mod host_slots;
mod keypad;
mod passkey;
mod recovery;
mod routing;
mod usb;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => embassy_rp::pio::InterruptHandler<PIO0>;
    I2C0_IRQ => i2c::InterruptHandler<I2C0>;
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>, dma::InterruptHandler<DMA_CH2>, dma::InterruptHandler<DMA_CH4>;
});

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<
        'static,
        cyw43::SpiBus<Output<'static>, PioSpi<'static, PIO0, 0>>,
        Cyw43439,
    >,
) -> ! {
    runner.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(embassy_rp::config::Config::default());

    // --- Keypad: TCA9555 on I2C0 (SCL=GP5, SDA=GP4) ---
    let mut i2c_cfg = I2cConfig::default();
    i2c_cfg.frequency = 400_000;
    i2c_cfg.sda_pullup = true;
    i2c_cfg.scl_pullup = true;
    let i2c = I2c::new_async(p.I2C0, p.PIN_5, p.PIN_4, Irqs, i2c_cfg);
    // TCA9555 INT on GP3: 10k on-board pull-up to 3V3 (RM1-7), unused elsewhere.
    let keypad_int = Input::new(p.PIN_3, Pull::Up);
    let mut keypad = keypad::Keypad::new(i2c, keypad_int);
    keypad.init().await.expect("keypad init");

    // --- Backlight: APA102 on SPI0 (CLK=GP18, MOSI=GP19), CS=GP17 ---
    let cs = Output::new(p.PIN_17, Level::High);
    let mut spi_cfg = SpiConfig::default();
    spi_cfg.frequency = 1_000_000;
    let spi = Spi::new_txonly(p.SPI0, p.PIN_18, p.PIN_19, p.DMA_CH4, Irqs, spi_cfg);
    let mut backlight = backlight::Backlight::new(spi, cs);
    backlight.set_brightness(0x08);

    // --- Config store: last 64 KiB of QSPI flash ---
    let mut flash: config_store::ConfigFlash = Flash::new(p.FLASH, p.DMA_CH2, Irqs);
    if let Some(c) = config_store::load(&mut flash).await {
        *config_bus::CONFIG.lock().await = c;
    }
    config_store::init(flash);
    let hosts = match config_store::load_hosts().await {
        Ok(hosts) => hosts,
        Err(reason) => {
            defmt::error!("host storage recovery: {}", reason);
            // Recovery needs neither the radio nor a valid bond record.
            join(
                ble::recover(keypad, backlight),
                usb::run_usb(UsbDriver::new(p.USB, Irqs)),
            )
            .await;
            return;
        }
    };

    // --- CYW43 Bluetooth on PIO0 (PWR=GP23, DIO=GP24, CS=GP25, CLK=GP29) ---
    let pwr = Output::new(p.PIN_23, Level::Low);
    let cy_cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let pio_spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        RM2_CLOCK_DIVIDER,
        pio.irq0,
        cy_cs,
        p.PIN_24,
        p.PIN_29,
        Channel::new(p.DMA_CH0, Irqs),
        Channel::new(p.DMA_CH1, Irqs),
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (_net, bt_device, mut control, runner) = cyw43::new_with_bluetooth(
        state,
        pwr,
        pio_spi,
        aligned_bytes!("../cyw43-firmware/43439A0.bin"),
        aligned_bytes!("../cyw43-firmware/43439A0_btfw.bin"),
        aligned_bytes!("../cyw43-firmware/nvram_rp2040.bin"),
    )
    .await;

    spawner.spawn(unwrap!(cyw43_task(runner)));
    control
        .init(aligned_bytes!("../cyw43-firmware/43439A0_clm.bin"))
        .await;

    let controller: ExternalController<_, 10> = ExternalController::new(bt_device);
    let usb_driver = UsbDriver::new(p.USB, Irqs);

    info!("pico-numpad P2 up: starting BLE HID + WebUSB config");
    join(
        ble::run(controller, keypad, backlight, hosts),
        usb::run_usb(usb_driver),
    )
    .await;
}
