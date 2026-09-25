//! The shared, lockable [`Config`] instance.
//!
//! Split out of [`crate::config`] so that module stays free of external crates
//! and can be compiled standalone by `just test` / `just clippy-host`, which use
//! bare `rustc --test` with no `--extern`. Only this tiny bus needs
//! `embassy_sync`; the record format itself is pure and therefore testable.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

use crate::config::Config;

/// Shared config. Lock with `.lock().await` from any task.
pub static CONFIG: Mutex<CriticalSectionRawMutex, Config> = Mutex::new(Config::default());
