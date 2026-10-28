//! MangaReeda の BOOKSCAN プラグイン。BOOKSCAN の本棚の一覧を返し、PDF をダウンロードする。
//!
//! 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
//!   mangareeda-bookscan list
//!   mangareeda-bookscan download <id> <保存先>
//! 標準入力に {"credentials": {"email": …, "password": …}} を1行で受け取る。
//!
//! サーバーに負担をかけないよう、ページの取得のあいだは間を空け、ダウンロードは1冊ずつ行う
//! （同時に何冊も落とさないのは本体の側で守る）。

use mangareeda_plugin_common::{SaveError, save_book};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;
use ureq::{Agent, ResponseExt};

const BASE_URL: &str = "https://system.bookscan.co.jp";
const LOGIN_PATH: &str = "/mypage/login.php";
const SHELF_PATH: &str = "/mypage/bookshelf_all_cover.php";
/// 本棚のページを続けて取るときの間隔
const PAGE_INTERVAL: Duration = Duration::from_secs(1);
/// 本棚のページ数の上限（ページ送りの読み違いで止まらなくならないように）
const MAX_PAGES: usize = 200;
/// HTML として読む大きさの上限
const HTML_LIMIT: u64 = 50 * 1024 * 1024;

/// 終了コード: ログインできなかった（本体はログイン情報の入力を促す）
const EXIT_AUTH: u8 = 2;
const EXIT_ERROR: u8 = 1;

#[derive(Deserialize, Default)]
struct Request {
    #[serde(default)]
    credentials: HashMap<String, String>,
}

#[derive(Serialize, Debug, PartialEq)]
struct Book {
    /// 詳細ページへの相対 URL。ダウンロードのときにこれで詳細ページを開く
    id: String,
    title: String,
    file_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cover_url: Option<String>,
}

enum Failure {
    Auth(String),
    Other(String),
}

impl From<ureq::Error> for Failure {
    fn from(error: ureq::Error) -> Self {
        Failure::Other(format!("通信に失敗しました: {error}"))
    }
}

impl From<SaveError> for Failure {
    fn from(error: SaveError) -> Self {
        Failure::Other(error.to_string())
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Failure::Other(format!("ファイルを書けませんでした: {error}"))
    }
}

const USAGE: &str = "使い方: mangareeda-bookscan list | download <id> <保存先>\n\
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
        ["list"] => list(&request),
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
        .user_agent(concat!("MangaReeda-bookscan/", env!("CARGO_PKG_VERSION")))
        // つながらない・答えが来ないときに、いつまでも待たない（本文の受け取りは本体が見張る）
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .build()
        .into()
}

fn get_html(agent: &Agent, url: &str) -> Result<String, Failure> {
    let mut response = agent.get(url).call()?;
    Ok(response
        .body_mut()
        .with_config()
        .limit(HTML_LIMIT)
        .read_to_string()?)
}

/// ログインする。ログイン画面のフォームの項目（隠し項目を含む）にメールアドレスとパスワードを入れて送る
fn login(agent: &Agent, request: &Request) -> Result<(), Failure> {
    let credential = |key: &str| {
        request
            .credentials
            .get(key)
            .filter(|value| !value.is_empty())
            .cloned()
            .ok_or_else(|| Failure::Auth("メールアドレスとパスワードを設定してください".to_owned()))
    };
    let (email, password) = (credential("email")?, credential("password")?);
    let page = get_html(agent, &format!("{BASE_URL}{LOGIN_PATH}"))?;
    let mut fields = login_form_fields(&page);
    for (name, value) in fields.iter_mut() {
        match name.as_str() {
            "email" => *value = email.clone(),
            "password" => *value = password.clone(),
            _ => {}
        }
    }
    let response = agent
        .post(format!("{BASE_URL}{LOGIN_PATH}"))
        .send_form(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())))?;
    // 失敗するとログイン画面のまま
    if response.get_uri().path().ends_with("login.php") {
        return Err(Failure::Auth(
            "ログインできませんでした。メールアドレスとパスワードを確かめてください".to_owned(),
        ));
    }
    Ok(())
}

fn list(request: &Request) -> Result<(), Failure> {
    let agent = agent();
    login(&agent, request)?;
    let mut books = Vec::new();
    for page in 1..=MAX_PAGES {
        if page > 1 {
            sleep(PAGE_INTERVAL);
        }
        let html = get_html(
            &agent,
            &format!("{BASE_URL}{SHELF_PATH}?q=&sort=s&page={page}"),
        )?;
        let (found, has_next) = parse_shelf(&html, page);
        books.extend(found);
        if !has_next {
            break;
        }
    }
    // アカウントを見分ける値（本体は、これから作った短い値だけを手元に置く）
    let account = request
        .credentials
        .get("email")
        .map(|email| email.trim().to_lowercase());
    let json = serde_json::json!({ "books": books, "account": account });
    let mut stdout = io::stdout().lock();
    writeln!(stdout, "{json}")?;
    Ok(())
}

fn download(request: &Request, id: &str, dest: &Path) -> Result<(), Failure> {
    if !id.starts_with("/mypage/showbook.php?") {
        return Err(Failure::Other(format!("知らない本の id です: {id}")));
    }
    let agent = agent();
    login(&agent, request)?;
    let detail = get_html(&agent, &format!("{BASE_URL}{id}"))?;
    let link = download_link(&detail)
        .ok_or_else(|| Failure::Other("ダウンロードのリンクが見つかりません".to_owned()))?;
    let response = agent.get(format!("{BASE_URL}{link}")).call()?;
    let total: Option<u64> = response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok()?.parse().ok());
    let mut reader = response.into_body().into_reader();
    save_book(&mut reader, dest, total, |received| {
        let mut stdout = io::stdout().lock();
        writeln!(
            stdout,
            "{}",
            serde_json::json!({ "progress": received, "total": total })
        )?;
        Ok(())
    })
}

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("セレクタが正しい")
}

/// ログイン画面のフォームの項目（名前と既定の値）
fn login_form_fields(html: &str) -> Vec<(String, String)> {
    let document = Html::parse_document(html);
    let Some(form) = document.select(&selector("form")).find(|form| {
        form.select(&selector("input[name=password]"))
            .next()
            .is_some()
    }) else {
        // 見つからなければ、知っている項目だけで送る
        return ["email", "password", "push", "url"]
            .into_iter()
            .map(|name| (name.to_owned(), String::new()))
            .collect();
    };
    form.select(&selector("input[name]"))
        .filter(|input| input.value().attr("type") != Some("submit"))
        .map(|input| {
            let value = input.value();
            (
                value.attr("name").unwrap_or_default().to_owned(),
                value.attr("value").unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// 本棚の1ページから本を読み取る。次のページがあるかも返す
fn parse_shelf(html: &str, page: usize) -> (Vec<Book>, bool) {
    let document = Html::parse_document(html);
    let books = document
        .select(&selector("li.sortable_item"))
        .filter_map(|item| {
            let link = item.select(&selector("a[href*='showbook.php']")).next()?;
            let id = link.value().attr("href")?.to_owned();
            let image = item.select(&selector("img")).next();
            let attr = |name| {
                image
                    .and_then(|image| image.value().attr(name))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            };
            let file_name = item
                .select(&selector("input.order_ids"))
                .next()
                .and_then(|input| decode_hex(input.value().attr("value")?))
                .unwrap_or_default();
            let title = attr("alt")
                .or_else(|| attr("title"))
                .unwrap_or_else(|| file_name.trim_end_matches(".pdf").to_owned());
            let cover_url = attr("data-original")
                .or_else(|| attr("data-amazon-path"))
                .filter(|url| url.starts_with("https://"));
            Some(Book {
                id,
                title,
                file_name,
                cover_url,
            })
        })
        .collect();
    let next = format!("page={}", page + 1);
    let has_next = document.select(&selector("a[href]")).any(|a| {
        a.value()
            .attr("href")
            .is_some_and(|href| href.contains(&next))
    });
    (books, has_next)
}

/// 詳細ページのダウンロードのリンク
fn download_link(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    document
        .select(&selector("a[href^='/download.php']"))
        .next()
        .and_then(|a| a.value().attr("href"))
        .map(str::to_owned)
}

/// 16進で書いた UTF-8 の文字列を戻す
fn decode_hex(hex: &str) -> Option<String> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 実際のページの作りをまねた、架空の本の HTML
    const SHELF: &str = r#"<html><body>
<ul id="sortable">
<li class="sortable_item shadow">
  <input type="hidden" name="order_ids[]" class="order_ids" value="e381a6e38199e381a820e69cac20313233702e706466">
  <input type="hidden" name="book_ids[]" class="book_ids" value="1">
  <a class="fancybox" href="/mypage/showbook.php?h=aa&amp;d=bb&amp;f=x.pdf&amp;bid=1">
    <img src="https://example.invalid/a.jpg" data-original="https://example.invalid/a.jpg" alt="てすと 本" title="てすと 本" class="lazy">
  </a>
</li>
<li class="sortable_item shadow">
  <input type="hidden" name="order_ids[]" class="order_ids" value="6e6f2d636f7665722e706466">
  <a class="fancybox" href="/mypage/showbook.php?h=cc&amp;d=dd&amp;f=y.pdf&amp;bid=2">
    <img src="/mypage/assets/img/common/loading.gif" alt="">
  </a>
</li>
</ul>
<a href="?&amp;page=2">2</a>
</body></html>"#;

    #[test]
    fn shelf_pages_give_books_and_tell_if_more_follow() {
        let (books, has_next) = parse_shelf(SHELF, 1);
        assert!(has_next);
        assert_eq!(
            books,
            [
                Book {
                    id: "/mypage/showbook.php?h=aa&d=bb&f=x.pdf&bid=1".to_owned(),
                    title: "てすと 本".to_owned(),
                    file_name: "てすと 本 123p.pdf".to_owned(),
                    cover_url: Some("https://example.invalid/a.jpg".to_owned()),
                },
                // 名前が無ければファイル名から、表紙が https でなければ無し
                Book {
                    id: "/mypage/showbook.php?h=cc&d=dd&f=y.pdf&bid=2".to_owned(),
                    title: "no-cover".to_owned(),
                    file_name: "no-cover.pdf".to_owned(),
                    cover_url: None,
                },
            ]
        );
        let (_, has_next) = parse_shelf(SHELF, 2);
        assert!(!has_next);
    }

    #[test]
    fn login_form_keeps_hidden_fields() {
        let html = r#"<form method="post"><input type="hidden" name="push" value="1">
            <input type="text" name="email"><input type="password" name="password">
            <input type="hidden" name="url" value=""><input type="submit" value="ログイン" id="login-btn"></form>"#;
        assert_eq!(
            login_form_fields(html),
            [
                ("push".to_owned(), "1".to_owned()),
                ("email".to_owned(), String::new()),
                ("password".to_owned(), String::new()),
                ("url".to_owned(), String::new()),
            ]
        );
    }

    #[test]
    fn detail_pages_give_the_download_link() {
        let html = r#"<a href="javascript:void(0)">今すぐ読む</a>
            <a href="/download.php?d=bb&amp;f=%E3%81%82.pdf" onclick="post_log('download', 'x')">ダウンロード</a>"#;
        assert_eq!(
            download_link(html).as_deref(),
            Some("/download.php?d=bb&f=%E3%81%82.pdf")
        );
        assert_eq!(download_link("<a href='/other'>x</a>"), None);
    }

    #[test]
    fn hex_names_decode_as_utf8() {
        assert_eq!(decode_hex("e3818220").as_deref(), Some("あ "));
        assert_eq!(decode_hex("e381"), None);
        assert_eq!(decode_hex("zz"), None);
        assert_eq!(decode_hex("abc"), None);
    }
}
