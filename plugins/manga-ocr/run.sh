#!/bin/sh
# MangaReeda から呼ばれる。このフォルダの Python の環境で、プラグインのスクリプトを動かす
here=$(cd "$(dirname "$0")" && pwd)
exec "$here/venv/bin/python" "$here/manga_ocr_plugin.py" "$@"
