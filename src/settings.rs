use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub fullscreen: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig { fullscreen: false }
    }
}

/// Config lives under `%APPDATA%\Tischer\PhotoMatic\config.toml`. Resolved by hand (rather
/// than via the cross-platform `directories` crate) because that crate's `dirs-sys` dependency
/// pulls in `windows-sys`/`windows-targets`, which conflicts with native-windows-gui's `winapi`
/// import of `GetWindowSubclass` (exported by comctl32.dll only by ordinal, not by name) and
/// makes the app fail to start with an "Entry Point Not Found" error. PhotoMatic is
/// Windows-only, so the cross-platform abstraction wasn't earning its keep anyway.
pub fn config_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").expect("%APPDATA% is not set");
    PathBuf::from(appdata)
        .join("Tischer")
        .join("PhotoMatic")
        .join("config.toml")
}

pub fn load() -> AppConfig {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(config: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = toml::to_string_pretty(config).expect("AppConfig always serializes");
    std::fs::write(path, text)
}
