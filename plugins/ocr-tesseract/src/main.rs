//! MangaReeda の OCR プラグイン。ページの画像の文字を Tesseract で読む。
//!
//! 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
//!   mangareeda-ocr-tesseract ocr
//! 標準入力に {"credentials": {}, "images": [{"page": 0, "path": "…"}, …]} を受け取り、
//! 読み終えたページから {"page": 0, "text": "…"} を1行ずつ書く。
//!
//! Tesseract は別に入れておく（Arch Linux なら `pacman -S tesseract tesseract-data-jpn_vert`）。
//! 縦書き用の日本語のデータ（jpn_vert）があればそれを、無ければ横書き用（jpn）を使う。

use serde::Deserialize;
use serde_json::json;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

const USAGE: &str = "使い方: mangareeda-ocr-tesseract ocr\n\
     MangaReeda から呼ばれ、標準入力に1行の JSON で画像を受け取ります（docs/plugins.md）";

/// 入っていないときの知らせ
const NOT_INSTALLED: &str = "Tesseract と日本語のデータが見つかりません。\
     Arch Linux なら `pacman -S tesseract tesseract-data-jpn_vert` で入れてください";

#[derive(Deserialize, Default)]
struct Request {
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
struct Image {
    page: usize,
    path: PathBuf,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 使い方は、標準入力を待たずに出す
    if args.first().map(String::as_str) != Some("ocr") {
        println!("{USAGE}");
        return if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let request: Request = serde_json::from_str(input.trim()).unwrap_or_default();
    let Some((program, language)) = find_tesseract() else {
        eprintln!("{NOT_INSTALLED}");
        return ExitCode::FAILURE;
    };
    read_images(&program, language, &request.images);
    ExitCode::SUCCESS
}

/// tesseract の命令と、使う言語のデータ。見つからなければ None
fn find_tesseract() -> Option<(PathBuf, &'static str)> {
    let mut candidates = vec![PathBuf::from("tesseract")];
    if cfg!(windows) {
        // Windows の Tesseract の入れ方の、ふつうの置き場所
        candidates.push(PathBuf::from(
            r"C:\Program Files\Tesseract-OCR\tesseract.exe",
        ));
    }
    candidates.into_iter().find_map(|program| {
        let output = Command::new(&program)
            .arg("--list-langs")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let languages = String::from_utf8_lossy(&output.stdout).into_owned();
        let language = choose_language(&languages)?;
        Some((program, language))
    })
}

/// `tesseract --list-langs` の答えから、使う言語を選ぶ（縦書き用が先）
fn choose_language(languages: &str) -> Option<&'static str> {
    let has = |name: &str| languages.lines().any(|line| line.trim() == name);
    if has("jpn_vert") {
        Some("jpn_vert")
    } else if has("jpn") {
        Some("jpn")
    } else {
        None
    }
}

/// 画像を何枚かずつ並べて読み、読めたものから書く。読めなかったページは書かない
fn read_images(program: &Path, language: &str, images: &[Image]) {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get() / 2)
        .clamp(1, 4)
        .min(images.len().max(1));
    let next = AtomicUsize::new(0);
    let stdout = Mutex::new(io::stdout());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while let Some(image) = images.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let Some(text) = read_image(program, language, &image.path) else {
                        continue;
                    };
                    let line = json!({ "page": image.page, "text": text });
                    if let Ok(mut stdout) = stdout.lock() {
                        let _ = writeln!(stdout, "{line}");
                        let _ = stdout.flush();
                    }
                }
            });
        }
    });
}

/// 1枚の画像を読む。並べて読むので、tesseract の中では1つのスレッドだけを使わせる
fn read_image(program: &Path, language: &str, path: &Path) -> Option<String> {
    let output = Command::new(program)
        .arg(path)
        .arg("stdout")
        .args(["-l", language])
        .env("OMP_THREAD_LIMIT", "1")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_japanese_is_preferred() {
        assert_eq!(
            choose_language("List of languages:\njpn\njpn_vert\nosd\n"),
            Some("jpn_vert")
        );
        assert_eq!(choose_language("jpn\neng\n"), Some("jpn"));
        assert_eq!(choose_language("eng\nosd\n"), None);
    }
}
