# mangareeda-kindle

Kindle Cloud Reader を Chromium 系のブラウザで開き、MangaReeda のキー設定で操作できるようにする読むプラグイン（`kind: "reader"`）です。
本はブラウザで表示するだけで、ページの画像を取り出したり保存したりはしません。

MangaReeda からの使い方は [docs/plugins.md](../../docs/plugins.md#kindle) にあります。ここには、単独で動かすときの使い方と、作りのメモを書きます。

## 使い方

```sh
cargo build -p mangareeda-kindle
target/debug/mangareeda-kindle open            # ライブラリを開く（最初はここで Amazon にログインする）
target/debug/mangareeda-kindle open B0XXXXXXXX # 本を開く（--asin でも可）
target/debug/mangareeda-kindle list            # ライブラリの本を書き出す（library も同じ）
target/debug/mangareeda-kindle probe > probe.json   # 開いているページのボタンなどを書き出す（調査用）
target/debug/mangareeda-kindle eval --expr 'location.href'   # リーダーで式を評価する（調査用）
```

共通の引数:

| 引数 | 意味 |
|---|---|
| `--browser PATH` / `--browser flatpak:ID` | ブラウザを指定する（環境変数 `MANGAREEDA_BROWSER` でも可） |
| `--settings PATH` | キー設定を読む `settings.json`（読むだけで書き込まない） |
| `--site co.jp` / `--site com` | Amazon のサイト |
| `--fullscreen` | 全画面で開く |
| `--headless` | ウィンドウを出さずに開く（調べもの用） |

MangaReeda から呼ばれたときは、キー設定と単ページか見開きかを標準入力で受け取ります。端末から動かしたときは `settings.json` を読みます。
すでに Kindle の窓が開いているときの `open` は、その窓で本を開くよう頼むだけで終わります（キーは最初の `open` が受け持つ）。
ログインしていないときの `list` は、終了コード 2（ログインが必要）で終わります。

`list` は、ライブラリのページが続きを読み込むときと同じ `/kindle-library/search`（`querySize=50`）を1回だけ呼び、50 冊と続きの印（`next`）を返します。
続きを頼まれたら（標準入力の `"next"`）、その印を `paginationToken` にして次の 50 冊を返します。
本を開く URL（`webReaderUrl`。マンガは `/kindle-library/manga-wr/<ASIN>`）は `reader-urls.json` に覚えておき、`open` で使います。

## ブラウザの探し方

版は問いません。Windows は Edge、Chrome、Brave、Vivaldi、Chromium の順に探します。
Linux は `PATH` にある chromium、google-chrome、microsoft-edge、brave、vivaldi を探し、無ければ flatpak 版を探します。

ふだん使っているプロファイルとは別の、Kindle 専用のプロファイルを使います。

- Linux: `~/.local/share/MangaReeda/kindle/profile`
- Windows: `%LOCALAPPDATA%\MangaReeda\kindle\profile`

## キーの置き換え

`settings.json` の `keys` にあるキーのうち、Kindle に関係する動作のものだけを横取りし、Kindle 側の操作に置き換えます。
送るキーが押したキーと同じ場合は、横取りせずにそのまま渡します。割り当てのないキーもそのまま Kindle に渡します。

既定の Kindle 側の操作は推測を含みます。プロファイルと同じフォルダの `actions.json` で上書きできます。

```json
{
  "next_page": {"key": "ArrowLeft"},
  "toggle_auto_page_layout": {"click": "button[aria-label='…']"},
  "first_page": {"js": "…"},
  "fullscreen": {"builtin": "fullscreen"},
  "quit_app": null
}
```

| 操作 | 意味 |
|---|---|
| `key` | ブラウザにキーを送る（`KeyboardEvent.key` の名前） |
| `click` | CSS セレクタに合う要素をクリックする |
| `js` | ページでスクリプトを実行する |
| `builtin` | `fullscreen`（全画面の切り替え）、`close`（ウィンドウを閉じる）、`library`（ライブラリに戻る） |
| `null` | その動作を横取りしない |

## 同じ窓で読む

ライブラリは `window.open` で `/kindle-library/manga-wr/<ASIN>` を新しいタブに開き、そこから `/manga/<ASIN>` へ移ります。
アプリ表示ではそれが別の窓になるので、リーダーへの `window.open` は同じ窓での移動に置き換えています。

## 単ページ

Cloud Reader に単ページと見開きを切り替える項目はありませんが、窓が縦長だと1ページずつ表示します。
そこで単ページ（`builtin: "single"`、既定は「自動見開きの切り替え」のキー）では、ページの中で `body` を中央の縦長の枠
（幅 = min(窓の幅, 窓の高さ × 0.7)）に絞り、リーダーが読む `innerWidth` をその枠の幅にしています。
どちらも窓の大きさからそのつど計算するので、窓の大きさを変えても追従します。

DevTools のデバイスエミュレーションでも単ページにはなりますが、表示が左に寄り、中央に寄せる指定（`positionX`）が効きませんでした。

## 確かめたこと（Linux の Google Chrome、マンガ）

- ライブラリの本を開くと同じ窓でリーダーになり、リーダーの「ライブラリ」ボタンで同じ窓のまま戻る
- 右開きのマンガで ← が次のページ、→ が前のページ（既定の操作のままでよい）
- 全画面のキーで全画面になり、もう一度押すと戻る
- 「本を閉じる」を Ctrl+W にしていても、窓は閉じずにライブラリに戻る
- 単ページ: 中央に1ページ出て、見開きに戻せる。全画面・窓の大きさの変更に追従し、1ページずつ進む。
  単ページか見開きかはプラグインが1つだけ持ち（本ごとではない）、別の本を開いてもそのまま
- `list` はログイン済みなら headless でも一覧を返す。最初の 50 冊と続きの 50 冊を確認（続きの印が 45 なので 5 冊重なる。本体が同じ id を足さない）

## まだ確かめていないこと

- 文字の本（マンガ以外）を開いたときの動き（URL は `webReaderUrl` を使う）。単ページはマンガのリーダー（`/manga/`）だけで効く
- flatpak 版のブラウザで DevTools につなげるか
- Windows の Edge で動くか
