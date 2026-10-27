//! サムネイル一覧の画面。ミーヤと同じく、開いたときは先頭から見せ、
//! 今のページに白枠、選んでいるページに青枠を付ける。クリックか Enter で決定し、
//! 見開きならそのページから始まる組で表示する。Esc で一覧を閉じて元のページに戻る。
//! 表紙サムネイルも同じ画面で、フォルダの中の冊を表紙で並べる

use super::MangaReader;
use crate::config::SourceMode;
use crate::engine::source::list_images;
use crate::engine::thumbnail::{
    CoverEntry, Direction, Grid, THUMBNAIL_MAX_SIDE, ThumbnailMode, cover_entries, move_selection,
    spawn_cover_loader, spawn_loader,
};
use crate::localize::tr;
use eframe::egui;
use image::RgbaImage;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;

/// 1フレームでテクスチャにするサムネイルの数（多いと一覧を開いた直後に引っかかる）
const TEXTURES_PER_FRAME: usize = 32;
/// PDF は別スレッドで描けないので、1フレームに描くページ数を絞る
const PDF_PAGES_PER_FRAME: usize = 2;
const SELECTED_COLOR: egui::Color32 = egui::Color32::from_rgb(70, 130, 255);
const FRAME_WIDTH: f32 = 2.0;
/// 表紙サムネイルで、マスの下に冊の名前を出す高さ
const COVER_LABEL_HEIGHT: f32 = 26.0;

/// 作ったサムネイル。一覧を閉じても、同じ冊のあいだは持っておく
#[derive(Default)]
pub(super) struct ThumbnailCache {
    book: Option<PathBuf>,
    pub(super) textures: Vec<Option<egui::TextureHandle>>,
}

/// 表紙サムネイルで並べているフォルダの冊
struct CoverShelf {
    folder: PathBuf,
    entries: Vec<CoverEntry>,
    textures: Vec<Option<egui::TextureHandle>>,
}

/// 開いている一覧
pub(super) struct ThumbnailView {
    mode: ThumbnailMode,
    selected: usize,
    scroll: f32,
    rx: Option<Receiver<(usize, RgbaImage)>>,
    cancel: Arc<AtomicBool>,
    /// PDF で次に描くページ
    pdf_next: usize,
    /// 表紙サムネイルのとき、並べている冊
    covers: Option<CoverShelf>,
}

impl ThumbnailView {
    /// 読み込むものが残っていないか（作り直しが要らなかったか）
    #[cfg(test)]
    pub(super) fn rx_is_idle(&self) -> bool {
        self.rx.is_none()
    }

    /// 表紙サムネイルで並べている冊
    #[cfg(test)]
    pub(super) fn cover_paths(&self) -> Vec<PathBuf> {
        self.covers
            .iter()
            .flat_map(|shelf| shelf.entries.iter().map(|entry| entry.path.clone()))
            .collect()
    }

    #[cfg(test)]
    pub(super) fn selected(&self) -> usize {
        self.selected
    }
}

/// 冊として開けるフォルダか（画像が直下にある）。画像の無いフォルダは中の冊の一覧に移る
fn folder_has_pages(folder: &std::path::Path) -> bool {
    list_images(folder, false).is_some_and(|listing| !listing.images.is_empty())
}

impl Drop for ThumbnailView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn color_image(image: &RgbaImage) -> egui::ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw())
}

impl MangaReader {
    /// サムネイル一覧を開く。同じ種類が開いていれば閉じ、別の種類なら切り替える
    pub(super) fn open_thumbnails(&mut self, mode: ThumbnailMode) {
        if self
            .thumbnails
            .as_ref()
            .is_some_and(|view| view.covers.is_some())
        {
            self.thumbnails = None;
        }
        if let Some(view) = self.thumbnails.as_mut() {
            if view.mode == mode {
                self.thumbnails = None;
            } else {
                view.mode = mode;
                view.scroll = 0.0;
            }
            return;
        }
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        if self.image_files.is_empty() {
            return;
        }
        let count = self.image_files.len();
        if self.thumbnail_cache.book.as_ref() != Some(&book)
            || self.thumbnail_cache.textures.len() != count
        {
            self.thumbnail_cache = ThumbnailCache {
                book: Some(book.clone()),
                textures: vec![None; count],
            };
        }
        let missing: Vec<usize> = (0..count)
            .filter(|&index| self.thumbnail_cache.textures[index].is_none())
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = (!missing.is_empty() && self.source_mode != SourceMode::Pdf).then(|| {
            spawn_loader(
                self.source_mode,
                book,
                self.image_files.clone(),
                missing,
                self.config.enable_auto_image_byte_fix,
                cancel.clone(),
            )
        });
        self.thumbnails = Some(ThumbnailView {
            mode,
            selected: self.current_index,
            // ミーヤと同じく、今のページが選ばれていても表示は先頭から
            scroll: 0.0,
            rx,
            cancel,
            pdf_next: 0,
            covers: None,
        });
    }

    /// 表紙サムネイル。ミーヤと同じく、書庫を開いていればそれが入っているフォルダ、
    /// フォルダを開いていればそのフォルダの中の冊を並べる。フォルダの中に冊が無ければ（画像だけのフォルダ）、
    /// それが入っているフォルダの冊を並べる。開いていれば閉じる
    pub(super) fn toggle_cover_thumbnails(&mut self) {
        if self
            .thumbnails
            .as_ref()
            .is_some_and(|view| view.covers.is_some())
        {
            self.thumbnails = None;
            return;
        }
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        let folder = if book.is_dir() && !cover_entries(&book).is_empty() {
            book.clone()
        } else {
            match book.parent() {
                Some(parent) => parent.to_path_buf(),
                None => return,
            }
        };
        self.open_cover_thumbnails(folder, Some(book));
    }

    /// 上のフォルダへ。今の冊が入っているフォルダの表紙サムネイルを出す。
    /// 表紙サムネイルを開いていれば、並べているフォルダの上のフォルダに移る
    pub(super) fn go_to_upper_folder(&mut self) {
        let from = match self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
        {
            Some(shelf) => shelf.folder.clone(),
            None => match self.zip_path.clone() {
                Some(book) => book,
                None => return,
            },
        };
        let Some(parent) = from.parent() else {
            return;
        };
        self.open_cover_thumbnails(parent.to_path_buf(), Some(from));
    }

    /// `folder` の中の冊を表紙で並べる。`select` があればそれを選んでおく
    fn open_cover_thumbnails(&mut self, folder: PathBuf, select: Option<PathBuf>) {
        let entries = cover_entries(&folder);
        if entries.is_empty() {
            self.show_fading_error(tr("error.no_books_in_folder"));
            return;
        }
        let position = |path: &PathBuf| entries.iter().position(|entry| &entry.path == path);
        let selected = select
            .as_ref()
            .and_then(position)
            .or_else(|| self.zip_path.as_ref().and_then(position))
            .unwrap_or(0);
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_cover_loader(
            entries.iter().map(|entry| entry.path.clone()).collect(),
            self.config.enable_auto_image_byte_fix,
            cancel.clone(),
        );
        let count = entries.len();
        self.thumbnails = Some(ThumbnailView {
            mode: ThumbnailMode::Covers,
            selected,
            scroll: 0.0,
            rx: Some(rx),
            cancel,
            pdf_next: 0,
            covers: Some(CoverShelf {
                folder,
                entries,
                textures: vec![None; count],
            }),
        });
    }

    /// 一覧に並べている数
    fn thumbnail_count(&self) -> usize {
        match self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
        {
            Some(shelf) => shelf.entries.len(),
            None => self.image_files.len(),
        }
    }

    fn thumbnail_texture(&self, index: usize) -> Option<&egui::TextureHandle> {
        match self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
        {
            Some(shelf) => shelf.textures.get(index)?.as_ref(),
            None => self.thumbnail_cache.textures.get(index)?.as_ref(),
        }
    }

    /// 白枠を付けるもの: ページの一覧なら今のページ、表紙の一覧なら今の冊
    fn thumbnail_current(&self) -> Option<usize> {
        match self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
        {
            Some(shelf) => shelf
                .entries
                .iter()
                .position(|entry| Some(&entry.path) == self.zip_path.as_ref()),
            None => Some(self.current_index),
        }
    }

    /// 左下に出す、選んでいるものの説明
    fn thumbnail_label(&self, selected: usize) -> String {
        let count = self.thumbnail_count();
        match self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
        {
            Some(shelf) => format!(
                "{} / {}  {}",
                selected + 1,
                count,
                shelf.folder.to_string_lossy()
            ),
            None => {
                let name = self.image_files[selected]
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default();
                format!("{} / {}  {}", selected + 1, count, name)
            }
        }
    }

    /// 冊が変わったら、前の冊のサムネイルは捨てる
    pub(super) fn forget_thumbnails(&mut self) {
        self.thumbnails = None;
        self.thumbnail_cache = ThumbnailCache::default();
    }

    /// 別スレッドで作ったサムネイルをテクスチャにする。PDF はここで少しずつ描く
    fn receive_thumbnails(&mut self, ctx: &egui::Context) {
        let Some(view) = self.thumbnails.as_mut() else {
            return;
        };
        let mut loading = false;
        if let Some(rx) = &view.rx {
            for _ in 0..TEXTURES_PER_FRAME {
                match rx.try_recv() {
                    Ok((index, image)) => {
                        let slots = match view.covers.as_mut() {
                            Some(shelf) => &mut shelf.textures,
                            None => &mut self.thumbnail_cache.textures,
                        };
                        if let Some(slot) = slots.get_mut(index) {
                            *slot = Some(ctx.load_texture(
                                format!("mangareeda:thumbnail:{index}"),
                                color_image(&image),
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        loading = true;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        loading = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        view.rx = None;
                        break;
                    }
                }
            }
        }
        let showing_covers = self
            .thumbnails
            .as_ref()
            .is_some_and(|view| view.covers.is_some());
        if self.source_mode == SourceMode::Pdf && !showing_covers {
            let mut drawn = 0;
            while drawn < PDF_PAGES_PER_FRAME {
                let Some(view) = self.thumbnails.as_mut() else {
                    return;
                };
                let index = view.pdf_next;
                if index >= self.thumbnail_cache.textures.len() {
                    break;
                }
                view.pdf_next += 1;
                if self.thumbnail_cache.textures[index].is_some() {
                    continue;
                }
                if let Some(page) = self.render_pdf_page(index, ctx) {
                    let image = page
                        .thumbnail(THUMBNAIL_MAX_SIDE, THUMBNAIL_MAX_SIDE)
                        .to_rgba8();
                    self.thumbnail_cache.textures[index] = Some(ctx.load_texture(
                        format!("mangareeda:thumbnail:{index}"),
                        color_image(&image),
                        egui::TextureOptions::LINEAR,
                    ));
                }
                drawn += 1;
                loading = true;
            }
        }
        if loading {
            ctx.request_repaint();
        }
    }

    /// 一覧で選んだページを表示して一覧を閉じる。表紙の一覧では、選んだ冊をしおりから開く
    /// （ミーヤと同じ）。画像の無いフォルダは、中の冊の一覧に移る
    pub(super) fn choose_thumbnail(&mut self, index: usize, ctx: &egui::Context) {
        let Some(view) = self.thumbnails.take() else {
            return;
        };
        let Some(shelf) = view.covers.as_ref() else {
            self.show_page_from_thumbnail(index, ctx);
            return;
        };
        let Some(entry) = shelf.entries.get(index).cloned() else {
            return;
        };
        if entry.is_folder && !folder_has_pages(&entry.path) {
            self.open_cover_thumbnails(entry.path, None);
        } else {
            self.open_source(entry.path, ctx);
        }
    }

    /// サムネイル一覧を描き、操作を受け付ける
    pub(super) fn render_thumbnails(&mut self, ctx: &egui::Context) {
        self.receive_thumbnails(ctx);
        let count = self.thumbnail_count();
        let current = self.thumbnail_current();
        let Some(mode) = self.thumbnails.as_ref().map(|view| view.mode) else {
            return;
        };
        let covers = mode == ThumbnailMode::Covers;
        let background = self.image_panel_background_color();
        let mut chosen = None;
        let mut close = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(background))
            .show(ctx, |ui| {
                let rect = ui.available_rect_before_wrap();
                let grid = Grid::new(mode, count, [rect.width(), rect.height()]);
                let [grid_width, grid_height] = grid.size();
                let max_scroll = (grid_height - rect.height()).max(0.0);
                let Some(view) = self.thumbnails.as_mut() else {
                    return;
                };
                if count == 0 {
                    return;
                }

                // キー: 方向キーで選び、Enter で決定、Esc で閉じる
                let (keys, scroll_delta, pointer, pointer_moved, clicked) = ctx.input(|i| {
                    let keys = [
                        (egui::Key::ArrowLeft, Direction::Left),
                        (egui::Key::ArrowRight, Direction::Right),
                        (egui::Key::ArrowUp, Direction::Up),
                        (egui::Key::ArrowDown, Direction::Down),
                    ]
                    .into_iter()
                    .filter(|(key, _)| i.key_pressed(*key))
                    .map(|(_, direction)| direction)
                    .collect::<Vec<_>>();
                    (
                        (
                            keys,
                            i.key_pressed(egui::Key::Enter),
                            i.key_pressed(egui::Key::Escape),
                        ),
                        i.smooth_scroll_delta.y,
                        i.pointer.hover_pos(),
                        i.pointer.delta() != egui::Vec2::ZERO,
                        i.pointer.primary_clicked(),
                    )
                });
                let (directions, enter, escape) = keys;
                let moved_by_key = !directions.is_empty();
                for direction in directions {
                    view.selected = move_selection(view.selected, direction, grid.columns, count);
                }
                if moved_by_key {
                    // 選んだマスが見えるようにスクロールする
                    let top = grid.cell_origin(view.selected)[1];
                    if top < view.scroll {
                        view.scroll = top;
                    } else if top + grid.cell > view.scroll + rect.height() {
                        view.scroll = top + grid.cell - rect.height();
                    }
                }
                view.scroll = (view.scroll - scroll_delta).clamp(0.0, max_scroll);

                // 1画面に収まるときは中央に置く
                let origin = egui::pos2(
                    rect.left() + ((rect.width() - grid_width) * 0.5).max(0.0),
                    rect.top() + ((rect.height() - grid_height) * 0.5).max(0.0) - view.scroll,
                );
                let hovered = pointer
                    .filter(|pos| rect.contains(*pos))
                    .and_then(|pos| grid.index_at([pos.x - origin.x, pos.y - origin.y], count));
                if let Some(index) = hovered {
                    // マウスを動かしたマスを選ぶ（ミーヤと同じく青枠が付く）
                    if pointer_moved {
                        view.selected = index;
                    }
                    if clicked {
                        chosen = Some(index);
                    }
                }
                if enter {
                    chosen = Some(view.selected);
                }
                close = escape;
                let selected = view.selected;
                let scroll = view.scroll;

                let painter = ui.painter().with_clip_rect(rect);
                let grid_rect =
                    egui::Rect::from_min_size(origin, egui::vec2(grid_width, grid_height));
                painter.rect_filled(grid_rect, 0.0, egui::Color32::BLACK);
                let first_row = (scroll / grid.cell).floor() as usize;
                let last_row = ((scroll + rect.height()) / grid.cell).ceil() as usize;
                let first = first_row * grid.columns;
                let last = (last_row * grid.columns).min(count);
                for index in first..last {
                    let [x, y] = grid.cell_origin(index);
                    let cell = egui::Rect::from_min_size(
                        origin + egui::vec2(x, y),
                        egui::vec2(grid.cell, grid.cell),
                    );
                    // 表紙の一覧では、下に冊の名前を出すぶん画像を小さくする
                    let image_area = if covers {
                        egui::Rect::from_min_max(
                            cell.min,
                            cell.max - egui::vec2(0.0, COVER_LABEL_HEIGHT),
                        )
                        .shrink(6.0)
                    } else {
                        cell
                    };
                    if let Some(texture) = self.thumbnail_texture(index) {
                        // マスに収まるよう縮める（横長は幅に、縦長は高さに合わせる）
                        let size = texture.size_vec2();
                        let scale = (image_area.width() / size.x).min(image_area.height() / size.y);
                        painter.image(
                            texture.id(),
                            egui::Rect::from_center_size(image_area.center(), size * scale),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    } else {
                        painter.rect_filled(
                            image_area.shrink(image_area.width() * 0.1),
                            0.0,
                            egui::Color32::from_gray(40),
                        );
                    }
                    if let Some(entry) = self
                        .thumbnails
                        .as_ref()
                        .and_then(|view| view.covers.as_ref())
                        .and_then(|shelf| shelf.entries.get(index))
                    {
                        // ミーヤと同じく、アイコンと名前を表紙の下に出す
                        let icon = if entry.is_folder { "📁" } else { "📖" };
                        let name = entry.path.file_name().unwrap_or_default().to_string_lossy();
                        let label_rect = egui::Rect::from_min_max(
                            egui::pos2(cell.min.x, cell.max.y - COVER_LABEL_HEIGHT),
                            cell.max,
                        );
                        painter.with_clip_rect(label_rect.intersect(rect)).text(
                            label_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            format!("{icon} {name}"),
                            egui::FontId::proportional(14.0),
                            egui::Color32::WHITE,
                        );
                    }
                    let frame = if index == selected {
                        Some(SELECTED_COLOR)
                    } else if Some(index) == current {
                        Some(egui::Color32::WHITE)
                    } else {
                        None
                    };
                    if let Some(color) = frame {
                        painter.rect_stroke(
                            cell,
                            0.0,
                            egui::Stroke::new(FRAME_WIDTH, color),
                            egui::StrokeKind::Inside,
                        );
                    }
                }

                // 選んでいるものの番号と名前（ミーヤではタイトルバーに出る）
                let label = self.thumbnail_label(selected);
                let galley = painter.layout_no_wrap(
                    label,
                    egui::FontId::proportional(16.0),
                    egui::Color32::WHITE,
                );
                let label_rect = egui::Rect::from_min_size(
                    rect.left_bottom() + egui::vec2(8.0, -galley.size().y - 12.0),
                    galley.size() + egui::vec2(12.0, 6.0),
                );
                painter.rect_filled(label_rect, 4.0, egui::Color32::from_black_alpha(180));
                painter.galley(
                    label_rect.min + egui::vec2(6.0, 3.0),
                    galley,
                    egui::Color32::WHITE,
                );
            });
        if let Some(index) = chosen {
            self.choose_thumbnail(index, ctx);
        } else if close {
            self.thumbnails = None;
        }
    }
}
