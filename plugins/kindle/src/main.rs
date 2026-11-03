//! Kindle Cloud Reader を Chromium 系ブラウザで開き、MangaReeda のキー設定で操作できるようにする。
//! 本は表示するだけで、ページの画像を取り出したり保存したりはしない。
//! MangaReeda からは読むプラグイン（kind: "reader"）として、list と open で呼ばれる。
//!
//! 使い方:
//!   mangareeda-kindle open [ASIN] [--url URL] [--fullscreen]
//!                                      本（無ければライブラリ）を開く。標準入力に MangaReeda の
//!                                      キー設定などの JSON があれば使い、無ければ settings.json を読む
//!   mangareeda-kindle list             ライブラリの本を書き出す（library も同じ）
//!   mangareeda-kindle view ASIN        本体の窓の中で読むために、画面を標準出力に流す（view.rs）
//!   mangareeda-kindle probe            開いているページのボタンなどを JSON で書き出す（調査用）
//!   mangareeda-kindle gpu              動いているブラウザの GPU の状態を書き出す（調査用）
//!   mangareeda-kindle eval --expr JS [--url 部分]  開いているリーダー（または URL に部分を含むページ）で式を評価する（調査用）
//! 共通: [--browser PATH|flatpak:ID] [--settings settings.json] [--site co.jp|com]

mod browser;
mod cdp;
mod keys;
mod view;

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
/// ログインしていないときの知らせ。MangaReeda には終了コード 2（ログインが必要）で返す
const NOT_SIGNED_IN: &str =
    "Kindle にログインしていません。設定画面の「ブラウザを開く」から Amazon にログインしてください";
const EXIT_AUTH: u8 = 2;

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
    let command = args.next().ok_or("命令（open / list / probe / eval）を指定してください")?;
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
            // open の本（ASIN）。list のフォルダは無いので読み捨てる
            other if !other.starts_with("--") && matches!(options.command.as_str(), "open" | "view") => {
                options.asin = Some(other.to_owned());
            }
            other if !other.starts_with("--") && matches!(options.command.as_str(), "list" | "library") => {}
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

/// 使い終わったページを閉じる。ほかにページが無ければ（ほかの命令が使っていなければ）ブラウザも閉じる。
/// ブラウザは、一覧を取る list・窓の中で読む view・ログインの open が分け合うことがあるので、
/// 起動した者ではなく、最後に使っていた者が閉じる
fn release(cdp: &mut Cdp, target: &str) {
    let _ = cdp.call("Target.closeTarget", json!({"targetId": target}), None);
    // 閉じたページが一覧から消えるのを少し待つ
    thread::sleep(Duration::from_millis(200));
    let others = page_targets(cdp).map(|pages| pages.into_iter().any(|(t, _)| t != target));
    if matches!(others, Ok(false)) {
        let _ = cdp.call("Browser.close", json!({}), None);
    }
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

/// MangaReeda が標準入力に書いた1行（キー設定など）。端末から動かしたときは読まない
fn read_request() -> Option<Value> {
    use std::io::{BufRead, IsTerminal};
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return None;
    }
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok()?;
    serde_json::from_str(line.trim()).ok()
}

fn open(options: &Options) -> Result<(), String> {
    let request = read_request().unwrap_or(Value::Null);
    // キー設定は MangaReeda から受け取る。無ければ（手で動かしたとき）設定ファイルを読む
    let settings: Value = if request.get("keys").is_some() {
        request.clone()
    } else {
        let settings_file = settings_path(options.settings.clone());
        std::fs::read_to_string(&settings_file)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| {
                eprintln!("キー設定を読めません（{}）。キーは置き換えません", settings_file.display());
                json!({})
            })
    };
    // MangaReeda が単ページなら単ページで始める
    let single = settings.get("page_view_options").and_then(Value::as_str) == Some("Single");
    let actions_file = data_dir().join("actions.json");
    let operations = keys::load_operations(&actions_file)?;
    let matchers = keys::matchers(&settings, &operations);
    for m in &matchers {
        eprintln!("キー: {} ← {:?}{:?}", m.action, m.codes, m.keys);
    }
    let script = INJECT.replace("__MATCHERS__", &serde_json::to_string(&matchers).unwrap());

    let url = match (&options.url, &options.asin) {
        (Some(url), _) => url.clone(),
        (None, Some(asin)) => reader_url(&options.site, asin),
        (None, None) => library_url(&options.site),
    };
    let (mut cdp, launched) = connect(options, &url, options.headless)?;
    if !launched {
        // すでに開いている。キーは最初に開いた open が受け持っているので、その窓で開くよう頼んで終わる
        // （ここでもキーを横取りすると、1回押しただけで2回動いてしまう）
        return show_in_running(&mut cdp, &url);
    }
    Reader {
        cdp,
        operations,
        script,
        sessions: HashMap::new(),
        site: options.site.clone(),
        suppress: None,
        single,
    }
    .run()
}

/// 動いているブラウザの窓で `url` を開き、その窓を前に出す。窓が無ければ新しく開く
fn show_in_running(cdp: &mut Cdp, url: &str) -> Result<(), String> {
    let pages = page_targets(cdp)?;
    let Some((target, _)) = pages
        .iter()
        .find(|(_, u)| u.contains("read.amazon."))
        .or(pages.first())
    else {
        cdp.call("Target.createTarget", json!({"url": url, "newWindow": true}), None)?;
        return Ok(());
    };
    let session = attach(cdp, target)?;
    cdp.call("Page.navigate", json!({"url": url}), Some(&session))?;
    cdp.call("Target.activateTarget", json!({"targetId": target}), None)?;
    cdp.call("Target.detachFromTarget", json!({"sessionId": session}), None)?;
    Ok(())
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
/// 調査用: 動いているブラウザの GPU の状態（描き方ごとの使える・使えない、GPU の名前）を書き出す
fn gpu() -> Result<(), String> {
    let profile = data_dir().join("profile");
    let ws = browser::devtools_url(&profile).ok_or("先に open か view でブラウザを開いてください")?;
    let mut cdp = Cdp::connect(&ws)?;
    let info = cdp.call("SystemInfo.getInfo", json!({}), None)?;
    let gpu = &info["gpu"];
    let summary = json!({
        "featureStatus": gpu["featureStatus"],
        "devices": gpu["devices"].as_array().map(|devices| devices.iter().map(|d| json!({
            "vendor": d["vendorString"], "device": d["deviceString"], "driver": d["driverVersion"],
        })).collect::<Vec<_>>()),
        "gl_renderer": gpu["auxAttributes"]["glRenderer"],
        "gl_implementation": gpu["auxAttributes"]["glImplementationParts"],
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    Ok(())
}

fn eval(options: &Options) -> Result<(), String> {
    let expr = options.expr.as_deref().ok_or("--expr で式を指定してください")?;
    let profile = data_dir().join("profile");
    let ws = browser::devtools_url(&profile).ok_or("先に open でブラウザを開いてください")?;
    let mut cdp = Cdp::connect(&ws)?;
    // --url を付ければ、その文字を含むページ（ライブラリなど）で評価する
    for (target, url) in page_targets(&mut cdp)? {
        let wanted = match &options.url {
            Some(part) => url.contains(part.as_str()),
            None => is_reader_url(&url),
        };
        if !wanted {
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
/// 一度に取る冊数。ライブラリのページが下へ送られたときに取るのと同じ数にする（サイトに負担をかけないように）
const PAGE_SIZE: u32 = 50;

/// ライブラリの本を `PAGE_SIZE` 冊だけ取る。`token` は続きの印（無ければ最初から）。
/// ページ自身が続きを読み込むときと同じ呼び方をする
fn library_page_script(token: Option<&str>) -> String {
    let mut query = format!("libraryType=BOOKS&sortType=recency&querySize={PAGE_SIZE}");
    if let Some(token) = token {
        query.push_str("&paginationToken=");
        query.push_str(&token.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', ""));
    }
    format!(
        r#"(async () => {{
  // ログインしていなければ、ライブラリから別のページへ移されている
  if (!location.pathname.startsWith("/kindle-library")) return {{ status: 0, url: location.href }};
  let r;
  try {{
    r = await fetch("/kindle-library/search?{query}", {{ credentials: "include" }});
  }} catch (e) {{
    // サインインのページ（別のドメイン）へ移されると、取れずに失敗する
    return {{ status: 0, url: location.href, failed: String(e) }};
  }}
  if (!r.ok) return {{ status: r.status, url: location.href }};
  const j = await r.json();
  return {{ status: r.status, url: location.href, items: j.itemsList || [], next: j.paginationToken || null }};
}})()"#
    )
}

/// 本を開く URL を覚えておくファイル（ASIN → リーダーの URL）。文字の本とマンガで URL が違うため
fn reader_urls_path() -> PathBuf {
    data_dir().join("reader-urls.json")
}

fn remember_reader_urls(items: &[Value]) {
    let path = reader_urls_path();
    let mut urls: BTreeMap<String, String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    for item in items {
        if let (Some(asin), Some(url)) = (item["asin"].as_str(), item["webReaderUrl"].as_str()) {
            urls.insert(asin.to_owned(), url.to_owned());
        }
    }
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(text) = serde_json::to_string(&urls) {
        let _ = std::fs::write(&path, text);
    }
}

/// 本を開く URL。list で覚えたものが無ければ、マンガのリーダーの URL にする
fn reader_url(site: &str, asin: &str) -> String {
    std::fs::read_to_string(reader_urls_path())
        .ok()
        .and_then(|t| serde_json::from_str::<BTreeMap<String, String>>(&t).ok())
        .and_then(|urls| urls.get(asin).cloned())
        .filter(|url| url.starts_with(&format!("https://read.amazon.{site}/")))
        .unwrap_or_else(|| format!("https://read.amazon.{site}/manga/{asin}"))
}

/// 本の一覧を `PAGE_SIZE` 冊ずつ返す。続きがあれば "next" を付ける。
/// 本体は続きを頼むとき、標準入力の1行に "next" を入れてくる
fn library(options: &Options) -> Result<(), String> {
    let token = read_request()
        .and_then(|request| request.get("next").and_then(Value::as_str).map(str::to_owned));
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
    // 読み込みが落ち着くまで待つ（ログインしていなければ、この間に別のページへ移される）
    let start = Instant::now();
    loop {
        let state = cdp.evaluate(&session, "document.readyState")?;
        if state == "complete" || start.elapsed() > Duration::from_secs(30) {
            break;
        }
        thread::sleep(Duration::from_millis(300));
    }
    thread::sleep(Duration::from_secs(2));
    let result = cdp.evaluate(&session, &library_page_script(token.as_deref()));
    release(&mut cdp, &target);
    let result = result?;
    let page_url = result["url"].as_str().unwrap_or_default();
    let status = result["status"].as_u64().unwrap_or(0);
    // ログインしていないと /landing やサインインのページへ移され、一覧も取れない
    if !page_url.contains("kindle-library") || matches!(status, 0 | 401 | 403) {
        return Err(NOT_SIGNED_IN.into());
    }
    if status != 200 {
        return Err(format!("ライブラリの一覧を取れませんでした（{status}）"));
    }
    let items = result["items"].as_array().cloned().unwrap_or_default();
    remember_reader_urls(&items);
    let books: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            let asin = item["asin"].as_str()?;
            let title = item["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(asin);
            Some(json!({
                "id": asin, "title": title, "file_name": title,
                "cover_url": item["productUrl"],
            }))
        })
        .collect();
    let mut reply = json!({"books": books});
    // 続きの印。届いた冊数が1回分に足りなければ、もう続きは無い
    if let Some(next) = result["next"].as_str().filter(|n| !n.is_empty())
        && items.len() as u32 >= PAGE_SIZE
    {
        reply["next"] = json!(next);
    }
    eprintln!("{} 冊（続き {}）", books.len(), reply["next"]);
    println!("{reply}");
    Ok(())
}

fn main() -> ExitCode {
    let result = parse_args().and_then(|options| match options.command.as_str() {
        "open" => open(&options),
        "view" => view::view(&options),
        "probe" => probe(&options),
        "eval" => eval(&options),
        "gpu" => gpu(),
        "list" | "library" => library(&options),
        other => Err(format!("知らない命令です: {other}")),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            // 途中で失敗して、起動したブラウザを閉じないまま終わらないように
            browser::kill_launched();
            if e == NOT_SIGNED_IN {
                ExitCode::from(EXIT_AUTH)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
