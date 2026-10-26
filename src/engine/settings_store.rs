use crate::config::AppSettings;
use std::env;
use std::fs;
use std::path::PathBuf;
#[cfg(windows)]
use windows::Win32::Foundation::APPMODEL_ERROR_NO_PACKAGE;
#[cfg(windows)]
use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;

#[cfg(not(windows))]
pub fn settings_path() -> PathBuf {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("RustMangaReader");
    let _ = fs::create_dir_all(&dir);
    dir.join("settings.json")
}

#[cfg(windows)]
pub fn settings_path() -> PathBuf {
    if is_packaged_app() {
        let base = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join("RustMangaReader");
        let _ = fs::create_dir_all(&dir);
        return dir.join("settings.json");
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
