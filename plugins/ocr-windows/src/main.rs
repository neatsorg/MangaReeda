//! MangaReeda の OCR プラグイン。ページの画像の文字を、Windows に入っている OCR（Windows.Media.Ocr）で読む。
//!
//! 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
//!   mangareeda-ocr-windows ocr
//! 標準入力に {"credentials": {}, "images": [{"page": 0, "path": "…"}, …]} を受け取り、
//! 読み終えたページから {"page": 0, "text": "…"} を1行ずつ書く。
//!
//! Windows の日本語の OCR の部品が要る（Windows の設定で日本語を追加すると入る）。Windows 以外では動かない。

use serde::Deserialize;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "使い方: mangareeda-ocr-windows ocr\n\
     MangaReeda から呼ばれ、標準入力に1行の JSON で画像を受け取ります（docs/plugins.md）";

#[derive(Deserialize, Default)]
struct Request {
    #[serde(default)]
    images: Vec<Image>,
}

#[derive(Deserialize)]
#[cfg_attr(not(windows), allow(dead_code))]
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
    match windows_ocr::read_images(&request.images) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(windows)]
mod windows_ocr {
    use super::Image;
    use serde_json::json;
    use std::io::Write;
    use std::path::Path;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::{FileAccessMode, StorageFile};
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    use windows::core::HSTRING;

    /// 日本語の OCR が入っていないときの知らせ
    const NOT_INSTALLED: &str = "Windows の日本語の OCR が入っていません。\
         Windows の設定の「時刻と言語」→「言語」で日本語を追加し、文字認識（OCR）の部品を入れてください";

    /// 画像を順に読み、読めたものから書く。読めなかったページは書かない
    pub fn read_images(images: &[Image]) -> Result<(), String> {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let language = Language::CreateLanguage(&HSTRING::from("ja")).map_err(|e| e.to_string())?;
        let engine = OcrEngine::IsLanguageSupported(&language)
            .unwrap_or(false)
            .then(|| OcrEngine::TryCreateFromLanguage(&language).ok())
            .flatten()
            .ok_or_else(|| NOT_INSTALLED.to_owned())?;
        let mut stdout = std::io::stdout();
        for image in images {
            let Ok(text) = read_image(&engine, &image.path) else {
                continue;
            };
            let _ = writeln!(stdout, "{}", json!({ "page": image.page, "text": text }));
            let _ = stdout.flush();
        }
        Ok(())
    }

    /// 1枚の画像を読む。行ごとに改行でつなぐ
    fn read_image(engine: &OcrEngine, path: &Path) -> windows::core::Result<String> {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
        let stream = file.OpenAsync(FileAccessMode::Read)?.join()?;
        let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
        let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
        let result = engine.RecognizeAsync(&bitmap)?.join()?;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            lines.push(line.Text()?.to_string_lossy());
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(not(windows))]
mod windows_ocr {
    use super::Image;

    pub fn read_images(_images: &[Image]) -> Result<(), String> {
        Err("このプラグインは Windows 専用です".to_owned())
    }
}
