use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub fullscreen: bool,
    /// Full path to the last project file loaded or saved, used to default the
    /// directory shown in the Load / Save As dialogs. `None` until a project has
    /// been loaded or saved at least once.
    #[serde(default)]
    pub recent_path: Option<PathBuf>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            fullscreen: false,
            recent_path: None,
        }
    }
}

/// Config lives under `%APPDATA%\Tischer\PhotoMatic\config.json`. Resolved by hand (rather
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
        .join("config.json")
}

pub fn load() -> AppConfig {
    load_from(&config_path())
}

pub fn save(config: &AppConfig) -> std::io::Result<()> {
    save_to(&config_path(), config)
}

fn load_from(path: &Path) -> AppConfig {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_to(path: &Path, config: &AppConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(config).expect("AppConfig always serializes");
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("photomatic-test-{}-{}", std::process::id(), name))
    }

    #[test]
    fn round_trips_through_json() {
        let path = temp_path("config-roundtrip.json");
        let config = AppConfig {
            fullscreen: true,
            recent_path: Some(PathBuf::from(r"C:\projects\demo.json")),
        };

        save_to(&path, &config).unwrap();
        let loaded = load_from(&path);

        assert_eq!(loaded, config);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_falls_back_to_default() {
        let path = temp_path("config-missing.json");
        std::fs::remove_file(&path).ok();

        let loaded = load_from(&path);

        assert_eq!(loaded, AppConfig::default());
    }
}
