# RapidOCR のプラグインを、MangaReeda に入れる（Windows）。
# Python 3.12 の環境を作り、RapidOCR と ONNX Runtime を入れて、モデルも先に取ってくる。
# 合わせて 300MB ほどになる。モデルはこの環境の中に置くので、外せば一緒に消える。
#
# 使い方（PowerShell で。<フォルダ> は mangareeda.exe のあるフォルダ）:
#   powershell -ExecutionPolicy Bypass -File install.ps1 <フォルダ>              入れる
#   powershell -ExecutionPolicy Bypass -File install.ps1 <フォルダ> -Uninstall   外す
#
# PowerShell 5.1 は BOM の無いスクリプトを Shift_JIS で読むので、このファイルは BOM 付きの UTF-8 にしておく。
param(
    [string]$MangaReeda,
    [switch]$Uninstall
)
$ErrorActionPreference = "Stop"

if (-not $MangaReeda -or -not (Test-Path (Join-Path $MangaReeda "mangareeda.exe"))) {
    Write-Error "mangareeda.exe のあるフォルダを指定してください（例: install.ps1 C:\Tools\mangareeda）"
    exit 1
}
# Windows の MangaReeda は、mangareeda.exe の隣の plugins を見る
$plugins = Join-Path (Resolve-Path $MangaReeda) "plugins"
$dest = Join-Path $plugins "rapidocr"

if ($Uninstall) {
    if (Test-Path $dest) { Remove-Item -Recurse -Force $dest }
    Write-Host "外しました: $dest"
    exit 0
}

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    Write-Error "uv が見つかりません。https://docs.astral.sh/uv/ を見て入れてください（Python 3.12 の環境を作るのに使います）"
    exit 1
}

# 外の命令は失敗しても止まらないので、終了コードを見て止める
function Invoke-Checked {
    & $args[0] @($args | Select-Object -Skip 1)
    if ($LASTEXITCODE -ne 0) { throw "失敗しました: $($args -join ' ')" }
}

# 途中で失敗しても、動かないプラグインが登録されたり、前に入れたものが壊れたりしないよう、
# 別のフォルダ（MangaReeda はそこを見ない名前）で用意し、動くのを確かめてから入れ替える
$here = $PSScriptRoot
$work = Join-Path $plugins ".rapidocr.installing"
$old = Join-Path $plugins ".rapidocr.old"
foreach ($dir in @($work, $old)) {
    if (Test-Path $dir) { Remove-Item -Recurse -Force $dir }
}
New-Item -ItemType Directory -Force $work | Out-Null
try {
    Copy-Item (Join-Path $here "rapidocr_plugin.py"), (Join-Path $here "run.cmd") $work
    # Windows では run.sh の代わりに run.cmd を起動する。
    # MangaReeda は BOM 付きの JSON を読めないので、BOM を付けずに書く
    $manifest = (Get-Content -Raw -Encoding UTF8 (Join-Path $here "plugin.json")).Replace('"./run.sh"', '"./run.cmd"')
    [System.IO.File]::WriteAllText((Join-Path $work "plugin.json"), $manifest, (New-Object System.Text.UTF8Encoding $false))

    $venv = Join-Path $work "venv"
    Invoke-Checked uv venv --python 3.12 $venv
    # 設定の書き方が版で変わるので、確かめた 3 系に留める
    $env:VIRTUAL_ENV = $venv
    Invoke-Checked uv pip install "rapidocr>=3.10,<4" onnxruntime
    Remove-Item Env:VIRTUAL_ENV

    Write-Host "モデルを取ってきます"
    Invoke-Checked (Join-Path $venv "Scripts\python.exe") -c "from rapidocr import RapidOCR; RapidOCR(params={'Global.log_level': 'error'})"

    # 用意できたので入れ替える。前に入れたものは、入れ替え終えてから消す
    if (Test-Path $dest) { Move-Item $dest $old }
    Move-Item $work $dest
    if (Test-Path $old) { Remove-Item -Recurse -Force $old }
} finally {
    if (Test-Path $work) { Remove-Item -Recurse -Force $work }
}

Write-Host "入れました: $dest"
Write-Host "MangaReeda を起動し直すと、設定画面の「プラグイン」で RapidOCR を選べます"
