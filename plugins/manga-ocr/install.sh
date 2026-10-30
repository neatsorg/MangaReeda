#!/bin/sh
# manga-ocr のプラグインを、この利用者の MangaReeda に入れる（Linux）。
# Python 3.12 の環境を作り、PyTorch と mokuro（manga-ocr を含む）を入れて、モデルも先に取ってくる。
# 合わせて 2〜3GB ほどになる（GPU 版の PyTorch ではもっと大きい）。
#
# 使い方:
#   plugins/manga-ocr/install.sh              CPU 版の PyTorch で入れる（GPU が無くても動く）
#   plugins/manga-ocr/install.sh --gpu        GPU 版の PyTorch で入れる（NVIDIA の GPU がある人向け）
#   plugins/manga-ocr/install.sh --uninstall  外す（取ってきたモデルは ~/.cache に残る）
set -eu

here=$(cd "$(dirname "$0")" && pwd)
# MangaReeda と同じく、XDG_CONFIG_HOME は絶対パスのときだけ使う
case "${XDG_CONFIG_HOME:-}" in
    /*) config=$XDG_CONFIG_HOME ;;
    *) config=$HOME/.config ;;
esac
dest="$config/MangaReeda/plugins/manga-ocr"

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
work="$plugins/.manga-ocr.installing"
old="$plugins/.manga-ocr.old"
# 前に入れ替えの途中で止まっていたら、前に入れたものを戻してから始める
if [ -e "$old" ] && [ ! -e "$dest" ]; then
    mv "$old" "$dest"
fi
rm -rf "$work" "$old"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work"
for file in plugin.json run.sh manga_ocr_plugin.py LICENSE; do
    install -m 644 "$here/$file" "$work/$file"
done
chmod 755 "$work/run.sh"

uv venv --python 3.12 "$work/venv"
if [ "${1:-}" = "--gpu" ]; then
    VIRTUAL_ENV="$work/venv" uv pip install torch torchvision
else
    VIRTUAL_ENV="$work/venv" uv pip install \
        --index-url https://download.pytorch.org/whl/cpu torch torchvision
fi
VIRTUAL_ENV="$work/venv" uv pip install mokuro

echo "モデルを取ってきます（初めてのときは数分かかります）"
"$work/venv/bin/python" -c \
    "from mokuro.manga_page_ocr import MangaPageOcr; MangaPageOcr(force_cpu=True)"

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
echo "MangaReeda を起動し直すと、設定画面の「プラグイン」で manga-ocr を選べます"
