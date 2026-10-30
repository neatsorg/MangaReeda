#!/usr/bin/env python3
# MangaReeda の OCR プラグイン。ページの文字を RapidOCR（PaddleOCR のモデルを ONNX Runtime で
# 動かすもの）で読む。PyTorch を使わないので軽く、CPU でも1ページ1秒ほどで読める。
#
# 使い方（MangaReeda から呼ばれる。取り決めは docs/plugins.md）:
#   run.sh ocr（Windows では run.cmd ocr）
# 標準入力に {"credentials": {}, "images": [{"page": 0, "path": "…"}, …]} を受け取り、
# 読み終えたページから {"page": 0, "text": "…"} を1行ずつ書く。
# RapidOCR は縦書きの行も左から並べて返すので、日本語の読む順（右から）に並べ直す。

import json
import sys

USAGE = (
    "使い方: run.sh ocr（Windows では run.cmd ocr）\n"
    "MangaReeda から呼ばれ、標準入力に1行の JSON で画像を受け取ります（docs/plugins.md）"
)


def box_rect(box):
    """4つの角の座標から (左, 上, 右, 下) にする"""
    xs = [point[0] for point in box]
    ys = [point[1] for point in box]
    return min(xs), min(ys), max(xs), max(ys)


def reading_order(boxes, texts):
    """行を読む順に並べる。縦書きの行が多いページは、高さの重なる帯ごとに右から並べる
    （検索は行をまたいで探すので、段の順が逆だと段をまたぐ言葉が見つからない）"""
    lines = [(box_rect(box), text) for box, text in zip(boxes, texts)]
    vertical = sum(len(text) for (l, t, r, b), text in lines if b - t > r - l)
    if vertical * 2 <= sum(len(text) for _, text in lines):
        return [text for _, text in lines]  # 横書きは RapidOCR の順（上から）のまま
    bands = []  # [上, 下, 行の一覧]
    for rect, text in sorted(lines, key=lambda line: line[0][1]):
        left, top, right, bottom = rect
        for band in bands:
            overlap = min(bottom, band[1]) - max(top, band[0])
            # 行と帯の、低いほうの高さの半分以上が重なれば同じ帯（短い行が先に帯を作っても入れる）
            if overlap * 2 >= min(bottom - top, band[1] - band[0]):
                band[1] = max(band[1], bottom)
                band[2].append((rect, text))
                break
        else:
            bands.append([top, bottom, [(rect, text)]])
    order = []
    for _, _, band_lines in bands:
        band_lines.sort(key=lambda line: (-line[0][2], line[0][1]))  # 右から、同じなら上から
        order.extend(text for _, text in band_lines)
    return order


def main():
    # MangaReeda とは UTF-8 でやり取りする（Windows の Python は、パイプを cp932 で読み書きする）
    for stream in (sys.stdin, sys.stdout, sys.stderr):
        stream.reconfigure(encoding="utf-8")
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
        from rapidocr import RapidOCR
    except ImportError as error:
        print(
            f"RapidOCR を読み込めませんでした（{error}）。install.sh で入れ直してください",
            file=sys.stderr,
        )
        return 1
    # 既定のモデル（PP-OCRv6 の多言語・small）がマンガも本もいちばんよく読めた。
    # 行が上下逆さまかを見分ける機能（use_cls）は、逆さまでない横書きの行をひっくり返して
    # 句読点や小さい「っ」を落とすので使わない（スキャンした本に逆さまの行はまず無い）。
    # MangaReeda が渡す画像は長い辺が 2400 ピクセルまでなので、縮めずに読む（既定は 2000 に縮める）。
    # 進み具合などの表示は標準出力を汚さないよう抑える
    ocr = RapidOCR(
        params={
            "Global.log_level": "error",
            "Global.use_cls": False,
            "Global.max_side_len": 3000,
        }
    )
    for image in images:
        try:
            result = ocr(image["path"])
            boxes = result.boxes if result.boxes is not None else []
            text = "\n".join(reading_order(boxes, result.txts or ()))
        except Exception as error:  # 1ページ読めなくても、ほかのページは続ける
            print(f"{image.get('path')}: {error}", file=sys.stderr)
            continue
        line = json.dumps({"page": image["page"], "text": text}, ensure_ascii=False)
        print(line, flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
