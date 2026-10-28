//! ブックリストのプラグイン。プラグインは外部のプログラムで、一覧（list）とダウンロード（download）、
//! 必要なら接続（auth-url）の命令に答える。ログイン情報は標準入力に JSON で渡し、答えは標準出力に
//! JSON で受け取る。答えに "store" があれば、その値をログイン情報として保存する（トークンなど）。
//! 取り決めの詳細は docs/plugins.md。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 終了コード: ログインできなかった
const EXIT_AUTH: i32 = 2;
/// 一覧を返すまで待つ時間。これを過ぎたら止める
const LIST_LIMIT: Duration = Duration::from_secs(10 * 60);
/// 許可のページの URL を返すまで待つ時間
const AUTH_URL_LIMIT: Duration = Duration::from_secs(30);
/// ダウンロードの進み具合が届かないまま待つ時間
const DOWNLOAD_IDLE_LIMIT: Duration = Duration::from_secs(3 * 60);
/// 見張りが確かめる間隔
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
/// プラグインが終わったあと、残りの出力を待つ時間
const DRAIN_LIMIT: Duration = Duration::from_secs(2);

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
    /// 設定画面に出さない（プラグインが "store" で保存する値）
    #[serde(default)]
    pub hidden: bool,
}

impl CredentialField {
    fn new(key: &str, secret: bool, hidden: bool) -> Self {
        Self {
            key: key.to_owned(),
            label: None,
            secret,
            hidden,
        }
    }
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
    /// ブラウザで許可する接続（auth-url）に答えるか
    #[serde(default)]
    pub auth: bool,
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
    /// フォルダなら真。`id` を `list` に渡すと中を返す
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub folder: bool,
}

/// `list` の答え
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Listing {
    pub books: Vec<RemoteBook>,
    /// 上のフォルダ。一番上なら None
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// いま使っているアカウントを見分ける値。本体は、この値から作った短い値だけを手元に置く
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

/// プラグインが保存してほしい値。空の値は消す
pub type Store = HashMap<String, String>;

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
    Store(Store),
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
    for manifest in bundled(exe_dir) {
        let program = Path::new(&manifest.command[0]);
        if program.is_file() && !found.iter().any(|m| m.id == manifest.id) {
            found.push(manifest);
        }
    }
    found
}

/// 本体と同じ場所に置いて使う、同梱のプラグイン
fn bundled(exe_dir: &Path) -> Vec<PluginManifest> {
    let program = |name: &str| {
        let path = exe_dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        vec![path.to_string_lossy().into_owned()]
    };
    vec![
        PluginManifest {
            id: "bookscan".to_owned(),
            name: "BOOKSCAN".to_owned(),
            command: program("mangareeda-bookscan"),
            credentials: vec![
                CredentialField::new("email", false, false),
                CredentialField::new("password", true, false),
            ],
            auth: false,
            dir: exe_dir.to_path_buf(),
        },
        PluginManifest {
            id: "dropbox".to_owned(),
            name: "Dropbox".to_owned(),
            command: program("mangareeda-dropbox"),
            credentials: vec![
                CredentialField::new("app_key", false, false),
                CredentialField::new("auth_code", true, false),
                CredentialField::new("code_verifier", true, true),
                CredentialField::new("refresh_token", true, true),
                CredentialField::new("account_id", false, true),
            ],
            auth: true,
            dir: exe_dir.to_path_buf(),
        },
    ]
}

/// 標準出力の行から、"store" の値を集める
fn collect_store(stdout: &str) -> Store {
    #[derive(Deserialize)]
    struct StoreLine {
        store: Store,
    }
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<StoreLine>(line).ok())
        .flat_map(|line| line.store)
        .collect()
}

/// プラグインと、その子孫のプロセスをまとめて止めるための組
/// （シェルから起動したプログラムなどが残って、出力を握ったままにならないように）
#[cfg(unix)]
mod group {
    use std::process::{Child, Command};

    /// プロセスグループ。グループの番号は、最初のプロセスの番号
    pub struct Group(libc::pid_t);

    pub fn configure(command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    impl Group {
        pub fn new(child: &Child) -> Option<Self> {
            libc::pid_t::try_from(child.id()).ok().map(Self)
        }

        /// 終わったか。終わりを受け取らずに見る（受け取るまでは番号が別のプロセスに使われないので、
        /// そのあとグループを止めても関係のないプロセスを止めない）
        pub fn exited(&self, _child: &mut Child) -> bool {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let found = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.0 as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            found == 0 && unsafe { info.si_pid() } != 0
        }

        /// グループのプロセスをすべて止める。最初のプロセスの終わりを受け取る前に呼ぶ
        pub fn kill(&self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
mod group {
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    /// ジョブオブジェクト。閉じると中のプロセスもすべて止まる
    pub struct Group(HANDLE);

    // ハンドルは見張りのスレッドだけが使う
    unsafe impl Send for Group {}

    pub fn configure(_command: &mut Command) {}

    impl Group {
        /// 起動したプロセスをジョブに入れる。ログイン情報を渡す前に入れるので、
        /// 取り決めどおり標準入力を読んでから働くプラグインの子孫は、すべてジョブに入る
        pub fn new(child: &Child) -> Option<Self> {
            unsafe {
                let job = CreateJobObjectW(None, None).ok()?;
                let group = Self(job);
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .ok()?;
                AssignProcessToJobObject(job, HANDLE(child.as_raw_handle())).ok()?;
                Some(group)
            }
        }

        pub fn exited(&self, child: &mut Child) -> bool {
            !matches!(child.try_wait(), Ok(None))
        }

        pub fn kill(&self) {
            unsafe {
                let _ = TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod group {
    use std::process::{Child, Command};

    pub struct Group;

    pub fn configure(_command: &mut Command) {}

    impl Group {
        pub fn new(_child: &Child) -> Option<Self> {
            Some(Self)
        }

        pub fn exited(&self, child: &mut Child) -> bool {
            !matches!(child.try_wait(), Ok(None))
        }

        pub fn kill(&self) {}
    }
}

/// 起動する。書き込んだばかりのプログラムは、ほかのスレッドがプロセスを起動している途中だと
/// しばらく実行できない（Text file busy）ので、少し待ってやり直す
fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    for _ in 0..20 {
        match command.spawn() {
            #[cfg(unix)]
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                thread::sleep(Duration::from_millis(50));
            }
            result => return result,
        }
    }
    command.spawn()
}

/// 見張りのスレッドと分け合う状態
struct Watch {
    /// 最後に動きがあったとき（ダウンロードでは行が届くたびに新しくする）
    last: Mutex<Instant>,
    /// 止めてほしい（受け手がいなくなったときなど）
    stop: AtomicBool,
    timed_out: AtomicBool,
    /// 終わったときと、終わり方
    ended: Mutex<Option<(Instant, Result<ExitStatus, String>)>>,
}

impl Watch {
    fn ended_for(&self) -> Option<Duration> {
        self.ended.lock().ok()?.as_ref().map(|(at, _)| at.elapsed())
    }
}

/// 走っているプラグイン。見張りのスレッドが、決めた時間のあいだ動きが無ければ、子孫ごと止める。
/// プラグインが終わったら、残った子孫も止める
struct Running {
    watch: Arc<Watch>,
    /// 標準出力の行
    lines: Receiver<String>,
    /// 標準エラー。読み終えたら `stderr_done` が切れる
    stderr: Arc<Mutex<String>>,
    stderr_done: Receiver<()>,
}

impl Running {
    /// プラグインを起動し、ログイン情報を渡して、見張りを始める
    fn start(
        manifest: &PluginManifest,
        credentials: &HashMap<String, String>,
        args: &[&str],
        limit: Duration,
    ) -> Result<Self, PluginError> {
        let mut command = manifest
            .command(args)
            .ok_or_else(|| PluginError::Other("プラグインの命令がありません".to_owned()))?;
        group::configure(&mut command);
        let mut child = spawn(&mut command)
            .map_err(|e| PluginError::Other(format!("プラグインを起動できませんでした: {e}")))?;
        let group = group::Group::new(&child);
        let streams = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let (Some(group), (Some(mut stdin), Some(stdout), Some(stderr))) = (group, streams) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginError::Other(
                "プラグインとつながりませんでした".to_owned(),
            ));
        };
        let _ = writeln!(stdin, "{}", request_line(credentials));
        drop(stdin);

        let (line_tx, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_text = Arc::new(Mutex::new(String::new()));
        let (stderr_tx, stderr_done) = channel();
        let text = stderr_text.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut text) = text.lock() {
                    text.push_str(&line);
                    text.push('\n');
                }
            }
            drop(stderr_tx);
        });

        let watch = Arc::new(Watch {
            last: Mutex::new(Instant::now()),
            stop: AtomicBool::new(false),
            timed_out: AtomicBool::new(false),
            ended: Mutex::new(None),
        });
        let shared = watch.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(WATCH_INTERVAL);
                let idle = shared
                    .last
                    .lock()
                    .map_or(Duration::ZERO, |last| last.elapsed());
                let stop = shared.stop.load(Ordering::Relaxed);
                let exited = group.exited(&mut child);
                if !exited && !stop && idle <= limit {
                    continue;
                }
                if !exited && !stop {
                    shared.timed_out.store(true, Ordering::Relaxed);
                }
                // 終わっていても、残った子孫を止める
                group.kill();
                let _ = child.kill();
                let status = child.wait().map_err(|e| e.to_string());
                if let Ok(mut ended) = shared.ended.lock() {
                    *ended = Some((Instant::now(), status));
                }
                return;
            }
        });
        Ok(Self {
            watch,
            lines,
            stderr: stderr_text,
            stderr_done,
        })
    }

    /// 標準出力の次の行。出力が閉じたか、プラグインが終わってしばらく経ったら None
    /// （止めきれなかったプロセスが出力を握ったままでも待ち続けないように）
    fn next_line(&self) -> Option<String> {
        loop {
            match self.lines.recv_timeout(WATCH_INTERVAL) {
                Ok(line) => return Some(line),
                Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => {
                    if self
                        .watch
                        .ended_for()
                        .is_some_and(|ended| ended > DRAIN_LIMIT)
                    {
                        return None;
                    }
                }
            }
        }
    }

    /// 動きがあった
    fn touch(&self) {
        if let Ok(mut last) = self.watch.last.lock() {
            *last = Instant::now();
        }
    }

    /// 止める（受け手がいなくなったときなど）
    fn stop(&self) {
        self.watch.stop.store(true, Ordering::Relaxed);
    }

    /// 終わるのを待つ。見張りが、終わったか止めたかを知らせるまで待つ（時間の上限があるので、待ち続けない）
    fn finish(self) -> Result<(), PluginError> {
        let status = loop {
            if let Ok(mut ended) = self.watch.ended.lock()
                && let Some((_, status)) = ended.take()
            {
                break status;
            }
            thread::sleep(WATCH_INTERVAL / 5);
        };
        let _ = self.stderr_done.recv_timeout(DRAIN_LIMIT);
        let stderr = self
            .stderr
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        if self.watch.timed_out.load(Ordering::Relaxed) {
            return Err(PluginError::Other(
                "プラグインから答えが無いので止めました".to_owned(),
            ));
        }
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(failure(status.code(), &stderr)),
            Err(e) => Err(PluginError::Other(e)),
        }
    }
}

/// プラグインを走らせて、終わるまで待つ。(標準出力, 保存してほしい値) を返す。
/// `limit` を過ぎても終わらなければ止める。
/// 失敗しても保存してほしい値は返す（トークンを取り替えてから失敗したときなど）
fn run(
    manifest: &PluginManifest,
    credentials: &HashMap<String, String>,
    args: &[&str],
    limit: Duration,
) -> (Result<String, PluginError>, Store) {
    let running = match Running::start(manifest, credentials, args, limit) {
        Ok(running) => running,
        Err(error) => return (Err(error), Store::new()),
    };
    let mut output = String::new();
    while let Some(line) = running.next_line() {
        output.push_str(&line);
        output.push('\n');
    }
    let store = collect_store(&output);
    match running.finish() {
        Ok(()) => (Ok(output), store),
        Err(error) => (Err(error), store),
    }
}

/// 標準出力の最後の、`T` として読める行
fn last_reply<T: for<'de> Deserialize<'de>>(stdout: &str) -> Result<T, PluginError> {
    stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str(line).ok())
        .ok_or_else(|| PluginError::Other("プラグインの答えを読めませんでした".to_owned()))
}

/// ブラウザで許可するページの URL を作ってもらう（auth-url）
pub fn auth_url(
    manifest: &PluginManifest,
    credentials: &HashMap<String, String>,
) -> (Result<String, PluginError>, Store) {
    #[derive(Deserialize)]
    struct UrlReply {
        url: String,
    }
    let (stdout, store) = run(manifest, credentials, &["auth-url"], AUTH_URL_LIMIT);
    let url = stdout.and_then(|stdout| last_reply::<UrlReply>(&stdout).map(|reply| reply.url));
    (url, store)
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

/// 一覧を取る。`folder` が Some ならそのフォルダの中。終わるまで待つので、別スレッドで呼ぶ
pub fn list(
    manifest: &PluginManifest,
    credentials: &HashMap<String, String>,
    folder: Option<&str>,
) -> (Result<Listing, PluginError>, Store) {
    let mut args = vec!["list"];
    args.extend(folder);
    let (stdout, store) = run(manifest, credentials, &args, LIST_LIMIT);
    (stdout.and_then(|stdout| last_reply(&stdout)), store)
}

/// `list` を別スレッドで行う
pub fn spawn_list(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
    folder: Option<String>,
) -> Receiver<(Result<Listing, PluginError>, Store)> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let _ = tx.send(list(&manifest, &credentials, folder.as_deref()));
    });
    rx
}

#[derive(Deserialize)]
struct ProgressLine {
    progress: u64,
    #[serde(default)]
    total: Option<u64>,
}

/// 本を `dest` にダウンロードする。進み具合と結果を送る。
/// 進み具合が `DOWNLOAD_IDLE_LIMIT` のあいだ届かなければ止めて、失敗として知らせる
pub fn spawn_download(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
    id: String,
    dest: PathBuf,
) -> Receiver<DownloadEvent> {
    spawn_download_within(manifest, credentials, id, dest, DOWNLOAD_IDLE_LIMIT)
}

fn spawn_download_within(
    manifest: PluginManifest,
    credentials: HashMap<String, String>,
    id: String,
    dest: PathBuf,
    idle_limit: Duration,
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
        let args = ["download", id.as_str(), dest_text.as_str()];
        let running = match Running::start(&manifest, &credentials, &args, idle_limit) {
            Ok(running) => running,
            Err(error) => return done(Err(error)),
        };
        while let Some(line) = running.next_line() {
            running.touch();
            let store = collect_store(&line);
            if !store.is_empty() {
                let _ = tx.send(DownloadEvent::Store(store));
            }
            if let Ok(progress) = serde_json::from_str::<ProgressLine>(&line) {
                let event = DownloadEvent::Progress {
                    received: progress.progress,
                    total: progress.total,
                };
                if tx.send(event).is_err() {
                    // 受け手がいなくなった（アプリを閉じた）ら止める
                    running.stop();
                    break;
                }
            }
        }
        let result = running.finish().and_then(|()| {
            if dest.is_file() {
                Ok(())
            } else {
                Err(PluginError::Other(
                    "プラグインが本を書きませんでした".to_owned(),
                ))
            }
        });
        done(result);
    });
    rx
}

/// ダウンロードした本を置く場所。`file_name` は "/" で区切ってフォルダを含められる
/// （クラウドのフォルダの並びをそのまま保つため）。使えない文字は置き換え、
/// ".." などでライブラリの外に出られないようにする
pub fn local_path(library: &Path, plugin_id: &str, file_name: &str) -> PathBuf {
    let mut path = library.join(plugin_id);
    let mut pushed = false;
    for part in file_name.split('/') {
        let safe: String = part
            .chars()
            .map(|c| match c {
                '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                c if c.is_control() => '_',
                c => c,
            })
            .collect();
        let safe = safe.trim().trim_start_matches('.');
        if !safe.is_empty() {
            path.push(safe);
            pushed = true;
        }
    }
    if !pushed {
        path.push("book");
    }
    path
}

/// 一覧を手元にためておくファイル
#[derive(Serialize, Deserialize, Default)]
pub struct ListCache {
    pub books: Vec<RemoteBook>,
    #[serde(default)]
    pub parent: Option<String>,
    /// 一覧を取ったアカウント（`Listing::account` から作った値）
    #[serde(default)]
    pub account: Option<String>,
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
            let _ = crate::engine::settings_store::write_atomic(path, json.as_bytes());
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
        // 置いていない同梱のプラグイン（Dropbox）は入らない
        assert_eq!(found.len(), 2);
        assert_eq!(discover(&plugins, None).len(), 1);
        std::fs::remove_dir_all(plugins).unwrap();
        std::fs::remove_dir_all(exe).unwrap();
    }

    #[test]
    fn file_names_cannot_leave_the_library() {
        let library = Path::new("/lib");
        assert_eq!(
            local_path(library, "p", "a/b:c.pdf"),
            Path::new("/lib/p/a/b_c.pdf")
        );
        assert_eq!(
            local_path(library, "p", "../x.pdf"),
            Path::new("/lib/p/x.pdf")
        );
        assert_eq!(
            local_path(library, "p", "/a/../../b\\c.zip"),
            Path::new("/lib/p/a/b_c.zip")
        );
        assert_eq!(local_path(library, "p", ".."), Path::new("/lib/p/book"));
    }

    #[cfg(unix)]
    fn script_plugin(dir: &Path, body: &str) -> PluginManifest {
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("run.sh");
        std::fs::write(&script, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        PluginManifest {
            id: "s".to_owned(),
            name: "S".to_owned(),
            command: vec!["./run.sh".to_owned()],
            credentials: Vec::new(),
            auth: false,
            dir: dir.to_path_buf(),
        }
    }

    /// 答えを返さなくなったプラグインは、出力を握った子孫ごと止めて、失敗として知らせる
    #[cfg(unix)]
    #[test]
    fn stalled_plugins_are_stopped() {
        let dir = temp("stall");
        let limit = Duration::from_millis(300);
        // exec しないので、sleep がシェルの子として標準出力を握ったまま残る
        let stalled = script_plugin(&dir, "read request\necho '{\"progress\": 1}'\nsleep 30\n");
        let started = Instant::now();
        let (result, _) = run(&stalled, &HashMap::new(), &["list"], limit);
        assert!(matches!(result, Err(PluginError::Other(_))));
        // 子孫も止まって出力が閉じるので、残りの出力を待つ時間（DRAIN_LIMIT）より早く終わる
        assert!(started.elapsed() < DRAIN_LIMIT);

        let events: Vec<_> = spawn_download_within(
            stalled,
            HashMap::new(),
            "1".to_owned(),
            dir.join("out.zip"),
            limit,
        )
        .iter()
        .collect();
        assert!(matches!(
            events.last(),
            Some(DownloadEvent::Done(Err(PluginError::Other(_))))
        ));
        assert!(started.elapsed() < Duration::from_secs(10));

        // 出力を閉じてから止まっても、待ち続けない
        let silent = script_plugin(&dir, "read request\nexec >&- 2>&-\nsleep 30\n");
        let started = Instant::now();
        assert!(run(&silent, &HashMap::new(), &["list"], limit).0.is_err());
        assert!(started.elapsed() < Duration::from_secs(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 答えを返して終わったプラグインの子孫が出力を握っていても、待ち続けない（残った子孫は止める）
    #[cfg(unix)]
    #[test]
    fn finished_plugins_do_not_wait_for_their_children() {
        let dir = temp("children");
        let plugin = script_plugin(
            &dir,
            "read request\nsleep 30 &\necho '{\"books\": []}'\nexit 0\n",
        );
        let started = Instant::now();
        let (listing, _) = list(&plugin, &HashMap::new(), None);
        assert_eq!(listing, Ok(Listing::default()));
        assert!(started.elapsed() < DRAIN_LIMIT);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Windows でも、子孫ごと止める（ジョブオブジェクト）。子孫が残っていると出力が閉じず、
    /// 残りの出力を待つ時間（DRAIN_LIMIT）まで終わらない
    #[cfg(windows)]
    #[test]
    fn stalled_plugins_are_stopped_with_their_children_on_windows() {
        let plugin = |script: &str| PluginManifest {
            id: "s".to_owned(),
            name: "S".to_owned(),
            command: vec![
                "cmd".to_owned(),
                "/d".to_owned(),
                "/c".to_owned(),
                script.to_owned(),
            ],
            credentials: Vec::new(),
            auth: false,
            dir: std::env::temp_dir(),
        };
        // cmd の子の ping が、標準出力を握ったまま 30 秒動く
        let stalled = plugin("ping -n 30 127.0.0.1");
        let started = Instant::now();
        let (result, _) = run(&stalled, &HashMap::new(), &[], Duration::from_millis(300));
        assert!(matches!(result, Err(PluginError::Other(_))));
        assert!(started.elapsed() < DRAIN_LIMIT, "{:?}", started.elapsed());

        // 答えを返して終わっても、裏で動かした ping が残っていれば止める
        // （cmd の引数では " を渡せないので、バッチファイルに書く）
        let dir = temp("children-windows");
        let batch = dir.join("run.cmd");
        std::fs::write(
            &batch,
            "@echo off\r\nstart /b ping -n 30 127.0.0.1\r\necho {\"books\": []}\r\n",
        )
        .unwrap();
        let finished = plugin(&batch.to_string_lossy());
        let started = Instant::now();
        let (listing, _) = list(&finished, &HashMap::new(), None);
        assert_eq!(listing, Ok(Listing::default()));
        assert!(started.elapsed() < DRAIN_LIMIT, "{:?}", started.elapsed());
        std::fs::remove_dir_all(dir).unwrap();
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
list)
  echo '{"store": {"refresh_token": "new"}}'
  if [ "$2" = "sub" ]; then echo '{"books": [], "parent": ""}'; exit 0; fi
  echo '{"books": [{"id": "sub", "title": "Sub", "file_name": "sub", "folder": true}, {"id": "1", "title": "T", "file_name": "t.cbz", "cover_url": "https://example.invalid/c.jpg"}]}';;
download) echo '{"progress": 5, "total": 10}'; printf data > "$3"; echo '{"progress": 10, "total": 10}';;
auth-url) echo '{"store": {"code_verifier": "v"}}'; echo '{"url": "https://example.invalid/auth"}';;
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
            auth: true,
            dir: dir.clone(),
        };
        let good = HashMap::from([("token".to_owned(), "good".to_owned())]);
        let (listing, store) = list(&manifest, &good, None);
        let listing = listing.unwrap();
        assert!(listing.books[0].folder);
        assert_eq!(listing.books[1].file_name, "t.cbz");
        assert_eq!(listing.parent, None);
        // 保存してほしい値は、答えと一緒に返る
        assert_eq!(store.get("refresh_token").map(String::as_str), Some("new"));
        // フォルダの中は、上のフォルダも返る
        let (listing, _) = list(&manifest, &good, Some("sub"));
        assert_eq!(listing.unwrap().parent.as_deref(), Some(""));
        assert_eq!(
            list(&manifest, &HashMap::new(), None).0,
            Err(PluginError::Auth("bad token".to_owned()))
        );
        let (url, store) = auth_url(&manifest, &good);
        assert_eq!(url.as_deref(), Ok("https://example.invalid/auth"));
        assert_eq!(store.get("code_verifier").map(String::as_str), Some("v"));

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
