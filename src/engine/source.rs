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
pub fn list_images(path: &Path) -> Option<Listing> {
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
                if let Ok(entries) = fs::read_dir(&root) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_file()
                            && IMAGE_EXTENSIONS.iter().any(|&e| {
                                p.extension()
                                    .map_or(false, |ext| ext.to_string_lossy().to_lowercase() == e)
                            })
                        {
                            images.push(p.to_string_lossy().to_string());
                        }
                    }
                }
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

/// Reads page bytes out of a source, keeping a ZIP archive open across reads.
pub struct SourceReader<'a> {
    mode: SourceMode,
    path: &'a Path,
    zip: Option<zip::ZipArchive<File>>,
}

impl<'a> SourceReader<'a> {
    pub fn open(mode: SourceMode, path: &'a Path) -> Self {
        let zip = if mode == SourceMode::Zip {
            File::open(path)
                .ok()
                .and_then(|f| zip::ZipArchive::new(f).ok())
        } else {
            None
        };
        Self { mode, path, zip }
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
            SourceMode::Rar => read_rar_entry(self.path, name),
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

        let listing = list_images(&archive).unwrap();
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
        assert!(list_images(&dir.0.join("missing.zip")).is_none());
    }

    #[test]
    fn opening_an_image_lists_its_folder_and_starts_there() {
        let dir = TempDir::new();
        for name in ["a.png", "b.webp", "c.txt"] {
            fs::write(dir.0.join(name), name).unwrap();
        }
        fs::create_dir(dir.0.join("nested.png")).unwrap();

        let opened = dir.0.join("b.webp");
        let listing = list_images(&opened).unwrap();
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
}
