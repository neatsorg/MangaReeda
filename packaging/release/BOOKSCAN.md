# BOOKSCAN plugin for MangaReeda

[English](#english)

MangaReeda の表紙サムネイルに「BOOKSCAN」のタブを足し、[BOOKSCAN](https://www.bookscan.co.jp/) の本棚の本を一覧にして、PDF をダウンロードして読めるようにするプラグインです。

## 入れ方

1. このフォルダの `mangareeda-bookscan`（Windows では `mangareeda-bookscan.exe`）を、`mangareeda` と同じフォルダに置く
2. MangaReeda を起動し直し、設定画面の「プラグイン」で、BOOKSCAN のメールアドレスとパスワードを入れて保存する
3. 表紙サムネイルの「BOOKSCAN」タブを開く

ログイン情報は OS のキーリングに保存し、設定ファイルには書きません。詳しくは MangaReeda の `docs/plugins.md` を見てください。

## 注意

- BOOKSCAN の利用規約に従って、ご自分のアカウントで使ってください。
- サイトに負担をかけないよう、本棚のページは間を空けて取り、ダウンロードは1冊ずつ行います。
- サイトの作りが変わると、動かなくなることがあります。
- BOOKSCAN とは関係のない、非公式のプラグインです。

ライセンス: Apache License 2.0（LICENSE、NOTICE）。

---

<a name="english"></a>

## English

Adds a BOOKSCAN tab to MangaReeda's cover thumbnails: lists the books on your [BOOKSCAN](https://www.bookscan.co.jp/) bookshelf and downloads their PDFs for reading.

1. Put `mangareeda-bookscan` (`mangareeda-bookscan.exe` on Windows) in the same folder as `mangareeda`.
2. Restart MangaReeda and enter your BOOKSCAN email address and password under Plugins in the settings.
3. Open the BOOKSCAN tab in the cover thumbnails.

Credentials are kept in the OS keyring, never in the settings file. Use it with your own account and follow BOOKSCAN's terms of use. The site is read gently (bookshelf pages are fetched with pauses, one download at a time), and changes to the site may break the plugin. This is an unofficial plugin, not affiliated with BOOKSCAN.

License: Apache License 2.0 (LICENSE, NOTICE).
