//! 読んだ冊の履歴としおり、お気に入り。
//! 冊ごとに最後に表示していたページを覚え、次に開いたときにそこから読めるようにする。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 履歴に残す冊の数（ミーヤの履歴の既定と同じ）
pub const HISTORY_LIMIT: usize = 100;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryEntry {
    /// 冊（書庫・フォルダ・PDF）の絶対パス
    pub path: PathBuf,
    /// しおりのページのファイル名。ページが増減しても同じ画像を探せるようにする
    pub page_name: String,
    /// しおりのページ番号（0始まり）。ファイル名で見つからないときに使う
    pub page_index: usize,
    pub page_count: usize,
    /// 最終アクセス日時（UNIX 秒）
    pub last_access: u64,
}

/// お気に入りに登録した冊
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FavoriteEntry {
    /// 冊（書庫・フォルダ・PDF）の絶対パス
    pub path: PathBuf,
    /// 登録した日時（UNIX 秒）
    pub added: u64,
}

/// サブフォルダを含むフォルダで、利用者が選んだ読み方
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SubfolderChoice {
    /// フォルダの絶対パス
    pub path: PathBuf,
    /// サブフォルダの画像も読むか
    pub include: bool,
}

/// 利用者が選んだ表紙
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CoverChoice {
    /// 冊（書庫・フォルダ・PDF）の絶対パス
    pub path: PathBuf,
    /// 表紙にするページ（`source::page_key` の形）
    pub page: String,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
pub struct History {
    /// 新しいものが先頭
    pub entries: Vec<HistoryEntry>,
    /// 新しいものが先頭。冊の履歴と同じ数まで覚える
    #[serde(default)]
    pub subfolder_choices: Vec<SubfolderChoice>,
    /// 新しく登録したものが先頭。数の上限は無い
    #[serde(default)]
    pub favorites: Vec<FavoriteEntry>,
    /// 冊ごとに選んだ表紙。数の上限は無い
    #[serde(default)]
    pub covers: Vec<CoverChoice>,
}

/// 同じ冊を別の書き方のパスで開いても同じ記録になるよう、正規化したパスを使う
pub fn normalize(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl History {
    /// 冊を読んでいた位置を記録し、その冊を履歴の先頭に移す。上限を超えたら古いものから消す
    pub fn record(&mut self, path: &Path, pages: &[String], page_index: usize) {
        let Some(page_name) = pages.get(page_index) else {
            return;
        };
        let path = normalize(path);
        self.entries.retain(|entry| entry.path != path);
        self.entries.insert(
            0,
            HistoryEntry {
                path,
                page_name: page_name.clone(),
                page_index,
                page_count: pages.len(),
                last_access: now(),
            },
        );
        self.entries.truncate(HISTORY_LIMIT);
    }

    /// しおりのページ番号を返す。同じ名前のページがあればそれを、無ければ記録した番号を範囲内に収めて使う
    pub fn bookmark(&self, path: &Path, pages: &[String]) -> Option<usize> {
        if pages.is_empty() {
            return None;
        }
        let path = normalize(path);
        let entry = self.entries.iter().find(|entry| entry.path == path)?;
        Some(
            pages
                .iter()
                .position(|name| *name == entry.page_name)
                .unwrap_or(entry.page_index.min(pages.len() - 1)),
        )
    }

    /// 冊を履歴から消す
    pub fn forget(&mut self, path: &Path) {
        let path = normalize(path);
        self.entries.retain(|entry| entry.path != path);
    }

    pub fn is_favorite(&self, path: &Path) -> bool {
        let path = normalize(path);
        self.favorites.iter().any(|entry| entry.path == path)
    }

    /// お気に入りに登録する。登録済みなら外す。登録したら true
    pub fn toggle_favorite(&mut self, path: &Path) -> bool {
        if self.is_favorite(path) {
            self.remove_favorite(path);
            return false;
        }
        self.favorites.insert(
            0,
            FavoriteEntry {
                path: normalize(path),
                added: now(),
            },
        );
        true
    }

    pub fn remove_favorite(&mut self, path: &Path) {
        let path = normalize(path);
        self.favorites.retain(|entry| entry.path != path);
    }

    /// 冊の表紙にするページを覚える
    pub fn choose_cover(&mut self, path: &Path, page: String) {
        let path = normalize(path);
        self.covers.retain(|choice| choice.path != path);
        self.covers.push(CoverChoice { path, page });
    }

    /// 選んだ表紙（冊の正規化したパス → ページ）。表紙を作るスレッドに渡す
    pub fn cover_choices(&self) -> std::collections::HashMap<PathBuf, String> {
        self.covers
            .iter()
            .map(|choice| (choice.path.clone(), choice.page.clone()))
            .collect()
    }

    /// フォルダで覚えたサブフォルダの読み方
    pub fn subfolder_choice(&self, folder: &Path) -> Option<bool> {
        let folder = normalize(folder);
        self.subfolder_choices
            .iter()
            .find(|choice| choice.path == folder)
            .map(|choice| choice.include)
    }

    pub fn remember_subfolder_choice(&mut self, folder: &Path, include: bool) {
        let path = normalize(folder);
        self.subfolder_choices.retain(|choice| choice.path != path);
        self.subfolder_choices
            .insert(0, SubfolderChoice { path, include });
        self.subfolder_choices.truncate(HISTORY_LIMIT);
    }

    /// 読めなかったときは空の履歴にする（壊れたファイルで起動できなくならないように）。
    /// 壊れたファイルは、次の保存で上書きしないよう別の名前（.broken）に移しておく
    pub fn load(path: &Path) -> Self {
        let Ok(data) = fs::read_to_string(path) else {
            return Self::default();
        };
        match serde_json::from_str(&data) {
            Ok(history) => history,
            Err(e) => {
                let broken = path.with_extension("json.broken");
                eprintln!(
                    "履歴を読めませんでした（{e}）。元のファイルは {} に残します",
                    broken.display()
                );
                let _ = fs::rename(path, broken);
                Self::default()
            }
        }
    }

    /// 途中で止まっても前の履歴が壊れないように保存する
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        crate::engine::settings_store::write_atomic(path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn bookmark_follows_the_page_name_when_pages_change() {
        let mut history = History::default();
        let book = Path::new("/no/such/book.zip");
        history.record(book, &pages(&["a.png", "b.png", "c.png"]), 1);
        assert_eq!(
            history.bookmark(book, &pages(&["a.png", "b.png", "c.png"])),
            Some(1)
        );
        // 前にページが足されても同じ画像から読む
        assert_eq!(
            history.bookmark(book, &pages(&["0.png", "a.png", "b.png", "c.png"])),
            Some(2)
        );
        // 同じ名前が無ければ番号を範囲内に収める
        assert_eq!(history.bookmark(book, &pages(&["x.png"])), Some(0));
        assert_eq!(history.bookmark(book, &[]), None);
        assert_eq!(
            history.bookmark(Path::new("/other.zip"), &pages(&["a.png"])),
            None
        );
    }

    #[test]
    fn recording_moves_the_book_to_the_front_and_keeps_the_limit() {
        let mut history = History::default();
        let names = pages(&["a.png", "b.png"]);
        for index in 0..HISTORY_LIMIT + 5 {
            history.record(Path::new(&format!("/book{index}")), &names, 0);
        }
        assert_eq!(history.entries.len(), HISTORY_LIMIT);
        assert_eq!(history.entries[0].path, Path::new("/book104"));

        history.record(Path::new("/book50"), &names, 1);
        assert_eq!(history.entries.len(), HISTORY_LIMIT);
        assert_eq!(history.entries[0].path, Path::new("/book50"));
        assert_eq!(history.entries[0].page_name, "b.png");
        assert_eq!(
            history
                .entries
                .iter()
                .filter(|e| e.path == Path::new("/book50"))
                .count(),
            1
        );
    }

    #[test]
    fn subfolder_choices_are_remembered_per_folder() {
        let mut history = History::default();
        let folder = Path::new("/no/such/folder");
        assert_eq!(history.subfolder_choice(folder), None);
        history.remember_subfolder_choice(folder, true);
        history.remember_subfolder_choice(Path::new("/other"), false);
        assert_eq!(history.subfolder_choice(folder), Some(true));
        assert_eq!(history.subfolder_choice(Path::new("/other")), Some(false));
        // 選び直したら上書きする
        history.remember_subfolder_choice(folder, false);
        assert_eq!(history.subfolder_choice(folder), Some(false));
        assert_eq!(history.subfolder_choices.len(), 2);

        for index in 0..HISTORY_LIMIT + 5 {
            history.remember_subfolder_choice(Path::new(&format!("/f{index}")), true);
        }
        assert_eq!(history.subfolder_choices.len(), HISTORY_LIMIT);

        // この項目が無い古い履歴ファイルも読める
        let old: History = serde_json::from_str(r#"{"entries": []}"#).unwrap();
        assert!(old.subfolder_choices.is_empty());
    }

    #[test]
    fn chosen_covers_are_kept_per_book_and_replaced() {
        let mut history = History::default();
        let (a, b) = (Path::new("/books/a.zip"), Path::new("/books/b"));
        history.choose_cover(a, "02.png".to_owned());
        history.choose_cover(b, "sub/03.png".to_owned());
        history.choose_cover(a, "05.png".to_owned());
        let choices = history.cover_choices();
        assert_eq!(choices.len(), 2);
        assert_eq!(choices[&normalize(a)], "05.png");
        // 古い履歴のファイル（covers が無い）も読める
        let old: History = serde_json::from_str(r#"{"entries": []}"#).unwrap();
        assert!(old.covers.is_empty());
    }

    #[test]
    fn favorites_toggle_and_put_the_newest_first() {
        let mut history = History::default();
        let (a, b) = (Path::new("/no/such/a.zip"), Path::new("/no/such/b.zip"));
        assert!(history.toggle_favorite(a));
        assert!(history.toggle_favorite(b));
        assert!(history.is_favorite(a));
        assert_eq!(history.favorites[0].path, b);
        // もう一度登録すると外れる
        assert!(!history.toggle_favorite(a));
        assert!(!history.is_favorite(a));
        history.remove_favorite(b);
        assert!(history.favorites.is_empty());

        // 履歴から消してもお気に入りは残る
        history.record(a, &pages(&["a.png"]), 0);
        history.toggle_favorite(a);
        history.forget(a);
        assert!(history.entries.is_empty());
        assert!(history.is_favorite(a));

        // この項目が無い古い履歴ファイルも読める
        let old: History = serde_json::from_str(r#"{"entries": []}"#).unwrap();
        assert!(old.favorites.is_empty());
    }

    #[test]
    fn broken_history_file_loads_as_empty() {
        let path = std::env::temp_dir().join(format!("manga-history-test-{}", std::process::id()));
        fs::write(&path, "not json").unwrap();
        assert_eq!(History::load(&path), History::default());
        // 壊れたファイルは残しておく
        let broken = path.with_extension("json.broken");
        assert_eq!(fs::read_to_string(&broken).unwrap(), "not json");
        assert!(!path.exists());

        let mut history = History::default();
        history.record(Path::new("/book"), &pages(&["a.png"]), 0);
        history.save(&path).unwrap();
        assert_eq!(History::load(&path), history);
        // 一時ファイルは残らない
        let leftovers = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("manga-history-test")
            })
            .count();
        assert_eq!(leftovers, 0);
        fs::remove_file(&path).unwrap();
        fs::remove_file(&broken).unwrap();
    }
}
