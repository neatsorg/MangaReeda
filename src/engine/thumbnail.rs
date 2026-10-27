//! サムネイル一覧（MM-THUMB-01, 02）の、画面に依存しない部分。
//! マスの並べ方、方向キーでの選択の動き、縮小画像の作成と、別スレッドでの読み込み。

use crate::config::SourceMode;
use crate::engine::imaging::strip_adobe_app14_if_invalid;
use crate::engine::source::SourceReader;
use image::{ImageFormat, RgbaImage};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// サムネイル画像の長辺（ピクセル）。大きいマスでも粗く見えない程度
pub const THUMBNAIL_MAX_SIDE: u32 = 320;
/// 大きいマスの一覧で、画面の高さに入るおよその行数（ミーヤの「8列」の見た目に合わせた）
const LARGE_ROWS_PER_SCREEN: f32 = 3.0;

/// 一覧の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbnailMode {
    /// 全ページを1画面に収める（ミーヤの「1画面」）
    FitAll,
    /// 大きいマスで並べ、スクロールする（ミーヤの「8列」）
    Large,
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
    Some(image.thumbnail(max_side, max_side).to_rgba8())
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
    let (tx, rx) = channel();
    if mode == SourceMode::Pdf {
        return rx;
    }
    let names = Arc::new(names);
    let indices = Arc::new(indices);
    let next = Arc::new(AtomicUsize::new(0));
    let workers = thread::available_parallelism()
        .map_or(2, |n| n.get())
        .clamp(1, 4);
    for _ in 0..workers {
        let (tx, root, names, indices, next, cancel) = (
            tx.clone(),
            root.clone(),
            names.clone(),
            indices.clone(),
            next.clone(),
            cancel.clone(),
        );
        thread::spawn(move || {
            // 書庫は開いたまま続けて読む
            let mut reader = SourceReader::open(mode, &root);
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let Some(&index) = indices.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    return;
                };
                let Some(image) = names
                    .get(index)
                    .and_then(|name| reader.read(name))
                    .and_then(|bytes| decode_thumbnail(&bytes, THUMBNAIL_MAX_SIDE, fix_adobe))
                else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_all_matches_mangameeya_for_sixty_pages() {
        // 実機 で 1920x1000 前後の画面に 60 ページが 10 列 × 6 行で並んだ（2026-10-27）
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

    #[test]
    fn thumbnails_keep_the_aspect_ratio() {
        let mut bytes = Vec::new();
        image::RgbaImage::new(800, 1200)
            .write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        let thumb = decode_thumbnail(&bytes, 300, false).unwrap();
        assert_eq!(thumb.dimensions(), (200, 300));
        assert!(decode_thumbnail(b"not an image", 300, false).is_none());
    }
}
