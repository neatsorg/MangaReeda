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

    /// 読めなかったときは空の履歴にする（壊れたファイルで起動できなくならないように）
    pub fn load(path: &Path) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(path, json);
        }
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

        let mut history = History::default();
        history.record(Path::new("/book"), &pages(&["a.png"]), 0);
        history.save(&path);
        assert_eq!(History::load(&path), history);
        fs::remove_file(&path).unwrap();
    }
}
