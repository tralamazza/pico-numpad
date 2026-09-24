//! Flash-backed persistence for [`Config`].
//!
//! The config lives in the last 64 KiB of the 4 MiB QSPI flash, which is kept out
//! of the linker's code region by `memory.x`. One 4 KiB sector holds one 32-byte
//! record; saving erases the sector and programs the record.

use core::ops::Range;

use defmt::{info, warn};
use embassy_rp::flash::{Async, Flash, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::once_lock::OnceLock;
use embedded_storage_async::nor_flash::NorFlash;
use sequential_storage::cache::NoCache;
use sequential_storage::map::{MapConfig, MapStorage, PostcardValue};
use serde::{Deserialize, Serialize};
use trouble_host::prelude::BondInformation;

use crate::config::{Config, CONFIG, CONFIG_LEN};

/// Total QSPI flash on the Pico 2 W.
pub const FLASH_SIZE: usize = 4 * 1024 * 1024;
/// Offset of the config sector from the start of flash (0x103F_0000).
pub const CONFIG_OFFSET: u32 = (FLASH_SIZE - 64 * 1024) as u32;
/// Offset of the BLE bond sector (0x103F_1000).
pub const BOND_OFFSET: u32 = CONFIG_OFFSET + ERASE_SIZE as u32;
/// Size of the BLE bond storage range.
pub const BOND_LEN: u32 = 8 * 1024;

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
    match Config::from_bytes(&buf.0) {
        Some(c) => {
            info!("config loaded from flash");
            Some(c)
        }
        None => {
            info!("no valid config in flash");
            None
        }
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
        .erase(CONFIG_OFFSET, CONFIG_OFFSET + ERASE_SIZE as u32)
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

impl<'a> PostcardValue<'a> for StoredBond {}

fn bond_range() -> Range<u32> {
    BOND_OFFSET..(BOND_OFFSET + BOND_LEN)
}

/// Load the persisted BLE bond, if any.
pub async fn load_bond() -> Option<BondInformation> {
    let store = STORE.try_get()?;
    let mut flash = store.flash.lock().await;
    let Some(config) = MapConfig::try_new(bond_range()) else {
        warn!("bond range invalid");
        return None;
    };
    let mut map = MapStorage::new(&mut *flash, config, NoCache::new());
    let mut buf = [0u8; 256];
    match map.fetch_item(&mut buf, &()).await {
        Ok(Some(StoredBond(bond))) => {
            info!("bond loaded from flash");
            Some(bond)
        }
        Ok(None) => None,
        Err(e) => {
            warn!("bond load failed: {:?}", defmt::Debug2Format(&e));
            None
        }
    }
}

/// Persist a BLE bond to flash.
pub async fn save_bond(bond: &BondInformation) -> bool {
    let Some(store) = STORE.try_get() else {
        warn!("config store not initialised");
        return false;
    };
    let mut flash = store.flash.lock().await;
    let Some(config) = MapConfig::try_new(bond_range()) else {
        warn!("bond range invalid");
        return false;
    };
    let mut map = MapStorage::new(&mut *flash, config, NoCache::new());
    let mut buf = [0u8; 256];
    match map.store_item(&mut buf, &(), &StoredBond(bond.clone())).await {
        Ok(()) => {
            info!("bond saved to flash");
            true
        }
        Err(e) => {
            warn!("bond save failed: {:?}", defmt::Debug2Format(&e));
            false
        }
    }
}
