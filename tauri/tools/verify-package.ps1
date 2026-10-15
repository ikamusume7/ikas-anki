# 验证**打出来的目录**能跑（M4 打包的第一半）。
#
# 与 verify-decks.ps1 同一套隔离集合与钩子，但：
#   - 跑 `package.ps1` 打出 `out/package/anki-tauri/`（exe + web/ + sveltekit/）；
#   - 用 `ANKI_RESOURCE_DIR` **强制打包模式**（在源码树里跑时默认会走仓库模式，
#     见 `tauri/src/paths.rs`），也就是真的从打包目录读 web/ 与 sveltekit/；
#   - 断言：
#       1. 日志里是"以打包模式运行（ANKI_RESOURCE_DIR）"；
#       2. 主窗口加载 /decks/ 且页面报到（`domDone`）—— 说明 SvelteKit 资源齐全；
#       3. `/decks/` 页面与 legacy 资源（`/imgs/...`）都能从打包目录取到；
#       4. 跑一遍 `decks-check.mjs`（完整页面交互），证明资源不缺东西。
#
# 用法：
#   pwsh -File tauri/tools/verify-package.ps1                 # release 产物
#   pwsh -File tauri/tools/verify-package.ps1 -Profile debug   # 快速跑一遍

[CmdletBinding()]
param(
    [ValidateSet("release", "debug")]
    [string]$Profile = "release",
    [int]$Port = 40041,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/package-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$pkgDir = Join-Path $root "out/package/anki-tauri"
$exePath = Join-Path $pkgDir "anki-tauri.exe"

# 先收掉可能还在跑的旧实例，再打一遍包（-SkipBuild：二进制已经由 cargo 构建过）
Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

Write-Host "打可分发目录（$Profile）…"
& pwsh -File (Join-Path $PSScriptRoot "package.ps1") -Profile $Profile -SkipBuild
if ($LASTEXITCODE -ne 0) { throw "打包失败" }
if (-not (Test-Path $exePath)) { throw "找不到 $exePath" }

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = $Lang
$env:ANKI_SAVE_PATH = (Join-Path $root $OutDir)
# 原生"打开文件"对话框会**阻塞**（没有钩子时点「导入文件」会弹出真的对话框，整条链路
# 就卡住了），所以这里也给一个不存在的路径：`import` 命令照样发出，宿主只是报错
$env:ANKI_OPEN_PATH = (Join-Path $root "$OutDir/nonexistent.apkg")
$env:ANKI_BACKUP_NOW = "1"
$env:ANKI_RESTORE_PATH = "newest"
$env:ANKI_SYNC_TICK_SECONDS = "2"
# **关键**：强制打包模式，资源从 out/package/anki-tauri 读
$env:ANKI_RESOURCE_DIR = $pkgDir
Get-ChildItem (Join-Path $root "$OutDir/*") -File -Include "*.apkg", "*.colpkg", "*.txt" `
    -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

function Read-Log {
    param([string]$Path)
    try { return (Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Wait-LogPattern {
    param([string]$Pattern, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Read-Log $logPath) -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

Write-Host "启动打包后的 $exePath（端口 $Port，资源 $pkgDir）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $pkgDir `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "以打包模式运行（ANKI_RESOURCE_DIR）" $StartupTimeoutSeconds)) {
        Write-Host "没有以打包模式启动（详见 $logPath）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "以打包模式运行（资源目录 = out/package/anki-tauri）" -ForegroundColor Green
    }

    if (-not (Wait-LogPattern "窗口将加载: http://127.0.0.1:$Port/decks" $StartupTimeoutSeconds)) {
        Write-Host "打包产物没有加载牌组列表页" -ForegroundColor Red
        $passed = $false
    }
    if (-not (Wait-LogPattern "bridge 命令\[main\]: domDone" 30)) {
        Write-Host "牌组列表页面没有报到（SvelteKit 资源可能不全）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "SvelteKit 页面从打包目录加载成功" -ForegroundColor Green
    }

    # 页面与 legacy 资源各取一次，确认两条资源路径都在包里
    try {
        $page = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/decks/" -UseBasicParsing
        if ($page.StatusCode -eq 200 -and $page.Content -match "_app/immutable") {
            Write-Host "GET /decks/ → 200 且是 SvelteKit 产物" -ForegroundColor Green
        } else {
            Write-Host "GET /decks/ 内容不像 SvelteKit 产物（$($page.StatusCode)）" -ForegroundColor Red
            $passed = $false
        }
    } catch {
        Write-Host "GET /decks/ 失败：$_" -ForegroundColor Red
        $passed = $false
    }
    try {
        # `/favicon.ico` 是走内置资源那条路的（`assets.rs` 把它映射到 `<web>/imgs/favicon.ico`），
        # 因此能证明打包目录里的 **legacy `web/`** 也在（`/imgs/...` 那条 URL 被当成集合媒体）
        $asset = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/favicon.ico" -UseBasicParsing
        if ($asset.StatusCode -eq 200 -and $asset.RawContentLength -gt 0) {
            Write-Host "GET /favicon.ico → 200（legacy web/ 在包里）" -ForegroundColor Green
        } else {
            Write-Host "GET legacy 资源失败（$($asset.StatusCode)）" -ForegroundColor Red
            $passed = $false
        }
    } catch {
        Write-Host "GET legacy 资源失败：$_" -ForegroundColor Red
        $passed = $false
    }

    # 上一次跑崩了会留下临时牌组，而 decks-check 按名字前缀找自己那份 —— 留着会让它
    # 认错牌组（「折叠父牌组后子牌组消失」就是这么挂的）。开跑前先清一遍
    Write-Host "清理上次可能残留的临时牌组…"
    node tauri/tools/cleanup-test-decks.mjs | Out-Null

    Write-Host "跑 decks-check.mjs（完整页面交互，验证资源不缺）…"
    node tauri/tools/decks-check.mjs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "decks-check.mjs 在打包产物上有失败项" -ForegroundColor Red
        $passed = $false
    }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    if ($proc -and -not $proc.HasExited) { Stop-Process -Id $proc.Id -Force }
}

if ($passed) {
    Write-Host "打包产物验证通过（$Profile）" -ForegroundColor Green
    exit 0
} else {
    Write-Host "打包产物验证失败（详见 $logPath）" -ForegroundColor Red
    exit 1
}
