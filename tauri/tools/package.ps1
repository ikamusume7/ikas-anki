# 打出**可分发目录**（Tauri 的"打包模式"布局），供没有 Tauri CLI / 没网的环境使用：
#
#   out/package/anki-tauri/
#     anki-tauri.exe
#     web/         ← qt/_aqt/data/web（legacy 页面、css、imgs）
#     sveltekit/   ← out/sveltekit（SvelteKit 静态产物）
#     README.txt
#
# 布局必须与 `tauri/src/paths.rs` 的 `for_resource_dir()` 一致
# （`<resources>/web` + `<resources>/sveltekit`）。
#
# 为什么不用 `tauri build`（NSIS/MSI 安装包）：那需要 Tauri CLI 与 WiX/NSIS 的下载
# （都要联网）。`tauri/tauri.conf.json` 里已经把 bundle 配置写好了（`resources` 映射到
# `web/` + `sveltekit/`、`icons/icon.ico`、NSIS 的 currentUser 安装），有网时直接
# `cargo tauri build` 即可 —— 这里手搓同一套布局并让 `verify-package.ps1` 验证它能跑。
#
# 用法：
#   pwsh -File tauri/tools/package.ps1                    # release
#   pwsh -File tauri/tools/package.ps1 -Profile debug      # 快速验证布局

param(
    [ValidateSet("release", "debug")]
    [string]$Profile = "release",
    [switch]$SkipBuild,
    [switch]$SkipWeb
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$webSource = Join-Path $root "qt/_aqt/data/web"
$sveltekitSource = Join-Path $root "out/sveltekit"
$exeName = "anki-tauri.exe"
$exePath = Join-Path $root "tauri/target/$Profile/$exeName"
$outDir = Join-Path $root "out/package/anki-tauri"

if (-not $SkipWeb) {
    if (-not (Test-Path (Join-Path $sveltekitSource "index.html"))) {
        Write-Host "out/sveltekit 还没有：先跑 corepack yarn build" -ForegroundColor Red
        exit 1
    }
    if (-not (Test-Path $webSource)) {
        Write-Host "qt/_aqt/data/web 不存在：先跑 Anki 的 web 构建（just rebuild-web）" -ForegroundColor Red
        exit 1
    }
}

if (-not $SkipBuild) {
    Write-Host "cargo build --profile $Profile …"
    Push-Location (Join-Path $root "tauri")
    try {
        & cargo build --profile $Profile
        if ($LASTEXITCODE -ne 0) { throw "cargo build 失败" }
    } finally {
        Pop-Location
    }
}

if (-not (Test-Path $exePath)) {
    Write-Host "找不到 $exePath" -ForegroundColor Red
    exit 1
}

if (Test-Path $outDir) { Remove-Item $outDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

Copy-Item $exePath (Join-Path $outDir $exeName) -Force
Copy-Item $webSource (Join-Path $outDir "web") -Recurse -Force
Copy-Item $sveltekitSource (Join-Path $outDir "sveltekit") -Recurse -Force

@"
Anki（Tauri 外壳）—— $Profile 构建

  anki-tauri.exe   主程序（Tauri v2）
  web/             legacy 页面与静态资源（原 qt/_aqt/data/web）
  sveltekit/       SvelteKit 静态产物（原 out/sveltekit）

运行：直接双击 anki-tauri.exe。程序默认用**资源目录**（本目录）下的 web/ 与 sveltekit/；
调试时可以用 ANKI_RESOURCE_DIR 指定另一个资源根目录。
集合与 profile 仍放在 %APPDATA%\Anki2（与桌面版一致），可用 ANKI_COLLECTION 覆盖。
"@ | Set-Content -Path (Join-Path $outDir "README.txt") -Encoding utf8

$size = (Get-ChildItem $outDir -Recurse -File | Measure-Object -Property Length -Sum).Sum
Write-Host ("可分发目录已生成: {0}（{1:N1} MB）" -f $outDir, ($size / 1MB)) -ForegroundColor Green
