# manga-ocr のプラグイン

MangaReeda の本の中の検索で使う OCR のプラグインです。マンガのページの文字を、[mokuro](https://github.com/kha-white/mokuro) で読みます。
mokuro は、吹き出しの場所を [comic-text-detector](https://github.com/dmMaze/comic-text-detector) で見つけ、文字を [manga-ocr](https://github.com/kha-white/manga-ocr) で読みます。マンガの縦書き・ふりがな・手書き風の文字にも強く、吹き出しごとに読むので、別の吹き出しの行が混ざりません。

同梱の OCR（Linux は Tesseract、Windows は Windows の OCR）より読める文字は多いですが、PyTorch を使うため重く（入れると 2〜3GB）、CPU では1ページに数秒かかります。

## 入れ方（Linux）

[uv](https://docs.astral.sh/uv/) が要ります。次を実行すると、`~/.config/MangaReeda/plugins/manga-ocr` に入ります（`XDG_CONFIG_HOME` があればその下）。
リリースの `mangareeda-manga-ocr-<版>.tar.gz` を使うときは、展開したフォルダの `./install.sh` を実行します。

```sh
plugins/manga-ocr/install.sh          # CPU 版の PyTorch で入れる
plugins/manga-ocr/install.sh --gpu    # NVIDIA の GPU がある人は、GPU 版で入れる
```

モデル（約 850MB）は、入れるときに `~/.cache` へ取ってきます。
MangaReeda を起動し直し、設定画面の「プラグイン」の「本の中の検索で使う OCR」で manga-ocr を選んでください。
外すときは `--uninstall` を付けます（取ってきたモデルは `~/.cache/huggingface` などに残るので、要らなければ消してください）。

GPU 版で入れても CPU で読ませたいときは、環境変数 `MANGAREEDA_OCR_CPU=1` を付けて MangaReeda を起動します。

## ライセンス

このフォルダ（manga-ocr のプラグイン）は、GNU General Public License v3.0 以降です（[LICENSE](LICENSE)）。使っている mokuro と comic-text-detector が GPL-3.0 のためです。
MangaReeda の本体は Apache License 2.0 のままで、このプラグインは別のプログラムとして呼び出します。
