# Third-party notices

This repository and the release archives bundle the following third-party files and code.
Their license texts are in [third_party_licenses/](third_party_licenses/), unmodified from the
upstream distributions.

The Rust crates compiled into the programs are listed with their license texts in
`THIRD_PARTY_LICENSES.html`, which is generated with
[cargo-about](https://github.com/EmbarkStudios/cargo-about) and included in the release archives.

## pdfium.dll

PDFium 146.0.7665.0 (chromium/7665) for Windows x64, the V8 build (`pdfium-v8-win-x64.tgz`)
from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) by Benoit Blanchon.

- PDFium: [third_party_licenses/pdfium/pdfium.txt](third_party_licenses/pdfium/pdfium.txt)
- pdfium-binaries build scripts: [third_party_licenses/pdfium/pdfium-binaries-LICENSE.txt](third_party_licenses/pdfium/pdfium-binaries-LICENSE.txt)
- Components built into the library: Abseil, Anti-Grain Geometry, bigint, fast_float, fdlibm,
  FP16, FreeType, Highway, ICU, inspector_protocol, Little CMS, libjpeg-turbo, OpenJPEG, libpng,
  LibTIFF, LLVM libc, simdutf, Strongtalk, V8 and zlib. Each license is in
  [third_party_licenses/pdfium/](third_party_licenses/pdfium/).

The Linux release archives include `libpdfium.so` from the same release
(`pdfium-v8-linux-x64.tgz`), under the same licenses.

## dav1d.dll

dav1d 1.5.3 for Windows, by VideoLAN and the dav1d authors, under the BSD 2-Clause License:
[third_party_licenses/dav1d/COPYING](third_party_licenses/dav1d/COPYING).

## Noto Sans JP

`src/assets/NotoSansJP-Regular.ttf`, under the SIL Open Font License 1.1:
[src/assets/OFL.txt](src/assets/OFL.txt).

The Linux release builds link dav1d 1.5.0 statically, under the same license.

## UnRAR

RAR archives are read with the UnRAR source code by Alexander Roshal, included through the
`unrar` crate. Its license is in [third_party_licenses/unrar/license.txt](third_party_licenses/unrar/license.txt).
As that license requires, this paragraph is included here:

> UnRAR source code may be used in any software to handle RAR archives without limitations
> free of charge, but cannot be used to develop RAR (WinRAR) compatible archiver and to
> re-create RAR compression algorithm, which is proprietary. Distribution of modified UnRAR
> source code in separate form or as a part of other software is permitted, provided that full
> text of this paragraph, starting from "UnRAR source code" words, is included in license, or
> in documentation if license is not available, and in source code comments of resulting
> package.
