//! サムネイル一覧の画面。ミーヤと同じく、開いたときは先頭から見せ、
//! 今のページに白枠、選んでいるページに青枠を付ける。クリックか Enter で決定し、
//! 見開きならそのページから始まる組で表示する。Esc で一覧を閉じて元のページに戻る。
//! 表紙サムネイルも同じ画面で、フォルダの中の冊・履歴・お気に入りを表紙で並べる。
//! 上のタブ（Tab キー）で並べるものを切り替える

use super::MangaReader;
use super::loader::OpenAt;
use super::plugins::DownloadStatus;
use crate::config::SourceMode;
use crate::engine::plugin::PluginError;
use crate::engine::source::page_key;
use crate::engine::thumbnail::{
    CoverEntry, CoverOptions, CoverSource, Direction, Grid, THUMBNAIL_MAX_SIDE, ThumbnailMode,
    cover_cache_name, cover_entries, folder_shelf, move_selection, spawn_cover_loader,
    spawn_loader,
};
use crate::localize::tr;
use eframe::egui;
use image::RgbaImage;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{SystemTime, UNIX_EPOCH};

/// 1フレームでテクスチャにするサムネイルの数（多いと一覧を開いた直後に引っかかる）
const TEXTURES_PER_FRAME: usize = 32;
/// PDF は別スレッドで描けないので、1フレームに描くページ数を絞る
const PDF_PAGES_PER_FRAME: usize = 2;
const SELECTED_COLOR: egui::Color32 = egui::Color32::from_rgb(70, 130, 255);
const FRAME_WIDTH: f32 = 2.0;
/// 表紙サムネイルで、マスの下に冊の名前を出す高さ（2行ぶん）と行数
const COVER_LABEL_HEIGHT: f32 = 36.0;
const COVER_LABEL_ROWS: usize = 2;
/// 表紙サムネイルの上に出す、並べるものを切り替えるタブの高さ
const TAB_HEIGHT: f32 = 30.0;
/// 戻るで覚えておく数
const SHELF_BACK_LIMIT: usize = 50;

/// 作ったサムネイル。一覧を閉じても、同じ冊のあいだは持っておく
#[derive(Default)]
pub(super) struct ThumbnailCache {
    book: Option<PathBuf>,
    pub(super) textures: Vec<Option<egui::TextureHandle>>,
}

/// 表紙サムネイルで並べるもの
#[derive(Clone, Debug, PartialEq)]
pub(super) enum ShelfSource {
    /// フォルダの中の冊
    Folder(PathBuf),
    History,
    Favorites,
    /// プラグインの一覧（プラグインの id, フォルダ。"" は一番上）
    Plugin(String, String),
}

/// タブの並び。プラグインは見つけた順の番号
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum ShelfTab {
    Folder,
    History,
    Favorites,
    Plugin(usize),
}

/// 表紙サムネイルで並べている冊
struct CoverShelf {
    source: ShelfSource,
    entries: Vec<CoverEntry>,
    textures: Vec<Option<egui::TextureHandle>>,
    /// 読み込み中の表紙の、送られてくる番号から entries の番号への対応
    loading: Vec<usize>,
}

/// 何秒前かを「3日前」のように短く言う
fn time_ago(then: u64, now: u64) -> String {
    let seconds = now.saturating_sub(then);
    let (key, n) = match seconds {
        0..60 => return tr("time.just_now").to_owned(),
        60..3600 => ("time.minutes_ago", seconds / 60),
        3600..86400 => ("time.hours_ago", seconds / 3600),
        _ => ("time.days_ago", seconds / 86400),
    };
    tr(key).replace("{n}", &n.to_string())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
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

    #[cfg(test)]
    pub(super) fn select(&mut self, index: usize) {
        self.selected = index;
    }
}

/// 冊を並べるフォルダ。冊のフォルダの中に冊があれば（画像の無いフォルダ）そのフォルダ、
/// 無ければ冊が入っているフォルダ
fn shelf_folder_of(book: &Path) -> Option<PathBuf> {
    if book.is_dir() && !cover_entries(book).is_empty() {
        Some(book.to_path_buf())
    } else {
        book.parent().map(Path::to_path_buf)
    }
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
        match self.shelf_source() {
            Some(ShelfSource::Folder(_)) => self.thumbnails = None,
            // 履歴やお気に入りを並べていれば、フォルダのタブに移る
            Some(_) => self.switch_shelf_tab(ShelfTab::Folder),
            None => match self.zip_path.clone() {
                Some(book) => {
                    if let Some(folder) = shelf_folder_of(&book) {
                        self.open_cover_thumbnails(folder, Some(book));
                    }
                }
                // 冊を開いていなければ、フォルダのタブと同じ順で探したフォルダ
                None => {
                    if let Some(folder) = self.fallback_shelf_folder(None) {
                        self.show_shelf(ShelfSource::Folder(folder), None);
                    }
                }
            },
        }
    }

    /// フォルダの表紙サムネイルで並べるフォルダ。今の冊、`selected` の冊、前に並べたフォルダ、
    /// 最後に読んだ冊、ホームの順に探す
    fn fallback_shelf_folder(&self, selected: Option<PathBuf>) -> Option<PathBuf> {
        self.zip_path
            .clone()
            .or(selected)
            .and_then(|book| shelf_folder_of(&book))
            .or_else(|| self.last_shelf_folder.clone())
            .or_else(|| {
                let entry = self.history.entries.iter().find(|e| e.path.exists())?;
                shelf_folder_of(&entry.path)
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
            })
    }

    /// 履歴やお気に入りを表紙で並べる。同じものを並べていれば閉じる
    pub(super) fn toggle_book_list(&mut self, source: ShelfSource) {
        if self.shelf_source().as_ref() == Some(&source) {
            self.thumbnails = None;
            return;
        }
        self.show_shelf(source, None);
    }

    /// 今の冊をお気に入りに登録する。登録済みなら外す
    pub(super) fn toggle_current_favorite(&mut self) {
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        let added = self.history.toggle_favorite(&book);
        self.save_history();
        self.show_fading_error(tr(if added {
            "shelf.favorite_added"
        } else {
            "shelf.favorite_removed"
        }));
    }

    /// いま見ているページを、この冊の表紙にする。MangaReeda の記録（履歴のファイル）に持ち、本のファイルは変えない
    /// （プラグインからダウンロードした本も同じように選べる）
    pub(super) fn choose_current_page_as_cover(&mut self) {
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        let Some(name) = self.image_files.get(self.current_index) else {
            return;
        };
        let page = page_key(self.source_mode, &book, name);
        self.history.choose_cover(&book, page);
        self.save_history();
        // 表紙の一覧を出していれば、この冊の表紙を作り直す
        if let Some(shelf) = self
            .thumbnails
            .as_mut()
            .and_then(|view| view.covers.as_mut())
        {
            for (entry, texture) in shelf.entries.iter().zip(shelf.textures.iter_mut()) {
                if entry.path == book {
                    *texture = None;
                }
            }
            self.reshow_shelf();
        }
        self.show_fading_error(tr("shelf.cover_chosen"));
    }

    /// 並べているもの。表紙サムネイルを開いていなければ None
    pub(super) fn shelf_source(&self) -> Option<ShelfSource> {
        self.thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
            .map(|shelf| shelf.source.clone())
    }

    /// 上のフォルダへ。今の冊が入っているフォルダの表紙サムネイルを出す。
    /// 表紙サムネイルを開いていれば、並べているフォルダ（履歴などでは選んでいる冊）の上のフォルダに移る
    pub(super) fn go_to_upper_folder(&mut self) {
        if let Some(ShelfSource::Plugin(id, folder)) = self.shelf_source() {
            let key = (id.clone(), folder.clone());
            if let Some(parent) = self.plugin_parent(&key) {
                self.show_shelf(ShelfSource::Plugin(id, parent), None);
                self.select_remote(&folder);
            }
            return;
        }
        let from = match self
            .thumbnails
            .as_ref()
            .and_then(|view| Some((view.selected, view.covers.as_ref()?)))
        {
            Some((selected, shelf)) => match &shelf.source {
                ShelfSource::Folder(folder) => folder.clone(),
                _ => match shelf.entries.get(selected) {
                    Some(entry) => entry.path.clone(),
                    None => return,
                },
            },
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

    /// プラグインの一覧で、id が `id` の本（フォルダ）を選ぶ
    fn select_remote(&mut self, id: &str) {
        let Some(view) = self.thumbnails.as_mut() else {
            return;
        };
        let Some(shelf) = view.covers.as_ref() else {
            return;
        };
        if let Some(index) = shelf
            .entries
            .iter()
            .position(|entry| entry.remote.as_ref().is_some_and(|r| r.id == id))
        {
            view.selected = index;
        }
    }

    /// `folder` の中の冊を表紙で並べる。`select` があればそれを選んでおく
    fn open_cover_thumbnails(&mut self, folder: PathBuf, select: Option<PathBuf>) {
        if cover_entries(&folder).is_empty() {
            self.show_fading_error(tr("error.no_books_in_folder"));
            return;
        }
        self.show_shelf(ShelfSource::Folder(folder), select);
    }

    /// 並べる冊。履歴とお気に入りは、今は無い冊（消したものや外したドライブ）を除く
    fn shelf_entries(&self, source: &ShelfSource) -> Vec<CoverEntry> {
        let paths: Vec<&PathBuf> = match source {
            ShelfSource::Folder(folder) => return folder_shelf(folder),
            ShelfSource::Plugin(id, folder) => {
                return self.plugin_entries(&(id.clone(), folder.clone()));
            }
            ShelfSource::History => self.history.entries.iter().map(|e| &e.path).collect(),
            ShelfSource::Favorites => self.history.favorites.iter().map(|e| &e.path).collect(),
        };
        paths
            .into_iter()
            .filter(|path| path.exists())
            .map(|path| CoverEntry {
                path: path.clone(),
                is_folder: path.is_dir(),
                loose_pages: None,
                remote: None,
            })
            .collect()
    }

    /// タブの並び: フォルダ・履歴・お気に入りと、見つけたプラグイン
    fn shelf_tabs(&self) -> Vec<ShelfTab> {
        let mut tabs = vec![ShelfTab::Folder, ShelfTab::History, ShelfTab::Favorites];
        tabs.extend((0..self.plugins.manifests.len()).map(ShelfTab::Plugin));
        tabs
    }

    fn tab_label(&self, tab: ShelfTab) -> String {
        match tab {
            ShelfTab::Folder => tr("shelf.folder").to_owned(),
            ShelfTab::History => tr("shelf.history").to_owned(),
            ShelfTab::Favorites => tr("shelf.favorites").to_owned(),
            ShelfTab::Plugin(index) => self
                .plugins
                .manifests
                .get(index)
                .map_or_else(String::new, |m| m.name.clone()),
        }
    }

    fn tab_of(&self, source: &ShelfSource) -> ShelfTab {
        match source {
            ShelfSource::Folder(_) => ShelfTab::Folder,
            ShelfSource::History => ShelfTab::History,
            ShelfSource::Favorites => ShelfTab::Favorites,
            ShelfSource::Plugin(id, _) => ShelfTab::Plugin(
                self.plugins
                    .manifests
                    .iter()
                    .position(|m| &m.id == id)
                    .unwrap_or(0),
            ),
        }
    }

    /// 一覧で選んでいる冊の場所
    pub(super) fn selected_shelf_entry(&self) -> Option<CoverEntry> {
        let view = self.thumbnails.as_ref()?;
        view.covers.as_ref()?.entries.get(view.selected).cloned()
    }

    pub(super) fn selected_shelf_path(&self) -> Option<PathBuf> {
        let view = self.thumbnails.as_ref()?;
        Some(
            view.covers
                .as_ref()?
                .entries
                .get(view.selected)?
                .path
                .clone(),
        )
    }

    /// 並べているものを、選んでいる冊とスクロールの位置を保ったまま並べ直す
    pub(super) fn reshow_shelf(&mut self) {
        let Some(source) = self.shelf_source() else {
            return;
        };
        let selected = self.selected_shelf_path();
        let scroll = self.thumbnails.as_ref().map_or(0.0, |view| view.scroll);
        self.show_shelf(source, selected);
        if let Some(view) = self.thumbnails.as_mut() {
            view.scroll = scroll;
        }
    }

    /// `source` を表紙で並べる。`select` があればそれを、無ければ今の冊を選んでおく。
    /// 前に並べていた表紙は、同じ冊なら作り直さずに使う
    fn show_shelf(&mut self, source: ShelfSource, select: Option<PathBuf>) {
        let previous = self.thumbnails.as_ref().and_then(|view| {
            let shelf = view.covers.as_ref()?;
            let selected = shelf.entries.get(view.selected).map(|e| e.path.clone());
            Some((shelf.source.clone(), selected))
        });
        match previous {
            // 一覧を新しく開いたときは、前の道のりを忘れる
            None => self.shelf_back.clear(),
            Some((previous, selected)) if previous != source && !self.going_back => {
                self.shelf_back.push((previous, selected));
                if self.shelf_back.len() > SHELF_BACK_LIMIT {
                    self.shelf_back.remove(0);
                }
            }
            Some(_) => {}
        }
        if let ShelfSource::Plugin(id, folder) = &source {
            self.ensure_plugin_list(&(id.clone(), folder.clone()));
        }
        let entries = self.shelf_entries(&source);
        if let ShelfSource::Folder(folder) = &source {
            self.last_shelf_folder = Some(folder.clone());
        }
        let position = |path: &PathBuf| entries.iter().position(|entry| &entry.path == path);
        let selected = select
            .as_ref()
            .and_then(position)
            .or_else(|| self.zip_path.as_ref().and_then(position))
            .unwrap_or(0);
        let mut made: HashMap<PathBuf, egui::TextureHandle> = self
            .thumbnails
            .take()
            .and_then(|mut view| view.covers.take())
            .map(|shelf| {
                shelf
                    .entries
                    .into_iter()
                    .zip(shelf.textures)
                    .filter_map(|(entry, texture)| Some((entry.path, texture?)))
                    .collect()
            })
            .unwrap_or_default();
        let textures: Vec<Option<egui::TextureHandle>> = entries
            .iter()
            .map(|entry| made.remove(&entry.path))
            .collect();
        let loading: Vec<usize> = (0..entries.len())
            .filter(|&index| textures[index].is_none())
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let options = (!loading.is_empty()).then(|| CoverOptions {
            fix_adobe: self.config.enable_auto_image_byte_fix,
            pdfium: crate::engine::pdf::pdfium(),
            chosen: Arc::new(self.history.cover_choices()),
        });
        let covers = self.plugins.cover_cache_dir();
        let sources: Vec<CoverSource> = loading
            .iter()
            .map(|&index| {
                let entry = &entries[index];
                // プラグインの本は、手元に置く（置いた）場所で探す
                let local = self.remote_path(entry);
                // 表紙の URL があるプラグインの本は URL から。取れなければ、ダウンロードした本から
                match entry.remote.as_ref().and_then(|r| r.cover_url.as_ref()) {
                    Some(url) => CoverSource::Url {
                        url: url.clone(),
                        cache: covers.join(cover_cache_name(url)),
                        local: Some(local),
                    },
                    None => CoverSource::Local(local),
                }
            })
            .collect();
        let rx = options.map(|options| spawn_cover_loader(sources, options, cancel.clone()));
        self.thumbnails = Some(ThumbnailView {
            mode: ThumbnailMode::Covers,
            selected,
            scroll: 0.0,
            rx,
            cancel,
            pdf_next: 0,
            covers: Some(CoverShelf {
                source,
                entries,
                textures,
                loading,
            }),
        });
    }

    /// タブを切り替える。フォルダのタブは今の冊（無ければ選んでいる冊）が入っているフォルダ
    pub(super) fn switch_shelf_tab(&mut self, tab: ShelfTab) {
        let Some(source) = self.shelf_source() else {
            return;
        };
        if self.tab_of(&source) == tab {
            return;
        }
        let selected_path = self.thumbnails.as_ref().and_then(|view| {
            let shelf = view.covers.as_ref()?;
            Some(shelf.entries.get(view.selected)?.path.clone())
        });
        let source = match tab {
            ShelfTab::Plugin(index) => match self.plugins.manifests.get(index) {
                Some(manifest) => {
                    ShelfSource::Plugin(manifest.id.clone(), self.plugin_start_folder(&manifest.id))
                }
                None => return,
            },
            ShelfTab::History => ShelfSource::History,
            ShelfTab::Favorites => ShelfSource::Favorites,
            ShelfTab::Folder => match self.fallback_shelf_folder(selected_path) {
                Some(folder) => ShelfSource::Folder(folder),
                None => return,
            },
        };
        self.show_shelf(source, None);
    }

    /// 表紙サムネイルで、一つ前に並べていたものに戻る。そのとき選んでいた冊を選んでおく
    pub(super) fn go_back_in_shelf(&mut self) {
        if self.shelf_source().is_none() {
            return;
        }
        let Some((source, selected)) = self.shelf_back.pop() else {
            return;
        };
        self.going_back = true;
        self.show_shelf(source, selected);
        self.going_back = false;
    }

    /// 履歴やお気に入りから、選んでいる冊を外す（ファイルは消さない）
    pub(super) fn remove_selected_from_shelf(&mut self) {
        let Some(view) = self.thumbnails.as_ref() else {
            return;
        };
        let Some(shelf) = view.covers.as_ref() else {
            return;
        };
        let Some(entry) = shelf.entries.get(view.selected) else {
            return;
        };
        let path = entry.path.clone();
        let source = shelf.source.clone();
        let selected = view.selected;
        match source {
            ShelfSource::History => self.history.forget(&path),
            ShelfSource::Favorites => self.history.remove_favorite(&path),
            ShelfSource::Folder(_) | ShelfSource::Plugin(..) => return,
        }
        self.save_history();
        self.show_shelf(source, None);
        // 外したところの次の冊を選ぶ
        if let Some(view) = self.thumbnails.as_mut() {
            let count = view.covers.as_ref().map_or(0, |shelf| shelf.entries.len());
            view.selected = selected.min(count.saturating_sub(1));
        }
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
            Some(shelf) => {
                let Some(entry) = shelf.entries.get(selected) else {
                    return String::new();
                };
                // プラグインの本は、書名とダウンロードの状態
                if let Some(remote) = &entry.remote {
                    let status = match self.download_status(entry) {
                        Some(DownloadStatus::Running(Some(ratio))) => {
                            format!("{}%", (ratio * 100.0) as u32)
                        }
                        Some(DownloadStatus::Running(None)) => tr("plugin.downloading").to_owned(),
                        Some(DownloadStatus::Queued) => tr("plugin.queued").to_owned(),
                        None if self.remote_path(entry).is_file() => {
                            tr("plugin.downloaded").to_owned()
                        }
                        None => tr("plugin.not_downloaded").to_owned(),
                    };
                    return format!(
                        "{} / {}  {}  ({})",
                        selected + 1,
                        count,
                        remote.title,
                        status
                    );
                }
                // 履歴とお気に入りは冊がばらばらの場所にあるので、冊のパスと日時を出す
                let (place, when) = match &shelf.source {
                    ShelfSource::Folder(folder) => (folder.to_string_lossy(), None),
                    ShelfSource::Plugin(id, _) => (std::borrow::Cow::Borrowed(id.as_str()), None),
                    ShelfSource::History => (
                        entry.path.to_string_lossy(),
                        self.history
                            .entries
                            .iter()
                            .find(|e| e.path == entry.path)
                            .map(|e| e.last_access),
                    ),
                    ShelfSource::Favorites => (
                        entry.path.to_string_lossy(),
                        self.history
                            .favorites
                            .iter()
                            .find(|e| e.path == entry.path)
                            .map(|e| e.added),
                    ),
                };
                match when {
                    Some(then) => format!(
                        "{} / {}  {}  ({})",
                        selected + 1,
                        count,
                        place,
                        time_ago(then, unix_now())
                    ),
                    None => format!("{} / {}  {}", selected + 1, count, place),
                }
            }
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
                        let (slots, index) = match view.covers.as_mut() {
                            Some(shelf) => {
                                let Some(&index) = shelf.loading.get(index) else {
                                    continue;
                                };
                                (&mut shelf.textures, index)
                            }
                            None => (&mut self.thumbnail_cache.textures, index),
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
    /// （ミーヤと同じ）。冊の入ったフォルダは、中の冊の一覧に移る
    pub(super) fn choose_thumbnail(&mut self, index: usize, ctx: &egui::Context) {
        let Some(view) = self.thumbnails.as_mut() else {
            return;
        };
        let Some(shelf) = view.covers.as_ref() else {
            self.thumbnails = None;
            self.show_page_from_thumbnail(index, ctx);
            return;
        };
        let Some(mut entry) = shelf.entries.get(index).cloned() else {
            return;
        };
        // 一覧を開いたまま移るときに、戻るで選んだものに戻れるように
        view.selected = index;
        if let Some(remote) = &entry.remote
            && entry.is_folder
        {
            let source = ShelfSource::Plugin(remote.plugin.clone(), remote.id.clone());
            self.show_shelf(source, None);
            return;
        }
        // プラグインの本は、ダウンロードしてあれば開き、無ければダウンロードする（終わったら開く）。
        // 置く場所は、並べたあとに別の本が使い始めていないか確かめ直す
        if entry.remote.is_some() {
            let path = self.remote_path(&entry);
            if !path.is_file() {
                self.request_download(&entry);
                return;
            }
            entry.path = path;
        }
        if entry.loose_pages.is_none() && entry.is_folder && !cover_entries(&entry.path).is_empty()
        {
            self.open_cover_thumbnails(entry.path, None);
            return;
        }
        self.thumbnails = None;
        if entry.loose_pages.is_some() {
            // フォルダの直下の画像だけを読む（サブフォルダは一覧から選ぶ）
            self.open_book_with(entry.path, false, OpenAt::Bookmark, ctx);
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
        let source = self.shelf_source();
        let covers = mode == ThumbnailMode::Covers;
        let background = self.image_panel_background_color();
        let mut chosen = None;
        let mut close = false;
        let mut tab_to_show = None;
        let mut remove_selected = false;
        let mut go_up = false;
        let mut go_back = false;
        let mut refresh = false;
        let tabs: Vec<(ShelfTab, String)> = self
            .shelf_tabs()
            .into_iter()
            .map(|tab| (tab, self.tab_label(tab)))
            .collect();
        let active_tab = source.as_ref().map(|source| self.tab_of(source));
        // プラグインの一覧の状態（空のときの知らせと、更新中か）
        let plugin_state = match &source {
            Some(ShelfSource::Plugin(id, folder)) => self
                .plugins
                .shelves
                .get(&(id.clone(), folder.clone()))
                .map(|shelf| {
                    (
                        shelf.is_loading(),
                        shelf.error.clone(),
                        self.plugins.download_error.clone(),
                    )
                }),
            _ => None,
        };
        let statuses: Vec<(Option<DownloadStatus>, bool)> = self
            .thumbnails
            .as_ref()
            .and_then(|view| view.covers.as_ref())
            .map(|shelf| {
                shelf
                    .entries
                    .iter()
                    .map(|entry| match entry.remote {
                        Some(_) => (
                            self.download_status(entry),
                            self.remote_path(entry).is_file(),
                        ),
                        None => (None, true),
                    })
                    .collect()
            })
            .unwrap_or_default();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(background))
            .show(ctx, |ui| {
                let mut rect = ui.available_rect_before_wrap();

                // キー: 方向キーで選び、Enter で決定、Esc で閉じる。
                // 表紙の一覧では Tab で並べるものを切り替え、Delete で履歴やお気に入りから外す
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
                            i.key_pressed(egui::Key::Backspace),
                            i.key_pressed(egui::Key::Delete),
                            i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft)
                                || i.pointer.button_clicked(egui::PointerButton::Extra1),
                            i.key_pressed(egui::Key::F5),
                        ),
                        i.smooth_scroll_delta.y,
                        i.pointer.hover_pos(),
                        i.pointer.delta() != egui::Vec2::ZERO,
                        i.pointer.primary_clicked(),
                    )
                });
                let (mut directions, enter, escape, backspace, delete, back, f5) = keys;
                let tab_key = self.pending_shelf_tab.take();
                close = escape;

                if let (Some(source), Some(active)) = (&source, active_tab) {
                    let tab_rect =
                        egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), TAB_HEIGHT));
                    rect.min.y += TAB_HEIGHT;
                    if let Some(backward) = tab_key {
                        let position = tabs.iter().position(|(t, _)| *t == active).unwrap_or(0);
                        let step = if backward { tabs.len() - 1 } else { 1 };
                        tab_to_show = Some(tabs[(position + step) % tabs.len()].0);
                    }
                    let in_plugin = matches!(source, ShelfSource::Plugin(..));
                    // 上のフォルダへ移れるか（プラグインは上のフォルダがあるときだけ）
                    let can_go_up = match source {
                        ShelfSource::Folder(_) => true,
                        ShelfSource::Plugin(id, folder) => {
                            self.plugin_parent(&(id.clone(), folder.clone())).is_some()
                        }
                        _ => false,
                    };
                    refresh |= f5 && in_plugin;
                    let strip = TabStrip {
                        tabs: &tabs,
                        active,
                        can_go_back: !self.shelf_back.is_empty(),
                        can_go_up,
                        refreshing: plugin_state.as_ref().map(|state| state.0),
                        error: plugin_state.as_ref().and_then(|state| state.2.clone()),
                    };
                    match draw_shelf_tabs(ui, tab_rect, &strip, pointer.filter(|_| clicked)) {
                        Some(TabClick::Tab(tab)) => tab_to_show = Some(tab),
                        Some(TabClick::Upper) => go_up = true,
                        Some(TabClick::Back) => go_back = true,
                        Some(TabClick::Refresh) => refresh = true,
                        None => {}
                    }
                    if back {
                        go_back = true;
                        // Alt+← で選択が左に動かないように
                        directions.clear();
                    }
                    let in_folder = matches!(source, ShelfSource::Folder(_));
                    remove_selected = delete && !in_folder && !in_plugin;
                    go_up |= backspace && can_go_up;
                    if count == 0 {
                        let message = match source {
                            ShelfSource::Folder(_) => tr("shelf.no_books").to_owned(),
                            ShelfSource::History => tr("shelf.no_history").to_owned(),
                            ShelfSource::Favorites => tr("shelf.no_favorites").to_owned(),
                            ShelfSource::Plugin(..) => match &plugin_state {
                                Some((true, _, _)) => tr("plugin.loading").to_owned(),
                                Some((_, Some(PluginError::Auth(message)), _)) => {
                                    format!("{message}\n{}", tr("plugin.set_credentials"))
                                }
                                Some((_, Some(error), _)) => error.message().to_owned(),
                                _ => tr("plugin.no_books").to_owned(),
                            },
                        };
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            message,
                            egui::FontId::proportional(18.0),
                            egui::Color32::from_gray(200),
                        );
                    }
                }

                let grid = Grid::new(mode, count, [rect.width(), rect.height()]);
                let [grid_width, grid_height] = grid.size();
                let max_scroll = (grid_height - rect.height()).max(0.0);
                let Some(view) = self.thumbnails.as_mut() else {
                    return;
                };
                if count == 0 {
                    return;
                }

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
                    let cover_entry = self
                        .thumbnails
                        .as_ref()
                        .and_then(|view| view.covers.as_ref())
                        .and_then(|shelf| shelf.entries.get(index));
                    if let Some(texture) = self.thumbnail_texture(index) {
                        // マスに収まるよう縮める（横長は幅に、縦長は高さに合わせる）
                        let size = texture.size_vec2();
                        let scale = (image_area.width() / size.x).min(image_area.height() / size.y);
                        let image_rect =
                            egui::Rect::from_center_size(image_area.center(), size * scale);
                        painter.image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                        // 冊の入ったフォルダは、中の表紙に、隅のフォルダの印で本と見分けられるようにする
                        if cover_entry.is_some_and(|e| e.is_folder && e.loose_pages.is_none()) {
                            paint_folder_badge(&painter, image_rect);
                        }
                    } else if let Some(entry) = cover_entry {
                        // 表紙が無い（まだ作っていない・作れない）項目は、フォルダか本の絵
                        paint_placeholder(&painter, image_area, entry);
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
                        let (status, downloaded) =
                            statuses.get(index).copied().unwrap_or((None, true));
                        let (icon, name) = match (&entry.remote, entry.loose_pages) {
                            // まだダウンロードしていない本は雲のしるし
                            (Some(remote), _) => (
                                if entry.is_folder {
                                    "📁"
                                } else if downloaded {
                                    "📖"
                                } else {
                                    "☁"
                                },
                                remote.title.as_str().into(),
                            ),
                            (None, Some(count)) => (
                                "🖼",
                                tr("shelf.loose_pages")
                                    .replace("{n}", &count.to_string())
                                    .into(),
                            ),
                            (None, None) => (
                                if entry.is_folder { "📁" } else { "📖" },
                                entry.path.file_name().unwrap_or_default().to_string_lossy(),
                            ),
                        };
                        // ダウンロードの進み具合を表紙の下に
                        match status {
                            Some(DownloadStatus::Running(ratio)) => {
                                let bar = egui::Rect::from_min_size(
                                    egui::pos2(image_area.left(), image_area.bottom() - 8.0),
                                    egui::vec2(image_area.width(), 8.0),
                                );
                                painter.rect_filled(bar, 2.0, egui::Color32::from_black_alpha(200));
                                // 大きさがわからなければ、行き来する帯
                                let (start, width) = match ratio {
                                    Some(ratio) => (0.0, ratio.clamp(0.0, 1.0)),
                                    None => {
                                        let t = ui.input(|i| i.time) as f32;
                                        ((t * 0.8).fract() * 0.7, 0.3)
                                    }
                                };
                                painter.rect_filled(
                                    egui::Rect::from_min_size(
                                        bar.min + egui::vec2(bar.width() * start, 0.0),
                                        egui::vec2(bar.width() * width, bar.height()),
                                    ),
                                    2.0,
                                    SELECTED_COLOR,
                                );
                            }
                            Some(DownloadStatus::Queued) => {
                                painter.text(
                                    image_area.center(),
                                    egui::Align2::CENTER_CENTER,
                                    tr("plugin.queued"),
                                    egui::FontId::proportional(14.0),
                                    egui::Color32::WHITE,
                                );
                            }
                            None => {}
                        }
                        let label_rect = egui::Rect::from_min_max(
                            egui::pos2(cell.min.x, cell.max.y - COVER_LABEL_HEIGHT),
                            cell.max,
                        );
                        // 長い名前はマスの幅で2行まで折り返し、残りは「…」で省く
                        // （全部は選んだときに左下に出る）
                        let mut job = egui::text::LayoutJob::single_section(
                            format!("{icon} {name}"),
                            egui::TextFormat::simple(
                                egui::FontId::proportional(13.0),
                                egui::Color32::WHITE,
                            ),
                        );
                        job.wrap = egui::text::TextWrapping {
                            max_width: cell.width() - 8.0,
                            max_rows: COVER_LABEL_ROWS,
                            break_anywhere: true,
                            overflow_character: Some('…'),
                        };
                        job.halign = egui::Align::Center;
                        let galley = painter.layout_job(job);
                        // 中央に揃えた行は、x = 0 を中心に並ぶ
                        let top = label_rect.center().y - galley.size().y * 0.5;
                        painter.with_clip_rect(label_rect.intersect(rect)).galley(
                            egui::pos2(label_rect.center().x, top),
                            galley,
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
        } else if let Some(tab) = tab_to_show {
            self.switch_shelf_tab(tab);
        } else if remove_selected {
            self.remove_selected_from_shelf();
        } else if go_up {
            self.go_to_upper_folder();
        } else if go_back {
            self.go_back_in_shelf();
        } else if refresh && let Some(ShelfSource::Plugin(id, folder)) = self.shelf_source() {
            self.refresh_plugin_list(&(id, folder));
        }
    }
}

/// タブの帯でクリックしたもの
enum TabClick {
    Tab(ShelfTab),
    /// 上のフォルダへのボタン
    Upper,
    /// 戻るボタン
    Back,
    /// プラグインの一覧の更新ボタン
    Refresh,
}

/// タブの帯に置く小さなボタン。`enabled` でなければ灰色にする。描いた場所を返す
fn tab_button(
    ui: &egui::Ui,
    painter: &egui::Painter,
    min: egui::Pos2,
    height: f32,
    label: &str,
    enabled: bool,
) -> egui::Rect {
    let color = egui::Color32::from_gray(if enabled { 220 } else { 110 });
    let galley = painter.layout_no_wrap(label.to_owned(), egui::FontId::proportional(15.0), color);
    let button = egui::Rect::from_min_size(min, egui::vec2(galley.size().x + 20.0, height));
    let hovered = enabled
        && ui
            .ctx()
            .pointer_hover_pos()
            .is_some_and(|pos| button.contains(pos));
    painter.rect_filled(
        button,
        4.0,
        egui::Color32::from_gray(if hovered { 70 } else { 48 }),
    );
    painter.galley(button.center() - galley.size() * 0.5, galley, color);
    button
}

/// タブの帯に出すもの
struct TabStrip<'a> {
    tabs: &'a [(ShelfTab, String)],
    active: ShelfTab,
    can_go_back: bool,
    /// 上のフォルダへのボタンを出すか
    can_go_up: bool,
    /// プラグインのタブなら、一覧を更新中か
    refreshing: Option<bool>,
    /// ダウンロードの失敗
    error: Option<String>,
}

/// 表紙サムネイルの上のタブ。フォルダのタブでは上のフォルダへ、プラグインのタブでは更新の
/// ボタンを並べ、右端に操作の案内（ダウンロードに失敗したらその知らせ）を出す。
/// `click` の位置にあるものを返す
fn draw_shelf_tabs(
    ui: &egui::Ui,
    rect: egui::Rect,
    strip: &TabStrip,
    click: Option<egui::Pos2>,
) -> Option<TabClick> {
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_gray(24));
    let font = egui::FontId::proportional(15.0);
    let mut clicked = None;
    let back = tab_button(
        ui,
        &painter,
        egui::pos2(rect.left() + 8.0, rect.top() + 4.0),
        rect.height() - 8.0,
        tr("shelf.back"),
        strip.can_go_back,
    );
    if strip.can_go_back && click.is_some_and(|pos| back.contains(pos)) {
        clicked = Some(TabClick::Back);
    }
    let mut x = back.right() + 12.0;
    for (tab, label) in strip.tabs {
        let galley = painter.layout_no_wrap(label.clone(), font.clone(), egui::Color32::WHITE);
        let tab_rect = egui::Rect::from_min_size(
            egui::pos2(x, rect.top() + 3.0),
            egui::vec2(galley.size().x + 24.0, rect.height() - 3.0),
        );
        let (fill, text) = if *tab == strip.active {
            (SELECTED_COLOR, egui::Color32::WHITE)
        } else {
            (egui::Color32::from_gray(48), egui::Color32::from_gray(190))
        };
        painter.rect_filled(
            tab_rect,
            egui::CornerRadius {
                nw: 4,
                ne: 4,
                sw: 0,
                se: 0,
            },
            fill,
        );
        painter.galley(tab_rect.center() - galley.size() * 0.5, galley, text);
        if click.is_some_and(|pos| tab_rect.contains(pos)) {
            clicked = Some(TabClick::Tab(*tab));
        }
        x = tab_rect.right() + 4.0;
    }
    let button_at = egui::pos2(x + 12.0, rect.top() + 4.0);
    let mut button_at = button_at;
    if strip.can_go_up {
        let button = tab_button(
            ui,
            &painter,
            button_at,
            rect.height() - 8.0,
            tr("shelf.upper"),
            true,
        );
        if click.is_some_and(|pos| button.contains(pos)) {
            clicked = Some(TabClick::Upper);
        }
        button_at.x = button.right() + 8.0;
    }
    if let Some(refreshing) = strip.refreshing {
        let label = if refreshing {
            tr("plugin.refreshing")
        } else {
            tr("plugin.refresh")
        };
        let button = tab_button(
            ui,
            &painter,
            button_at,
            rect.height() - 8.0,
            label,
            !refreshing,
        );
        if !refreshing && click.is_some_and(|pos| button.contains(pos)) {
            clicked = Some(TabClick::Refresh);
        }
    }
    let (hint, color) = match (&strip.error, strip.active) {
        (Some(error), _) => (error.as_str(), egui::Color32::from_rgb(255, 140, 120)),
        (None, ShelfTab::Folder) => (tr("shelf.hint"), egui::Color32::from_gray(150)),
        (None, ShelfTab::Plugin(_)) => (tr("plugin.hint"), egui::Color32::from_gray(150)),
        (None, _) => (tr("shelf.hint_remove"), egui::Color32::from_gray(150)),
    };
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        hint,
        egui::FontId::proportional(13.0),
        color,
    );
    clicked
}

/// フォルダの絵の色
const FOLDER_COLOR: egui::Color32 = egui::Color32::from_rgb(0xc9, 0x9a, 0x3c);
const FOLDER_TAB_COLOR: egui::Color32 = egui::Color32::from_rgb(0xa8, 0x7c, 0x2a);
/// 本の絵の色
const BOOK_COLOR: egui::Color32 = egui::Color32::from_rgb(0x3d, 0x4a, 0x63);
const BOOK_SPINE_COLOR: egui::Color32 = egui::Color32::from_rgb(0x2a, 0x34, 0x48);

/// フォルダの形を `rect` いっぱいに描く（つまみと本体）
fn paint_folder(painter: &egui::Painter, rect: egui::Rect) {
    let tab = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width() * 0.42, rect.height() * 0.2),
    );
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y + rect.height() * 0.12),
        rect.max,
    );
    let rounding = rect.height() * 0.08;
    painter.rect_filled(tab, rounding, FOLDER_TAB_COLOR);
    painter.rect_filled(body, rounding, FOLDER_COLOR);
}

/// 表紙の無い項目の絵。フォルダはフォルダの形、本は拡張子を書いた本の形
fn paint_placeholder(painter: &egui::Painter, area: egui::Rect, entry: &CoverEntry) {
    if entry.is_folder {
        let width = (area.width() * 0.7).min(area.height() * 0.7 / 0.78);
        let rect = egui::Rect::from_center_size(area.center(), egui::vec2(width, width * 0.78));
        paint_folder(painter, rect);
        return;
    }
    let height = (area.height() * 0.78).min(area.width() * 0.78 / 0.72);
    let rect = egui::Rect::from_center_size(area.center(), egui::vec2(height * 0.72, height));
    painter.rect_filled(rect, height * 0.03, BOOK_COLOR);
    let spine = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * 0.1, rect.height()));
    painter.rect_filled(spine, height * 0.03, BOOK_SPINE_COLOR);
    let extension = entry
        .path
        .extension()
        .map(|ext| ext.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    if !extension.is_empty() {
        painter.text(
            rect.center() + egui::vec2(rect.width() * 0.05, 0.0),
            egui::Align2::CENTER_CENTER,
            extension,
            egui::FontId::proportional((rect.width() * 0.22).clamp(10.0, 28.0)),
            egui::Color32::from_gray(230),
        );
    }
}

/// 表紙の右下に付ける、小さなフォルダの印
fn paint_folder_badge(painter: &egui::Painter, image: egui::Rect) {
    let size = egui::vec2(30.0, 24.0).min(image.size() * 0.4);
    let backing = egui::Rect::from_min_size(image.max - size - egui::vec2(4.0, 4.0), size);
    painter.rect_filled(backing, 4.0, egui::Color32::from_black_alpha(170));
    paint_folder(painter, backing.shrink2(size * egui::vec2(0.15, 0.18)));
}
