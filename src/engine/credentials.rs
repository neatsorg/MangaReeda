//! プラグインのログイン情報。OS のキーリング（Linux は Secret Service、Windows は資格情報マネージャー、
//! macOS はキーチェーン）に保存する。キーリングが使えないときは、ファイルには書かず、
//! 起動しているあいだだけメモリに持つ。

use std::collections::{HashMap, HashSet};

/// キーリングでの名前（サービス名）
const SERVICE: &str = "MangaReeda";

/// 保存した場所
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stored {
    Keyring,
    /// キーリングが使えず、起動しているあいだだけ持っている
    Memory,
    /// 消そうとしたがキーリングから消せなかった（起動しているあいだは使わない）
    DeleteFailed,
}

/// 値を保存する場所（OS のキーリング）。テストでは差し替える
pub trait SecretStore {
    fn get(&self, account: &str) -> Option<String>;
    /// 保存できたら true
    fn set(&self, account: &str, value: &str) -> bool;
    /// 消す。もともと無ければ成功とする
    fn delete(&self, account: &str) -> Result<(), String>;
}

/// OS のキーリング
struct OsKeyring;

impl OsKeyring {
    fn entry(account: &str) -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, account).ok()
    }
}

impl SecretStore for OsKeyring {
    fn get(&self, account: &str) -> Option<String> {
        Self::entry(account)?.get_password().ok()
    }

    fn set(&self, account: &str, value: &str) -> bool {
        Self::entry(account).is_some_and(|entry| entry.set_password(value).is_ok())
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let Some(entry) = Self::entry(account) else {
            return Ok(());
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

pub struct Credentials {
    memory: HashMap<String, String>,
    /// 消そうとしたがキーリングから消せなかった値。起動しているあいだは使わない
    forgotten: HashSet<String>,
    /// None ならキーリングを使わない（テスト用）
    store: Option<Box<dyn SecretStore>>,
}

impl Default for Credentials {
    fn default() -> Self {
        Self::with_store(Box::new(OsKeyring))
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
            forgotten: HashSet::new(),
            store: None,
        }
    }

    /// キーリングの代わりに `store` に保存する
    pub fn with_store(store: Box<dyn SecretStore>) -> Self {
        Self {
            memory: HashMap::new(),
            forgotten: HashSet::new(),
            store: Some(store),
        }
    }

    pub fn get(&self, plugin: &str, key: &str) -> Option<String> {
        let name = account(plugin, key);
        if self.forgotten.contains(&name) {
            return None;
        }
        if let Some(value) = self.memory.get(&name) {
            return Some(value.clone());
        }
        self.store.as_ref()?.get(&name)
    }

    /// どこに保存されているか。無ければ None
    pub fn stored(&self, plugin: &str, key: &str) -> Option<Stored> {
        let name = account(plugin, key);
        if self.forgotten.contains(&name) {
            return Some(Stored::DeleteFailed);
        }
        if self.memory.contains_key(&name) {
            return Some(Stored::Memory);
        }
        self.store.as_ref()?.get(&name).map(|_| Stored::Keyring)
    }

    /// キーリングに保存する。使えなければメモリに持つ
    pub fn set(&mut self, plugin: &str, key: &str, value: &str) -> Stored {
        let name = account(plugin, key);
        self.forgotten.remove(&name);
        if self
            .store
            .as_ref()
            .is_some_and(|store| store.set(&name, value))
        {
            self.memory.remove(&name);
            Stored::Keyring
        } else {
            self.memory.insert(name, value.to_owned());
            Stored::Memory
        }
    }

    /// 消す。キーリングから消せなかったら、その理由を返す（消せるまで、起動しているあいだは使わない）
    pub fn delete(&mut self, plugin: &str, key: &str) -> Result<(), String> {
        let name = account(plugin, key);
        self.memory.remove(&name);
        let result = self
            .store
            .as_ref()
            .map_or(Ok(()), |store| store.delete(&name));
        match result {
            Ok(()) => {
                self.forgotten.remove(&name);
                Ok(())
            }
            Err(error) => {
                self.forgotten.insert(name);
                Err(error)
            }
        }
    }

    /// プラグインに渡す、キーと値の組
    pub fn for_plugin(&self, plugin: &str, keys: &[String]) -> HashMap<String, String> {
        keys.iter()
            .filter_map(|key| Some((key.clone(), self.get(plugin, key)?)))
            .collect()
    }
}

/// テスト用の保存場所。消すのを失敗させられる
#[cfg(test)]
#[derive(Clone, Default)]
pub struct FakeStore {
    pub values: std::rc::Rc<std::cell::RefCell<HashMap<String, String>>>,
    pub fail_delete: std::rc::Rc<std::cell::Cell<bool>>,
}

#[cfg(test)]
impl SecretStore for FakeStore {
    fn get(&self, account: &str) -> Option<String> {
        self.values.borrow().get(account).cloned()
    }

    fn set(&self, account: &str, value: &str) -> bool {
        self.values
            .borrow_mut()
            .insert(account.to_owned(), value.to_owned());
        true
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        if self.fail_delete.get() {
            return Err("locked".to_owned());
        }
        self.values.borrow_mut().remove(account);
        Ok(())
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
        credentials.delete("test", "email").unwrap();
        assert_eq!(credentials.get("test", "email"), None);
        assert_eq!(credentials.stored("test", "email"), None);
    }

    #[test]
    fn values_that_could_not_be_deleted_are_not_used() {
        let store = FakeStore::default();
        let mut credentials = Credentials::with_store(Box::new(store.clone()));
        assert_eq!(credentials.set("p", "token", "t"), Stored::Keyring);
        assert_eq!(credentials.stored("p", "token"), Some(Stored::Keyring));

        store.fail_delete.set(true);
        assert_eq!(credentials.delete("p", "token"), Err("locked".to_owned()));
        // キーリングには残っているが、起動しているあいだは使わない
        assert!(store.values.borrow().contains_key("p:token"));
        assert_eq!(credentials.stored("p", "token"), Some(Stored::DeleteFailed));
        assert_eq!(credentials.get("p", "token"), None);
        assert!(
            credentials
                .for_plugin("p", &["token".to_owned()])
                .is_empty()
        );

        // もう一度消せば、消える
        store.fail_delete.set(false);
        assert_eq!(credentials.delete("p", "token"), Ok(()));
        assert_eq!(credentials.stored("p", "token"), None);
        // 入れ直せば、また使う
        credentials.set("p", "token", "u");
        assert_eq!(credentials.get("p", "token").as_deref(), Some("u"));
    }
}
