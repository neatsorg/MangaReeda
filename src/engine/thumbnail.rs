//! サムネイル一覧の、画面に依存しない部分。
//! マスの並べ方、方向キーでの選択の動き、縮小画像の作成と、別スレッドでの読み込み。

use crate::config::SourceMode;
use crate::engine::imaging::strip_adobe_app14_if_invalid;
use crate::engine::library::{natural_sort_strings, scan_folder};
use crate::engine::source::{SourceReader, list_images};
use image::{ImageFormat, RgbaImage};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// サムネイル画像の長辺（ピクセル）。大きいマスでも粗く見えない程度
pub const THUMBNAIL_MAX_SIDE: u32 = 320;
/// 大きいマスの一覧で、画面の高さに入るおよその行数（ミーヤの「8列」の見た目に合わせた）
const LARGE_ROWS_PER_SCREEN: f32 = 3.0;
/// 表紙サムネイルのマスの一辺（ミーヤの見た目に合わせた）。下に冊の名前を出す
pub const COVER_CELL: f32 = 180.0;
/// 冊の入ったフォルダの表紙を、中の冊からたどって探す深さ
const COVER_SEARCH_DEPTH: usize = 3;

/// 一覧の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailMode {
    /// 全ページを1画面に収める（ミーヤの「1画面」）
    FitAll,
    /// 大きいマスで並べ、スクロールする（ミーヤの「8列」）
    Large,
    /// フォルダの中の冊を表紙で並べる。決まった大きさのマスを中央に置く
    Covers,
}

/// 正方形のマスの並べ方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub columns: usize,
    pub rows: usize,
    /// マスの一辺
    pub cell: f32,
}

impl Grid {
    /// `count` 枚を `area`（幅, 高さ）に並べる
    pub fn new(mode: ThumbnailMode, count: usize, area: [f32; 2]) -> Self {
        let count = count.max(1);
        let [width, height] = [area[0].max(1.0), area[1].max(1.0)];
        match mode {
            ThumbnailMode::FitAll => {
                // マスがいちばん大きくなる列数を選ぶ（同じ大きさなら列の少ないほう）
                let mut best = Grid {
                    columns: 1,
                    rows: count,
                    cell: (width).min(height / count as f32),
                };
                for columns in 2..=count {
                    let rows = count.div_ceil(columns);
                    let cell = (width / columns as f32).min(height / rows as f32);
                    if cell > best.cell {
                        best = Grid {
                            columns,
                            rows,
                            cell,
                        };
                    }
                }
                best
            }
            ThumbnailMode::Covers => {
                let columns = ((width / COVER_CELL).floor() as usize).clamp(1, count);
                Grid {
                    columns,
                    rows: count.div_ceil(columns),
                    cell: COVER_CELL,
                }
            }
            ThumbnailMode::Large => {
                // 高さの 1/3 ほどのマスを、横幅いっぱいに並べる
                let columns = ((width / (height / LARGE_ROWS_PER_SCREEN)).round() as usize).max(1);
                Grid {
                    columns,
                    rows: count.div_ceil(columns),
                    cell: width / columns as f32,
                }
            }
        }
    }

    /// 並べたときの全体の大きさ（幅, 高さ）
    pub fn size(&self) -> [f32; 2] {
        [
            self.columns as f32 * self.cell,
            self.rows as f32 * self.cell,
        ]
    }

    /// `index` 枚目のマスの左上（一覧の左上が原点）
    pub fn cell_origin(&self, index: usize) -> [f32; 2] {
        [
            (index % self.columns) as f32 * self.cell,
            (index / self.columns) as f32 * self.cell,
        ]
    }

    /// 一覧の左上を原点とした位置にあるマス。マスが無ければ None
    pub fn index_at(&self, point: [f32; 2], count: usize) -> Option<usize> {
        let [x, y] = point;
        if x < 0.0 || y < 0.0 {
            return None;
        }
        let column = (x / self.cell) as usize;
        let row = (y / self.cell) as usize;
        if column >= self.columns {
            return None;
        }
        let index = row * self.columns + column;
        (index < count).then_some(index)
    }
}

/// 方向キーで選択を動かす向き
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// 方向キーで動かしたあとの選択。端より先へは動かない
pub fn move_selection(
    selected: usize,
    direction: Direction,
    columns: usize,
    count: usize,
) -> usize {
    if count == 0 {
        return 0;
    }
    let last = count - 1;
    match direction {
        Direction::Left => selected.saturating_sub(1),
        Direction::Right => (selected + 1).min(last),
        Direction::Up => selected.checked_sub(columns).unwrap_or(selected),
        // 最後の行が欠けているときは、最後のページで止まる
        Direction::Down if selected + columns <= last => selected + columns,
        Direction::Down if selected / columns < last / columns => last,
        Direction::Down => selected,
    }
}

/// 画像のバイト列から、長辺が `max_side` 以下のサムネイルを作る
pub fn decode_thumbnail(bytes: &[u8], max_side: u32, fix_adobe: bool) -> Option<RgbaImage> {
    let fixed;
    let bytes = if fix_adobe {
        fixed = strip_adobe_app14_if_invalid(bytes);
        &fixed[..]
    } else {
        bytes
    };
    let image = match image::guess_format(bytes) {
        Ok(format) => image::load_from_memory_with_format(bytes, format).ok()?,
        // 判別できないものは TGA として読んでみる（ページの読み込みと同じ）
        Err(_) => image::load_from_memory_with_format(bytes, ImageFormat::Tga).ok()?,
    };
    // thumbnail は小さい画像を拡大もするので、収まっているものはそのまま使う
    if image.width() <= max_side && image.height() <= max_side {
        return Some(image.to_rgba8());
    }
    Some(image.thumbnail(max_side, max_side).to_rgba8())
}

/// 表紙サムネイルに並べる冊
#[derive(Clone, Debug, PartialEq)]
pub struct CoverEntry {
    pub path: PathBuf,
    pub is_folder: bool,
    /// Some(枚数) なら、フォルダ `path` の直下の画像だけを冊として読むマス
    pub loose_pages: Option<usize>,
}

/// フォルダの中の冊（書庫・PDF・フォルダ）。ミーヤと同じく、フォルダを先に、それぞれ名前順に並べる。
/// 冊でないファイルは選んでも開けないので入れない（ミーヤは出す）
pub fn cover_entries(folder: &Path) -> Vec<CoverEntry> {
    let mut books = scan_folder(folder);
    // 並べ替えは安定なので、名前順のままフォルダが先になる
    books.sort_by_key(|path| !path.is_dir());
    books
        .into_iter()
        .map(|path| CoverEntry {
            is_folder: path.is_dir(),
            path,
            loose_pages: None,
        })
        .collect()
}

/// 表紙サムネイルでフォルダを開いたときに並べるもの。中の冊に加えて、直下に画像もあれば
/// その画像だけを冊として読むマスを先頭に置く（冊の入ったフォルダは一覧として開くので、
/// 直下の画像はここから読む）
pub fn folder_shelf(folder: &Path) -> Vec<CoverEntry> {
    let mut entries = cover_entries(folder);
    let loose = list_images(folder, false).map_or(0, |listing| listing.images.len());
    if !entries.is_empty() && loose > 0 {
        entries.insert(
            0,
            CoverEntry {
                path: folder.to_path_buf(),
                is_folder: true,
                loose_pages: Some(loose),
            },
        );
    }
    entries
}

/// 冊の表紙のバイト列。表紙は並び順で最初のページ。画像の無いフォルダは、中の最初の冊の表紙。
/// PDF は描けないので None
fn cover_bytes(path: &Path, depth: usize) -> Option<Vec<u8>> {
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        return None;
    }
    if let Some(listing) = list_images(path, false)
        && !listing.images.is_empty()
    {
        let mut images = listing.images;
        natural_sort_strings(&mut images);
        return SourceReader::open(listing.mode, &listing.root).read(&images[0]);
    }
    if !path.is_dir() || depth == 0 {
        return None;
    }
    cover_entries(path)
        .iter()
        .find_map(|entry| cover_bytes(&entry.path, depth - 1))
}

/// 冊の表紙のサムネイル
pub fn cover_thumbnail(path: &Path, fix_adobe: bool) -> Option<RgbaImage> {
    let bytes = cover_bytes(path, COVER_SEARCH_DEPTH)?;
    decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, fix_adobe)
}

/// `indices` の順に、`make_worker` で作った処理でサムネイルを作って送る。処理はスレッドごとに作る
/// （書庫を開いたまま続けて読めるように）。`cancel` を立てるか受け手を捨てると止まる
fn spawn_workers<F, W>(
    indices: Vec<usize>,
    cancel: Arc<AtomicBool>,
    make_worker: F,
) -> Receiver<(usize, RgbaImage)>
where
    F: Fn() -> W + Send + Sync + 'static,
    W: FnMut(usize) -> Option<RgbaImage>,
{
    let (tx, rx) = channel();
    let indices = Arc::new(indices);
    let next = Arc::new(AtomicUsize::new(0));
    let make_worker = Arc::new(make_worker);
    let workers = thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4);
    for _ in 0..workers {
        let (tx, indices, next, cancel, make_worker) = (
            tx.clone(),
            indices.clone(),
            next.clone(),
            cancel.clone(),
            make_worker.clone(),
        );
        thread::spawn(move || {
            let mut work = make_worker();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(&index) = indices.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    return;
                };
                let Some(image) = work(index) else {
                    continue;
                };
                if tx.send((index, image)).is_err() {
                    return;
                }
            }
        });
    }
    rx
}

/// 冊の表紙のサムネイルを、別スレッドで作って送る。(冊の番号, 画像) を送る
pub fn spawn_cover_loader(
    books: Vec<PathBuf>,
    fix_adobe: bool,
    cancel: Arc<AtomicBool>,
) -> Receiver<(usize, RgbaImage)> {
    let indices = (0..books.len()).collect();
    let books = Arc::new(books);
    spawn_workers(indices, cancel, move || {
        let books = books.clone();
        move |index| cover_thumbnail(books.get(index)?, fix_adobe)
    })
}

/// 書庫やフォルダのページのサムネイルを、別スレッドで作って送る。
/// `indices` の順に作り、(ページ番号, 画像) を送る。読めなかったページは送らない。
/// `cancel` を立てるか受け手を捨てると止まる。PDF は扱わない（描画を別スレッドに出せないため）
pub fn spawn_loader(
    mode: SourceMode,
    root: PathBuf,
    names: Vec<String>,
    indices: Vec<usize>,
    fix_adobe: bool,
    cancel: Arc<AtomicBool>,
) -> Receiver<(usize, RgbaImage)> {
    if mode == SourceMode::Pdf {
        return channel().1;
    }
    let names = Arc::new(names);
    spawn_workers(indices, cancel, move || {
        let names = names.clone();
        // 書庫は開いたまま続けて読む
        let mut reader = SourceReader::open(mode, &root);
        move |index| {
            let bytes = reader.read(names.get(index)?)?;
            decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, fix_adobe)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_all_matches_mangameeya_for_sixty_pages() {
        // 1900x960 の画面に 60 ページなら 10 列 × 6 行
        let grid = Grid::new(ThumbnailMode::FitAll, 60, [1900.0, 960.0]);
        assert_eq!((grid.columns, grid.rows), (10, 6));
        assert_eq!(grid.cell, 160.0);
        let [width, height] = grid.size();
        assert!(width <= 1900.0 && height <= 960.0);
    }

    #[test]
    fn large_cells_show_about_three_rows_and_fill_the_width() {
        // 同じ画面で「8列」は 1 マス約 316 の 6 列で、3 行と少しが見えた
        let grid = Grid::new(ThumbnailMode::Large, 60, [1897.0, 964.0]);
        assert_eq!((grid.columns, grid.rows), (6, 10));
        assert!((grid.cell - 1897.0 / 6.0).abs() < 0.01);
        // 縦長の画面では列が減る
        let grid = Grid::new(ThumbnailMode::Large, 60, [800.0, 1200.0]);
        assert_eq!(grid.columns, 2);
        assert_eq!(
            Grid::new(ThumbnailMode::Large, 60, [100.0, 2000.0]).columns,
            1
        );
    }

    #[test]
    fn cells_are_found_by_position() {
        let grid = Grid {
            columns: 4,
            rows: 3,
            cell: 10.0,
        };
        assert_eq!(grid.cell_origin(5), [10.0, 10.0]);
        assert_eq!(grid.index_at([15.0, 15.0], 10), Some(5));
        assert_eq!(grid.index_at([45.0, 5.0], 10), None);
        // 最後の行の欠けているところ
        assert_eq!(grid.index_at([25.0, 25.0], 10), None);
        assert_eq!(grid.index_at([-1.0, 5.0], 10), None);
    }

    #[test]
    fn selection_moves_within_the_pages() {
        use Direction::*;
        // 4 列、10 枚: 0..3 / 4..7 / 8 9
        assert_eq!(move_selection(0, Left, 4, 10), 0);
        assert_eq!(move_selection(3, Right, 4, 10), 4);
        assert_eq!(move_selection(9, Right, 4, 10), 9);
        assert_eq!(move_selection(2, Up, 4, 10), 2);
        assert_eq!(move_selection(6, Up, 4, 10), 2);
        assert_eq!(move_selection(5, Down, 4, 10), 9);
        assert_eq!(move_selection(6, Down, 4, 10), 9);
        assert_eq!(move_selection(9, Down, 4, 10), 9);
        assert_eq!(move_selection(0, Down, 4, 0), 0);
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn write_zip(path: &Path, entries: &[(&str, Vec<u8>)]) {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, data) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn cover_entries_put_folders_first_and_covers_are_first_pages() {
        let root = std::env::temp_dir().join(format!("manga-cover-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("03 C")).unwrap();
        std::fs::create_dir_all(root.join("04 shelf")).unwrap();
        // 書庫の中では後に入れたものが並び順で先（表紙）になる
        write_zip(
            &root.join("01 A.zip"),
            &[("02.png", png(30, 10)), ("01.png", png(10, 20))],
        );
        write_zip(&root.join("02 B.zip"), &[("01.png", png(10, 30))]);
        std::fs::write(root.join("03 C/01.png"), png(40, 10)).unwrap();
        write_zip(&root.join("04 shelf/D.zip"), &[("01.png", png(10, 50))]);
        std::fs::write(root.join("memo.txt"), "memo").unwrap();

        // フォルダが先、それぞれ名前順。冊でないファイルは入れない
        let names: Vec<_> = cover_entries(&root)
            .iter()
            .map(|entry| {
                (
                    entry
                        .path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                    entry.is_folder,
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("03 C".to_string(), true),
                ("04 shelf".to_string(), true),
                ("01 A.zip".to_string(), false),
                ("02 B.zip".to_string(), false),
            ]
        );
        let size = |path: &str| cover_thumbnail(&root.join(path), false).map(|t| t.dimensions());
        assert_eq!(size("01 A.zip"), Some((10, 20)));
        assert_eq!(size("03 C"), Some((40, 10)));
        // 画像の無いフォルダは、中の最初の冊の表紙
        assert_eq!(size("04 shelf"), Some((10, 50)));
        assert_eq!(size("memo.txt"), None);

        // 直下に画像が無ければ、画像を読むマスは置かない。あれば先頭に置く
        assert_eq!(folder_shelf(&root), cover_entries(&root));
        std::fs::write(root.join("cover.png"), png(10, 10)).unwrap();
        let shelf = folder_shelf(&root);
        assert_eq!(shelf.len(), 5);
        assert_eq!(shelf[0].path, root);
        assert_eq!(shelf[0].loose_pages, Some(1));
        assert_eq!(&shelf[1..], cover_entries(&root));
        // 画像だけのフォルダには置かない
        assert_eq!(folder_shelf(&root.join("03 C")), []);
        std::fs::remove_file(root.join("cover.png")).unwrap();

        // 別スレッドでも同じものが届く
        let books: Vec<_> = cover_entries(&root).into_iter().map(|e| e.path).collect();
        let rx = spawn_cover_loader(books, false, Arc::new(AtomicBool::new(false)));
        let mut received: Vec<_> = rx
            .iter()
            .map(|(index, image)| (index, image.dimensions()))
            .collect();
        received.sort();
        assert_eq!(
            received,
            [(0, (40, 10)), (1, (10, 50)), (2, (10, 20)), (3, (10, 30))]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn thumbnails_keep_the_aspect_ratio() {
        let mut bytes = Vec::new();
        image::RgbaImage::new(800, 1200)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let thumb = decode_thumbnail(&bytes, 300, false).unwrap();
        assert_eq!(thumb.dimensions(), (200, 300));
        // 小さい画像は拡大しない
        let mut small = Vec::new();
        image::RgbaImage::new(10, 20)
            .write_to(&mut std::io::Cursor::new(&mut small), ImageFormat::Png)
            .unwrap();
        assert_eq!(
            decode_thumbnail(&small, 300, false).unwrap().dimensions(),
            (10, 20)
        );
        assert!(decode_thumbnail(b"not an image", 300, false).is_none());
    }
}
