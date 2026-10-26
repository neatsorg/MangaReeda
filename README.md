[English](#english)

# MangaReeda

Linux で動く、オフラインのマンガ・コミックビューアです。
見開き表示、書庫を展開せずに読む、キー・マウス・ゲームパッドでの操作を中心にしています。

マンガミーヤの使い心地を Linux で再現することを目標に開発中です。
目標とする挙動は [spec/mangameeya-behavior.md](spec/mangameeya-behavior.md) にまとめています。
このソフトはマンガミーヤのコードを含まず、マンガミーヤの作者とも関係ありません。

## 主な機能

- 単ページ、見開き（右から左／左から右）、縦スクロール（Web マンガ）表示
- 縦長のページが2枚続くと見開きにする自動レイアウト、表紙を単独で見せる奇数／偶数切り替え
- 前後のページを先読みして、すぐにページをめくれる
- 書庫を展開せずに直接読む
- 縮小・拡大のフィルタ（最近傍〜Lanczos3）とズーム
- 同じフォルダの前後の書庫、隣のフォルダへの移動
- キーボード・マウス（クリック、ダブルクリック、長押し）・ゲームパッドの割り当てを自由に変えられる
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
PDF の表示に使う Pdfium は Copyright 2014 Google Inc. で、Apache License 2.0 で提供されています。

---

<a name="english"></a>

# MangaReeda (English)

An offline manga and comic viewer for Linux, focused on double-page spreads, reading archives without extracting them, and keyboard / mouse / gamepad control.

It is being developed to bring the feel of MangaMeeya to Linux; the target behavior is described (in Japanese) in [spec/mangameeya-behavior.md](spec/mangameeya-behavior.md).
It contains no MangaMeeya code and is not affiliated with MangaMeeya's author.

## Build

Requires a Rust toolchain, a C/C++ compiler (for RAR support) and dav1d (for AVIF).

```sh
cargo build --release
./target/release/mangareeda [file or folder to open]
```

Opening PDFs needs `libpdfium.so`, looked up in the working directory, next to the executable, then system-wide. Without it, only PDFs are unavailable and a message says so.

Settings are stored in `~/.config/MangaReeda/settings.json` (under `XDG_CONFIG_HOME` when set).

## Disclaimer

MangaReeda is a viewing utility only. It does not provide, host, or link to any content or download sources. Users are responsible for what they open, and the developers take no responsibility for copyright or other issues arising from it.

## License

Apache License 2.0 ([LICENSE](LICENSE)). Based on [RustMangaReader](https://github.com/Lycoris52/RustMangaReader) by Lycoris52; see [NOTICE](NOTICE).
The bundled Noto Sans JP font is under the SIL Open Font License 1.1. Pdfium is Copyright 2014 Google Inc., licensed under Apache License 2.0.
