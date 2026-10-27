use crate::config::SourceMode;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "bmp", "webp", "gif", "tiff", "tga", "avif",
];

/// Pages found in a ZIP/RAR archive or an image folder, in listing order.
pub struct Listing {
    pub mode: SourceMode,
    /// The archive itself, or the folder holding the images.
    pub root: PathBuf,
    /// Entry names for archives, full file paths for folders.
    pub images: Vec<String>,
    /// When a single image was opened, the page to start at.
    pub start_at: Option<String>,
}

/// Lists the pages of `path`. A single image file opens its whole folder.
/// PDFs are not handled here: their pages come from the loaded document.
/// Returns `None` when a ZIP archive cannot be opened at all.
/// `include_subfolders` が真なら、フォルダの下のサブフォルダの画像もページにする（MM-SRC-06）
pub fn list_images(path: &Path, include_subfolders: bool) -> Option<Listing> {
    let mut root = path.to_path_buf();
    let mut start_at = None;
    let mut images = Vec::new();
    let extension = path
        .extension()
        .map_or("".to_string(), |ext| ext.to_string_lossy().to_lowercase());
    let mode = match extension.as_str() {
        "zip" | "cbz" => {
            let file = File::open(path).ok()?;
            if let Ok(mut archive) = zip::ZipArchive::new(file) {
                for i in 0..archive.len() {
                    if let Ok(f) = archive.by_index(i) {
                        let name = f.name().to_lowercase();
                        if IMAGE_EXTENSIONS
                            .iter()
                            .any(|&e| name.ends_with(&format!(".{}", e)))
                        {
                            images.push(f.name().to_string());
                        }
                    }
                }
            }
            SourceMode::Zip
        }
        "cbr" | "rar" => {
            if let Ok(archive) = unrar::Archive::new(path).open_for_listing() {
                for entry in archive {
                    if let Ok(e) = entry {
                        // Convert Option<&str> to String safely
                        if let Some(name_str) = e.filename.to_str() {
                            let name = name_str.to_string();
                            // Check if it's an image extension
                            if IMAGE_EXTENSIONS
                                .iter()
                                .any(|&e_ext| name.to_lowercase().ends_with(e_ext))
                            {
                                images.push(name);
                            }
                        }
                    }
                }
            }
            SourceMode::Rar
        }
        _ => {
            if path.is_file() {
                // Pivot: Use the folder containing this image as the source
                if let Some(parent) = path.parent() {
                    start_at = Some(path.to_string_lossy().to_string());
                    root = parent.to_path_buf();
                }
            }

            if root.is_dir() {
                collect_folder_images(&root, include_subfolders, &mut images);
            }
            SourceMode::Folder
        }
    };
    Some(Listing {
        mode,
        root,
        images,
        start_at,
    })
}

fn is_image_file(path: &Path) -> bool {
    path.is_file()
        && path.extension().is_some_and(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            IMAGE_EXTENSIONS.contains(&ext.as_str())
        })
}

/// サブフォルダとして読むフォルダか。隠しフォルダと、ループのおそれがあるシンボリックリンクは読まない
fn is_readable_subfolder(entry: &fs::DirEntry) -> bool {
    entry.file_type().is_ok_and(|kind| kind.is_dir())
        && !entry.file_name().to_string_lossy().starts_with('.')
}

fn collect_folder_images(folder: &Path, include_subfolders: bool, images: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_image_file(&path) {
            images.push(path.to_string_lossy().to_string());
        } else if include_subfolders && is_readable_subfolder(&entry) {
            collect_folder_images(&path, true, images);
        }
    }
}

/// `path` を開いたときに冊になるフォルダ。画像ファイルならそのフォルダ。書庫や PDF なら None
pub fn book_folder(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else if is_image_file(path) {
        path.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// フォルダの下に、画像の入ったサブフォルダがあるか（孫以下のフォルダも含む）
pub fn has_image_subfolders(folder: &Path) -> bool {
    let Ok(entries) = fs::read_dir(folder) else {
        return false;
    };
    entries.flatten().any(|entry| {
        if !is_readable_subfolder(&entry) {
            return false;
        }
        let mut images = Vec::new();
        collect_folder_images(&entry.path(), true, &mut images);
        !images.is_empty()
    })
}

/// Reads page bytes out of a source, keeping a ZIP archive open across reads.
/// パスは自分で持つので、別スレッドに渡して使える
pub struct SourceReader {
    mode: SourceMode,
    path: PathBuf,
    zip: Option<zip::ZipArchive<File>>,
}

impl SourceReader {
    pub fn open(mode: SourceMode, path: &Path) -> Self {
        let zip = if mode == SourceMode::Zip {
            File::open(path)
                .ok()
                .and_then(|f| zip::ZipArchive::new(f).ok())
        } else {
            None
        };
        Self {
            mode,
            path: path.to_path_buf(),
            zip,
        }
    }

    /// `name` is an entry of [`Listing::images`]. PDF pages are rendered, not read.
    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        match self.mode {
            SourceMode::Folder => fs::read(name).ok(),
            SourceMode::Zip => {
                let mut entry = self.zip.as_mut()?.by_name(name).ok()?;
                let mut buffer = Vec::new();
                entry.read_to_end(&mut buffer).ok()?;
                Some(buffer)
            }
            SourceMode::Rar => read_rar_entry(&self.path, name),
            SourceMode::Pdf => None,
        }
    }
}

pub fn read_entry(mode: SourceMode, path: &Path, name: &str) -> Option<Vec<u8>> {
    SourceReader::open(mode, path).read(name)
}

fn read_rar_entry(path: &Path, name: &str) -> Option<Vec<u8>> {
    let mut cursor = unrar::Archive::new(path)
        .open_for_processing()
        .ok()?
        .read_header()
        .ok()
        .flatten();
    loop {
        let e = cursor?;
        // Entries whose names are not valid UTF-8 can never match
        if e.entry().filename.to_str() == Some(name) {
            return e.read().ok().map(|(bytes, _arc)| bytes);
        }
        cursor = e
            .skip()
            .ok()
            .and_then(|arc| arc.read_header().ok().flatten());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "manga-source-test-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn zip_lists_only_images_and_reads_them() {
        let dir = TempDir::new();
        let archive = dir.0.join("book.CBZ");
        write_zip(
            &archive,
            &[
                ("p2.PNG", b"two"),
                ("notes.txt", b"skip"),
                ("sub/p1.jpg", b"one"),
            ],
        );

        let listing = list_images(&archive, false).unwrap();
        assert!(listing.mode == SourceMode::Zip);
        assert_eq!(listing.root, archive);
        assert_eq!(listing.images, ["p2.PNG", "sub/p1.jpg"]);
        assert_eq!(listing.start_at, None);

        let mut reader = SourceReader::open(SourceMode::Zip, &archive);
        assert_eq!(reader.read("sub/p1.jpg").as_deref(), Some(&b"one"[..]));
        assert_eq!(reader.read("p2.PNG").as_deref(), Some(&b"two"[..]));
        assert_eq!(reader.read("missing.png"), None);
        assert_eq!(
            read_entry(SourceMode::Zip, &archive, "p2.PNG").as_deref(),
            Some(&b"two"[..])
        );
    }

    #[test]
    fn missing_zip_cannot_be_listed() {
        let dir = TempDir::new();
        assert!(list_images(&dir.0.join("missing.zip"), false).is_none());
    }

    #[test]
    fn opening_an_image_lists_its_folder_and_starts_there() {
        let dir = TempDir::new();
        for name in ["a.png", "b.webp", "c.txt"] {
            fs::write(dir.0.join(name), name).unwrap();
        }
        fs::create_dir(dir.0.join("nested.png")).unwrap();

        let opened = dir.0.join("b.webp");
        let listing = list_images(&opened, false).unwrap();
        assert!(listing.mode == SourceMode::Folder);
        assert_eq!(listing.root, dir.0);
        let mut images = listing.images.clone();
        images.sort();
        let expected: Vec<String> = ["a.png", "b.webp"]
            .iter()
            .map(|n| dir.0.join(n).to_string_lossy().to_string())
            .collect();
        assert_eq!(images, expected);
        assert_eq!(listing.start_at, Some(opened.to_string_lossy().to_string()));

        let mut reader = SourceReader::open(SourceMode::Folder, &dir.0);
        assert_eq!(reader.read(&expected[0]).as_deref(), Some(&b"a.png"[..]));
    }

    #[test]
    fn subfolders_are_read_only_when_asked() {
        let dir = TempDir::new();
        for name in ["a.png", "vol1/b.png", "vol1/sub/c.png", ".hidden/d.png"] {
            let path = dir.0.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
        fs::create_dir(dir.0.join("empty")).unwrap();
        let names = |include| {
            let mut images: Vec<String> = list_images(&dir.0, include)
                .unwrap()
                .images
                .iter()
                .map(|p| {
                    Path::new(p)
                        .strip_prefix(&dir.0)
                        .unwrap()
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            images.sort();
            images
        };
        assert_eq!(names(false), ["a.png"]);
        // 隠しフォルダは読まない
        assert_eq!(names(true), ["a.png", "vol1/b.png", "vol1/sub/c.png"]);

        assert!(has_image_subfolders(&dir.0));
        assert!(!has_image_subfolders(&dir.0.join("vol1/sub")));
        // 画像の無いサブフォルダや隠しフォルダだけなら、サブフォルダは無いのと同じ
        fs::remove_dir_all(dir.0.join("vol1")).unwrap();
        assert!(!has_image_subfolders(&dir.0));

        assert_eq!(book_folder(&dir.0), Some(dir.0.clone()));
        assert_eq!(book_folder(&dir.0.join("a.png")), Some(dir.0.clone()));
        assert_eq!(book_folder(&dir.0.join("book.zip")), None);
    }
}
