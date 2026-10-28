//! ブックリストのプラグイン。プラグインは外部のプログラムで、一覧（list）とダウンロード（download）の
//! 2つの命令に答える。ログイン情報は標準入力に JSON で渡し、答えは標準出力に JSON で受け取る。
//! 取り決めの詳細は docs/plugins.md。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;

/// 終了コード: ログインできなかった
const EXIT_AUTH: i32 = 2;
/// 本体と同じ場所に置いて使う、同梱のプラグイン
const BUNDLED: [(&str, &str, &str); 1] = [("bookscan", "BOOKSCAN", "mangareeda-bookscan")];

/// ログイン情報の項目
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct CredentialField {
    pub key: String,
    /// 設定画面に出す名前。無ければ key（email と password は本体が訳す）
    #[serde(default)]
    pub label: Option<String>,
    /// 入力を伏せ字にするか
    #[serde(default)]
    pub secret: bool,
}

/// プラグインの説明（plugin.json）
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    /// 実行する命令。"./" で始まるものは plugin.json のフォルダから探す
    pub command: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<CredentialField>,
    #[serde(skip)]
    pub dir: PathBuf,
}

/// プラグインが返す本
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RemoteBook {
    /// プラグインの中での名前。ダウンロードのときにそのまま返す
    pub id: String,
    pub title: String,
    /// 保存するときのファイル名
    pub file_name: String,
    #[serde(default)]
    pub cover_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginError {
    /// ログインできなかった（ログイン情報を設定してほしい）
    Auth(String),
    Other(String),
}

impl PluginError {
    pub fn message(&self) -> &str {
        match self {
            Self::Auth(message) | Self::Other(message) => message,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadEvent {
    Progress { received: u64, total: Option<u64> },
    Done(Result<(), PluginError>),
}

impl PluginManifest {
    /// ログイン情報の項目のキー
    pub fn keys(&self) -> Vec<String> {
        self.credentials.iter().map(|f| f.key.clone()).collect()
    }

    fn command(&self, args: &[&str]) -> Option<Command> {
        let (program, rest) = self.command.split_first()?;
        let program = match program.strip_prefix("./") {
            Some(relative) => self.dir.join(relative),
            None => PathBuf::from(program),
        };
        let mut command = Command::new(program);
        command
            .args(rest)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // コンソールの窓を出さない（CREATE_NO_WINDOW）
            command.creation_flags(0x0800_0000);
        }
        Some(command)
    }
}

/// `plugins_dir/*/plugin.json` と、`exe_dir` に置いた同梱のプラグイン。同じ id は先に見つけたほう
pub fn discover(plugins_dir: &Path, exe_dir: Option<&Path>) -> Vec<PluginManifest> {
    let mut found: Vec<PluginManifest> = Vec::new();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(plugins_dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    dirs.sort();
    for dir in dirs {
        let Ok(data) = std::fs::read_to_string(dir.join("plugin.json")) else {
            continue;
        };
        match serde_json::from_str::<PluginManifest>(&data) {
            Ok(mut manifest) if !manifest.command.is_empty() => {
                manifest.dir = dir;
                if !found.iter().any(|m| m.id == manifest.id) {
                    found.push(manifest);
                }
            }
            _ => eprintln!("プラグインの説明を読めませんでした: {}", dir.display()),
        }
    }
    let Some(exe_dir) = exe_dir else {
        return found;
    };
    for (id, name, program) in BUNDLED {
        let path = exe_dir.join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
        if path.is_file() && !found.iter().any(|m| m.id == id) {
            found.push(PluginManifest {
                id: id.to_owned(),
                name: name.to_owned(),
                command: vec![path.to_string_lossy().into_owned()],
                credentials: vec![
                    CredentialField {
                        key: "email".to_owned(),
                        label: None,
                        secret: false,
                    },
                    CredentialField {
                        key: "password".to_owned(),
                        label: None,
                        secret: true,
                    },
                ],
                dir: exe_dir.to_path_buf(),
            });
        }
    }
    found
}

/// プラグインに渡す1行目
fn request_line(credentials: &HashMap<String, String>) -> String {
    serde_json::json!({ "credentials": credentials }).to_string()
}

fn failure(code: Option<i32>, stderr: &str) -> PluginError {
    let message = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("プラグインが失敗しました")
        .trim()
        .to_owned();
    if code == Some(EXIT_AUTH) {
        PluginError::Auth(message)
    } else {
        PluginError::Other(message)
    }
}

#[derive(Deserialize)]
struct ListReply {
    books: Vec<RemoteBook>,
}

/// 一覧を取る。終わるまで待つので、別スレッドで呼ぶ
pub fn list(
    manifest: &PluginManifest,
    credentials: &HashMap<String, String>,
) -> Result<Vec<RemoteBook>, PluginError> {
    let mut command = manifest
        .command(&["list"])
        .ok_or_else(|| PluginError::Other("プラグインの命令がありません".to_owned()))?;
    let mut child = command
        .spawn()
        .map_err(|e| PluginError::Other(format!("プラグインを起動できませんでした: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "{}", request_line(credentials));
    }
    let output = child
        .wait_with_output()
        .map_err(|e| PluginError::Other(e.to_string()))?;
    if !output.status.success() {
        return Err(failure(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reply: ListReply = stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| PluginError::Other("プラグインの答えを読めませんでした".to_owned()))?;
    Ok(reply.books)
}

/// `list` を別スレッドで行う
pub fn spawn_list(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
) -> Receiver<Result<Vec<RemoteBook>, PluginError>> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(list(&manifest, &credentials));
    });
    rx
}

#[derive(Deserialize)]
struct ProgressLine {
    progress: u64,
    #[serde(default)]
    total: Option<u64>,
}

/// 本を `dest` にダウンロードする。進み具合と結果を送る
pub fn spawn_download(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
    id: String,
    dest: PathBuf,
) -> Receiver<DownloadEvent> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let done = |result| {
            let _ = tx.send(DownloadEvent::Done(result));
        };
        if let Some(parent) = dest.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            return done(Err(PluginError::Other(format!(
                "保存先のフォルダを作れませんでした: {e}"
            ))));
        }
        let dest_text = dest.to_string_lossy().into_owned();
        let Some(mut command) = manifest.command(&["download", &id, &dest_text]) else {
            return done(Err(PluginError::Other(
                "プラグインの命令がありません".to_owned(),
            )));
        };
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(e) => {
                return done(Err(PluginError::Other(format!(
                    "プラグインを起動できませんでした: {e}"
                ))));
            }
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{}", request_line(&credentials));
        }
        // 標準エラーは別に読んでおく（詰まらないように）
        let stderr = child.stderr.take().map(|mut stderr| {
            thread::spawn(move || {
                let mut text = String::new();
                let _ = std::io::Read::read_to_string(&mut stderr, &mut text);
                text
            })
        });
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(progress) = serde_json::from_str::<ProgressLine>(&line) {
                    let event = DownloadEvent::Progress {
                        received: progress.progress,
                        total: progress.total,
                    };
                    if tx.send(event).is_err() {
                        // 受け手がいなくなった（アプリを閉じた）ら止める
                        let _ = child.kill();
                        return;
                    }
                }
            }
        }
        let status = child.wait();
        let stderr = stderr
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        let result = match status {
            Ok(status) if status.success() && dest.is_file() => Ok(()),
            Ok(status) => Err(failure(status.code(), &stderr)),
            Err(e) => Err(PluginError::Other(e.to_string())),
        };
        done(result);
    });
    rx
}

/// ダウンロードした本を置く場所。ファイル名にフォルダの区切りなどがあれば置き換える
pub fn local_path(library: &Path, plugin_id: &str, file_name: &str) -> PathBuf {
    let safe: String = file_name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let safe = safe.trim().trim_start_matches('.');
    let safe = if safe.is_empty() { "book" } else { safe };
    library.join(plugin_id).join(safe)
}

/// 一覧を手元にためておくファイル
#[derive(Serialize, Deserialize, Default)]
pub struct ListCache {
    pub books: Vec<RemoteBook>,
    /// 取った日時（UNIX 秒）
    pub fetched_at: u64,
}

impl ListCache {
    pub fn load(path: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save(&self, path: &Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("manga-plugin-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn manifests_are_found_in_folders_and_beside_the_program() {
        let plugins = temp("discover");
        let one = plugins.join("one");
        std::fs::create_dir(&one).unwrap();
        std::fs::write(
            one.join("plugin.json"),
            r#"{"id": "one", "name": "One", "command": ["./run.sh"],
                "credentials": [{"key": "token", "label": "Token", "secret": true}]}"#,
        )
        .unwrap();
        // 壊れた説明は飛ばす
        std::fs::create_dir(plugins.join("broken")).unwrap();
        std::fs::write(plugins.join("broken").join("plugin.json"), "{").unwrap();
        let exe = temp("exe");
        std::fs::write(
            exe.join(format!(
                "mangareeda-bookscan{}",
                std::env::consts::EXE_SUFFIX
            )),
            "",
        )
        .unwrap();

        let found = discover(&plugins, Some(&exe));
        assert_eq!(
            found.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["one", "bookscan"]
        );
        assert_eq!(found[0].dir, one);
        assert!(found[0].credentials[0].secret);
        assert_eq!(found[1].keys(), ["email", "password"]);
        assert_eq!(discover(&plugins, None).len(), 1);
        std::fs::remove_dir_all(plugins).unwrap();
        std::fs::remove_dir_all(exe).unwrap();
    }

    #[test]
    fn file_names_cannot_leave_the_library() {
        let library = Path::new("/lib");
        assert_eq!(
            local_path(library, "p", "a/b:c.pdf"),
            Path::new("/lib/p/a_b_c.pdf")
        );
        assert_eq!(
            local_path(library, "p", "../x.pdf"),
            Path::new("/lib/p/_x.pdf")
        );
        assert_eq!(local_path(library, "p", ".."), Path::new("/lib/p/book"));
    }

    /// シェルスクリプトで作ったプラグインと、実際にやり取りする
    #[cfg(unix)]
    #[test]
    fn plugins_answer_list_and_download_through_pipes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp("script");
        let script = dir.join("run.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
read request
case "$request" in *'"token":"good"'*) ;; *) echo "bad token" >&2; exit 2;; esac
case "$1" in
list) echo '{"books": [{"id": "1", "title": "T", "file_name": "t.cbz", "cover_url": "https://example.invalid/c.jpg"}]}';;
download) echo '{"progress": 5, "total": 10}'; printf data > "$3"; echo '{"progress": 10, "total": 10}';;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let manifest = PluginManifest {
            id: "s".to_owned(),
            name: "S".to_owned(),
            command: vec!["./run.sh".to_owned()],
            credentials: Vec::new(),
            dir: dir.clone(),
        };
        let good = HashMap::from([("token".to_owned(), "good".to_owned())]);
        let books = list(&manifest, &good).unwrap();
        assert_eq!(books[0].file_name, "t.cbz");
        assert_eq!(
            list(&manifest, &HashMap::new()),
            Err(PluginError::Auth("bad token".to_owned()))
        );

        let dest = dir.join("out").join("t.cbz");
        let events: Vec<_> = spawn_download(manifest, good, "1".to_owned(), dest.clone())
            .iter()
            .collect();
        assert_eq!(
            events,
            [
                DownloadEvent::Progress {
                    received: 5,
                    total: Some(10)
                },
                DownloadEvent::Progress {
                    received: 10,
                    total: Some(10)
                },
                DownloadEvent::Done(Ok(())),
            ]
        );
        assert_eq!(std::fs::read_to_string(dest).unwrap(), "data");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
