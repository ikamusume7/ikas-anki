# 一键验证「命令行带文件启动」（PARITY 6.7 缺口②：双击 `.apkg` 打开）。
#
# 两个阶段：
#   1. 起一个外壳（专用集合，不带参数）→ `make-import-fixture.mjs` 造一份**真的** `.apkg`
#      （临时牌组 + 一张卡 → 导出 → 删掉临时牌组）→ 停外壳；
#   2. 再用**同一个集合**起外壳，但这次把夹具路径当**命令行参数**传进去（等价于双击 .apkg）
#      → `cli-open-check.mjs` 断言：日志有「命令行传入文件」、主窗口导航到了该文件的导入页
#      （`/import-anki-package/<编码路径>`，与拖入同一条路），且没有走拖入那条日志。
#
# 导入页自身（`.filename`、点「导入」、卡片入库）由 `verify-decks.ps1` 里的
# `import-check.mjs` 覆盖，这里不重复。
#
# 前置：`cd tauri; cargo build` 过；改过 `ts/` 要重跑 `yarn build`。
#
# 用法：
#   pwsh -File tauri/tools/verify-cli-open.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40051,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection-cli.anki2",
    [string]$ProfileDir = "out/tauri-test/profile-cli",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/cli-open",
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
$fixture = [System.IO.Path]::GetFullPath((Join-Path $root "$OutDir/import-fixture.apkg"))
if (Test-Path $fixture) { Remove-Item $fixture -Force }

function Read-Log {
    param([string]$Path)
    try { return (Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Wait-LogPattern {
    param([string]$Path, [string]$Pattern, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Read-Log $Path) -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_PROFILE_DIR = (Join-Path $root $ProfileDir)
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = $Lang

$passed = $true
$log1 = Join-Path $OutDir "run1.log"
$log2 = Join-Path $OutDir "run2.log"

try {
    # ---------- 阶段 1：造夹具 ----------
    foreach ($path in @($log1, "$log1.err")) { if (Test-Path $path) { Remove-Item $path -Force } }
    Write-Host "阶段 1：起外壳造 .apkg 夹具…"
    $proc1 = Start-Process -FilePath $exePath -WorkingDirectory $root `
        -RedirectStandardOutput $log1 -RedirectStandardError "$log1.err" -NoNewWindow -PassThru
    if (-not (Wait-LogPattern $log1 "bridge 命令\[main\]: domDone" $StartupTimeoutSeconds)) {
        throw "阶段 1 启动超时（详见 $log1）"
    }
    $env:ANKI_LOG = $log1
    $env:ANKI_IMPORT_FIXTURE = $fixture
    node tauri/tools/make-import-fixture.mjs
    if ($LASTEXITCODE -ne 0) { throw "造夹具失败" }
    if (-not (Test-Path $fixture)) { throw "夹具没有生成：$fixture" }
    if ($proc1 -and -not $proc1.HasExited) { Stop-Process -Id $proc1.Id -Force }
    Start-Sleep -Milliseconds 800

    # ---------- 阶段 2：带参数启动 ----------
    foreach ($path in @($log2, "$log2.err")) { if (Test-Path $path) { Remove-Item $path -Force } }
    Write-Host "阶段 2：把夹具路径当命令行参数启动（等价于双击 .apkg）…"
    $proc2 = Start-Process -FilePath $exePath -WorkingDirectory $root -ArgumentList @($fixture) `
        -RedirectStandardOutput $log2 -RedirectStandardError "$log2.err" -NoNewWindow -PassThru
    if (-not (Wait-LogPattern $log2 "bridge 命令\[main\]: domDone" $StartupTimeoutSeconds)) {
        throw "阶段 2 启动超时（详见 $log2）"
    }
    $env:ANKI_LOG = $log2
    $env:CLI_FIXTURE = $fixture
    node tauri/tools/cli-open-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }
    if ($proc2 -and -not $proc2.HasExited) { Stop-Process -Id $proc2.Id -Force }
} catch {
    Write-Host "验证过程出错：$($_.Exception.Message)" -ForegroundColor Red
    $passed = $false
    Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host "产物目录：$OutDir（run1.log 造夹具，run2.log 命令行启动）"
if ($passed) {
    Write-Host "命令行打开验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "命令行打开验证失败" -ForegroundColor Red
exit 1
