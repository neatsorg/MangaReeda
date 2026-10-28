//! MangaReeda の同梱プラグインで共通の部品。ダウンロードした本を、壊れたファイルを残さず、
//! ほかのファイルを置き換えずに保存する。

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

/// 本を保存できなかった理由
#[derive(Debug)]
pub enum SaveError {
    /// 保存先に同じ名前のファイルがある（置き換えない）
    Exists,
    /// 届いた大きさが、知らされた大きさと違う
    Truncated,
    /// この保存先では、置き換えずに名前を変える方法が無い
    Unsupported,
    Io(io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Exists => write!(
                f,
                "保存先に同じ名前のファイルがあるので、置き換えずにやめました"
            ),
            Self::Truncated => write!(f, "ダウンロードが途中で切れました"),
            Self::Unsupported => write!(
                f,
                "この保存先では、ほかのファイルを置き換えずに置けないので、やめました"
            ),
            Self::Io(error) => write!(f, "ファイルを書けませんでした: {error}"),
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 本を `dest` に書く。途中で止まったときに壊れたファイルが残らないよう、同じフォルダの一時ファイルに書いてから、
/// 置き換えずに名前を変える。一時ファイルは、新しく作れた名前だけを使う（あるファイルを切り詰めたり、
/// リンクの先に書いたりしない）。失敗したら一時ファイルを消す。`progress` には受け取った量を知らせる
pub fn save_book<E: From<SaveError>>(
    reader: &mut impl Read,
    dest: &Path,
    total: Option<u64>,
    mut progress: impl FnMut(u64) -> Result<(), E>,
) -> Result<(), E> {
    let (partial, file) = create_partial(dest).map_err(SaveError::from)?;
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

/// 保存先と同じフォルダに、ほかと重ならない一時ファイルを新しく作る
fn create_partial(dest: &Path) -> io::Result<(PathBuf, File)> {
    let name = dest.file_name().map_or_else(
        || "book".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    for serial in 0..100 {
        let partial = dest.with_file_name(format!(".{name}.{}-{serial}.part", std::process::id()));
        match OpenOptions::new()
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
fn receive<E: From<SaveError>>(
    reader: &mut impl Read,
    mut file: File,
    total: Option<u64>,
    progress: &mut impl FnMut(u64) -> Result<(), E>,
) -> Result<u64, E> {
    let mut buffer = vec![0; 256 * 1024];
    let (mut received, mut reported) = (0u64, 0u64);
    loop {
        let read = reader.read(&mut buffer).map_err(SaveError::from)?;
        if read == 0 {
            break;
        }
        file.write_all(&buffer[..read]).map_err(SaveError::from)?;
        received += read as u64;
        if received - reported >= 1024 * 1024 {
            reported = received;
            progress(received)?;
        }
    }
    file.sync_all().map_err(SaveError::from)?;
    if total.is_some_and(|total| total != received) {
        return Err(SaveError::Truncated.into());
    }
    Ok(received)
}

/// 書き終えた一時ファイルを `dest` に置く。`dest` がすでにあれば置き換えずに失敗する
/// （ダウンロード中に同じ名前のファイルが置かれたときや、別に起動したアプリが同じ場所に置いたときに、それを消さないように）。
/// 名前の変更は一度に済むので、途中で止まっても正式な名前で書きかけのファイルが残ることはない
pub fn place(partial: &Path, dest: &Path) -> Result<(), SaveError> {
    match rename_no_replace(partial, dest) {
        Ok(()) => return Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(SaveError::Exists),
        Err(e) if !unsupported(&e) => return Err(SaveError::Io(e)),
        Err(_) => {}
    }
    // 置き換えずに名前を変えられない保存先では、ハードリンクを使う（同じ名前があると作れない）
    match std::fs::hard_link(partial, dest) {
        Ok(()) => {
            let _ = std::fs::remove_file(partial);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(SaveError::Exists),
        // どちらも使えなければ、置き換えてしまうおそれのある方法は使わずにやめる
        Err(_) => Err(SaveError::Unsupported),
    }
}

/// その保存先（ファイルシステム）が、置き換えない名前の変更に対応していないことを示すエラーか
fn unsupported(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(
            error.raw_os_error(),
            Some(libc::EINVAL | libc::ENOSYS | libc::EOPNOTSUPP)
        )
    }
    #[cfg(not(unix))]
    {
        error.kind() == io::ErrorKind::Unsupported
    }
}

/// 同じ名前があれば失敗する、名前の変更
#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from = CString::new(from.as_os_str().as_bytes())?;
    let to = CString::new(to.as_os_str().as_bytes())?;
    let result = unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Windows の API に渡すパス。絶対パスにそろえ、長いパスの書き方（`\\?\`、ネットワークのパスは
/// `\\?\UNC\`）にする。ふつうの書き方では 260 文字までしか扱えないため
/// （Rust の標準のファイル操作は自分で長いパスを扱うので、書けたのに名前を変えられない、とならないように）
#[cfg(windows)]
fn verbatim_wide(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let full: Vec<u16> = std::path::absolute(path)?
        .as_os_str()
        .encode_wide()
        .collect();
    let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let mut out = if full.starts_with(&wide(r"\\?\")) {
        full
    } else if let Some(rest) = full.strip_prefix(wide(r"\\").as_slice()) {
        [wide(r"\\?\UNC\").as_slice(), rest].concat()
    } else {
        [wide(r"\\?\").as_slice(), &full].concat()
    };
    out.push(0);
    Ok(out)
}

#[cfg(windows)]
fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
    let (from, to) = (verbatim_wide(from)?, verbatim_wide(to)?);
    // 置き換えの指定（MOVEFILE_REPLACE_EXISTING）を付けないので、同じ名前があれば失敗する
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } != 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    windows
)))]
fn rename_no_replace(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mangareeda-common-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一時ファイルは新しい名前で作り、同じ名前のファイルがあっても切り詰めない。失敗したら消す
    #[test]
    fn books_are_written_without_touching_other_files() {
        let dir = temp("save");
        let dest = dir.join("book.pdf");
        let other = dir.join("book.part");
        std::fs::write(&other, "keep").unwrap();
        let mut reported = Vec::new();
        save_book::<SaveError>(&mut &b"data"[..], &dest, Some(4), |n| {
            reported.push(n);
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
        assert_eq!(std::fs::read(&other).unwrap(), b"keep");
        assert_eq!(reported, [4]);
        // 保存先にすでにファイルがあれば、置き換えずに失敗し、一時ファイルも残さない
        let result = save_book::<SaveError>(&mut &b"new"[..], &dest, Some(3), |_| Ok(()));
        assert!(matches!(result, Err(SaveError::Exists)));
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
        // 途中で切れたら、本も一時ファイルも残さない
        let cut = dir.join("cut.pdf");
        let result = save_book::<SaveError>(&mut &b"da"[..], &cut, Some(4), |_| Ok(()));
        assert!(matches!(result, Err(SaveError::Truncated)));
        assert!(!cut.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 置き換えない名前の変更は、同じ名前があれば失敗し、元のファイルを残す
    #[test]
    fn placing_never_replaces_a_file() {
        let dir = temp("place");
        let (partial, dest) = (dir.join("p.part"), dir.join("book.pdf"));
        std::fs::write(&partial, "new").unwrap();
        std::fs::write(&dest, "old").unwrap();
        assert!(matches!(place(&partial, &dest), Err(SaveError::Exists)));
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        assert!(partial.exists());
        std::fs::remove_file(&dest).unwrap();
        place(&partial, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        assert!(!partial.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Windows でも、260 文字を超える保存先に、置き換えずに置ける
    #[cfg(windows)]
    #[test]
    fn long_paths_can_be_placed_on_windows() {
        let dir = temp("long");
        let mut deep = dir.clone();
        for _ in 0..6 {
            deep.push("a".repeat(50));
        }
        std::fs::create_dir_all(&deep).unwrap();
        let dest = deep.join("book.pdf");
        assert!(dest.as_os_str().len() > 300);
        save_book::<SaveError>(&mut &b"data"[..], &dest, Some(4), |_| Ok(())).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
        let again = save_book::<SaveError>(&mut &b"new"[..], &dest, Some(3), |_| Ok(()));
        assert!(matches!(again, Err(SaveError::Exists)));
        assert_eq!(std::fs::read(&dest).unwrap(), b"data");
        // ネットワークのパスの書き方も変える
        let unc = verbatim_wide(Path::new(r"\\server\share\a.pdf")).unwrap();
        assert_eq!(
            String::from_utf16_lossy(&unc[..unc.len() - 1]),
            r"\\?\UNC\server\share\a.pdf"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
