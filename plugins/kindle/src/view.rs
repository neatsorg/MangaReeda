//! 本体の窓の中で読むための `view` 命令。
//!
//! 画面を出さない（headless の）ブラウザで本を開き、変わりそうなときに撮った画面を
//! 標準出力に1行ずつ流す。本体はそれを窓に映すだけで、保存も、文字の読み取りも、
//! 表紙などへの使い回しもしない。操作は本体から標準入力で受け取る。
//!
//! 標準入力の1行目: {"credentials": {…}, "size": [幅, 高さ], "scale": 倍率, "single": 真偽, "vertical": 真偽}
//!   （大きさは本体の論理ピクセル。倍率は1論理ピクセルあたりの画素の数）
//! 2行目から: {"action": "next_page"} / {"size": [幅, 高さ], "scale": 倍率} / {"single": 真偽}
//!   / {"vertical": 真偽}（縦スクロールで読む。リーダーの設定の「縦スクロール」をこれに揃える）
//!   / {"click": [x, y]}（画面の左上からの論理ピクセル）/ {"toggle_chrome": true}（リーダーのバナーを出し入れする）
//!   / {"go_to_page": ページ}（1 から数える）/ {"quit": true}
//!   / {"tap": [x, y], …}（ほかの命令に付く位置。そこが本の表示でなければ、命令の代わりにそこをクリックする）
//! 標準出力: {"frame": "<JPEG の base64>", "width": 幅, "height": 高さ}（幅と高さは論理ピクセル。前と同じ画面は送らない）
//!   / {"chrome": 真偽}（リーダーのバナーが出た・消えた。出ているあいだ、本体はクリックをそのまま送る）
//!   / {"page": [今のページ, 総ページ数]}（変わったとき。本体のツールバーに出す）
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

/// 画面を撮る時刻。Cloud Reader のページは変わらなくても描き直し続けるので、描かれた画面を
/// 流し続ける中継（screencast）だと、そのたびに JPEG に圧縮して CPU を 1 コア以上使う。
/// そこで、変わりそうなときだけ1枚ずつ撮る（`Page.captureScreenshot`）。
/// 操作のあとに撮る時刻（操作からの経過、ミリ秒）。ページめくりは 0.1 秒ほどで描かれ、画像の読み込みが遅れることもある
const AFTER_ACTION: [u64; 4] = [50, 150, 400, 1000];
/// 撮った画面が変わっていたら、まだ変わっている途中かもしれないので、このあとにもう1枚撮る
const FOLLOW_UP: Duration = Duration::from_millis(300);
/// ページを移った直後は、読み込みが落ち着くまで、この間隔で撮る
const LOADING_INTERVAL: Duration = Duration::from_millis(300);
const LOADING: Duration = Duration::from_secs(10);
/// 何もしていないあいだも、遅れて変わった画面を拾うために、この間隔で1枚撮る
const IDLE_INTERVAL: Duration = Duration::from_secs(4);

/// リーダーのバナーが出ているか、今のページ・総ページ数（keys::PAGE_FUNCTION）、縦スクロールか、
/// 本を読み込めなかったか（「Issue Loading Content」と TRY AGAIN の出る窓）。
/// バナーは、出ているあいだ kw-overlay-fade-enter-done が付く。見開きのときは下の進み具合のバー、
/// 縦スクロールのときは上のバー（#readerChromeTop）と右の縦のつまみ（#readerChromeRight）。
/// 縦スクロールかは、リーダーが覚えている localStorage の scrollDirectionPreference で見分ける。
/// あわせて、リーダーの設定の「縦スクロール」の行を隠す（MangaReeda の縦スクロール表示に揃えるため。
/// 縦スクロールではバナーがマウスを乗せたときだけ出るので、押すとクリックで戻せなくなっていた）
fn reader_state_script() -> String {
    format!(
        "(() => {{
  {page}
  if (!document.getElementById('mangareeda-style')) {{
    const style = document.createElement('style');
    style.id = 'mangareeda-style';
    style.textContent = '.kw-rd-chrome-settings-popup-option:has(> #kw-rd-scroll-direction) {{ display: none !important; }}';
    (document.head || document.documentElement).appendChild(style);
  }}
  const page = mangareedaPage();
  return {{
    chrome: !!document.querySelector(
      '.kw-wdg-chrome-bar.kw-overlay-fade-enter-done, #readerChromeTop.kw-overlay-fade-enter-done, #readerChromeRight.kw-overlay-fade-enter-done'),
    page: page && page.every((n) => Number.isInteger(n) && n > 0) ? page : null,
    vertical: localStorage.getItem('scrollDirectionPreference') === 'Vertical',
    error: !!document.querySelector('.kw-error-content-container'),
  }};
}})()",
        page = keys::PAGE_FUNCTION
    )
}

/// リーダーの設定の「縦スクロール」を `vertical` に揃えるスクリプト。設定のパネルを開いて
/// スイッチ（#kw-rd-scroll-direction）を押し、閉じる。リーダーがまだ読み込み中なら "not-ready"
fn set_vertical_script(vertical: bool) -> String {
    format!(
        "(async () => {{
  const want = {vertical};
  if ((localStorage.getItem('scrollDirectionPreference') === 'Vertical') === want) return 'same';
  const button = document.querySelector('.kw-rd-chrome-settings-btn');
  if (!button) return 'not-ready';
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  let toggle = document.querySelector('#kw-rd-scroll-direction');
  const opened = !toggle;
  if (opened) {{
    button.click();
    for (let i = 0; i < 20 && !(toggle = document.querySelector('#kw-rd-scroll-direction')); i++) await wait(50);
  }}
  if (toggle && toggle.checked !== want) toggle.click();
  if (opened) {{
    await wait(100);
    button.click();
  }}
  return toggle ? 'switched' : 'not-ready';
}})()"
    )
}

/// 画面の (x, y) が本の表示（#renderContainer の中）か。読み終わりのトレイ、広告、ポップアップなどが
/// 重なっていれば false。本の表示の入れ物が見つからなければ true（今までどおり動作を行う）
fn is_book_at(cdp: &mut Cdp, session: &str, x: f64, y: f64) -> bool {
    let script = format!(
        "(() => {{
  const root = document.querySelector('#renderContainer');
  const e = document.elementFromPoint({x}, {y});
  return !root || !e || root.contains(e);
}})()"
    );
    cdp.evaluate(session, &script).map_or(true, |value| value != Value::Bool(false))
}

/// リーダーの設定を揃え直すまでの間（読み込み中で揃えられなかったとき）
const VERTICAL_RETRY: Duration = Duration::from_secs(2);
/// 縦スクロールのとき、スクロールの動作1回で送るホイールの量（CSS ピクセル。マウスのホイール1目盛りくらい）
const WHEEL_STEP: f64 = 100.0;
/// 縦スクロールのとき、上のバナーを出すためにマウスを置く、上端からの位置
const TOP_HOVER_Y: f64 = 10.0;
/// 縦スクロールのとき、右の縦のつまみを出すためにマウスを置く、右端からの位置
const RIGHT_HOVER_X: f64 = 10.0;

/// ブラウザとの接続が切れたとき（スリープに入るときに切れる。ブラウザは動き続けている）に、
/// つなぎ直すまでの間。1回目は動いているブラウザにつなぎ直し、それでもだめなら起動し直す。
/// 使い切ったら諦めて、本体に知らせる。つなぎ直したあと `STEADY` だけ読めていれば数え直す
const RESTART_DELAYS: [u64; 6] = [1, 2, 4, 8, 15, 30];
const STEADY: Duration = Duration::from_secs(60);
/// 本を読み込めなかったとき（スリープに入るときや戻った直後の、ネットにつながっていないあいだなど）に、
/// 読み込み直すまでの間。続けて失敗したら倍にしていき、`RELOAD_MAX` で止める。読めたら戻す
const RELOAD_DELAY: Duration = Duration::from_secs(3);
const RELOAD_MAX: Duration = Duration::from_secs(60);

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

/// マウスを動かす（縦スクロールのバナーは、マウスを乗せたときに出る）
fn hover(cdp: &mut Cdp, session: &str, x: f64, y: f64) -> Result<(), String> {
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({"type": "mouseMoved", "x": x, "y": y}),
        Some(session),
    )?;
    Ok(())
}

/// 画面の真ん中でホイールを回す（縦スクロールのとき。下へは正）
fn wheel(cdp: &mut Cdp, session: &str, screen: &Screen, delta_y: f64) -> Result<(), String> {
    let (width, height) = screen.viewport();
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel", "x": f64::from(width) / 2.0, "y": f64::from(height) / 2.0,
            "deltaX": 0, "deltaY": delta_y,
        }),
        Some(session),
    )?;
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

/// 動作が既定の「最初・最後のページ」のままなら、飛ぶ先（`go_to_page` に渡す式）
fn default_jump(operations: &BTreeMap<String, Operation>, action: &str) -> Option<&'static str> {
    let page = match action {
        "first_page" => "1",
        "last_page" => "total",
        _ => return None,
    };
    match operations.get(action) {
        Some(Operation::Js(code)) if *code == keys::go_to_page_script(page) => Some(page),
        _ => None,
    }
}

/// `page`（JavaScript の式。総ページ数を `total` で使える）のページに飛ぶ。進み具合のつまみがまだ無い
/// （バナーを一度も出していない）ときは、出してから飛ぶ。見開きは真ん中のクリックで下のバナーを出し
/// （出たままになる）、縦スクロールは右端にマウスを置いて右のつまみを出す（飛んだら真ん中に戻して消す）
fn go_to_page(cdp: &mut Cdp, session: &str, screen: &Screen, vertical: bool, page: &str) -> Result<(), String> {
    let script = keys::go_to_page_script(page);
    if cdp.evaluate(session, &script)? == Value::Bool(true) {
        return Ok(());
    }
    let (width, height) = screen.viewport();
    let (x, y) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
    if vertical {
        hover(cdp, session, f64::from(width) - RIGHT_HOVER_X, y)?;
    } else {
        click(cdp, session, x, y)?;
    }
    thread::sleep(Duration::from_millis(400));
    cdp.evaluate(session, &script)?;
    if vertical {
        hover(cdp, session, x, y)?;
    }
    Ok(())
}

/// 失敗はどれも、本体に JSON で知らせる（本体は標準エラー出力を読まない）
pub fn view(options: &Options) -> Result<(), String> {
    let result = view_book(options);
    if let Err(e) = &result {
        emit(&json!({"error": e}));
    }
    result
}

fn view_book(options: &Options) -> Result<(), String> {
    let asin = options.asin.clone().ok_or("開く本（ASIN）を指定してください")?;
    let request = read_request().unwrap_or(Value::Null);
    let number = |value: &Value, default: f64| value.as_f64().filter(|v| *v > 0.0).unwrap_or(default);
    let mut screen = Screen {
        width: number(&request["size"][0], 1200.0),
        height: number(&request["size"][1], 900.0),
        scale: number(&request["scale"], 1.0),
        single: request["single"].as_bool().unwrap_or(false),
    };
    let mut state = Keep {
        vertical: request["vertical"].as_bool().unwrap_or(false),
        page: None,
        target: None,
    };
    let operations = keys::load_operations(&data_dir().join("actions.json"))?;
    let url = reader_url(&options.site, &asin);
    let commands = read_commands();

    // ブラウザとの接続が切れたら、つなぎ直して（ブラウザが終わっていれば起動し直して）同じページに戻る
    let mut attempt = 0;
    loop {
        let started = Instant::now();
        let reopening = attempt > 0;
        // 切れたわけと、つなぎ直せなかったのか（ブラウザが終わったか、答えなくなった）
        let (lost, unreachable) =
            match read_in_browser(options, &operations, &url, &mut screen, &mut state, &commands) {
                Ok(Ended::Done) => return Ok(()),
                Ok(Ended::Lost(e)) => (e, false),
                // つなぎ直しの途中の失敗（ブラウザがまだ起動できないなど）も、もう一度試す。ログインを求められたときは別
                Err(e) if reopening && e != NOT_SIGNED_IN => (e, true),
                Err(e) => return Err(e),
            };
        if started.elapsed() >= STEADY {
            attempt = 0;
        }
        let Some(delay) = RESTART_DELAYS.get(attempt) else {
            return Err(lost);
        };
        attempt += 1;
        eprintln!("{lost}。{delay} 秒後にブラウザにつなぎ直します（{attempt} 回目）");
        // つなぎ直せなかったら、起動したブラウザ（このプロセスが起動したなら）を止めて、次は起動し直す
        if unreachable {
            crate::browser::kill_launched();
            state.target = None;
        }
        if !wait_to_restart(Duration::from_secs(*delay), &commands, &mut screen, &mut state) {
            return Ok(());
        }
    }
}

/// 開き直すときに引き継ぐもの
struct Keep {
    /// 縦スクロールで読むか（本体から最後に届いたもの）
    vertical: bool,
    /// 最後に知らせたページ
    page: Option<u64>,
    /// 本を開いたページ（ブラウザのタブ）。接続だけが切れたときは、つなぎ直してそのまま使う
    target: Option<String>,
}

/// ブラウザで本を読み終えたわけ
enum Ended {
    /// 本体に頼まれた・本を閉じた
    Done,
    /// ブラウザとの接続が切れた（ブラウザが終わったか、スリープで切れた）
    Lost(String),
}

/// 起動し直すまで待つ。そのあいだに届いた命令のうち、画面の大きさなどは覚えておく。
/// 本体が終わるよう頼んだら false
fn wait_to_restart(
    delay: Duration,
    commands: &Receiver<Option<Value>>,
    screen: &mut Screen,
    state: &mut Keep,
) -> bool {
    let until = Instant::now() + delay;
    while Instant::now() < until {
        loop {
            let command = match commands.try_recv() {
                Ok(Some(command)) => command,
                Ok(None) | Err(TryRecvError::Disconnected) => return false,
                Err(TryRecvError::Empty) => break,
            };
            if command["quit"].as_bool() == Some(true) {
                return false;
            }
            if let (Some(width), Some(height)) = (command["size"][0].as_f64(), command["size"][1].as_f64()) {
                screen.width = width.max(1.0);
                screen.height = height.max(1.0);
                screen.scale = command["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(screen.scale);
            }
            if let Some(single) = command["single"].as_bool() {
                screen.single = single;
            }
            if let Some(vertical) = command["vertical"].as_bool() {
                state.vertical = vertical;
            }
            // ページを動かす操作などは、開き直すまで届かないので捨てる
        }
        thread::sleep(Duration::from_millis(100));
    }
    true
}

/// ブラウザにつないで（無ければ起動して）本を開き、読み終えるまで画面を流す
fn read_in_browser(
    options: &Options,
    operations: &BTreeMap<String, Operation>,
    url: &str,
    screen: &mut Screen,
    state: &mut Keep,
    commands: &Receiver<Option<Value>>,
) -> Result<Ended, String> {
    let (mut cdp, launched) = connect(options, "about:blank", true)?;
    // 接続だけが切れて、ブラウザに本を開いたページが残っていれば、読み込み直さずにそのまま使う
    let kept = match &state.target {
        Some(kept) if !launched => page_targets(&mut cdp)?
            .into_iter()
            .find(|(t, _)| t == kept)
            .map(|(t, _)| t),
        _ => None,
    };
    let reuse = kept.is_some();
    let target = if let Some(kept) = kept {
        kept
    } else if launched {
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
    state.target = Some(target.clone());
    let result = prepare(&mut cdp, &session, screen, url, !reuse)
        .and_then(|()| run(&mut cdp, &session, operations, url, screen, state, commands));
    if cdp.is_lost() {
        return Ok(Ended::Lost(result.err().unwrap_or_default()));
    }
    crate::release(&mut cdp, &target);
    result.map(|()| Ended::Done)
}

/// ページの名乗りと大きさを整え、本を開く。`navigate` が false なら、もう開いているページをそのまま使う
fn prepare(cdp: &mut Cdp, session: &str, screen: &Screen, url: &str, navigate: bool) -> Result<(), String> {
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
    if navigate {
        cdp.call("Page.navigate", json!({"url": url}), Some(session))?;
    }
    Ok(())
}

fn run(
    cdp: &mut Cdp,
    session: &str,
    operations: &BTreeMap<String, Operation>,
    url: &str,
    screen: &mut Screen,
    state: &mut Keep,
    commands: &Receiver<Option<Value>>,
) -> Result<(), String> {
    let mut captures = Captures::default();
    // この時刻までは、ページを移ったあとの読み込み中として、間を詰めて撮る
    let mut loading_until = Instant::now() + LOADING;
    let mut last_capture = Instant::now();
    // 最後に送った画面（同じ画面は送らない）
    let mut last_sent = String::new();
    // 最後に知らせた、バナーが出ているかと、ページの位置
    let mut last_chrome = false;
    let mut last_page = Value::Null;
    let reader_state = reader_state_script();
    // リーダーがいま縦スクロールか（最後に読んだもの）と、最後に揃えようとした時刻
    let mut vertical = false;
    let mut vertical_tried: Option<Instant> = None;
    // 開き直したときは、読んでいたページに戻す（リーダーが読み込めて、ページが分かってから）
    let mut return_to = state.page;
    // 読み込めなかったとき、読み込み直す時刻と、次に失敗したときの間
    let mut reload_at: Option<Instant> = None;
    let mut reload_delay = RELOAD_DELAY;

    loop {
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
            // 位置つきの命令は、そこが本の表示でなければ、命令の代わりにそこをクリックする
            // （読み終わりのトレイは画面全体を覆いで隠し、覆いのクリックで閉じる。広告の閉じるボタンなども）
            if let (Some(x), Some(y)) = (command["tap"][0].as_f64(), command["tap"][1].as_f64())
                && !is_book_at(cdp, session, x, y)
            {
                click(cdp, session, x, y)?;
                captures.after_action(Instant::now());
                continue;
            }
            if let Some(action) = command["action"].as_str() {
                // スクロールは縦スクロールのときだけ、ホイールにして送る（見開きでは何もしない）
                let result = match action {
                    "scroll_down" if vertical => wheel(cdp, session, screen, WHEEL_STEP),
                    "scroll_up" if vertical => wheel(cdp, session, screen, -WHEEL_STEP),
                    "scroll_down" | "scroll_up" => Ok(()),
                    // 既定の「最初・最後のページ」は、ページへの飛び方と同じ道で（つまみがまだ無ければ、
                    // バナーやつまみを出してから飛ぶ）。actions.json で変えていれば、その操作に従う
                    _ => match default_jump(operations, action) {
                        Some(page) => go_to_page(cdp, session, screen, vertical, page),
                        None => perform(cdp, session, operations, action),
                    },
                };
                if let Err(e) = result {
                    eprintln!("{action} に失敗しました: {e}");
                }
            }
            if let Some(next) = command["vertical"].as_bool()
                && next != state.vertical
            {
                state.vertical = next;
                vertical_tried = None;
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
            if let Some(page) = command["go_to_page"].as_u64()
                && let Err(e) = go_to_page(cdp, session, screen, vertical, &page.to_string())
            {
                eprintln!("{page} ページへの移動に失敗しました: {e}");
            }
            // リーダーのバナー（ツールバーと進み具合）を出し入れする。Cloud Reader は真ん中のクリックで出し入れする
            // 縦スクロールのときは、マウスを乗せたときに出るので、上端にマウスを置く（真ん中に戻せば消える）
            if command["toggle_chrome"].as_bool() == Some(true) {
                let (width, height) = screen.viewport();
                let (x, y) = (f64::from(width) / 2.0, f64::from(height) / 2.0);
                if !vertical {
                    click(cdp, session, x, y)?;
                } else if last_chrome {
                    hover(cdp, session, x, y)?;
                } else {
                    hover(cdp, session, x, TOP_HOVER_Y)?;
                }
            }
            // 操作のあと、変わっていく画面を何回かに分けて撮る
            captures.after_action(Instant::now());
        }

        if reload_at.is_some_and(|at| Instant::now() >= at) {
            reload_at = None;
            loading_until = Instant::now() + LOADING;
            // 読み込み直したら、読んでいたページに戻す
            if return_to.is_none() {
                return_to = state.page;
            }
            eprintln!("本を読み込めなかったので、読み込み直します");
            if let Err(e) = cdp.call("Page.navigate", json!({"url": url}), Some(session)) {
                eprintln!("本を読み込み直せませんでした: {e}");
                reload_at = Some(Instant::now() + reload_delay);
            }
        }

        // 撮る時刻が来たら撮る。前と違う画面だけを本体に送る
        let now = Instant::now();
        let since = now.duration_since(last_capture);
        let due = captures.take_due(now)
            || since >= IDLE_INTERVAL
            || (now < loading_until && since >= LOADING_INTERVAL);
        if due {
            last_capture = now;
            // 読み込みの途中などで撮れないときは、次の機会に撮る
            if let Ok(shot) = cdp.call(
                "Page.captureScreenshot",
                json!({"format": "jpeg", "quality": 85, "optimizeForSpeed": true}),
                Some(session),
            ) && let Some(data) = shot["data"].as_str()
                && data != last_sent
            {
                let (width, height) = screen.viewport();
                emit(&json!({"frame": data, "width": width, "height": height}));
                last_sent = data.to_owned();
                // まだ変わっている途中かもしれないので、もう1枚撮る
                captures.at(Instant::now() + FOLLOW_UP);
            }
            // バナーが出たか消えたか（本体は、出ているあいだクリックをリーダーに送る）と、
            // ページの位置（本体のツールバーに出す）が変わったら知らせる
            let reader = cdp.evaluate(session, &reader_state);
            // 本を読み込めなかった。ページの位置などは読まずに、少し待って読み込み直す
            if let Ok(reader) = &reader
                && reader["error"].as_bool() == Some(true)
            {
                if reload_at.is_none() {
                    reload_at = Some(Instant::now() + reload_delay);
                    reload_delay = (reload_delay * 2).min(RELOAD_MAX);
                }
            } else if let Ok(reader) = reader {
                // 読めた。読み込み直す予定があれば取りやめる
                if reader["page"].is_array() {
                    reload_delay = RELOAD_DELAY;
                    reload_at = None;
                }
                // リーダーの「縦スクロール」を、MangaReeda の縦スクロール表示に揃える
                if let Some(now_vertical) = reader["vertical"].as_bool() {
                    vertical = now_vertical;
                }
                if vertical != state.vertical
                    && vertical_tried.is_none_or(|time| time.elapsed() >= VERTICAL_RETRY)
                {
                    vertical_tried = Some(Instant::now());
                    match cdp.evaluate(session, &set_vertical_script(state.vertical)) {
                        Ok(result) if result == "switched" => {
                            vertical = state.vertical;
                            captures.after_action(Instant::now());
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("縦スクロールの切り替えに失敗しました: {e}"),
                    }
                }
                if let Some(chrome) = reader["chrome"].as_bool()
                    && chrome != last_chrome
                {
                    last_chrome = chrome;
                    emit(&json!({"chrome": chrome}));
                }
                let page = reader["page"].clone();
                if let Some(current) = page[0].as_u64()
                    && let Some(target) = return_to.take()
                    && current != target
                {
                    match go_to_page(cdp, session, screen, vertical, &target.to_string()) {
                        Ok(()) => captures.after_action(Instant::now()),
                        // リーダーがまだ読み込み中なのかもしれない。次に様子を見るときにまた試す
                        Err(e) => {
                            eprintln!("{target} ページに戻れませんでした: {e}");
                            return_to = Some(target);
                        }
                    }
                    continue;
                }
                if page.is_array() && page != last_page {
                    emit(&json!({"page": page}));
                    state.page = page[0].as_u64();
                    last_page = page;
                }
            }
        }

        // ブラウザからの知らせ
        let Some(event) = cdp.poll_event(Duration::from_millis(10))? else {
            continue;
        };
        if event.method == "Page.frameNavigated" && event.params["frame"]["parentId"].is_null() {
            loading_until = Instant::now() + LOADING;
            let url = event.params["frame"]["url"].as_str().unwrap_or_default();
            if is_sign_in_page(url) {
                return Err(NOT_SIGNED_IN.into());
            }
            // 読み込めなかった（ネットにつながっていないなど）。少し待って読み込み直す
            if url.starts_with("chrome-error://") && reload_at.is_none() {
                reload_at = Some(Instant::now() + reload_delay);
                reload_delay = (reload_delay * 2).min(RELOAD_MAX);
            }
            // リーダーの「ライブラリ」ボタンなどで本を閉じた
            if is_library_page(url) {
                emit(&json!({"closed": true}));
                return Ok(());
            }
        }
    }
}

/// これから画面を撮る時刻
#[derive(Default)]
struct Captures {
    times: Vec<Instant>,
}

impl Captures {
    fn at(&mut self, time: Instant) {
        self.times.push(time);
    }

    fn after_action(&mut self, now: Instant) {
        self.times
            .extend(AFTER_ACTION.iter().map(|ms| now + Duration::from_millis(*ms)));
    }

    /// 時刻が来たものがあれば、それらを除いて真を返す
    fn take_due(&mut self, now: Instant) -> bool {
        let before = self.times.len();
        self.times.retain(|time| *time > now);
        self.times.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 既定の「最初・最後のページ」だけを飛び方の道に回し、actions.json で変えたものや外したものは回さない
    #[test]
    fn only_default_first_and_last_pages_jump() {
        let mut operations = keys::default_operations();
        assert_eq!(default_jump(&operations, "first_page"), Some("1"));
        assert_eq!(default_jump(&operations, "last_page"), Some("total"));
        assert_eq!(default_jump(&operations, "next_page"), None);
        operations.insert("first_page".into(), Operation::Key("Home".into()));
        operations.remove("last_page");
        assert_eq!(default_jump(&operations, "first_page"), None);
        assert_eq!(default_jump(&operations, "last_page"), None);
    }

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
