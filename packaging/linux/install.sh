#!/bin/sh
# MangaReeda を、この利用者のアプリケーション一覧に入れる（Linux）。
# ランチャーや dock は、ここで入れる .desktop ファイルとアイコンを、アプリが名乗る ID
# （io.github.neatsorg.MangaReeda）で探す。
#
# 使い方:
#   packaging/linux/install.sh [実行ファイルのパス]   入れる（省くと target/release/mangareeda）
#   packaging/linux/install.sh --uninstall           外す
set -eu

id=io.github.neatsorg.MangaReeda
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
desktop="$data/applications/$id.desktop"
icon="$data/icons/hicolor/256x256/apps/$id.png"

refresh() {
    update-desktop-database "$data/applications" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$desktop" "$icon"
    refresh
    echo "外しました: $desktop"
    exit 0
fi

exe=$(realpath "${1:-$root/target/release/mangareeda}")
if [ ! -x "$exe" ]; then
    echo "実行ファイルが見つかりません: $exe" >&2
    echo "先に cargo build --release でビルドしてください" >&2
    exit 1
fi
# .desktop の Exec で特別な意味を持つ文字（" ` $ \ と %）、改行、sed の区切りに使う | は断る
newline='
'
case $exe in
    *'"'* | *'|'* | *'\'* | *'`'* | *'$'* | *'%'* | *"$newline"*)
        echo "実行ファイルのパスに使えない文字があります: $exe" >&2
        exit 1
        ;;
esac
# sed の置換では & が「見つけた文字列」になるので、文字として書く
exe_for_sed=$(printf '%s' "$exe" | sed 's/&/\\&/g')

mkdir -p "$(dirname "$desktop")" "$(dirname "$icon")"
install -m 644 "$root/src/assets/icon256.png" "$icon"
sed "s|@EXEC@|$exe_for_sed|" "$here/$id.desktop" >"$desktop"
chmod 644 "$desktop"
refresh
echo "入れました: $desktop"
echo "実行ファイル: $exe"
