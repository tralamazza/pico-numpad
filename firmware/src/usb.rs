//! Composite USB keyboard and `WebUSB` configuration interface.
//!
//! Presents a vendor-specific bulk interface reachable from a browser via `WebUSB`
//! (and from libusb/pyusb). A tiny binary protocol reads/writes the shared
//! [`Config`]:
//!
//! ```text
//! host -> device: [cmd, ...]
//!   0x01 GET_CONFIG   -> [0x00, <32 config bytes>]
//!   0x02 SET_CONFIG   -> [0x00, 0x02, ...] on accept, [0x01, 0x02, ...] if malformed
//!   0x03 SAVE         -> [0x00, 0x03, ...] / [0x01, 0x03, ...]
//!   0x04 RESET_DEFAULTS -> [0x00, 0x04, ...]
//!
//! All responses are padded to 33 bytes so the host can always read a fixed size.
//! ```

use crate::hid::{CONSUMER_REPORT_LEN, REPORT_LEN, usb_consumer_report, usb_report};
use defmt::{info, warn};
use embassy_futures::join::{join, join3};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb::class::hid::{
    Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter, State as HidState,
};
use portable_atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_usb::class::web_usb::{Config as WebUsbConfig, State, Url, WebUsb};
use embassy_usb::driver::{Driver, Endpoint, EndpointIn, EndpointOut};
use embassy_usb::msos::{self, windows_version};
use embassy_usb::types::InterfaceNumber;
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::StaticCell;

use crate::config::{CONFIG_LEN, Config};
use crate::config_bus::CONFIG;

static CONFIGURED: AtomicBool = AtomicBool::new(false);
static SUSPENDED: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicU32 = AtomicU32::new(0);
static KEY_REPORT: Mutex<CriticalSectionRawMutex, (u32, [u8; REPORT_LEN])> =
    Mutex::new((0, [0; REPORT_LEN]));
static CONSUMER_REPORT: Mutex<CriticalSectionRawMutex, (u32, [u8; CONSUMER_REPORT_LEN])> =
    Mutex::new((0, [0; CONSUMER_REPORT_LEN]));

/// Publish both input reports for the current key state.
///
/// Sent together rather than via two separate calls so a reader cannot observe a
/// half-updated state -- both reports come from the same poll of the keys, and
/// splitting them would let the keyboard report land a moment before the media
/// one.
pub async fn publish_reports(report: [u8; REPORT_LEN], consumer: [u8; CONSUMER_REPORT_LEN]) {
    let generation = GENERATION.load(Ordering::Relaxed);
    *KEY_REPORT.lock().await = (generation, report);
    *CONSUMER_REPORT.lock().await = (generation, consumer);
}

pub fn keyboard_active() -> bool {
    CONFIGURED.load(Ordering::Relaxed) && !SUSPENDED.load(Ordering::Relaxed)
}

struct UsbEvents;
impl embassy_usb::Handler for UsbEvents {
    fn configured(&mut self, configured: bool) {
        CONFIGURED.store(configured, Ordering::Relaxed);
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
    fn reset(&mut self) {
        self.configured(false);
        SUSPENDED.store(false, Ordering::Relaxed);
    }
    fn enabled(&mut self, enabled: bool) {
        if !enabled {
            self.reset();
        }
    }
    fn suspended(&mut self, suspended: bool) {
        SUSPENDED.store(suspended, Ordering::Relaxed);
        GENERATION.fetch_add(1, Ordering::Relaxed);
    }
}

async fn keyboard_loop<D: Driver<'static>>(mut writer: HidWriter<'static, D, 9>) {
    let mut last = None;
    let mut generation = GENERATION.load(Ordering::Relaxed);
    loop {
        let current = GENERATION.load(Ordering::Relaxed);
        if generation != current || !keyboard_active() {
            last = None;
            generation = current;
        }
        if keyboard_active() {
            let (report_generation, value) = *KEY_REPORT.lock().await;
            // Never replay a report from before reset, unconfigure, or suspend.
            let report = if report_generation == current {
                value
            } else {
                [0; REPORT_LEN]
            };
            if last != Some(report)
                && let Ok(Ok(())) =
                    with_timeout(Duration::from_millis(20), writer.write(&usb_report(report))).await
            {
                last = Some(report);
            }
        }
        Timer::after_millis(5).await;
    }
}

/// The consumer report goes out on its own HID interface, not the keyboard's.
///
/// Same generation and suspend discipline as [`keyboard_loop`], but a separate
/// task with a separate writer: the two interfaces are separate devices to the
/// host, so neither may hold up the other.
async fn consumer_loop<D: Driver<'static>>(mut writer: HidWriter<'static, D, 3>) {
    let mut last = None;
    let mut generation = GENERATION.load(Ordering::Relaxed);
    loop {
        let current = GENERATION.load(Ordering::Relaxed);
        if generation != current || !keyboard_active() {
            last = None;
            generation = current;
        }
        if keyboard_active() {
            let (consumer_generation, consumer_value) = *CONSUMER_REPORT.lock().await;
            let consumer = if consumer_generation == current {
                consumer_value
            } else {
                [0; CONSUMER_REPORT_LEN]
            };
            if last != Some(consumer)
                && let Ok(Ok(())) = with_timeout(
                    Duration::from_millis(20),
                    writer.write(&usb_consumer_report(consumer)),
                )
                .await
            {
                last = Some(consumer);
            }
        }
        Timer::after_millis(5).await;
    }
}
const VID: u16 = 0x2E8A;
const PID: u16 = 0x000A;
const MAX_PACKET: u16 = 64;

// Windows needs a stable interface GUID to bind WinUSB without an INF.
const DEVICE_INTERFACE_GUIDS: &[&str] = &["{2E8A000A-0000-4000-8000-00000000000A}"];

const CMD_GET: u8 = 0x01;
const CMD_SET: u8 = 0x02;
const CMD_SAVE: u8 = 0x03;
const CMD_RESET: u8 = 0x04;

const OK: u8 = 0x00;
const ERR: u8 = 0x01;
const RESP_LEN: usize = 1 + CONFIG_LEN;

fn resp(status: u8, cmd: u8) -> [u8; RESP_LEN] {
    let mut a = [0u8; RESP_LEN];
    a[0] = status;
    a[1] = cmd;
    a
}

struct Endpoints<'d, D: Driver<'d>> {
    write_ep: D::EndpointIn,
    read_ep: D::EndpointOut,
}

/// Run the composite USB device, configuration protocol, and keyboard writer.
pub async fn run_usb<D: Driver<'static> + 'static>(driver: D) -> ! {
    static HID_STATE: StaticCell<HidState<'static>> = StaticCell::new();
    static CONSUMER_HID_STATE: StaticCell<HidState<'static>> = StaticCell::new();
    static EVENTS: StaticCell<UsbEvents> = StaticCell::new();
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 512]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static WEBUSB_STATE: StaticCell<State<'static>> = StaticCell::new();
    static WEBUSB_CONFIG: StaticCell<WebUsbConfig<'static>> = StaticCell::new();

    let config_descriptor = CONFIG_DESC.init([0u8; 256]);
    let bos_descriptor = BOS_DESC.init([0u8; 256]);
    let msos_descriptor = MSOS_DESC.init([0u8; 512]);
    let control_buf = CONTROL_BUF.init([0u8; 64]);
    let state = WEBUSB_STATE.init(State::new());
    let webusb_config = WEBUSB_CONFIG.init(WebUsbConfig {
        max_packet_size: MAX_PACKET,
        vendor_code: 1,
        // What the browser's "pico-numpad detected" notification points at.
        // Deployed to GitHub Pages by .github/workflows/pages.yml, so the nudge
        // resolves without running anything locally. `just serve` still works
        // for iterating on the editor, but the browser grants USB access per
        // origin -- a grant for the Pages origin does not cover localhost and
        // vice versa. The firmware cannot defer or rate-limit the notification;
        // it fires on every enumeration.
        landing_url: Some(Url::new("https://tralamazza.github.io/pico-numpad/")),
    });

    let mut config = UsbConfig::new(VID, PID);
    config.manufacturer = Some("pico-numpad");
    config.product = Some("pico-numpad");
    config.serial_number = Some("0001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut builder = Builder::new(
        driver,
        config,
        config_descriptor,
        bos_descriptor,
        msos_descriptor,
        control_buf,
    );

    builder.msos_descriptor(windows_version::WIN8_1, 0);
    builder.msos_writer().configuration(0);
    builder.msos_writer().function(InterfaceNumber(0));
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));
    builder.msos_writer().function(InterfaceNumber(1));
    builder.msos_feature(msos::CompatibleIdFeatureDescriptor::new("WINUSB", ""));
    builder.msos_feature(msos::RegistryPropertyFeatureDescriptor::new(
        "DeviceInterfaceGUIDs",
        msos::PropertyData::RegMultiSz(DEVICE_INTERFACE_GUIDS),
    ));

    WebUsb::configure(&mut builder, state, webusb_config);

    let mut func = builder.function(0xff, 0x00, 0x00);
    let mut iface = func.interface();
    let mut alt = iface.alt_setting(0xff, 0x00, 0x00, None);
    let write_ep = alt.endpoint_bulk_in(None, MAX_PACKET);
    let read_ep = alt.endpoint_bulk_out(None, MAX_PACKET);
    // FunctionBuilder finalises the function descriptor when dropped, so it must
    // go before the builder is consumed. The alt/iface builders have no Drop and
    // are released by NLL at their last use.
    drop(func);

    // HID is appended last on purpose. The WebUSB editor opens the vendor bulk
    // interface by a fixed number (`IFACE` in web/index.html), so putting HID
    // ahead of it would shift that number and silently break the editor.
    builder.handler(EVENTS.init(UsbEvents));
    let keyboard = HidWriter::<_, 9>::new(
        &mut builder,
        HID_STATE.init(HidState::new()),
        HidConfig {
            report_descriptor: crate::hid::KEYBOARD_REPORT_MAP,
            request_handler: None,
            poll_ms: 5,
            max_packet_size: 16,
            hid_subclass: HidSubclass::No,
            hid_boot_protocol: HidBootProtocol::None,
        },
    );
    // A second HID interface carrying only the consumer collection. See the
    // note on KEYBOARD_REPORT_MAP for why sharing the keyboard's interface
    // leaves media keys dead on macOS.
    let consumer = HidWriter::<_, 3>::new(
        &mut builder,
        CONSUMER_HID_STATE.init(HidState::new()),
        HidConfig {
            report_descriptor: crate::hid::CONSUMER_REPORT_MAP,
            request_handler: None,
            poll_ms: 5,
            max_packet_size: 16,
            hid_subclass: HidSubclass::No,
            hid_boot_protocol: HidBootProtocol::None,
        },
    );
    let mut usb = builder.build();
    let mut ep: Endpoints<'static, D> = Endpoints { write_ep, read_ep };

    join3(
        usb.run(),
        protocol_loop(&mut ep),
        join(keyboard_loop(keyboard), consumer_loop(consumer)),
    )
    .await;
    loop {
        embassy_time::Timer::after_millis(1000).await;
    }
}

async fn protocol_loop<'d, D: Driver<'d>>(ep: &mut Endpoints<'d, D>) {
    let mut buf = [0u8; MAX_PACKET as usize];
    loop {
        ep.read_ep.wait_enabled().await;
        let Ok(n) = ep.read_ep.read(&mut buf).await else {
            continue;
        };
        if n == 0 {
            continue;
        }
        match buf[0] {
            CMD_GET => {
                let bytes = CONFIG.lock().await.to_bytes();
                let mut out = [0u8; RESP_LEN];
                out[0] = OK;
                out[1..].copy_from_slice(&bytes);
                let _ = ep.write_ep.write(&out).await;
            }
            CMD_SET => {
                if n < 1 + CONFIG_LEN {
                    let _ = ep.write_ep.write(&resp(ERR, CMD_SET)).await;
                    continue;
                }
                let mut arr = [0u8; CONFIG_LEN];
                arr.copy_from_slice(&buf[1..=CONFIG_LEN]);
                if let Some(c) = Config::from_bytes(&arr) {
                    *CONFIG.lock().await = c;
                    info!("usb: config updated");
                    let _ = ep.write_ep.write(&resp(OK, CMD_SET)).await;
                } else {
                    warn!("usb: malformed config");
                    let _ = ep.write_ep.write(&resp(ERR, CMD_SET)).await;
                }
            }
            CMD_SAVE => {
                if crate::config_store::save().await {
                    let _ = ep.write_ep.write(&resp(OK, CMD_SAVE)).await;
                } else {
                    let _ = ep.write_ep.write(&resp(ERR, CMD_SAVE)).await;
                }
            }
            CMD_RESET => {
                *CONFIG.lock().await = Config::default();
                info!("usb: reset to defaults");
                let _ = ep.write_ep.write(&resp(OK, CMD_RESET)).await;
            }
            other => {
                warn!("usb: unknown cmd {=u8}", other);
                let _ = ep.write_ep.write(&resp(0x7F, other)).await;
            }
        }
    }
}
