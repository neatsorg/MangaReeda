//! ブックリストのプラグインを、表紙サムネイルの画面と設定画面につなぐ。
//! 一覧はフォルダごとに手元にためておき、更新は利用者が頼んだときだけ行う（サイトに負担をかけないように）。
//! ダウンロードは1冊ずつ順に行う。プラグインが保存してほしいと返した値（トークンなど）はキーリングに置く。

use super::MangaReader;
use super::thumbnails::ShelfSource;
use crate::engine::credentials::{Credentials, Stored};
use crate::engine::plugin::{
    DownloadEvent, ListCache, ListProgress, Listing, PluginError, PluginKind, PluginManifest,
    RemoteBook, Store, auth_url, local_path, spawn_download, spawn_list,
};
use crate::engine::settings_store;
use crate::engine::thumbnail::{CoverEntry, RemoteRef, cover_cache_name};
use crate::localize::tr;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

/// プラグインの一覧の場所: (プラグインの id, フォルダ)。フォルダの "" は一番上
pub(super) type ShelfKey = (String, String);

/// プラグインの答えと、保存してほしい値
type PluginAnswer<T> = (Result<T, PluginError>, Store);

/// プラグインのフォルダごとの一覧
#[derive(Default)]
pub(super) struct PluginShelf {
    pub(super) books: Vec<RemoteBook>,
    /// 上のフォルダ。一番上なら None
    pub(super) parent: Option<String>,
    /// 一覧を取った日時（UNIX 秒）。0 ならまだ取っていない
    pub(super) fetched_at: u64,
    loading: Option<Receiver<PluginAnswer<Listing>>>,
    /// 取っているあいだの進み具合
    progress: Option<ListProgress>,
    pub(super) error: Option<PluginError>,
    /// プラグインが一覧と一緒に返した一言
    pub(super) message: Option<String>,
}

impl PluginShelf {
    pub(super) fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// 取っているあいだの進み具合（「12/60」）。プラグインが知らせていなければ None
    pub(super) fn progress_text(&self) -> Option<String> {
        let (done, total) = (*self.progress.as_ref()?.lock().ok()?)?;
        Some(match total {
            Some(total) => format!("{done}/{total}"),
            None => done.to_string(),
        })
    }
}

/// ダウンロードした本の持ち主: アカウント（`Listing::account` から作った短い値）と本の id
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(super) struct Owner {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    id: String,
}

/// 記録のファイルの値。前の形（本の id だけ）も読む
#[derive(Deserialize)]
#[serde(untagged)]
enum OwnerRecord {
    Owner(Owner),
    Id(String),
}

impl From<OwnerRecord> for Owner {
    fn from(record: OwnerRecord) -> Self {
        match record {
            OwnerRecord::Owner(owner) => owner,
            OwnerRecord::Id(id) => Owner { account: None, id },
        }
    }
}

/// ダウンロードを待っている本
struct PendingDownload {
    plugin: String,
    owner: Owner,
    title: String,
    dest: PathBuf,
}

/// ダウンロード中の本
struct ActiveDownload {
    pending: PendingDownload,
    /// 始めたときの、ログイン情報の世代
    generation: u64,
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
    /// 本の一覧のプラグイン（表紙サムネイルのタブ）
    pub(super) manifests: Vec<PluginManifest>,
    /// OCR のプラグイン
    pub(super) ocr_engines: Vec<PluginManifest>,
    pub(super) shelves: HashMap<ShelfKey, PluginShelf>,
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
    /// 設定画面に出す、接続（ブラウザでの許可）の結果
    auth_message: HashMap<String, String>,
    /// ダウンロードした本が、どのプラグインのどの本か（プラグインごとに、置いた場所 → 本の id）。
    /// 名前が同じ別の本を取り違えないように使う。場所は `file_key` の形で持つ
    pub(super) owners: HashMap<String, HashMap<PathBuf, Owner>>,
    /// 場所を比べるときに、大文字・小文字を区別しないか（Windows と macOS のふつうのファイルシステム）
    pub(super) ignore_case: bool,
    /// プラグインごとの、いま使っているアカウント（一覧の答えから作った短い値。答えに無ければ None）
    accounts: HashMap<String, Option<String>>,
    /// 許可のページの URL を作ってもらっているプラグインと、その答え
    auth_pending: Option<(String, u64, Receiver<PluginAnswer<String>>)>,
    /// プラグインごとの、ログイン情報の世代。設定画面でログイン情報を変えるたびに進める
    generations: HashMap<String, u64>,
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
        // 本の一覧のプラグインはタブに並べ、OCR のプラグインは本の中の検索で使う
        let (ocr_engines, manifests) = manifests
            .into_iter()
            .partition(|manifest| manifest.kind == PluginKind::Ocr);
        Self {
            manifests,
            ocr_engines,
            shelves: HashMap::new(),
            credentials,
            queue: VecDeque::new(),
            active: None,
            cache_dir,
            inputs: HashMap::new(),
            stored: HashMap::new(),
            download_error: None,
            auth_message: HashMap::new(),
            auth_pending: None,
            owners: HashMap::new(),
            accounts: HashMap::new(),
            generations: HashMap::new(),
            ignore_case: cfg!(any(windows, target_os = "macos")),
        }
    }

    pub(super) fn manifest(&self, id: &str) -> Option<&PluginManifest> {
        self.manifests.iter().find(|m| m.id == id)
    }

    /// 一覧をためておくファイル。一番上は `<id>.json`、フォルダは `<id>/<名前から作った値>.json`
    fn list_cache_path(&self, (id, folder): &ShelfKey) -> PathBuf {
        let plugins = self.cache_dir.join("plugins");
        if folder.is_empty() {
            plugins.join(format!("{id}.json"))
        } else {
            plugins
                .join(id)
                .join(format!("{}.json", cover_cache_name(folder)))
        }
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

    #[cfg(test)]
    pub(super) fn credential(&self, plugin: &str, key: &str) -> Option<String> {
        self.credentials.get(plugin, key)
    }

    /// プラグインに渡すログイン情報（隠した項目も含む）
    pub(super) fn credentials_for(&self, manifest: &PluginManifest) -> HashMap<String, String> {
        self.credentials.for_plugin(&manifest.id, &manifest.keys())
    }

    /// プラグインが保存してほしいと返した値を保存する。空の値は消す
    fn apply_store(&mut self, plugin: &str, store: Store) {
        for (key, value) in store {
            let stored = if value.is_empty() {
                self.delete_credential(plugin, &key)
            } else {
                Some(self.credentials.set(plugin, &key, &value))
            };
            self.stored.insert((plugin.to_owned(), key), stored);
        }
    }

    fn generation(&self, plugin: &str) -> u64 {
        self.generations.get(plugin).copied().unwrap_or(0)
    }

    /// 処理を始めたときと同じ世代のログイン情報なら、プラグインが返した値を保存する
    /// （切り替える前に始めた処理が、新しいログイン情報を上書きしたり消したりしないように）
    fn apply_store_from(&mut self, plugin: &str, generation: u64, store: Store) {
        if generation == self.generation(plugin) {
            self.apply_store(plugin, store);
        }
    }

    /// ログイン情報を消す。消せなければ設定画面で知らせる（消せるまで、起動しているあいだは使わない）。
    /// 消したあとの保存の状態を返す
    fn delete_credential(&mut self, plugin: &str, key: &str) -> Option<Stored> {
        match self.credentials.delete(plugin, key) {
            Ok(()) => {
                self.auth_message.remove(plugin);
            }
            Err(e) => {
                self.auth_message.insert(
                    plugin.to_owned(),
                    format!("{}: {e}", tr("plugin.forget_failed")),
                );
            }
        }
        self.credentials.stored(plugin, key)
    }

    /// プラグインのいまのアカウントで、本 `id` の持ち主
    pub(super) fn owner(&self, plugin: &str, id: &str) -> Owner {
        Owner {
            account: self.accounts.get(plugin).cloned().flatten(),
            id: id.to_owned(),
        }
    }

    /// 同じファイルを指すかを比べるための形。大文字・小文字を区別しないファイルシステムでは、
    /// "Book.pdf" と "book.pdf" は同じファイルなので、小文字にそろえる
    pub(super) fn file_key(&self, path: &Path) -> PathBuf {
        if self.ignore_case {
            PathBuf::from(path.to_string_lossy().to_lowercase())
        } else {
            path.to_path_buf()
        }
    }

    /// その場所を使っている本。ダウンロード中や待っている本も含める（取りかかる前に場所を押さえる）
    fn path_owner(&self, plugin: &str, path: &Path) -> Option<Owner> {
        let key = self.file_key(path);
        let downloading = self.active.iter().map(|active| &active.pending);
        downloading
            .chain(self.queue.iter())
            .find(|pending| pending.plugin == plugin && self.file_key(&pending.dest) == key)
            .map(|pending| pending.owner.clone())
            .or_else(|| self.owners.get(plugin)?.get(&key).cloned())
    }

    /// 本を置く場所。その本の場所か、まだ何も無い場所を使う。別の本が使っている場所や、
    /// 記録の無いファイル（記録を始める前の版でダウンロードしたものなど。どの本かわからない）の場所、
    /// `used`（`file_key` の形）の場所は使わず、印を付けた場所を順に確かめる（もとのファイルは消さない）
    fn resolve_path(
        &self,
        plugin: &str,
        owner: &Owner,
        path: &Path,
        used: &HashSet<PathBuf>,
    ) -> PathBuf {
        let usable = |candidate: &PathBuf| {
            !used.contains(&self.file_key(candidate))
                && match self.path_owner(plugin, candidate) {
                    Some(other) => &other == owner,
                    None => !candidate.exists(),
                }
        };
        (0..MARKER_TRIES)
            .map(|round| match round {
                0 => path.to_path_buf(),
                round => with_marker(path, owner, round),
            })
            .find(usable)
            // 印を変えても埋まっていれば（まず起きない）、確かめていない場所ではなく、まだ試していない印にする
            .unwrap_or_else(|| with_marker(path, owner, MARKER_TRIES))
    }

    /// プラグインのためておいた一覧（`keep` のフォルダのものは残す）と、待っているダウンロードを捨てる。
    /// ダウンロード中の本は、頼んだときのアカウントの本として最後まで受け取る
    fn drop_account_data(&mut self, plugin: &str, keep: Option<&str>) {
        self.shelves
            .retain(|(id, folder), _| id != plugin || Some(folder.as_str()) == keep);
        self.queue.retain(|pending| pending.plugin != plugin);
        let plugins = self.cache_dir.join("plugins");
        let _ = std::fs::remove_file(plugins.join(format!("{plugin}.json")));
        let _ = std::fs::remove_dir_all(plugins.join(plugin));
    }

    /// ログイン情報が変わった（別のアカウントになったかもしれない）。次に一覧を取るまで、アカウントはわからないものとする
    fn forget_account(&mut self, plugin: &str) {
        *self.generations.entry(plugin.to_owned()).or_default() += 1;
        self.accounts.remove(plugin);
        self.drop_account_data(plugin, None);
    }

    /// 一覧の答えで、プラグインのアカウントを覚える。前と違えば、ほかのフォルダの一覧と待っているダウンロードを捨てる
    pub(super) fn set_account(&mut self, key: &ShelfKey, account: Option<String>) {
        if self
            .accounts
            .get(&key.0)
            .is_some_and(|known| known != &account)
        {
            self.drop_account_data(&key.0, Some(&key.1));
        }
        self.accounts.insert(key.0.clone(), account);
    }
}

/// ダウンロードした本の記録のファイル名。プラグインのダウンロード先のフォルダに置く
const OWNERS_FILE: &str = ".mangareeda-downloads.json";

/// アカウントを見分ける値から、手元に置く短い値を作る（メールアドレスなどをそのまま書かないように）
fn account_key(account: &str) -> String {
    cover_cache_name(account)
}

/// 印を変えて置く場所を探す回数
const MARKER_TRIES: u32 = 16;

/// 置く場所に、持ち主から作った短い印を付ける（"a.zip" → "a~1b2c3d4e.zip"）。
/// `round` が 2 からは、その場所も埋まっていたときの別の印
fn with_marker(path: &Path, owner: &Owner, round: u32) -> PathBuf {
    let mut source = match &owner.account {
        Some(account) => format!("{account}\n{}", owner.id),
        None => owner.id.clone(),
    };
    if round > 1 {
        source.push_str(&format!("\n{round}"));
    }
    let marker = &cover_cache_name(&source)[..8];
    let stem = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let name = match path.extension() {
        Some(extension) => format!("{stem}~{marker}.{}", extension.to_string_lossy()),
        None => format!("{stem}~{marker}"),
    };
    path.with_file_name(name)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// URL を既定のブラウザで開く
fn open_in_browser(url: &str) -> std::io::Result<()> {
    let mut command = if cfg!(windows) {
        // cmd の start は URL の "&" を命令の区切りと読んでしまうので、cmd を通さずに開く
        let mut command = std::process::Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    command.arg(url).spawn().map(|_| ())
}

impl MangaReader {
    /// プラグインからダウンロードした本を置くフォルダ
    pub(super) fn library_dir(&self) -> PathBuf {
        if self.config.library_dir.trim().is_empty() {
            settings_store::default_library_dir()
        } else {
            PathBuf::from(self.config.library_dir.trim())
        }
    }

    /// プラグインのそのフォルダで最後に見ていたフォルダ。無ければ一番上
    pub(super) fn plugin_start_folder(&self, id: &str) -> String {
        self.config
            .plugin_folders
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// プラグインの一覧を表紙サムネイルに並べる形にする
    /// プラグインがダウンロードした本の記録（置いた場所 → 本の id）のファイル
    fn owners_path(&self, plugin: &str) -> PathBuf {
        self.library_dir().join(plugin).join(OWNERS_FILE)
    }

    /// ダウンロードした本の記録を読んでおく
    pub(super) fn load_owners(&mut self, plugin: &str) {
        if self.plugins.owners.contains_key(plugin) {
            return;
        }
        let base = self.library_dir().join(plugin);
        let owners = std::fs::read_to_string(self.owners_path(plugin))
            .ok()
            .and_then(|data| serde_json::from_str::<HashMap<String, OwnerRecord>>(&data).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|(relative, record)| (self.plugins.file_key(&base.join(relative)), record.into()))
            .collect();
        self.plugins.owners.insert(plugin.to_owned(), owners);
    }

    /// ダウンロードした本を記録する
    pub(super) fn record_owner(&mut self, plugin: &str, dest: &Path, owner: &Owner) {
        self.load_owners(plugin);
        let dir = self.library_dir().join(plugin);
        let base = self.plugins.file_key(&dir);
        let dest = self.plugins.file_key(dest);
        let owners = self.plugins.owners.entry(plugin.to_owned()).or_default();
        owners.insert(dest, owner.clone());
        let relative: HashMap<String, &Owner> = owners
            .iter()
            .filter_map(|(path, id)| {
                Some((
                    path.strip_prefix(&base)
                        .ok()?
                        .to_string_lossy()
                        .into_owned(),
                    id,
                ))
            })
            .collect();
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(json) = serde_json::to_string_pretty(&relative)
            && let Err(e) = settings_store::write_atomic(&self.owners_path(plugin), json.as_bytes())
        {
            eprintln!("ダウンロードの記録を保存できませんでした: {e}");
        }
    }

    /// プラグインの一覧を表紙サムネイルに並べる形にする。本を置く場所は、名前が同じ別の本
    /// （同じ一覧の中で重なるもの、前に別の本や別のアカウントの本を置いた場所、ダウンロード中や
    /// 待っている別の本の場所）と重ならないよう、持ち主から作った印を付けて分ける
    pub(super) fn plugin_entries(&self, key: &ShelfKey) -> Vec<CoverEntry> {
        let Some(shelf) = self.plugins.shelves.get(key) else {
            return Vec::new();
        };
        let library = self.library_dir();
        let mut used = HashSet::new();
        shelf
            .books
            .iter()
            .map(|book| {
                let mut path = local_path(&library, &key.0, &book.file_name);
                if !book.folder {
                    let owner = self.plugins.owner(&key.0, &book.id);
                    path = self.plugins.resolve_path(&key.0, &owner, &path, &used);
                }
                used.insert(self.plugins.file_key(&path));
                (book, path)
            })
            .map(|(book, path)| CoverEntry {
                path,
                is_folder: book.folder,
                loose_pages: None,
                remote: Some(RemoteRef {
                    plugin: key.0.clone(),
                    id: book.id.clone(),
                    title: book.title.clone(),
                    cover_url: book.cover_url.clone(),
                }),
            })
            .collect()
    }

    /// プラグインのフォルダの上のフォルダ。わからなくても、一番上でなければ一番上に戻れるようにする
    /// （消えたフォルダなど、読めなかったフォルダから抜け出せるように）
    pub(super) fn plugin_parent(&self, key: &ShelfKey) -> Option<String> {
        self.plugins
            .shelves
            .get(key)
            .and_then(|shelf| shelf.parent.clone())
            .or_else(|| (!key.1.is_empty()).then(String::new))
    }

    /// 次にタブを開いたときにこのフォルダから始めるよう覚える。読めたフォルダだけを覚える
    fn remember_plugin_folder(&mut self, key: &ShelfKey) {
        if self.plugin_start_folder(&key.0) != key.1 {
            self.config
                .plugin_folders
                .insert(key.0.clone(), key.1.clone());
            self.save_settings();
        }
    }

    /// 一覧が無ければ、手元にためたものを読む。それも無ければ取りに行く
    pub(super) fn ensure_plugin_list(&mut self, key: &ShelfKey) {
        self.load_owners(&key.0);
        if let Some(shelf) = self.plugins.shelves.get(key) {
            if shelf.fetched_at > 0 {
                self.remember_plugin_folder(key);
            }
            return;
        }
        // 別のアカウントで取った一覧は使わない
        let known = self.plugins.accounts.get(&key.0).cloned();
        let cache = ListCache::load(&self.plugins.list_cache_path(key))
            .filter(|cache| known.as_ref().is_none_or(|known| known == &cache.account));
        if let Some(cache) = &cache
            && known.is_none()
        {
            self.plugins
                .accounts
                .insert(key.0.clone(), cache.account.clone());
        }
        let fetched = cache.is_some();
        let shelf = cache
            .map(|cache| PluginShelf {
                books: cache.books,
                parent: cache.parent,
                fetched_at: cache.fetched_at,
                ..PluginShelf::default()
            })
            .unwrap_or_default();
        self.plugins.shelves.insert(key.clone(), shelf);
        if fetched {
            self.remember_plugin_folder(key);
        } else {
            self.refresh_plugin_list(key);
        }
    }

    /// 一覧を取り直す（利用者が頼んだとき）
    pub(super) fn refresh_plugin_list(&mut self, key: &ShelfKey) {
        let Some(manifest) = self.plugins.manifest(&key.0).cloned() else {
            return;
        };
        if self
            .plugins
            .shelves
            .get(key)
            .is_some_and(PluginShelf::is_loading)
        {
            return;
        }
        let credentials = self.plugins.credentials_for(&manifest);
        let folder = (!key.1.is_empty()).then(|| key.1.clone());
        let covers = self.plugins.cover_cache_dir();
        let shelf = self.plugins.shelves.entry(key.clone()).or_default();
        shelf.error = None;
        let (loading, progress) = spawn_list(manifest, credentials, folder, covers);
        shelf.loading = Some(loading);
        shelf.progress = Some(progress);
    }

    /// 一覧の本を、いま置いてある（これから置く）場所。別の本が使っていれば、印を付けた場所
    /// （一覧を並べたあとに、別の本がその場所を使い始めることもあるので、そのつど確かめる）
    pub(super) fn remote_path(&self, entry: &CoverEntry) -> PathBuf {
        match &entry.remote {
            Some(remote) if !entry.is_folder => {
                let owner = self.plugins.owner(&remote.plugin, &remote.id);
                self.plugins
                    .resolve_path(&remote.plugin, &owner, &entry.path, &HashSet::new())
            }
            _ => entry.path.clone(),
        }
    }

    /// ダウンロードの状態。待っても動いてもいなければ None
    pub(super) fn download_status(&self, entry: &CoverEntry) -> Option<DownloadStatus> {
        let remote = entry.remote.as_ref()?;
        let owner = self.plugins.owner(&remote.plugin, &remote.id);
        let same =
            |pending: &PendingDownload| pending.plugin == remote.plugin && pending.owner == owner;
        if let Some(active) = &self.plugins.active
            && same(&active.pending)
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
            .any(same)
            .then_some(DownloadStatus::Queued)
    }

    /// 本のダウンロードを頼む。同じ本を待っていれば何もしない
    pub(super) fn request_download(&mut self, entry: &CoverEntry) {
        let Some(remote) = &entry.remote else {
            return;
        };
        if self.download_status(entry).is_some() {
            return;
        }
        self.plugins.download_error = None;
        let dest = self.remote_path(entry);
        self.plugins.queue.push_back(PendingDownload {
            plugin: remote.plugin.clone(),
            owner: self.plugins.owner(&remote.plugin, &remote.id),
            title: remote.title.clone(),
            dest,
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
        let credentials = self.plugins.credentials_for(&manifest);
        let rx = spawn_download(
            manifest,
            credentials,
            pending.owner.id.clone(),
            pending.dest.clone(),
        );
        self.plugins.active = Some(ActiveDownload {
            generation: self.plugins.generation(&pending.plugin),
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
        let mut stores = Vec::new();
        let mut accounts = Vec::new();
        for (key, shelf) in self.plugins.shelves.iter_mut() {
            let Some(rx) = &shelf.loading else {
                continue;
            };
            match rx.try_recv() {
                Ok((result, store)) => {
                    shelf.loading = None;
                    shelf.progress = None;
                    stores.push((key.0.clone(), store));
                    match result {
                        Ok(listing) => {
                            shelf.message = listing.message;
                            shelf.books = listing.books;
                            shelf.parent = listing.parent;
                            shelf.fetched_at = unix_now();
                            accounts
                                .push((key.clone(), listing.account.as_deref().map(account_key)));
                        }
                        Err(error) => shelf.error = Some(error),
                    }
                    // 届いたら並べ直す（空のときの知らせを変えるため、失敗したときも）
                    arrived.push(key.clone());
                }
                Err(TryRecvError::Empty) => busy = true,
                Err(TryRecvError::Disconnected) => {
                    shelf.loading = None;
                    shelf.progress = None;
                }
            }
        }
        for (plugin, store) in stores {
            self.plugins.apply_store(&plugin, store);
        }
        for (key, account) in accounts {
            self.plugins.set_account(&key, account);
        }
        arrived.dedup();
        for key in arrived {
            let showing =
                self.shelf_source() == Some(ShelfSource::Plugin(key.0.clone(), key.1.clone()));
            let loaded = self
                .plugins
                .shelves
                .get(&key)
                .is_some_and(|shelf| shelf.error.is_none());
            if loaded && let Some(shelf) = self.plugins.shelves.get(&key) {
                ListCache {
                    books: shelf.books.clone(),
                    parent: shelf.parent.clone(),
                    account: self.plugins.accounts.get(&key.0).cloned().flatten(),
                    fetched_at: shelf.fetched_at,
                }
                .save(&self.plugins.list_cache_path(&key));
                if showing {
                    self.remember_plugin_folder(&key);
                }
            } else if !loaded && self.plugin_start_folder(&key.0) == key.1 && !key.1.is_empty() {
                // 覚えていたフォルダが読めなければ、次は一番上から始める
                self.config.plugin_folders.remove(&key.0);
                self.save_settings();
            }
            if showing {
                self.reshow_shelf();
            }
        }

        let mut finished = None;
        let mut store = Store::new();
        if let Some(active) = self.plugins.active.as_mut() {
            busy = true;
            loop {
                match active.rx.try_recv() {
                    Ok(DownloadEvent::Progress { received, total }) => {
                        active.received = received;
                        active.total = total;
                    }
                    Ok(DownloadEvent::Store(values)) => store.extend(values),
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
            if !store.is_empty() {
                let (plugin, generation) = (active.pending.plugin.clone(), active.generation);
                self.plugins.apply_store_from(&plugin, generation, store);
            }
        }
        if let Some(result) = finished
            && let Some(active) = self.plugins.active.take()
        {
            match result {
                Ok(()) => {
                    let pending = active.pending;
                    self.record_owner(&pending.plugin, &pending.dest, &pending.owner);
                    // ダウンロードした本も、下ごしらえの対象にする
                    self.text_prefetch.rescan();
                    // 選んだ本の一覧をまだ見ていれば、そのまま開く（場所ではなく、どの本かで確かめる）
                    let selected = self.selected_shelf_entry().and_then(|entry| entry.remote);
                    let chosen = selected.is_some_and(|remote| {
                        remote.plugin == pending.plugin
                            && self.plugins.owner(&remote.plugin, &remote.id) == pending.owner
                    });
                    if chosen {
                        self.thumbnails = None;
                        self.open_source(pending.dest, ctx);
                    }
                }
                Err(error) => {
                    self.plugins.download_error =
                        Some(format!("{}: {}", active.pending.title, error.message()));
                    // ログインできなければ、待っているものも同じなのでやめる
                    if matches!(error, PluginError::Auth(_)) {
                        self.plugins.queue.clear();
                    }
                }
            }
            self.start_next_download();
        }
        busy |= self.poll_plugin_auth();
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    /// ブラウザで許可するページを開く。プラグインが返した値（PKCE の検証用の値など）は保存する
    /// 画面を止めないよう、プラグインは別のスレッドで呼ぶ。答えは poll_plugin_auth で受け取る
    fn start_plugin_auth(&mut self, manifest: &PluginManifest) {
        if self.plugins.auth_pending.is_some() {
            return;
        }
        let credentials = self.plugins.credentials_for(manifest);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_manifest = manifest.clone();
        std::thread::spawn(move || {
            let _ = tx.send(auth_url(&worker_manifest, &credentials));
        });
        let generation = self.plugins.generation(&manifest.id);
        self.plugins.auth_pending = Some((manifest.id.clone(), generation, rx));
        self.plugins
            .auth_message
            .insert(manifest.id.clone(), tr("plugin.auth_preparing").to_owned());
    }

    /// 許可のページの URL が届いたら、プラグインが返した値（PKCE の検証用の値など）を保存し、
    /// ブラウザで開く。届くまでは true
    fn poll_plugin_auth(&mut self) -> bool {
        let Some((plugin, generation, rx)) = &self.plugins.auth_pending else {
            return false;
        };
        let (result, store) = match rx.try_recv() {
            Ok(answer) => answer,
            Err(TryRecvError::Empty) => return true,
            Err(TryRecvError::Disconnected) => (
                Err(PluginError::Other(tr("plugin.download_failed").to_owned())),
                Store::new(),
            ),
        };
        let (plugin, generation) = (plugin.clone(), *generation);
        self.plugins.auth_pending = None;
        if generation != self.plugins.generation(&plugin) {
            // 待っているあいだにログイン情報が変わった。古いログイン情報で作ったページは開かない
            self.plugins.auth_message.remove(&plugin);
            return false;
        }
        self.plugins.apply_store(&plugin, store);
        let message = match result {
            Ok(url) => match open_in_browser(&url) {
                Ok(()) => tr("plugin.auth_opened").to_owned(),
                Err(_) => format!("{}\n{url}", tr("plugin.auth_open_failed")),
            },
            Err(error) => error.message().to_owned(),
        };
        self.plugins.auth_message.insert(plugin, message);
        false
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
        self.render_cross_search_settings(ui);
        if self.plugins.manifests.is_empty() && self.plugins.ocr_engines.is_empty() {
            ui.add_space(6.0);
            ui.label(tr("settings.no_plugins"));
            return;
        }
        // 本の中の検索で使う OCR のエンジン
        if !self.plugins.ocr_engines.is_empty() {
            ui.add_space(6.0);
            let current = self
                .ocr_engine()
                .map(|engine| engine.name.clone())
                .unwrap_or_default();
            let mut chosen = None;
            egui::ComboBox::from_label(tr("settings.ocr_engine"))
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for engine in &self.plugins.ocr_engines {
                        if ui.selectable_label(false, &engine.name).clicked() {
                            chosen = Some(engine.id.clone());
                        }
                    }
                });
            if let Some(id) = chosen {
                self.config.ocr_engine = id;
                self.save_settings();
            }
        }
        // ログイン情報の欄は、本の一覧のプラグインと OCR のプラグインの両方に出す
        let manifests: Vec<PluginManifest> = self
            .plugins
            .manifests
            .iter()
            .chain(&self.plugins.ocr_engines)
            .cloned()
            .collect();
        for manifest in manifests {
            let mut changed = false;
            ui.add_space(10.0);
            ui.label(egui::RichText::new(&manifest.name).strong());
            for field in manifest.credentials.iter().filter(|field| !field.hidden) {
                let index = (manifest.id.clone(), field.key.clone());
                let label = match (field.key.as_str(), &field.label) {
                    (_, Some(label)) => label.clone(),
                    ("email", None) => tr("plugin.field.email").to_owned(),
                    ("password", None) => tr("plugin.field.password").to_owned(),
                    ("app_key", None) => tr("plugin.field.app_key").to_owned(),
                    ("auth_code", None) => tr("plugin.field.auth_code").to_owned(),
                    ("client_id", None) => tr("plugin.field.client_id").to_owned(),
                    ("client_secret", None) => tr("plugin.field.client_secret").to_owned(),
                    ("auth_response", None) => tr("plugin.field.auth_response").to_owned(),
                    (key, None) => key.to_owned(),
                };
                let stored = self.plugins.stored(&manifest.id, &field.key);
                let hint = match stored {
                    Some(Stored::Keyring) => tr("plugin.stored_keyring"),
                    Some(Stored::Memory) => tr("plugin.stored_memory"),
                    Some(Stored::DeleteFailed) => tr("plugin.delete_failed"),
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
                    // 貼り付けたときの前後の空白や改行は取る
                    if save && !input.trim().is_empty() {
                        let value = std::mem::take(input).trim().to_owned();
                        let stored = self
                            .plugins
                            .credentials
                            .set(&manifest.id, &field.key, &value);
                        self.plugins.stored.insert(index.clone(), Some(stored));
                        changed = true;
                    }
                    if stored.is_some()
                        && ui
                            .small_button("×")
                            .on_hover_text(tr("plugin.forget"))
                            .clicked()
                    {
                        let stored = self.plugins.delete_credential(&manifest.id, &field.key);
                        self.plugins.stored.insert(index.clone(), stored);
                        changed = true;
                    }
                });
            }
            // 設定画面に出さない値で、消せなかったものは、もう一度消せるようにする
            let undeleted: Vec<String> = manifest
                .credentials
                .iter()
                .filter(|field| field.hidden)
                .filter(|field| {
                    self.plugins.stored(&manifest.id, &field.key) == Some(Stored::DeleteFailed)
                })
                .map(|field| field.key.clone())
                .collect();
            if !undeleted.is_empty() && ui.button(tr("plugin.retry_delete")).clicked() {
                for key in undeleted {
                    let stored = self.plugins.delete_credential(&manifest.id, &key);
                    self.plugins
                        .stored
                        .insert((manifest.id.clone(), key), stored);
                }
            }
            if changed {
                // 別のアカウントになったかもしれないので、一覧を取り直す
                self.plugins.forget_account(&manifest.id);
                if matches!(self.shelf_source(), Some(ShelfSource::Plugin(id, _)) if id == manifest.id)
                {
                    self.reshow_shelf();
                }
            }
            if manifest.auth {
                // 許可の状態は、更新用のトークンを保存しているかで見る
                let connected = matches!(
                    self.plugins.stored(&manifest.id, "refresh_token"),
                    Some(Stored::Keyring | Stored::Memory)
                );
                ui.horizontal(|ui| {
                    if ui.button(tr("plugin.auth_start")).clicked() {
                        self.start_plugin_auth(&manifest);
                    }
                    ui.label(if connected {
                        tr("plugin.connected")
                    } else {
                        tr("plugin.not_connected")
                    });
                });
            }
            if let Some(message) = self.plugins.auth_message.get(&manifest.id) {
                ui.label(egui::RichText::new(message).small());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::credentials::FakeStore;

    /// プラグインが頼んだ削除が失敗したら、設定画面で知らせ、もう一度消せるようにする
    #[test]
    fn failed_deletions_asked_by_plugins_are_shown() {
        let store = FakeStore::default();
        let mut host = PluginHost::with(
            Vec::new(),
            Credentials::with_store(Box::new(store.clone())),
            std::env::temp_dir(),
        );
        let value = |value: &str| Store::from([("auth_code".to_owned(), value.to_owned())]);
        host.apply_store("d", value("code"));
        assert_eq!(host.stored("d", "auth_code"), Some(Stored::Keyring));

        store.fail_delete.set(true);
        host.apply_store("d", value(""));
        assert_eq!(host.stored("d", "auth_code"), Some(Stored::DeleteFailed));
        assert!(host.auth_message.contains_key("d"));
        // 消せていない値はプラグインに渡さない
        assert_eq!(host.credentials.get("d", "auth_code"), None);

        store.fail_delete.set(false);
        assert_eq!(host.delete_credential("d", "auth_code"), None);
        assert!(!host.auth_message.contains_key("d"));
        assert!(store.values.borrow().is_empty());
    }

    /// ログイン情報を変える前に始めた処理が返した値は、新しいログイン情報に混ぜない
    #[test]
    fn values_from_work_started_before_a_credential_change_are_ignored() {
        let mut host = PluginHost::with(
            Vec::new(),
            Credentials::memory_only(),
            std::env::temp_dir().join(format!("manga-generation-{}", std::process::id())),
        );
        let old = host.generation("d");
        // B のログイン情報を入れた
        host.credentials.set("d", "auth_code", "code-b");
        host.forget_account("d");
        // A のときに始めたダウンロードが、トークンを取り替えて保存を頼んできた
        let store = Store::from([
            ("refresh_token".to_owned(), "token-a".to_owned()),
            ("auth_code".to_owned(), String::new()),
        ]);
        host.apply_store_from("d", old, store.clone());
        assert_eq!(host.credentials.get("d", "refresh_token"), None);
        assert_eq!(
            host.credentials.get("d", "auth_code").as_deref(),
            Some("code-b")
        );
        // 新しい世代で始めた処理の値は保存する
        let current = host.generation("d");
        host.apply_store_from("d", current, store);
        assert_eq!(
            host.credentials.get("d", "refresh_token").as_deref(),
            Some("token-a")
        );
    }
}
