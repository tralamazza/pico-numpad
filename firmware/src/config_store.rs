//! Flash-backed persistence for [`Config`].
//!
//! The config lives in the last 64 KiB of the 4 MiB QSPI flash, which is kept out
//! of the linker's code region by `memory.x`. One 4 KiB sector holds one 32-byte
//! record; saving erases the sector and programs the record.

// Flash offsets are `u32` (the NorFlash API) while flash sizes are `usize` (they
// are generic array parameters). On the 4 MiB RP2350 part every conversion here
// is loss-free, so the narrowing casts are intentional.
#![allow(clippy::cast_possible_truncation)]

use core::ops::Range;

use defmt::{info, warn};
use embassy_rp::flash::{Async, ERASE_SIZE, Flash};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::once_lock::OnceLock;
use embedded_storage_async::nor_flash::NorFlash;
use sequential_storage::cache::Cache;
use sequential_storage::map::{MapConfig, MapStorage, PostcardValue};
use serde::{Deserialize, Serialize};
use trouble_host::prelude::BondInformation;

use crate::config::{CONFIG_LEN, Config};
use crate::config_bus::CONFIG;
use crate::host_slots::Slots;

/// Total QSPI flash on the Pico 2 W.
pub const FLASH_SIZE: usize = 4 * 1024 * 1024;
/// Offset of the config sector from the start of flash (`0x103F_0000`).
pub const CONFIG_OFFSET: u32 = (FLASH_SIZE - 64 * 1024) as u32;
/// Erase granularity as a flash offset.
const SECTOR: u32 = ERASE_SIZE as u32;
/// Offset of the BLE bond sector (`0x103F_1000`).
pub const BOND_OFFSET: u32 = CONFIG_OFFSET + SECTOR;
/// Size of the BLE bond storage range.
pub const BOND_LEN: u32 = 8 * 1024;
// A separate journal makes migration non-destructive and prevents a cleared
// slot from resurrecting the legacy bond after a restart.
const HOSTS_OFFSET: u32 = BOND_OFFSET + BOND_LEN;
const HOSTS_LEN: u32 = 8 * 1024;

pub type HostSlots = Slots<BondInformation>;

impl PostcardValue<'_> for HostSlots {}

pub type ConfigFlash = Flash<'static, FLASH, Async, FLASH_SIZE>;

#[repr(align(4))]
struct AlignedBuf([u8; CONFIG_LEN]);

struct Store {
    flash: Mutex<CriticalSectionRawMutex, ConfigFlash>,
}

static STORE: OnceLock<Store> = OnceLock::new();

/// Move the flash driver into the global store so the USB task can save to it.
pub fn init(flash: ConfigFlash) {
    let _ = STORE.init(Store {
        flash: Mutex::new(flash),
    });
}

/// Read the config from flash into the shared [`CONFIG`].
///
/// Returns the loaded config, or `None` if the sector is blank/corrupt.
pub async fn load(flash: &mut ConfigFlash) -> Option<Config> {
    let mut buf = AlignedBuf([0u8; CONFIG_LEN]);
    if flash.read(CONFIG_OFFSET, &mut buf.0).await.is_err() {
        warn!("config read failed");
        return None;
    }
    if let Some(c) = Config::from_bytes(&buf.0) {
        info!("config loaded from flash");
        Some(c)
    } else {
        info!("no valid config in flash");
        None
    }
}

/// Persist the current shared [`CONFIG`] to flash.
pub async fn save() -> bool {
    let bytes = CONFIG.lock().await.to_bytes();
    let Some(store) = STORE.try_get() else {
        warn!("config store not initialised");
        return false;
    };
    let mut flash = store.flash.lock().await;
    if flash
        .erase(CONFIG_OFFSET, CONFIG_OFFSET + SECTOR)
        .await
        .is_err()
    {
        warn!("config erase failed");
        return false;
    }
    if flash.write(CONFIG_OFFSET, &bytes).await.is_err() {
        warn!("config write failed");
        return false;
    }
    info!("config saved to flash");
    true
}

#[derive(Serialize, Deserialize)]
struct StoredBond(BondInformation);

impl PostcardValue<'_> for StoredBond {}

fn bond_range() -> Range<u32> {
    BOND_OFFSET..(BOND_OFFSET + BOND_LEN)
}

/// Load the persisted BLE bond, if any.
async fn load_bond() -> Result<Option<BondInformation>, &'static str> {
    let store = STORE.try_get().ok_or("config store not initialised")?;
    let mut flash = store.flash.lock().await;
    let Ok(config) = MapConfig::try_new(bond_range()) else {
        warn!("bond range invalid");
        return Err("bond range invalid");
    };
    let mut map = MapStorage::new(&mut *flash, config, Cache::new_uncached());
    let mut buf = [0u8; 256];
    match map.fetch_item(&mut buf, &()).await {
        Ok(Some(StoredBond(bond))) => {
            info!("bond loaded from flash");
            Ok(Some(bond))
        }
        Ok(None) => Ok(None),
        Err(e) => {
            warn!("bond load failed: {:?}", defmt::Debug2Format(&e));
            Err("legacy bond read failed")
        }
    }
}

/// Load the host-slot journal or migrate the original bond into slot 1 once.
pub async fn load_hosts() -> Result<HostSlots, &'static str> {
    let store = STORE.try_get().ok_or("config store not initialised")?;
    let existing = {
        let mut flash = store.flash.lock().await;
        let config = MapConfig::try_new(HOSTS_OFFSET..HOSTS_OFFSET + HOSTS_LEN)
            .map_err(|_| "host slot range invalid")?;
        let mut map = MapStorage::new(&mut *flash, config, Cache::new_uncached());
        let mut buf = [0u8; 1024];
        map.fetch_item::<HostSlots>(&mut buf, &())
            .await
            .map_err(|_| "host slot read failed")?
    };
    if let Some(hosts) = existing {
        if !hosts.valid() {
            return Err("invalid host slot data");
        }
        info!("host slots loaded; active={}", hosts.active + 1);
        return Ok(hosts);
    }
    let hosts = HostSlots::from_legacy(load_bond().await?);
    if !save_hosts(&hosts).await {
        return Err("host slot migration failed");
    }
    info!("host slots initialised; legacy bond migrated to slot 1");
    Ok(hosts)
}

/// Atomically journal all slots and the active selection as one record.
/// Callers update their RAM state only after this succeeds.
pub async fn save_hosts(hosts: &HostSlots) -> bool {
    if !hosts.valid() {
        return false;
    }
    let Some(store) = STORE.try_get() else {
        warn!("config store not initialised");
        return false;
    };
    let mut flash = store.flash.lock().await;
    let Ok(config) = MapConfig::try_new(HOSTS_OFFSET..HOSTS_OFFSET + HOSTS_LEN) else {
        warn!("host slot range invalid");
        return false;
    };
    let mut map = MapStorage::new(&mut *flash, config, Cache::new_uncached());
    let mut buf = [0u8; 1024];
    match map.store_item(&mut buf, &(), hosts).await {
        Ok(()) => {
            info!("host slots saved; active={}", hosts.active + 1);
            true
        }
        Err(e) => {
            warn!("host slot save failed: {:?}", defmt::Debug2Format(&e));
            false
        }
    }
}

/// Explicit recovery reset only. This erases all host bonds, not key/LED config.
/// Erase the legacy region first so an interrupted reset cannot migrate an old
/// bond back into an empty journal. Failure leaves the caller in recovery mode.
pub async fn reset_hosts() -> bool {
    let Some(store) = STORE.try_get() else {
        return false;
    };
    {
        let mut flash = store.flash.lock().await;
        if flash.erase(BOND_OFFSET, HOSTS_OFFSET).await.is_err() {
            warn!("legacy bond reset failed");
            return false;
        }
        if flash
            .erase(HOSTS_OFFSET, HOSTS_OFFSET + HOSTS_LEN)
            .await
            .is_err()
        {
            warn!("host journal reset failed");
            return false;
        }
    }
    save_hosts(&HostSlots::from_legacy(None)).await
}
