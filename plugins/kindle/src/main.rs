//! Kindle Cloud Reader を Chromium 系ブラウザで開き、MangaReeda のキー設定で操作できるようにする試作。
//! 本は表示するだけで、ページの画像を取り出したり保存したりはしない。
//!
//! 使い方:
//!   mangareeda-kindle open [--url URL | --asin ASIN] [--fullscreen]
//!   mangareeda-kindle probe            開いているページのボタンなどを JSON で書き出す（調査用）
//!   mangareeda-kindle eval --expr JS   開いているリーダーで式を評価して書き出す（調査用）
//!   mangareeda-kindle library          ライブラリの本を list と同じ形の JSON で書き出す
//! 共通: [--browser PATH|flatpak:ID] [--settings settings.json] [--site co.jp|com]

mod browser;
mod cdp;
mod keys;

use cdp::Cdp;
use keys::Operation;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

const INJECT: &str = include_str!("inject.js");
const BINDING: &str = "__mangareedaAction";

struct Options {
    command: String,
    url: Option<String>,
    /// eval で評価する式
    expr: Option<String>,
    asin: Option<String>,
    browser: Option<String>,
    settings: Option<PathBuf>,
    site: String,
    fullscreen: bool,
    /// 調べもの用。ウィンドウを出さずに開く
    headless: bool,
    /// probe で測る前に、画面をこの大きさに見せかける（幅, 高さ）
    viewport: Option<(u32, u32)>,
}

fn parse_args() -> Result<Options, String> {
    let mut args = env::args().skip(1);
    let command = args.next().ok_or("命令（open / probe / library）を指定してください")?;
    let mut options = Options {
        command,
        url: None,
        expr: None,
        asin: None,
        browser: None,
        settings: None,
        site: "co.jp".into(),
        fullscreen: false,
        headless: false,
        viewport: None,
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} の値がありません"));
        match arg.as_str() {
            "--url" => options.url = Some(value()?),
            "--expr" => options.expr = Some(value()?),
            "--asin" => options.asin = Some(value()?),
            "--browser" => options.browser = Some(value()?),
            "--settings" => options.settings = Some(PathBuf::from(value()?)),
            "--site" => options.site = value()?,
            "--fullscreen" => options.fullscreen = true,
            "--headless" => options.headless = true,
            "--viewport" => {
                let text = value()?;
                let (w, h) = text.split_once('x').ok_or("--viewport は 幅x高さ で指定してください")?;
                options.viewport = Some((
                    w.parse().map_err(|_| "--viewport の幅が数ではありません")?,
                    h.parse().map_err(|_| "--viewport の高さが数ではありません")?,
                ));
            }
            other => return Err(format!("知らない引数です: {other}")),
        }
    }
    Ok(options)
}

/// プロファイルなどを置くフォルダ。ログインの状態（Cookie）は設定ではないのでデータの側に置く
fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let base = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.unwrap_or_else(|| PathBuf::from("."))
        .join("MangaReeda")
        .join("kindle")
}

/// MangaReeda の設定ファイル（キー設定を読むだけで、書き込まない）
fn settings_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    #[cfg(windows)]
    {
        // 試作: 本体と同じフォルダか、ストア版の場所
        if let Some(path) = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|d| d.join("settings.json")))
            .filter(|p| p.is_file())
        {
            return path;
        }
        return env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("MangaReeda")
            .join("settings.json");
    }
    #[cfg(not(windows))]
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join("MangaReeda")
        .join("settings.json")
}

fn library_url(site: &str) -> String {
    format!("https://read.amazon.{site}/kindle-library")
}

/// 動いているブラウザにつなぐ。無ければ起動する
fn connect(options: &Options, url: &str, headless: bool) -> Result<(Cdp, bool), String> {
    let profile = data_dir().join("profile");
    if let Some(ws) = browser::devtools_url(&profile)
        && let Ok(cdp) = Cdp::connect(&ws)
    {
        return Ok((cdp, false));
    }
    let found = browser::find(options.browser.as_deref()).ok_or(
        "Chromium 系のブラウザ（Edge、Chrome、Chromium など）が見つかりません。--browser で場所を指定してください",
    )?;
    eprintln!("ブラウザ: {}", found.describe());
    let ws = browser::launch(
        &found,
        &browser::Launch {
            profile: &profile,
            url,
            fullscreen: options.fullscreen,
            headless,
        },
    )?;
    Ok((Cdp::connect(&ws)?, true))
}

fn page_targets(cdp: &mut Cdp) -> Result<Vec<(String, String)>, String> {
    let targets = cdp.call("Target.getTargets", json!({}), None)?;
    Ok(targets["targetInfos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["type"] == "page")
        .filter_map(|t| Some((t["targetId"].as_str()?.to_owned(), t["url"].as_str()?.to_owned())))
        .collect())
}

fn attach(cdp: &mut Cdp, target: &str) -> Result<String, String> {
    let reply = cdp.call(
        "Target.attachToTarget",
        json!({"targetId": target, "flatten": true}),
        None,
    )?;
    reply["sessionId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "ページにつなげません".to_owned())
}

/// ページにキーの横取りを入れる。この先の読み込みでも働くようにする
fn install(cdp: &mut Cdp, session: &str, script: &str) -> Result<(), String> {
    cdp.call("Runtime.enable", json!({}), Some(session))?;
    cdp.call("Page.enable", json!({}), Some(session))?;
    cdp.call("Runtime.addBinding", json!({"name": BINDING}), Some(session))?;
    cdp.call(
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": script}),
        Some(session),
    )?;
    cdp.evaluate(session, script)?;
    Ok(())
}

struct Reader {
    cdp: Cdp,
    operations: BTreeMap<String, Operation>,
    script: String,
    /// sessionId → targetId
    sessions: HashMap<String, String>,
    site: String,
    /// 送り直したキーが iframe などで横取りされて戻ってきたときに捨てる
    suppress: Option<(String, Instant)>,
    /// 単ページで読むか。Cloud Reader は窓が縦長だと1ページずつ表示するので、
    /// ページの中で縦長の枠を作り、リーダーには窓の幅をその枠の幅と伝える
    single: bool,
}

fn is_reader_url(url: &str) -> bool {
    url.contains("/manga/")
}

impl Reader {
    /// ページに単ページか見開きかを伝える。枠の大きさはページの側で窓に合わせて決める（inject.js）
    fn apply_layout(&mut self, session: &str) -> Result<(), String> {
        self.cdp.evaluate(
            session,
            &format!(
                "window.__mangareedaSetSingle && window.__mangareedaSetSingle({})",
                self.single
            ),
        )?;
        Ok(())
    }

    fn attach_page(&mut self, target: &str) {
        if self.sessions.values().any(|t| t == target) {
            return;
        }
        match attach(&mut self.cdp, target).and_then(|session| {
            install(&mut self.cdp, &session, &self.script)?;
            Ok(session)
        }) {
            Ok(session) => {
                self.sessions.insert(session, target.to_owned());
            }
            Err(e) => eprintln!("ページの準備に失敗しました: {e}"),
        }
    }

    fn run_action(&mut self, session: &str, action: &str) -> Result<(), String> {
        if let Some((suppressed, at)) = self.suppress.take()
            && suppressed == action
            && at.elapsed() < Duration::from_millis(150)
        {
            return Ok(());
        }
        let Some(operation) = self.operations.get(action).cloned() else {
            return Ok(());
        };
        eprintln!("{action} → {operation:?}");
        match operation {
            Operation::Key(key) => {
                // 送るキーがほかの動作に割り当てられていても、横取りされないようにする
                self.cdp.evaluate(session, &format!("window.__mangareedaPassKey = {}", json!(key)))?;
                let (code, vk, text) = keys::key_event_fields(&key);
                for kind in ["keyDown", "keyUp"] {
                    let mut params = json!({
                        "type": kind, "key": key, "code": code,
                        "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk,
                    });
                    if kind == "keyDown"
                        && let Some(text) = &text
                    {
                        params["text"] = json!(text);
                    }
                    self.cdp.call("Input.dispatchKeyEvent", params, Some(session))?;
                }
                self.suppress = Some((action.to_owned(), Instant::now()));
            }
            Operation::Click(selector) => {
                let found = self.cdp.evaluate(
                    session,
                    &format!(
                        "(() => {{ const e = document.querySelector({}); if (!e) return false; e.click(); return true; }})()",
                        json!(selector)
                    ),
                )?;
                if found != json!(true) {
                    eprintln!("押すボタンが見つかりません: {selector}");
                }
            }
            Operation::Js(code) => {
                self.cdp.evaluate(session, &code)?;
            }
            Operation::Builtin(name) => self.builtin(session, &name)?,
        }
        Ok(())
    }

    fn builtin(&mut self, session: &str, name: &str) -> Result<(), String> {
        let target = self.sessions.get(session).cloned().unwrap_or_default();
        match name {
            "fullscreen" => {
                let window = self
                    .cdp
                    .call("Browser.getWindowForTarget", json!({"targetId": target}), None)?;
                let state = window["bounds"]["windowState"].as_str().unwrap_or("normal");
                let next = if state == "fullscreen" { "normal" } else { "fullscreen" };
                self.cdp.call(
                    "Browser.setWindowBounds",
                    json!({"windowId": window["windowId"], "bounds": {"windowState": next}}),
                    None,
                )?;
            }
            "single" => {
                self.single = !self.single;
                eprintln!("{}", if self.single { "単ページ" } else { "見開き" });
                self.apply_layout(session)?;
            }
            "close" => {
                self.cdp.call("Target.closeTarget", json!({"targetId": target}), None)?;
            }
            "library" => {
                self.cdp
                    .call("Page.navigate", json!({"url": library_url(&self.site)}), Some(session))?;
            }
            other => eprintln!("知らない builtin です: {other}"),
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), String> {
        self.cdp
            .call("Target.setDiscoverTargets", json!({"discover": true}), None)?;
        for (target, _) in page_targets(&mut self.cdp)? {
            self.attach_page(&target);
        }
        loop {
            let event = match self.cdp.next_event() {
                Ok(event) => event,
                // ブラウザを閉じると接続が切れる
                Err(_) => return Ok(()),
            };
            match event.method.as_str() {
                "Runtime.bindingCalled" if event.params["name"] == BINDING => {
                    let action = event.params["payload"].as_str().unwrap_or_default().to_owned();
                    if let Some(session) = event.session
                        && let Err(e) = self.run_action(&session, &action)
                    {
                        eprintln!("{action} に失敗しました: {e}");
                    }
                }
                "Target.targetCreated" if event.params["targetInfo"]["type"] == "page" => {
                    if let Some(target) = event.params["targetInfo"]["targetId"].as_str() {
                        self.attach_page(target);
                    }
                }
                // ライブラリとリーダーを行き来したら、見せかけをかけ直す
                "Page.frameNavigated" | "Page.navigatedWithinDocument"
                    if event.params["frame"]["parentId"].is_null() =>
                {
                    if let Some(session) = event.session
                        && let Err(e) = self.apply_layout(&session)
                    {
                        eprintln!("単ページの切り替えに失敗しました: {e}");
                    }
                }
                "Target.detachedFromTarget" => {
                    if let Some(session) = event.params["sessionId"].as_str() {
                        self.sessions.remove(session);
                    }
                    if self.sessions.is_empty() {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
    }
}

fn open(options: &Options) -> Result<(), String> {
    let settings_file = settings_path(options.settings.clone());
    let settings: Value = std::fs::read_to_string(&settings_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| {
            eprintln!("キー設定を読めません（{}）。キーは置き換えません", settings_file.display());
            json!({})
        });
    let actions_file = data_dir().join("actions.json");
    let operations = keys::load_operations(&actions_file)?;
    let matchers = keys::matchers(&settings, &operations);
    for m in &matchers {
        eprintln!("キー: {} ← {:?}{:?}", m.action, m.codes, m.keys);
    }
    let script = INJECT.replace("__MATCHERS__", &serde_json::to_string(&matchers).unwrap());

    let url = match (&options.url, &options.asin) {
        (Some(url), _) => url.clone(),
        // マンガのリーダーは /manga/<ASIN>（文字の本は未確認）
        (None, Some(asin)) => format!("https://read.amazon.{}/manga/{asin}", options.site),
        (None, None) => library_url(&options.site),
    };
    let (mut cdp, launched) = connect(options, &url, options.headless)?;
    if !launched {
        // すでに開いているので、新しいウィンドウで開く
        cdp.call("Target.createTarget", json!({"url": url, "newWindow": true}), None)?;
    }
    Reader {
        cdp,
        operations,
        script,
        sessions: HashMap::new(),
        site: options.site.clone(),
        suppress: None,
        single: false,
    }
    .run()
}

/// 調査用: ボタンや aria-label を持つ要素、iframe、ASIN らしい id を書き出す
const PROBE: &str = r#"(() => {
  const pick = (e) => ({
    tag: e.tagName.toLowerCase(), id: e.id || undefined,
    class: (typeof e.className === 'string' && e.className) || undefined,
    aria: e.getAttribute('aria-label') || undefined, title: e.title || undefined,
    href: e.getAttribute('href') || undefined,
    text: (e.innerText || '').trim().slice(0, 40) || undefined,
  });
  const q = (s) => [...document.querySelectorAll(s)].slice(0, 300).map(pick);
  return {
    url: location.href, title: document.title,
    size: [innerWidth, innerHeight],
    controls: q('button, [role=button], [role=menuitem], [role=radio], [role=switch], input, select, [aria-label]'),
    iframes: q('iframe'),
    asinLike: q('[id*="B0"], [href*="asin"]').slice(0, 50),
    // 見えているページらしい要素の位置と大きさ（中身は読まない）
    pages: [...document.querySelectorAll('canvas, img, svg image')]
      .map((e) => [e, e.getBoundingClientRect()])
      .filter(([, r]) => r.width > 100 && r.height > 100 && r.right > 0 && r.left < innerWidth)
      .map(([e, r]) => ({ tag: e.tagName.toLowerCase(), class: (typeof e.className === 'string' && e.className) || undefined,
        x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) })),
  };
})()"#;

/// 調査用: リーダーのページで式を評価する
fn eval(options: &Options) -> Result<(), String> {
    let expr = options.expr.as_deref().ok_or("--expr で式を指定してください")?;
    let profile = data_dir().join("profile");
    let ws = browser::devtools_url(&profile).ok_or("先に open でブラウザを開いてください")?;
    let mut cdp = Cdp::connect(&ws)?;
    for (target, url) in page_targets(&mut cdp)? {
        if !is_reader_url(&url) {
            continue;
        }
        let session = attach(&mut cdp, &target)?;
        let value = cdp.evaluate(&session, expr)?;
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
        cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None)?;
    }
    Ok(())
}

fn probe(options: &Options) -> Result<(), String> {
    let profile = data_dir().join("profile");
    let ws = browser::devtools_url(&profile).ok_or("先に open でブラウザを開いてください")?;
    let mut cdp = Cdp::connect(&ws)?;
    let mut pages = Vec::new();
    for (target, _) in page_targets(&mut cdp)? {
        let session = attach(&mut cdp, &target)?;
        // 見かけの大きさはこのセッションのあいだだけ効く
        if let Some((width, height)) = options.viewport {
            cdp.call(
                "Emulation.setDeviceMetricsOverride",
                json!({"width": width, "height": height, "deviceScaleFactor": 0, "mobile": false}),
                Some(&session),
            )?;
            thread::sleep(Duration::from_secs(3));
        }
        pages.push(cdp.evaluate(&session, PROBE)?);
        cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None)?;
    }
    println!("{}", serde_json::to_string_pretty(&pages).unwrap());
    Ok(())
}

/// ライブラリの本を集める。作りに頼らないよう、ASIN を含む id や href から拾う
const LIBRARY: &str = r#"(async () => {
  const asinRe = /\b(B0[0-9A-Z]{8}|\d{9}[\dX])\b/;
  const seen = new Map();
  const collect = () => {
    for (const e of document.querySelectorAll('[id], a[href]')) {
      const src = (e.id || '') + ' ' + (e.getAttribute('href') || '');
      const m = src.match(asinRe);
      if (!m) continue;
      const asin = m[1];
      const box = e.closest('li, [role=listitem], [role=gridcell]') || e;
      const img = box.querySelector('img');
      const title = (img && img.alt) || box.getAttribute('aria-label') ||
        (box.querySelector('[id^=title], [class*=title]') || {}).innerText || '';
      const old = seen.get(asin) || {};
      seen.set(asin, {
        asin,
        title: old.title || title.trim(),
        cover_url: old.cover_url || (img && img.src) || undefined,
        url: old.url || (e.tagName === 'A' ? e.href : undefined),
      });
    }
  };
  // 下まで送りながら、増えなくなるまで集める
  let last = -1, still = 0;
  while (still < 3) {
    collect();
    if (seen.size === last) still++; else still = 0;
    last = seen.size;
    const scroller = [...document.querySelectorAll('*')].find(
      (e) => e.scrollHeight > e.clientHeight + 50 && /(auto|scroll)/.test(getComputedStyle(e).overflowY)) || document.scrollingElement;
    scroller.scrollTop = scroller.scrollHeight;
    await new Promise((r) => setTimeout(r, 1200));
  }
  return { url: location.href, books: [...seen.values()] };
})()"#;

fn library(options: &Options) -> Result<(), String> {
    let url = library_url(&options.site);
    let (mut cdp, launched) = connect(options, &url, true)?;
    let target = if launched {
        page_targets(&mut cdp)?
            .into_iter()
            .next()
            .map(|(t, _)| t)
            .ok_or("ページがありません")?
    } else {
        cdp.call("Target.createTarget", json!({"url": url, "background": true}), None)?["targetId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let session = attach(&mut cdp, &target)?;
    // 読み込みが落ち着くまで待つ
    let start = Instant::now();
    loop {
        let state = cdp.evaluate(&session, "document.readyState")?;
        if state == "complete" || start.elapsed() > Duration::from_secs(30) {
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    thread::sleep(Duration::from_secs(3));
    let result = cdp.evaluate(&session, LIBRARY)?;
    let page_url = result["url"].as_str().unwrap_or_default().to_owned();
    if launched {
        let _ = cdp.call("Browser.close", json!({}), None);
    } else {
        let _ = cdp.call("Target.closeTarget", json!({"targetId": target}), None);
    }
    // ログインしていないと /landing やサインインのページへ移される
    if !page_url.contains("kindle-library") {
        return Err("ログインしていません。先に open でブラウザを開いてログインしてください".into());
    }
    let books: Vec<Value> = result["books"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let title = b["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(b["asin"].as_str().unwrap_or(""));
            json!({
                "id": b["asin"], "title": title, "file_name": title,
                "cover_url": b["cover_url"], "url": b["url"],
            })
        })
        .collect();
    eprintln!("{} 冊（{page_url}）", books.len());
    println!("{}", json!({"books": books}));
    Ok(())
}

fn main() -> ExitCode {
    let result = parse_args().and_then(|options| match options.command.as_str() {
        "open" => open(&options),
        "probe" => probe(&options),
        "eval" => eval(&options),
        "library" => library(&options),
        other => Err(format!("知らない命令です: {other}")),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
