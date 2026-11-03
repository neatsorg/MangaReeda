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
use crate::engine::plugin::spawn_view;
use crate::localize::tr;
use eframe::egui;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// リーダーのバナーを出すためにクリックする、上端と下端の帯の高さ（表示場所の高さに対する割合）
const CHROME_BAND: f32 = 0.1;

/// プラグインから届いたもの
enum ViewEvent {
    /// 画面と、その大きさ（ブラウザの見かけの大きさ。論理ピクセル）
    Frame(egui::ColorImage, [f32; 2]),
    /// リーダーのバナー（ツールバーと進み具合）が出た・消えた
    Chrome(bool),
    /// 本を閉じた（リーダーでライブラリに戻った）
    Closed,
    Error(String),
}

pub(super) struct WebView {
    child: Option<Child>,
    stdin: ChildStdin,
    events: Receiver<ViewEvent>,
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
}

impl WebView {
    fn send(&mut self, value: Value) {
        let _ = writeln!(self.stdin, "{value}");
        let _ = self.stdin.flush();
    }
}

impl Drop for WebView {
    /// プラグインに終わるよう頼み、終わらなければ止める（画面を止めないよう、待つのは別のスレッドで）
    fn drop(&mut self) {
        self.send(json!({"quit": true}));
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(5) {
                    if child.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
        }
    }
}

/// プラグインの出力を読み、画面を画像に直して送る。画像はここでも保存しない
fn read_events(stdout: ChildStdout, ctx: egui::Context) -> Receiver<ViewEvent> {
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
                ViewEvent::Frame(color, [width, height])
            } else if let Some(chrome) = value["chrome"].as_bool() {
                ViewEvent::Chrome(chrome)
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
        let size = ctx.content_rect().size();
        let request = json!({
            "credentials": self.plugins.credentials_for(&manifest),
            "size": [size.x.round(), size.y.round()],
            "scale": ctx.pixels_per_point(),
            "single": single,
        });
        match spawn_view(&manifest, &request, id) {
            Ok((child, stdin, stdout)) => {
                self.thumbnails = None;
                self.web_view = Some(WebView {
                    child: Some(child),
                    stdin,
                    events: read_events(stdout, ctx.clone()),
                    texture: None,
                    frame_size: [0.0, 0.0],
                    shelf,
                    single,
                    chrome: false,
                    sent_size: None,
                });
            }
            Err(error) => {
                self.show_fading_error(&format!("{}: {}", manifest.name, error.message()));
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
        let mut latest = None;
        let mut leave = false;
        let mut error = None;
        if let Some(view) = self.web_view.as_mut() {
            loop {
                match view.events.try_recv() {
                    Ok(ViewEvent::Frame(image, size)) => latest = Some((image, size)),
                    Ok(ViewEvent::Chrome(chrome)) => view.chrome = chrome,
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
                if band_click {
                    toggle_chrome = true;
                } else if !chrome
                    && self.binding_action.is_none()
                    && *action == MangaAction::None
                    && let Some(mouse_action) = self.collect_mouse_action(&response, ctx)
                {
                    *action = mouse_action;
                }
                // バナーが出ているあいだと、左クリックに動作を割り当てていないときは、クリックをリーダーに送る
                // （進み具合のバーや文字の設定などを使えるように）
                let click_to_reader = chrome
                    || self.get_mouse_action(MouseGesture::Click(MouseButton::Button1))
                        == MangaAction::None;
                let Some(view) = self.web_view.as_mut() else {
                    return;
                };
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
                if click_to_reader
                    && !band_click
                    && let Some(pos) = left_click
                    && image_rect.contains(pos)
                {
                    forward_click = Some([
                        (pos.x - image_rect.left()) / scale,
                        (pos.y - image_rect.top()) / scale,
                    ]);
                }
            });
        if let Some(view) = self.web_view.as_mut() {
            if let Some([x, y]) = forward_click {
                view.send(json!({"click": [x, y]}));
            }
            if toggle_chrome {
                view.send(json!({"toggle_chrome": true}));
            }
        }

        // 動作（キー・マウス・ゲームパッド）
        match *action {
            MangaAction::CloseBook => {
                *action = MangaAction::None;
                self.leave_web_view();
            }
            MangaAction::ToggleAutoPageLayout => {
                *action = MangaAction::None;
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
            }
            // 本体が受け持つもの
            MangaAction::FullScreen | MangaAction::QuitApp => {}
            // 別のものを開くときは、窓の中で読むのをやめてから
            MangaAction::OpenFile | MangaAction::ShowHistory | MangaAction::ShowFavorites => {
                self.web_view = None;
            }
            other => {
                *action = MangaAction::None;
                if let Some(name) = plugin_action_name(other)
                    && let Some(view) = self.web_view.as_mut()
                {
                    view.send(json!({"action": name}));
                }
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
