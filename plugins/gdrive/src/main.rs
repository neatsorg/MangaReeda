//! MangaReeda の Google Drive プラグイン。マイドライブのフォルダの中身（フォルダと本）を返し、本をダウンロードする。
//!
//! 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
//!   mangareeda-gdrive auth-url            ブラウザで許可するページの URL
//!   mangareeda-gdrive list [<フォルダ>]   フォルダの中身（省くとマイドライブの一番上）
//!   mangareeda-gdrive download <id> <保存先>
//! 標準入力に {"credentials": {"client_id": …, "client_secret": …, "auth_response": …, …}} を受け取る。
//!
//! 許可は PKCE の方式で行う。Google はコードを画面に出す方式をやめているので、リダイレクト先を
//! 手元（127.0.0.1）にする。何も待ち受けていないのでページは開けないが、アドレス欄の URL に認可コードが
//! 入っているので、その URL（かコード）を利用者に貼ってもらう。認可コードは一度だけ更新用のトークンに
//! 取り替え、"store" で本体に保存してもらう。

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

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const API_URL: &str = "https://www.googleapis.com/drive/v3";
/// 読むだけの権限
const SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
/// 許可したあとに移る先。何も待ち受けない（アドレス欄の URL を貼ってもらう）
const REDIRECT_URI: &str = "http://127.0.0.1:47853/";
const FOLDER_TYPE: &str = "application/vnd.google-apps.folder";
/// 本として並べる拡張子（MangaReeda が開けるもの）
const BOOK_EXTENSIONS: [&str; 5] = ["zip", "cbz", "rar", "cbr", "pdf"];
/// 混み合っているときに、待ってやり直す回数
const RETRIES: u32 = 4;
/// 上のフォルダをたどる深さの上限
const MAX_DEPTH: usize = 64;

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
    /// 表紙の画像（Drive が作ったサムネイルを base64 にしたもの）。本体が手元に置いて使う
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_data: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Failure {
    Auth(String),
    Other(String),
}

impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        Failure::Other(format!("Google Drive と通信できませんでした: {error}"))
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Other(format!("ファイルを書けませんでした: {error}"))
    }
}

const USAGE: &str = "使い方: mangareeda-gdrive auth-url | list [フォルダ] | download <id> <保存先>\n\
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
        .user_agent(concat!("MangaReeda-gdrive/", env!("CARGO_PKG_VERSION")))
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

/// 設定画面で入れてもらう、OAuth クライアントの ID とシークレット
fn client(request: &Request) -> Result<(&str, &str), Failure> {
    match (request.get("client_id"), request.get("client_secret")) {
        (Some(id), Some(secret)) => Ok((id, secret)),
        _ => Err(Failure::Auth(
            "設定画面で Google Drive のクライアント ID とクライアント シークレットを保存してください"
                .to_owned(),
        )),
    }
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
    let (client_id, _) = client(request)?;
    let verifier = new_verifier()?;
    // 更新用のトークンを毎回受け取れるよう、access_type=offline と prompt=consent を付ける
    let url = format!(
        "{AUTHORIZE_URL}?client_id={}&redirect_uri={}&response_type=code&scope={}\
         &code_challenge={}&code_challenge_method=S256&access_type=offline&prompt=consent",
        percent_encode(client_id),
        percent_encode(REDIRECT_URI),
        percent_encode(SCOPE),
        challenge(&verifier),
    );
    // 認可コードを取り替えるときに要るので、本体に保存してもらう。前に貼ったものは消す
    print(json!({ "store": { "code_verifier": verifier, "auth_response": "" } }))?;
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

/// %XX と "+" を元に戻す
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 貼ってもらった値から認可コードを取り出す。アドレス欄の URL でも、コードだけでもよい
fn code_from(pasted: &str) -> Result<String, Failure> {
    let Some((_, query)) = pasted.split_once('?') else {
        return Ok(pasted.to_owned());
    };
    let query = query.split('#').next().unwrap_or_default();
    let params: HashMap<&str, String> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key, percent_decode(value)))
        .collect();
    if let Some(code) = params.get("code") {
        return Ok(code.clone());
    }
    let reason = params.get("error").map_or("", String::as_str);
    Err(Failure::Auth(format!(
        "許可されませんでした（{reason}）。もう一度「ブラウザで許可する」から始めてください"
    )))
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

/// 使える access token。`exchanged` は、いま認可コードを取り替えた（別のアカウントになったかもしれない）
struct Token {
    access: String,
    exchanged: bool,
}

/// 失敗の応答の、短い説明
fn error_summary(body: &str) -> String {
    let value: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let error = value.get("error");
    error
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("error_description"))
        .or(error)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| body.chars().take(200).collect())
}

/// 使える access token を得る。貼ってもらった認可コードがあれば、更新用のトークンに取り替えて保存してもらう
fn access_token(agent: &Agent, request: &Request) -> Result<Token, Failure> {
    let (client_id, client_secret) = client(request)?;
    if let Some(pasted) = request.get("auth_response") {
        let code = code_from(pasted).inspect_err(|_| {
            // 使えない値は消してもらう（残っていると、次も更新用のトークンより先に使ってしまう）
            let _ = print(json!({ "store": { "auth_response": "" } }));
        })?;
        let verifier = request.get("code_verifier").ok_or_else(|| {
            Failure::Auth("もう一度「ブラウザで許可する」から始めてください".to_owned())
        })?;
        let mut response = agent.post(TOKEN_URL).send_form([
            ("code", code.as_str()),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", REDIRECT_URI),
        ])?;
        if !response.status().is_success() {
            let body = response.body_mut().read_to_string().unwrap_or_default();
            print(json!({ "store": { "auth_response": "" } }))?;
            return Err(Failure::Auth(format!(
                "認可コードを使えませんでした。もう一度「ブラウザで許可する」から始めてください（{}）",
                error_summary(&body)
            )));
        }
        let reply: TokenReply = response.body_mut().read_json()?;
        let Some(refresh) = reply.refresh_token else {
            print(json!({ "store": { "auth_response": "" } }))?;
            return Err(Failure::Auth(
                "更新用のトークンを受け取れませんでした。もう一度「ブラウザで許可する」から始めてください"
                    .to_owned(),
            ));
        };
        // アカウントは取り替えた access token で尋ね直すので、前の値は消す
        print(json!({ "store": {
            "refresh_token": refresh,
            "auth_response": "",
            "code_verifier": "",
            "account_id": "",
        }}))?;
        return Ok(Token {
            access: reply.access_token,
            exchanged: true,
        });
    }
    let Some(refresh) = request.get("refresh_token") else {
        return Err(Failure::Auth(
            "設定画面の「ブラウザで許可する」から Google Drive に接続してください".to_owned(),
        ));
    };
    let mut response = agent.post(TOKEN_URL).send_form([
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])?;
    if !response.status().is_success() {
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(Failure::Auth(format!(
            "Google Drive の許可が使えなくなりました。もう一度「ブラウザで許可する」から接続してください（{}）",
            error_summary(&body)
        )));
    }
    Ok(Token {
        access: response.body_mut().read_json::<TokenReply>()?.access_token,
        exchanged: false,
    })
}

/// 混み合っているという答えか（429 か、403 の rateLimitExceeded など）
fn rate_limited(status: u16, body: &str) -> bool {
    status == 429 || (status == 403 && body.contains("ateLimitExceeded"))
}

/// API を GET で呼ぶ。混み合っていれば、間を空けてやり直す
fn call_api(agent: &Agent, token: &str, path_and_query: &str) -> Result<Value, Failure> {
    for attempt in 0..=RETRIES {
        let mut response = agent
            .get(format!("{API_URL}{path_and_query}"))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string()?;
        if rate_limited(status, &text) && attempt < RETRIES {
            sleep(Duration::from_secs(1 << attempt));
            continue;
        }
        return match status {
            200 => serde_json::from_str(&text)
                .map_err(|e| Failure::Other(format!("Google Drive の答えを読めませんでした: {e}"))),
            401 => Err(Failure::Auth(format!(
                "Google Drive の許可が使えません。もう一度接続してください（{}）",
                error_summary(&text)
            ))),
            404 => Err(Failure::Other(format!(
                "フォルダが見つかりません（{}）",
                error_summary(&text)
            ))),
            _ => Err(Failure::Other(format!(
                "Google Drive がエラーを返しました: {status} {}",
                error_summary(&text)
            ))),
        };
    }
    Err(Failure::Other("Google Drive が混み合っています".to_owned()))
}

/// 接続しているアカウント（Drive の利用者の permissionId）。覚えていなければ尋ねて、保存してもらう。
/// わからなくても一覧は返せるので、尋ねられなければ None
fn account_id(agent: &Agent, token: &Token, request: &Request) -> Option<String> {
    if !token.exchanged
        && let Some(id) = request.get("account_id")
    {
        return Some(id.to_owned());
    }
    let about = call_api(agent, &token.access, "/about?fields=user(permissionId)").ok()?;
    let id = about.get("user")?.get("permissionId")?.as_str()?.to_owned();
    print(json!({ "store": { "account_id": id } })).ok()?;
    Some(id)
}

/// Drive の id として正しいか（問い合わせの文に埋め込むので、英数字と "-_" だけを受け付ける）
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// フォルダの名前と上のフォルダ
fn folder_info(agent: &Agent, token: &str, id: &str) -> Result<(String, Option<String>), Failure> {
    let meta = call_api(agent, token, &format!("/files/{id}?fields=name,parents"))?;
    let name = meta
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let parent = meta
        .get("parents")
        .and_then(Value::as_array)
        .and_then(|parents| parents.first())
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((name, parent))
}

/// フォルダの、マイドライブの一番上からの名前の並びと、上のフォルダ（一番上の一つ下なら ""）
fn folder_path(
    agent: &Agent,
    token: &str,
    root: &str,
    folder: &str,
) -> Result<(Vec<String>, String), Failure> {
    let mut names = Vec::new();
    let mut parent = None;
    let mut current = folder.to_owned();
    for _ in 0..MAX_DEPTH {
        if current == root {
            break;
        }
        let (name, up) = folder_info(agent, token, &current)?;
        names.push(name);
        let Some(up) = up else {
            // 共有されたものなど、マイドライブの外。ここまでを並びとする
            break;
        };
        parent.get_or_insert_with(|| up.clone());
        current = up;
    }
    names.reverse();
    let parent = parent.filter(|parent| parent != root).unwrap_or_default();
    Ok((names, parent))
}

fn list(request: &Request, folder: &str) -> Result<(), Failure> {
    if !folder.is_empty() && !valid_id(folder) {
        return Err(Failure::Other(format!(
            "フォルダの id が正しくありません: {folder}"
        )));
    }
    let agent = agent();
    let token = access_token(&agent, request)?;
    let account = account_id(&agent, &token, request);
    let token = token.access;
    let (names, parent) = if folder.is_empty() {
        (Vec::new(), None)
    } else {
        let root = call_api(&agent, &token, "/files/root?fields=id")?
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("root")
            .to_owned();
        let (names, parent) = folder_path(&agent, &token, &root, folder)?;
        (names, Some(parent))
    };
    let query = format!(
        "'{}' in parents and trashed = false",
        if folder.is_empty() { "root" } else { folder }
    );
    let mut files = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut path = format!(
            "/files?q={}&fields={}&pageSize=1000",
            percent_encode(&query),
            percent_encode("nextPageToken,files(id,name,mimeType,thumbnailLink)")
        );
        if let Some(page) = &page_token {
            path.push_str(&format!("&pageToken={}", percent_encode(page)));
        }
        let page = call_api(&agent, &token, &path)?;
        if let Some(list) = page.get("files").and_then(Value::as_array) {
            files.extend(list.iter().cloned());
        }
        page_token = page
            .get("nextPageToken")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if page_token.is_none() {
            break;
        }
    }
    let mut books = books_from_files(&files, &names);
    let covers = add_covers(&agent, &token, &files, &mut books);
    print(json!({
        "books": books,
        "parent": parent,
        "account": account,
        "message": covers.message(),
    }))
}

/// 表紙を付けた結果
#[derive(Default, Debug, PartialEq)]
struct CoverCount {
    /// 付けた
    found: usize,
    /// Drive にサムネイルが無かった
    missing: usize,
    /// サムネイルを取れなかった
    failed: usize,
}

impl CoverCount {
    /// 利用者に見せる一言。本が無ければ None
    fn message(&self) -> Option<String> {
        let total = self.found + self.missing + self.failed;
        if total == 0 {
            return None;
        }
        let mut message = format!("表紙 {}/{total} 冊", self.found);
        if self.missing > 0 {
            message.push_str(&format!("（Drive にサムネイルが無い {} 冊", self.missing));
            if self.failed > 0 {
                message.push_str(&format!("、取れなかった {} 冊", self.failed));
            }
            message.push('）');
        } else if self.failed > 0 {
            message.push_str(&format!("（取れなかった {} 冊）", self.failed));
        }
        Some(message)
    }
}

/// 表紙の画像の大きさの上限
const COVER_LIMIT: u64 = 2 * 1024 * 1024;

/// Drive が作ったサムネイル（PDF など）を取って、本に付ける。サムネイルの URL はすぐ使えなくなり、
/// 共有していないファイルではログインの情報も要るので、本体ではなくここで取る。取れなければ付けない
fn add_covers(agent: &Agent, token: &str, files: &[Value], books: &mut [Book]) -> CoverCount {
    let links: HashMap<&str, &str> = files
        .iter()
        .filter_map(|file| {
            Some((
                file.get("id")?.as_str()?,
                file.get("thumbnailLink")?.as_str()?,
            ))
        })
        .collect();
    let mut count = CoverCount::default();
    let total = books.iter().filter(|book| !book.folder).count();
    for (done, book) in books.iter_mut().filter(|book| !book.folder).enumerate() {
        // 進み具合を知らせる（本体の「更新中」に出る）
        let _ = print(json!({ "progress": done, "total": total }));
        let Some(link) = links.get(book.id.as_str()) else {
            count.missing += 1;
            continue;
        };
        book.cover_data = fetch_cover(agent, token, &thumbnail_size(link, 400));
        if book.cover_data.is_some() {
            count.found += 1;
        } else {
            count.failed += 1;
        }
    }
    let _ = print(json!({ "progress": total, "total": total }));
    count
}

/// サムネイルの URL の大きさの指定（末尾の "=s220" など）を `side` にする
fn thumbnail_size(link: &str, side: u32) -> String {
    match link.rsplit_once("=s") {
        Some((base, size)) if size.chars().all(|c| c.is_ascii_digit()) => {
            format!("{base}=s{side}")
        }
        _ => link.to_owned(),
    }
}

fn fetch_cover(agent: &Agent, token: &str, url: &str) -> Option<String> {
    let mut response = agent
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    let bytes = response
        .body_mut()
        .with_config()
        .limit(COVER_LIMIT)
        .read_to_vec()
        .ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Drive の一覧の項目から、フォルダと本だけを、フォルダを先に名前順で並べる。
/// `folders` は一覧のフォルダの、一番上からの名前の並び（ダウンロード先にもフォルダの並びを保つ）
fn books_from_files(files: &[Value], folders: &[String]) -> Vec<Book> {
    let mut books: Vec<Book> = files
        .iter()
        .filter_map(|file| {
            let id = file.get("id")?.as_str()?.to_owned();
            let name = file.get("name")?.as_str()?.to_owned();
            let folder = match file.get("mimeType")?.as_str()? {
                FOLDER_TYPE => true,
                mime if mime.starts_with("application/vnd.google-apps.") => return None,
                _ => {
                    let extension = name.rsplit_once('.')?.1.to_ascii_lowercase();
                    if !BOOK_EXTENSIONS.contains(&extension.as_str()) {
                        return None;
                    }
                    false
                }
            };
            // 名前の "/" はフォルダの区切りと区別がつかないので置き換える
            let mut parts: Vec<String> = folders.iter().map(|n| n.replace('/', "_")).collect();
            parts.push(name.replace('/', "_"));
            Some(Book {
                id,
                file_name: parts.join("/"),
                title: name,
                folder,
                cover_data: None,
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

fn download(request: &Request, id: &str, dest: &Path) -> Result<(), Failure> {
    if !valid_id(id) {
        return Err(Failure::Other(format!("本の id が正しくありません: {id}")));
    }
    let agent = agent();
    let token = access_token(&agent, request)?.access;
    let mut attempt = 0;
    let response = loop {
        let mut response = agent
            .get(format!("{API_URL}/files/{id}?alt=media"))
            .header("Authorization", format!("Bearer {token}"))
            .call()?;
        let status = response.status().as_u16();
        if status == 200 {
            break response;
        }
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if rate_limited(status, &text) && attempt < RETRIES {
            sleep(Duration::from_secs(1 << attempt));
            attempt += 1;
            continue;
        }
        let message = format!("ダウンロードできませんでした（{}）", error_summary(&text));
        return Err(if status == 401 {
            Failure::Auth(message)
        } else {
            Failure::Other(message)
        });
    };
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok()?.parse().ok());
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
        .and_then(|received| Ok((place(&partial, dest)?, received)));
    match result {
        Ok(((), received)) => progress(received),
        Err(error) => {
            let _ = std::fs::remove_file(&partial);
            Err(error)
        }
    }
}

/// 保存先にすでにファイルがあったときの知らせ
const DEST_EXISTS: &str = "保存先に同じ名前のファイルがあるので、置き換えずにやめました";

/// 一時ファイルを `dest` に置く。`dest` がすでにあれば置き換えずに失敗する
/// （ダウンロード中に同じ名前のファイルが置かれたときや、別に起動したアプリが同じ場所に置いたときに、それを消さないように）
fn place(partial: &Path, dest: &Path) -> Result<(), Failure> {
    // ハードリンクは、同じ名前があると作れないので、確かめるのと置くのが一度に済む
    match std::fs::hard_link(partial, dest) {
        Ok(()) => {
            let _ = std::fs::remove_file(partial);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            Err(Failure::Other(DEST_EXISTS.to_owned()))
        }
        Err(_) => copy_into_new(partial, dest),
    }
}

/// 保存先を「新しく作れたときだけ」開いて、一時ファイルの中身を写す。ハードリンクを作れないファイルシステム
/// （FAT など）で使う（名前を変える方法では、無いのを確かめたあとにできたファイルを置き換えてしまうので）
fn copy_into_new(partial: &Path, dest: &Path) -> Result<(), Failure> {
    let mut target = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
    {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            return Err(Failure::Other(DEST_EXISTS.to_owned()));
        }
        Err(e) => return Err(e.into()),
    };
    let copied = std::fs::File::open(partial)
        .and_then(|mut source| io::copy(&mut source, &mut target))
        .and_then(|_| target.sync_all());
    drop(target);
    match copied {
        Ok(()) => {
            let _ = std::fs::remove_file(partial);
            Ok(())
        }
        Err(e) => {
            // 自分で作った保存先なので、書きかけは消す
            let _ = std::fs::remove_file(dest);
            Err(e.into())
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

    /// ハードリンクを作れないときの写し方も、保存先にファイルがあれば置き換えない
    #[test]
    fn copying_into_place_never_replaces_a_file() {
        let dir = std::env::temp_dir().join(format!(
            "{}-copy-{}",
            env!("CARGO_PKG_NAME"),
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (partial, dest) = (dir.join("p.part"), dir.join("book.pdf"));
        std::fs::write(&partial, "new").unwrap();
        std::fs::write(&dest, "old").unwrap();
        assert!(copy_into_new(&partial, &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        std::fs::remove_file(&dest).unwrap();
        assert!(copy_into_new(&partial, &dest).is_ok());
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!partial.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

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

        // 保存先にすでにファイルがあれば、置き換えずに失敗し、一時ファイルも残さない
        assert!(save_book(&mut &b"new"[..], &dest, Some(3), |_| Ok(())).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
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
    }

    #[test]
    fn codes_are_taken_from_the_pasted_address_or_the_code_itself() {
        assert_eq!(
            code_from("http://127.0.0.1:47853/?code=4/0Ab%2Bc-d&scope=https://x"),
            Ok("4/0Ab+c-d".to_owned())
        );
        assert_eq!(code_from("4/0Abc"), Ok("4/0Abc".to_owned()));
        assert!(matches!(
            code_from("http://127.0.0.1:47853/?error=access_denied"),
            Err(Failure::Auth(message)) if message.contains("access_denied")
        ));
        assert_eq!(percent_decode("a%2Fb+c%"), "a/b c%");
    }

    #[test]
    fn files_become_folders_then_books_under_their_folders() {
        let files: Vec<Value> = serde_json::from_str(
            r#"[
            {"id": "b1", "name": "b.ZIP", "mimeType": "application/zip"},
            {"id": "m1", "name": "memo.txt", "mimeType": "text/plain"},
            {"id": "d1", "name": "doc.pdf", "mimeType": "application/vnd.google-apps.document"},
            {"id": "f1", "name": "Series", "mimeType": "application/vnd.google-apps.folder"},
            {"id": "a1", "name": "a/1.pdf", "mimeType": "application/pdf"}
        ]"#,
        )
        .unwrap();
        let book = |id: &str, title: &str, file_name: &str, folder| Book {
            id: id.to_owned(),
            title: title.to_owned(),
            file_name: file_name.to_owned(),
            folder,
            cover_data: None,
        };
        assert_eq!(
            books_from_files(&files, &["Comics".to_owned()]),
            [
                book("f1", "Series", "Comics/Series", true),
                book("a1", "a/1.pdf", "Comics/a_1.pdf", false),
                book("b1", "b.ZIP", "Comics/b.ZIP", false),
            ]
        );
    }

    #[test]
    fn thumbnail_links_ask_for_a_larger_size() {
        assert_eq!(
            thumbnail_size("https://lh3.example/abc=s220", 400),
            "https://lh3.example/abc=s400"
        );
        assert_eq!(
            thumbnail_size("https://example/abc", 400),
            "https://example/abc"
        );
        assert_eq!(
            thumbnail_size("https://example/a=sx", 400),
            "https://example/a=sx"
        );
    }

    #[test]
    fn cover_counts_say_why_covers_are_missing() {
        let count = |found, missing, failed| CoverCount {
            found,
            missing,
            failed,
        };
        assert_eq!(count(0, 0, 0).message(), None);
        assert_eq!(count(3, 0, 0).message().unwrap(), "表紙 3/3 冊");
        assert_eq!(
            count(36, 20, 4).message().unwrap(),
            "表紙 36/60 冊（Drive にサムネイルが無い 20 冊、取れなかった 4 冊）"
        );
        assert_eq!(
            count(1, 0, 2).message().unwrap(),
            "表紙 1/3 冊（取れなかった 2 冊）"
        );
    }

    #[test]
    fn only_plain_ids_are_accepted() {
        assert!(valid_id("1AbC-d_e"));
        assert!(!valid_id(""));
        assert!(!valid_id("x' or 'a"));
    }
}
