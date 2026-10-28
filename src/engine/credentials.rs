//! プラグインのログイン情報。OS のキーリング（Linux は Secret Service、Windows は資格情報マネージャー、
//! macOS はキーチェーン）に保存する。キーリングが使えないときは、ファイルには書かず、
//! 起動しているあいだだけメモリに持つ。

use std::collections::HashMap;

/// キーリングでの名前（サービス名）
const SERVICE: &str = "MangaReeda";

/// 保存した場所
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stored {
    Keyring,
    /// キーリングが使えず、起動しているあいだだけ持っている
    Memory,
}

pub struct Credentials {
    memory: HashMap<String, String>,
    /// false ならキーリングを使わない（テスト用）
    use_keyring: bool,
}

impl Default for Credentials {
    fn default() -> Self {
        Self {
            memory: HashMap::new(),
            use_keyring: true,
        }
    }
}

fn account(plugin: &str, key: &str) -> String {
    format!("{plugin}:{key}")
}

impl Credentials {
    /// キーリングを使わず、メモリにだけ持つ
    pub fn memory_only() -> Self {
        Self {
            memory: HashMap::new(),
            use_keyring: false,
        }
    }

    fn entry(&self, plugin: &str, key: &str) -> Option<keyring::Entry> {
        if !self.use_keyring {
            return None;
        }
        keyring::Entry::new(SERVICE, &account(plugin, key)).ok()
    }

    pub fn get(&self, plugin: &str, key: &str) -> Option<String> {
        if let Some(value) = self.memory.get(&account(plugin, key)) {
            return Some(value.clone());
        }
        self.entry(plugin, key)?.get_password().ok()
    }

    /// どこに保存されているか。無ければ None
    pub fn stored(&self, plugin: &str, key: &str) -> Option<Stored> {
        if self.memory.contains_key(&account(plugin, key)) {
            return Some(Stored::Memory);
        }
        self.entry(plugin, key)?
            .get_password()
            .ok()
            .map(|_| Stored::Keyring)
    }

    /// キーリングに保存する。使えなければメモリに持つ
    pub fn set(&mut self, plugin: &str, key: &str, value: &str) -> Stored {
        let saved = self
            .entry(plugin, key)
            .is_some_and(|entry| entry.set_password(value).is_ok());
        if saved {
            self.memory.remove(&account(plugin, key));
            Stored::Keyring
        } else {
            self.memory.insert(account(plugin, key), value.to_owned());
            Stored::Memory
        }
    }

    pub fn delete(&mut self, plugin: &str, key: &str) {
        self.memory.remove(&account(plugin, key));
        if let Some(entry) = self.entry(plugin, key) {
            let _ = entry.delete_credential();
        }
    }

    /// プラグインに渡す、キーと値の組
    pub fn for_plugin(&self, plugin: &str, keys: &[String]) -> HashMap<String, String> {
        keys.iter()
            .filter_map(|key| Some((key.clone(), self.get(plugin, key)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_kept_per_plugin_and_key() {
        // 本物のキーリングには触れない
        let mut credentials = Credentials::memory_only();
        assert_eq!(credentials.get("test", "email"), None);
        assert_eq!(
            credentials.set("test", "email", "a@example.invalid"),
            Stored::Memory
        );
        assert_eq!(credentials.stored("test", "email"), Some(Stored::Memory));
        assert_eq!(
            credentials.get("test", "email").as_deref(),
            Some("a@example.invalid")
        );
        assert_eq!(credentials.get("other", "email"), None);
        let keys = ["email".to_owned(), "password".to_owned()];
        assert_eq!(credentials.for_plugin("test", &keys).len(), 1);
        credentials.delete("test", "email");
        assert_eq!(credentials.get("test", "email"), None);
    }
}
