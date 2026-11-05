//! 読むプラグインの本を、本体の窓の中に映す（プラグインの `view` 命令）。
//!
//! プラグインは画面を出さないブラウザで本を開き、描かれた画面を送ってくる。本体はそれを
//! 本の表示場所に映すだけで、保存も、文字の読み取りも、表紙やサムネイルへの使い回しもしない
//! （DRM のかかった本の中身を取り出さないため。テクスチャは1枚を上書きして使う）。
//! キー・マウス・ゲームパッドの操作は、本を読んでいるときと同じく本体の設定で動作に直し、
//! ページを動かすものはその名前をプラグインに送る。

use super::MangaReader;
use super::thumbnails::ShelfSource;
use crate::config::{MangaAction, MouseButton, MouseGesture, PageViewOptions};
use crate::engine::plugin::{ViewProcess, spawn_view};
use crate::localize::tr;
use eframe::egui;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::ChildStdout;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 本を閉じたときに、プラグインが終わるのを待つ時間。過ぎたら、起動したブラウザごと止める
const STOP_GRACE: Duration = Duration::from_secs(5);
/// MangaReeda を終えるときに待つ時間（終わる前に後始末を済ませる）
pub(super) const EXIT_GRACE: Duration = Duration::from_secs(2);

/// 届いた画面の置き場。最新の1枚だけを持ち、描いていないあいだに届いた古い画面は上書きして捨てる
type FrameSlot = Arc<Mutex<Option<(egui::ColorImage, [f32; 2])>>>;

/// リーダーのバナーを出すためにクリックする、上端と下端の帯の高さ（表示場所の高さに対する割合）
const CHROME_BAND: f32 = 0.1;
/// 左クリックのあと、この間に出てきたマウスの動作は、そのクリックから出たものとみなす
/// （ダブルクリックを待つあいだ、クリックの動作は遅れて出てくる）
const TAP_WINDOW: Duration = Duration::from_secs(1);

/// プラグインから届いた、画面のほかの知らせ（画面は `FrameSlot` に置く）
enum ViewEvent {
    /// リーダーのバナー（ツールバーと進み具合）が出た・消えた
    Chrome(bool),
    /// 今のページと総ページ数（1 から数える）
    Page([usize; 2]),
    /// 本を閉じた（リーダーでライブラリに戻った）
    Closed,
    Error(String),
}

pub(super) struct WebView {
    process: Option<ViewProcess>,
    events: Receiver<ViewEvent>,
    /// 届いた最新の画面（画面の大きさは、ブラウザの見かけの大きさ。論理ピクセル）
    frame: FrameSlot,
    texture: Option<egui::TextureHandle>,
    /// いま映している画面の、ブラウザでの大きさ（クリックの位置を直すのに使う）
    frame_size: [f32; 2],
    /// 閉じたときに戻る一覧
    shelf: ShelfSource,
    single: bool,
    /// リーダーのバナーが出ているか。出ているあいだは、クリックをそのままリーダーに送る
    chrome: bool,
    /// 最後にプラグインに伝えた、表示場所の大きさと倍率
    sent_size: Option<([u32; 2], f32)>,
    /// 最後にプラグインに伝えた、縦スクロールで読むか（MangaReeda の縦スクロール表示に合わせる）
    vertical: bool,
    /// 今のページと総ページ数（プラグインが知らせたもの。まだ届いていなければ None）
    pub(super) page: Option<[usize; 2]>,
    /// ツールバーのシークバーで選んでいるページ。ドラッグのあいだと、飛んだ先の位置が届くまで持つ
    pub(super) seek: Option<usize>,
    /// 最後に左クリックした時刻（クリックから出た動作を、その位置と一緒に送るため）
    last_left_click: Option<Instant>,
}

impl WebView {
    fn send(&mut self, value: Value) {
        if let Some(process) = self.process.as_mut() {
            let _ = writeln!(process.stdin, "{value}");
            let _ = process.stdin.flush();
        }
    }

    /// プラグインに終わるよう頼み、終わるのを `grace` まで待つ。過ぎたら、起動したブラウザごと止める
    pub(super) fn shut_down(mut self, grace: Duration) {
        self.send(json!({"quit": true}));
        if let Some(process) = self.process.take() {
            process.stop(grace);
        }
    }
}

impl Drop for WebView {
    /// プラグインに終わるよう頼み、終わらなければ止める（画面を止めないよう、待つのは別のスレッドで）
    fn drop(&mut self) {
        self.send(json!({"quit": true}));
        if let Some(process) = self.process.take() {
            std::thread::spawn(move || process.stop(STOP_GRACE));
        }
    }
}

/// プラグインの出力を読み、画面を画像に直して送る。画像はここでも保存しない
fn read_events(stdout: ChildStdout, ctx: egui::Context, frame: FrameSlot) -> Receiver<ViewEvent> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        use base64::Engine;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let event = if let Some(data) = value["frame"].as_str() {
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else {
                    continue;
                };
                let Ok(image) = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
                else {
                    continue;
                };
                let image = image.to_rgba8();
                let size = [image.width() as usize, image.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
                let width = value["width"].as_f64().unwrap_or(size[0] as f64) as f32;
                let height = value["height"].as_f64().unwrap_or(size[1] as f64) as f32;
                if let Ok(mut slot) = frame.lock() {
                    *slot = Some((color, [width, height]));
                }
                ctx.request_repaint();
                continue;
            } else if let Some(chrome) = value["chrome"].as_bool() {
                ViewEvent::Chrome(chrome)
            } else if let (Some(current), Some(total)) =
                (value["page"][0].as_u64(), value["page"][1].as_u64())
                && current >= 1
                && total >= current
            {
                ViewEvent::Page([current as usize, total as usize])
            } else if value["closed"].as_bool() == Some(true) {
                ViewEvent::Closed
            } else if let Some(error) = value["error"].as_str() {
                ViewEvent::Error(error.to_owned())
            } else {
                continue;
            };
            if tx.send(event).is_err() {
                return;
            }
            ctx.request_repaint();
        }
        // プラグインが終わった
        let _ = tx.send(ViewEvent::Closed);
        ctx.request_repaint();
    });
    rx
}

/// 動作のうち、プラグインに名前で送るもの（ページを動かす操作）
fn plugin_action_name(action: MangaAction) -> Option<&'static str> {
    Some(match action {
        MangaAction::NextPage | MangaAction::ScrollNextPage => "next_page",
        MangaAction::PrevPage | MangaAction::ScrollPrevPage => "prev_page",
        MangaAction::OneNextPage => "one_next_page",
        MangaAction::OnePrevPage => "one_prev_page",
        MangaAction::FirstPage => "first_page",
        MangaAction::LastPage => "last_page",
        // 縦スクロールのときのスクロール（プラグインは、見開きのときは何もしない）
        MangaAction::SlideImageDown | MangaAction::ScrollDown => "scroll_down",
        MangaAction::SlideImageUp | MangaAction::ScrollUp => "scroll_up",
        _ => return None,
    })
}

impl MangaReader {
    /// 読むプラグインの本を、この窓の中で開く
    pub(super) fn open_web_view(&mut self, plugin: &str, id: &str, ctx: &egui::Context) {
        let Some(manifest) = self.plugins.manifest(plugin).cloned() else {
            return;
        };
        let shelf = self
            .shelf_source()
            .unwrap_or_else(|| ShelfSource::Plugin(plugin.to_owned(), String::new()));
        let single = self.config.page_view_options == PageViewOptions::Single;
        let vertical = self.config.page_view_options == PageViewOptions::TopDown;
        let size = ctx.content_rect().size();
        let request = json!({
            "credentials": self.plugins.credentials_for(&manifest),
            "size": [size.x.round(), size.y.round()],
            "scale": ctx.pixels_per_point(),
            "single": single,
            "vertical": vertical,
        });
        match spawn_view(&manifest, &request, id) {
            Ok((process, stdout)) => {
                self.thumbnails = None;
                let frame = FrameSlot::default();
                self.web_view = Some(WebView {
                    process: Some(process),
                    events: read_events(stdout, ctx.clone(), frame.clone()),
                    frame,
                    texture: None,
                    frame_size: [0.0, 0.0],
                    shelf,
                    single,
                    chrome: false,
                    sent_size: None,
                    vertical,
                    page: None,
                    seek: None,
                    last_left_click: None,
                });
            }
            Err(error) => {
                self.show_fading_error(&format!("{}: {}", manifest.name, error.message()));
            }
        }
    }

    /// 窓の中で読んでいる本の、`page` ページ（1 から数える）に飛ぶ。
    /// 位置が届くまで、ツールバーは選んだページを出しておく
    pub(super) fn web_view_go_to_page(&mut self, page: usize) {
        if let Some(view) = self.web_view.as_mut() {
            if view.page.is_some_and(|[current, _]| current == page) {
                view.seek = None;
            } else {
                view.seek = Some(page);
                view.send(json!({"go_to_page": page}));
            }
        }
    }

    /// 窓の中で読むのをやめ、来た一覧に戻る
    pub(super) fn leave_web_view(&mut self) {
        if let Some(view) = self.web_view.take() {
            let shelf = view.shelf.clone();
            drop(view);
            self.toggle_book_list(shelf);
        }
    }

    /// 窓の中で読んでいる本を描き、動作を受け取る。受け取った動作は `action` から消す
    /// （全画面や終了など、本体が受け持つものは残す）
    pub(super) fn render_web_view(&mut self, ctx: &egui::Context, action: &mut MangaAction) {
        // 届いたもの。画面は最新のものだけを映す
        let mut leave = false;
        let mut error = None;
        if let Some(view) = self.web_view.as_mut() {
            let latest = view.frame.lock().ok().and_then(|mut slot| slot.take());
            loop {
                match view.events.try_recv() {
                    Ok(ViewEvent::Chrome(chrome)) => view.chrome = chrome,
                    Ok(ViewEvent::Page(page)) => {
                        view.page = Some(page);
                        view.seek = None;
                    }
                    Ok(ViewEvent::Closed) | Err(TryRecvError::Disconnected) => {
                        leave = true;
                        break;
                    }
                    Ok(ViewEvent::Error(message)) => error = Some(message),
                    Err(TryRecvError::Empty) => break,
                }
            }
            if let Some((image, size)) = latest {
                view.frame_size = size;
                match &mut view.texture {
                    Some(texture) => texture.set(image, egui::TextureOptions::LINEAR),
                    None => {
                        view.texture =
                            Some(ctx.load_texture("web-view", image, egui::TextureOptions::LINEAR));
                    }
                }
            }
        }
        if let Some(message) = error {
            self.show_fading_error(&message);
        }
        if leave {
            self.leave_web_view();
            return;
        }

        // 描く。マウスの操作は、本を読んでいるときと同じく、MangaReeda のマウスの設定で動作に直す
        let pixels_per_point = ctx.pixels_per_point();
        let mut forward_click = None;
        let mut toggle_chrome = false;
        // 位置つきで送るもの: 位置（画面の論理ピクセル）と、その位置が本の表示だったときに行うこと。
        // 本の表示でなければ（読み終わりのトレイや広告など）、プラグインはそこをクリックする
        let mut tap: Option<([f32; 2], Value)> = None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let response = ui.interact(
                    rect,
                    ui.id().with("web_view"),
                    egui::Sense::click_and_drag(),
                );
                let chrome = self.web_view.as_ref().is_some_and(|view| view.chrome);
                let left_click = response
                    .clicked_by(egui::PointerButton::Primary)
                    .then(|| response.interact_pointer_pos())
                    .flatten();
                // 上端か下端の帯のクリックは、リーダーのバナーを出す（Cloud Reader は真ん中のクリックで出す）
                let band = rect.height() * CHROME_BAND;
                let band_click = !chrome
                    && left_click.is_some_and(|pos| pos.y < rect.top() + band || pos.y > rect.bottom() - band);
                if left_click.is_some()
                    && let Some(view) = self.web_view.as_mut()
                {
                    view.last_left_click = Some(Instant::now());
                }
                // 左クリックから出た、ページを動かす動作と、その位置
                let mut tap_action = None;
                if band_click {
                    toggle_chrome = true;
                } else if !chrome
                    && self.binding_action.is_none()
                    && *action == MangaAction::None
                    && let Some(mouse_action) = self.collect_mouse_action(&response, ctx)
                {
                    *action = mouse_action;
                    let scrolled = ctx.input(|i| i.raw_scroll_delta != egui::Vec2::ZERO);
                    if let Some(name) = plugin_action_name(mouse_action)
                        && !scrolled
                        && let Some(view) = self.web_view.as_mut()
                        && view.last_left_click.take().is_some_and(|time| time.elapsed() < TAP_WINDOW)
                        && let Some(pos) = ctx.input(|i| i.pointer.hover_pos()).or(left_click)
                    {
                        tap_action = Some((pos, name));
                    }
                }
                // バナーが出ているあいだと、左クリックに動作を割り当てていないときは、クリックをリーダーに送る
                // （進み具合のバーや文字の設定などを使えるように）
                let click_to_reader = chrome
                    || self.get_mouse_action(MouseGesture::Click(MouseButton::Button1))
                        == MangaAction::None;
                // 縦スクロール表示を切り替えたら（メニューや設定画面から）、リーダーの設定も揃える
                let vertical = self.config.page_view_options == PageViewOptions::TopDown;
                let Some(view) = self.web_view.as_mut() else {
                    return;
                };
                if view.vertical != vertical {
                    view.vertical = vertical;
                    view.send(json!({"vertical": vertical}));
                }
                // 表示場所の大きさが変わったら、プラグインのブラウザの大きさも合わせる
                let size = [rect.width().round() as u32, rect.height().round() as u32];
                if view.sent_size != Some((size, pixels_per_point)) && size[0] > 0 && size[1] > 0 {
                    view.sent_size = Some((size, pixels_per_point));
                    view.send(json!({"size": size, "scale": pixels_per_point}));
                }
                let Some(texture) = &view.texture else {
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        tr("plugin.web_view_loading"),
                        egui::FontId::proportional(18.0),
                        egui::Color32::from_gray(200),
                    );
                    return;
                };
                // 縦横比を保って、表示場所の中央に置く（単ページの縦長の画面も中央に来る）
                let [width, height] = view.frame_size;
                let scale = (rect.width() / width.max(1.0)).min(rect.height() / height.max(1.0));
                let image_rect =
                    egui::Rect::from_center_size(rect.center(), egui::vec2(width, height) * scale);
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                let to_frame = |pos: egui::Pos2| {
                    [(pos.x - image_rect.left()) / scale, (pos.y - image_rect.top()) / scale]
                };
                if click_to_reader
                    && !band_click
                    && let Some(pos) = left_click
                    && image_rect.contains(pos)
                {
                    forward_click = Some(to_frame(pos));
                }
                // 帯のクリックと、左クリックから出たページの動作は、位置つきで送る
                // （下端に出る広告など、本の表示でないものはクリックできるように）
                if band_click
                    && let Some(pos) = left_click
                    && image_rect.contains(pos)
                {
                    toggle_chrome = false;
                    tap = Some((to_frame(pos), json!({"toggle_chrome": true})));
                }
                if let Some((pos, name)) = tap_action
                    && image_rect.contains(pos)
                {
                    *action = MangaAction::None;
                    tap = Some((to_frame(pos), json!({"action": name})));
                }
            });
        if let Some(view) = self.web_view.as_mut() {
            if let Some([x, y]) = forward_click {
                view.send(json!({"click": [x, y]}));
            }
            if toggle_chrome {
                view.send(json!({"toggle_chrome": true}));
            }
            if let Some(([x, y], mut command)) = tap {
                command["tap"] = json!([x, y]);
                view.send(command);
            }
        }

        // 動作（キー・マウス・ゲームパッド）
        *action = self.route_web_view_action(*action);
    }

    /// 窓の中で読んでいるときの動作を振り分ける。キー・マウス・ゲームパッドのほか、メニューからの
    /// 動作もここを通す（読んでいた別の本を動かさないように）。本体がそのまま行う動作（全画面・終了など）
    /// を返し、窓の中の表示が受け持ったものは None を返す
    pub(super) fn route_web_view_action(&mut self, action: MangaAction) -> MangaAction {
        if self.web_view.is_none() {
            return action;
        }
        match action {
            // 表紙サムネイルは、来た一覧（Kindle のライブラリなど）に戻る
            MangaAction::CloseBook | MangaAction::CoverThumbnails => {
                self.leave_web_view();
                MangaAction::None
            }
            MangaAction::ToggleAutoPageLayout => {
                if let Some(view) = self.web_view.as_mut() {
                    view.single = !view.single;
                    let single = view.single;
                    view.send(json!({"single": single}));
                    self.show_fading_error(tr(if single {
                        "settings.page_view.single"
                    } else {
                        "settings.page_view.double_rl"
                    }));
                }
                MangaAction::None
            }
            // 本体が受け持つもの
            MangaAction::FullScreen | MangaAction::QuitApp | MangaAction::None => action,
            // 別のものを開くときは、窓の中で読むのをやめてから
            MangaAction::OpenFile | MangaAction::ShowHistory | MangaAction::ShowFavorites => {
                self.web_view = None;
                action
            }
            other => {
                if let Some(name) = plugin_action_name(other)
                    && let Some(view) = self.web_view.as_mut()
                {
                    view.send(json!({"action": name}));
                }
                MangaAction::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_page_moves_are_sent_to_the_plugin() {
        assert_eq!(plugin_action_name(MangaAction::NextPage), Some("next_page"));
        assert_eq!(plugin_action_name(MangaAction::OnePrevPage), Some("one_prev_page"));
        assert_eq!(plugin_action_name(MangaAction::FullScreen), None);
        assert_eq!(plugin_action_name(MangaAction::ZoomIn), None);
    }
}
