[English](#english)

# MangaReeda

Linux で動く、オフラインのマンガ・コミックビューアです。
見開き表示、書庫を展開せずに読む、キー・マウス・ゲームパッドでの操作を中心にしています。

かつての「マンガミーヤ」にインスパイアされて作りました。マンガミーヤのコードは含まず、マンガミーヤの作者とも関係ありません。

## 主な機能

- 単ページ、見開き（右から左／左から右）、縦スクロール（Web マンガ）表示
- 縦長のページが続くと見開きにする自動見開き、1枚目の前に白紙を入れる奇数ページ開始
- 横長のページを左右に分けて半分ずつ見せる自動単ページ表示
- ページのサムネイル一覧と、同じフォルダの冊を表紙で並べる表紙サムネイル
- 冊ごとに読んでいた位置を覚えるしおりと履歴
- 履歴とお気に入りを表紙で並べるブックリスト（表紙サムネイルの画面で Tab で切り替え）
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

## ビルド

Rust のツールチェーン、C/C++ コンパイラ（RAR の展開に使う）、AVIF のデコードに使う dav1d が必要です。

```sh
# Arch Linux / Manjaro の例
sudo pacman -S --needed base-devel rust dav1d

cargo build --release
./target/release/mangareeda [開くファイルやフォルダ]
```

PDF を開くには `libpdfium.so` が必要です。カレントディレクトリ、実行ファイルと同じフォルダ、システムのライブラリの順に探します。見つからない場合は、PDF を開こうとしたときにメッセージを表示します（PDF 以外には影響しません）。

Arch Linux / Manjaro では、AUR の `pdfium-binaries-bin` を入れると `/usr/lib/libpdfium.so` に置かれ、そのまま使えます。ほかの環境では、[pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) の Linux 版を展開し、`lib/libpdfium.so` を実行ファイルと同じフォルダに置いてください。

設定は `~/.config/MangaReeda/settings.json`（`XDG_CONFIG_HOME` があればその下）に保存します。

## 注意事項

このソフトは閲覧するための道具です。

- マンガ、コミック、書庫などのコンテンツを提供・配布せず、入手先へのリンクも持ちません。
- 開くコンテンツについては、利用者が責任を持ってください。
- 利用者が選んだコンテンツや入手先によって生じた著作権侵害その他の問題について、開発者は責任を負いません。

## ライセンス

Apache License 2.0（[LICENSE](LICENSE)）。
このソフトは Lycoris52 さんの [RustMangaReader](https://github.com/Lycoris52/RustMangaReader) を元にしています。詳しくは [NOTICE](NOTICE) を参照してください。

同梱しているフォント（Noto Sans JP）は SIL Open Font License 1.1 です（[src/assets/OFL.txt](src/assets/OFL.txt)）。
Windows 用に同梱している `pdfium.dll` と `dav1d.dll` のライセンスは [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) を参照してください。

---

<a name="english"></a>

# MangaReeda (English)

An offline manga and comic viewer for Linux, focused on double-page spreads, reading archives without extracting them, and keyboard / mouse / gamepad control.

Inspired by the classic Japanese viewer MangaMeeya. It contains no MangaMeeya code and is not affiliated with MangaMeeya's author.

## Features

- Single page, double-page spread (right to left / left to right) and vertical scroll (web comic) views
- Automatic spreads for runs of portrait pages, and an odd-page start that puts a blank page before the first page
- Splitting wide pages into halves shown one at a time
- Page thumbnails, and cover thumbnails of the books in the same folder
- Per-book bookmarks and history
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

## Build

Requires a Rust toolchain, a C/C++ compiler (for RAR support) and dav1d (for AVIF).

```sh
# Arch Linux / Manjaro
sudo pacman -S --needed base-devel rust dav1d

cargo build --release
./target/release/mangareeda [file or folder to open]
```

Opening PDFs needs `libpdfium.so`, looked up in the working directory, next to the executable, then system-wide. Without it, only PDFs are unavailable and a message says so.

On Arch Linux / Manjaro, the AUR package `pdfium-binaries-bin` installs it as `/usr/lib/libpdfium.so`, which works as is. Elsewhere, extract the Linux build from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) and put `lib/libpdfium.so` next to the executable.

Settings are stored in `~/.config/MangaReeda/settings.json` (under `XDG_CONFIG_HOME` when set).

## Disclaimer

MangaReeda is a viewing utility only. It does not provide, host, or link to any content or download sources. Users are responsible for what they open, and the developers take no responsibility for copyright or other issues arising from it.

## License

Apache License 2.0 ([LICENSE](LICENSE)). Based on [RustMangaReader](https://github.com/Lycoris52/RustMangaReader) by Lycoris52; see [NOTICE](NOTICE).
The bundled Noto Sans JP font is under the SIL Open Font License 1.1 ([src/assets/OFL.txt](src/assets/OFL.txt)). For `pdfium.dll` and `dav1d.dll`, bundled for Windows, see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
