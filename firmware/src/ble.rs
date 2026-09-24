//! BLE HOGP (HID over GATT) peripheral.
//!
//! Exposes the Human Interface Device service (0x1812) with a single boot-keyboard
//! input report, plus a minimal Device Information service (0x180A) carrying the
//! PnP ID required by HOGP. Security is left open for now (no bonding/encryption).

// The `#[gatt_server]`/`#[gatt_service]` macros rebuild the structs and drop
// per-field attributes, so service fields the app never reads (e.g. `dis`) are
// reported as dead even though the generated registration uses them.
#![allow(dead_code)]

use embassy_futures::select::{select, Either};
use embassy_time::Timer;
use defmt::{info, warn};
use trouble_host::prelude::*;

use crate::backlight::{Backlight, NUM_LEDS};
use crate::hid::{build_report, REPORT_LEN, REPORT_MAP};
use crate::keypad::Keypad;

/// GATT attribute server: HID + Device Information services.
#[allow(dead_code)]
#[gatt_server]
pub struct Server {
    hid: HidService,
    dis: DisService,
}

/// Human Interface Device service (0x1812).
#[gatt_service(uuid = service::HUMAN_INTERFACE_DEVICE)]
struct HidService {
    /// HID Information: bcdHID 1.11, country 0x00, normally-connectable.
    #[characteristic(uuid = characteristic::HID_INFORMATION, read, value = [0x11u8, 0x01, 0x00, 0x02])]
    info: [u8; 4],

    /// Report Map: boot-keyboard descriptor.
    #[characteristic(uuid = characteristic::REPORT_MAP, read, value = REPORT_MAP)]
    report_map: &'static [u8],

    /// HID Control Point: 0x00 normal, 0x01 halt.
    #[characteristic(uuid = characteristic::HID_CONTROL_POINT, write_without_response)]
    control_point: (),

    /// Keyboard input report (Report ID 1).
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [1u8, 0, 0, 0, 0, 0, 0, 0, 0])]
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [0x01u8, 0x01])]
    report: [u8; REPORT_LEN],

    /// Protocol Mode: 1 = report protocol.
    #[characteristic(uuid = characteristic::PROTOCOL_MODE, read, write, value = 1u8)]
    protocol_mode: u8,
}

/// Device Information service (0x180A).
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DisService {
    #[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = "pico-numpad")]
    manufacturer: &'static str,

    #[characteristic(uuid = characteristic::MODEL_NUMBER_STRING, read, value = "Pico RGB Keypad")]
    model: &'static str,

    /// PnP ID: vendor source 0x01 (USB-IF), VID 0x2E8A, PID 0x0001, ver 0x0100.
    #[characteristic(uuid = characteristic::PNP_ID, read, value = [0x01u8, 0x8a, 0x2e, 0x01, 0x00, 0x00, 0x01])]
    pnp_id: [u8; 7],
}

/// Service UUID advertised so hosts can discover the HID service.
const HID_SERVICE_UUID: &[[u8; 2]] = &[[0x12, 0x18]];

/// Bring up the BLE stack and run the keyboard peripheral forever.
pub async fn run<C: Controller>(
    controller: C,
    keypad: Keypad<'static>,
    backlight: Backlight<'static>,
) {
    let address = Address::random([0xff, 0x8f, 0x1a, 0x05, 0xe4, 0xff]);
    let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(address)
        .build();

    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "pico-numpad",
        appearance: &appearance::human_interface_device::KEYBOARD,
    })) {
        Ok(s) => s,
        Err(e) => panic!("gatt server init failed: {e}"),
    };

    let mut runner = stack.runner();
    let peripheral = stack.peripheral();

    // Run the BLE host packet processor concurrently with the app loop.
    select(runner.run(), app_loop(peripheral, &server, keypad, backlight)).await;
}

async fn app_loop<C: Controller>(
    mut peripheral: Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
    mut keypad: Keypad<'static>,
    mut backlight: Backlight<'static>,
) {
    let mut adv_data = [0u8; 31];
    loop {
        set_advertising_pattern(&mut backlight).await;

        let n = match AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::CompleteServiceUuids16(&HID_SERVICE_UUID),
                AdStructure::CompleteLocalName(b"pico-numpad"),
            ],
            &mut adv_data,
        ) {
            Ok(n) => n,
            Err(e) => {
                warn!("adv encode failed: {:?}", e);
                continue;
            }
        };

        let advertiser = match peripheral
            .advertise(
                &AdvertisementParameters::default(),
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_data[..n],
                    scan_data: &[],
                },
            )
            .await
        {
            Ok(a) => a,
            Err(e) => {
                warn!("advertise failed: {:?}", e);
                Timer::after_millis(500).await;
                continue;
            }
        };

        info!("advertising");
        let conn = match advertiser.accept().await {
            Ok(c) => c,
            Err(e) => {
                warn!("accept failed: {:?}", e);
                continue;
            }
        };

        let gatt = match conn.with_attribute_server(server) {
            Ok(g) => g,
            Err(e) => {
                warn!("gatt attach failed: {:?}", e);
                continue;
            }
        };

        info!("connected");
        connection_task(&gatt, &server.hid.report, &mut keypad, &mut backlight).await;
        info!("disconnected, re-advertising");
    }
}

/// Process GATT events and push HID reports until the peer disconnects.
async fn connection_task(
    gatt: &GattConnection<'_, '_, DefaultPacketPool>,
    report: &Characteristic<[u8; REPORT_LEN]>,
    keypad: &mut Keypad<'static>,
    backlight: &mut Backlight<'static>,
) {
    let mut last: u16 = 0xFFFF;
    loop {
        match select(gatt.next(), Timer::after_millis(5)).await {
            Either::First(event) => match event {
                GattConnectionEvent::Disconnected { reason } => {
                    info!("disconnect: {:?}", reason);
                    return;
                }
                GattConnectionEvent::Gatt { event } => {
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                _ => {}
            },
            Either::Second(()) => {
                let pressed = match keypad.read_pressed().await {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if pressed != last {
                    let value = build_report(pressed);
                    if let Err(e) = report.notify(gatt, &value, true).await {
                        warn!("notify failed: {:?}", e);
                    }
                    paint(backlight, pressed).await;
                    last = pressed;
                }
            }
        }
    }
}

async fn paint(backlight: &mut Backlight<'static>, pressed: u16) {
    let mut leds = [[0u8; 3]; NUM_LEDS];
    for i in 0..NUM_LEDS {
        leds[i] = if pressed & (1 << i) != 0 {
            [0, 255, 0]
        } else {
            [4, 4, 4]
        };
    }
    let _ = backlight.write(&leds).await;
}

async fn set_advertising_pattern(backlight: &mut Backlight<'static>) {
    let mut leds = [[0u8; 3]; NUM_LEDS];
    for i in 0..NUM_LEDS {
        leds[i] = [0, 0, 40]; // dim blue while advertising
    }
    let _ = backlight.write(&leds).await;
}
