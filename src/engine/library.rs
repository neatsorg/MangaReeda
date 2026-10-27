use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

/// マンガミーヤの「パス[数字順]」に合わせた比較（MM-SORT-01）。
/// spec/sort-check/mangameeya-order.txt の実物の並びから読み取った規則:
///
/// - フォルダの区切りも含めたパス全体で比べる（フォルダの中身がほかと混ざらない）
/// - 文字の種類の順: 区切り（/ \）→ `-` → ほかの記号（ASCII 順）→ 全角・半角カナ
///   → 数字 → 英字（大文字小文字は区別しない）→ ひらがな・カタカナ・漢字など（文字コード順）
/// - 続いた数字は値で比べる（桁数に上限なし）。値が同じなら先頭のゼロが多いほうが先
///   （`00` → `0`、`001` → `01` → `1`）
///
/// 全角文字と半角カナどうしの順は単純な規則で説明できなかったので、
/// 文字コード順にしてある（観察リスト2）。
pub fn mangameeya_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    // 値が同じ数字の桁数の違い。ほかに差が無ければこれで決める
    let mut zeros_tie = Ordering::Equal;
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return zeros_tie.then_with(|| a.cmp(b)),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let n1 = take_digits(&mut x);
                let n2 = take_digits(&mut y);
                let (t1, t2) = (n1.trim_start_matches('0'), n2.trim_start_matches('0'));
                let by_value = t1.len().cmp(&t2.len()).then_with(|| t1.cmp(t2));
                if by_value != Ordering::Equal {
                    return by_value;
                }
                if zeros_tie == Ordering::Equal {
                    // 桁数の多い（ゼロの多い）ほうが先
                    zeros_tie = n2.len().cmp(&n1.len());
                }
            }
            (Some(c), Some(d)) => {
                let order = char_key(c).cmp(&char_key(d));
                if order != Ordering::Equal {
                    return order;
                }
                x.next();
                y.next();
            }
        }
    }
}

fn take_digits(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut digits = String::new();
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        digits.push(c);
        chars.next();
    }
    digits
}

/// 数字以外の1文字の並びの重み（種類, 種類の中での順）
fn char_key(c: char) -> (u8, u32) {
    match c {
        '/' | '\\' => (0, 0),
        '-' => (1, 0),
        // 片方だけが数字のとき（数字どうしは mangameeya_cmp が値で比べる）
        c if c.is_ascii_digit() => (4, 0),
        c if c.is_ascii_alphabetic() => (5, c.to_ascii_lowercase() as u32),
        c if c.is_ascii() => (2, c as u32),
        // 全角英数・記号と半角カナ
        '\u{FF00}'..='\u{FFEF}' => (3, c as u32),
        c => (6, c as u32),
    }
}

/// 本（書庫・フォルダ）の並び替え。同じフォルダの中なのでファイル名で比べる
pub fn natural_sort(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| {
        mangameeya_cmp(
            &a.file_name().unwrap_or_default().to_string_lossy(),
            &b.file_name().unwrap_or_default().to_string_lossy(),
        )
    });
}

/// ページの並び替え。書庫の中のフォルダも含めたパス全体で比べる
pub fn natural_sort_strings(strings: &mut [String]) {
    strings.sort_by(|a, b| mangameeya_cmp(a, b));
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
        // 全角文字と半角カナを含む名前は、どこに来るか（記号と数字のあいだ）だけを比べる。
        // マンガミーヤでのそれらどうしの順（２ → ｱ → ｐａｇｅ5 → １０）は規則が分からないため
        fn wide(name: &str) -> &str {
            if name.chars().any(|c| ('\u{FF00}'..='\u{FFEF}').contains(&c)) {
                "（全角・半角カナ）"
            } else {
                name
            }
        }
        let expected: Vec<&str> = expected
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with("# "))
            .map(wide)
            .collect();
        let names: Vec<&str> = names.iter().map(|name| wide(name)).collect();
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
        // フォルダも含めたパス全体で比べるので、dir/ の中身は d の位置に来る（マンガミーヤと同じ）
        assert_eq!(
            names,
            [
                "dir/page3.png",
                "Page1.png",
                "page2.png",
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
