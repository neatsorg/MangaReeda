//! MangaReeda のキー設定を読み、ページに入れるキーの対応表と Kindle 側の操作を作る。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Kindle に関係する動作（settings.json の keys の名前）
pub const ACTIONS: [&str; 11] = [
    "next_page",
    "prev_page",
    "one_next_page",
    "one_prev_page",
    "first_page",
    "last_page",
    "fullscreen",
    "close_book",
    "quit_app",
    "toggle_auto_page_layout",
    "view_mode",
];

/// 動作に対する Kindle 側の操作
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Operation {
    /// ブラウザにキーを送る（KeyboardEvent.key の名前）
    Key(String),
    /// CSS セレクタに合う要素をクリックする
    Click(String),
    /// ページでスクリプトを実行する
    Js(String),
    /// プラグインが自分で行う: "fullscreen" "close" "library"
    Builtin(String),
}

/// 既定の操作。Kindle 側のキーやボタンは推測を含むので、actions.json で上書きできる
pub fn default_operations() -> BTreeMap<String, Operation> {
    let mut map = BTreeMap::new();
    // 推測: 右開きのマンガでは左矢印が次のページ
    map.insert("next_page".into(), Operation::Key("ArrowLeft".into()));
    map.insert("prev_page".into(), Operation::Key("ArrowRight".into()));
    map.insert("one_next_page".into(), Operation::Key("ArrowLeft".into()));
    map.insert("one_prev_page".into(), Operation::Key("ArrowRight".into()));
    map.insert("fullscreen".into(), Operation::Builtin("fullscreen".into()));
    map.insert("close_book".into(), Operation::Builtin("library".into()));
    map.insert("quit_app".into(), Operation::Builtin("close".into()));
    // 最初と最後のページは、進み具合のつまみで飛ぶ
    map.insert("first_page".into(), Operation::Js(go_to_page_script("1")));
    map.insert("last_page".into(), Operation::Js(go_to_page_script("total")));
    // Cloud Reader に切り替えの項目は無いが、縦長の画面だと1ページずつ表示する
    map.insert("toggle_auto_page_layout".into(), Operation::Builtin("single".into()));
    map
}

/// 今のページと総ページ数を返す JavaScript の関数（`mangareedaPage()`）。見開き（横）のときは
/// 進み具合のつまみ #sliderBar の値と最大値。つまみは下のバナーを一度出すまで作られず、
/// 縦スクロールのときは割合（0〜99）のつまみに変わるので、無ければ「165/199ページ」の文字から読む
pub const PAGE_FUNCTION: &str = "function mangareedaPage() {
  const s = document.querySelector('#sliderBar');
  if (s) return [Number(s.value), Number(s.max)];
  for (const e of document.querySelectorAll('.kw-wdg-page-info')) {
    const m = /(\\d+)\\s*\\/\\s*(\\d+)/.exec(e.textContent || '');
    if (m) return [Number(m[1]), Number(m[2])];
  }
  return null;
}";

/// `page` のページに飛ぶスクリプト。`page` は JavaScript の式で、総ページ数を `total` で使える。
/// 見開きのときは進み具合のつまみ（#sliderBar）に値を入れ、mouseup を送る（つまみは値を変えただけでは
/// 数字の表示しか変わらず、離したとき＝mouseup か touchend にページを移る）。
/// 縦スクロールのときは、右の縦のつまみ（割合 0〜99。上端＝値 99 が最初のページ）に値を入れ、
/// 同じく mouseup を送る（100 段階なので、ページ数の多い本ではおおよその位置になる）。
/// どちらのつまみも無ければ（バナーをまだ出していない）false を返す
pub fn go_to_page_script(page: &str) -> String {
    format!(
        "(() => {{
  {PAGE_FUNCTION}
  const s = document.querySelector('#sliderBar');
  const v = document.querySelector('#readerChromeRight input.kw-slider-input');
  const total = s ? Number(s.max) : (mangareedaPage() || [0, 0])[1];
  if (!total || !(s || v)) return false;
  const page = Math.min(Math.max(Math.round({page}), 1), total);
  const slider = s || v;
  const value = s ? page : Math.round(((total - page) / Math.max(total - 1, 1)) * Number(v.max));
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(slider, String(value));
  slider.dispatchEvent(new Event('input', {{ bubbles: true }}));
  slider.dispatchEvent(new MouseEvent('mouseup', {{ bubbles: true }}));
  return true;
}})()"
    )
}

/// 既定の操作に actions.json の中身を重ねる。値が null の動作は外す
pub fn load_operations(path: &Path) -> Result<BTreeMap<String, Operation>, String> {
    let mut map = default_operations();
    let Ok(text) = fs::read_to_string(path) else {
        return Ok(map);
    };
    let overrides: BTreeMap<String, Option<Operation>> = serde_json::from_str(&text)
        .map_err(|e| format!("{} を読めません: {e}", path.display()))?;
    for (action, operation) in overrides {
        match operation {
            Some(operation) => map.insert(action, operation),
            None => map.remove(&action),
        };
    }
    Ok(map)
}

/// ページの keydown と照らし合わせる形
#[derive(Serialize, Debug, PartialEq)]
pub struct Matcher {
    pub action: String,
    /// KeyboardEvent.code の候補
    pub codes: Vec<String>,
    /// KeyboardEvent.key の候補（大文字小文字は区別しない）
    pub keys: Vec<String>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

/// egui::Key の名前を DOM の code と key に直す
fn dom_names(egui_key: &str) -> (Vec<String>, Vec<String>) {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    if let Some(digit) = egui_key.strip_prefix("Num")
        && digit.len() == 1
        && digit.chars().all(|c| c.is_ascii_digit())
    {
        return (s(&[&format!("Digit{digit}"), &format!("Numpad{digit}")]), vec![]);
    }
    if egui_key.len() == 1 && egui_key.chars().all(|c| c.is_ascii_alphabetic()) {
        return (vec![format!("Key{}", egui_key.to_ascii_uppercase())], vec![]);
    }
    let key = match egui_key {
        "Space" => " ",
        "Minus" => "-",
        "Plus" => "+",
        "Equals" => "=",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Backslash" => "\\",
        "Semicolon" => ";",
        "Colon" => ":",
        "Quote" => "'",
        "OpenBracket" => "[",
        "CloseBracket" => "]",
        "Backtick" => "`",
        "Questionmark" => "?",
        "Exclamationmark" => "!",
        "Pipe" => "|",
        // ArrowLeft, Home, End, PageUp, Enter, Escape, F1… などは名前が同じ
        other => other,
    };
    (vec![], vec![key.to_owned()])
}

/// settings.json の keys から、操作が決まっている動作の対応表を作る
pub fn matchers(settings: &Value, operations: &BTreeMap<String, Operation>) -> Vec<Matcher> {
    let Some(keys) = settings.get("keys") else {
        return vec![];
    };
    ACTIONS
        .iter()
        .filter(|action| operations.contains_key(**action))
        .filter_map(|action| {
            let shortcut = keys.get(*action)?;
            let key = shortcut.get("key")?.as_str()?;
            let flag = |name: &str| shortcut.get(name).and_then(Value::as_bool).unwrap_or(false);
            let (codes, keys) = dom_names(key);
            let (ctrl, alt, shift) = (flag("ctrl"), flag("alt"), flag("shift"));
            // 送るキーが押したキーと同じなら、横取りせずに Kindle へそのまま渡す
            if let Some(Operation::Key(target)) = operations.get(*action)
                && !ctrl
                && !alt
                && !shift
                && (keys.contains(target) || codes.contains(&key_event_fields(target).0))
            {
                return None;
            }
            Some(Matcher {
                action: (*action).to_owned(),
                codes,
                keys,
                ctrl,
                alt,
                shift,
            })
        })
        .collect()
}

/// Input.dispatchKeyEvent に渡す code と仮想キーコード
pub fn key_event_fields(key: &str) -> (String, i64, Option<String>) {
    let (code, vk) = match key {
        "ArrowLeft" => ("ArrowLeft", 37),
        "ArrowUp" => ("ArrowUp", 38),
        "ArrowRight" => ("ArrowRight", 39),
        "ArrowDown" => ("ArrowDown", 40),
        "PageUp" => ("PageUp", 33),
        "PageDown" => ("PageDown", 34),
        "End" => ("End", 35),
        "Home" => ("Home", 36),
        "Enter" => ("Enter", 13),
        "Escape" => ("Escape", 27),
        " " => ("Space", 32),
        _ if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric()) => {
            let c = key.chars().next().unwrap().to_ascii_uppercase();
            let code = if c.is_ascii_digit() {
                format!("Digit{c}")
            } else {
                format!("Key{c}")
            };
            return (code, c as i64, Some(key.to_owned()));
        }
        _ => (key, 0),
    };
    let text = (key == " ").then(|| " ".to_owned());
    (code.to_owned(), vk, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn egui_names_become_dom_names() {
        assert_eq!(dom_names("Num0").0, vec!["Digit0", "Numpad0"]);
        assert_eq!(dom_names("O").0, vec!["KeyO"]);
        assert_eq!(dom_names("ArrowLeft").1, vec!["ArrowLeft"]);
        assert_eq!(dom_names("Space").1, vec![" "]);
    }

    #[test]
    fn only_actions_with_operations_are_mapped() {
        let settings = json!({"keys": {
            "next_page": {"key": "ArrowLeft", "ctrl": false, "alt": false, "shift": false},
            "open_file": {"key": "O", "ctrl": true, "alt": false, "shift": false},
            "fullscreen": {"key": "F11", "ctrl": false, "alt": false, "shift": false},
            "prev_page": null
        }});
        let found = matchers(&settings, &default_operations());
        let actions: Vec<_> = found.iter().map(|m| m.action.as_str()).collect();
        // next_page は ArrowLeft を ArrowLeft として送るだけなので横取りしない
        assert_eq!(actions, vec!["fullscreen"]);
    }

    #[test]
    fn keys_sent_as_something_else_are_mapped() {
        let settings = json!({"keys": {
            "next_page": {"key": "ArrowLeft", "ctrl": false, "alt": false, "shift": false},
            "one_next_page": {"key": "ArrowLeft", "ctrl": false, "alt": false, "shift": true}
        }});
        let mut operations = default_operations();
        operations.insert("next_page".into(), Operation::Key("PageDown".into()));
        let found = matchers(&settings, &operations);
        let actions: Vec<_> = found.iter().map(|m| m.action.as_str()).collect();
        assert_eq!(actions, vec!["next_page", "one_next_page"]);
    }

    #[test]
    fn overrides_replace_and_remove_operations() {
        let dir = std::env::temp_dir().join(format!("mangareeda-kindle-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("actions.json");
        fs::write(&path, r#"{"next_page": {"key": "PageDown"}, "quit_app": null}"#).unwrap();
        let map = load_operations(&path).unwrap();
        assert!(matches!(map.get("next_page"), Some(Operation::Key(k)) if k == "PageDown"));
        assert!(!map.contains_key("quit_app"));
        fs::remove_dir_all(&dir).unwrap();
    }
}
