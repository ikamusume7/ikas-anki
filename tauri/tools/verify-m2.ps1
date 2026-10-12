# 一键验证 M2 的平台能力：剪贴板文件列表、TTS 合成、录音闭环。
#
# 串起三件事：
#   1. 用隔离集合启动外壳，并额外开一个窗口（顺带检查多窗口下的命令归属）
#   2. 往剪贴板放一个真实文件，让 `text/uri-list` 有内容可读
#   3. 跑 tauri/tools/m2-check.mjs 做端到端断言
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（out/rust/debug/anki-tauri.exe 存在）。
#
# 用法：
#   pwsh -File tauri/tools/verify-m2.ps1

[CmdletBinding()]
param(
    [string]$Exe = "out/rust/debug/anki-tauri.exe",
    [int]$Port = 40016,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/m2-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$exePath = Join-Path $root $Exe
if (-not (Test-Path $exePath)) {
    throw "找不到可执行文件：$exePath（请先按 TAURI_HANDOVER.md §2 构建）"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

# 只终止本工具启动的实例，避免旧进程占用 exe 或端口
Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "reviewer"
$env:ANKI_LANG = $Lang
# 多开一个窗口：它的页面加载时会发 domDone，用来验证命令归属（应被忽略）
$env:ANKI_OPEN_ROUTES = "deck-options/1"

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

# 剪贴板里放两个真实存在的文件：`readClipboard` 的 text/uri-list 才有内容可读。
# 第二个刻意带空格，用来验证路径转义（空格不转义会让 rslib 的 Url::parse 失败）。
Write-Host "把测试文件放进剪贴板…"
Add-Type -AssemblyName System.Windows.Forms
$clipboardFiles = @(
    (Join-Path $root "tauri/tauri.conf.json"),
    (Join-Path $OutDir "clip board.ini")
)
foreach ($file in $clipboardFiles) {
    if (-not (Test-Path $file)) {
        New-Item -ItemType Directory -Force -Path (Split-Path $file) | Out-Null
        Set-Content -Path $file -Value "clipboard test file" -Encoding utf8
    }
}
$fileList = New-Object System.Collections.Specialized.StringCollection
foreach ($file in $clipboardFiles) { $fileList.Add($file) | Out-Null }
[System.Windows.Forms.Clipboard]::SetFileDropList($fileList)
$env:M2_CLIPBOARD_FILES = ($clipboardFiles -join ";")

Write-Host "启动 $Exe（端口 $Port，集合 $Collection）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "队列: 共" $StartupTimeoutSeconds)) {
        throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $logPath）"
    }
    # 第二个窗口的页面就绪后才谈得上命令归属
    if (-not (Wait-LogPattern "窗口页面已就绪" 30)) {
        throw "附加窗口没有报到（详见 $logPath）"
    }

    node tauri/tools/m2-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 多窗口的命令归属：附加窗口的 domDone 必须被忽略，
    # 否则复习会话会被莫名重启（取队列日志会出现第二次）
    $log = Read-Log $logPath
    if ($log -notmatch "忽略 deck-options 的 domDone") {
        Write-Host "多窗口命令归属：未看到忽略日志" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "多窗口命令归属：附加窗口的 domDone 已被忽略" -ForegroundColor Green
    }
    $queues = ([regex]::Matches($log, "队列: 共")).Count
    if ($queues -ne 1) {
        Write-Host "复习会话被重启了（取队列出现 $queues 次）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "复习会话只启动了一次" -ForegroundColor Green
    }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
Write-Host "日志：$logPath"
if ($passed) {
    Write-Host "M2 平台能力验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "M2 平台能力验证失败" -ForegroundColor Red
exit 1
