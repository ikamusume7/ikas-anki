# 一键验证「同步的成功路径」（PARITY 6.1 缺口①）。
#
# 串起这几件事：
#   1. 起仓库自带的**真**同步服务器（`anki-sync-server`，见 `sync-server.ps1`）
#   2. 用**专用**集合 + 专用 profile 起外壳（起始页 decks；sync 的端点在 profile 的
#      `customSyncUrl` 里，所以不能和其它脚本共用 profile）
#   3. 跑 sync-check.mjs：登录打到真 `/sync/hostKey` → 服务器无集合时弹「上传/下载」→
#      选上传 → 断言服务器侧真的出现 `SYNC_BASE/<user>/collection.anki2`、服务器日志有
#      `/sync/upload`、客户端状态回到非 error
#   4. 收尾停掉同步服务器
#
# 前置：`cargo build -p anki-sync-server` 与 `cd tauri; cargo build` 都做过；
#       改过 `ts/routes/decks/` 要重跑 `yarn build`。
#
# 用法：
#   pwsh -File tauri/tools/verify-sync.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40041,
    [int]$SyncPort = 40011,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection-sync.anki2",
    [string]$ProfileDir = "out/tauri-test/profile-sync",
    [string]$SyncBase = "out/tauri-test/sync-server",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/sync-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$exePath = Join-Path $root $Exe
if (-not (Test-Path $exePath)) {
    throw "找不到可执行文件：$exePath（请先在 tauri/ 下 cargo build）"
}

Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
$syncLogPath = Join-Path $OutDir "sync-server.log"
foreach ($path in @($logPath, $errPath, $syncLogPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

# 0. 清掉上一次的状态：服务器侧的集合（服务器开着时它占着文件，删不掉，所以必须在起之前删）、
#    本地集合、以及 profile（`customSyncUrl`/`syncKey` 存在 profile 里，要一个干净起点）
# 先把可能还在跑的旧同步服务器停掉 —— 它占着服务器侧集合文件，不先停就删不掉
& pwsh -File (Join-Path $root "tauri/tools/sync-server.ps1") -Stop | Out-Null
$syncUserDir = Join-Path $root (Join-Path $SyncBase "tauri")
Get-ChildItem (Join-Path $syncUserDir "collection.anki2*") -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
if (Test-Path (Join-Path $root $Collection)) { Remove-Item (Join-Path $root $Collection) -Force }
Get-ChildItem (Join-Path $root "$Collection-*") -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
if (Test-Path (Join-Path $root $ProfileDir)) { Remove-Item (Join-Path $root $ProfileDir) -Recurse -Force }

# 1. 先起真同步服务器（它是独立的进程，与外壳无关）
Write-Host "启动本地同步服务器（端口 $SyncPort，数据 $SyncBase）…"
& pwsh -File (Join-Path $root "tauri/tools/sync-server.ps1") -Port $SyncPort `
    -Base $SyncBase -LogFile $syncLogPath -User "tauri" -Password "tauri-pass"
if ($LASTEXITCODE -ne 0) {
    throw "同步服务器没起来（详见 $syncLogPath）"
}

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_PROFILE_DIR = (Join-Path $root $ProfileDir)
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = $Lang

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

$proc = $null
$passed = $true
try {
    Write-Host "启动 $Exe（端口 $Port，集合 $Collection，profile $ProfileDir）…"
    $proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
        -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

    if (-not (Wait-LogPattern "窗口将加载: http://127.0.0.1:$Port/decks" $StartupTimeoutSeconds)) {
        throw "启动超时：没有看到主窗口加载牌组列表（详见 $logPath）"
    }
    if (-not (Wait-LogPattern "bridge 命令\[main\]: domDone" 30)) {
        throw "牌组列表页面没有报到（详见 $logPath）"
    }
    Write-Host "牌组列表已渲染" -ForegroundColor Green

    $env:ANKI_LOG = $logPath
    $env:SYNC_PORT = "$SyncPort"
    $env:SYNC_BASE = (Join-Path $root $SyncBase)
    $env:SYNC_LOG = $syncLogPath
    $env:SYNC_USER = "tauri"
    $env:SYNC_PASSWORD = "tauri-pass"
    Write-Host "检查同步成功路径（登录 / 上传 / 服务器侧产物）…"
    node tauri/tools/sync-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }
} catch {
    Write-Host "验证过程出错：$($_.Exception.Message)" -ForegroundColor Red
    $passed = $false
} finally {
    if ($proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
    & pwsh -File (Join-Path $root "tauri/tools/sync-server.ps1") -Stop | Out-Host
}

Write-Host "产物目录：$OutDir（run.log 是外壳日志，sync-server.log 是同步服务器日志）"
if ($passed) {
    Write-Host "同步验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "同步验证失败" -ForegroundColor Red
exit 1
