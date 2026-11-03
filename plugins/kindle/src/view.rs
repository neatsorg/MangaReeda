//! 本体の窓の中で読むための `view` 命令。
//!
//! 画面を出さない（headless の）ブラウザで本を開き、描かれた画面（screencast）を
//! 標準出力に1行ずつ流す。本体はそれを窓に映すだけで、保存も、文字の読み取りも、
//! 表紙などへの使い回しもしない。操作は本体から標準入力で受け取る。
//!
//! 標準入力の1行目: {"credentials": {…}, "size": [幅, 高さ], "scale": 倍率, "single": 真偽}
//!   （大きさは本体の論理ピクセル。倍率は1論理ピクセルあたりの画素の数）
//! 2行目から: {"action": "next_page"} / {"size": [幅, 高さ], "scale": 倍率} / {"single": 真偽}
//!   / {"click": [x, y]}（画面の左上からの論理ピクセル）/ {"quit": true}
//! 標準出力: {"frame": "<JPEG の base64>", "width": 幅, "height": 高さ}（幅と高さは論理ピクセル）
//!   / {"closed": true}（ライブラリに移ったとき。本体は表紙サムネイルに戻る）
//!   / {"error": "…"}

use crate::cdp::Cdp;
use crate::keys::{self, Operation};
use crate::{NOT_SIGNED_IN, Options, attach, connect, data_dir, page_targets, read_request, reader_url};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;
use std::time::{Duration, Instant};

/// 本体へ画面を送る間隔の下限（毎秒 30 枚まで）。Cloud Reader は変わらなくても描き直し続けるので抑える
const FRAME_INTERVAL: Duration = Duration::from_millis(33);

/// 画面の中継を続ける時間。Cloud Reader のページは、変わらなくても描き直し続け、中継していると
/// そのたびに JPEG に圧縮するので CPU を 1 コア以上使う。そこで、操作や新しい画面があってから
/// この時間だけ中継し、あとは止める
const WAKE: Duration = Duration::from_millis(1500);
/// 止めているあいだに、遅れて変わった画面（本の画像の読み込みなど）を拾うために、ときどき少しだけ中継する
const PEEK_INTERVAL: Duration = Duration::from_secs(4);
const PEEK: Duration = Duration::from_millis(300);
/// 開いた直後は、読み込みが落ち着くまで長めに中継する
const FIRST_WAKE: Duration = Duration::from_secs(10);

/// 単ページのときの幅と高さの比（Cloud Reader は縦長の画面だと1ページずつ表示する）
const SINGLE_RATIO: f64 = 0.7;

/// 見せかける画面
#[derive(Clone, Copy, PartialEq)]
struct Screen {
    width: f64,
    height: f64,
    scale: f64,
    single: bool,
}

impl Screen {
    /// ブラウザに見せかける幅と高さ。単ページなら縦長にする（本体が中央に置く）
    fn viewport(&self) -> (u32, u32) {
        let width = if self.single {
            self.width.min(self.height * SINGLE_RATIO)
        } else {
            self.width
        };
        (width.round().max(1.0) as u32, self.height.round().max(1.0) as u32)
    }
}

fn emit(line: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// 標準入力の2行目からを、別のスレッドで読む。本体が終われば閉じる
fn read_commands() -> Receiver<Option<Value>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Ok(value) = serde_json::from_str::<Value>(line.trim())
                && tx.send(Some(value)).is_err()
            {
                return;
            }
        }
        let _ = tx.send(None);
    });
    rx
}

fn apply_screen(cdp: &mut Cdp, session: &str, screen: &Screen) -> Result<(), String> {
    let (width, height) = screen.viewport();
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width": width, "height": height,
            "deviceScaleFactor": screen.scale, "mobile": false,
        }),
        Some(session),
    )?;
    Ok(())
}

/// ライブラリのページ（本を閉じたとき）か。/kindle-library/manga-wr/… はリーダーへの入口なので違う
fn is_library_page(url: &str) -> bool {
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| path)
        .unwrap_or_default();
    let path = path.split(['?', '#']).next().unwrap_or_default();
    path.trim_end_matches('/') == "kindle-library"
}

/// ログインを求められたページか
fn is_sign_in_page(url: &str) -> bool {
    url.contains("/ap/signin") || url.contains("/landing")
}

/// 動作の名前に当たる操作を行う。全画面・本を閉じる・単ページは本体が受け持つので、ここには来ない
fn perform(
    cdp: &mut Cdp,
    session: &str,
    operations: &BTreeMap<String, Operation>,
    action: &str,
) -> Result<(), String> {
    match operations.get(action) {
        Some(Operation::Key(key)) => {
            let (code, vk, text) = keys::key_event_fields(key);
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
                cdp.call("Input.dispatchKeyEvent", params, Some(session))?;
            }
        }
        Some(Operation::Click(selector)) => {
            cdp.evaluate(
                session,
                &format!("document.querySelector({})?.click()", json!(selector)),
            )?;
        }
        Some(Operation::Js(code)) => {
            cdp.evaluate(session, code)?;
        }
        Some(Operation::Builtin(_)) | None => {}
    }
    Ok(())
}

fn click(cdp: &mut Cdp, session: &str, x: f64, y: f64) -> Result<(), String> {
    for kind in ["mousePressed", "mouseReleased"] {
        cdp.call(
            "Input.dispatchMouseEvent",
            json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            Some(session),
        )?;
    }
    Ok(())
}

pub fn view(options: &Options) -> Result<(), String> {
    let asin = options.asin.clone().ok_or("開く本（ASIN）を指定してください")?;
    let request = read_request().unwrap_or(Value::Null);
    let number = |value: &Value, default: f64| value.as_f64().filter(|v| *v > 0.0).unwrap_or(default);
    let mut screen = Screen {
        width: number(&request["size"][0], 1200.0),
        height: number(&request["size"][1], 900.0),
        scale: number(&request["scale"], 1.0),
        single: request["single"].as_bool().unwrap_or(false),
    };
    let operations = keys::load_operations(&data_dir().join("actions.json"))?;
    let url = reader_url(&options.site, &asin);
    let commands = read_commands();

    let (mut cdp, launched) = connect(options, "about:blank", true)?;
    let target = if launched {
        page_targets(&mut cdp)?
            .into_iter()
            .next()
            .map(|(t, _)| t)
            .ok_or("ページがありません")?
    } else {
        // ログインのために開いた窓などで、ブラウザがもう動いている。そこに見えないページを足す
        cdp.call("Target.createTarget", json!({"url": "about:blank", "background": true}), None)?
            ["targetId"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };
    let session = attach(&mut cdp, &target)?;
    let result = run(&mut cdp, &session, &operations, &url, &mut screen, &commands);
    if launched {
        let _ = cdp.call("Browser.close", json!({}), None);
    } else {
        let _ = cdp.call("Target.closeTarget", json!({"targetId": target}), None);
    }
    if let Err(e) = &result {
        emit(&json!({"error": e}));
    }
    result
}

fn run(
    cdp: &mut Cdp,
    session: &str,
    operations: &BTreeMap<String, Operation>,
    url: &str,
    screen: &mut Screen,
    commands: &Receiver<Option<Value>>,
) -> Result<(), String> {
    // headless のブラウザは名乗りに "HeadlessChrome" が入り、Cloud Reader に断られる。
    // ブラウザ自身が答える名乗りから、その文字だけを直す（版はそのまま）
    let version = cdp.call("Browser.getVersion", json!({}), None)?;
    let user_agent = version["userAgent"]
        .as_str()
        .unwrap_or_default()
        .replace("HeadlessChrome", "Chrome");
    cdp.call(
        "Emulation.setUserAgentOverride",
        json!({"userAgent": user_agent}),
        Some(session),
    )?;
    cdp.call("Page.enable", json!({}), Some(session))?;
    apply_screen(cdp, session, screen)?;
    cdp.call("Page.navigate", json!({"url": url}), Some(session))?;
    // 画面の中継は、読み込みの途中だと断られるので、始められるまで間を空けてやり直す
    let started = Instant::now();
    let mut casting = false;
    let mut last_try: Option<Instant> = None;
    // この時刻まで中継する。操作や新しい画面のたびに延ばす
    let mut wake_until = Instant::now() + FIRST_WAKE;
    let mut last_peek = Instant::now();
    // まだ送っていない最新の画面と、最後に送った画面
    let mut pending: Option<Value> = None;
    let mut last_sent = String::new();
    let mut last_emit: Option<Instant> = None;

    loop {
        if pending.is_some() && last_emit.is_none_or(|at| at.elapsed() >= FRAME_INTERVAL) {
            let frame = pending.take().unwrap_or_default();
            last_sent = frame["frame"].as_str().unwrap_or_default().to_owned();
            last_emit = Some(Instant::now());
            emit(&frame);
        }
        let now = Instant::now();
        if now >= wake_until && now.duration_since(last_peek) >= PEEK_INTERVAL {
            last_peek = now;
            wake_until = now + PEEK;
        }
        let wanted = now < wake_until;
        if casting && !wanted {
            cdp.call("Page.stopScreencast", json!({}), Some(session))?;
            casting = false;
        }
        if wanted && !casting && last_try.is_none_or(|at| at.elapsed() > Duration::from_millis(500)) {
            last_try = Some(Instant::now());
            match cdp.call(
                "Page.startScreencast",
                json!({"format": "jpeg", "quality": 85}),
                Some(session),
            ) {
                Ok(_) => casting = true,
                Err(e) if started.elapsed() > Duration::from_secs(60) => return Err(e),
                Err(_) => {}
            }
        }
        // 本体からの命令
        loop {
            let command = match commands.try_recv() {
                Ok(Some(command)) => command,
                // 本体が終わった
                Ok(None) | Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => break,
            };
            if command["quit"].as_bool() == Some(true) {
                return Ok(());
            }
            // 操作があったら、変わった画面を受け取るために中継する
            wake_until = wake_until.max(Instant::now() + WAKE);
            if let Some(action) = command["action"].as_str()
                && let Err(e) = perform(cdp, session, operations, action)
            {
                eprintln!("{action} に失敗しました: {e}");
            }
            let mut next = *screen;
            if let (Some(width), Some(height)) = (command["size"][0].as_f64(), command["size"][1].as_f64()) {
                next.width = width.max(1.0);
                next.height = height.max(1.0);
                next.scale = command["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(next.scale);
            }
            if let Some(single) = command["single"].as_bool() {
                next.single = single;
            }
            if next != *screen {
                *screen = next;
                apply_screen(cdp, session, screen)?;
            }
            if let (Some(x), Some(y)) = (command["click"][0].as_f64(), command["click"][1].as_f64()) {
                click(cdp, session, x, y)?;
            }
        }

        // ブラウザからの知らせ
        let Some(event) = cdp.poll_event(Duration::from_millis(15))? else {
            continue;
        };
        match event.method.as_str() {
            "Page.screencastFrame" => {
                let params = &event.params;
                // 受け取ったと返さないと、次の画面が来ない
                cdp.call(
                    "Page.screencastFrameAck",
                    json!({"sessionId": params["sessionId"]}),
                    Some(session),
                )?;
                // 前に送ったものと同じ画面は送らない。間引くときは最新のものだけを残す
                if params["data"].as_str() != Some(last_sent.as_str()) {
                    // 画面が変わっているあいだは中継を続ける
                    wake_until = wake_until.max(Instant::now() + WAKE);
                    pending = Some(json!({
                        "frame": params["data"],
                        "width": params["metadata"]["deviceWidth"],
                        "height": params["metadata"]["deviceHeight"],
                    }));
                } else {
                    pending = None;
                }
            }
            "Page.frameNavigated" if event.params["frame"]["parentId"].is_null() => {
                wake_until = wake_until.max(Instant::now() + FIRST_WAKE);
                let url = event.params["frame"]["url"].as_str().unwrap_or_default();
                if is_sign_in_page(url) {
                    return Err(NOT_SIGNED_IN.into());
                }
                // リーダーの「ライブラリ」ボタンなどで本を閉じた
                if is_library_page(url) {
                    emit(&json!({"closed": true}));
                    return Ok(());
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_pages_are_told_apart_from_reader_entrances() {
        assert!(is_library_page("https://read.amazon.co.jp/kindle-library"));
        assert!(is_library_page("https://read.amazon.co.jp/kindle-library?ref_=x"));
        assert!(!is_library_page("https://read.amazon.co.jp/kindle-library/manga-wr/B0TEST"));
        assert!(!is_library_page("https://read.amazon.co.jp/manga/B0TEST"));
    }

    #[test]
    fn single_page_makes_the_screen_tall() {
        let mut screen = Screen { width: 1600.0, height: 900.0, scale: 2.0, single: false };
        assert_eq!(screen.viewport(), (1600, 900));
        screen.single = true;
        assert_eq!(screen.viewport(), (630, 900));
        // もともと縦長なら、そのまま
        screen.width = 500.0;
        assert_eq!(screen.viewport(), (500, 900));
    }
}
