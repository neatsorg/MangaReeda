//! Chromium 系ブラウザを探して、Kindle 専用のプロファイルで起動する。
//! 版は問わない。使う CDP の命令は古くからあるものだけにしている。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// 見つかったブラウザ
#[derive(Clone, Debug)]
pub enum Browser {
    /// 実行ファイル
    Native(PathBuf),
    /// flatpak のアプリ ID
    Flatpak(String),
}

impl Browser {
    pub fn describe(&self) -> String {
        match self {
            Self::Native(path) => path.display().to_string(),
            Self::Flatpak(id) => format!("flatpak {id}"),
        }
    }
}

/// ブラウザを探す。指定があればそれを使う
pub fn find(explicit: Option<&str>) -> Option<Browser> {
    if let Some(explicit) = explicit.map(str::to_owned).or_else(|| env::var("MANGAREEDA_BROWSER").ok()) {
        if let Some(id) = explicit.strip_prefix("flatpak:") {
            return Some(Browser::Flatpak(id.to_owned()));
        }
        return Some(Browser::Native(PathBuf::from(explicit)));
    }
    native_candidates()
        .into_iter()
        .find(|path| path.is_file())
        .map(Browser::Native)
        .or_else(find_flatpak)
}

#[cfg(windows)]
fn native_candidates() -> Vec<PathBuf> {
    // Edge は Windows に最初から入っているので先に探す
    let mut list = Vec::new();
    let roots = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"];
    let tails = [
        r"Microsoft\Edge\Application\msedge.exe",
        r"Google\Chrome\Application\chrome.exe",
        r"BraveSoftware\Brave-Browser\Application\brave.exe",
        r"Vivaldi\Application\vivaldi.exe",
        r"Chromium\Application\chrome.exe",
    ];
    for tail in tails {
        for root in roots {
            if let Some(base) = env::var_os(root) {
                list.push(PathBuf::from(base).join(tail));
            }
        }
    }
    list
}

#[cfg(not(windows))]
fn native_candidates() -> Vec<PathBuf> {
    let names = [
        "chromium",
        "chromium-browser",
        "google-chrome-stable",
        "google-chrome",
        "microsoft-edge-stable",
        "microsoft-edge",
        "brave-browser",
        "brave",
        "vivaldi-stable",
        "vivaldi",
    ];
    let paths: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();
    names
        .iter()
        .flat_map(|name| paths.iter().map(move |dir| dir.join(name)))
        .collect()
}

#[cfg(windows)]
fn find_flatpak() -> Option<Browser> {
    None
}

#[cfg(not(windows))]
fn find_flatpak() -> Option<Browser> {
    let ids = [
        "org.chromium.Chromium",
        "com.google.Chrome",
        "com.microsoft.Edge",
        "com.brave.Browser",
        "com.vivaldi.Vivaldi",
        "io.github.ungoogled_software.ungoogled_chromium",
    ];
    ids.iter()
        .find(|id| {
            Command::new("flatpak")
                .args(["info", id])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .map(|id| Browser::Flatpak((*id).to_owned()))
}

/// 起動のしかた
pub struct Launch<'a> {
    pub profile: &'a Path,
    /// --app で開く URL
    pub url: &'a str,
    pub fullscreen: bool,
    pub headless: bool,
}

/// このプロセスが起動したブラウザ。途中で失敗したときに止めるために持っておく
static LAUNCHED: Mutex<Option<Child>> = Mutex::new(None);

/// このプロセスが起動したブラウザが動いていれば止める（途中で失敗して、後始末をしないまま終わるとき）
pub fn kill_launched() {
    if let Ok(mut launched) = LAUNCHED.lock()
        && let Some(mut child) = launched.take()
        && matches!(child.try_wait(), Ok(None))
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// プロファイルの DevToolsActivePort から、ブラウザ全体の WebSocket の URL を作る
pub fn devtools_url(profile: &Path) -> Option<String> {
    let text = fs::read_to_string(profile.join("DevToolsActivePort")).ok()?;
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    let path = lines.next()?.trim();
    Some(format!("ws://127.0.0.1:{port}{path}"))
}

/// ブラウザを起動し、DevTools の URL が書かれるまで待つ
pub fn launch(browser: &Browser, launch: &Launch) -> Result<String, String> {
    fs::create_dir_all(launch.profile)
        .map_err(|e| format!("プロファイルのフォルダを作れません（{}）: {e}", launch.profile.display()))?;
    let port_file = launch.profile.join("DevToolsActivePort");
    // 前回の残りを読まないように消しておく
    let _ = fs::remove_file(&port_file);

    let mut args = vec![
        format!("--user-data-dir={}", launch.profile.display()),
        "--remote-debugging-port=0".to_owned(),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
    ];
    // 試すための起動の指定（空白で区切る）。GPU の使い方を変えるときなどに使う
    if let Ok(extra) = env::var("MANGAREEDA_BROWSER_ARGS") {
        args.extend(extra.split_whitespace().map(str::to_owned));
    }
    if launch.headless {
        args.push("--headless=new".to_owned());
        args.push(launch.url.to_owned());
    } else {
        args.push(format!("--app={}", launch.url));
        if launch.fullscreen {
            args.push("--start-fullscreen".to_owned());
        }
    }

    let mut command = match browser {
        Browser::Native(path) => Command::new(path),
        Browser::Flatpak(id) => {
            let mut command = Command::new("flatpak");
            // プロファイルのフォルダだけをサンドボックスの中から見えるようにする
            // flatpak の命令が止められたら、サンドボックスの中のブラウザも終わるようにする
            command
                .arg("run")
                .arg("--die-with-parent")
                .arg(format!("--filesystem={}", launch.profile.display()))
                .arg(id);
            command
        }
    };
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command
        .spawn()
        .map_err(|e| format!("ブラウザを起動できません（{}）: {e}", browser.describe()))?;
    if let Ok(mut launched) = LAUNCHED.lock() {
        *launched = Some(child);
    }

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if let Some(url) = devtools_url(launch.profile) {
            return Ok(url);
        }
        thread::sleep(Duration::from_millis(200));
    }
    kill_launched();
    Err(format!(
        "ブラウザが DevTools の待ち受けを始めませんでした（{}）",
        browser.describe()
    ))
}
