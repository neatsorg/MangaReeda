//! サムネイル一覧の、画面に依存しない部分。
//! マスの並べ方、方向キーでの選択の動き、縮小画像の作成と、別スレッドでの読み込み。

use crate::config::SourceMode;
use crate::engine::history::normalize;
use crate::engine::imaging::strip_adobe_app14_if_invalid;
use crate::engine::library::{natural_sort_strings, scan_folder};
use crate::engine::pdf::SharedPdfium;
use crate::engine::source::{SourceReader, list_images, page_key};
use image::{ImageFormat, RgbaImage};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// サムネイル画像の長辺（ピクセル）。大きいマスでも粗く見えない程度
pub const THUMBNAIL_MAX_SIDE: u32 = 320;
/// 大きいマスの一覧で、画面の高さに入るおよその行数（ミーヤの「8列」の見た目に合わせた）
const LARGE_ROWS_PER_SCREEN: f32 = 3.0;
/// 表紙サムネイルのマスの一辺（ミーヤの見た目に合わせた）。下に冊の名前を出す
pub const COVER_CELL: f32 = 180.0;
/// 冊の入ったフォルダの表紙を、中の冊からたどって探す深さ
const COVER_SEARCH_DEPTH: usize = 3;

/// 一覧の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailMode {
    /// 全ページを1画面に収める（ミーヤの「1画面」）
    FitAll,
    /// 大きいマスで並べ、スクロールする（ミーヤの「8列」）
    Large,
    /// フォルダの中の冊を表紙で並べる。決まった大きさのマスを中央に置く
    Covers,
}

/// 正方形のマスの並べ方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    /// マスの一辺
    pub cell: f32,
}

impl Grid {
    /// `count` 枚を `area`（幅, 高さ）に並べる
    pub fn new(mode: ThumbnailMode, count: usize, area: [f32; 2]) -> Self {
        let count = count.max(1);
        let [width, height] = [area[0].max(1.0), area[1].max(1.0)];
        match mode {
            ThumbnailMode::FitAll => {
                // マスがいちばん大きくなる列数を選ぶ（同じ大きさなら列の少ないほう）
                let mut best = Grid {
                    columns: 1,
                    rows: count,
                    cell: (width).min(height / count as f32),
                };
                for columns in 2..=count {
                    let rows = count.div_ceil(columns);
                    let cell = (width / columns as f32).min(height / rows as f32);
                    if cell > best.cell {
                        best = Grid {
                            columns,
                            rows,
                            cell,
                        };
                    }
                }
                best
            }
            ThumbnailMode::Covers => {
                let columns = ((width / COVER_CELL).floor() as usize).clamp(1, count);
                Grid {
                    columns,
                    rows: count.div_ceil(columns),
                    cell: COVER_CELL,
                }
            }
            ThumbnailMode::Large => {
                // 高さの 1/3 ほどのマスを、横幅いっぱいに並べる
                let columns = ((width / (height / LARGE_ROWS_PER_SCREEN)).round() as usize).max(1);
                Grid {
                    columns,
                    rows: count.div_ceil(columns),
                    cell: width / columns as f32,
                }
            }
        }
    }

    /// 並べたときの全体の大きさ（幅, 高さ）
    pub fn size(&self) -> [f32; 2] {
        [
            self.columns as f32 * self.cell,
            self.rows as f32 * self.cell,
        ]
    }

    /// `index` 枚目のマスの左上（一覧の左上が原点）
    pub fn cell_origin(&self, index: usize) -> [f32; 2] {
        [
            (index % self.columns) as f32 * self.cell,
            (index / self.columns) as f32 * self.cell,
        ]
    }

    /// 一覧の左上を原点とした位置にあるマス。マスが無ければ None
    pub fn index_at(&self, point: [f32; 2], count: usize) -> Option<usize> {
        let [x, y] = point;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let column = (x / self.cell) as usize;
        let row = (y / self.cell) as usize;
        if column >= self.columns {
            return None;
        }
        let index = row * self.columns + column;
        (index < count).then_some(index)
    }
}

/// 方向キーで選択を動かす向き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// 方向キーで動かしたあとの選択。端より先へは動かない
pub fn move_selection(
    selected: usize,
    direction: Direction,
    columns: usize,
    count: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    let last = count - 1;
    match direction {
        Direction::Left => selected.saturating_sub(1),
        Direction::Right => (selected + 1).min(last),
        Direction::Up => selected.checked_sub(columns).unwrap_or(selected),
        // 最後の行が欠けているときは、最後のページで止まる
        Direction::Down if selected + columns <= last => selected + columns,
        Direction::Down if selected / columns < last / columns => last,
        Direction::Down => selected,
    }
}

/// 画像のバイト列から、長辺が `max_side` 以下のサムネイルを作る
pub fn decode_thumbnail(bytes: &[u8], max_side: u32, fix_adobe: bool) -> Option<RgbaImage> {
    let fixed;
    let bytes = if fix_adobe {
        fixed = strip_adobe_app14_if_invalid(bytes);
        &fixed[..]
    } else {
        bytes
    };
    let image = match image::guess_format(bytes) {
        Ok(format) => image::load_from_memory_with_format(bytes, format).ok()?,
        // 判別できないものは TGA として読んでみる（ページの読み込みと同じ）
        Err(_) => image::load_from_memory_with_format(bytes, ImageFormat::Tga).ok()?,
    };
    // thumbnail は小さい画像を拡大もするので、収まっているものはそのまま使う
    if image.width() <= max_side && image.height() <= max_side {
        return Some(image.to_rgba8());
    }
    Some(image.thumbnail(max_side, max_side).to_rgba8())
}

/// 表紙サムネイルに並べる冊
#[derive(Clone, Debug, PartialEq)]
pub struct CoverEntry {
    pub path: PathBuf,
    pub is_folder: bool,
    /// Some(枚数) なら、フォルダ `path` の直下の画像だけを冊として読むマス
    pub loose_pages: Option<usize>,
    /// プラグインの本なら、その情報。`path` はダウンロードしたときに置く場所
    pub remote: Option<RemoteRef>,
}

/// プラグインの本
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRef {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub cover_url: Option<String>,
}

/// 表紙を取ってくるところ
#[derive(Clone, Debug, PartialEq)]
pub enum CoverSource {
    /// 手元の冊の最初のページ
    Local(PathBuf),
    /// 表紙の画像の URL。一度取ったら `cache` に置き、次からはそれを使う。
    /// 取れなければ、手元にある本（`local`。ダウンロードした本）から作る
    Url {
        url: String,
        cache: PathBuf,
        local: Option<PathBuf>,
    },
}

/// 表紙の画像の大きさの上限
const COVER_DOWNLOAD_LIMIT: u64 = 10 * 1024 * 1024;

/// 表紙として使う画像の短い辺の最小（ピクセル）。これより小さいものは「画像なし」の印とみなす
const MIN_COVER_SIDE: u32 = 16;

/// 表紙の画像をサイトから取る間隔（サイトに負担をかけないよう、1枚ずつ間を空ける）
/// （サイトの前の Cloudflare などにたまっていれば、すぐ返るので、間は短くてよい）
const COVER_FETCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// 答えを待つ上限。BOOKSCAN は、表紙の無い本だと約 60 秒待たせてから 1×1 の画像を返すので、
/// それより短く切る（表紙のある本は数秒で返る）
const COVER_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// この長さ以上かかったか、時間切れになったら、元のサーバーが働いたとみて、終わってから長めに空ける
const COVER_SLOW: std::time::Duration = std::time::Duration::from_secs(5);
const COVER_SLOW_PAUSE: std::time::Duration = std::time::Duration::from_secs(10);
/// 取れなかった表紙を、取りに行かずにおく時間（秒）
const COVER_RETRY_AFTER: u64 = 24 * 60 * 60;
/// 時間切れになった表紙を、取りに行かずにおく時間（秒）。表紙の無い本のことが多いので、長めにする
const COVER_TIMEOUT_RETRY_AFTER: u64 = 7 * 24 * 60 * 60;
/// 時間切れの記録の、名前の前に付ける印
const TIMEOUT_MARK: &str = "timeout:";

/// 表紙を取りに行った結果
enum Fetched {
    Ok(Vec<u8>),
    TimedOut,
    Failed,
}

/// 手元に置いた URL の表紙の画像。名前の計算を変える前（Rust 標準のハッシュ）の名前で置いたものがあれば、
/// 名前を付け替えて使う（何百冊もの表紙を、サイトから取り直さないように）
fn cached_url_cover(url: &str, cache: &Path) -> Option<Vec<u8>> {
    if let Ok(bytes) = std::fs::read(cache) {
        return Some(bytes);
    }
    let legacy = cache.with_file_name(legacy_cache_name(url));
    std::fs::rename(&legacy, cache).ok()?;
    std::fs::read(cache).ok()
}

/// 取れなかった表紙の記録（表紙の名前 → 取れなかった日時）。表紙を置くフォルダに置く
fn failures_path(cache: &Path) -> PathBuf {
    cache.with_file_name("failed.json")
}

fn load_failures(cache: &Path) -> HashMap<String, u64> {
    std::fs::read_to_string(failures_path(cache))
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or_default()
}

fn save_failures(cache: &Path, failures: &HashMap<String, u64>) {
    if let Ok(json) = serde_json::to_string(failures) {
        let _ = crate::engine::settings_store::write_atomic(&failures_path(cache), json.as_bytes());
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// 記録を覚えておく時間（時間切れの記録は長め）
fn retry_after(key: &str) -> u64 {
    if key.starts_with(TIMEOUT_MARK) {
        COVER_TIMEOUT_RETRY_AFTER
    } else {
        COVER_RETRY_AFTER
    }
}

/// 最近取れなかった表紙か（ふつうは1日、時間切れは7日以内）
fn recently_failed(cache: &Path, url: &str, now: u64) -> bool {
    let name = cover_cache_name(url);
    let failures = load_failures(cache);
    [name.clone(), format!("{TIMEOUT_MARK}{name}")]
        .into_iter()
        .any(|key| {
            failures
                .get(&key)
                .is_some_and(|&at| now.saturating_sub(at) < retry_after(&key))
        })
}

/// 取れなかったことを覚える（古い記録は捨てる）。取れたら記録から外す。
/// `timed_out` なら、時間切れとして長めに覚える
fn note_fetch(cache: &Path, url: &str, ok: bool, timed_out: bool, now: u64) {
    let mut failures = load_failures(cache);
    let before = failures.len();
    failures.retain(|key, at| now.saturating_sub(*at) < retry_after(key));
    let name = cover_cache_name(url);
    let timeout_key = format!("{TIMEOUT_MARK}{name}");
    let changed = if ok {
        let removed = failures.remove(&name).is_some();
        failures.remove(&timeout_key).is_some() || removed
    } else {
        failures.insert(if timed_out { timeout_key } else { name }, now);
        true
    };
    if changed || failures.len() != before {
        save_failures(cache, &failures);
    }
}

/// 表紙の画像をサイトから取って、手元に置く。サイトに負担をかけないよう、一度に1枚ずつ、
/// 前に取り始めてから `COVER_FETCH_INTERVAL` 空けて取る。遅かったり時間切れになったりしたら、
/// 終わってから `COVER_SLOW_PAUSE` 空ける。最近取れなかった表紙は取りに行かない。
/// 順番を待っているあいだに `stop()` が真になったら（一覧を離れた、スクロールしてほかの冊を先に取る）取りに行かない
fn fetch_url_cover(url: &str, cache: &Path, stop: &dyn Fn() -> bool) -> Option<Vec<u8>> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return None;
    }
    /// 次に取りに行ってよい時刻
    static LAST_FETCH: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    // ほかのスレッドが取っているあいだも、止める印を見ながら待つ（遅いサイトでは1枚に1分かかることもある）
    let mut last = loop {
        match LAST_FETCH.try_lock() {
            Ok(last) => break last,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if stop() {
                    return None;
                }
                thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    };
    // 待っているあいだに、ほかのスレッドが同じ表紙を取ったかもしれない
    if let Ok(bytes) = std::fs::read(cache) {
        return Some(bytes);
    }
    if stop() || recently_failed(cache, url, unix_now()) {
        return None;
    }
    while last.is_some_and(|at| std::time::Instant::now() < at) {
        if stop() {
            return None;
        }
        thread::sleep(std::time::Duration::from_millis(100));
    }
    let started = std::time::Instant::now();
    let fetched = fetch_bytes(url);
    // 遅かったら（元のサーバーが働いたとみて）、終わってから長めに空ける
    let took = started.elapsed();
    *last = Some(match fetched {
        Fetched::TimedOut => std::time::Instant::now() + COVER_SLOW_PAUSE,
        _ if took >= COVER_SLOW => std::time::Instant::now() + COVER_SLOW_PAUSE,
        _ => started + COVER_FETCH_INTERVAL,
    });
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let timed_out = matches!(fetched, Fetched::TimedOut);
    let Fetched::Ok(bytes) = fetched else {
        note_fetch(cache, url, false, timed_out, unix_now());
        return None;
    };
    note_fetch(cache, url, true, false, unix_now());
    let _ = crate::engine::settings_store::write_atomic(cache, &bytes);
    Some(bytes)
}

/// 表紙の画像を取る。答えないサーバーで止まり続けないよう、`COVER_FETCH_TIMEOUT` で切る
fn fetch_bytes(url: &str) -> Fetched {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(COVER_FETCH_TIMEOUT))
        .build()
        .into();
    let read = agent.get(url).call().and_then(|mut response| {
        response
            .body_mut()
            .with_config()
            .limit(COVER_DOWNLOAD_LIMIT)
            .read_to_vec()
    });
    match read {
        Ok(bytes) => Fetched::Ok(bytes),
        Err(ureq::Error::Timeout(_)) => Fetched::TimedOut,
        Err(_) => Fetched::Failed,
    }
}

/// 名前の計算を変える前の、表紙のためておきの名前（Rust 標準のハッシュ。前に置いたものを探すためだけに使う）
fn legacy_cache_name(url: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 文字列から作る、16 桁の短い名前（表紙や一覧のファイル名、ダウンロードした本の印、アカウントの記録に使う）。
/// 手元に残る名前なので、Rust の版や作り直しで変わらないよう、決まった計算（FNV-1a、64 ビット）で作る
pub fn cover_cache_name(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// フォルダの中の冊（書庫・PDF・フォルダ）。ミーヤと同じく、フォルダを先に、それぞれ名前順に並べる。
/// 冊でないファイルは選んでも開けないので入れない（ミーヤは出す）
pub fn cover_entries(folder: &Path) -> Vec<CoverEntry> {
    let mut books = scan_folder(folder);
    // 並べ替えは安定なので、名前順のままフォルダが先になる
    books.sort_by_key(|path| !path.is_dir());
    books
        .into_iter()
        .map(|path| CoverEntry {
            is_folder: path.is_dir(),
            path,
            loose_pages: None,
            remote: None,
        })
        .collect()
}

/// 表紙サムネイルでフォルダを開いたときに並べるもの。中の冊に加えて、直下に画像もあれば
/// その画像だけを冊として読むマスを先頭に置く（冊の入ったフォルダは一覧として開くので、
/// 直下の画像はここから読む）
pub fn folder_shelf(folder: &Path) -> Vec<CoverEntry> {
    let mut entries = cover_entries(folder);
    let loose = list_images(folder, false).map_or(0, |listing| listing.images.len());
    if !entries.is_empty() && loose > 0 {
        entries.insert(
            0,
            CoverEntry {
                path: folder.to_path_buf(),
                is_folder: true,
                loose_pages: Some(loose),
                remote: None,
            },
        );
    }
    entries
}

/// 冊の表紙のバイト列。表紙は並び順で最初のページ。画像の無いフォルダは、中の最初の冊の表紙。
/// PDF は描けないので None
/// 表紙の作り方
#[derive(Clone, Default)]
pub struct CoverOptions {
    /// 壊れた Adobe のマーカーを直してから読むか
    pub fix_adobe: bool,
    /// PDF の表紙を描く pdfium。無ければ PDF の表紙は作らない
    pub pdfium: Option<SharedPdfium>,
    /// 利用者が選んだ表紙（冊の正規化したパス → `page_key` の形のページ）
    pub chosen: Arc<HashMap<PathBuf, String>>,
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

/// 選んだ表紙のページを読む。冊をサブフォルダごと読んだときに選んだページも探す
fn chosen_page_bytes(path: &Path, page: &str) -> Option<Vec<u8>> {
    [false, true].into_iter().find_map(|include_subfolders| {
        let listing = list_images(path, include_subfolders)?;
        let name = listing
            .images
            .iter()
            .find(|name| page_key(listing.mode, &listing.root, name) == page)?;
        SourceReader::open(listing.mode, &listing.root).read(name)
    })
}

/// 冊の表紙。選んだ表紙があればそのページ、無ければ最初のページ。PDF は描く。
/// 冊の入ったフォルダは、`depth` 段まで下の冊の表紙を探す
fn cover_image(path: &Path, depth: usize, options: &CoverOptions) -> Option<RgbaImage> {
    let chosen = options.chosen.get(&normalize(path));
    if is_pdf(path) {
        let index = chosen
            .and_then(|page| page.strip_prefix("pdf_page_")?.parse().ok())
            .unwrap_or(0);
        let side = THUMBNAIL_MAX_SIDE as f32;
        let image = options
            .pdfium?
            .open(path)?
            .render(index, side, Some(side))?;
        return Some(image.to_rgba8());
    }
    if let Some(bytes) = chosen.and_then(|page| chosen_page_bytes(path, page)) {
        return decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, options.fix_adobe);
    }
    if let Some(listing) = list_images(path, false)
        && !listing.images.is_empty()
    {
        let mut images = listing.images;
        natural_sort_strings(&mut images);
        let bytes = SourceReader::open(listing.mode, &listing.root).read(&images[0])?;
        return decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, options.fix_adobe);
    }
    if !path.is_dir() || depth == 0 {
        return None;
    }
    cover_entries(path)
        .iter()
        .find_map(|entry| cover_image(&entry.path, depth - 1, options))
}

/// 冊の表紙のサムネイル
pub fn cover_thumbnail(path: &Path, options: &CoverOptions) -> Option<RgbaImage> {
    cover_image(path, COVER_SEARCH_DEPTH, options)
}

/// 冊の表紙を作る順番。画面に見えている冊（`show` で知らせる）を先に、次にその下の冊、最後に上の冊を作る
pub struct CoverQueue {
    /// 作り始めた冊（番号は `spawn_cover_loader` に渡した並び）
    taken: std::sync::Mutex<Vec<bool>>,
    /// 見えている冊の範囲（start..end）
    start: AtomicUsize,
    end: AtomicUsize,
}

impl CoverQueue {
    pub fn new(count: usize) -> Self {
        Self {
            taken: std::sync::Mutex::new(vec![false; count]),
            start: AtomicUsize::new(0),
            end: AtomicUsize::new(count),
        }
    }

    /// 見えている冊の範囲を知らせる
    pub fn show(&self, start: usize, end: usize) {
        self.start.store(start, Ordering::Relaxed);
        self.end.store(end.max(start), Ordering::Relaxed);
    }

    fn taken(&self) -> std::sync::MutexGuard<'_, Vec<bool>> {
        self.taken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 次に作る冊
    fn take(&self) -> Option<usize> {
        let mut taken = self.taken();
        let start = self.start.load(Ordering::Relaxed).min(taken.len());
        let index = (start..taken.len())
            .chain(0..start)
            .find(|&index| !taken[index])?;
        taken[index] = true;
        Some(index)
    }

    /// 作り始めた冊を、あとで作り直すよう戻す
    fn put_back(&self, index: usize) {
        if let Some(slot) = self.taken().get_mut(index) {
            *slot = false;
        }
    }

    /// `index` の冊が見えておらず、見えている冊にまだ作り始めていないものがある（そちらを先にしたい）
    fn behind(&self, index: usize) -> bool {
        let (start, end) = (
            self.start.load(Ordering::Relaxed),
            self.end.load(Ordering::Relaxed),
        );
        if (start..end).contains(&index) {
            return false;
        }
        let taken = self.taken();
        let end = end.min(taken.len());
        taken[start.min(end)..end].iter().any(|taken| !taken)
    }
}

/// `take` が返す順に、`make_worker` で作った処理でサムネイルを作って送る。処理はスレッドごとに作る
/// （書庫を開いたまま続けて読めるように）。`cancel` を立てるか受け手を捨てると止まる
fn spawn_workers<T, F, W>(
    take: T,
    cancel: Arc<AtomicBool>,
    make_worker: F,
) -> Receiver<(usize, RgbaImage)>
where
    T: Fn() -> Option<usize> + Send + Sync + 'static,
    F: Fn() -> W + Send + Sync + 'static,
    W: FnMut(usize) -> Option<RgbaImage>,
{
    let (tx, rx) = channel();
    let take = Arc::new(take);
    let make_worker = Arc::new(make_worker);
    let workers = thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4);
    for _ in 0..workers {
        let (tx, take, cancel, make_worker) = (
            tx.clone(),
            take.clone(),
            cancel.clone(),
            make_worker.clone(),
        );
        thread::spawn(move || {
            let mut work = make_worker();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(index) = take() else {
                    return;
                };
                // 1冊で失敗して（パニックして）も、その冊を飛ばして続ける
                // （スレッドが黙って終わり、残りの表紙が作られなくならないように）
                let image =
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(index))) {
                        Ok(Some(image)) => image,
                        Ok(None) => continue,
                        Err(_) => {
                            eprintln!("サムネイルを作れませんでした（{index} 番目）");
                            continue;
                        }
                    };
                if tx.send((index, image)).is_err() {
                    return;
                }
            }
        });
    }
    rx
}

/// URL の表紙がある本の表紙。サイトにはなるべく取りに行かないよう、次の順に探す:
/// 選んだ表紙（ダウンロードした本）、手元に置いた表紙の画像、ダウンロードした本の最初のページ、サイト
fn url_cover(
    url: &str,
    cache: &Path,
    local: Option<&Path>,
    options: &CoverOptions,
    stop: &dyn Fn() -> bool,
) -> Option<RgbaImage> {
    let local = local.filter(|path| path.is_file());
    // 小さすぎる画像は表紙として使わない（Amazon などは、画像が無いと 1×1 の透明な GIF を返す）
    let decode = |bytes: Vec<u8>| {
        decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, options.fix_adobe)
            .filter(|image| image.width().min(image.height()) >= MIN_COVER_SIDE)
    };
    if let Some(local) = local
        && options.chosen.contains_key(&normalize(local))
        && let Some(image) = cover_thumbnail(local, options)
    {
        return Some(image);
    }
    if let Some(image) = cached_url_cover(url, cache).and_then(decode) {
        return Some(image);
    }
    if let Some(image) = local.and_then(|local| cover_thumbnail(local, options)) {
        return Some(image);
    }
    fetch_url_cover(url, cache, stop).and_then(decode)
}

/// 冊の表紙のサムネイルを、別スレッドで作って送る。(冊の番号, 画像) を送る。
/// `queue` の順に作る。サイトに取りに行く順番を待っているあいだに、見えている冊が変わったら、
/// その冊は後回しにして、見えている冊を先に取る
pub fn spawn_cover_loader(
    books: Vec<CoverSource>,
    options: CoverOptions,
    cancel: Arc<AtomicBool>,
    queue: Arc<CoverQueue>,
) -> Receiver<(usize, RgbaImage)> {
    let books = Arc::new(books);
    let stop = cancel.clone();
    let take = {
        let queue = queue.clone();
        move || queue.take()
    };
    spawn_workers(take, cancel, move || {
        let (books, options, stop, queue) =
            (books.clone(), options.clone(), stop.clone(), queue.clone());
        move |index| match books.get(index)? {
            CoverSource::Local(path) => cover_thumbnail(path, &options),
            CoverSource::Url { url, cache, local } => {
                let later = std::cell::Cell::new(false);
                let stop_or_later = || {
                    if stop.load(Ordering::Relaxed) {
                        return true;
                    }
                    let behind = queue.behind(index);
                    later.set(behind);
                    behind
                };
                let image = url_cover(url, cache, local.as_deref(), &options, &stop_or_later);
                if later.get() && image.is_none() && !stop.load(Ordering::Relaxed) {
                    queue.put_back(index);
                }
                image
            }
        }
    })
}

/// 書庫やフォルダのページのサムネイルを、別スレッドで作って送る。
/// `indices` の順に作り、(ページ番号, 画像) を送る。読めなかったページは送らない。
/// `cancel` を立てるか受け手を捨てると止まる。PDF は扱わない（描画を別スレッドに出せないため）
pub fn spawn_loader(
    mode: SourceMode,
    root: PathBuf,
    names: Vec<String>,
    indices: Vec<usize>,
    fix_adobe: bool,
    cancel: Arc<AtomicBool>,
) -> Receiver<(usize, RgbaImage)> {
    if mode == SourceMode::Pdf {
        return channel().1;
    }
    let names = Arc::new(names);
    let next = AtomicUsize::new(0);
    let take = move || indices.get(next.fetch_add(1, Ordering::Relaxed)).copied();
    spawn_workers(take, cancel, move || {
        let names = names.clone();
        // 書庫は開いたまま続けて読む
        let mut reader = SourceReader::open(mode, &root);
        move |index| {
            let bytes = reader.read(names.get(index)?)?;
            decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, fix_adobe)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手元に残る名前は、作り直しても変わらない（変わると、ダウンロードした本や記録が浮いてしまう）
    #[test]
    fn short_names_stay_the_same_across_builds() {
        assert_eq!(cover_cache_name(""), "cbf29ce484222325");
        assert_eq!(cover_cache_name("a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn fit_all_matches_mangameeya_for_sixty_pages() {
        // 1900x960 の画面に 60 ページなら 10 列 × 6 行
        let grid = Grid::new(ThumbnailMode::FitAll, 60, [1900.0, 960.0]);
        assert_eq!((grid.columns, grid.rows), (10, 6));
        assert_eq!(grid.cell, 160.0);
        let [width, height] = grid.size();
        assert!(width <= 1900.0 && height <= 960.0);
    }

    #[test]
    fn large_cells_show_about_three_rows_and_fill_the_width() {
        // 同じ画面で「8列」は 1 マス約 316 の 6 列で、3 行と少しが見えた
        let grid = Grid::new(ThumbnailMode::Large, 60, [1897.0, 964.0]);
        assert_eq!((grid.columns, grid.rows), (6, 10));
        assert!((grid.cell - 1897.0 / 6.0).abs() < 0.01);
        // 縦長の画面では列が減る
        let grid = Grid::new(ThumbnailMode::Large, 60, [800.0, 1200.0]);
        assert_eq!(grid.columns, 2);
        assert_eq!(
            Grid::new(ThumbnailMode::Large, 60, [100.0, 2000.0]).columns,
            1
        );
    }

    #[test]
    fn cells_are_found_by_position() {
        let grid = Grid {
            columns: 4,
            rows: 3,
            cell: 10.0,
        };
        assert_eq!(grid.cell_origin(5), [10.0, 10.0]);
        assert_eq!(grid.index_at([15.0, 15.0], 10), Some(5));
        assert_eq!(grid.index_at([45.0, 5.0], 10), None);
        // 最後の行の欠けているところ
        assert_eq!(grid.index_at([25.0, 25.0], 10), None);
        assert_eq!(grid.index_at([-1.0, 5.0], 10), None);
    }

    #[test]
    fn selection_moves_within_the_pages() {
        use Direction::*;
        // 4 列、10 枚: 0..3 / 4..7 / 8 9
        assert_eq!(move_selection(0, Left, 4, 10), 0);
        assert_eq!(move_selection(3, Right, 4, 10), 4);
        assert_eq!(move_selection(9, Right, 4, 10), 9);
        assert_eq!(move_selection(2, Up, 4, 10), 2);
        assert_eq!(move_selection(6, Up, 4, 10), 2);
        assert_eq!(move_selection(5, Down, 4, 10), 9);
        assert_eq!(move_selection(6, Down, 4, 10), 9);
        assert_eq!(move_selection(9, Down, 4, 10), 9);
        assert_eq!(move_selection(0, Down, 4, 0), 0);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    /// 300×400 と 400×300 の2ページの PDF
    fn two_page_pdf() -> Vec<u8> {
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 300] >>".to_owned(),
        ];
        let mut pdf = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.push_str(&format!("{} 0 obj\n{object}\nendobj\n", i + 1));
        }
        let xref = pdf.len();
        pdf.push_str(&format!(
            "xref\n0 {}\n0000000000 65535 f \n",
            objects.len() + 1
        ));
        for offset in offsets {
            pdf.push_str(&format!("{offset:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        ));
        pdf.into_bytes()
    }

    /// PDF の表紙は1ページ目（選んだ表紙があればそのページ）を描く。pdfium が無い環境では確かめない
    #[test]
    fn pdf_covers_are_rendered_from_the_chosen_or_first_page() {
        let Some(pdfium) = crate::engine::pdf::pdfium() else {
            return;
        };
        let root = std::env::temp_dir().join(format!("manga-pdf-cover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let book = root.join("book.pdf");
        std::fs::write(&book, two_page_pdf()).unwrap();
        let options = |chosen: Option<&str>| CoverOptions {
            pdfium: Some(pdfium),
            chosen: Arc::new(
                chosen
                    .map(|page| (normalize(&book), page.to_owned()))
                    .into_iter()
                    .collect(),
            ),
            ..CoverOptions::default()
        };
        let size = |options: &CoverOptions| cover_thumbnail(&book, options).map(|t| t.dimensions());
        assert_eq!(size(&options(None)), Some((240, 320)));
        assert_eq!(size(&options(Some("pdf_page_1"))), Some((320, 240)));
        // pdfium が無ければ作らない
        assert_eq!(size(&CoverOptions::default()), None);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 取れなかった表紙は1日（時間切れは7日）覚えておき、そのあいだは取りに行かない
    #[test]
    fn failed_cover_fetches_are_not_retried_for_a_day() {
        let dir = std::env::temp_dir().join(format!("manga-cover-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cover.jpg");
        let url = "https://example.invalid/a.jpg";
        assert!(!recently_failed(&cache, url, 1000));
        note_fetch(&cache, url, false, false, 1000);
        assert!(recently_failed(&cache, url, 1000 + COVER_RETRY_AFTER - 1));
        assert!(!recently_failed(&cache, url, 1000 + COVER_RETRY_AFTER));
        // 取れたら記録から外す
        note_fetch(&cache, url, true, false, 2000);
        assert!(!recently_failed(&cache, url, 2000));
        // 時間切れは7日覚えておく。取れたら外す
        note_fetch(&cache, url, false, true, 3000);
        assert!(recently_failed(&cache, url, 3000 + COVER_RETRY_AFTER));
        assert!(recently_failed(
            &cache,
            url,
            3000 + COVER_TIMEOUT_RETRY_AFTER - 1
        ));
        assert!(!recently_failed(
            &cache,
            url,
            3000 + COVER_TIMEOUT_RETRY_AFTER
        ));
        // 古い記録を捨てるときも、時間切れの記録は7日までは残す
        note_fetch(
            &cache,
            "https://example.invalid/b.jpg",
            false,
            false,
            3000 + COVER_RETRY_AFTER,
        );
        assert!(recently_failed(&cache, url, 3000 + COVER_RETRY_AFTER));
        note_fetch(&cache, url, true, false, 4000);
        assert!(!recently_failed(&cache, url, 4000));
        // 最近取れなかった表紙は、サイトに取りに行かない（ここでは通信しない）
        note_fetch(&cache, url, false, false, unix_now());
        let started = std::time::Instant::now();
        assert_eq!(fetch_url_cover(url, &cache, &|| false), None);
        assert!(started.elapsed() < COVER_FETCH_INTERVAL);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// URL の表紙がある本は、選んだ表紙、手元の表紙の画像、ダウンロードした本の順に探す（ここでは通信しない）
    #[test]
    fn url_covers_prefer_chosen_pages_then_cached_images_then_downloads() {
        let dir = std::env::temp_dir().join(format!("manga-url-cover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("book.zip");
        write_zip(&book, &[("01.png", png(10, 20)), ("02.png", png(20, 10))]);
        let cache = dir.join("cover.png");
        let url = "unreachable:cover.png";
        let cover = |options: &CoverOptions| {
            url_cover(url, &cache, Some(&book), options, &|| false).map(|image| image.dimensions())
        };
        let plain = CoverOptions::default();
        // 手元に表紙の画像が無ければ、ダウンロードした本の最初のページ
        assert_eq!(cover(&plain), Some((10, 20)));
        // 表紙の画像があれば、それ
        std::fs::write(&cache, png(30, 30)).unwrap();
        assert_eq!(cover(&plain), Some((30, 30)));
        // 「画像なし」の印（1×1 の画像）は表紙にせず、ダウンロードした本から作る
        std::fs::write(&cache, png(1, 1)).unwrap();
        assert_eq!(cover(&plain), Some((10, 20)));
        assert_eq!(url_cover(url, &cache, None, &plain, &|| false), None);
        // 表紙を選んであれば、それがいちばん先
        let chosen = CoverOptions {
            chosen: Arc::new([(normalize(&book), "02.png".to_owned())].into()),
            ..CoverOptions::default()
        };
        assert_eq!(cover(&chosen), Some((20, 10)));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 選んだ表紙があれば、そのページを表紙にする（書庫・フォルダ・サブフォルダの中のページ）
    #[test]
    fn chosen_pages_become_the_cover() {
        let root = std::env::temp_dir().join(format!("manga-chosen-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder/sub")).unwrap();
        let book = root.join("book.zip");
        write_zip(&book, &[("01.png", png(10, 20)), ("02.png", png(30, 10))]);
        std::fs::write(root.join("folder/01.png"), png(10, 20)).unwrap();
        std::fs::write(root.join("folder/sub/02.png"), png(40, 10)).unwrap();
        let folder = root.join("folder");
        let options = |choices: &[(&Path, &str)]| CoverOptions {
            chosen: Arc::new(
                choices
                    .iter()
                    .map(|(path, page)| (normalize(path), page.to_string()))
                    .collect(),
            ),
            ..CoverOptions::default()
        };
        let size = |path: &Path, options: &CoverOptions| {
            cover_thumbnail(path, options).map(|t| t.dimensions())
        };
        assert_eq!(size(&book, &options(&[])), Some((10, 20)));
        assert_eq!(size(&book, &options(&[(&book, "02.png")])), Some((30, 10)));
        // フォルダは冊のフォルダからの相対パス。サブフォルダごと読んだときに選んだページも探す
        let sub_page = page_key(
            SourceMode::Folder,
            &folder,
            &folder.join("sub").join("02.png").to_string_lossy(),
        );
        assert_eq!(sub_page, Path::new("sub").join("02.png").to_string_lossy());
        assert_eq!(
            size(&folder, &options(&[(&folder, &sub_page)])),
            Some((40, 10))
        );
        // 選んだページが無くなっていれば、最初のページ
        assert_eq!(
            size(&book, &options(&[(&book, "gone.png")])),
            Some((10, 20))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cover_entries_put_folders_first_and_covers_are_first_pages() {
        let root = std::env::temp_dir().join(format!("manga-cover-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("03 C")).unwrap();
        std::fs::create_dir_all(root.join("04 shelf")).unwrap();
        // 書庫の中では後に入れたものが並び順で先（表紙）になる
        write_zip(
            &root.join("01 A.zip"),
            &[("02.png", png(30, 10)), ("01.png", png(10, 20))],
        );
        write_zip(&root.join("02 B.zip"), &[("01.png", png(10, 30))]);
        std::fs::write(root.join("03 C/01.png"), png(40, 10)).unwrap();
        write_zip(&root.join("04 shelf/D.zip"), &[("01.png", png(10, 50))]);
        std::fs::write(root.join("memo.txt"), "memo").unwrap();

        // フォルダが先、それぞれ名前順。冊でないファイルは入れない
        let names: Vec<_> = cover_entries(&root)
            .iter()
            .map(|entry| {
                (
                    entry
                        .path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                    entry.is_folder,
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("03 C".to_string(), true),
                ("04 shelf".to_string(), true),
                ("01 A.zip".to_string(), false),
                ("02 B.zip".to_string(), false),
            ]
        );
        let size = |path: &str| {
            cover_thumbnail(&root.join(path), &CoverOptions::default()).map(|t| t.dimensions())
        };
        assert_eq!(size("01 A.zip"), Some((10, 20)));
        assert_eq!(size("03 C"), Some((40, 10)));
        // 画像の無いフォルダは、中の最初の冊の表紙
        assert_eq!(size("04 shelf"), Some((10, 50)));
        assert_eq!(size("memo.txt"), None);

        // 直下に画像が無ければ、画像を読むマスは置かない。あれば先頭に置く
        assert_eq!(folder_shelf(&root), cover_entries(&root));
        std::fs::write(root.join("cover.png"), png(10, 10)).unwrap();
        let shelf = folder_shelf(&root);
        assert_eq!(shelf.len(), 5);
        assert_eq!(shelf[0].path, root);
        assert_eq!(shelf[0].loose_pages, Some(1));
        assert_eq!(&shelf[1..], cover_entries(&root));
        // 画像だけのフォルダには置かない
        assert_eq!(folder_shelf(&root.join("03 C")), []);
        std::fs::remove_file(root.join("cover.png")).unwrap();

        // 別スレッドでも同じものが届く
        let mut books: Vec<_> = cover_entries(&root)
            .into_iter()
            .map(|e| CoverSource::Local(e.path))
            .collect();
        // URL の表紙は、手元に置いたものがあればそれを使う（ここでは通信しない）
        let cache = root.join("cover-cache.png");
        std::fs::write(&cache, png(30, 20)).unwrap();
        books.push(CoverSource::Url {
            url: "https://example.invalid/cover.png".to_owned(),
            cache,
            local: None,
        });
        // URL で取れなければ、手元の本から作る（http でない URL は取りに行かないので、通信しない）
        books.push(CoverSource::Url {
            url: "unreachable:cover.png".to_owned(),
            cache: root.join("missing-cache.png"),
            local: Some(root.join("02 B.zip")),
        });
        let count = books.len();
        let rx = spawn_cover_loader(
            books,
            CoverOptions::default(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(CoverQueue::new(count)),
        );
        let mut received: Vec<_> = rx
            .iter()
            .map(|(index, image)| (index, image.dimensions()))
            .collect();
        received.sort();
        assert_eq!(
            received,
            [
                (0, (40, 10)),
                (1, (10, 50)),
                (2, (10, 20)),
                (3, (10, 30)),
                (4, (30, 20)),
                (5, (10, 30))
            ]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// 表紙は、見えている冊、その下の冊、上の冊の順に作る。見えていない冊は、見えている冊を先にするため後回しにできる
    #[test]
    fn covers_are_made_from_the_visible_books_first() {
        let queue = CoverQueue::new(6);
        queue.show(2, 4);
        assert_eq!(queue.take(), Some(2));
        assert!(!queue.behind(2));
        // 見えている冊が残っているあいだ、見えていない冊は後回しにできる
        assert!(queue.behind(0));
        assert_eq!(queue.take(), Some(3));
        assert!(!queue.behind(0));
        assert_eq!(queue.take(), Some(4));
        // スクロールしたら、そこから
        queue.show(0, 2);
        assert!(queue.behind(4));
        queue.put_back(4);
        assert_eq!(queue.take(), Some(0));
        assert_eq!(queue.take(), Some(1));
        assert_eq!(queue.take(), Some(4));
        assert_eq!(queue.take(), Some(5));
        assert_eq!(queue.take(), None);
    }

    #[test]
    fn thumbnails_keep_the_aspect_ratio() {
        let mut bytes = Vec::new();
        image::RgbaImage::new(800, 1200)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let thumb = decode_thumbnail(&bytes, 300, false).unwrap();
        assert_eq!(thumb.dimensions(), (200, 300));
        // 小さい画像は拡大しない
        let mut small = Vec::new();
        image::RgbaImage::new(10, 20)
            .write_to(&mut std::io::Cursor::new(&mut small), ImageFormat::Png)
            .unwrap();
        assert_eq!(
            decode_thumbnail(&small, 300, false).unwrap().dimensions(),
            (10, 20)
        );
        assert!(decode_thumbnail(b"not an image", 300, false).is_none());
    }
}
