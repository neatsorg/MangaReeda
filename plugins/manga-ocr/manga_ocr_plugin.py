#!/usr/bin/env python3
# MangaReeda の OCR プラグイン。マンガのページの文字を、mokuro（吹き出しの場所を見つける
# comic-text-detector と、文字を読む manga-ocr）で読む。
#
# Copyright (C) 2026 MangaReeda contributors
#
# This program is free software: you can redistribute it and/or modify it under the terms of
# the GNU General Public License as published by the Free Software Foundation, either version 3
# of the License, or (at your option) any later version. See LICENSE in this folder.
#
# 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
#   run.sh ocr
# 標準入力に {"credentials": {}, "images": [{"page": 0, "path": "…"}, …]} を受け取り、
# 読み終えたページから {"page": 0, "text": "…"} を1行ずつ書く。
# 吹き出し（文字のまとまり）ごとに、中の行をつないで1行にする（別の吹き出しの行が混ざらないように）。
#
# 環境変数 MANGAREEDA_OCR_CPU=1 を付けると、GPU があっても CPU で読む。

import json
import os
import sys

USAGE = (
    "使い方: run.sh ocr\n"
    "MangaReeda から呼ばれ、標準入力に1行の JSON で画像を受け取ります（docs/plugins.md）"
)


def page_text(result):
    """mokuro の答えから、吹き出しごとに1行の文字にする"""
    blocks = result.get("blocks", [])
    return "\n".join("".join(block.get("lines", [])) for block in blocks)


def main():
    if sys.argv[1:2] != ["ocr"]:
        print(USAGE)
        return 0 if sys.argv[1:2] in (["--help"], ["-h"]) else 1
    try:
        request = json.loads(sys.stdin.readline() or "{}")
    except json.JSONDecodeError:
        request = {}
    images = request.get("images", [])
    if not images:
        return 0
    try:
        # 読み込みに時間がかかるので、渡された画像があるときだけ読み込む
        from mokuro.manga_page_ocr import MangaPageOcr
    except ImportError as error:
        print(
            f"mokuro を読み込めませんでした（{error}）。install.sh で入れ直してください",
            file=sys.stderr,
        )
        return 1
    force_cpu = os.environ.get("MANGAREEDA_OCR_CPU") == "1"
    ocr = MangaPageOcr(force_cpu=force_cpu)
    for image in images:
        try:
            text = page_text(ocr(image["path"]))
        except Exception as error:  # 1ページ読めなくても、ほかのページは続ける
            print(f"{image.get('path')}: {error}", file=sys.stderr)
            continue
        line = json.dumps({"page": image["page"], "text": text}, ensure_ascii=False)
        print(line, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
