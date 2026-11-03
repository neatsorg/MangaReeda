# mangareeda-kindle（試作）

Kindle Cloud Reader を Chromium 系のブラウザで開き、MangaReeda のキー設定で操作できるようにするプラグインの試作です。
本はブラウザで表示するだけで、ページの画像を取り出したり保存したりはしません。

まだ本体には組み込んでいません。単独のコマンドとして動かします。

## 使い方

```sh
cargo build -p mangareeda-kindle
target/debug/mangareeda-kindle open            # ライブラリを開く（最初はここで Amazon にログインする）
target/debug/mangareeda-kindle open --asin B0XXXXXXXX
target/debug/mangareeda-kindle probe > probe.json   # 開いているページのボタンなどを書き出す（調査用）
target/debug/mangareeda-kindle library         # ライブラリの本を list と同じ形の JSON で書き出す
```

共通の引数:

| 引数 | 意味 |
|---|---|
| `--browser PATH` / `--browser flatpak:ID` | ブラウザを指定する（環境変数 `MANGAREEDA_BROWSER` でも可） |
| `--settings PATH` | キー設定を読む `settings.json`（読むだけで書き込まない） |
| `--site co.jp` / `--site com` | Amazon のサイト |
| `--fullscreen` | 全画面で開く |
| `--headless` | ウィンドウを出さずに開く（調べもの用） |

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
- `library` はログイン済みなら headless でも一覧を返す（50 冊を確認。それで全部かは未確認）

## まだ確かめていないこと

- 単ページと見開きの切り替えの方法（リーダーの設定メニュー）
- 文字の本（マンガ以外）のリーダーの URL
- `library` が全部の本を拾えるか（下まで送って集めている）
- flatpak 版のブラウザで DevTools につなげるか
- Windows の Edge で動くか
