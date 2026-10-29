[English](#english)

# MangaReeda

LinuxとWindowsで動く、オフラインのマンガ・コミックビューアです。Rustで書かれています。Macでは検証していませんが移植できる可能性は高いです。
見開き表示、書庫を展開せずに読む、キー・マウス・ゲームパッドでの操作などができます。

かつての人気ソフト「マンガミーヤ」にインスパイアされて作りました。マンガミーヤのコードは含まず、マンガミーヤの作者とも関係ありません。

プラグイン形式でDropbox、GoogleDriveなどのクラウド連携やブックリストのインクリメンタル検索、OCRテキストの検索、OCR透明テキストのないPDF等に対してはOCRエンジンを利用してテキスト補完を行うなど、今日的な機能を多数搭載しました。

## 主な機能

- 単ページ、見開き（右から左／左から右）、縦スクロール（Web マンガ）表示
- 縦長のページが続くと見開きにする自動見開き、1枚目の前に白紙を入れる奇数ページ開始
- 横長のページを左右に分けて半分ずつ見せる自動単ページ表示
- ページのサムネイル一覧と、同じフォルダの冊を表紙で並べる表紙サムネイル
- 冊ごとに読んでいた位置を覚えるしおりと履歴
- 履歴とお気に入りを表紙で並べるブックリスト（表紙サムネイルの画面で Tab で切り替え）
- プラグインで本の一覧を足せるブックリスト。Dropbox・Google Driveのフォルダ、そしてBOOKSCAN の本棚を表示できるプラグインを同梱（[docs/plugins.md](docs/plugins.md)）
- 本の中の検索（Ctrl+F）。PDF の透明テキストを探し、文字の層の無い本は OCR のプラグインで読んでから探す（Windows は OS 標準の高性能な OCRを使います。Linux は Tesseract のプラグインを同梱）。マンガ向けに [manga-ocr のプラグイン](plugins/manga-ocr/README.md)も入れられます
- ルーペ、ズーム、縮小・拡大のフィルタ（最近傍〜Lanczos3）
- 書庫を展開せずに直接読む。サブフォルダの画像も読める
- 同じフォルダの前後の冊、隣のフォルダへの移動
- キーボード・マウス（クリック、ダブルクリック、長押し、ボタンの同時押し）・ゲームパッドの割り当てを自由に変えられる。マンガミーヤ風の割り当ても選べる
- 動く WebP・GIF・APNG の再生、自動ページ送り

## 対応形式

| 種類 | 形式 |
|---|---|
| 書庫 | `.zip` `.cbz` `.rar` `.cbr` |
| 文書 | `.pdf`（pdfium ライブラリが必要。下記参照） |
| フォルダ | フォルダ内の画像をそのまま読む |
| 画像 | `.png` `.jpg` `.jpeg` `.bmp` `.tiff` `.webp` `.avif` `.tga` `.gif` |

## リリース

ビルドしないひと用。[Releases](https://github.com/neatsorg/MangaReeda/releases) から、使用する OS のファイルをダウンロードして展開し、バイナリを実行すれば起動できます。

| ファイル | 中身 |
|---|---|
| `mangareeda-<版>-windows-x64.zip` | Windows 版。本体と、Dropbox・Google Drive・OCR のプラグイン |
| `mangareeda-<版>-linux-x64.tar.gz` | Linux 版。同じく本体とプラグイン（PDF を開くための libpdfium.so も同梱） |
| `mangareeda-bookscan-<版>-<OS>-x64` | BOOKSCAN のプラグイン。使う人だけ、本体と同じフォルダに置く |
| `mangareeda-manga-ocr-<版>.tar.gz` | マンガ向けの OCR のプラグイン（Linux、GPL-3.0） |

- Windows では、署名をしていないので、初めて起動するときに SmartScreen の警告が出ることがあります。
- Linux では、展開したフォルダの `packaging/linux/install.sh` を実行すると、ランチャーにアイコン付きで出せます。OCR を使うには Tesseract が要ります（ビルドの節を参照）。

プラグインを使用するには各サービスのAPIキーや登録情報が必要です。詳しくはdocs/plugins.mdを参照してください。

## ビルド

ビルドには Rust のツールチェーン、C/C++ コンパイラ（RAR の展開に使う）、AVIF のデコードに使う dav1d が必要です。

```sh
# Arch Linux 系の例
sudo pacman -S --needed base-devel rust dav1d

cargo build --release --workspace
./target/release/mangareeda [開くファイルやフォルダ]
```

Windows では、Visual Studio（C++ のビルドツール）と [vcpkg](https://github.com/microsoft/vcpkg) で入れた dav1d を使います。`.cargo.win` を `.cargo` という名前で写すと、vcpkg の pkg-config を使うようになります。

```bat
rem vcpkg を C:\vcpkg に入れた例
C:\vcpkg\vcpkg install dav1d:x64-windows pkgconf:x64-windows
set VCPKG_ROOT=C:\vcpkg
xcopy /e /i .cargo.win .cargo
cargo build --release --workspace
```

ビルド時に `--workspace` を付けると、BOOKSCAN・Dropbox・Google Drive のプラグイン（`mangareeda-bookscan`、`mangareeda-dropbox`、`mangareeda-gdrive`）と、OCR のプラグイン（`mangareeda-ocr-tesseract`、Windows では `mangareeda-ocr-windows`）も同じフォルダにできます。要らなければ外してください。

Linux で OCR を使うには Tesseract と日本語のデータが要ります（Arch Linux 系では `sudo pacman -S tesseract tesseract-data-jpn tesseract-data-jpn_vert`）。Windows では、Windows の言語機能のインストールで日本語が追加されていれば使えるはずです。

Linux で PDF を開くには `libpdfium.so` が必要です。別途インストールしてください。実行ファイルと同じフォルダ、システムのライブラリの順に探します。Arch Linux 系の場合は、AUR の `pdfium-binaries-bin` を入れると `/usr/lib/libpdfium.so` に置かれ、そのまま使えます。ほかの Linux 環境では、[pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) の Linux 版を展開し、`lib/libpdfium.so` を実行ファイルと同じフォルダに置いてください。

Linux の Wayland 環境の場合、ランチャーや dock にアイコン付きで出すには、ビルドしたあとに次を実行する必要があるかもしれません。これはアプリケーション一覧（`~/.local/share/applications`）に `.desktop` ファイルとアイコンを入れるスクリプトです。外すときは `--uninstall` を付けて実行してください。

```sh
packaging/linux/install.sh            # target/release/mangareeda を登録する
packaging/linux/install.sh /path/to/mangareeda   # ほかの場所の実行ファイルを登録する
```

ソフトの設定は `~/.config/MangaReeda/settings.json`（`XDG_CONFIG_HOME` があるならその下）に保存されます。


## 注意事項

このソフトは閲覧するための道具です。

- マンガ、コミック、書庫などのコンテンツを提供・配布するためのものではなく、入手先へのリンクも持ちません。
- プラグインは、利用者自身が契約しているサービスに、利用者のアカウントで接続します。サービスの利用規約にしたがって使ってください。
- 開くコンテンツについては、利用者が責任を持ってください。
- 利用者が選んだコンテンツや入手先によって生じた著作権侵害その他の問題について、開発者は責任を負いません。

## ライセンス

Apache License 2.0（[LICENSE](LICENSE)）。
このソフトのベースにさせていただいたのは Lycoris52 氏の [RustMangaReader](https://github.com/Lycoris52/RustMangaReader) です。詳しくは [NOTICE](NOTICE) を参照してください。

同梱しているフォント（Noto Sans JP）は SIL Open Font License 1.1 です（[src/assets/OFL.txt](src/assets/OFL.txt)）。
Windows 用に同梱している `pdfium.dll` と `dav1d.dll` のライセンスは [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照してください。

---

<a name="english"></a>

# MangaReeda (English)

An offline manga and comic viewer for Linux and Windows, written in Rust. It has not been tried on macOS, but porting it is likely to be straightforward.
It shows double-page spreads, reads archives without extracting them, and is controlled with the keyboard, mouse or a gamepad.

Inspired by MangaMeeya, a once-popular Japanese viewer. It contains no MangaMeeya code and is not affiliated with MangaMeeya's author.

Plugins bring up-to-date features: cloud services such as Dropbox and Google Drive, incremental search in the book list, searching the text of books, and reading PDFs and other books without a text layer with an OCR engine so that they can be searched too.

## Features

- Single page, double-page spread (right to left / left to right) and vertical scroll (web comic) views
- Automatic spreads for runs of portrait pages, and an odd-page start that puts a blank page before the first page
- Splitting wide pages into halves shown one at a time
- Page thumbnails, and cover thumbnails of the books in the same folder
- Per-book bookmarks and history
- A book list of history and favorites shown as covers (Tab switches lists in the cover thumbnails)
- Plugins that add book lists; plugins for Dropbox and Google Drive folders and the BOOKSCAN bookshelf are included ([docs/plugins.md](docs/plugins.md), in Japanese)
- Search in a book (Ctrl+F): the text layer of PDFs, or text read by an OCR plugin for books without one (on Windows, the capable OCR built into the OS; on Linux, a Tesseract plugin is included). A [manga-ocr plugin](plugins/manga-ocr/README.md) for manga can be installed as well
- Loupe, zoom, and resampling filters (nearest to Lanczos3)
- Reads archives directly without extracting them, optionally including images in subfolders
- Moves to the previous / next book in the folder and to neighbouring folders
- Fully remappable keyboard, mouse (click, double click, long press, button chords) and gamepad controls, with a MangaMeeya-style preset
- Animated WebP / GIF / APNG playback and automatic page turning

## Supported formats

| Kind | Formats |
|---|---|
| Archives | `.zip` `.cbz` `.rar` `.cbr` |
| Documents | `.pdf` (needs the pdfium library, see below) |
| Folders | images in the folder |
| Images | `.png` `.jpg` `.jpeg` `.bmp` `.tiff` `.webp` `.avif` `.tga` `.gif` |

## Releases

For those who do not build it themselves: download the file for your OS from [Releases](https://github.com/neatsorg/MangaReeda/releases), extract it and run the binary.

| File | Contents |
|---|---|
| `mangareeda-<version>-windows-x64.zip` | Windows build: the viewer and the Dropbox, Google Drive and OCR plugins |
| `mangareeda-<version>-linux-x64.tar.gz` | Linux build: the same, with libpdfium.so for opening PDFs |
| `mangareeda-bookscan-<version>-<OS>-x64` | The BOOKSCAN plugin. Put it in the viewer's folder only if you use it |
| `mangareeda-manga-ocr-<version>.tar.gz` | An OCR plugin for manga (Linux, GPL-3.0) |

- On Windows, the programs are not signed, so SmartScreen may warn the first time you run them.
- On Linux, run `packaging/linux/install.sh` in the extracted folder to show the app with its icon in launchers. OCR needs Tesseract (see Build).

Plugins need an API key or account details for each service. See docs/plugins.md for details.

## Build

Building needs a Rust toolchain, a C/C++ compiler (for RAR support) and dav1d (for AVIF).

```sh
# Arch Linux and derivatives
sudo pacman -S --needed base-devel rust dav1d

cargo build --release --workspace
./target/release/mangareeda [file or folder to open]
```

On Windows, use Visual Studio (C++ build tools) and dav1d installed with [vcpkg](https://github.com/microsoft/vcpkg). Copying `.cargo.win` to `.cargo` makes the build use vcpkg's pkg-config.

```bat
rem with vcpkg in C:\vcpkg
C:\vcpkg\vcpkg install dav1d:x64-windows pkgconf:x64-windows
set VCPKG_ROOT=C:\vcpkg
xcopy /e /i .cargo.win .cargo
cargo build --release --workspace
```

Building with `--workspace` also builds the BOOKSCAN, Dropbox and Google Drive plugins (`mangareeda-bookscan`, `mangareeda-dropbox`, `mangareeda-gdrive`) and the OCR plugins (`mangareeda-ocr-tesseract`, and `mangareeda-ocr-windows` on Windows) next to the viewer. Leave it out if you do not need them.

OCR on Linux needs Tesseract with Japanese data (on Arch Linux and derivatives, `sudo pacman -S tesseract tesseract-data-jpn tesseract-data-jpn_vert`). On Windows, it should work once Japanese has been added through Windows' language features.

Opening PDFs on Linux needs `libpdfium.so`; install it separately. It is looked up next to the executable, then among the system libraries. On Arch Linux and derivatives, the AUR package `pdfium-binaries-bin` installs it as `/usr/lib/libpdfium.so`, which works as is. On other Linux systems, extract the Linux build from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) and put `lib/libpdfium.so` next to the executable.

On Linux with Wayland, you may need to run the following after building to show the app with its icon in launchers and docks. The script installs a `.desktop` file and the icon into your applications (`~/.local/share/applications`); run it with `--uninstall` to remove them.

```sh
packaging/linux/install.sh            # registers target/release/mangareeda
packaging/linux/install.sh /path/to/mangareeda   # registers an executable elsewhere
```

Settings are stored in `~/.config/MangaReeda/settings.json` (under `XDG_CONFIG_HOME` when set).

## Disclaimer

MangaReeda is a tool for viewing.

- It is not meant to provide or distribute manga, comics, archives or other content, and it has no links to download sources.
- Plugins connect to services the user subscribes to, with the user's own account. Follow each service's terms of use.
- Users are responsible for the content they open.
- The developers take no responsibility for copyright infringement or other issues arising from the content or sources users choose.

## License

Apache License 2.0 ([LICENSE](LICENSE)).
This software is based on [RustMangaReader](https://github.com/Lycoris52/RustMangaReader) by Lycoris52; see [NOTICE](NOTICE).

The bundled Noto Sans JP font is under the SIL Open Font License 1.1 ([src/assets/OFL.txt](src/assets/OFL.txt)).
For `pdfium.dll` and `dav1d.dll`, bundled for Windows, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
