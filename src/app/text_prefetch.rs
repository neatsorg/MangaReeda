//! 本の文字の下ごしらえ。横断検索で探せるよう、PDF の文字を裏で取り出してためておく。
//! - 本を開いたとき: その PDF の文字を取り出す（1ページごとに間を空け、読む邪魔をしない）
//! - 操作していないとき: 本を読んでいないあいだに1分操作が無ければ、まだためていない PDF を1冊ずつ取り出す。
//!   キーやマウスを触ったらすぐ止め、また1分たったら続きから（途中の1冊は最初から）
//!
//! OCR はしない（時間がかかるので、利用者が頼んだときだけ）。

use super::MangaReader;
use crate::config::SourceMode;
use crate::engine::book_text::prepare_pdf_text;
use crate::engine::cross_search::scan_books;
use crate::engine::history::normalize;
use crate::engine::settings_store;
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 操作が無くなってから、下ごしらえを始めるまで
const IDLE_AFTER: Duration = Duration::from_secs(60);
/// 操作していないときに下ごしらえする本の数の上限（ためておきの上限と同じ）
const PREPARE_LIMIT: usize = 1000;

pub(super) struct TextPrefetch {
    /// 開いている本の下ごしらえ（本, 止める印）
    book_job: Option<(PathBuf, Arc<AtomicBool>)>,
    /// 操作していないときの下ごしらえの、止める印
    idle_job: Option<Arc<AtomicBool>>,
    /// 下ごしらえする本（初めて始めるときに集める）と、次に取り出す本の番号
    books: Arc<Mutex<Option<Vec<PathBuf>>>>,
    next: Arc<AtomicUsize>,
    /// 全部済んだ
    finished: Arc<AtomicBool>,
    last_input: Instant,
}

impl Default for TextPrefetch {
    fn default() -> Self {
        Self {
            book_job: None,
            idle_job: None,
            books: Arc::default(),
            next: Arc::default(),
            finished: Arc::default(),
            last_input: Instant::now(),
        }
    }
}

impl Drop for TextPrefetch {
    fn drop(&mut self) {
        self.stop_book_job();
        self.stop_idle_job();
    }
}

impl TextPrefetch {
    fn stop_book_job(&mut self) {
        if let Some((_, cancel)) = self.book_job.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn stop_idle_job(&mut self) {
        if let Some(stop) = self.idle_job.take() {
            stop.store(true, Ordering::Relaxed);
        }
    }

    /// `book` の下ごしらえをしていれば止める（本の中の検索が取り出すので、同じことを二度しない）。
    /// 止めた印は残し、同じ本のあいだはやり直さない
    pub(super) fn leave_book(&mut self, book: &Path) {
        if let Some((working, cancel)) = &self.book_job
            && working == book
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    /// 操作していないときの下ごしらえをしているか
    #[cfg(test)]
    pub(super) fn idle_running(&self) -> bool {
        self.idle_job.is_some()
    }

    /// 1分前から操作していないことにする
    #[cfg(test)]
    pub(super) fn pretend_idle(&mut self) {
        self.last_input = Instant::now() - IDLE_AFTER;
    }
}

/// PDF か
fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pdf"))
}

impl MangaReader {
    /// 本の文字の下ごしらえを、始めたり止めたりする。毎フレーム呼ぶ
    pub(super) fn prepare_book_texts(&mut self, ctx: &egui::Context) {
        let Some(pdfium) = crate::engine::pdf::pdfium() else {
            return;
        };
        // 開いている PDF の文字を取り出す（本を変えたら、前の本の分は止める）
        let current = self
            .zip_path
            .clone()
            .filter(|_| self.source_mode == SourceMode::Pdf);
        let working = self.text_prefetch.book_job.as_ref().map(|(book, _)| book);
        if current.as_ref() != working {
            self.text_prefetch.stop_book_job();
            if let Some(book) = current {
                let cancel = Arc::new(AtomicBool::new(false));
                let stop = cancel.clone();
                let target = book.clone();
                std::thread::spawn(move || {
                    prepare_pdf_text(pdfium, &target, &settings_store::cache_dir(), &stop);
                });
                self.text_prefetch.book_job = Some((book, cancel));
            }
        }

        // 操作していないとき（本を読んでいない、OCR もしていない）だけ、残りの本を取り出す
        let touched = ctx.input(|i| !i.events.is_empty() || i.pointer.delta() != egui::Vec2::ZERO);
        if touched {
            self.text_prefetch.last_input = Instant::now();
        }
        let reading = self.thumbnails.is_none() && self.zip_path.is_some();
        let idle_for = self.text_prefetch.last_input.elapsed();
        let finished = self.text_prefetch.finished.load(Ordering::Relaxed);
        let may_run = !reading && self.book_ocr.is_none() && !finished;
        if !(may_run && idle_for >= IDLE_AFTER) {
            self.text_prefetch.stop_idle_job();
            if may_run {
                // 何も起きなくても、1分たったら始められるように起こしてもらう
                ctx.request_repaint_after(IDLE_AFTER.saturating_sub(idle_for));
            }
            return;
        }
        if self.text_prefetch.idle_job.is_none() {
            self.start_idle_prefetch(pdfium);
        }
    }

    /// 操作していないときの下ごしらえを始める
    fn start_idle_prefetch(&mut self, pdfium: crate::engine::pdf::SharedPdfium) {
        let stop = Arc::new(AtomicBool::new(false));
        // 下ごしらえする本: 履歴とお気に入りを先に、次にダウンロード先と設定で足したフォルダの下
        let mut known: Vec<PathBuf> = self
            .history
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .chain(
                self.history
                    .favorites
                    .iter()
                    .map(|entry| entry.path.clone()),
            )
            .collect();
        known.retain(|path| is_pdf(path));
        let mut roots = vec![self.library_dir()];
        roots.extend(
            self.config
                .search_folders
                .iter()
                .map(|folder| folder.trim())
                .filter(|folder| !folder.is_empty())
                .map(PathBuf::from),
        );
        let (books, next, finished, halt) = (
            self.text_prefetch.books.clone(),
            self.text_prefetch.next.clone(),
            self.text_prefetch.finished.clone(),
            stop.clone(),
        );
        std::thread::spawn(move || {
            let stopped = || halt.load(Ordering::Relaxed);
            let list = {
                let mut books = books
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if books.is_none() {
                    let mut seen = HashSet::new();
                    let found: Vec<PathBuf> = known
                        .into_iter()
                        .chain(
                            scan_books(&roots, &stopped)
                                .into_iter()
                                .filter(|path| is_pdf(path)),
                        )
                        .filter(|path| seen.insert(normalize(path)))
                        .take(PREPARE_LIMIT)
                        .collect();
                    // たどっている途中で止められたら、集めた本を使わない（次に初めから集める）
                    if stopped() {
                        return;
                    }
                    *books = Some(found);
                }
                books.clone().unwrap_or_default()
            };
            let cache_dir = settings_store::cache_dir();
            loop {
                let Some(book) = list.get(next.load(Ordering::Relaxed)) else {
                    finished.store(true, Ordering::Relaxed);
                    return;
                };
                if stopped() {
                    return;
                }
                // 止められたら、その本は次にやり直す
                if book.is_file() && !prepare_pdf_text(pdfium, book, &cache_dir, &halt) {
                    return;
                }
                next.fetch_add(1, Ordering::Relaxed);
            }
        });
        self.text_prefetch.idle_job = Some(stop);
    }
}
