//! The shared, lockable [`Config`] instance.
//!
//! Split out of [`crate::config`] so that module stays free of external crates
//! and `just test` can compile it with bare `rustc --test`.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use crate::config::Config;

/// Shared config. Lock with `.lock().await` from any task.
pub static CONFIG: Mutex<CriticalSectionRawMutex, Config> = Mutex::new(Config::default());
