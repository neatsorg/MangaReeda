//! 横断検索。手元の本の名前（履歴・お気に入り・決まったフォルダの下）と、ためておいた本の文字
//! （PDF の埋め込まれた文字と OCR の結果）を、まとめて探す。
//!
//! 画面を止めないよう、探すのは別のスレッドで行う。語が変わったら前の検索はやめ、打つ手が止まってから探し始める。
//! 探すときに新しく文字を取り出したり OCR したりはせず、ためてあるものだけを読む。
//! 本の文字は、探すたびにためておきのファイルから読む。最近読んだものは、決まった量まで手元に持っておく。
//! 大きすぎるためておきは読まず、1冊ずつ探して、冊の切れ目ごとに語が変わっていないかを見る。

use crate::engine::book_text::{
    BookText, CacheFile, Hit, MAX_QUERY_CHARS, OCR_TEXT_DIR, PageSource, SearchIndex, TEXT_DIR,
    fold_char, version,
};
use crate::engine::history::normalize;
use crate::engine::source::{IMAGE_EXTENSIONS, list_images};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, SystemTime};

/// 本として扱うファイルの拡張子
const BOOK_EXTENSIONS: [&str; 5] = ["zip", "cbz", "rar", "cbr", "pdf"];
/// フォルダをたどる深さの上限
const SCAN_DEPTH: usize = 8;
/// フォルダをたどるときに見る項目の数の上限（大きなドライブで長く止まらないように）
const SCAN_LIMIT: usize = 50_000;
/// 名前で見つかったものを出す上限
pub const NAME_LIMIT: usize = 500;
/// 本文で見つかったところを出す上限（1冊あたりと、全体）
pub const TEXT_PER_BOOK: usize = 5;
pub const TEXT_LIMIT: usize = 500;
/// 本文で探す語の、最低の文字数（1文字では見つかりすぎる）
pub const TEXT_MIN_CHARS: usize = 2;
/// 打つ手が止まってから探し始めるまで
const DEBOUNCE: Duration = Duration::from_millis(200);
/// 読んだ本の文字を手元に持っておく量（文字数。1文字あたり約 15 バイトなので、約 60MB）
const INDEX_BUDGET: usize = 4_000_000;
/// 読むためておきのファイルの大きさの上限（約 270 万字。索引にしても、持っておく量に収まる）。
/// これより大きいものは横断検索では読まない
const MAX_CACHE_FILE: u64 = 8 * 1024 * 1024;

/// 名前で探す本がどこから来たか
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameFrom {
    History,
    Favorite,
    Folder,
}

/// 名前で見つかった本
#[derive(Clone, Debug, PartialEq)]
pub struct NameHit {
    pub path: PathBuf,
    pub from: NameFrom,
}

/// 本文で見つかったところ
#[derive(Clone, Debug, PartialEq)]
pub struct TextHit {
    pub book: PathBuf,
    pub page: usize,
    pub snippet: String,
}

/// 探す処理からの知らせ。どの語（`generation`）の結果かを付ける
#[derive(Debug)]
pub enum Message {
    /// フォルダをたどり終えた（名前で探す本の数）
    Scanned(usize),
    /// 名前で見つかった本（語ごとに1回）
    Names(Vec<NameHit>),
    /// 本文で見つかったところ（1冊ぶん）
    Text(Vec<TextHit>),
    /// 本文を探した冊の数
    Progress { done: usize, total: usize },
    /// 探し終えた
    Done,
}

/// 横断検索を受け持つスレッド。捨てると止まる
pub struct CrossSearch {
    requests: Sender<(u64, String)>,
    /// (語の番号, 知らせ)
    pub messages: Receiver<(u64, Message)>,
    /// 今の語の番号。変わったら、探している途中でもやめる
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
}

impl Drop for CrossSearch {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl CrossSearch {
    /// 探す処理を始める。`known` は履歴とお気に入りの本、`roots` は下をたどるフォルダ、
    /// `cache_dir` は本の文字のためておきがあるところ
    pub fn start(known: Vec<NameHit>, roots: Vec<PathBuf>, cache_dir: PathBuf) -> Self {
        let (requests, request_rx) = channel::<(u64, String)>();
        let (message_tx, messages) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            generation: generation.clone(),
            closed: closed.clone(),
            tx: message_tx,
            cache_dir,
            indexes: IndexCache::default(),
        };
        std::thread::spawn(move || worker.run(known, roots, request_rx));
        Self {
            requests,
            messages,
            generation,
            closed,
        }
    }

    /// 語を変える。前の語の検索はやめる。返すのは、この語の番号（知らせに付く）
    pub fn search(&self, query: &str) -> u64 {
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.requests.send((generation, query.to_owned()));
        generation
    }
}

/// 比べやすい形（空白を除き、全角半角や大文字小文字をそろえる）
fn fold(text: &str) -> String {
    text.chars().filter_map(fold_char).collect()
}

/// 本の名前（書庫や PDF は拡張子を除いたファイル名、フォルダはフォルダの名前）
pub fn book_name(path: &Path) -> String {
    let name = if is_book_file(path) {
        path.file_stem()
    } else {
        path.file_name()
    };
    name.unwrap_or_default().to_string_lossy().into_owned()
}

/// 本として扱うファイルか
fn is_book_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| BOOK_EXTENSIONS.contains(&ext.to_string_lossy().to_lowercase().as_str()))
}

/// 画像のファイルか
fn is_image_name(path: &Path) -> bool {
    path.extension().is_some_and(|ext| {
        IMAGE_EXTENSIONS.contains(&ext.to_string_lossy().to_lowercase().as_str())
    })
}

/// `roots` の下の本（書庫・PDF と、画像が直接入ったフォルダ）を集める。
/// 隠しフォルダとシンボリックリンクはたどらない。深さと見る数に上限がある。`stop()` が真になったらやめる
pub fn scan_books(roots: &[PathBuf], stop: &dyn Fn() -> bool) -> Vec<PathBuf> {
    let mut books = Vec::new();
    let mut seen = 0;
    let mut queue: VecDeque<(PathBuf, usize)> =
        roots.iter().map(|root| (root.clone(), 0)).collect();
    let mut visited = HashSet::new();
    while let Some((dir, depth)) = queue.pop_front() {
        if stop() || seen >= SCAN_LIMIT {
            break;
        }
        if !visited.insert(normalize(&dir)) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut has_images = false;
        for entry in entries.flatten() {
            seen += 1;
            if seen >= SCAN_LIMIT {
                break;
            }
            let path = entry.path();
            let hidden = entry.file_name().to_string_lossy().starts_with('.');
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if hidden {
                continue;
            }
            if kind.is_dir() {
                if depth < SCAN_DEPTH {
                    queue.push_back((path, depth + 1));
                }
            } else if is_book_file(&path) {
                books.push(path);
            } else if is_image_name(&path) {
                has_images = true;
            }
        }
        // 画像が直接入ったフォルダは、それ自体が1冊（たどり始めたフォルダは除く）
        if has_images && depth > 0 {
            books.push(dir);
        }
    }
    books
}

/// 名前で探す。`query` は比べやすい形にしたもの
fn search_names(books: &[NameHit], query: &str) -> Vec<NameHit> {
    books
        .iter()
        .filter(|book| fold(&book_name(&book.path)).contains(query))
        .take(NAME_LIMIT)
        .cloned()
        .collect()
}

/// 読んだ本の文字の索引を、決まった量まで持っておく（古く使ったものから捨てる）
#[derive(Default)]
struct IndexCache {
    entries: HashMap<PathBuf, IndexEntry>,
    order: VecDeque<PathBuf>,
    chars: usize,
}

struct IndexEntry {
    /// ためておきのファイルの更新日時
    modified: SystemTime,
    book: PathBuf,
    /// 読んだときの本の版。使い回すたびに確かめる（置き換えたり消したりした本の文字を出さないように）
    version: (u64, u64),
    /// ページの並び（`pages_match` で比べる）
    pages_key: String,
    index: Arc<SearchIndex>,
    chars: usize,
}

/// 索引から見つけた本。使い回した索引なら `recheck` に、見つかったときに確かめ直すページの並びが入る
struct Found {
    book: PathBuf,
    index: Arc<SearchIndex>,
    recheck: Option<String>,
}

/// ためておきの文字が、今の本のページの並びに合うか。PDF はファイルの版だけで決まる。
/// 書庫とフォルダは、本の中の検索と同じくページの並び（フォルダは各画像の版も）を比べる
/// （フォルダの画像を上書きしても、フォルダの版は変わらないため）。
/// サブフォルダを読むかはここではわからないので、どちらかで合えばよい
fn pages_match(book: &Path, pages_key: &str) -> bool {
    if book
        .extension()
        .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("pdf"))
    {
        return true;
    }
    !pages_key.is_empty()
        && [false, true].into_iter().any(|include_subfolders| {
            list_images(book, include_subfolders).is_some_and(|listing| {
                PageSource {
                    mode: listing.mode,
                    root: listing.root,
                    names: listing.images,
                }
                .key()
                    == pages_key
            })
        })
}

impl IndexCache {
    /// ためておきのファイル `file` の (本, 索引)。ファイルも本も変わっていなければ持っているものを使う。
    /// 本が無くなっていたり、変わっていたり（ためておきが古い）、ファイルが大きすぎたりしたら None
    fn get(&mut self, file: &Path) -> Option<Found> {
        let meta = std::fs::metadata(file).ok()?;
        let modified = meta.modified().ok()?;
        if let Some(entry) = self.entries.get(file)
            && entry.modified == modified
        {
            if version(&entry.book) != Some(entry.version) {
                self.remove(file);
                return None;
            }
            // フォルダの画像の上書きは本の版に出ないので、ページの並びは、見つかったときに確かめ直す
            // （毎回すべての本の画像を調べると重いため）
            let found = Found {
                book: entry.book.clone(),
                index: entry.index.clone(),
                recheck: Some(entry.pages_key.clone()),
            };
            self.order.retain(|path| path != file);
            self.order.push_back(file.to_path_buf());
            return Some(found);
        }
        self.remove(file);
        // 大きすぎるものは読まない（1冊の読み込みと索引づくりが長くならないよう、また手元に持てる量に収まるように）
        if meta.len() > MAX_CACHE_FILE {
            return None;
        }
        let cache: CacheFile = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
        if cache.book.is_empty() {
            return None;
        }
        let book = PathBuf::from(&cache.book);
        let book_version = version(&book)?;
        if book_version != (cache.size, cache.modified) || !pages_match(&book, &cache.pages_key) {
            return None;
        }
        let chars = cache.pages.iter().map(|page| page.chars().count()).sum();
        let index = Arc::new(SearchIndex::new(&BookText {
            pages: cache.pages,
            failed: 0,
            ocr: None,
        }));
        if chars <= INDEX_BUDGET {
            while self.chars + chars > INDEX_BUDGET {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.remove(&oldest);
            }
            self.entries.insert(
                file.to_path_buf(),
                IndexEntry {
                    modified,
                    book: book.clone(),
                    version: book_version,
                    pages_key: cache.pages_key,
                    index: index.clone(),
                    chars,
                },
            );
            self.order.push_back(file.to_path_buf());
            self.chars += chars;
        }
        Some(Found {
            book,
            index,
            recheck: None,
        })
    }

    fn remove(&mut self, file: &Path) {
        if let Some(entry) = self.entries.remove(file) {
            self.chars -= entry.chars;
            self.order.retain(|path| path != file);
        }
    }
}

/// 本文を探すためておきのファイル。同じ本に OCR の結果と埋め込まれた文字の両方があれば、OCR の結果を使う
fn text_files(cache_dir: &Path) -> Vec<PathBuf> {
    let mut names = HashSet::new();
    let mut files = Vec::new();
    for dir in [OCR_TEXT_DIR, TEXT_DIR] {
        let Ok(entries) = std::fs::read_dir(cache_dir.join(dir)) else {
            continue;
        };
        let mut found: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        found.sort();
        for path in found {
            if names.insert(path.file_name().map(|name| name.to_owned())) {
                files.push(path);
            }
        }
    }
    files
}

struct Worker {
    generation: Arc<AtomicU64>,
    closed: Arc<AtomicBool>,
    tx: Sender<(u64, Message)>,
    cache_dir: PathBuf,
    indexes: IndexCache,
}

impl Worker {
    fn run(mut self, known: Vec<NameHit>, roots: Vec<PathBuf>, requests: Receiver<(u64, String)>) {
        // 名前で探す本。履歴とお気に入りを先に、フォルダの下の本をあとに（同じ本は一度だけ）
        let mut books = Vec::new();
        let mut seen = HashSet::new();
        for book in known {
            if seen.insert(normalize(&book.path)) {
                books.push(book);
            }
        }
        let closed = self.closed.clone();
        for path in scan_books(&roots, &|| closed.load(Ordering::Relaxed)) {
            if seen.insert(normalize(&path)) {
                books.push(NameHit {
                    path,
                    from: NameFrom::Folder,
                });
            }
        }
        let current = self.generation.load(Ordering::Relaxed);
        let _ = self.tx.send((current, Message::Scanned(books.len())));
        while let Ok((mut generation, mut query)) = requests.recv() {
            // 打つ手が止まるまで待つ（そのあいだに来た新しい語だけを探す）
            loop {
                std::thread::sleep(DEBOUNCE);
                let mut newer = false;
                while let Ok(request) = requests.try_recv() {
                    (generation, query) = request;
                    newer = true;
                }
                if !newer {
                    break;
                }
            }
            if self.closed.load(Ordering::Relaxed) {
                return;
            }
            if self.stale(generation) {
                continue;
            }
            self.search(generation, &query, &books);
        }
    }

    /// 語が変わったか、窓を閉じた
    fn stale(&self, generation: u64) -> bool {
        self.closed.load(Ordering::Relaxed) || self.generation.load(Ordering::Relaxed) != generation
    }

    fn send(&self, generation: u64, message: Message) -> bool {
        self.tx.send((generation, message)).is_ok()
    }

    fn search(&mut self, generation: u64, query: &str, books: &[NameHit]) {
        let folded: String = query
            .chars()
            .filter_map(fold_char)
            .take(MAX_QUERY_CHARS)
            .collect();
        if folded.is_empty() {
            self.send(generation, Message::Done);
            return;
        }
        if !self.send(generation, Message::Names(search_names(books, &folded))) {
            return;
        }
        if folded.chars().count() >= TEXT_MIN_CHARS {
            let files = text_files(&self.cache_dir);
            let total = files.len();
            let mut found = 0;
            for (done, file) in files.iter().enumerate() {
                if self.stale(generation) {
                    return;
                }
                if let Some(Found {
                    book,
                    index,
                    recheck,
                }) = self.indexes.get(file)
                {
                    let mut hits: Vec<TextHit> = index
                        .search(query, TEXT_PER_BOOK.min(TEXT_LIMIT - found))
                        .into_iter()
                        .map(|Hit { page, snippet }| TextHit {
                            book: book.clone(),
                            page,
                            snippet,
                        })
                        .collect();
                    // 使い回した索引で見つかったら、本のページの並びが今も合うか確かめ直す
                    if !hits.is_empty()
                        && let Some(key) = recheck
                        && !pages_match(&book, &key)
                    {
                        self.indexes.remove(file);
                        hits.clear();
                    }
                    found += hits.len();
                    if !hits.is_empty() && !self.send(generation, Message::Text(hits)) {
                        return;
                    }
                }
                if !self.send(
                    generation,
                    Message::Progress {
                        done: done + 1,
                        total,
                    },
                ) {
                    return;
                }
                if found >= TEXT_LIMIT {
                    break;
                }
            }
        }
        self.send(generation, Message::Done);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manga-cross-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 書庫・PDF と、画像が直接入ったフォルダを本として集める。隠しフォルダはたどらない
    #[test]
    fn books_under_the_folders_are_collected() {
        let root = temp_dir("scan");
        std::fs::create_dir_all(root.join("sub/画像の本")).unwrap();
        std::fs::create_dir_all(root.join(".hidden")).unwrap();
        for file in [
            "a.zip",
            "sub/b.PDF",
            "sub/画像の本/01.png",
            ".hidden/c.zip",
            "note.txt",
        ] {
            std::fs::write(root.join(file), b"x").unwrap();
        }
        // たどり始めたフォルダの直下の画像は、本にしない
        std::fs::write(root.join("loose.png"), b"x").unwrap();
        let mut books = scan_books(std::slice::from_ref(&root), &|| false);
        books.sort();
        assert_eq!(
            books,
            [
                root.join("a.zip"),
                root.join("sub/b.PDF"),
                root.join("sub/画像の本")
            ]
        );
        assert_eq!(book_name(&root.join("sub/b.PDF")), "b");
        assert_eq!(book_name(&root.join("sub/画像の本")), "画像の本");
        // 止められたら、たどらない
        assert!(scan_books(std::slice::from_ref(&root), &|| true).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 結果を、終わりの知らせまで集める
    fn collect(search: &CrossSearch, generation: u64) -> (Vec<NameHit>, Vec<TextHit>) {
        let (mut names, mut texts) = (Vec::new(), Vec::new());
        loop {
            let (got, message) = search
                .messages
                .recv_timeout(Duration::from_secs(10))
                .expect("横断検索の知らせが来ない");
            if got != generation {
                continue;
            }
            match message {
                Message::Names(hits) => names = hits,
                Message::Text(hits) => texts.extend(hits),
                Message::Done => return (names, texts),
                _ => {}
            }
        }
    }

    /// 名前と、ためておいた文字（OCR の結果を優先）をまとめて探す。本が変わったためておきは使わない
    #[test]
    fn names_and_cached_text_are_searched_together() {
        let root = temp_dir("search");
        let cache = root.join("cache");
        let shelf = root.join("shelf");
        std::fs::create_dir_all(&shelf).unwrap();
        let (a, b, gone) = (
            shelf.join("仇討ちの本.pdf"),
            shelf.join("別の本.zip"),
            shelf.join("消えた本.pdf"),
        );
        for book in [&a, &b, &gone] {
            std::fs::write(book, b"x").unwrap();
        }
        let write = |dir: &str, book: &Path, pages: &[&str]| {
            let (size, modified) = version(book).unwrap();
            let file = CacheFile {
                book: book.to_string_lossy().into_owned(),
                size,
                modified,
                source: if dir == OCR_TEXT_DIR { "ocr:x" } else { "pdf" }.to_owned(),
                pages_key: String::new(),
                fingerprint: String::new(),
                digest: String::new(),
                pages: pages.iter().map(|page| page.to_string()).collect(),
                failed: 0,
            };
            let name = crate::engine::thumbnail::cover_cache_name(&book.to_string_lossy());
            std::fs::create_dir_all(cache.join(dir)).unwrap();
            std::fs::write(
                cache.join(dir).join(format!("{name}.json")),
                serde_json::to_string(&file).unwrap(),
            )
            .unwrap();
        };
        // a は埋め込まれた文字と OCR の結果の両方。OCR の結果を使う
        write(TEXT_DIR, &a, &["埋め込みの 仇 を 取る"]);
        write(
            OCR_TEXT_DIR,
            &a,
            &["表紙", "ヤーボよォ！仇を取る時が来たようだぜ"],
        );
        write(TEXT_DIR, &b, &["仇を取る話ではない"]);
        write(TEXT_DIR, &gone, &["仇を取る"]);
        std::fs::remove_file(&gone).unwrap();
        // 版が変わった本のためておきは使わない
        std::fs::write(&b, b"changed").unwrap();

        let search = CrossSearch::start(
            vec![NameHit {
                path: shelf.join("履歴の本.zip"),
                from: NameFrom::History,
            }],
            vec![shelf.clone()],
            cache.clone(),
        );
        let generation = search.search("仇を取る");
        let (names, texts) = collect(&search, generation);
        assert!(names.is_empty());
        assert_eq!(
            texts,
            [TextHit {
                book: a.clone(),
                page: 1,
                snippet: "ヤーボよォ！仇を取る時が来たようだぜ".to_owned(),
            }]
        );
        // 名前は、履歴とフォルダの下の本から。全角半角や空白はそろえて比べる
        let generation = search.search("仇討ち");
        let (names, _) = collect(&search, generation);
        assert_eq!(
            names,
            [NameHit {
                path: a.clone(),
                from: NameFrom::Folder,
            }]
        );
        let generation = search.search("履歴 の");
        assert_eq!(collect(&search, generation).0[0].from, NameFrom::History);
        // 1文字では本文は探さない
        let generation = search.search("仇");
        assert!(collect(&search, generation).1.is_empty());
        drop(search);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// ためておきを書く（テスト用）
    fn write_cache(cache: &Path, book: &Path, pages_key: String, pages: &[&str]) {
        let (size, modified) = version(book).unwrap();
        let file = CacheFile {
            book: book.to_string_lossy().into_owned(),
            size,
            modified,
            source: "ocr:x".to_owned(),
            pages_key,
            fingerprint: String::new(),
            digest: String::new(),
            pages: pages.iter().map(|page| page.to_string()).collect(),
            failed: 0,
        };
        let name = crate::engine::thumbnail::cover_cache_name(&book.to_string_lossy());
        std::fs::create_dir_all(cache.join(OCR_TEXT_DIR)).unwrap();
        std::fs::write(
            cache.join(OCR_TEXT_DIR).join(format!("{name}.json")),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
    }

    /// 窓を開いたまま本を置き換えたり消したりしたら、読んであった本文はもう出さない
    #[test]
    fn replaced_or_removed_books_drop_their_text() {
        let root = temp_dir("replace");
        let cache = root.join("cache");
        let book = root.join("本.pdf");
        std::fs::write(&book, b"x").unwrap();
        write_cache(&cache, &book, String::new(), &["仇を取る"]);
        let search = CrossSearch::start(Vec::new(), Vec::new(), cache.clone());
        let generation = search.search("仇を取る");
        assert_eq!(collect(&search, generation).1.len(), 1);
        // 同じ場所に別の本を置く（大きさが変わる）
        std::fs::write(&book, b"another book").unwrap();
        let generation = search.search("仇を取る ");
        assert!(collect(&search, generation).1.is_empty());
        std::fs::remove_file(&book).unwrap();
        let generation = search.search("仇を取る");
        assert!(collect(&search, generation).1.is_empty());
        drop(search);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// フォルダの本は、画像を上書きしたら（フォルダの版は変わらなくても）ためておきの本文を使わない
    #[test]
    fn folder_text_follows_the_images() {
        let root = temp_dir("folder");
        let cache = root.join("cache");
        let book = root.join("画像の本");
        std::fs::create_dir_all(&book).unwrap();
        image::RgbaImage::new(4, 4)
            .save(book.join("01.png"))
            .unwrap();
        let key = || {
            let listing = list_images(&book, false).unwrap();
            PageSource {
                mode: listing.mode,
                root: listing.root,
                names: listing.images,
            }
            .key()
        };
        let first_key = key();
        write_cache(&cache, &book, first_key.clone(), &["仇を取る"]);
        // 窓を開いたまま（索引を使い回して）、画像を上書きしても、前の本文は出さない
        let search = CrossSearch::start(Vec::new(), Vec::new(), cache.clone());
        let generation = search.search("仇を取る");
        assert_eq!(collect(&search, generation).1.len(), 1);
        // 画像を上書きすると、ページの並び（各画像の版）が変わる
        std::thread::sleep(Duration::from_millis(1100));
        image::RgbaImage::new(8, 8)
            .save(book.join("01.png"))
            .unwrap();
        assert_ne!(key(), first_key);
        let generation = search.search("仇を取る");
        assert!(collect(&search, generation).1.is_empty());
        drop(search);
        // 新しく開いた窓（初めて読むとき）でも出さない
        let search = CrossSearch::start(Vec::new(), Vec::new(), cache.clone());
        let generation = search.search("仇を取る");
        assert!(collect(&search, generation).1.is_empty());
        drop(search);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// 打ち直したら、前の語の検索はやめ、新しい語だけを探す
    #[test]
    fn a_new_query_replaces_the_previous_one() {
        let root = temp_dir("stale");
        std::fs::write(root.join("abc.zip"), b"x").unwrap();
        std::fs::write(root.join("xyz.zip"), b"x").unwrap();
        let search = CrossSearch::start(Vec::new(), vec![root.clone()], root.join("cache"));
        let first = search.search("abc");
        let second = search.search("xyz");
        let (names, _) = collect(&search, second);
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].path, root.join("xyz.zip"));
        // 前の語の結果は、途中で捨てられて届かない（届いても番号で見分けられる）
        while let Ok((generation, message)) = search.messages.try_recv() {
            if !matches!(message, Message::Scanned(_)) {
                assert_ne!(generation, first);
            }
        }
        drop(search);
        std::fs::remove_dir_all(root).unwrap();
    }
}
