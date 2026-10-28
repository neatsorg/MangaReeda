//! 本の中の検索。PDF の透明テキストを探す。文字の層の無い本は、OCR のプラグインで読んでから探す。
//! 本の文字は別のスレッドで取り出し（読み）、ためておく。語を打つたびに探し直し、選ぶとそのページへ移る。
//! OCR は検索の窓とは別に持つ。窓を閉じても、別の本に移っても、「やめる」を押すまで続ける。

use super::MangaReader;
use crate::config::SourceMode;
use crate::engine::book_text::{
    Hit, LoadedText, PageSource, SearchIndex, TextProgress, spawn_book_ocr, spawn_book_text,
};
use crate::engine::plugin::{PluginError, PluginManifest};
use crate::engine::settings_store;
use crate::localize::tr;
use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

/// 出す件数の上限
const HIT_LIMIT: usize = 1000;

pub(super) struct BookSearch {
    /// 探している本
    book: PathBuf,
    pub(super) query: String,
    /// 次のフレームで入力欄に移る
    focus: bool,
    index: Option<SearchIndex>,
    loading: Option<Receiver<Option<LoadedText>>>,
    /// この本の OCR が失敗した理由
    ocr_error: Option<String>,
    /// 取り出しの進み具合
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
    /// 文字の層が無い（探せない）
    no_text: bool,
    /// 文字を読めなかったページの数
    failed: usize,
    pub(super) hits: Vec<Hit>,
    /// `hits` を探した語
    searched: String,
    /// 選んでいる見つかったところ
    selected: Option<usize>,
}

impl Drop for BookSearch {
    fn drop(&mut self) {
        // 閉じたら、取り出しを止める
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// OCR のプラグインで本を読んでいるところ。一度に1冊だけ
pub(super) struct BookOcr {
    /// 読んでいる本
    book: PathBuf,
    rx: Receiver<Result<LoadedText, PluginError>>,
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
}

impl BookOcr {
    /// 止めて、プラグインを止め終え作業用の画像を消し終えるまで（長くて `limit`）待つ。
    /// MangaReeda を閉じるときに呼ぶ（待たずに終わると、プラグインと作業用の画像が残る）
    pub(super) fn stop_and_wait(&self, limit: std::time::Duration) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.rx.recv_timeout(limit);
    }
}

impl Drop for BookOcr {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl BookSearch {
    /// OCR で読み終えた文字に入れ替える
    fn take_ocr(&mut self, loaded: LoadedText) {
        self.loading = None;
        self.no_text = false;
        self.ocr_error = None;
        self.failed = loaded.failed;
        self.index = Some(loaded.index);
        self.searched.clear();
    }

    /// 取り出しの知らせを受け取る。取り出しているあいだは true
    fn poll(&mut self) -> bool {
        let Some(rx) = &self.loading else {
            return false;
        };
        match rx.try_recv() {
            Ok(loaded) => {
                self.loading = None;
                match loaded.filter(|loaded| !loaded.empty) {
                    Some(loaded) => {
                        self.failed = loaded.failed;
                        self.index = Some(loaded.index);
                    }
                    None => self.no_text = true,
                }
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => {
                self.loading = None;
                self.no_text = true;
                false
            }
        }
    }

    /// 語が変わっていれば探し直す
    fn refresh(&mut self) {
        let Some(index) = &self.index else {
            return;
        };
        if self.query != self.searched {
            self.hits = index.search(&self.query, HIT_LIMIT);
            self.searched = self.query.clone();
            self.selected = None;
        }
    }

    /// 窓に出す一言。`ocr` は、この本を OCR で読んでいるときの進み具合
    fn status(&self, ocr: Option<&TextProgress>) -> String {
        let progress = |key: &str, progress: &TextProgress| {
            tr(key)
                .replace("{done}", &progress.done.load(Ordering::Relaxed).to_string())
                .replace(
                    "{total}",
                    &progress.total.load(Ordering::Relaxed).to_string(),
                )
        };
        if let Some(ocr) = ocr {
            return progress("search.ocr_reading", ocr);
        }
        if let Some(error) = &self.ocr_error {
            return format!("{}: {error}", tr("search.ocr_failed"));
        }
        if self.no_text {
            return tr("search.no_text").to_owned();
        }
        if self.loading.is_some() {
            return progress("search.loading", &self.progress);
        }
        let failed = if self.failed > 0 {
            format!(
                " {}",
                tr("search.failed").replace("{n}", &self.failed.to_string())
            )
        } else {
            String::new()
        };
        if self.query.trim().is_empty() {
            return failed.trim_start().to_owned();
        }
        let count = match self.hits.len() {
            0 => tr("search.none").to_owned(),
            HIT_LIMIT => tr("search.count_more").replace("{n}", &HIT_LIMIT.to_string()),
            n => tr("search.count").replace("{n}", &n.to_string()),
        };
        count + &failed
    }
}

impl MangaReader {
    /// 本の中の検索を開く（開いていれば入力欄に移る）
    pub(super) fn open_book_search(&mut self) {
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        if let Some(search) = self.book_search.as_mut()
            && search.book == book
        {
            search.focus = true;
            return;
        }
        let progress = Arc::new(TextProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        // PDF は透明テキストを、ほかの本は前に OCR してためた文字を読む
        let pdfium = crate::engine::pdf::pdfium().filter(|_| self.source_mode == SourceMode::Pdf);
        let pages = PageSource {
            mode: self.source_mode,
            root: book.clone(),
            names: self.image_files.clone(),
        };
        let loading = spawn_book_text(
            pdfium,
            book.clone(),
            pages,
            settings_store::cache_dir(),
            progress.clone(),
            cancel.clone(),
        );
        self.book_search = Some(BookSearch {
            book,
            query: String::new(),
            focus: true,
            index: None,
            no_text: false,
            failed: 0,
            loading: Some(loading),
            ocr_error: None,
            progress,
            cancel,
            hits: Vec::new(),
            searched: String::new(),
            selected: None,
        });
    }

    /// 使う OCR のプラグイン。設定で選んだもの、無ければ最初に見つかったもの
    pub(super) fn ocr_engine(&self) -> Option<&PluginManifest> {
        let engines = &self.plugins.ocr_engines;
        engines
            .iter()
            .find(|engine| engine.id == self.config.ocr_engine)
            .or_else(|| engines.first())
    }

    /// OCR で読んでいる本があるか
    pub(super) fn book_ocr_running(&self) -> bool {
        self.book_ocr.is_some()
    }

    /// 今の本を OCR のプラグインで読み始める。ほかの本を読んでいるあいだは始めない
    pub(super) fn start_book_ocr(&mut self) {
        if self.book_ocr.is_some() {
            return;
        }
        let Some(engine) = self.ocr_engine().cloned() else {
            return;
        };
        let Some(book) = self.zip_path.clone() else {
            return;
        };
        let credentials = self.plugins.credentials_for(&engine);
        let source = PageSource {
            mode: self.source_mode,
            root: book.clone(),
            names: self.image_files.clone(),
        };
        let pdfium = crate::engine::pdf::pdfium();
        if let Some(search) = self.book_search.as_mut() {
            search.ocr_error = None;
        }
        let progress = Arc::new(TextProgress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_book_ocr(
            engine,
            credentials,
            book.clone(),
            source,
            pdfium,
            settings_store::cache_dir(),
            progress.clone(),
            cancel.clone(),
        );
        self.book_ocr = Some(BookOcr {
            book,
            rx,
            progress,
            cancel,
        });
    }

    /// OCR の知らせを受け取る。検索の窓を閉じていても毎フレーム呼ぶ
    pub(super) fn poll_book_ocr(&mut self, ctx: &egui::Context) {
        let Some(ocr) = &self.book_ocr else {
            return;
        };
        let result = match ocr.rx.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Err(TryRecvError::Disconnected) => None,
        };
        let Some(ocr) = self.book_ocr.take() else {
            return;
        };
        let name = ocr
            .book
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 検索の窓でこの本を見ていれば、窓に出す。見ていなければ、画面に一言出す
        let search = self
            .book_search
            .as_mut()
            .filter(|search| search.book == ocr.book);
        match (result, search) {
            (Some(Ok(loaded)), Some(search)) => search.take_ocr(loaded),
            // 読んだ文字はためておきに入っているので、次に検索の窓を開けば使われる
            (Some(Ok(loaded)), None) => {
                let mut message = tr("search.ocr_done").replace("{book}", &name);
                if loaded.failed > 0 {
                    message += &tr("search.failed").replace("{n}", &loaded.failed.to_string());
                }
                self.show_fading_error(&message);
            }
            (Some(Err(error)), search) => {
                // 理由が空なのは、利用者がやめたとき
                let message = error.message().trim();
                if message.is_empty() {
                    return;
                }
                match search {
                    Some(search) => search.ocr_error = Some(message.to_owned()),
                    None => self.show_fading_error(&format!(
                        "{} ({name}): {message}",
                        tr("search.ocr_failed")
                    )),
                }
            }
            (None, _) => {}
        }
    }

    /// 検索の窓が開いていれば、Esc で閉じる。Esc はここで使い切り、ほかのキー操作には渡さない
    /// （入力欄から離れていても、終了などに割り当てた Esc が働かないように）
    pub(super) fn close_book_search_on_escape(&mut self, ctx: &egui::Context) {
        if self.book_search.is_some()
            && self.thumbnails.is_none()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.book_search = None;
        }
    }

    /// 検索の窓を描く。毎フレーム呼ぶ
    pub(super) fn render_book_search(&mut self, ctx: &egui::Context) {
        let Some(search) = self.book_search.as_mut() else {
            return;
        };
        // 別の本に移ったら閉じる
        if Some(&search.book) != self.zip_path.as_ref() {
            self.book_search = None;
            return;
        }
        if search.poll() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        search.refresh();
        let mut open = true;
        let mut close = false;
        let mut go = None;
        let mut start_ocr = false;
        let mut stop_ocr = false;
        let engine_name = self.ocr_engine().map(|engine| short_name(&engine.name));
        // この本を OCR で読んでいれば、その進み具合。ほかの本を読んでいるか
        let (ocr, other_ocr) = match &self.book_ocr {
            Some(ocr) if Some(&ocr.book) == self.zip_path.as_ref() => {
                (Some(ocr.progress.clone()), false)
            }
            Some(_) => (None, true),
            None => (None, false),
        };
        let Some(search) = self.book_search.as_mut() else {
            return;
        };
        egui::Window::new(tr("search.title"))
            .open(&mut open)
            .default_pos(ctx.content_rect().right_top() + egui::vec2(-400.0, 60.0))
            .default_width(380.0)
            .default_height(420.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut search.query)
                        .hint_text(tr("search.hint"))
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut search.focus) {
                    field.request_focus();
                }
                let (enter, escape) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Enter),
                        i.key_pressed(egui::Key::Escape),
                    )
                });
                // 入力欄で Enter: 次に見つかったところへ。Esc: 閉じる
                if field.lost_focus() && enter && !search.hits.is_empty() {
                    let next = search
                        .selected
                        .map_or(0, |selected| (selected + 1) % search.hits.len());
                    go = Some(next);
                    field.request_focus();
                }
                if escape && (field.has_focus() || field.lost_focus()) {
                    close = true;
                }
                ui.label(egui::RichText::new(search.status(ocr.as_deref())).small());
                // 文字の層が無い本は、OCR で読める。読んでいるあいだは、やめられる
                // （読み直しはまれで、前の結果を上書きするので、ツールのメニューから確かめてから行う）
                if ocr.is_some() {
                    if ui.button(tr("search.ocr_stop")).clicked() {
                        stop_ocr = true;
                    }
                } else if search.no_text && search.index.is_none() {
                    match &engine_name {
                        Some(_) if other_ocr => {
                            ui.label(egui::RichText::new(tr("search.ocr_busy")).small());
                        }
                        Some(name) => {
                            if ui
                                .button(tr("search.ocr_start").replace("{engine}", name))
                                .clicked()
                            {
                                start_ocr = true;
                            }
                        }
                        None => {
                            ui.label(egui::RichText::new(tr("search.no_ocr")).small());
                        }
                    }
                }
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (index, hit) in search.hits.iter().enumerate() {
                        let label = format!("p.{}  {}", hit.page + 1, hit.snippet);
                        let response = ui.selectable_label(search.selected == Some(index), label);
                        if go == Some(index) {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() {
                            go = Some(index);
                        }
                    }
                });
            });
        if stop_ocr && let Some(ocr) = &self.book_ocr {
            ocr.cancel.store(true, Ordering::Relaxed);
        }
        if !open || close {
            self.book_search = None;
            return;
        }
        if start_ocr {
            self.start_book_ocr();
            return;
        }
        let Some(index) = go else {
            return;
        };
        let page = search.hits.get(index).map(|hit| hit.page);
        search.selected = Some(index);
        if let Some(page) = page {
            self.go_to_page(page, ctx);
        }
    }
}

/// 窓に出すエンジンの名前。長すぎる名前は切る
pub(super) fn short_name(name: &str) -> String {
    const MAX: usize = 40;
    if name.chars().count() <= MAX {
        name.to_owned()
    } else {
        name.chars().take(MAX - 1).collect::<String>() + "…"
    }
}
