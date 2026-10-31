//! 本の文字（いまは PDF の透明テキスト）を取り出し、ためておき、検索する。
//! 取り出した文字は本ごとにファイルにためておき、本が変わっていなければ次からはそれを使う。

use crate::config::SourceMode;
use crate::engine::pdf::{PdfDoc, SharedPdfium};
use crate::engine::plugin::{OcrImage, PluginError, PluginManifest, ocr_images};
use crate::engine::source::SourceReader;
use crate::engine::thumbnail::cover_cache_name;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 本の、ページごとの文字
#[derive(Default, Debug, PartialEq)]
pub struct BookText {
    pub pages: Vec<String>,
    /// 文字を読めなかったページの数（そのページは探せない）
    pub failed: usize,
    /// OCR で読んだときは、そのプラグインの id
    pub ocr: Option<String>,
}

impl BookText {
    /// 文字が1つも無いか（透明テキストの無い本）
    pub fn is_empty(&self) -> bool {
        self.pages.iter().all(|page| page.trim().is_empty())
    }
}

/// 取り出しの進み具合（済んだページ数, 全ページ数）
#[derive(Default)]
pub struct TextProgress {
    pub done: AtomicUsize,
    pub total: AtomicUsize,
}

/// ためておく本の数の上限。超えたら、長く使っていないものから消す
/// （消したり動かしたりした本のぶんが、たまり続けないように）。
/// OCR の結果は、読み直すのに時間がかかるので、別のフォルダに置いて上限の外にする
const CACHE_LIMIT: usize = 1000;

/// PDF の埋め込まれた文字をためるフォルダ（上限まで）と、OCR の結果をためるフォルダ（上限なし）
pub const TEXT_DIR: &str = "text";
pub const OCR_TEXT_DIR: &str = "text-ocr";

/// ためておくファイルの中身。本が変わったか見分けるため、大きさと更新日時も書く
#[derive(Serialize, Deserialize)]
pub(crate) struct CacheFile {
    /// 本の場所（横断検索で、どの本の文字かを引くのに使う。前の版で書いたものには無い）
    #[serde(default)]
    pub book: String,
    pub size: u64,
    pub modified: u64,
    /// どう読んだか（"pdf" は透明テキスト、"ocr:<プラグインの id>" は OCR）
    #[serde(default)]
    pub source: String,
    /// 読んだときのページの並び（`PageSource::key`）。今の並びと違えば使わない
    #[serde(default)]
    pub pages_key: String,
    /// 本の中身から作った値（`fingerprint` と `digest`）。OCR の結果にだけ書く。本を動かしたり、
    /// 別の機械から写して更新日時が変わったりしても、同じ本と見分けられるように
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub digest: String,
    pub pages: Vec<String>,
    /// OCR で読めなかったページの数（そのページは空）
    #[serde(default)]
    pub failed: usize,
}

/// 本の版（大きさ, 更新日時）
pub(crate) fn version(book: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(book).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some((meta.len(), modified))
}

/// 本の中身から作る値で、先頭と末尾から読む大きさ
const FINGERPRINT_SPAN: u64 = 64 * 1024;

/// 本の中身の目安になる値（大きさと、先頭と末尾を少しずつ読んだハッシュ）。大きな本でもすぐ作れるので、
/// 動かした本の OCR の結果を探す鍵に使う（同じ本と決めるのは `digest` で確かめてから）。
/// フォルダの本は None（ページの並びに画像の場所が入るので、動かすと合わない）
pub(crate) fn fingerprint(book: &Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(book).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    let size = meta.len();
    let mut bytes = Vec::new();
    (&mut file)
        .take(FINGERPRINT_SPAN)
        .read_to_end(&mut bytes)
        .ok()?;
    if size > FINGERPRINT_SPAN {
        file.seek(SeekFrom::Start(
            size.saturating_sub(FINGERPRINT_SPAN).max(FINGERPRINT_SPAN),
        ))
        .ok()?;
        file.take(FINGERPRINT_SPAN).read_to_end(&mut bytes).ok()?;
    }
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    });
    Some(format!("{size:x}-{hash:016x}"))
}

/// 本の中身全体のハッシュ（SHA-256）。本全体を読むので、OCR の結果を書くときと、
/// 動かしたり写したりした本の結果を使ってよいか確かめるときだけ作る
fn digest(book: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(book).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).ok()?;
    Some(format!("{:x}", hasher.finalize()))
}

fn read_cache_file(path: &Path) -> Option<CacheFile> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write_cache_file(path: &Path, cache: &CacheFile) -> bool {
    serde_json::to_string(cache).is_ok_and(|json| {
        crate::engine::settings_store::write_atomic(path, json.as_bytes()).is_ok()
    })
}

/// ためておきを書いたり消したりするときに取る鍵（OCR の結果を書くのと、書き足しや付け替えが重ならないよう）
static CACHE_LOCK: Mutex<()> = Mutex::new(());

fn cache_lock() -> MutexGuard<'static, ()> {
    CACHE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// ためておきのファイルの中身。読んでから書くまでに、ほかで書き換えられていないかを見るのに使う
/// （使ったときに時刻だけを新しくすることがあるので、更新日時では見ない）
fn read_raw(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn parse_cache(raw: &str) -> Option<CacheFile> {
    serde_json::from_str(raw).ok()
}

/// 読んだとき（`raw`）から変わっていなければ、`cache` で書き直す
fn rewrite_unchanged(path: &Path, raw: &str, cache: &CacheFile) -> bool {
    let _lock = cache_lock();
    read_raw(path).as_deref() == Some(raw) && write_cache_file(path, cache)
}

/// 値を書き足す前に本を動かした結果を、一覧で引くための鍵（本の大きさと更新日時）。
/// 同じファイルシステムの中で動かしたときや Dropbox で写したときは、更新日時も変わらない
fn legacy_key((size, modified): (u64, u64)) -> String {
    format!("{size}:{modified}")
}

/// OCR の結果の一覧の1つ（読んだときのファイルの版と、引くのに使うもの）
struct IndexEntry {
    stamp: (SystemTime, u64),
    fingerprint: String,
    book: String,
    version: (u64, u64),
}

impl IndexEntry {
    /// 引く鍵。値があればそれを、無ければ（前の版で書き、値を書き足す前に本を動かしたもの）本の版を使う。
    /// 本が元の場所にあれば、動かしたものではないので引かない（本の有る無しは、引くたびに見る）
    fn key(&self) -> Option<String> {
        if !self.fingerprint.is_empty() {
            return Some(self.fingerprint.clone());
        }
        (!self.book.is_empty() && !Path::new(&self.book).exists()).then(|| legacy_key(self.version))
    }
}

/// OCR の結果の一覧（本を動かしたとき、前の場所の結果を探すのに使う）。
/// 引くたびにファイルの版を見て、変わったものだけを読み直す
static OCR_INDEX: LazyLock<Mutex<HashMap<PathBuf, IndexEntry>>> = LazyLock::new(Default::default);

/// 一覧で `keys`（中身の目安の値か `legacy_key`）に当たる、OCR の結果のファイル
fn ocr_results_for(cache_dir: &Path, keys: &[&str]) -> Vec<PathBuf> {
    let dir = cache_dir.join(OCR_TEXT_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    let present: HashSet<&PathBuf> = paths.iter().collect();
    let mut index = OCR_INDEX.lock().unwrap_or_else(PoisonError::into_inner);
    index.retain(|path, _| path.parent() != Some(dir.as_path()) || present.contains(path));
    let mut found = Vec::new();
    for path in &paths {
        let Ok(meta) = std::fs::metadata(path) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let stamp = (modified, meta.len());
        if index.get(path).is_none_or(|entry| entry.stamp != stamp) {
            let Some(cache) = read_cache_file(path) else {
                index.remove(path);
                continue;
            };
            index.insert(
                path.clone(),
                IndexEntry {
                    stamp,
                    fingerprint: cache.fingerprint,
                    book: cache.book,
                    version: (cache.size, cache.modified),
                },
            );
        }
        if index[path]
            .key()
            .is_some_and(|key| keys.contains(&key.as_str()))
        {
            found.push(path.clone());
        }
    }
    found
}

/// 前の版で書いた OCR の結果に、本の中身から作った値を書き足す（本が元の場所にあり、変わっていないものだけ）。
/// 書き足しておけば、そのあと本を動かしても見つかる
fn backfill_ocr_results(cache_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(cache_dir.join(OCR_TEXT_DIR)) else {
        return;
    };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Some(raw) = read_raw(&path) else {
            continue;
        };
        let Some(mut cache) = parse_cache(&raw) else {
            continue;
        };
        let book = Path::new(&cache.book);
        let in_place = || version(book) == Some((cache.size, cache.modified));
        if !cache.fingerprint.is_empty() && !cache.digest.is_empty()
            || cache.book.is_empty()
            || !in_place()
        {
            continue;
        }
        let (Some(value), Some(whole)) = (fingerprint(book), digest(book)) else {
            continue;
        };
        // 読んでいるあいだに本が書き換えられていたら書かない
        if !in_place() {
            continue;
        }
        cache.fingerprint = value;
        cache.digest = whole;
        rewrite_unchanged(&path, &raw, &cache);
    }
}

/// 起動したとき、前の版で書いた OCR の結果に値を書き足し、一覧を作っておく（裏で）
pub fn spawn_ocr_index(cache_dir: PathBuf) {
    std::thread::spawn(move || {
        backfill_ocr_results(&cache_dir);
        ocr_results_for(&cache_dir, &[]);
    });
}

/// ためておいたものが、今の本（版が `current`、ページの並びが `pages_key`）のものか。
/// 大きさと更新日時が同じか、OCR の結果なら中身が同じなら使う（別の機械から写して、更新日時が違うときなど）
fn matches_book(cache: &CacheFile, book: &Path, current: (u64, u64), pages_key: &str) -> bool {
    if cache.pages_key != pages_key {
        return false;
    }
    if (cache.size, cache.modified) == current {
        return true;
    }
    !cache.fingerprint.is_empty()
        && cache.size == current.0
        && fingerprint(book).as_ref() == Some(&cache.fingerprint)
        && (cache.digest.is_empty() || digest(book).as_ref() == Some(&cache.digest))
}

/// この本の場所に使えるものが無いとき、中身の同じ本の OCR の結果を探して、この場所のものにする
/// （本を動かしたり、写したりしたとき）。元の本が元の場所に無ければ（動かしたとき）、元の結果は消す。
/// 中身全体のハッシュ（`digest`）で確かめ、書く結果にも入れる。ハッシュの無い結果は、元の本が残っていれば
/// その本のハッシュと比べ、無ければ目安の値で（前の版で書き、値を書き足す前に動かしたものは大きさと更新日時で）見分ける。
/// `seen` は、探すと決めたときのこの場所のためておきの中身（OCR の結果, 埋め込まれた文字）。
/// 探しているあいだにほかで書かれていたらやめる
fn adopt_ocr_result(
    book: &Path,
    cache_dir: &Path,
    pages_key: &str,
    current: (u64, u64),
    seen: (&Option<String>, &Option<String>),
) -> bool {
    let Some(value) = fingerprint(book) else {
        return false;
    };
    let ocr_path = cache_path(book, cache_dir, true);
    let text_path = cache_path(book, cache_dir, false);
    let mut whole: Option<Option<String>> = None;
    for file in ocr_results_for(cache_dir, &[&value, &legacy_key(current)]) {
        if file == ocr_path {
            continue;
        }
        let Some(raw) = read_raw(&file) else {
            continue;
        };
        let Some(mut cache) = parse_cache(&raw) else {
            continue;
        };
        if cache.pages_key != pages_key {
            continue;
        }
        let Some(whole) = whole.get_or_insert_with(|| digest(book)).clone() else {
            return false;
        };
        let old_book = Path::new(&cache.book);
        let old_in_place = version(old_book) == Some((cache.size, cache.modified));
        let same_book = if !cache.digest.is_empty() {
            cache.fingerprint == value && cache.digest == whole
        } else if cache.fingerprint.is_empty() {
            !cache.book.is_empty() && !old_book.exists() && (cache.size, cache.modified) == current
        } else if old_in_place {
            cache.fingerprint == value && digest(old_book).as_ref() == Some(&whole)
        } else {
            cache.fingerprint == value
        };
        if !same_book {
            continue;
        }
        // 元の本が残っていれば写したもの（元の結果も残す）。更新日時だけが変わっていても、中身が同じなら残っている
        let copied = old_in_place
            || (!cache.book.is_empty()
                && old_book.is_file()
                && digest(old_book).as_ref() == Some(&whole));
        cache.book = book.to_string_lossy().into_owned();
        (cache.size, cache.modified) = current;
        cache.fingerprint = value;
        cache.digest = whole;
        let _lock = cache_lock();
        if (&read_raw(&ocr_path), &read_raw(&text_path)) != seen
            || read_raw(&file).as_deref() != Some(raw.as_str())
            || !write_cache_file(&ocr_path, &cache)
        {
            return false;
        }
        let _ = std::fs::remove_file(&text_path);
        if !copied {
            let _ = std::fs::remove_file(&file);
        }
        return true;
    }
    false
}

/// ためておくファイル。本の場所ごとに1つ（本が変わったら書き直す）。`ocr` なら OCR の結果のフォルダ
fn cache_path(book: &Path, cache_dir: &Path, ocr: bool) -> PathBuf {
    cache_dir
        .join(if ocr { OCR_TEXT_DIR } else { TEXT_DIR })
        .join(format!(
            "{}.json",
            cover_cache_name(&book.to_string_lossy())
        ))
}

/// ためておいた文字。OCR の結果があればそれを、無ければ PDF の埋め込まれた文字を使う。
/// どちらも無ければ（埋め込まれた文字が無い PDF も）、動かしたり写したりした本の OCR の結果を探す。
/// 本が変わっていれば（ページの並びが `pages_key` と違えば）None。使ったものとして、ファイルの時刻を新しくする
fn load_cache(book: &Path, cache_dir: &Path, pages_key: &str) -> Option<BookText> {
    let current = version(book)?;
    let ocr_path = cache_path(book, cache_dir, true);
    let text_path = cache_path(book, cache_dir, false);
    // 読んだ中身で決め、探すときも同じ中身を元にする（読んでから探すまでに書かれたものを上書きしないよう）
    let valid = |raw: &Option<String>| {
        let raw = raw.as_deref()?;
        let cache = parse_cache(raw)?;
        matches_book(&cache, book, current, pages_key).then(|| (raw.to_owned(), cache))
    };
    let has_text = |cache: &CacheFile| {
        cache.source.starts_with("ocr:") || cache.pages.iter().any(|page| !page.trim().is_empty())
    };
    let ocr_raw = read_raw(&ocr_path);
    let found = match valid(&ocr_raw) {
        Some(found) => Some((ocr_path.clone(), found)),
        None => {
            let text_raw = read_raw(&text_path);
            match valid(&text_raw) {
                // 埋め込まれた文字があれば探さない（埋め込まれた文字に戻した本に、写しの OCR の結果を戻さないよう）
                Some(text) if has_text(&text.1) => Some((text_path.clone(), text)),
                text => {
                    adopt_ocr_result(book, cache_dir, pages_key, current, (&ocr_raw, &text_raw));
                    // 付け替えたもの（ほかで同時に付け替えたものも）があればそれを使う
                    match valid(&read_raw(&ocr_path)) {
                        Some(found) => Some((ocr_path.clone(), found)),
                        None => text.map(|text| (text_path.clone(), text)),
                    }
                }
            }
        }
    };
    let (path, (raw, mut cache)) = found?;
    // 前の版で、上限のあるフォルダに置いた OCR の結果は、上限の外に移す
    let path = if cache.source.starts_with("ocr:") && path != ocr_path {
        let _ = std::fs::create_dir_all(cache_dir.join(OCR_TEXT_DIR));
        let _lock = cache_lock();
        if read_raw(&path).as_deref() == Some(raw.as_str())
            && std::fs::rename(&path, &ocr_path).is_ok()
        {
            ocr_path
        } else {
            path
        }
    } else {
        path
    };
    // 前の版で書いたものには、本の場所（横断検索で引くのに使う）や本の中身の目安の値が無い。
    // 書き足す（書き直すので時刻も新しくなる）。写した本なら、今の更新日時にする
    let book_name = book.to_string_lossy();
    let mut changed = (cache.size, cache.modified) != current;
    (cache.size, cache.modified) = current;
    if cache.book != book_name {
        cache.book = book_name.into_owned();
        changed = true;
    }
    if cache.source.starts_with("ocr:")
        && cache.fingerprint.is_empty()
        && let Some(value) = fingerprint(book)
    {
        cache.fingerprint = value;
        changed = true;
    }
    let rewritten = changed && rewrite_unchanged(&path, &raw, &cache);
    if !rewritten && let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(BookText {
        pages: cache.pages,
        failed: cache.failed,
        ocr: cache.source.strip_prefix("ocr:").map(str::to_owned),
    })
}

/// 取り出した文字をためておき、上限を超えたら古いものから消す
fn save_cache(book: &Path, cache_dir: &Path, text: &BookText, source: &str, pages_key: &str) {
    let Some((size, modified)) = version(book) else {
        return;
    };
    let ocr = source.starts_with("ocr:");
    let path = cache_path(book, cache_dir, ocr);
    // OCR の結果には本の中身の値を書く。作っているあいだに本が書き換えられていたら書かない
    // （ほかの本の値を付けないよう。版も古いので、この結果はもう使われない）
    let (fingerprint, digest) = if ocr {
        let values = (fingerprint(book), digest(book));
        match values {
            (Some(value), Some(whole)) if version(book) == Some((size, modified)) => (value, whole),
            _ => Default::default(),
        }
    } else {
        Default::default()
    };
    let cache = CacheFile {
        book: book.to_string_lossy().into_owned(),
        size,
        modified,
        source: source.to_owned(),
        pages_key: pages_key.to_owned(),
        fingerprint,
        digest,
        pages: text.pages.clone(),
        failed: text.failed,
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
    {
        let _lock = cache_lock();
        if write_cache_file(&path, &cache) {
            // もう片方のフォルダの古い結果は消す（OCR の結果をやめて埋め込まれた文字に戻したときなど）
            let _ = std::fs::remove_file(cache_path(book, cache_dir, !ocr));
        }
    }
    prune_cache(&cache_dir.join(TEXT_DIR), CACHE_LIMIT);
}

/// `dir` の、ためておいた文字のファイルを `limit` 個まで残し、長く使っていないものから消す
fn prune_cache(dir: &Path, limit: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    if files.len() <= limit {
        return;
    }
    files.sort();
    for (_, path) in &files[..files.len() - limit] {
        let _ = std::fs::remove_file(path);
    }
}

/// PDF の文字を取り出す。止められたら None。`use_cache` なら、ためておいたもの（OCR で読み直したものも）を使う。
/// 読めなかったページがあれば、ためておかない（次に開いたときにまた取り出す）。
/// 答えの2つ目は、PDF に埋め込まれた文字があるか
fn pdf_text(
    pdfium: SharedPdfium,
    book: &Path,
    cache_dir: &Path,
    progress: &TextProgress,
    cancel: &AtomicBool,
    use_cache: bool,
    pace: Duration,
) -> Option<(BookText, bool)> {
    // PDF のページの並びは、ファイルの版で決まる
    if use_cache && let Some(text) = load_cache(book, cache_dir, "") {
        if text.ocr.is_none() {
            let layer = !text.is_empty();
            return Some((text, layer));
        }
        // OCR で読み直したものなら、元に戻せるよう、埋め込まれた文字があるかを調べる（見つかったら止める）
        let document = pdfium.open(book)?;
        let mut layer = false;
        for index in 0..document.page_count() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            if document
                .page_text(index)
                .is_some_and(|page| !page.trim().is_empty())
            {
                layer = true;
                break;
            }
        }
        return Some((text, layer));
    }
    let document = pdfium.open(book)?;
    let count = document.page_count();
    progress.total.store(count, Ordering::Relaxed);
    let mut text = BookText {
        pages: Vec::with_capacity(count),
        failed: 0,
        ocr: None,
    };
    for index in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // ページごとに鍵を取るので、画面のページを描くのを長く止めない
        let page = document.page_text(index);
        if page.is_none() {
            text.failed += 1;
        }
        text.pages.push(page.unwrap_or_default());
        progress.done.store(index + 1, Ordering::Relaxed);
        // 裏で下ごしらえするときは、ページを描く処理を待たせないよう、1ページごとに間を空ける
        if !pace.is_zero() {
            std::thread::sleep(pace);
        }
    }
    // 読めないページがあれば、ためない。元に戻すときは、埋め込まれた文字があるときだけ置き換える
    // （そうでなければ、ためてある OCR の結果をそのまま残す）
    let layer = !text.is_empty();
    if text.failed == 0 && (use_cache || layer) {
        save_cache(book, cache_dir, &text, "pdf", "");
    }
    Some((text, layer))
}

/// 裏で下ごしらえするときの、1ページごとの間
const PREPARE_PACE: Duration = Duration::from_millis(20);

/// この本の、今の版の文字がためてあるか（PDF の埋め込まれた文字か OCR の結果）。
/// ためておきは本の場所・大きさ・更新日時の順で書き始めるので、先頭だけを読んで確かめる（軽く済むように）。
/// 先頭が合わなければ（前の版の書き方など）全部を読んで確かめ、使えるなら本の場所を書き足す
pub fn has_cache(book: &Path, cache_dir: &Path) -> bool {
    let Some((size, modified)) = version(book) else {
        return false;
    };
    let Ok(name) = serde_json::to_string(&book.to_string_lossy()) else {
        return false;
    };
    let head = format!("{{\"book\":{name},\"size\":{size},\"modified\":{modified},");
    let current = [true, false].into_iter().any(|ocr| {
        let mut start = vec![0; head.len()];
        std::fs::File::open(cache_path(book, cache_dir, ocr))
            .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut start))
            .is_ok_and(|()| start == head.as_bytes())
    });
    current || load_cache(book, cache_dir, "").is_some()
}

/// PDF の文字を取り出してためておく（横断検索で探せるよう、裏で下ごしらえする）。
/// ためてあれば何もしない。読む邪魔をしないよう、1ページごとに間を空ける。
/// 止められたら（`cancel`）false を返す（ためないので、あとでやり直す）
pub fn prepare_pdf_text(
    pdfium: SharedPdfium,
    book: &Path,
    cache_dir: &Path,
    cancel: &AtomicBool,
) -> bool {
    if has_cache(book, cache_dir) {
        return true;
    }
    let progress = TextProgress::default();
    pdf_text(
        pdfium,
        book,
        cache_dir,
        &progress,
        cancel,
        true,
        PREPARE_PACE,
    );
    !cancel.load(Ordering::Relaxed)
}

/// 取り出した本の文字
pub struct LoadedText {
    pub index: SearchIndex,
    /// 文字が1つも無い（透明テキストの無い本）
    pub empty: bool,
    /// 文字を読めなかったページの数
    pub failed: usize,
    /// OCR で読んだときは、そのプラグインの id
    pub ocr: Option<String>,
    /// PDF に埋め込まれた文字がある（OCR の結果から戻せる）
    pub pdf_layer: bool,
}

impl LoadedText {
    fn new(text: &BookText) -> Self {
        Self {
            index: SearchIndex::new(text),
            empty: text.is_empty(),
            failed: text.failed,
            ocr: text.ocr.clone(),
            pdf_layer: false,
        }
    }
}

/// 本の文字を別のスレッドで取り出し、検索のための形にする。`cancel` を立てると止まる。
/// PDF なら透明テキストを（`pdfium` を渡す）、ほかの本は前に OCR してためたもの
/// （ページの並びが `pages` と同じもの）だけを使う
pub fn spawn_book_text(
    pdfium: Option<SharedPdfium>,
    book: PathBuf,
    pages: PageSource,
    cache_dir: PathBuf,
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
) -> Receiver<Option<LoadedText>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let loaded = match pdfium {
            Some(pdfium) => pdf_text(
                pdfium,
                &book,
                &cache_dir,
                &progress,
                &cancel,
                true,
                Duration::ZERO,
            )
            .map(|(text, layer)| LoadedText {
                pdf_layer: layer,
                ..LoadedText::new(&text)
            }),
            None => Some(LoadedText::new(
                &load_cache(&book, &cache_dir, &pages.key()).unwrap_or_default(),
            )),
        };
        let _ = tx.send(loaded);
    });
    rx
}

/// OCR の結果をやめ、PDF に埋め込まれた文字を取り出し直す（ためておきも置き換える。別のスレッド）。
/// 読めないページがあったり文字が無かったりしたら None（ためてある OCR の結果は残す）
pub fn spawn_pdf_layer(
    pdfium: SharedPdfium,
    book: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
) -> Receiver<Option<LoadedText>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let loaded = pdf_text(
            pdfium,
            &book,
            &cache_dir,
            &progress,
            &cancel,
            false,
            Duration::ZERO,
        )
        .filter(|(text, layer)| text.failed == 0 && *layer)
        .map(|(text, layer)| LoadedText {
            pdf_layer: layer,
            ..LoadedText::new(&text)
        });
        let _ = tx.send(loaded);
    });
    rx
}

/// 本のページの並び（OCR で読むページと、ためておいた文字が今の本に合うかを見分けるのに使う）
pub struct PageSource {
    pub mode: SourceMode,
    /// 書庫・フォルダ・PDF
    pub root: PathBuf,
    /// ページの名前（`source::list_images` の並び。PDF は "pdf_page_<番号>"）
    pub names: Vec<String>,
}

impl PageSource {
    /// ページの並びを見分ける値。フォルダは、画像の上書きでフォルダの版が変わらないので、
    /// 各画像の大きさと更新日時も入れる（サブフォルダごと読んだときとそうでないときも、並びで見分ける）。
    /// PDF は、ファイルの版で決まるので空
    pub fn key(&self) -> String {
        if self.mode == SourceMode::Pdf {
            return String::new();
        }
        let mut text = String::new();
        for name in &self.names {
            text.push_str(name);
            if self.mode == SourceMode::Folder
                && let Some((size, modified)) = version(Path::new(name))
            {
                text.push_str(&format!("\t{size}\t{modified}"));
            }
            text.push('\n');
        }
        cover_cache_name(&text)
    }
}

/// 一度に OCR のプラグインに渡すページの数（プラグインの説明の `ocr_batch` で変えられる）
const OCR_BATCH: usize = 16;
/// `ocr_batch` の上限（作業用の画像で、ディスクを使いすぎないように）
const OCR_BATCH_MAX: usize = 128;
/// OCR に渡す画像の長い辺の上限（ピクセル）
const OCR_MAX_SIDE: u32 = 2400;
/// PDF のページを描く高さ（ピクセル）
const OCR_PDF_HEIGHT: f32 = 2000.0;

/// OCR に渡すページの画像を、PNG にそろえて `path` に書く
fn write_ocr_image(
    source: &PageSource,
    reader: &mut Option<SourceReader>,
    pdf: Option<&PdfDoc>,
    page: usize,
    path: &Path,
) -> Option<()> {
    let image = match pdf {
        Some(pdf) => pdf.render(page, OCR_PDF_HEIGHT, Some(OCR_PDF_HEIGHT))?,
        None => {
            let reader =
                reader.get_or_insert_with(|| SourceReader::open(source.mode, &source.root));
            let bytes = reader.read(source.names.get(page)?)?;
            image::load_from_memory(&bytes).ok()?
        }
    };
    let image = if image.width().max(image.height()) > OCR_MAX_SIDE {
        image.resize(
            OCR_MAX_SIDE,
            OCR_MAX_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    image.save_with_format(path, image::ImageFormat::Png).ok()
}

/// 作業用のフォルダの名前に付ける通し番号
static OCR_WORK: AtomicUsize = AtomicUsize::new(0);

/// このプロセスが OCR のために作った作業フォルダを消す（終了するときに、後始末を待ちきれなかったぶん）
pub fn remove_ocr_work(cache_dir: &Path) {
    let prefix = format!("{}-", std::process::id());
    let Ok(entries) = std::fs::read_dir(cache_dir.join("ocr-work")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// 本のページを OCR のプラグインで読む。読めなかったページは数え、そういう本はためておかない。
/// やめたとき（`cancel`）は空の理由の、プラグインが失敗したときはその理由のエラーを返す
#[allow(clippy::too_many_arguments)]
fn ocr_text(
    manifest: &PluginManifest,
    credentials: &HashMap<String, String>,
    book: &Path,
    source: &PageSource,
    pdfium: Option<SharedPdfium>,
    cache_dir: &Path,
    progress: &TextProgress,
    cancel: &Arc<AtomicBool>,
) -> Result<BookText, PluginError> {
    let pdf = match (source.mode, pdfium) {
        (SourceMode::Pdf, Some(pdfium)) => Some(
            pdfium
                .open(&source.root)
                .ok_or_else(|| PluginError::Other("PDF を開けませんでした".to_owned()))?,
        ),
        _ => None,
    };
    let total = source.names.len();
    progress.total.store(total, Ordering::Relaxed);
    let mut pages: Vec<Option<String>> = vec![None; total];
    let work = cache_dir.join("ocr-work").join(format!(
        "{}-{}",
        std::process::id(),
        OCR_WORK.fetch_add(1, Ordering::Relaxed)
    ));
    let mut reader = None;
    let mut done = 0;
    let mut unexpected = false;
    let mut result = Ok(());
    let order: Vec<usize> = (0..total).collect();
    let batch_size = manifest
        .ocr_batch
        .unwrap_or(OCR_BATCH)
        .clamp(1, OCR_BATCH_MAX);
    for batch in order.chunks(batch_size) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let _ = std::fs::create_dir_all(&work);
        let images: Vec<OcrImage> = batch
            .iter()
            .filter_map(|&page| {
                // 画像を用意するのにも時間がかかるので、1ページごとに止める印を見る
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let path = work.join(format!("page-{page:05}.png"));
                write_ocr_image(source, &mut reader, pdf.as_ref(), page, &path)?;
                Some(OcrImage { page, path })
            })
            .collect();
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_dir_all(&work);
            break;
        }
        // 今回渡したページで、まだ受け取っていないものだけを受け取る。それ以外が届いたら、結果を使わない
        let sent: HashSet<usize> = images.iter().map(|image| image.page).collect();
        let answer = ocr_images(
            manifest,
            credentials,
            &images,
            cancel.clone(),
            |page, text| match pages.get_mut(page) {
                Some(slot) if sent.contains(&page) && slot.is_none() => {
                    *slot = Some(text);
                    done += 1;
                    progress.done.store(done, Ordering::Relaxed);
                }
                _ => unexpected = true,
            },
        );
        let _ = std::fs::remove_dir_all(&work);
        if let Err(error) = answer {
            result = Err(error);
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(PluginError::Other(String::new()));
    }
    result?;
    if unexpected {
        return Err(PluginError::Other(
            "OCR のプラグインが、渡していないページの文字を返しました".to_owned(),
        ));
    }
    // 1ページも読めなければ、プラグインが動いていない。前に読んだ結果を消さないよう、ためない
    if total > 0 && done == 0 {
        return Err(PluginError::Other(
            "OCR のプラグインが、1ページも読めませんでした".to_owned(),
        ));
    }
    let text = BookText {
        failed: pages.iter().filter(|page| page.is_none()).count(),
        pages: pages.into_iter().map(Option::unwrap_or_default).collect(),
        ocr: Some(manifest.id.clone()),
    };
    // 読めなかったページがあっても、読めたページは使えるようにためる（長い時間をかけた結果を捨てない）
    save_cache(
        book,
        cache_dir,
        &text,
        &format!("ocr:{}", manifest.id),
        &source.key(),
    );
    Ok(text)
}

/// 本のページを OCR のプラグインで読み、検索のための形にする（別のスレッド）
#[allow(clippy::too_many_arguments)]
pub fn spawn_book_ocr(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
    book: PathBuf,
    source: PageSource,
    pdfium: Option<SharedPdfium>,
    cache_dir: PathBuf,
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
) -> Receiver<Result<LoadedText, PluginError>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let text = ocr_text(
            &manifest,
            &credentials,
            &book,
            &source,
            pdfium,
            &cache_dir,
            &progress,
            &cancel,
        );
        let _ = tx.send(text.map(|text| LoadedText::new(&text)));
    });
    rx
}

/// 比べやすい形の1文字。空白は None（無視する）。縦書き用の句読点やかっこはふつうの形に、
/// 全角の英数字や記号は半角に、大文字は小文字にする
pub(crate) fn fold_char(c: char) -> Option<char> {
    if c.is_whitespace() {
        return None;
    }
    let c = match c {
        '\u{FE10}' => '，',
        '\u{FE11}' => '、',
        '\u{FE12}' => '。',
        '\u{FE13}' => '：',
        '\u{FE14}' => '；',
        '\u{FE15}' => '！',
        '\u{FE16}' => '？',
        '\u{FE17}' => '〖',
        '\u{FE18}' => '〗',
        '\u{FE19}' => '…',
        '\u{FE30}' => '‥',
        '\u{FE31}' | '\u{FE32}' => '—',
        '\u{FE35}' => '（',
        '\u{FE36}' => '）',
        '\u{FE37}' => '｛',
        '\u{FE38}' => '｝',
        '\u{FE39}' => '〔',
        '\u{FE3A}' => '〕',
        '\u{FE3B}' => '【',
        '\u{FE3C}' => '】',
        '\u{FE3D}' => '《',
        '\u{FE3E}' => '》',
        '\u{FE3F}' => '〈',
        '\u{FE40}' => '〉',
        '\u{FE41}' => '「',
        '\u{FE42}' => '」',
        '\u{FE43}' => '『',
        '\u{FE44}' => '』',
        '\u{FE47}' => '［',
        '\u{FE48}' => '］',
        c => c,
    };
    let c = match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        c => c,
    };
    Some(c.to_lowercase().next().unwrap_or(c))
}

/// 検索で見つかったところ
#[derive(Debug, PartialEq)]
pub struct Hit {
    /// ページ（0 から）
    pub page: usize,
    /// 見つかった語と、その前後の文（空白は除く）
    pub snippet: String,
}

/// 前後に出す文字数
const SNIPPET_AROUND: usize = 16;
/// 探す語の長さの上限（文字）
pub const MAX_QUERY_CHARS: usize = 200;

/// 検索のための形にした1ページ
struct IndexedPage {
    /// 比べやすい形にした文字（空白は除く）
    folded: String,
    /// `folded` の各文字の、元の文字（前後の文を出すのに使う）
    kept: Vec<char>,
    /// `folded` の各文字の、先頭からの位置（バイト）
    starts: Vec<usize>,
}

/// 検索のための形にした本。本を読み込んだときに一度だけ作り、語を打つたびには作らない
pub struct SearchIndex {
    pages: Vec<IndexedPage>,
}

impl SearchIndex {
    pub fn new(text: &BookText) -> Self {
        let pages = text
            .pages
            .iter()
            .map(|content| {
                let mut page = IndexedPage {
                    folded: String::new(),
                    kept: Vec::new(),
                    starts: Vec::new(),
                };
                for c in content.chars() {
                    if let Some(folded) = fold_char(c) {
                        page.starts.push(page.folded.len());
                        page.folded.push(folded);
                        page.kept.push(c);
                    }
                }
                page
            })
            .collect();
        Self { pages }
    }

    /// `query` を探す。空白は無視するので、行をまたいでも見つかる。多くても `limit` 件まで。
    /// 標準の文字列検索を使うので、本の長さに比例する時間で済む
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let query: String = query
            .chars()
            .filter_map(fold_char)
            .take(MAX_QUERY_CHARS)
            .collect();
        if query.is_empty() {
            return Vec::new();
        }
        let length = query.chars().count();
        let mut hits = Vec::new();
        for (page, indexed) in self.pages.iter().enumerate() {
            let mut from = 0;
            while let Some(found) = indexed.folded[from..].find(&query) {
                let byte = from + found;
                let Ok(start) = indexed.starts.binary_search(&byte) else {
                    break;
                };
                let first = start.saturating_sub(SNIPPET_AROUND);
                let last = (start + length + SNIPPET_AROUND).min(indexed.kept.len());
                let mut snippet: String = indexed.kept[first..last].iter().collect();
                if first > 0 {
                    snippet.insert(0, '…');
                }
                if last < indexed.kept.len() {
                    snippet.push('…');
                }
                hits.push(Hit { page, snippet });
                if hits.len() >= limit {
                    return hits;
                }
                from = byte + query.len();
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn search(text: &BookText, query: &str, limit: usize) -> Vec<Hit> {
        SearchIndex::new(text).search(query, limit)
    }

    fn book_text(pages: Vec<String>) -> BookText {
        BookText {
            pages,
            failed: 0,
            ocr: None,
        }
    }

    #[test]
    fn searches_ignore_spaces_and_vertical_forms() {
        let text = book_text(vec![
            "表紙".to_owned(),
            "れ て です ね︒ でも︑ お願 いだ から\nこれ だ け は\n﹃フ ァー マシ ィー﹄"
                .to_owned(),
            "ＭａｎｇａＲｅｅｄａ で読む".to_owned(),
        ]);
        // 文字のあいだの空白や改行をまたいでも見つかる
        let hits = search(&text, "お願いだからこれだけは", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        assert!(hits[0].snippet.contains("お願いだからこれだけは"));
        // 縦書き用の句読点やかっこも、ふつうの形で探せる
        assert_eq!(search(&text, "ですね。でも、", 10)[0].page, 1);
        assert_eq!(search(&text, "『ファーマシィー』", 10)[0].page, 1);
        // 全角・半角、大文字・小文字を区別しない
        assert_eq!(search(&text, "mangareeda", 10)[0].page, 2);
        assert!(search(&text, "無い語", 10).is_empty());
        assert!(search(&text, "  ", 10).is_empty());
        assert!(!text.is_empty());
        assert!(book_text(vec![" ".to_owned()]).is_empty());
    }

    #[test]
    fn hits_are_limited_and_snippets_are_cut_around_the_word() {
        let text = book_text(vec!["あ".repeat(40) + "語" + &"い".repeat(40) + "語"]);
        let hits = search(&text, "語", 1);
        assert_eq!(hits.len(), 1);
        let expected = format!(
            "…{}語{}…",
            "あ".repeat(SNIPPET_AROUND),
            "い".repeat(SNIPPET_AROUND)
        );
        assert_eq!(hits[0].snippet, expected);
        assert_eq!(search(&text, "語", 10).len(), 2);
    }

    /// 取り出した文字は本ごとに1つのファイルにためておき、本が変わったら使わない
    #[test]
    fn extracted_text_is_cached_per_book_and_dropped_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("manga-book-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("a.pdf");
        std::fs::write(&book, "1").unwrap();
        let text = book_text(vec!["文字".to_owned()]);
        save_cache(&book, &dir, &text, "pdf", "");
        assert_eq!(load_cache(&book, &dir, ""), Some(text));
        // 本が変わったら、ためておいたものは使わず、同じファイルを書き直す
        std::fs::write(&book, "22").unwrap();
        assert_eq!(load_cache(&book, &dir, ""), None);
        save_cache(&book, &dir, &book_text(vec!["新".to_owned()]), "pdf", "");
        assert_eq!(std::fs::read_dir(dir.join("text")).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// ためておくファイルが上限を超えたら、長く使っていないものから消す
    #[test]
    fn the_cache_keeps_only_the_most_recently_used_books() {
        let dir =
            std::env::temp_dir().join(format!("manga-book-text-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let now = SystemTime::now();
        for i in 0..5u64 {
            let path = dir.join(format!("{i}.json"));
            std::fs::write(&path, "{}").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(now - std::time::Duration::from_secs(100 - i))
                .unwrap();
        }
        prune_cache(&dir, 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["2.json", "3.json", "4.json"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 長い語で、終わりだけが違うものを探しても、すぐ終わる
    #[test]
    fn long_queries_on_large_books_finish_quickly() {
        let page = "あ".repeat(200_000);
        let text = book_text(vec![page; 5]);
        let index = SearchIndex::new(&text);
        let query = "あ".repeat(MAX_QUERY_CHARS - 1) + "い";
        let started = std::time::Instant::now();
        assert!(index.search(&query, 10).is_empty());
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // 上限より長い語は、上限の長さまでで探す
        let too_long = "あ".repeat(MAX_QUERY_CHARS * 2) + "い";
        assert_eq!(index.search(&too_long, 1).len(), 1);
    }

    /// 2ページ目にだけ文字のある PDF
    fn pdf_with_text() -> Vec<u8> {
        let stream = "BT /F1 12 Tf 20 100 Td (Hello MangaReeda) Tj ET";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] >>".to_owned(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 5 0 R \
             /Resources << /Font << /F1 6 0 R >> >> >>"
                .to_owned(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_owned(),
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

    /// PDF の透明テキストを取り出して探せる。pdfium が無い環境では確かめない
    #[test]
    fn text_layers_of_pdfs_can_be_searched() {
        let Some(pdfium) = crate::engine::pdf::pdfium() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("manga-book-text-pdf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("book.pdf");
        std::fs::write(&book, pdf_with_text()).unwrap();
        let progress = Arc::new(TextProgress::default());
        let rx = spawn_book_text(
            Some(pdfium),
            book.clone(),
            PageSource {
                mode: SourceMode::Pdf,
                root: book.clone(),
                names: Vec::new(),
            },
            dir.join("cache"),
            progress.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let loaded = rx.recv().unwrap().unwrap();
        assert!(!loaded.empty);
        assert_eq!(loaded.failed, 0);
        assert_eq!(progress.done.load(Ordering::Relaxed), 2);
        let hits = loaded.index.search("mangareeda", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        // 2回目は、ためておいたものを使う
        assert!(cache_path(&book, &dir.join("cache"), false).is_file());
        // OCR で読み直したものは、埋め込まれた文字があることも知らせ、元に戻せる
        let ocr = book_text(vec!["x".to_owned(), "y".to_owned()]);
        save_cache(&book, &dir.join("cache"), &ocr, "ocr:fake", "");
        let pages = || PageSource {
            mode: SourceMode::Pdf,
            root: book.clone(),
            names: Vec::new(),
        };
        let cancel = || Arc::new(AtomicBool::new(false));
        let loaded = spawn_book_text(
            Some(pdfium),
            book.clone(),
            pages(),
            dir.join("cache"),
            Arc::new(TextProgress::default()),
            cancel(),
        )
        .recv()
        .unwrap()
        .unwrap();
        assert_eq!(
            (loaded.ocr.as_deref(), loaded.pdf_layer),
            (Some("fake"), true)
        );
        assert!(loaded.index.search("mangareeda", 10).is_empty());
        let restored = spawn_pdf_layer(
            pdfium,
            book.clone(),
            dir.join("cache"),
            Arc::new(TextProgress::default()),
            cancel(),
        )
        .recv()
        .unwrap()
        .unwrap();
        assert_eq!(restored.ocr, None);
        assert_eq!(restored.index.search("mangareeda", 10).len(), 1);
        assert_eq!(load_cache(&book, &dir.join("cache"), "").unwrap().ocr, None);
        // 埋め込まれた文字が無ければ戻さず、ためてある OCR の結果を残す
        // （文字を同じ長さの空白にして、PDF の中の位置をずらさない）
        let blank = dir.join("blank.pdf");
        let bytes = String::from_utf8(pdf_with_text())
            .unwrap()
            .replace("Hello MangaReeda", &" ".repeat("Hello MangaReeda".len()));
        std::fs::write(&blank, bytes).unwrap();
        save_cache(&blank, &dir.join("cache"), &ocr, "ocr:fake", "");
        let none = spawn_pdf_layer(
            pdfium,
            blank.clone(),
            dir.join("cache"),
            Arc::new(TextProgress::default()),
            cancel(),
        )
        .recv()
        .unwrap();
        assert!(none.is_none());
        let kept = load_cache(&blank, &dir.join("cache"), "").unwrap();
        assert_eq!(kept.ocr.as_deref(), Some("fake"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 裏での下ごしらえ: PDF の文字をためる。ためてあれば何もしない。止めたら、ためない
    #[test]
    fn pdf_text_can_be_prepared_in_the_background() {
        let Some(pdfium) = crate::engine::pdf::pdfium() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("manga-prepare-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("book.pdf");
        std::fs::write(&book, pdf_with_text()).unwrap();
        let cache = dir.join("cache");
        // 止めたら、ためない（あとでやり直す）
        assert!(!prepare_pdf_text(
            pdfium,
            &book,
            &cache,
            &AtomicBool::new(true)
        ));
        assert!(!has_cache(&book, &cache));
        assert!(prepare_pdf_text(
            pdfium,
            &book,
            &cache,
            &AtomicBool::new(false)
        ));
        let text = load_cache(&book, &cache, "").unwrap();
        assert!(text.pages[1].contains("MangaReeda"));
        // ためてあれば、止める印が立っていても済んだことになる
        assert!(prepare_pdf_text(
            pdfium,
            &book,
            &cache,
            &AtomicBool::new(true)
        ));
        // 前の版で書いたもの（本の場所が無い）は、取り出し直さずに本の場所を書き足す
        let path = cache_path(&book, &cache, false);
        let book_field = format!(
            "\"book\":{},",
            serde_json::to_string(&book.to_string_lossy()).unwrap()
        );
        let old = std::fs::read_to_string(&path)
            .unwrap()
            .replace(&book_field, "");
        std::fs::write(&path, old).unwrap();
        assert!(has_cache(&book, &cache));
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains(&book_field)
        );
        // 本を上書きしたら（版が変わる）、ためてあっても取り出し直す
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(&book, pdf_with_text()).unwrap();
        assert!(!has_cache(&book, &cache));
        assert!(prepare_pdf_text(
            pdfium,
            &book,
            &cache,
            &AtomicBool::new(false)
        ));
        assert!(has_cache(&book, &cache));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// ページごとに「ページ<番号> の文字」を返す、テスト用の OCR のプラグイン
    #[cfg(unix)]
    fn script_ocr(dir: &Path) -> PluginManifest {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("ocr.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nread request\n\
             for page in $(echo \"$request\" | grep -o '\"page\":[0-9]*' | cut -d: -f2); do\n\
             echo \"{\\\"page\\\": $page, \\\"text\\\": \\\"ページ$page の 文字\\\"}\"\n\
             done\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        PluginManifest {
            id: "fake".to_owned(),
            name: "Fake OCR".to_owned(),
            command: vec!["./ocr.sh".to_owned()],
            credentials: Vec::new(),
            auth: false,
            kind: crate::engine::plugin::PluginKind::Ocr,
            ocr_batch: None,
            dir: dir.to_path_buf(),
        }
    }

    /// 文字の層の無い本を OCR のプラグインで読み、探せるようにして、ためておく
    #[cfg(unix)]
    #[test]
    fn books_without_text_are_read_with_ocr_plugins() {
        let dir = std::env::temp_dir().join(format!("manga-book-ocr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let book = dir.join("book");
        std::fs::create_dir_all(&book).unwrap();
        for name in ["01.png", "02.png", "03.png"] {
            image::RgbaImage::new(20, 30).save(book.join(name)).unwrap();
        }
        let listing = crate::engine::source::list_images(&book, false).unwrap();
        let source = || PageSource {
            mode: listing.mode,
            root: listing.root.clone(),
            names: listing.images.clone(),
        };
        let progress = Arc::new(TextProgress::default());
        let rx = spawn_book_ocr(
            script_ocr(&dir),
            HashMap::new(),
            book.clone(),
            source(),
            None,
            dir.join("cache"),
            progress.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let loaded = rx.recv().unwrap().unwrap();
        assert_eq!(loaded.failed, 0);
        assert_eq!(progress.done.load(Ordering::Relaxed), 3);
        assert_eq!(loaded.index.search("ページ2の文字", 10)[0].page, 2);
        // 読んだ文字はためておき、次に開いたときに使う（作業用の画像は残さない）
        let cached = load_cache(&book, &dir.join("cache"), &source().key()).unwrap();
        assert_eq!(cached.pages[0], "ページ0 の 文字");
        assert!(
            std::fs::read_dir(dir.join("cache").join("ocr-work"))
                .map_or(true, |mut entries| entries.next().is_none())
        );
        // やめたら、理由の無いエラーで止まる
        let cancel = Arc::new(AtomicBool::new(true));
        let rx = spawn_book_ocr(
            script_ocr(&dir),
            HashMap::new(),
            book.clone(),
            source(),
            None,
            dir.join("cache2"),
            Arc::new(TextProgress::default()),
            cancel,
        );
        let error = rx.recv().unwrap().err().unwrap();
        assert!(error.message().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 渡していないページの番号や、文字の無い行を返すプラグインの結果は、ためない
    #[cfg(unix)]
    #[test]
    fn unexpected_ocr_answers_are_not_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("manga-book-ocr-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let book = dir.join("book");
        std::fs::create_dir_all(&book).unwrap();
        // 17 ページ（2つのまとまりに分かれる）
        for page in 0..17 {
            image::RgbaImage::new(4, 4)
                .save(book.join(format!("{page:02}.png")))
                .unwrap();
        }
        let listing = crate::engine::source::list_images(&book, false).unwrap();
        let source = || PageSource {
            mode: listing.mode,
            root: listing.root.clone(),
            names: listing.images.clone(),
        };
        let plugin = |body: &str| {
            let script = dir.join("ocr.sh");
            std::fs::write(&script, format!("#!/bin/sh\nread request\n{body}")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            PluginManifest {
                id: "bad".to_owned(),
                name: "Bad".to_owned(),
                command: vec!["./ocr.sh".to_owned()],
                credentials: Vec::new(),
                auth: false,
                kind: crate::engine::plugin::PluginKind::Ocr,
                ocr_batch: None,
                dir: dir.clone(),
            }
        };
        let run = |manifest: PluginManifest| {
            spawn_book_ocr(
                manifest,
                HashMap::new(),
                book.clone(),
                source(),
                None,
                dir.join("cache"),
                Arc::new(TextProgress::default()),
                Arc::new(AtomicBool::new(false)),
            )
            .recv()
            .unwrap()
        };
        // どのまとまりにも 0 ページ目を返す（2つ目のまとまりでは、渡していないページ）
        let always_first = plugin("echo '{\"page\": 0, \"text\": \"x\"}'\n");
        assert!(run(always_first).is_err());
        assert!(load_cache(&book, &dir.join("cache"), &source().key()).is_none());
        // text の無い行は読めた知らせにならない。1ページも読めなければ、失敗にしてためない
        let no_text = plugin(
            "for page in $(echo \"$request\" | grep -o '\"page\":[0-9]*' | cut -d: -f2); do\n\
             echo \"{\\\"page\\\": $page}\"\ndone\n",
        );
        assert!(run(no_text).is_err());
        assert!(load_cache(&book, &dir.join("cache"), &source().key()).is_none());
        // 読めないページがあっても、読めたページはためる（読めなかった数も）
        let skip_first = plugin(
            "for page in $(echo \"$request\" | grep -o '\"page\":[0-9]*' | cut -d: -f2); do\n\
             [ \"$page\" = 0 ] || echo \"{\\\"page\\\": $page, \\\"text\\\": \\\"p$page\\\"}\"\ndone\n",
        );
        assert_eq!(run(skip_first).ok().unwrap().failed, 1);
        let kept = load_cache(&book, &dir.join("cache"), &source().key()).unwrap();
        assert_eq!((kept.failed, kept.pages[1].as_str()), (1, "p1"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// OCR の結果は上限の外のフォルダに置き、古いものから消すときにも消さない。
    /// 前の版で上限のあるフォルダに置いた OCR の結果は、読んだときに移す。埋め込まれた文字に戻したら消す
    #[test]
    fn ocr_results_are_kept_apart_from_the_limit() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("book.pdf");
        std::fs::write(&book, b"x").unwrap();
        let cache = dir.join("cache");
        let text = book_text(vec!["一".to_owned()]);
        save_cache(&book, &cache, &text, "ocr:x", "");
        assert!(cache_path(&book, &cache, true).is_file());
        assert!(!cache_path(&book, &cache, false).exists());
        // 本の場所も書く（横断検索で使う）
        let file: CacheFile = serde_json::from_str(
            &std::fs::read_to_string(cache_path(&book, &cache, true)).unwrap(),
        )
        .unwrap();
        assert_eq!(file.book, book.to_string_lossy());
        // 上限まで消しても、OCR の結果は残る
        prune_cache(&cache.join(TEXT_DIR), 0);
        assert_eq!(
            load_cache(&book, &cache, "").unwrap().ocr.as_deref(),
            Some("x")
        );
        // 埋め込まれた文字に戻したら、OCR の結果は消える
        save_cache(&book, &cache, &text, "pdf", "");
        assert!(!cache_path(&book, &cache, true).exists());
        assert_eq!(load_cache(&book, &cache, "").unwrap().ocr, None);
        // 前の版で上限のあるフォルダに置いた OCR の結果は、読んだときに上限の外へ移す。
        // 前の版で書いたものには本の場所が無いので、書き足す
        let book_field = format!(
            "\"book\":{},",
            serde_json::to_string(&book.to_string_lossy()).unwrap()
        );
        let old = std::fs::read_to_string(cache_path(&book, &cache, false))
            .unwrap()
            .replace("\"source\":\"pdf\"", "\"source\":\"ocr:y\"")
            .replace(&book_field, "");
        assert!(!old.contains("\"book\""));
        std::fs::write(cache_path(&book, &cache, false), old).unwrap();
        assert_eq!(
            load_cache(&book, &cache, "").unwrap().ocr.as_deref(),
            Some("y")
        );
        assert!(cache_path(&book, &cache, true).is_file());
        assert!(!cache_path(&book, &cache, false).exists());
        let file: CacheFile = serde_json::from_str(
            &std::fs::read_to_string(cache_path(&book, &cache, true)).unwrap(),
        )
        .unwrap();
        assert_eq!(file.book, book.to_string_lossy());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// フォルダの本は、画像を上書きしたときや、ページの並びが違うときに、ためておいた文字を使わない
    #[test]
    fn folder_caches_follow_the_pages() {
        let dir = std::env::temp_dir().join(format!("manga-book-key-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let book = dir.join("book");
        std::fs::create_dir_all(book.join("sub")).unwrap();
        for name in ["01.png", "sub/02.png"] {
            image::RgbaImage::new(4, 4).save(book.join(name)).unwrap();
        }
        let source = |include_subfolders| {
            let listing = crate::engine::source::list_images(&book, include_subfolders).unwrap();
            PageSource {
                mode: listing.mode,
                root: listing.root,
                names: listing.images,
            }
        };
        let text = BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec!["一".to_owned(), "二".to_owned()])
        };
        save_cache(&book, &dir, &text, "ocr:x", &source(true).key());
        assert_eq!(load_cache(&book, &dir, &source(true).key()), Some(text));
        // サブフォルダを読まないときは、並びが違う
        assert_eq!(load_cache(&book, &dir, &source(false).key()), None);
        // 画像を上書きしたら（フォルダの版は変わらなくても）使わない
        image::RgbaImage::new(8, 8)
            .save(book.join("01.png"))
            .unwrap();
        assert_eq!(load_cache(&book, &dir, &source(true).key()), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// OCR の結果は、本の中身から作った値で見分ける。本を動かしたら新しい場所の結果にし、
    /// 写したら両方で使う。更新日時だけが違う本（別の機械から写したもの）にも使う
    #[test]
    fn ocr_results_follow_moved_and_copied_books() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("new")).unwrap();
        let cache = dir.join("cache");
        // 先頭と末尾だけを読む大きさの本
        let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let book = dir.join("book.pdf");
        std::fs::write(&book, &content).unwrap();
        let text = BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec!["一".to_owned()])
        };
        save_cache(&book, &cache, &text, "ocr:x", "");
        // 動かす（名前も変える）
        let moved = dir.join("new").join("renamed.pdf");
        std::fs::rename(&book, &moved).unwrap();
        assert_eq!(load_cache(&moved, &cache, ""), Some(text_like(&text)));
        assert!(!cache_path(&book, &cache, true).exists());
        assert!(cache_path(&moved, &cache, true).is_file());
        // 動かした先に、文字の無い PDF のためておきがあっても使う。文字があれば使わない
        let empty_pdf = dir.join("empty-pdf.pdf");
        std::fs::copy(&moved, &empty_pdf).unwrap();
        save_cache(
            &empty_pdf,
            &cache,
            &book_text(vec![String::new()]),
            "pdf",
            "",
        );
        assert_eq!(load_cache(&empty_pdf, &cache, ""), Some(text_like(&text)));
        assert!(!cache_path(&empty_pdf, &cache, false).exists());
        let layer_pdf = dir.join("layer-pdf.pdf");
        std::fs::copy(&moved, &layer_pdf).unwrap();
        save_cache(
            &layer_pdf,
            &cache,
            &book_text(vec!["埋".to_owned()]),
            "pdf",
            "",
        );
        assert_eq!(load_cache(&layer_pdf, &cache, "").unwrap().ocr, None);
        // 写す。元の結果も残す
        let copied = dir.join("copied.pdf");
        std::fs::copy(&moved, &copied).unwrap();
        assert_eq!(load_cache(&copied, &cache, ""), Some(text_like(&text)));
        assert!(cache_path(&moved, &cache, true).is_file());
        // 更新日時だけが違う本にも使い、今の日時に書き直す
        let earlier = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&copied)
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        assert_eq!(load_cache(&copied, &cache, ""), Some(text_like(&text)));
        let file = read_cache_file(&cache_path(&copied, &cache, true)).unwrap();
        assert_eq!(Some((file.size, file.modified)), version(&copied));
        // 大きさが同じでも中身が違えば使わない（末尾が違うときも、先頭と末尾が同じで中ほどだけ違うときも）
        let mut other = content.clone();
        *other.last_mut().unwrap() ^= 1;
        let different = dir.join("different.pdf");
        std::fs::write(&different, &other).unwrap();
        assert_eq!(load_cache(&different, &cache, ""), None);
        let mut middle = content.clone();
        middle[150_000] ^= 1;
        let middle_changed = dir.join("middle.pdf");
        std::fs::write(&middle_changed, &middle).unwrap();
        assert_eq!(fingerprint(&middle_changed), fingerprint(&moved));
        assert_eq!(load_cache(&middle_changed, &cache, ""), None);
        std::fs::write(&copied, &other).unwrap();
        assert_eq!(load_cache(&copied, &cache, ""), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn text_like(text: &BookText) -> BookText {
        BookText {
            pages: text.pages.clone(),
            failed: text.failed,
            ocr: text.ocr.clone(),
        }
    }

    /// 前の版で書いた OCR の結果には、一覧を作るときに値を書き足す（そのあと動かしても見つかる）。
    /// 書き足す前に本を動かしていたら、大きさと更新日時が同じ本だけを同じ本と見なす
    #[test]
    fn old_ocr_results_get_fingerprints_while_their_books_are_in_place() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-backfill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let text = BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec!["一".to_owned()])
        };
        let old_result = |book: &Path| {
            save_cache(book, &cache, &text, "ocr:x", "");
            let path = cache_path(book, &cache, true);
            let mut file = read_cache_file(&path).unwrap();
            file.fingerprint.clear();
            file.digest.clear();
            assert!(write_cache_file(&path, &file));
        };
        let kept = dir.join("kept.pdf");
        let early = dir.join("early.pdf");
        std::fs::write(&kept, b"kept book").unwrap();
        std::fs::write(&early, b"early book").unwrap();
        old_result(&kept);
        old_result(&early);
        let early_moved = dir.join("early-moved.pdf");
        std::fs::rename(&early, &early_moved).unwrap();
        backfill_ocr_results(&cache);
        let file = read_cache_file(&cache_path(&kept, &cache, true)).unwrap();
        assert_eq!(Some(file.fingerprint), fingerprint(&kept));
        assert_eq!(Some(file.digest), digest(&kept));
        let kept_moved = dir.join("kept-moved.pdf");
        std::fs::rename(&kept, &kept_moved).unwrap();
        assert_eq!(load_cache(&kept_moved, &cache, ""), Some(text_like(&text)));
        // 書き足す前に動かした本は、更新日時が変わっていなければ見つかり、値も書き足す
        assert_eq!(load_cache(&early_moved, &cache, ""), Some(text_like(&text)));
        assert!(!cache_path(&early, &cache, true).exists());
        let file = read_cache_file(&cache_path(&early_moved, &cache, true)).unwrap();
        assert_eq!(Some(file.fingerprint), fingerprint(&early_moved));
        // 更新日時が変わっていたら、見つけられない
        let late = dir.join("late.pdf");
        std::fs::write(&late, b"late book").unwrap();
        old_result(&late);
        let late_moved = dir.join("late-moved.pdf");
        std::fs::rename(&late, &late_moved).unwrap();
        let earlier = SystemTime::now() - Duration::from_secs(3600);
        std::fs::File::options()
            .write(true)
            .open(&late_moved)
            .unwrap()
            .set_modified(earlier)
            .unwrap();
        assert_eq!(load_cache(&late_moved, &cache, ""), None);
        assert!(cache_path(&late, &cache, true).exists());
        // 元の本が残っていれば（更新日時だけが変わっていても）、大きさと日時が同じ別の本には使わない
        let original = dir.join("original.pdf");
        std::fs::write(&original, b"original").unwrap();
        old_result(&original);
        let (size, modified) = version(&original).unwrap();
        let other = dir.join("other.pdf");
        std::fs::write(&other, b"another!").unwrap();
        let set_time = |book: &Path, time: SystemTime| {
            std::fs::File::options()
                .write(true)
                .open(book)
                .unwrap()
                .set_modified(time)
                .unwrap();
        };
        set_time(&original, earlier);
        set_time(&other, UNIX_EPOCH + Duration::from_secs(modified));
        assert_eq!(version(&other), Some((size, modified)));
        assert_eq!(load_cache(&other, &cache, ""), None);
        assert!(cache_path(&original, &cache, true).exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 前に別の本を置いていた場所に動かしても、そこの古いためておきに邪魔されずに、動かした本の結果を使う
    #[test]
    fn moved_books_replace_stale_results_at_the_new_place() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let ocr = |line: &str| BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec![line.to_owned()])
        };
        let place = dir.join("place.pdf");
        std::fs::write(&place, b"the old book").unwrap();
        save_cache(&place, &cache, &ocr("古"), "ocr:x", "");
        let book = dir.join("book.pdf");
        std::fs::write(&book, b"the book that moves").unwrap();
        save_cache(&book, &cache, &ocr("新"), "ocr:x", "");
        std::fs::rename(&book, &place).unwrap();
        assert_eq!(load_cache(&place, &cache, ""), Some(ocr("新")));
        assert!(!cache_path(&book, &cache, true).exists());
        // 文字のある、古い本の埋め込まれた文字のためておきがあっても同じ
        let other = dir.join("other.pdf");
        std::fs::write(&other, b"embedded text book").unwrap();
        save_cache(&other, &cache, &book_text(vec!["埋".to_owned()]), "pdf", "");
        let next = dir.join("next.pdf");
        std::fs::write(&next, b"the next book to move").unwrap();
        save_cache(&next, &cache, &ocr("次"), "ocr:x", "");
        std::fs::rename(&next, &other).unwrap();
        assert_eq!(load_cache(&other, &cache, ""), Some(ocr("次")));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 読んでから書くまでにほかで書かれたものは、書き直したり消したりしない
    #[test]
    fn results_written_meanwhile_are_kept() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let ocr = |line: &str| BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec![line.to_owned()])
        };
        let book = dir.join("book.pdf");
        std::fs::write(&book, b"book").unwrap();
        save_cache(&book, &cache, &ocr("前"), "ocr:x", "");
        let path = cache_path(&book, &cache, true);
        let raw = read_raw(&path).unwrap();
        let old = parse_cache(&raw).unwrap();
        // 読んだあとに OCR し直した
        save_cache(&book, &cache, &ocr("読み直した結果"), "ocr:x", "");
        assert!(!rewrite_unchanged(&path, &raw, &old));
        assert_eq!(load_cache(&book, &cache, ""), Some(ocr("読み直した結果")));
        // 使ったときに時刻だけを新しくしても（中身が同じなら）書き直せる
        let raw = read_raw(&path).unwrap();
        let same = parse_cache(&raw).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now() + Duration::from_secs(5))
            .unwrap();
        assert!(rewrite_unchanged(&path, &raw, &same));
        // 探すと決めたあとに、動かした先で埋め込まれた文字を取り出した
        let moved = dir.join("moved.pdf");
        std::fs::rename(&book, &moved).unwrap();
        let current = version(&moved).unwrap();
        save_cache(&moved, &cache, &book_text(vec!["埋".to_owned()]), "pdf", "");
        assert!(!adopt_ocr_result(
            &moved,
            &cache,
            "",
            current,
            (&None, &None)
        ));
        assert!(cache_path(&moved, &cache, false).exists());
        assert!(path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 中身全体のハッシュの無い結果（移行中のもの）も、元の本が残っていればそのハッシュと比べる。
    /// 付け替えた結果にはハッシュを書く。元の本の更新日時だけが変わっていても、写しなら元の結果を残す
    #[test]
    fn results_without_digests_are_checked_and_copies_keep_the_original() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-digest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let text = BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec!["一".to_owned()])
        };
        let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let book = dir.join("book.pdf");
        std::fs::write(&book, &content).unwrap();
        save_cache(&book, &cache, &text, "ocr:x", "");
        let path = cache_path(&book, &cache, true);
        let mut file = read_cache_file(&path).unwrap();
        file.digest.clear();
        assert!(write_cache_file(&path, &file));
        // 元の本が残っているので、中ほどだけ違う本はそのハッシュで見分ける
        let mut middle = content.clone();
        middle[150_000] ^= 1;
        let middle_changed = dir.join("middle.pdf");
        std::fs::write(&middle_changed, &middle).unwrap();
        assert_eq!(load_cache(&middle_changed, &cache, ""), None);
        // 動かしたら、付け替えた結果にハッシュを書く
        let moved = dir.join("moved.pdf");
        std::fs::rename(&book, &moved).unwrap();
        assert_eq!(load_cache(&moved, &cache, ""), Some(text_like(&text)));
        let moved_path = cache_path(&moved, &cache, true);
        assert_eq!(
            Some(read_cache_file(&moved_path).unwrap().digest),
            digest(&moved)
        );
        // 元の本の更新日時だけが変わってから写しを開いても、元の結果は残す
        std::fs::File::options()
            .write(true)
            .open(&moved)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3600))
            .unwrap();
        let copied = dir.join("copied.pdf");
        std::fs::write(&copied, &content).unwrap();
        assert_eq!(load_cache(&copied, &cache, ""), Some(text_like(&text)));
        assert!(moved_path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 一覧を作ったあとで本を動かしても、前の版で書いた結果（値の無いもの）を大きさと更新日時で見つける
    #[test]
    fn legacy_results_are_found_after_the_index_is_made() {
        let dir = std::env::temp_dir().join(format!("manga-ocr-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = dir.join("cache");
        let text = BookText {
            ocr: Some("x".to_owned()),
            ..book_text(vec!["一".to_owned()])
        };
        let book = dir.join("book.pdf");
        std::fs::write(&book, b"legacy book").unwrap();
        save_cache(&book, &cache, &text, "ocr:x", "");
        let path = cache_path(&book, &cache, true);
        let mut file = read_cache_file(&path).unwrap();
        file.fingerprint.clear();
        file.digest.clear();
        assert!(write_cache_file(&path, &file));
        ocr_results_for(&cache, &[]);
        let moved = dir.join("moved.pdf");
        std::fs::rename(&book, &moved).unwrap();
        assert_eq!(load_cache(&moved, &cache, ""), Some(text_like(&text)));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
