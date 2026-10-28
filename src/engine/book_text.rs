//! 本の文字（いまは PDF の透明テキスト）を取り出し、ためておき、検索する。
//! 取り出した文字は、本の場所・大きさ・更新日時ごとにファイルにためておき、次からはそれを使う。

use crate::engine::pdf::SharedPdfium;
use crate::engine::thumbnail::cover_cache_name;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::UNIX_EPOCH;

/// 本の、ページごとの文字
#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
pub struct BookText {
    pub pages: Vec<String>,
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

/// ためておくファイル。本が変わったら（大きさか更新日時が違えば）別のファイルになる
fn cache_path(book: &Path, cache_dir: &Path) -> Option<PathBuf> {
    let meta = std::fs::metadata(book).ok()?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let key = format!("{}\n{}\n{modified}", book.display(), meta.len());
    Some(
        cache_dir
            .join("text")
            .join(format!("{}.json", cover_cache_name(&key))),
    )
}

/// PDF の文字を取り出す（ためておいたものがあればそれを使う）。止められたら None
fn pdf_text(
    pdfium: SharedPdfium,
    book: &Path,
    cache_dir: &Path,
    progress: &TextProgress,
    cancel: &AtomicBool,
) -> Option<BookText> {
    let cache = cache_path(book, cache_dir);
    if let Some(text) = cache
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|data| serde_json::from_str::<BookText>(&data).ok())
    {
        return Some(text);
    }
    let document = pdfium.open(book)?;
    let count = document.page_count();
    progress.total.store(count, Ordering::Relaxed);
    let mut pages = Vec::with_capacity(count);
    for index in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        // ページごとに鍵を取るので、画面のページを描くのを長く止めない
        pages.push(document.page_text(index).unwrap_or_default());
        progress.done.store(index + 1, Ordering::Relaxed);
    }
    let text = BookText { pages };
    if let Some(path) = cache
        && let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_ok()
        && let Ok(json) = serde_json::to_string(&text)
    {
        let _ = crate::engine::settings_store::write_atomic(&path, json.as_bytes());
    }
    Some(text)
}

/// 本の文字を別のスレッドで取り出す。`cancel` を立てると止まる
pub fn spawn_book_text(
    pdfium: SharedPdfium,
    book: PathBuf,
    cache_dir: PathBuf,
    progress: Arc<TextProgress>,
    cancel: Arc<AtomicBool>,
) -> Receiver<Option<BookText>> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(pdf_text(pdfium, &book, &cache_dir, &progress, &cancel));
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

/// `query` を本の中から探す。空白は無視するので、行をまたいでも見つかる。多くても `limit` 件まで
pub fn search(text: &BookText, query: &str, limit: usize) -> Vec<Hit> {
    let query: Vec<char> = query.chars().filter_map(fold_char).collect();
    if query.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for (page, content) in text.pages.iter().enumerate() {
        // 空白を除いた元の文字と、比べやすい形の文字を、同じ位置で並べる
        let (kept, folded): (Vec<char>, Vec<char>) = content
            .chars()
            .filter_map(|c| fold_char(c).map(|folded| (c, folded)))
            .unzip();
        let mut start = 0;
        while start + query.len() <= folded.len() {
            if folded[start..start + query.len()] == query[..] {
                let from = start.saturating_sub(SNIPPET_AROUND);
                let to = (start + query.len() + SNIPPET_AROUND).min(kept.len());
                let mut snippet: String = kept[from..to].iter().collect();
                if from > 0 {
                    snippet.insert(0, '…');
                }
                if to < kept.len() {
                    snippet.push('…');
                }
                hits.push(Hit { page, snippet });
                if hits.len() >= limit {
                    return hits;
                }
                start += query.len();
            } else {
                start += 1;
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_ignore_spaces_and_vertical_forms() {
        let text = BookText {
            pages: vec![
                "表紙".to_owned(),
                "れ て です ね︒ でも︑ お願 いだ から\nこれ だ け は\n﹃フ ァー マシ ィー﹄"
                    .to_owned(),
                "ＭａｎｇａＲｅｅｄａ で読む".to_owned(),
            ],
        };
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
        assert!(
            BookText {
                pages: vec![" ".to_owned()]
            }
            .is_empty()
        );
    }

    #[test]
    fn hits_are_limited_and_snippets_are_cut_around_the_word() {
        let text = BookText {
            pages: vec!["あ".repeat(40) + "語" + &"い".repeat(40) + "語"],
        };
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

    /// 取り出した文字はためておき、本が変わったら別のものにする
    #[test]
    fn extracted_text_is_cached_per_book_version() {
        let dir = std::env::temp_dir().join(format!("manga-book-text-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let book = dir.join("a.pdf");
        std::fs::write(&book, "1").unwrap();
        let first = cache_path(&book, &dir).unwrap();
        assert_eq!(first, cache_path(&book, &dir).unwrap());
        std::fs::write(&book, "22").unwrap();
        assert_ne!(first, cache_path(&book, &dir).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
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
            pdfium,
            book.clone(),
            dir.join("cache"),
            progress.clone(),
            Arc::new(AtomicBool::new(false)),
        );
        let text = rx.recv().unwrap().unwrap();
        assert_eq!(text.pages.len(), 2);
        assert_eq!(progress.done.load(Ordering::Relaxed), 2);
        let hits = search(&text, "mangareeda", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].page, 1);
        // 2回目は、ためておいたものを使う
        assert!(cache_path(&book, &dir.join("cache")).unwrap().is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
