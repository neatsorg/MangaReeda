//! 大きさの合計に上限のある、ためておき。上限を超えたら、いちばん長く使っていないものから捨てる
//! （冊を読み進めるほどテクスチャや画像がたまり、メモリを使い切らないように）。

use std::collections::HashMap;

/// ためておくものの大きさ（バイト）
pub(super) trait CacheSize {
    fn cache_bytes(&self) -> usize;
}

impl CacheSize for eframe::egui::TextureHandle {
    fn cache_bytes(&self) -> usize {
        let [width, height] = self.size();
        width * height * 4
    }
}

impl CacheSize for Vec<u8> {
    fn cache_bytes(&self) -> usize {
        self.len()
    }
}

pub(super) struct SizedCache<V> {
    /// 名前 → (値, 大きさ, 最後に使った順番)
    entries: HashMap<String, (V, usize, u64)>,
    total: usize,
    limit: usize,
    tick: u64,
}

impl<V: CacheSize> SizedCache<V> {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            total: 0,
            limit,
            tick: 0,
        }
    }

    /// 取り出す。使ったものとして覚える
    pub(super) fn get(&mut self, name: &str) -> Option<&V> {
        self.tick += 1;
        let tick = self.tick;
        let (value, _, used) = self.entries.get_mut(name)?;
        *used = tick;
        Some(value)
    }

    /// 入れる。合計が上限を超えたら、いま入れたもの以外を、長く使っていない順に捨てる
    pub(super) fn insert(&mut self, name: String, value: V) {
        self.tick += 1;
        let bytes = value.cache_bytes();
        if let Some((_, old, _)) = self.entries.insert(name.clone(), (value, bytes, self.tick)) {
            self.total -= old;
        }
        self.total += bytes;
        while self.total > self.limit {
            let oldest = self
                .entries
                .iter()
                .filter(|(key, _)| **key != name)
                .min_by_key(|(_, (_, _, used))| *used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else {
                break;
            };
            if let Some((_, bytes, _)) = self.entries.remove(&oldest) {
                self.total -= bytes;
            }
        }
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.total = 0;
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_used_go_first_when_over_the_limit() {
        let mut cache = SizedCache::new(30);
        cache.insert("a".to_owned(), vec![0u8; 10]);
        cache.insert("b".to_owned(), vec![0u8; 10]);
        cache.insert("c".to_owned(), vec![0u8; 10]);
        // a を使ったので、次に捨てるのは b
        assert!(cache.get("a").is_some());
        cache.insert("d".to_owned(), vec![0u8; 10]);
        assert!(cache.get("b").is_none());
        assert_eq!(cache.len(), 3);
        // 上限より大きいものは、それだけ残る（入れた直後に使うので捨てない）
        cache.insert("big".to_owned(), vec![0u8; 50]);
        assert_eq!(cache.len(), 1);
        assert!(cache.get("big").is_some());
        // 同じ名前で入れ直すと、前の大きさは数えない
        cache.clear();
        cache.insert("a".to_owned(), vec![0u8; 20]);
        cache.insert("a".to_owned(), vec![0u8; 20]);
        cache.insert("b".to_owned(), vec![0u8; 10]);
        assert_eq!(cache.len(), 2);
    }
}
