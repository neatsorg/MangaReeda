//! PDF の文書。pdfium は同時に呼ぶと壊れる（pdfium-render は初期化のときにしか排他をかけない）ので、
//! 開く・数える・描く・閉じるのすべてで、ここの鍵を取る。画面に出すページは画面のスレッドで、
//! 表紙は表紙を作るスレッドで描くため。

use image::DynamicImage;
use pdfium_render::prelude::{PdfDocument, Pdfium, Pixels};
use std::path::Path;
use std::sync::{Mutex, MutexGuard, OnceLock};

static LOCK: Mutex<()> = Mutex::new(());
/// プロセスで1つだけの pdfium。pdfium-render は2つ目を作ると1つ目が無くなるまで待つので、使い回す
static PDFIUM: OnceLock<Option<SharedPdfium>> = OnceLock::new();

/// pdfium のライブラリを読む（初めの一度だけ）。見つからなければ None。
/// 実行ファイルの隣、次にシステムのライブラリを探す。作業フォルダは探さない（展開した本のフォルダなどから
/// 起動したときに、そこに置かれた別のライブラリを読み込んでしまわないように）
pub fn pdfium() -> Option<SharedPdfium> {
    *PDFIUM.get_or_init(|| {
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
            .and_then(|dir| {
                Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir)).ok()
            });
        // Windows では名前だけで探すと作業フォルダも探してしまうので、実行ファイルの隣だけを使う
        // （pdfium は Windows のシステムに入っているライブラリではない）
        #[cfg(windows)]
        let bindings = beside_exe;
        #[cfg(not(windows))]
        let bindings = beside_exe.or_else(|| Pdfium::bind_to_system_library().ok());
        let Some(bindings) = bindings else {
            eprintln!("pdfium library not found; PDF files cannot be opened.");
            return None;
        };
        let _lock = lock();
        Some(SharedPdfium(Box::leak(Box::new(Pdfium::new(bindings)))))
    })
}

fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// スレッドをまたいで渡せる pdfium。使うのは `PdfDoc` を通してだけで、そこで必ず鍵を取るので、
/// 同時に呼ばれることはない
#[derive(Clone, Copy)]
pub struct SharedPdfium(&'static Pdfium);

// SAFETY: pdfium は `PdfDoc` の中でだけ、鍵を取ってから呼ぶ
unsafe impl Send for SharedPdfium {}
unsafe impl Sync for SharedPdfium {}

impl SharedPdfium {
    pub fn open(self, path: &Path) -> Option<PdfDoc> {
        PdfDoc::open(self.0, path)
    }
}

/// 開いた PDF。閉じるときにも鍵を取る
pub struct PdfDoc(Option<PdfDocument<'static>>);

impl PdfDoc {
    fn open(pdfium: &'static Pdfium, path: &Path) -> Option<Self> {
        let _lock = lock();
        pdfium
            .load_pdf_from_file(path, None)
            .ok()
            .map(|document| Self(Some(document)))
    }

    pub fn page_count(&self) -> usize {
        let _lock = lock();
        self.0
            .as_ref()
            .map_or(0, |document| usize::from(document.pages().len()))
    }

    /// `index` のページの文字（透明テキストを含む）。文字が無ければ空
    pub fn page_text(&self, index: usize) -> Option<String> {
        let _lock = lock();
        let page = self
            .0
            .as_ref()?
            .pages()
            .get(u16::try_from(index).ok()?)
            .ok()?;
        let text = page.text().ok()?.all();
        Some(text)
    }

    /// `index` のページを、高さ `height` ピクセルで描く。`max_width` があれば、幅がそれを超えないよう縮める
    pub fn render(
        &self,
        index: usize,
        height: f32,
        max_width: Option<f32>,
    ) -> Option<DynamicImage> {
        let _lock = lock();
        let page = self
            .0
            .as_ref()?
            .pages()
            .get(u16::try_from(index).ok()?)
            .ok()?;
        let (width_pt, height_pt) = (page.width().value, page.height().value);
        if width_pt <= 0.0 || height_pt <= 0.0 {
            return None;
        }
        let mut scale = height / height_pt;
        if let Some(max_width) = max_width {
            scale = scale.min(max_width / width_pt);
        }
        let bitmap = page
            .render(
                (width_pt * scale).max(1.0) as Pixels,
                (height_pt * scale).max(1.0) as Pixels,
                None,
            )
            .ok()?;
        Some(bitmap.as_image())
    }
}

impl Drop for PdfDoc {
    fn drop(&mut self) {
        let _lock = lock();
        self.0.take();
    }
}
