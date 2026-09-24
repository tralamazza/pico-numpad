//! BLE HOGP (HID over GATT) peripheral.
//!
//! Exposes the Human Interface Device service (0x1812) with a single report-protocol keyboard
//! input report, plus a minimal Device Information service (0x180A) carrying the
//! `PnP` ID required by HOGP. HID access is encrypted and pairing bonds are persisted.

// The `#[gatt_server]`/`#[gatt_service]` macros rebuild the structs and drop
// per-field attributes, so service fields the app never reads (e.g. `dis`) are
// reported as dead even though the generated registration uses them.
#![allow(dead_code)]

use defmt::{debug, info, warn};
use embassy_futures::select::{select, select3, Either, Either3};
use embassy_time::{with_timeout, Duration, Instant, Ticker, Timer};
use trouble_host::prelude::*;

use crate::backlight::{Backlight, NUM_LEDS};
use crate::config::{led_mode, CONFIG};
use crate::config_store::{self, HostSlots};
use crate::hid::{build_report, REPORT_LEN, REPORT_MAP};
use crate::host_slots::{self, Action, Controls, Input, Menu, SLOT_KEYS};
use crate::keypad::Keypad;
use crate::recovery;

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
    #[characteristic(uuid = characteristic::HID_INFORMATION, read, value = [0x11u8, 0x01, 0x00, 0x02], permissions(encrypted))]
    info: [u8; 4],

    /// Report Map: report-protocol keyboard descriptor; boot mode is not exposed.
    #[characteristic(uuid = characteristic::REPORT_MAP, read, value = REPORT_MAP, permissions(encrypted))]
    report_map: &'static [u8],

    /// HID Control Point: 0x00 suspend, 0x01 exit suspend.
    #[characteristic(uuid = characteristic::HID_CONTROL_POINT, write_without_response, permissions(encrypted))]
    control_point: u8,

    /// Keyboard input report (Report ID 1).
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0u8; REPORT_LEN], permissions(encrypted))]
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read = encrypted, value = [0x01u8, 0x01])]
    report: [u8; REPORT_LEN],
}

/// Device Information service (0x180A).
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DisService {
    #[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = "pico-numpad")]
    manufacturer: &'static str,

    #[characteristic(uuid = characteristic::MODEL_NUMBER_STRING, read, value = "Pico RGB Keypad")]
    model: &'static str,

    /// `PnP` ID: vendor source 0x02 (USB-IF), VID 0x2E8A, PID 0x0001, ver 0x0100.
    #[characteristic(uuid = characteristic::PNP_ID, read, value = [0x02u8, 0x8a, 0x2e, 0x01, 0x00, 0x00, 0x01])]
    pnp_id: [u8; 7],
}

/// Service UUID advertised so hosts can discover the HID service.
const HID_SERVICE_UUID: &[[u8; 2]] = &[[0x12, 0x18]];

/// Storage-fault mode: no BLE identity is advertised and no empty bond set is
/// substituted. USB runs alongside this loop, independently of radio startup.
pub async fn recover(mut keypad: Keypad<'static>, mut backlight: Backlight<'static>) -> ! {
    let mut controls = recovery::Controls::default();
    let mut last_color = None;
    backlight.set_brightness(8);
    loop {
        if let Ok(keys) = keypad.read_pressed().await {
            let now = Instant::now().as_millis();
            let level = if (now / 400).is_multiple_of(2) {
                15
            } else {
                80
            };
            let color = if keys == 0 {
                [level, 0, 0]
            } else {
                [level, level / 3, 0]
            };
            if last_color != Some(color) && backlight.write(&[color; NUM_LEDS]).await.is_ok() {
                last_color = Some(color);
            }
            if let Some(action) = controls.update(now, keys) {
                let restored = match action {
                    recovery::Action::Retry => match config_store::load_hosts().await {
                        Ok(_) => true,
                        Err(reason) => {
                            warn!("host storage retry failed: {}", reason);
                            false
                        }
                    },
                    recovery::Action::ResetBonds => config_store::reset_hosts().await,
                };
                if restored {
                    if matches!(action, recovery::Action::ResetBonds) {
                        info!("host bonds cleared; every previously paired host must forget this device before it can pair again");
                        flash_bonds_cleared(&mut backlight).await;
                    } else {
                        info!("host storage recovered; restarting");
                        let _ = backlight.write(&[[0, 80, 0]; NUM_LEDS]).await;
                    }
                    restart().await;
                } else {
                    // `restart()` diverges, so a success can never log a failure
                    // underneath it.
                    warn!("host recovery failed; release keys before trying again");
                }
            }
        }
        Timer::after_millis(5).await;
    }
}

/// Blue pulse before the reboot that follows a bond reset.
///
/// A host cannot be told its bond is gone: the central owns its bond store and a
/// peripheral cannot invalidate a pairing it does not hold. All this device can
/// do is refuse the reconnect, which the host sees as a generic authentication
/// failure -- and macOS keeps the dead pairing and retries it silently. The
/// device is therefore the only place this can be communicated, so say it as
/// loudly as we can: every host that ever paired must "forget" this device
/// before it can pair again.
async fn flash_bonds_cleared(backlight: &mut Backlight<'static>) {
    for _ in 0..4 {
        let _ = backlight.write(&[[0, 0, 120]; NUM_LEDS]).await;
        Timer::after_millis(280).await;
        let _ = backlight.write(&[[0, 0, 0]; NUM_LEDS]).await;
        Timer::after_millis(180).await;
    }
}

/// Bring up only the selected slot's identity and bond. Switching slots reboots
/// the radio cleanly rather than retaining another host's security/CCCD state.
pub async fn run<C: Controller>(
    controller: C,
    keypad: Keypad<'static>,
    backlight: Backlight<'static>,
    mut hosts: HostSlots,
) {
    let slot = hosts.active;
    let mut resources: HostResources<DefaultPacketPool, 1, 2, 1, 1> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random(host_slots::address(slot)))
        .build();
    if let Some(bond) = &hosts.bonds[slot as usize] {
        stack
            .add_bond_information(bond.clone())
            .expect("active host bond");
    }
    let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: host_slots::name(slot),
        appearance: &appearance::human_interface_device::KEYBOARD,
    }))
    .expect("gatt server init");
    let mut ui = KeypadUi {
        keypad,
        backlight,
        controls: Controls::new(),
        router: crate::routing::Router::default(),
        last_leds: None,
        last_brightness: 0,
    };
    info!(
        "BLE host slot {}: bonded={}",
        slot + 1,
        hosts.bonds[slot as usize].is_some()
    );
    let mut runner = stack.runner();
    select(
        runner.run(),
        app_loop(stack.peripheral(), &server, &mut hosts, &mut ui),
    )
    .await;
}

async fn app_loop<C: Controller>(
    mut peripheral: Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
    hosts: &mut HostSlots,
    ui: &mut KeypadUi,
) {
    let mut adv_data = [0u8; 31];
    let mut adv_name_buf = [0u8; host_slots::MAX_ADV_NAME_LEN];
    loop {
        // Rebuilt each iteration so the advertised name tracks the current bond
        // state: after a pairing completes or a slot is cleared, the next
        // advertisement says so instead of carrying a stale name until reboot.
        let bonded = hosts.bonds[hosts.active as usize].is_some();
        let adv_name = host_slots::adv_name(hosts.active, bonded, &mut adv_name_buf);
        let n = AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::CompleteServiceUuids16(HID_SERVICE_UUID),
                AdStructure::CompleteLocalName(adv_name),
            ],
            &mut adv_data,
        )
        .expect("advertising data");
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
                // Keep the physical recovery menu usable even if advertising fails.
                if let Either::First(action) =
                    select(ui.wait_action(hosts), Timer::after_millis(500)).await
                {
                    apply_action(hosts, action, ui).await;
                }
                continue;
            }
        };
        // Periodic idle windows let Trouble update the controller resolving list.
        // Controls use absolute timestamps and survive these cancelled scans.
        match select3(
            advertiser.accept(),
            ui.wait_action(hosts),
            Timer::after_secs(1),
        )
        .await
        {
            Either3::First(Ok(conn)) => {
                if let Some(bond) = &hosts.bonds[hosts.active as usize] {
                    if !bond.identity.match_identity(&conn.peer_identity()) {
                        warn!("rejecting peer outside active host slot");
                        conn.disconnect();
                        Timer::after_millis(100).await;
                        continue;
                    }
                }
                let empty = hosts.bonds[hosts.active as usize].is_none();
                if let Err(e) = conn.set_bondable(empty) {
                    warn!("set bondable failed: {:?}", e);
                    conn.disconnect();
                    continue;
                }
                let gatt = match conn.with_attribute_server(server) {
                    Ok(g) => g,
                    Err(e) => {
                        warn!("gatt attach failed: {:?}", e);
                        continue;
                    }
                };
                if let Err(e) = gatt.raw().request_security() {
                    warn!("security request failed: {:?}", e);
                }
                info!("connected on host slot {}", hosts.active + 1);
                let action = connection_task(&gatt, &server.hid.report, hosts, ui).await;
                gatt.raw().disconnect();
                drop(gatt);
                if let Some(action) = action {
                    apply_action(hosts, action, ui).await;
                }
                info!("disconnected, re-advertising slot {}", hosts.active + 1);
            }
            Either3::First(Err(e)) => warn!("accept failed: {:?}", e),
            Either3::Second(action) => apply_action(hosts, action, ui).await,
            Either3::Third(()) => {}
        }
    }
}

/// Save before rebooting; never switch or clear a bond if persistence fails.
async fn apply_action(hosts: &HostSlots, action: Action, ui: &mut KeypadUi) {
    let mut next = hosts.clone();
    next.apply(action);
    if next == *hosts {
        return;
    }
    if !config_store::save_hosts(&next).await {
        warn!("host action failed; keeping existing slots");
        ui.backlight.set_brightness(8);
        let _ = ui.backlight.write(&[[100, 0, 0]; NUM_LEDS]).await;
        Timer::after_millis(500).await;
        ui.last_leds = None;
        return;
    }
    info!(
        "restarting into host slot {}; pairing={}",
        next.active + 1,
        next.bonds[next.active as usize].is_none()
    );
    restart().await;
}

async fn restart() -> ! {
    // Let the old BLE link terminate before restarting the controller.
    Timer::after_millis(150).await;
    // RP2350 ROM normal reboot via watchdog, not BOOTSEL or a flash erase.
    embassy_rp::rom_data::reboot(0, 10, 0, 0);
    loop {
        cortex_m::asm::wfi();
    }
}

async fn connection_task(
    gatt: &GattConnection<'_, '_, DefaultPacketPool>,
    report: &Characteristic<[u8; REPORT_LEN]>,
    hosts: &mut HostSlots,
    ui: &mut KeypadUi,
) -> Option<Action> {
    let mut last_report = None;
    let mut ticker = Ticker::every(Duration::from_millis(5));
    loop {
        match select(gatt.next(), ticker.next()).await {
            Either::First(event) => match event {
                GattConnectionEvent::Disconnected { reason } => {
                    info!("disconnect: {:?}", reason);
                    return None;
                }
                GattConnectionEvent::PairingComplete {
                    security_level,
                    bond,
                } => {
                    info!("pairing complete: {:?}", security_level);
                    if let Some(bond) = bond.filter(|b| b.is_bonded) {
                        let mut next = hosts.clone();
                        next.bonds[hosts.active as usize] = Some(bond);
                        if next != *hosts {
                            if !config_store::save_hosts(&next).await {
                                warn!("could not persist host bond");
                                return None;
                            }
                            *hosts = next;
                        }
                        let _ = gatt.raw().set_bondable(false);
                    }
                }
                GattConnectionEvent::PairingFailed(err) => warn!("pairing failed: {:?}", err),
                GattConnectionEvent::Gatt { event } => {
                    if let Ok(reply) = event.accept() {
                        reply.send().await;
                    }
                }
                _ => {}
            },
            Either::Second(()) => {
                let Some(input) = ui.poll(hosts, true).await else {
                    continue;
                };
                if let Some(action) = input.action {
                    // A released report prevents modifiers/keys sticking on the old host.
                    let _ = with_timeout(
                        Duration::from_millis(100),
                        report.notify(gatt, &[0; REPORT_LEN], true),
                    )
                    .await;
                    return Some(action);
                }
                let value = build_report(input.keys, &CONFIG.lock().await.keymap);
                if !report.should_notify(gatt) {
                    // Send the current state as soon as the host subscribes, even
                    // if the physical keys have not changed since connection.
                    last_report = None;
                } else if last_report != Some(value) {
                    match with_timeout(
                        Duration::from_millis(100),
                        report.notify(gatt, &value, true),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            debug!("keys={=u16:04x} report={=[u8]:02x}", input.keys, value);
                            last_report = Some(value);
                        }
                        Ok(Err(e)) => warn!("notify failed: {:?}", e),
                        Err(_) => warn!("notify timed out"),
                    }
                }
            }
        }
    }
}

struct KeypadUi {
    keypad: Keypad<'static>,
    backlight: Backlight<'static>,
    controls: Controls,
    router: crate::routing::Router,
    last_leds: Option<[[u8; 3]; NUM_LEDS]>,
    last_brightness: u8,
}

impl KeypadUi {
    async fn wait_action(&mut self, hosts: &HostSlots) -> Action {
        loop {
            if let Some(input) = self.poll(hosts, false).await {
                if let Some(action) = input.action {
                    return action;
                }
            }
            Timer::after_millis(5).await;
        }
    }

    async fn poll(&mut self, hosts: &HostSlots, connected: bool) -> Option<Input> {
        let pressed = self.keypad.read_pressed().await.ok()?;
        let now = Instant::now().as_millis();
        let mut input = self.controls.update(now, pressed);
        let usb_active = crate::usb::keyboard_active();
        let (usb_keys, ble_keys) = self.router.update(input.keys, usb_active, connected);
        crate::usb::publish_report(build_report(usb_keys, &CONFIG.lock().await.keymap)).await;
        input.keys = ble_keys;
        let connected = connected || usb_active;
        let (mode, mut brightness) = {
            let cfg = CONFIG.lock().await;
            (cfg.led_mode, cfg.brightness)
        };
        let mut leds = [[0; 3]; NUM_LEDS];
        if input.menu != Menu::Closed {
            // Management feedback is visible even when normal backlight is off.
            brightness = brightness.max(8);
            let colors = host_slots::menu_colors(
                core::array::from_fn(|i| hosts.bonds[i].is_some()),
                hosts.active,
                input.menu,
                now,
            );
            for (i, key) in SLOT_KEYS.iter().enumerate() {
                leds[key.trailing_zeros() as usize] = colors[i];
            }
        } else if mode != led_mode::OFF {
            for (i, led) in leds.iter_mut().enumerate() {
                *led = if connected {
                    if pressed & (1 << i) != 0 {
                        [0, 255, 0]
                    } else {
                        [4, 4, 4]
                    }
                } else {
                    [0, 0, 40]
                };
            }
            if !connected {
                leds[SLOT_KEYS[hosts.active as usize].trailing_zeros() as usize] =
                    if hosts.bonds[hosts.active as usize].is_some() {
                        [0, 80, 0]
                    } else {
                        [0, 0, 150]
                    };
            }
        }
        if self.last_leds != Some(leds) || self.last_brightness != brightness {
            self.backlight.set_brightness(brightness);
            if self.backlight.write(&leds).await.is_ok() {
                self.last_leds = Some(leds);
                self.last_brightness = brightness;
            }
        }
        Some(input)
    }
}
