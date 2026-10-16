# 一键验证「首选项」页面（PARITY 3.3 / 8.1）：
#   1. 用隔离的测试集合启动外壳，通过桥接命令打开首选项窗口（`preferences`）
#   2. 截取**真实窗口**渲染（`preferences.png`）
#   3. 跑 `prefs-check.mjs`：七个分页的结构 + 每页代表性控件的「改 → RPC 复查 → 还原」
#
# 前置：`cd tauri; cargo build` + `corepack yarn build`（页面产物在 out/sveltekit）。
#
# 用法：
#   pwsh -File tauri/tools/verify-prefs.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40061,
    [string]$ApiKey = "test-key-123",
    [string]$OutDir = "out/prefs-check",
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

# 隔离的 base：这个检查会改配置（脚本自己会还原，但独立目录更稳）
$baseDir = Join-Path $OutDir "base"
if (Test-Path $baseDir) { Remove-Item -Recurse -Force $baseDir }
New-Item -ItemType Directory -Force -Path (Join-Path $baseDir "User 1") | Out-Null
& py tauri/tools/make-prefs21-fixture.py init (Join-Path $baseDir "prefs21.db") "User 1" "zh_CN" | Out-Null

$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) { if (Test-Path $path) { Remove-Item $path -Force } }

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $baseDir "User 1/collection.anki2")
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = "zh-CN"

Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

function Read-Log {
    try { return (Get-Content $logPath -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Wait-LogPattern {
    param([string]$Pattern, [int]$TimeoutSeconds = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Read-Log) -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Send-Bridge {
    param([string]$Command, [string]$Window = "main")
    $cmdPath = Join-Path $OutDir "bridge.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::UTF8.GetBytes($Command))
    curl.exe -s -o NUL -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
}

$passed = $true
Write-Host "启动外壳（隔离集合，起始页 decks）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru
try {
    if (-not (Wait-LogPattern -Pattern "窗口将加载" $StartupTimeoutSeconds)) {
        throw "启动超时（见 $logPath）"
    }

    Send-Bridge "preferences"
    if (-not (Wait-LogPattern -Pattern "打开窗口: preferences ->" 20)) {
        Write-Host "首选项窗口没有打开" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "首选项窗口已打开" -ForegroundColor Green
        Start-Sleep -Seconds 3
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "preferences.png") `
            -WaitSeconds 20 -TitleLike "设置"
        if (-not (Test-Path (Join-Path $OutDir "preferences.png"))) {
            Write-Host "没有截到首选项窗口（标题里没有「设置」？）" -ForegroundColor Red
            $passed = $false
        }
    }

    $env:ANKI_SHOT_DIR = $OutDir
    node tauri/tools/prefs-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 6.8：写 JSON null 必须**从 pickle 里删掉键**（不是写一个 None 进去）。
    # 直接 dump `_global` 那一行来验：临时键不在、而别的键还在（对照
    # `profiles.py:721-722` 的 `profile.pop(key, None)`）。这一步只能在这里做 ——
    # RPC 读回 null 区分不出"删了"与"写了 null"。
    # 注意要把输出拼成一个字符串再匹配：`-notmatch` 对**数组**是"过滤"，只要有一行不匹配
    # 整个条件就为真（第一次写就踩了这个坑）
    $dumpText = (& py tauri/tools/make-prefs21-fixture.py show (Join-Path $baseDir "prefs21.db") "_global" 2>&1 | Out-String)
    if ($dumpText -match "tauriNullProbe") {
        Write-Host "写 null 之后键还留在 prefs21.db 里（应该删掉）" -ForegroundColor Red
        $passed = $false
    } elseif ($dumpText -notmatch '"defaultLang"') {
        Write-Host "dump 里看不到对照键，无法确认 dump 有效：$dumpText" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "写 null = 删键（prefs21.db 里已经没有那个键）" -ForegroundColor Green
    }

    if ((Read-Log) -match "ERROR") {
        Write-Host "运行日志里有 ERROR（见 $logPath）" -ForegroundColor Red
        $passed = $false
    }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
    Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
if ($passed) {
    Write-Host "首选项验证通过（产物目录 $OutDir）" -ForegroundColor Green
    exit 0
}
Write-Host "首选项验证失败（产物目录 $OutDir）" -ForegroundColor Red
exit 1
