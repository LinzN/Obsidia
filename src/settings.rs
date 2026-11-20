//! User settings, stored as JSON in the config folder.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Delete downloaded firmware once Apple stops signing it.
    pub auto_clean: bool,
    /// Look for a new Obsidia version on start-up.
    pub check_updates: bool,
    /// Retry a failed restore automatically once the device is back.
    pub auto_retry: bool,
    /// Custom firmware folder; `None` = Documents/Obsidia/Firmware.
    pub firmware_dir: Option<std::path::PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_clean: false,
            check_updates: true,
            auto_retry: true,
            firmware_dir: None,
        }
    }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("obsidia").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
    }
}
