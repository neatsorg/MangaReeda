#!/bin/sh
# リリースのファイルを作る（GitHub Actions のリリースの流れから呼ぶ。手元でも動く）。
# 先に `cargo build --release --workspace` でビルドしておく。
#
# 使い方:
#   packaging/release/package.sh linux   <版> <出力先>   Linux 版（PDFIUM_LIB に libpdfium.so の場所）
#   packaging/release/package.sh windows <版> <出力先>   Windows 版（Git Bash などで。7z を使う）
#
# 作るもの:
#   mangareeda-<版>-<OS>-x64            本体と、Dropbox・Google Drive・Kindle・OCR のプラグイン
#   mangareeda-bookscan-<版>-<OS>-x64   BOOKSCAN のプラグイン（使う人が選んで入れる。
#                                        環境変数 RELEASE_BOOKSCAN が no なら作らない）
#   mangareeda-manga-ocr-<版>           manga-ocr のプラグイン（GPL-3.0。Linux 版のときだけ作る）
#   mangareeda-rapidocr-<版>            RapidOCR のプラグイン（OS によらない。Linux 版は tar.gz、Windows 版は zip）
set -eu

os=$1
version=$2
out=$(mkdir -p "$3" && cd "$3" && pwd)
root=$(cd "$(dirname "$0")/../.." && pwd)
release="$root/target/release"

case $os in
    linux) exe= ;;
    windows) exe=.exe ;;
    *)
        echo "OS は linux か windows: $os" >&2
        exit 1
        ;;
esac

# 告知の文書（どのファイルにも入れる）。THIRD_PARTY_NOTICES.md から参照する場所に置く
# （埋め込んだフォントのライセンスは src/assets/OFL.txt）
notices() {
    install -m 644 "$root/LICENSE" "$root/NOTICE" "$root/THIRD_PARTY_NOTICES.md" "$1/"
    cp -R "$root/third_party_licenses" "$1/"
    mkdir -p "$1/src/assets"
    install -m 644 "$root/src/assets/OFL.txt" "$1/src/assets/"
    cp "$out/THIRD_PARTY_LICENSES.html" "$1/"
}

# 書庫にする
archive() {
    name=$1
    case $os in
        linux) tar -C "$out" -czf "$out/$name.tar.gz" "$name" ;;
        windows) (cd "$out" && 7z a -tzip -bso0 "$name.zip" "$name") ;;
    esac
    rm -rf "${out:?}/$name"
}

# プログラムに入る Rust の部品のライセンス文
(cd "$root" && cargo about generate --workspace \
    -c packaging/release/about.toml packaging/release/about.hbs \
    -o "$out/THIRD_PARTY_LICENSES.html")

# 本体
main="mangareeda-$version-$os-x64"
dir="$out/$main"
rm -rf "$dir"
mkdir -p "$dir/docs"
plugins="mangareeda-dropbox mangareeda-gdrive mangareeda-kindle mangareeda-ocr-tesseract"
if [ "$os" = windows ]; then
    plugins="$plugins mangareeda-ocr-windows"
fi
for program in mangareeda $plugins; do
    install -m 755 "$release/$program$exe" "$dir/"
done
install -m 644 "$root/README.md" "$dir/"
install -m 644 "$root/docs/plugins.md" "$dir/docs/"
notices "$dir"
case $os in
    windows)
        install -m 644 "$root/pdfium.dll" "$root/dav1d.dll" "$dir/"
        ;;
    linux)
        # pdfium を同梱する（実行ファイルと同じフォルダから読む）。dav1d は静的にリンクしてある
        install -m 644 "${PDFIUM_LIB:?PDFIUM_LIB に libpdfium.so の場所を}" "$dir/libpdfium.so"
        # ランチャーに登録するスクリプト（packaging/linux/install.sh。アイコンは src/assets から読む）
        mkdir -p "$dir/packaging/linux" "$dir/src/assets"
        install -m 755 "$root/packaging/linux/install.sh" "$dir/packaging/linux/"
        install -m 644 "$root/packaging/linux/io.github.neatsorg.MangaReeda.desktop" "$dir/packaging/linux/"
        install -m 644 "$root/src/assets/icon256.png" "$dir/src/assets/"
        ;;
esac
archive "$main"

# BOOKSCAN のプラグイン（変えていない版では作らず、前の版のものを使ってもらう）
if [ "${RELEASE_BOOKSCAN:-yes}" != no ]; then
    bookscan="mangareeda-bookscan-$version-$os-x64"
    dir="$out/$bookscan"
    rm -rf "$dir"
    mkdir -p "$dir"
    install -m 755 "$release/mangareeda-bookscan$exe" "$dir/"
    install -m 644 "$root/packaging/release/BOOKSCAN.md" "$dir/README.md"
    notices "$dir"
    archive "$bookscan"
fi

# manga-ocr のプラグイン（OS によらないので、Linux 版のときだけ作る）
if [ "$os" = linux ]; then
    manga="mangareeda-manga-ocr-$version"
    dir="$out/$manga"
    rm -rf "$dir"
    mkdir -p "$dir"
    for file in LICENSE README.md plugin.json manga_ocr_plugin.py; do
        install -m 644 "$root/plugins/manga-ocr/$file" "$dir/"
    done
    install -m 755 "$root/plugins/manga-ocr/install.sh" "$root/plugins/manga-ocr/run.sh" "$dir/"
    archive "$manga"
fi

# RapidOCR のプラグイン（Python。OS によらないので、どちらの OS でも同じ中身を、その OS の書庫の形で作る）
rapid="mangareeda-rapidocr-$version"
dir="$out/$rapid"
rm -rf "$dir"
mkdir -p "$dir"
for file in README.md plugin.json rapidocr_plugin.py run.cmd install.ps1; do
    install -m 644 "$root/plugins/rapidocr/$file" "$dir/"
done
install -m 755 "$root/plugins/rapidocr/install.sh" "$root/plugins/rapidocr/run.sh" "$dir/"
install -m 644 "$root/LICENSE" "$root/NOTICE" "$dir/"
archive "$rapid"

rm -f "$out/THIRD_PARTY_LICENSES.html"
echo "作りました:"
ls -l "$out"
