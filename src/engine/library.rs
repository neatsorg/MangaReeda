use std::fs;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use windows::Win32::UI::Shell::StrCmpLogicalW;
#[cfg(windows)]
use windows::core::PCWSTR;

/// Natural alphanumeric sorting by file name ("page2" before "page10").
/// Windows uses Explorer's own ordering; elsewhere `nat_cmp` approximates it.
#[cfg(windows)]
pub fn natural_sort(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        // Convert OsStr to null-terminated Wide Strings (UTF-16) for Windows API
        let a_name: Vec<u16> = a
            .file_name()
            .unwrap_or_default()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let b_name: Vec<u16> = b
            .file_name()
            .unwrap_or_default()
            .encode_wide()
            .chain(Some(0))
            .collect();

        let result = unsafe { StrCmpLogicalW(PCWSTR(a_name.as_ptr()), PCWSTR(b_name.as_ptr())) };

        match result {
            r if r < 0 => std::cmp::Ordering::Less,
            r if r > 0 => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    });
}

/// Natural alphanumeric sorting specifically for String vectors
#[cfg(windows)]
pub fn natural_sort_strings(strings: &mut [String]) {
    strings.sort_by(|a, b| {
        // Convert Strings to null-terminated UTF-16 for the Windows API
        let a_name: Vec<u16> = Path::new(a)
            .file_name()
            .unwrap_or_default()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let b_name: Vec<u16> = Path::new(b)
            .file_name()
            .unwrap_or_default()
            .encode_wide()
            .chain(Some(0))
            .collect();

        let result = unsafe { StrCmpLogicalW(PCWSTR(a_name.as_ptr()), PCWSTR(b_name.as_ptr())) };

        match result {
            r if r < 0 => std::cmp::Ordering::Less,
            r if r > 0 => std::cmp::Ordering::Greater,
            _ => std::cmp::Ordering::Equal,
        }
    });
}

pub fn scan_folder(current_parent: &Path) -> Vec<PathBuf> {
    let mut items = Vec::new();
    if let Ok(entries) = fs::read_dir(current_parent) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_zip = path.extension().map_or(false, |ext| {
                ext == "zip" || ext == "pdf" || ext == "cbz" || ext == "cbr" || ext == "rar"
            });
            // Treat non-hidden directories as readable manga sources
            let is_dir = path.is_dir()
                && !path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .starts_with('.');

            if is_zip || is_dir {
                items.push(path);
            }
        }
    }
    natural_sort(&mut items);
    items
}

pub fn get_adjacent_directories(
    path_with_file_name: Option<PathBuf>,
) -> (Option<PathBuf>, Option<PathBuf>) {
    // Unwrap the Option to get the actual PathBuf
    let path = match path_with_file_name {
        Some(p) => p,
        None => return (None, None),
    };

    let path = match path.parent() {
        Some(p) => p,
        None => return (None, None),
    };

    let root_dir = match path.parent() {
        Some(p) => p,
        None => return (None, None),
    };

    // Collect all valid sibling directories
    let mut dirs: Vec<PathBuf> = fs::read_dir(root_dir)
        .ok()
        .map(|read_dir| {
            read_dir
                .filter_map(|entry| {
                    let p = entry.ok()?.path();
                    // Ensure it's a directory and not a hidden file
                    if p.is_dir() && !p.file_name()?.to_str()?.starts_with('.') {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    // Sort using Windows natural alphanumeric order (test2 before test10)
    natural_sort(&mut dirs);

    // Find where we are
    let current_pos = dirs.iter().position(|p| *p == path);

    match current_pos {
        Some(pos) => {
            let prev = if pos > 0 {
                Some(dirs[pos - 1].clone())
            } else {
                None
            };
            let next = if pos + 1 < dirs.len() {
                Some(dirs[pos + 1].clone())
            } else {
                None
            };
            (prev, next)
        }
        None => (None, None),
    }
}

#[cfg(not(windows))]
fn nat_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(&c) = x.peek() {
                    if c.is_ascii_digit() {
                        n1.push(c);
                        x.next();
                    } else {
                        break;
                    }
                }
                let mut n2 = String::new();
                while let Some(&c) = y.peek() {
                    if c.is_ascii_digit() {
                        n2.push(c);
                        y.next();
                    } else {
                        break;
                    }
                }
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let o = t1.len().cmp(&t2.len()).then(t1.cmp(t2));
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                let o = c.to_lowercase().cmp(d.to_lowercase());
                if o != std::cmp::Ordering::Equal {
                    return o;
                }
                x.next();
                y.next();
            }
        }
    }
}
#[cfg(not(windows))]
pub fn natural_sort(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        nat_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
}
#[cfg(not(windows))]
pub fn natural_sort_strings(strings: &mut [String]) {
    strings.sort_by(|a, b| {
        nat_cmp(
            &Path::new(a)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            &Path::new(b)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// spec/sort-check/names.txt の名前（examples/sort_check_book.rs と同じ読み方）
    fn sort_check_names() -> Vec<String> {
        include_str!("../../spec/sort-check/names.txt")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("# ") && *line != "#")
            .map(str::to_owned)
            .collect()
    }

    /// 書庫の中の並び順をマンガミーヤと比べる（MM-SORT-01、観察リスト2）。
    /// マンガミーヤで確認した順番を spec/sort-check/mangameeya-order.txt に書くと比べる。
    /// まだ無いときは MangaReeda の順番を表示するだけ
    /// （cargo test sort_check -- --nocapture で見られる）
    #[test]
    fn sort_check_order_matches_mangameeya() {
        let mut names = sort_check_names();
        natural_sort_strings(&mut names);
        let expected = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("spec/sort-check/mangameeya-order.txt"),
        );
        let Ok(expected) = expected else {
            println!("MangaReeda の順番:");
            for (index, name) in names.iter().enumerate() {
                println!("{:>3} {name}", index + 1);
            }
            return;
        };
        let expected: Vec<&str> = expected
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("# "))
            .collect();
        assert_eq!(names, expected);
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "manga-library-test-{}-{}",
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

    fn names(paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn natural_sort_orders_numbers_by_value() {
        let mut names = vec![
            "page10.png".to_string(),
            "page2.png".to_string(),
            "Page1.png".to_string(),
            "page11.png".to_string(),
            "dir/page3.png".to_string(),
            "第10話.jpg".to_string(),
            "第9話.jpg".to_string(),
        ];
        natural_sort_strings(&mut names);
        assert_eq!(
            names,
            [
                "Page1.png",
                "page2.png",
                "dir/page3.png",
                "page10.png",
                "page11.png",
                "第9話.jpg",
                "第10話.jpg",
            ]
        );
    }

    #[test]
    fn scan_folder_finds_archives_and_visible_folders() {
        let dir = TempDir::new();
        for name in [
            "vol10.zip",
            "vol2.cbz",
            "extra.pdf",
            "readme.txt",
            "vol1.rar",
        ] {
            fs::write(dir.0.join(name), "").unwrap();
        }
        for name in ["chapter3", ".hidden"] {
            fs::create_dir(dir.0.join(name)).unwrap();
        }
        assert_eq!(
            names(&scan_folder(&dir.0)),
            ["chapter3", "extra.pdf", "vol1.rar", "vol2.cbz", "vol10.zip"]
        );
    }

    #[test]
    fn adjacent_directories_are_the_neighbouring_folders() {
        let dir = TempDir::new();
        for name in ["v1", "v2", "v10", ".hidden"] {
            fs::create_dir(dir.0.join(name)).unwrap();
        }
        let in_v2 = Some(dir.0.join("v2").join("book.zip"));
        assert_eq!(
            get_adjacent_directories(in_v2),
            (Some(dir.0.join("v1")), Some(dir.0.join("v10")))
        );
        let in_v1 = Some(dir.0.join("v1").join("book.zip"));
        assert_eq!(
            get_adjacent_directories(in_v1),
            (None, Some(dir.0.join("v2")))
        );
        assert_eq!(get_adjacent_directories(None), (None, None));
    }
}
