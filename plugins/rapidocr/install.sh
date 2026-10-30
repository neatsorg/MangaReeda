#!/bin/sh
# RapidOCR のプラグインを、この利用者の MangaReeda に入れる（Linux）。
# Python 3.12 の環境を作り、RapidOCR と ONNX Runtime を入れて、文字を読めるか確かめる。
# 合わせて 350MB ほどになる。モデルは RapidOCR のパッケージに入っているので、外せば一緒に消える。
#
# 使い方:
#   plugins/rapidocr/install.sh              入れる
#   plugins/rapidocr/install.sh --uninstall  外す
set -eu

here=$(cd "$(dirname "$0")" && pwd)
# MangaReeda と同じく、XDG_CONFIG_HOME は絶対パスのときだけ使う
case "${XDG_CONFIG_HOME:-}" in
    /*) config=$XDG_CONFIG_HOME ;;
    *) config=$HOME/.config ;;
esac
dest="$config/MangaReeda/plugins/rapidocr"

if [ "${1:-}" = "--uninstall" ]; then
    rm -rf "$dest"
    echo "外しました: $dest"
    exit 0
fi

if ! command -v uv >/dev/null 2>&1; then
    echo "uv が見つかりません。https://docs.astral.sh/uv/ を見て入れてください" >&2
    echo "（Python 3.12 の環境を作るのに使います）" >&2
    exit 1
fi

# 途中で失敗しても、動かないプラグインが登録されたり、前に入れたものが壊れたりしないよう、
# 別のフォルダ（MangaReeda はそこを見ない名前）で用意し、動くのを確かめてから入れ替える
plugins=$(dirname "$dest")
work="$plugins/.rapidocr.installing"
old="$plugins/.rapidocr.old"
# 前に入れ替えの途中で止まっていたら、前に入れたものを戻してから始める
if [ -e "$old" ] && [ ! -e "$dest" ]; then
    mv "$old" "$dest"
fi
rm -rf "$work" "$old"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work"
for file in plugin.json run.sh rapidocr_plugin.py; do
    install -m 644 "$here/$file" "$work/$file"
done
chmod 755 "$work/run.sh"

uv venv --python 3.12 "$work/venv"
# 設定の書き方が版で変わるので、確かめた 3 系に留める
VIRTUAL_ENV="$work/venv" uv pip install "rapidocr>=3.10,<4" onnxruntime

echo "文字を読めるか確かめます"
"$work/run.sh" check

# 用意できたので入れ替える。前に入れたものは、入れ替え終えてから消す
if [ -e "$dest" ]; then
    mv "$dest" "$old"
fi
if ! mv "$work" "$dest"; then
    # 入れられなかったら、前に入れたものを戻す
    if [ -e "$old" ]; then
        mv "$old" "$dest"
    fi
    exit 1
fi
rm -rf "$old"

echo "入れました: $dest"
echo "MangaReeda を起動し直すと、設定画面の「プラグイン」で RapidOCR を選べます"
