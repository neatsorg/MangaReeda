// このファイルは RustMangaReader（Lycoris52、Apache License 2.0）のコードを元に改変したものです。

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

/// ファイルを、途中で止まっても壊れないように書く。同じフォルダの一時ファイルに書いて
/// ディスクまで書き出してから、元のファイルと置き換える
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let file_name = path
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy().into_owned());
    // 別のスレッドが同じファイルを同時に書いても一時ファイルが重ならないよう、通し番号を付ける
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary =
        path.with_file_name(format!(".{file_name}.{}.{serial}.tmp", std::process::id()));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// 履歴ファイルの場所。設定ファイルと同じフォルダに置く
pub fn history_path() -> PathBuf {
    settings_path().with_file_name("history.json")
}

/// 利用者が足すプラグインを置くフォルダ。設定ファイルと同じフォルダの plugins
pub fn plugins_dir() -> PathBuf {
    settings_path().with_file_name("plugins")
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// 作り直せるもの（プラグインの一覧、表紙の画像）を置くフォルダ
pub fn cache_dir() -> PathBuf {
    let base = if cfg!(windows) {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home().map(|home| home.join(".cache")))
    };
    base.unwrap_or_else(env::temp_dir).join(APP_DIR)
}

/// プラグインからダウンロードした本を置くフォルダの既定。ホームの MangaReeda
pub fn default_library_dir() -> PathBuf {
    home().unwrap_or_else(|| PathBuf::from(".")).join(APP_DIR)
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
    if let Ok(json) = serde_json::to_string_pretty(settings)
        && let Err(e) = write_atomic(&settings_path(), json.as_bytes())
    {
        eprintln!("設定を保存できませんでした: {e}");
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

    /// 前のファイルがあっても置き換えられ（Windows でも）、一時ファイルを残さない
    #[test]
    fn atomic_writes_replace_the_old_file() {
        let dir = std::env::temp_dir().join(format!("manga-atomic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history.json");
        write_atomic(&path, b"old").unwrap();
        // 読んでいるあいだでも置き換えられる
        let reading = std::fs::File::open(&path).unwrap();
        write_atomic(&path, b"new").unwrap();
        drop(reading);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
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
