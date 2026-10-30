# RapidOCR のプラグイン

MangaReeda の本の中の検索で使う OCR のプラグインです。ページの文字を [RapidOCR](https://github.com/RapidAI/RapidOCR) で読みます。
RapidOCR は、[PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) のモデル（PP-OCRv6）を ONNX Runtime で動かすものです。

[manga-ocr のプラグイン](../manga-ocr/README.md) と違って PyTorch を使わないので軽く（入れると約 350MB）、CPU でも1ページ1秒ほどで読めます。
マンガの吹き出しの文字も、縦書き・横書きの本の文字も読めます。縦書きのページは、右の行から順に並べ直して返します。

小さい「っ」「ッ」を落とすことがあります（「楽しかった」→「楽しかた」）。検索で見つからないときは、言葉を短くしてみてください。

## 入れ方（Linux）

[uv](https://docs.astral.sh/uv/) が要ります。次を実行すると、`~/.config/MangaReeda/plugins/rapidocr` に入ります（`XDG_CONFIG_HOME` があればその下）。

```sh
plugins/rapidocr/install.sh
```

モデル（約 30MB）は RapidOCR のパッケージに入っているので、入れたあとはネットにつながなくても読めます。入れるときに、文字を読めるか確かめます。
MangaReeda を起動し直し、設定画面の「プラグイン」の「本の中の検索で使う OCR」で RapidOCR を選んでください。
外すときは `--uninstall` を付けます（モデルも一緒に消えます）。

## 入れ方（Windows）

[uv](https://docs.astral.sh/uv/) が要ります。PowerShell で、`mangareeda.exe` のあるフォルダを指定して実行すると、その下の `plugins\rapidocr` に入ります（リリースの zip を展開して使う MangaReeda 向けです）。

```powershell
powershell -ExecutionPolicy Bypass -File plugins\rapidocr\install.ps1 C:\Tools\mangareeda
```

MangaReeda を起動し直し、設定画面の「プラグイン」の「本の中の検索で使う OCR」で RapidOCR を選んでください。
外すときは `-Uninstall` を付けます。

Windows では、MangaReeda に同梱の OCR（Windows の OCR）のほうが速く読めます。RapidOCR は、Windows の日本語の OCR の部品が入っていないときや、マンガの小さな吹き出しを読ませたいときに使ってください。

## ライセンス

このフォルダは、MangaReeda の本体と同じ Apache License 2.0 です。
入れるときに取ってくる RapidOCR（PaddleOCR のモデルを含む）も Apache License 2.0 です。
