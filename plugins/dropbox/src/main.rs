//! MangaReeda の Dropbox プラグイン。Dropbox のフォルダの中身（フォルダと本）を返し、本をダウンロードする。
//!
//! 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
//!   mangareeda-dropbox auth-url            ブラウザで許可するページの URL
//!   mangareeda-dropbox list [<フォルダ>]   フォルダの中身（省くと一番上）
//!   mangareeda-dropbox download <id> <保存先>
//! 標準入力に {"credentials": {"app_key": …, "auth_code": …, "code_verifier": …, "refresh_token": …}} を受け取る。
//!
//! 許可は、秘密鍵を持たない PKCE の方式で行う。リダイレクト先は付けず、Dropbox の画面に出た認可コードを
//! 利用者に貼ってもらう。認可コードは一度だけ更新用のトークンに取り替え、"store" で本体に保存してもらう。

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;
use ureq::Agent;

const AUTHORIZE_URL: &str = "https://www.dropbox.com/oauth2/authorize";
const TOKEN_URL: &str = "https://api.dropboxapi.com/oauth2/token";
const API_URL: &str = "https://api.dropboxapi.com/2";
const CONTENT_URL: &str = "https://content.dropboxapi.com/2";
/// 読むだけに要る権限
/// account_info.read は、接続したアカウントを見分けるため（users/get_current_account）
const SCOPES: &str = "account_info.read files.metadata.read files.content.read";
/// 本として並べる拡張子（MangaReeda が開けるもの）
const BOOK_EXTENSIONS: [&str; 5] = ["zip", "cbz", "rar", "cbr", "pdf"];
/// 混み合っている（429）ときに、待ってやり直す回数
const RETRIES: usize = 3;

const EXIT_AUTH: u8 = 2;
const EXIT_ERROR: u8 = 1;

#[derive(Deserialize, Default)]
struct Request {
    #[serde(default)]
    credentials: HashMap<String, String>,
}

impl Request {
    fn get(&self, key: &str) -> Option<&str> {
        // 貼り付けたときの前後の空白や改行は取る
        self.credentials
            .get(key)
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
    }
}

#[derive(Serialize, Debug, PartialEq)]
struct Book {
    id: String,
    title: String,
    file_name: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    folder: bool,
}

enum Failure {
    Auth(String),
    Other(String),
}

impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        Failure::Other(format!("Dropbox と通信できませんでした: {error}"))
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Other(format!("ファイルを書けませんでした: {error}"))
    }
}

const USAGE: &str = "使い方: mangareeda-dropbox auth-url | list [フォルダ] | download <id> <保存先>\n\
     MangaReeda から呼ばれ、標準入力に1行の JSON でログイン情報を受け取ります（docs/plugins.md）";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // 使い方は、標準入力を待たずに出す
    if matches!(args.first().map(String::as_str), Some("--help" | "-h")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let request: Request = serde_json::from_str(input.trim()).unwrap_or_default();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["auth-url"] => auth_url(&request),
        ["list"] => list(&request, ""),
        ["list", folder] => list(&request, folder),
        ["download", id, dest] => download(&request, id, Path::new(dest)),
        _ => Err(Failure::Other(USAGE.to_owned())),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Auth(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_AUTH)
        }
        Err(Failure::Other(message)) => {
            eprintln!("{message}");
            ExitCode::from(EXIT_ERROR)
        }
    }
}

fn agent() -> Agent {
    Agent::config_builder()
        .user_agent(concat!("MangaReeda-dropbox/", env!("CARGO_PKG_VERSION")))
        // つながらない・答えが来ないときに、いつまでも待たない
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        // 失敗の応答も中身を読んで説明に使う
        .http_status_as_error(false)
        .build()
        .into()
}

fn print(value: Value) -> Result<(), Failure> {
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{value}")?;
    Ok(())
}

fn app_key(request: &Request) -> Result<&str, Failure> {
    request
        .get("app_key")
        .ok_or_else(|| Failure::Auth("設定画面で Dropbox の App key を保存してください".to_owned()))
}

/// PKCE の検証用の値（43〜128 文字。使える文字は英数字と "-._~"）
fn new_verifier() -> Result<String, Failure> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";
    let mut bytes = [0u8; 64];
    getrandom::fill(&mut bytes)
        .map_err(|e| Failure::Other(format!("乱数を作れませんでした: {e}")))?;
    Ok(bytes
        .iter()
        .map(|b| CHARS[*b as usize % CHARS.len()] as char)
        .collect())
}

/// 検証用の値から、許可のページに渡す値を作る（S256）
fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn auth_url(request: &Request) -> Result<(), Failure> {
    let app_key = app_key(request)?;
    let verifier = new_verifier()?;
    let url = format!(
        "{AUTHORIZE_URL}?client_id={}&response_type=code&token_access_type=offline\
         &code_challenge={}&code_challenge_method=S256&scope={}",
        percent_encode(app_key),
        challenge(&verifier),
        percent_encode(SCOPES)
    );
    // 認可コードを取り替えるときに要るので、本体に保存してもらう。前の認可コードは消す
    print(json!({ "store": { "code_verifier": verifier, "auth_code": "" } }))?;
    print(json!({ "url": url }))
}

/// URL の値に使えない文字を %XX にする
fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    /// 許可したアカウント（認可コードを取り替えたときだけ返る）
    #[serde(default)]
    account_id: Option<String>,
}

/// 使える access token と、取り替えの答えでわかったアカウントの id
struct Token {
    access: String,
    account_id: Option<String>,
}

/// 使える access token を得る。認可コードがあれば更新用のトークンに取り替えて保存してもらう
fn access_token(agent: &Agent, request: &Request) -> Result<Token, Failure> {
    let app_key = app_key(request)?;
    if let Some(code) = request.get("auth_code") {
        let verifier = request.get("code_verifier").ok_or_else(|| {
            Failure::Auth("もう一度「ブラウザで許可する」から始めてください".to_owned())
        })?;
        let mut response = agent.post(TOKEN_URL).send_form([
            ("code", code),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
            ("client_id", app_key),
        ])?;
        if !response.status().is_success() {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            // 使えない認可コードは消してもらう（残っていると、次も更新用のトークンより先に使ってしまう）
            print(json!({ "store": { "auth_code": "" } }))?;
            return Err(Failure::Auth(format!(
                "認可コードを使えませんでした。もう一度「ブラウザで許可する」から始めてください（{}）",
                error_summary(&body)
            )));
        }
        let reply: TokenReply = response.body_mut().read_json()?;
        let refresh = reply.refresh_token.unwrap_or_default();
        print(json!({ "store": {
            "refresh_token": refresh,
            "auth_code": "",
            "code_verifier": "",
            "account_id": reply.account_id.clone().unwrap_or_default(),
        }}))?;
        return Ok(Token {
            access: reply.access_token,
            account_id: reply.account_id,
        });
    }
    let Some(refresh) = request.get("refresh_token") else {
        return Err(Failure::Auth(
            "設定画面の「ブラウザで許可する」から Dropbox に接続してください".to_owned(),
        ));
    };
    let mut response = agent.post(TOKEN_URL).send_form([
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", app_key),
    ])?;
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(Failure::Auth(format!(
            "Dropbox の許可が使えなくなりました。もう一度「ブラウザで許可する」から接続してください（{}）",
            error_summary(&body)
        )));
    }
    Ok(Token {
        access: response.body_mut().read_json::<TokenReply>()?.access_token,
        account_id: None,
    })
}

/// 失敗の応答の、短い説明
fn error_summary(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    value
        .get("error_summary")
        .or_else(|| value.get("error_description"))
        .or_else(|| value.get("error"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// API を呼ぶ。混み合っていれば（429）言われた秒数だけ待ってやり直す
fn call_api(agent: &Agent, token: &str, endpoint: &str, body: &Value) -> Result<Value, Failure> {
    for attempt in 0..=RETRIES {
        let mut response = agent
            .post(format!("{API_URL}{endpoint}"))
            .header("Authorization", format!("Bearer {token}"))
            .send_json(body)?;
        let status = response.status().as_u16();
        if status == 429 && attempt < RETRIES {
            sleep(retry_after(&response));
            continue;
        }
        let text = response.body_mut().read_to_string()?;
        return match status {
            200 => serde_json::from_str(&text)
                .map_err(|e| Failure::Other(format!("Dropbox の答えを読めませんでした: {e}"))),
            401 => Err(Failure::Auth(format!(
                "Dropbox の許可が使えません。もう一度接続してください（{}）",
                error_summary(&text)
            ))),
            409 => Err(Failure::Other(format!(
                "フォルダが見つかりません（{}）",
                error_summary(&text)
            ))),
            _ => Err(Failure::Other(format!(
                "Dropbox がエラーを返しました: {status} {}",
                error_summary(&text)
            ))),
        };
    }
    Err(Failure::Other("Dropbox が混み合っています".to_owned()))
}

fn retry_after(response: &ureq::http::Response<ureq::Body>) -> Duration {
    let seconds = response
        .headers()
        .get("retry-after")
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
        .unwrap_or(2);
    Duration::from_secs(seconds.clamp(1, 60))
}

/// 接続しているアカウントの id。認可コードを取り替えたばかりならその答えの値、
/// 次に覚えている値を使う。どちらも無ければ Dropbox に尋ねて、保存してもらう。
/// わからなくても一覧は返せるので、尋ねられなければ None
fn account_id(agent: &Agent, token: &Token, request: &Request) -> Option<String> {
    if let Some(id) = &token.account_id {
        return Some(id.clone());
    }
    if let Some(id) = request.get("account_id") {
        return Some(id.to_owned());
    }
    let reply = call_api(
        agent,
        &token.access,
        "/users/get_current_account",
        &Value::Null,
    )
    .ok()?;
    let id = reply.get("account_id")?.as_str()?.to_owned();
    print(json!({ "store": { "account_id": id } })).ok()?;
    Some(id)
}

fn list(request: &Request, folder: &str) -> Result<(), Failure> {
    let agent = agent();
    let token = access_token(&agent, request)?;
    let account = account_id(&agent, &token, request);
    let token = token.access;
    let mut entries = Vec::new();
    let mut page = call_api(
        &agent,
        &token,
        "/files/list_folder",
        &json!({
            "path": folder,
            "recursive": false,
            "limit": 2000,
            "include_non_downloadable_files": false,
        }),
    )?;
    loop {
        if let Some(list) = page.get("entries").and_then(Value::as_array) {
            entries.extend(list.iter().cloned());
        }
        if page.get("has_more").and_then(Value::as_bool) != Some(true) {
            break;
        }
        let cursor = page
            .get("cursor")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        page = call_api(
            &agent,
            &token,
            "/files/list_folder/continue",
            &json!({ "cursor": cursor }),
        )?;
    }
    print(json!({
        "books": books_from_entries(&entries),
        "parent": parent_of(folder),
        "account": account,
    }))
}

/// Dropbox の一覧の項目から、フォルダと本だけを、フォルダを先に名前順で並べる
fn books_from_entries(entries: &[Value]) -> Vec<Book> {
    let mut books: Vec<Book> = entries
        .iter()
        .filter_map(|entry| {
            let tag = entry.get(".tag")?.as_str()?;
            let name = entry.get("name")?.as_str()?.to_owned();
            let path = entry.get("path_display")?.as_str()?.to_owned();
            let folder = match tag {
                "folder" => true,
                "file" => {
                    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
                    if !BOOK_EXTENSIONS.contains(&extension.as_str()) {
                        return None;
                    }
                    false
                }
                _ => return None,
            };
            Some(Book {
                // ダウンロード先にも Dropbox のフォルダの並びを保つ
                file_name: path.trim_start_matches('/').to_owned(),
                id: path,
                title: name,
                folder,
            })
        })
        .collect();
    books.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    books
}

/// 上のフォルダ。一番上（""）なら None、一つ下なら ""
fn parent_of(folder: &str) -> Option<String> {
    if folder.is_empty() || folder == "/" {
        return None;
    }
    let trimmed = folder.trim_end_matches('/');
    Some(
        trimmed
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent)
            .to_owned(),
    )
}

/// HTTP ヘッダーに入れられる JSON（ASCII の外の文字を \uXXXX にする）
fn header_json(value: &Value) -> String {
    let mut out = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() && c != '\u{7f}' {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    out
}

fn download(request: &Request, id: &str, dest: &Path) -> Result<(), Failure> {
    let agent = agent();
    let token = access_token(&agent, request)?.access;
    let arg = header_json(&json!({ "path": id }));
    let mut attempt = 0;
    let response = loop {
        let response = agent
            .post(format!("{CONTENT_URL}/files/download"))
            .header("Authorization", format!("Bearer {token}"))
            .header("Dropbox-API-Arg", &arg)
            .send_empty()?;
        let status = response.status().as_u16();
        if status == 429 && attempt < RETRIES {
            attempt += 1;
            sleep(retry_after(&response));
            continue;
        }
        if status != 200 {
            let mut response = response;
            let text = response.body_mut().read_to_string().unwrap_or_default();
            let message = format!("ダウンロードできませんでした（{}）", error_summary(&text));
            return Err(if status == 401 {
                Failure::Auth(message)
            } else {
                Failure::Other(message)
            });
        }
        break response;
    };
    // 大きさは Content-Length か、Dropbox-API-Result の size から
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok()?.parse().ok())
        .or_else(|| {
            let result = response
                .headers()
                .get("dropbox-api-result")?
                .to_str()
                .ok()?;
            serde_json::from_str::<Value>(result)
                .ok()?
                .get("size")?
                .as_u64()
        });
    let mut reader = response.into_body().into_reader();
    save_book(&mut reader, dest, total, |received| {
        print(json!({ "progress": received, "total": total }))
    })
}

/// 本を `dest` に書く。途中で止まったときに壊れたファイルが残らないよう、同じフォルダの一時ファイルに書いてから
/// 名前を変える。一時ファイルは、新しく作れた名前だけを使う（あるファイルを切り詰めたり、リンクの先に書いたりしない）。
/// 失敗したら一時ファイルを消す。`progress` には受け取った量を知らせる
fn save_book(
    reader: &mut impl Read,
    dest: &Path,
    total: Option<u64>,
    mut progress: impl FnMut(u64) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let (partial, file) = create_partial(dest)?;
    let result = receive(reader, file, total, &mut progress)
        .and_then(|received| Ok((std::fs::rename(&partial, dest)?, received)));
    match result {
        Ok(((), received)) => progress(received),
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

/// 保存先と同じフォルダに、ほかと重ならない一時ファイルを新しく作る
fn create_partial(dest: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    let name = dest.file_name().map_or_else(
        || "book".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for serial in 0..100 {
        let partial = dest.with_file_name(format!(".{name}.{}-{serial}.part", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
        {
            Ok(file) => return Ok((partial, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "一時ファイルの名前を決められませんでした",
    ))
}

/// 受け取って書く。受け取った量を返す。1MB ごとに進み具合を知らせる
fn receive(
    reader: &mut impl Read,
    mut file: std::fs::File,
    total: Option<u64>,
    progress: &mut impl FnMut(u64) -> Result<(), Failure>,
) -> Result<u64, Failure> {
    let mut buffer = vec![0; 256 * 1024];
    let (mut received, mut reported) = (0u64, 0u64);
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read])?;
        received += read as u64;
        if received - reported >= 1024 * 1024 {
            reported = received;
            progress(received)?;
        }
    }
    file.sync_all()?;
    if total.is_some_and(|total| total != received) {
        return Err(Failure::Other("ダウンロードが途中で切れました".to_owned()));
    }
    Ok(received)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一時ファイルは新しい名前で作り、同じ名前のファイルがあっても切り詰めない。失敗したら消す
    #[test]
    fn books_are_written_without_touching_other_files() {
        let dir = std::env::temp_dir().join(format!(
            "{}-save-{}",
            env!("CARGO_PKG_NAME"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("book.pdf");
        let other = dir.join("book.part");
        std::fs::write(&other, "keep").unwrap();
        let mut reported = Vec::new();
        save_book(&mut &b"data"[..], &dest, Some(4), |n| {
            reported.push(n);
            Ok(())
        })
        .ok()
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
        assert_eq!(std::fs::read(&other).unwrap(), b"keep");
        assert_eq!(reported, [4]);
        // 途中で切れたら、本も一時ファイルも残さない
        let cut = dir.join("cut.pdf");
        assert!(save_book(&mut &b"da"[..], &cut, Some(4), |_| Ok(())).is_err());
        assert!(!cut.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn challenge_matches_the_pkce_example() {
        // RFC 7636 の付録 B の例
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let verifier = new_verifier().ok().unwrap();
        assert_eq!(verifier.len(), 64);
        assert!(
            verifier
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._~".contains(c))
        );
    }

    #[test]
    fn entries_become_folders_then_books() {
        let entries: Vec<Value> = serde_json::from_str(
            r#"[
            {".tag": "file", "name": "b.ZIP", "path_display": "/Comics/b.ZIP"},
            {".tag": "file", "name": "memo.txt", "path_display": "/Comics/memo.txt"},
            {".tag": "folder", "name": "Series", "path_display": "/Comics/Series"},
            {".tag": "deleted", "name": "old.zip", "path_display": "/Comics/old.zip"},
            {".tag": "file", "name": "a.pdf", "path_display": "/Comics/a.pdf"}
        ]"#,
        )
        .unwrap();
        let book = |id: &str, title: &str, folder| Book {
            id: id.to_owned(),
            title: title.to_owned(),
            file_name: id.trim_start_matches('/').to_owned(),
            folder,
        };
        assert_eq!(
            books_from_entries(&entries),
            [
                book("/Comics/Series", "Series", true),
                book("/Comics/a.pdf", "a.pdf", false),
                book("/Comics/b.ZIP", "b.ZIP", false),
            ]
        );
    }

    #[test]
    fn parents_go_up_to_the_top() {
        assert_eq!(parent_of(""), None);
        assert_eq!(parent_of("/Comics").as_deref(), Some(""));
        assert_eq!(parent_of("/Comics/Series").as_deref(), Some("/Comics"));
    }

    #[test]
    fn header_json_escapes_non_ascii() {
        assert_eq!(
            header_json(&json!({ "path": "/漫画/😀.zip" })),
            "{\"path\":\"/\\u6f2b\\u753b/\\ud83d\\ude00.zip\"}"
        );
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
    }
}
