//! 横断検索の窓。手元の本の名前と、ためておいた本の文字をまとめて探し、選ぶとその本（のそのページ）を開く。
//! 探すのはエンジンの別スレッドで行い、ここは知らせを受け取って並べるだけ（画面を止めない）。

use super::MangaReader;
use crate::engine::cross_search::{
    CrossSearch, Message, NameFrom, NameHit, TEXT_LIMIT, TEXT_MIN_CHARS, TextHit, book_name,
};
use crate::engine::settings_store;
use crate::localize::tr;
use eframe::egui;
use std::path::{Path, PathBuf};

pub(super) struct CrossSearchWindow {
    search: CrossSearch,
    pub(super) query: String,
    /// 探している語と、その番号
    searched: String,
    generation: u64,
    /// フォルダをたどり終えたら、名前で探す本の数
    scanned: Option<usize>,
    pub(super) names: Vec<NameHit>,
    pub(super) texts: Vec<TextHit>,
    /// 本文を探した冊の数
    progress: (usize, usize),
    done: bool,
    /// 選んでいる結果（名前のあとに本文を続けて数える）
    selected: Option<usize>,
    /// 次のフレームで入力欄に移る
    focus: bool,
    /// 入力欄にいる（前のフレーム）。表紙サムネイルにキーを渡さないのに使う
    pub(super) typing: bool,
}

impl CrossSearchWindow {
    fn count(&self) -> usize {
        self.names.len() + self.texts.len()
    }

    /// 知らせを受け取る。探しているあいだは true
    fn poll(&mut self) -> bool {
        while let Ok((generation, message)) = self.search.messages.try_recv() {
            if let Message::Scanned(count) = message {
                self.scanned = Some(count);
                continue;
            }
            if generation != self.generation {
                continue;
            }
            match message {
                Message::Names(hits) => self.names = hits,
                Message::Text(hits) => self.texts.extend(hits),
                Message::Progress { done, total } => self.progress = (done, total),
                Message::Done => self.done = true,
                Message::Scanned(_) => {}
            }
        }
        self.scanned.is_none() || !self.done
    }

    /// 語が変わっていれば探し直す
    fn refresh(&mut self) {
        if self.query == self.searched {
            return;
        }
        self.searched = self.query.clone();
        self.generation = self.search.search(&self.query);
        self.names.clear();
        self.texts.clear();
        self.progress = (0, 0);
        self.selected = None;
        self.done = self.query.trim().is_empty();
    }

    fn status(&self) -> String {
        let Some(scanned) = self.scanned else {
            return tr("cross.scanning").to_owned();
        };
        if self.query.trim().is_empty() {
            return tr("cross.ready").replace("{n}", &scanned.to_string());
        }
        let mut status = tr("cross.count")
            .replace("{names}", &self.names.len().to_string())
            .replace("{texts}", &self.texts.len().to_string());
        if self.texts.len() >= TEXT_LIMIT {
            status += tr("cross.more");
        }
        let short = self.query.chars().filter(|c| !c.is_whitespace()).count() < TEXT_MIN_CHARS;
        if short {
            status += tr("cross.short");
        } else if !self.done {
            status += &tr("cross.progress")
                .replace("{done}", &self.progress.0.to_string())
                .replace("{total}", &self.progress.1.to_string());
        }
        status
    }
}

/// 結果に出す、本の場所（親のフォルダ）
fn folder_of(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl MangaReader {
    /// 横断検索の窓を開く（開いていれば入力欄に移る）
    pub(super) fn open_cross_search(&mut self) {
        if let Some(window) = self.cross_search.as_mut() {
            window.focus = true;
            return;
        }
        let mut known: Vec<NameHit> = self
            .history
            .favorites
            .iter()
            .map(|entry| NameHit {
                path: entry.path.clone(),
                from: NameFrom::Favorite,
            })
            .collect();
        known.extend(self.history.entries.iter().map(|entry| NameHit {
            path: entry.path.clone(),
            from: NameFrom::History,
        }));
        let mut roots = vec![self.library_dir()];
        roots.extend(
            self.config
                .search_folders
                .iter()
                .map(|folder| folder.trim())
                .filter(|folder| !folder.is_empty())
                .map(PathBuf::from),
        );
        self.cross_search = Some(CrossSearchWindow {
            search: CrossSearch::start(known, roots, settings_store::cache_dir()),
            query: String::new(),
            searched: String::new(),
            generation: 0,
            scanned: None,
            names: Vec::new(),
            texts: Vec::new(),
            progress: (0, 0),
            done: true,
            selected: None,
            focus: true,
            typing: false,
        });
    }

    /// 横断検索の窓が開いていれば、Esc で閉じる。Esc はここで使い切る（終了などに割り当てた Esc が働かないように）
    pub(super) fn close_cross_search_on_escape(&mut self, ctx: &egui::Context) {
        if self.cross_search.is_some()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.cross_search = None;
        }
    }

    /// 結果を開く。本文の結果なら、そのページへ移る
    fn open_cross_result(&mut self, index: usize, ctx: &egui::Context) {
        let Some(window) = self.cross_search.as_ref() else {
            return;
        };
        let (path, page) = match index.checked_sub(window.names.len()) {
            None => (window.names[index].path.clone(), None),
            Some(text) => match window.texts.get(text) {
                Some(hit) => (hit.book.clone(), Some(hit.page)),
                None => return,
            },
        };
        if !path.exists() {
            self.show_fading_error(tr("cross.missing"));
            return;
        }
        self.thumbnails = None;
        if self.zip_path.as_deref() != Some(path.as_path()) {
            self.open_source(path, ctx);
        }
        if let Some(page) = page {
            self.go_to_page(page, ctx);
        }
    }

    /// 横断検索の窓を描く。毎フレーム呼ぶ
    pub(super) fn render_cross_search(&mut self, ctx: &egui::Context) {
        let Some(window) = self.cross_search.as_mut() else {
            return;
        };
        if window.poll() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        window.refresh();
        let count = window.count();
        // 入力欄にいるあいだの ↑↓ と Enter は、ここで使い切る（入力欄や背後の一覧に渡さない）
        let (up, down, enter) = if window.typing {
            ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                )
            })
        } else {
            (false, false, false)
        };
        if count > 0 {
            if down {
                window.selected = Some(window.selected.map_or(0, |s| (s + 1).min(count - 1)));
            }
            if up {
                window.selected = Some(window.selected.map_or(0, |s| s.saturating_sub(1)));
            }
        }
        let mut chosen = (enter && count > 0).then(|| window.selected.unwrap_or(0));
        let scroll_to = (up || down).then_some(window.selected).flatten();
        let mut open = true;
        let names_label = tr("cross.names");
        let texts_label = tr("cross.texts");
        egui::Window::new(tr("cross.title"))
            .open(&mut open)
            .default_pos(ctx.content_rect().right_top() + egui::vec2(-520.0, 60.0))
            .default_width(500.0)
            .default_height(520.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(&mut window.query)
                        .hint_text(tr("cross.hint"))
                        .desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut window.focus) {
                    field.request_focus();
                }
                window.typing = field.has_focus();
                ui.label(egui::RichText::new(window.status()).small());
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !window.names.is_empty() {
                            ui.label(egui::RichText::new(names_label).strong());
                        }
                        for (index, hit) in window.names.iter().enumerate() {
                            let tag = match hit.from {
                                NameFrom::History => tr("cross.from_history"),
                                NameFrom::Favorite => tr("cross.from_favorite"),
                                NameFrom::Folder => "",
                            };
                            let response = ui
                                .selectable_label(
                                    window.selected == Some(index),
                                    format!("{tag}{}", book_name(&hit.path)),
                                )
                                .on_hover_text(folder_of(&hit.path));
                            if scroll_to == Some(index) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                chosen = Some(index);
                            }
                        }
                        if !window.texts.is_empty() {
                            ui.add_space(6.0);
                            ui.label(egui::RichText::new(texts_label).strong());
                        }
                        let offset = window.names.len();
                        for (index, hit) in window.texts.iter().enumerate() {
                            let index = offset + index;
                            let label = format!(
                                "{}  p.{}  {}",
                                book_name(&hit.book),
                                hit.page + 1,
                                hit.snippet
                            );
                            let response = ui
                                .selectable_label(window.selected == Some(index), label)
                                .on_hover_text(folder_of(&hit.book));
                            if scroll_to == Some(index) {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                chosen = Some(index);
                            }
                        }
                    });
            });
        if !open {
            self.cross_search = None;
            return;
        }
        if let Some(index) = chosen {
            if let Some(window) = self.cross_search.as_mut() {
                window.selected = Some(index);
            }
            self.open_cross_result(index, ctx);
        }
    }

    /// 設定画面の、横断検索でたどるフォルダ（1行に1つ）
    pub(super) fn render_cross_search_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.label(tr("settings.search_folders"));
        let text = self
            .search_folders_text
            .get_or_insert_with(|| self.config.search_folders.join("\n"));
        let response = ui.add(
            egui::TextEdit::multiline(text)
                .desired_rows(2)
                .desired_width(f32::INFINITY),
        );
        if response.lost_focus() {
            let folders: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_owned)
                .collect();
            self.search_folders_text = None;
            if folders != self.config.search_folders {
                self.config.search_folders = folders;
                self.save_settings();
                // 下ごしらえする本も、新しいフォルダで集め直す
                self.text_prefetch.rescan();
            }
        }
    }
}
