//! ブックリストのプラグインを、表紙サムネイルの画面と設定画面につなぐ。
//! 一覧は手元にためておき、更新は利用者が頼んだときだけ行う（サイトに負担をかけないように）。
//! ダウンロードは1冊ずつ順に行う。

use super::MangaReader;
use super::thumbnails::ShelfSource;
use crate::engine::credentials::{Credentials, Stored};
use crate::engine::plugin::{
    DownloadEvent, ListCache, PluginError, PluginManifest, RemoteBook, local_path, spawn_download,
    spawn_list,
};
use crate::engine::settings_store;
use crate::engine::thumbnail::{CoverEntry, RemoteRef};
use crate::localize::tr;
use eframe::egui;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

/// プラグインごとの一覧
#[derive(Default)]
pub(super) struct PluginShelf {
    pub(super) books: Vec<RemoteBook>,
    /// 一覧を取った日時（UNIX 秒）。0 ならまだ取っていない
    pub(super) fetched_at: u64,
    loading: Option<Receiver<Result<Vec<RemoteBook>, PluginError>>>,
    pub(super) error: Option<PluginError>,
}

impl PluginShelf {
    pub(super) fn is_loading(&self) -> bool {
        self.loading.is_some()
    }
}

/// ダウンロードを待っている本
struct PendingDownload {
    plugin: String,
    book: RemoteBook,
    dest: PathBuf,
}

/// ダウンロード中の本
struct ActiveDownload {
    pending: PendingDownload,
    rx: Receiver<DownloadEvent>,
    received: u64,
    total: Option<u64>,
}

/// 表紙サムネイルに出す、本のダウンロードの状態
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum DownloadStatus {
    Queued,
    /// 受け取った割合（大きさがわからなければ None）
    Running(Option<f32>),
}

pub(super) struct PluginHost {
    pub(super) manifests: Vec<PluginManifest>,
    pub(super) shelves: HashMap<String, PluginShelf>,
    credentials: Credentials,
    queue: VecDeque<PendingDownload>,
    active: Option<ActiveDownload>,
    /// 一覧と表紙を置くフォルダ
    cache_dir: PathBuf,
    /// 設定画面で入力中の値。(プラグイン, キー) ごと
    inputs: HashMap<(String, String), String>,
    /// 保存した場所。キーリングに毎フレーム問い合わせないよう覚えておく
    stored: HashMap<(String, String), Option<Stored>>,
    /// 最後に起きたダウンロードの失敗
    pub(super) download_error: Option<String>,
}

impl PluginHost {
    /// 置いてあるプラグインを探す
    pub(super) fn discover() -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let manifests =
            crate::engine::plugin::discover(&settings_store::plugins_dir(), exe_dir.as_deref());
        Self::with(
            manifests,
            Credentials::default(),
            settings_store::cache_dir(),
        )
    }

    pub(super) fn with(
        manifests: Vec<PluginManifest>,
        credentials: Credentials,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            manifests,
            shelves: HashMap::new(),
            credentials,
            queue: VecDeque::new(),
            active: None,
            cache_dir,
            inputs: HashMap::new(),
            stored: HashMap::new(),
            download_error: None,
        }
    }

    pub(super) fn manifest(&self, id: &str) -> Option<&PluginManifest> {
        self.manifests.iter().find(|m| m.id == id)
    }

    fn list_cache_path(&self, id: &str) -> PathBuf {
        self.cache_dir.join("plugins").join(format!("{id}.json"))
    }

    /// 表紙の画像を置くフォルダ
    pub(super) fn cover_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("covers")
    }

    fn stored(&mut self, plugin: &str, key: &str) -> Option<Stored> {
        let index = (plugin.to_owned(), key.to_owned());
        if let Some(stored) = self.stored.get(&index) {
            return *stored;
        }
        let stored = self.credentials.stored(plugin, key);
        self.stored.insert(index, stored);
        stored
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl MangaReader {
    /// プラグインからダウンロードした本を置くフォルダ
    fn library_dir(&self) -> PathBuf {
        if self.config.library_dir.trim().is_empty() {
            settings_store::default_library_dir()
        } else {
            PathBuf::from(self.config.library_dir.trim())
        }
    }

    /// プラグインの一覧を表紙サムネイルに並べる形にする
    pub(super) fn plugin_entries(&self, id: &str) -> Vec<CoverEntry> {
        let Some(shelf) = self.plugins.shelves.get(id) else {
            return Vec::new();
        };
        let library = self.library_dir();
        shelf
            .books
            .iter()
            .map(|book| CoverEntry {
                path: local_path(&library, id, &book.file_name),
                is_folder: false,
                loose_pages: None,
                remote: Some(RemoteRef {
                    plugin: id.to_owned(),
                    id: book.id.clone(),
                    title: book.title.clone(),
                    cover_url: book.cover_url.clone(),
                }),
            })
            .collect()
    }

    /// 一覧が無ければ、手元にためたものを読む。それも無ければ取りに行く
    pub(super) fn ensure_plugin_list(&mut self, id: &str) {
        if self.plugins.shelves.contains_key(id) {
            return;
        }
        let cache = ListCache::load(&self.plugins.list_cache_path(id));
        let fetched = cache.is_some();
        let shelf = cache
            .map(|cache| PluginShelf {
                books: cache.books,
                fetched_at: cache.fetched_at,
                ..PluginShelf::default()
            })
            .unwrap_or_default();
        self.plugins.shelves.insert(id.to_owned(), shelf);
        if !fetched {
            self.refresh_plugin_list(id);
        }
    }

    /// 一覧を取り直す（利用者が頼んだとき）
    pub(super) fn refresh_plugin_list(&mut self, id: &str) {
        let Some(manifest) = self.plugins.manifest(id).cloned() else {
            return;
        };
        let shelf = self.plugins.shelves.entry(id.to_owned()).or_default();
        if shelf.loading.is_some() {
            return;
        }
        let credentials = self.plugins.credentials.for_plugin(id, &manifest.keys());
        let shelf = self.plugins.shelves.entry(id.to_owned()).or_default();
        shelf.error = None;
        shelf.loading = Some(spawn_list(manifest, credentials));
    }

    /// ダウンロードの状態。待っても動いてもいなければ None
    pub(super) fn download_status(&self, dest: &Path) -> Option<DownloadStatus> {
        if let Some(active) = &self.plugins.active
            && active.pending.dest == dest
        {
            let ratio = active
                .total
                .filter(|&total| total > 0)
                .map(|total| active.received as f32 / total as f32);
            return Some(DownloadStatus::Running(ratio));
        }
        self.plugins
            .queue
            .iter()
            .any(|pending| pending.dest == dest)
            .then_some(DownloadStatus::Queued)
    }

    /// 本のダウンロードを頼む。同じ本を待っていれば何もしない
    pub(super) fn request_download(&mut self, entry: &CoverEntry) {
        let Some(remote) = &entry.remote else {
            return;
        };
        if self.download_status(&entry.path).is_some() {
            return;
        }
        let Some(book) = self.plugins.shelves.get(&remote.plugin).and_then(|shelf| {
            shelf
                .books
                .iter()
                .find(|book| book.id == remote.id)
                .cloned()
        }) else {
            return;
        };
        self.plugins.download_error = None;
        self.plugins.queue.push_back(PendingDownload {
            plugin: remote.plugin.clone(),
            book,
            dest: entry.path.clone(),
        });
        self.start_next_download();
    }

    fn start_next_download(&mut self) {
        if self.plugins.active.is_some() {
            return;
        }
        let Some(pending) = self.plugins.queue.pop_front() else {
            return;
        };
        let Some(manifest) = self.plugins.manifest(&pending.plugin).cloned() else {
            return self.start_next_download();
        };
        let credentials = self
            .plugins
            .credentials
            .for_plugin(&pending.plugin, &manifest.keys());
        let rx = spawn_download(
            manifest,
            credentials,
            pending.book.id.clone(),
            pending.dest.clone(),
        );
        self.plugins.active = Some(ActiveDownload {
            pending,
            rx,
            received: 0,
            total: None,
        });
    }

    /// 一覧とダウンロードの知らせを受け取る。毎フレーム呼ぶ
    pub(super) fn poll_plugins(&mut self, ctx: &egui::Context) {
        let mut busy = false;
        let mut arrived = Vec::new();
        for (id, shelf) in self.plugins.shelves.iter_mut() {
            let Some(rx) = &shelf.loading else {
                continue;
            };
            match rx.try_recv() {
                Ok(result) => {
                    shelf.loading = None;
                    match result {
                        Ok(books) => {
                            shelf.books = books;
                            shelf.fetched_at = unix_now();
                            arrived.push(id.clone());
                        }
                        Err(error) => shelf.error = Some(error),
                    }
                }
                Err(TryRecvError::Empty) => busy = true,
                Err(TryRecvError::Disconnected) => shelf.loading = None,
            }
        }
        for id in arrived {
            if let Some(shelf) = self.plugins.shelves.get(&id) {
                ListCache {
                    books: shelf.books.clone(),
                    fetched_at: shelf.fetched_at,
                }
                .save(&self.plugins.list_cache_path(&id));
            }
            // 並べていれば並べ直す
            if self.shelf_source() == Some(ShelfSource::Plugin(id.clone())) {
                self.reshow_shelf();
            }
        }

        let mut finished = None;
        if let Some(active) = self.plugins.active.as_mut() {
            busy = true;
            loop {
                match active.rx.try_recv() {
                    Ok(DownloadEvent::Progress { received, total }) => {
                        active.received = received;
                        active.total = total;
                    }
                    Ok(DownloadEvent::Done(result)) => {
                        finished = Some(result);
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = Some(Err(PluginError::Other(
                            tr("plugin.download_failed").to_owned(),
                        )));
                        break;
                    }
                }
            }
        }
        if let Some(result) = finished
            && let Some(active) = self.plugins.active.take()
        {
            match result {
                Ok(()) => {
                    // 選んだ本の一覧をまだ見ていれば、そのまま開く
                    let dest = active.pending.dest;
                    if self.selected_shelf_path().as_ref() == Some(&dest) {
                        self.thumbnails = None;
                        self.open_source(dest, ctx);
                    }
                }
                Err(error) => {
                    self.plugins.download_error = Some(format!(
                        "{}: {}",
                        active.pending.book.title,
                        error.message()
                    ));
                    // ログインできなければ、待っているものも同じなのでやめる
                    if matches!(error, PluginError::Auth(_)) {
                        self.plugins.queue.clear();
                    }
                }
            }
            self.start_next_download();
        }
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// 設定画面の、プラグインのログイン情報とダウンロード先
    pub(super) fn render_plugin_settings(&mut self, ui: &mut egui::Ui) {
        ui.add_space(20.0);
        ui.label(
            egui::RichText::new(tr("settings.plugins"))
                .size(20.0)
                .strong(),
        );
        crate::utils::separator_pct(ui);
        ui.label(tr("settings.library_dir"));
        let placeholder = settings_store::default_library_dir()
            .to_string_lossy()
            .into_owned();
        if ui
            .add(
                egui::TextEdit::singleline(&mut self.config.library_dir)
                    .hint_text(placeholder)
                    .desired_width(f32::INFINITY),
            )
            .lost_focus()
        {
            self.save_settings();
        }
        if self.plugins.manifests.is_empty() {
            ui.add_space(6.0);
            ui.label(tr("settings.no_plugins"));
            return;
        }
        let manifests = self.plugins.manifests.clone();
        for manifest in manifests {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(&manifest.name).strong());
            for field in &manifest.credentials {
                let index = (manifest.id.clone(), field.key.clone());
                let label = match (field.key.as_str(), &field.label) {
                    (_, Some(label)) => label.clone(),
                    ("email", None) => tr("plugin.field.email").to_owned(),
                    ("password", None) => tr("plugin.field.password").to_owned(),
                    (key, None) => key.to_owned(),
                };
                let stored = self.plugins.stored(&manifest.id, &field.key);
                let hint = match stored {
                    Some(Stored::Keyring) => tr("plugin.stored_keyring"),
                    Some(Stored::Memory) => tr("plugin.stored_memory"),
                    None => "",
                };
                ui.label(label);
                ui.horizontal(|ui| {
                    let input = self.plugins.inputs.entry(index.clone()).or_default();
                    let response = ui.add(
                        egui::TextEdit::singleline(input)
                            .password(field.secret)
                            .hint_text(hint)
                            .desired_width(ui.available_width() - 70.0),
                    );
                    let enter =
                        response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let save = ui.button(tr("plugin.save")).clicked() || enter;
                    if save && !input.is_empty() {
                        let value = std::mem::take(input);
                        let stored = self
                            .plugins
                            .credentials
                            .set(&manifest.id, &field.key, &value);
                        self.plugins.stored.insert(index.clone(), Some(stored));
                    }
                    if stored.is_some()
                        && ui
                            .small_button("✕")
                            .on_hover_text(tr("plugin.forget"))
                            .clicked()
                    {
                        self.plugins.credentials.delete(&manifest.id, &field.key);
                        self.plugins.stored.insert(index.clone(), None);
                    }
                });
            }
            let in_memory = manifest.credentials.iter().any(|field| {
                self.plugins
                    .stored
                    .get(&(manifest.id.clone(), field.key.clone()))
                    == Some(&Some(Stored::Memory))
            });
            if in_memory {
                ui.label(
                    egui::RichText::new(tr("plugin.no_keyring"))
                        .small()
                        .color(ui.visuals().warn_fg_color),
                );
            }
        }
    }
}
