use crate::config::AppSettings;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
#[cfg(windows)]
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

const APP_DIR: &str = "MangaReeda";
/// Settings folder name before the fork was renamed.
const LEGACY_APP_DIR: &str = "RustMangaReader";

/// Returns `base/MangaReeda/settings.json`, creating the folder. Settings left
/// in the pre-rename `base/RustMangaReader` folder are copied over once.
fn settings_path_in(base: &Path) -> PathBuf {
    let dir = base.join(APP_DIR);
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("settings.json");
    let legacy = base.join(LEGACY_APP_DIR).join("settings.json");
    if !path.exists() && legacy.is_file() {
        let _ = fs::copy(&legacy, &path);
    }
    path
}

#[cfg(not(windows))]
pub fn settings_path() -> PathBuf {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    settings_path_in(&base)
}

#[cfg(windows)]
pub fn settings_path() -> PathBuf {
    if is_packaged_app() {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        return settings_path_in(&base);
    }

    let mut exe_path = env::current_exe().expect("Failed to get current exe path");
    exe_path.pop();
    exe_path.push("settings.json");
    exe_path
}

#[cfg(windows)]
fn is_packaged_app() -> bool {
    let mut length = 0;
    let result = unsafe { GetCurrentPackageFullName(&mut length, None) };
    result != APPMODEL_ERROR_NO_PACKAGE
}

/// Loads settings, falling back to defaults when the file is missing or invalid.
pub fn load() -> AppSettings {
    let path = settings_path();
    eprintln!("Loading setting from : {:?}", path.to_str());
    if let Ok(data) = fs::read_to_string(path) {
        serde_json::from_str(&data).unwrap_or_else(|e| {
            eprintln!("Error is : {:?}", e);
            eprintln!("Failed to parse settings.json, using defaults.");
            AppSettings::default()
        })
    } else {
        AppSettings::default()
    }
}

pub fn save(settings: &AppSettings) {
    if let Ok(json) = serde_json::to_string_pretty(settings) {
        let _ = fs::write(settings_path(), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("manga-settings-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        base
    }

    #[test]
    fn legacy_settings_are_copied_once() {
        let base = temp_base("legacy");
        fs::create_dir(base.join(LEGACY_APP_DIR)).unwrap();
        fs::write(base.join(LEGACY_APP_DIR).join("settings.json"), "old").unwrap();

        let path = settings_path_in(&base);
        assert_eq!(path, base.join(APP_DIR).join("settings.json"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "old");

        // Once the new file exists, it is never overwritten by the legacy one.
        fs::write(&path, "new").unwrap();
        settings_path_in(&base);
        assert_eq!(fs::read_to_string(&path).unwrap(), "new");
        fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn fresh_install_starts_without_settings() {
        let base = temp_base("fresh");
        let path = settings_path_in(&base);
        assert!(base.join(APP_DIR).is_dir());
        assert!(!path.exists());
        fs::remove_dir_all(&base).unwrap();
    }
}
