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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};

/// 本の、ページごとの文字
#[derive(Default, Debug, PartialEq)]
pub struct BookText {
    pub pages: Vec<String>,
    /// 文字を読めなかったページの数（そのページは探せない）
    pub failed: usize,
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
/// （消したり動かしたりした本のぶんが、たまり続けないように）
const CACHE_LIMIT: usize = 300;

/// ためておくファイルの中身。本が変わったか見分けるため、大きさと更新日時も書く
#[derive(Serialize, Deserialize)]
struct CacheFile {
    size: u64,
    modified: u64,
    /// どう読んだか（"pdf" は透明テキスト、"ocr:<プラグインの id>" は OCR）
    #[serde(default)]
    source: String,
    /// 読んだときのページの並び（`PageSource::key`）。今の並びと違えば使わない
    #[serde(default)]
    pages_key: String,
    pages: Vec<String>,
    /// OCR で読めなかったページの数（そのページは空）
    #[serde(default)]
    failed: usize,
}

/// 本の版（大きさ, 更新日時）
fn version(book: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(book).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    Some((meta.len(), modified))
}

/// ためておくファイル。本の場所ごとに1つ（本が変わったら書き直す）
fn cache_path(book: &Path, cache_dir: &Path) -> PathBuf {
    cache_dir.join("text").join(format!(
        "{}.json",
        cover_cache_name(&book.to_string_lossy())
    ))
}

/// ためておいた文字。本が変わっていれば（ページの並びが `pages_key` と違えば）None。
/// 使ったものとして、ファイルの時刻を新しくする
fn load_cache(book: &Path, cache_dir: &Path, pages_key: &str) -> Option<BookText> {
    let path = cache_path(book, cache_dir);
    let cache: CacheFile = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
    if version(book)? != (cache.size, cache.modified) || cache.pages_key != pages_key {
        return None;
    }
    if let Ok(file) = std::fs::File::options().write(true).open(&path) {
        let _ = file.set_modified(SystemTime::now());
    }
    Some(BookText {
        pages: cache.pages,
        failed: cache.failed,
    })
}

/// 取り出した文字をためておき、上限を超えたら古いものから消す
fn save_cache(book: &Path, cache_dir: &Path, text: &BookText, source: &str, pages_key: &str) {
    let Some((size, modified)) = version(book) else {
        return;
    };
    let path = cache_path(book, cache_dir);
    let cache = CacheFile {
        size,
        modified,
        source: source.to_owned(),
        pages_key: pages_key.to_owned(),
        pages: text.pages.clone(),
        failed: text.failed,
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(json) = serde_json::to_string(&cache)
    {
        let _ = crate::engine::settings_store::write_atomic(&path, json.as_bytes());
    }
    prune_cache(&cache_dir.join("text"), CACHE_LIMIT);
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

/// PDF の文字を取り出す（ためておいたものがあればそれを使う）。止められたら None。
/// 読めなかったページがあれば、ためておかない（次に開いたときにまた取り出す）
fn pdf_text(
    pdfium: SharedPdfium,
    book: &Path,
    cache_dir: &Path,
    progress: &TextProgress,
    cancel: &AtomicBool,
) -> Option<BookText> {
    // PDF のページの並びは、ファイルの版で決まる
    if let Some(text) = load_cache(book, cache_dir, "") {
        return Some(text);
    }
    let document = pdfium.open(book)?;
    let count = document.page_count();
    progress.total.store(count, Ordering::Relaxed);
    let mut text = BookText {
        pages: Vec::with_capacity(count),
        failed: 0,
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
    }
    if text.failed == 0 {
        save_cache(book, cache_dir, &text, "pdf", "");
    }
    Some(text)
}

/// 取り出した本の文字
pub struct LoadedText {
    pub index: SearchIndex,
    /// 文字が1つも無い（透明テキストの無い本）
    pub empty: bool,
    /// 文字を読めなかったページの数
    pub failed: usize,
}

impl LoadedText {
    fn new(text: &BookText) -> Self {
        Self {
            index: SearchIndex::new(text),
            empty: text.is_empty(),
            failed: text.failed,
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
        let text = match pdfium {
            Some(pdfium) => pdf_text(pdfium, &book, &cache_dir, &progress, &cancel),
            None => Some(load_cache(&book, &cache_dir, &pages.key()).unwrap_or_default()),
        };
        let _ = tx.send(text.as_ref().map(LoadedText::new));
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
                let path = work.join(format!("page-{page:05}.png"));
                write_ocr_image(source, &mut reader, pdf.as_ref(), page, &path)?;
                Some(OcrImage { page, path })
            })
            .collect();
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
fn fold_char(c: char) -> Option<char> {
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
        BookText { pages, failed: 0 }
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
        assert!(cache_path(&book, &dir.join("cache")).is_file());
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
        let text = book_text(vec!["一".to_owned(), "二".to_owned()]);
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
}
