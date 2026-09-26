//! BLE HOGP (HID over GATT) peripheral: the Human Interface Device service
//! (0x1812) with keyboard and consumer input reports, plus a Device Information
//! service (0x180A) carrying the `PnP` ID HOGP requires. HID access is encrypted
//! and pairing bonds are persisted.

// The gatt macros rebuild these structs and drop per-field attributes, so `hid`
// and `dis` read as dead even though the generated registration uses them.
#![allow(dead_code)]

use defmt::{debug, info, warn};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use trouble_host::prelude::*;

use crate::backlight::{Backlight, NUM_LEDS};
use crate::config::led_mode;
use crate::config_bus::CONFIG;
use crate::config_store::{self, HostSlots};
use crate::hid::{CONSUMER_REPORT_LEN, REPORT_LEN, REPORT_MAP, build_reports};
use crate::host_slots::{self, Action, Controls, Input, Menu, SLOT_KEYS};
use crate::keypad::{Keypad, SAFETY_POLL_MS};
use crate::recovery;

/// GATT attribute server: HID + Device Information services.
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

    /// Consumer / media input report (Report ID 2): one usage page 0x0C bitmask
    /// per report. HOGP carries a second report type in a separate
    /// characteristic; the host tells them apart by the Report Reference ID.
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0u8; CONSUMER_REPORT_LEN], permissions(encrypted))]
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read = encrypted, value = [0x02u8, 0x01])]
    consumer_report: [u8; CONSUMER_REPORT_LEN],
}

/// Device Information service (0x180A).
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DisService {
    #[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = "pico-numpad")]
    manufacturer: &'static str,

    #[characteristic(uuid = characteristic::MODEL_NUMBER_STRING, read, value = "Pico RGB Keypad")]
    model: &'static str,

    /// `PnP` ID: source 0x02 (USB-IF), VID 0x2E8A, PID 0x000A, ver 0x0100.
    /// Pre: the PID matches the USB descriptor, so a host correlating both
    /// transports sees one product. macOS caches this per bond.
    #[characteristic(uuid = characteristic::PNP_ID, read, value = [0x02u8, 0x8a, 0x2e, 0x0a, 0x00, 0x00, 0x01])]
    pnp_id: [u8; 7],
}

/// Service UUID advertised so hosts can discover the HID service.
const HID_SERVICE_UUID: &[[u8; 2]] = &[[0x12, 0x18]];

/// Advertisement interval while a slot still needs pairing or just lost its link.
const ADV_INTERVAL_FAST: Duration = Duration::from_millis(160);

/// Advertisement interval for a bonded slot that has been sitting disconnected.
const ADV_INTERVAL_IDLE: Duration = Duration::from_millis(800);

/// Advertisement interval for the current bond state and idle streak; the cycle
/// count that gates the fast window lives in [`host_slots::advertise_fast`].
#[must_use]
fn adv_interval(bonded: bool, fast_cycles: u32) -> Duration {
    if host_slots::advertise_fast(bonded, fast_cycles) {
        ADV_INTERVAL_FAST
    } else {
        ADV_INTERVAL_IDLE
    }
}

/// Storage-fault mode: no BLE identity is advertised, no empty bond set is
/// substituted. USB runs alongside this loop.
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
                        info!(
                            "host bonds cleared; every previously paired host must forget this device before it can pair again"
                        );
                        flash_bonds_cleared(&mut backlight).await;
                    } else {
                        info!("host storage recovered; restarting");
                        let _ = backlight.write(&[[0, 80, 0]; NUM_LEDS]).await;
                    }
                    restart().await;
                } else {
                    warn!("host recovery failed; release keys before trying again");
                }
            }
        }
        Timer::after_millis(5).await;
    }
}

/// Blue pulse before the reboot that follows a bond reset: a peripheral cannot
/// invalidate a bond it does not hold, so every previously paired host must
/// "forget" this device before it can pair again.
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
        last_activity: 0,
        blanked: false,
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
    let mut fast_cycles = 0u32;
    loop {
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
        let interval = adv_interval(bonded, fast_cycles);
        fast_cycles = fast_cycles.saturating_add(1);
        let params = AdvertisementParameters {
            interval_min: interval,
            interval_max: interval,
            ..AdvertisementParameters::default()
        };
        // Inside the macro so a ship build at a higher log level compiles the
        // decode away too.
        debug!(
            "advertising as \"{}\" (bonded={}, {} ms interval, {} of 31 advertisement bytes)",
            core::str::from_utf8(adv_name).unwrap_or("<not utf8>"),
            bonded,
            interval.as_millis(),
            u8::try_from(n).unwrap_or(u8::MAX)
        );
        let advertiser = match peripheral
            .advertise(
                &params,
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
                fast_cycles = 0;
                if let Some(bond) = &hosts.bonds[hosts.active as usize]
                    && !bond.identity.match_identity(&conn.peer_identity())
                {
                    warn!("rejecting peer outside active host slot");
                    conn.disconnect();
                    Timer::after_millis(100).await;
                    continue;
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
                let action = connection_task(&gatt, &server.hid, hosts, ui).await;
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
    hid: &HidService,
    hosts: &mut HostSlots,
    ui: &mut KeypadUi,
) -> Option<Action> {
    let report = &hid.report;
    let consumer_report = &hid.consumer_report;
    let mut last_report = None;
    let mut last_consumer = None;
    loop {
        match select(gatt.next(), ui.wait_keypad()).await {
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
                    // Released reports stop modifiers and media keys sticking on
                    // the old host.
                    let _ = with_timeout(
                        Duration::from_millis(100),
                        report.notify(gatt, &[0; REPORT_LEN], true),
                    )
                    .await;
                    let _ = with_timeout(
                        Duration::from_millis(100),
                        consumer_report.notify(gatt, &[0; CONSUMER_REPORT_LEN], true),
                    )
                    .await;
                    return Some(action);
                }
                let (value, consumer) = {
                    let cfg = CONFIG.lock().await;
                    build_reports(input.keys, &cfg.keymap, cfg.consumer_mask)
                };

                // Each report is tracked and sent independently: a host may
                // subscribe to one and not the other. The `!should_notify` arm
                // only clears the memo, so the current state still goes out when
                // the host subscribes later.
                if !report.should_notify(gatt) {
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

                if !consumer_report.should_notify(gatt) {
                    last_consumer = None;
                } else if last_consumer != Some(consumer) {
                    match with_timeout(
                        Duration::from_millis(100),
                        consumer_report.notify(gatt, &consumer, true),
                    )
                    .await
                    {
                        Ok(Ok(())) => {
                            debug!("media report={=[u8]:02x}", consumer);
                            last_consumer = Some(consumer);
                        }
                        Ok(Err(e)) => warn!("media notify failed: {:?}", e),
                        Err(_) => warn!("media notify timed out"),
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
    last_activity: u64,
    blanked: bool,
}

impl KeypadUi {
    /// How long the input loop may sleep before it must wake anyway.
    /// Post: the minimum of the control state machine's next deadline, the LED
    /// idle deadline (only while lit) and `SAFETY_POLL_MS`, and never below 1.
    fn sleep_budget(&self, now: u64) -> u64 {
        let mut budget = SAFETY_POLL_MS;
        if let Some(until) = self.controls.next_deadline(now) {
            budget = budget.min(until);
        }
        if let Some(settle) = self.keypad.next_settle(now) {
            budget = budget.min(settle);
        }
        if !self.blanked {
            let idle = (self.last_activity + host_slots::LED_IDLE_OFF_MS).saturating_sub(now);
            budget = budget.min(idle);
        }
        budget.max(1)
    }

    /// Block until a key changes, or until the sleep budget runs out.
    async fn wait_keypad(&mut self) {
        let budget = self.sleep_budget(Instant::now().as_millis());
        let wake = self.keypad.wait_change(budget).await;
        debug!("keypad wake: {} (budget {} ms)", wake, budget);
    }

    async fn wait_action(&mut self, hosts: &HostSlots) -> Action {
        loop {
            if let Some(input) = self.poll(hosts, false).await
                && let Some(action) = input.action
            {
                return action;
            }
            self.wait_keypad().await;
        }
    }

    async fn poll(&mut self, hosts: &HostSlots, connected: bool) -> Option<Input> {
        let pressed = self.keypad.read_pressed().await.ok()?;
        let now = Instant::now().as_millis();
        let mut input = self.controls.update(now, pressed);
        if pressed != 0 {
            self.last_activity = now;
        }
        let usb_active = crate::usb::keyboard_active();
        let (usb_keys, ble_keys) = self.router.update(input.keys, usb_active, connected);
        let (usb_kbd, usb_consumer) = {
            let cfg = CONFIG.lock().await;
            build_reports(usb_keys, &cfg.keymap, cfg.consumer_mask)
        };
        crate::usb::publish_reports(usb_kbd, usb_consumer).await;
        input.keys = ble_keys;
        let connected = connected || usb_active;
        let (mode, mut brightness, slot_colors) = {
            let cfg = CONFIG.lock().await;
            (cfg.led_mode, cfg.brightness, cfg.slot_colors)
        };
        let mut leds = [[0; 3]; NUM_LEDS];
        let blank = host_slots::backlight_blank(now, self.last_activity, input.menu);
        self.blanked = blank;
        if input.menu != Menu::Closed {
            brightness = brightness.max(24);
            let colors = host_slots::menu_colors(
                core::array::from_fn(|i| hosts.bonds[i].is_some()),
                hosts.active,
                input.menu,
                now,
                slot_colors,
            );
            // LED index == physical key bit index on this board.
            for (i, key) in SLOT_KEYS.iter().enumerate() {
                leds[key.trailing_zeros() as usize] = colors[i];
            }
            if let Menu::Entering(progress) = input.menu {
                leds[host_slots::PLUS.trailing_zeros() as usize] = host_slots::enter_fill(progress);
            }
        } else if mode != led_mode::OFF && !blank {
            let tint =
                host_slots::scale(slot_colors[hosts.active as usize], host_slots::IDLE_LEVEL);
            for (i, led) in leds.iter_mut().enumerate() {
                *led = if connected {
                    if pressed & (1 << i) != 0 {
                        [0, 255, 0]
                    } else {
                        tint
                    }
                } else {
                    [0, 0, 40]
                };
            }
            if !connected {
                leds[SLOT_KEYS[hosts.active as usize].trailing_zeros() as usize] =
                    host_slots::scale(
                        slot_colors[hosts.active as usize],
                        if hosts.bonds[hosts.active as usize].is_some() {
                            80
                        } else {
                            40
                        },
                    );
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
