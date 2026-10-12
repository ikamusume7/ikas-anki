# 一键验证复习界面：启动外壳 → 必要时补一张卡片 → 截真实窗口 → 跑布局断言。
#
# 这是「改完复习界面马上确认没坏」的入口。它把三件事串起来：
#   1. 用隔离集合启动 out/rust/debug/anki-tauri.exe（ANKI_START_PAGE=reviewer）
#   2. 截取 WebView2 的**真实渲染**（tauri/tools/capture-window.ps1）
#   3. 用 Playwright 复现宿主注入并断言布局（tauri/tools/visual-check.mjs）
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（out/rust/debug/anki-tauri.exe 存在）。
#
# 用法：
#   pwsh -File tauri/tools/verify-reviewer.ps1
#   pwsh -File tauri/tools/verify-reviewer.ps1 -Lang en-US

[CmdletBinding()]
param(
    [string]$Exe = "out/rust/debug/anki-tauri.exe",
    [int]$Port = 40001,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/reviewer-check",
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
# 先判断存在性再删：某些环境的删除钩子会拒绝对不存在路径的操作并直接报错
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

function Read-Log {
    param([string]$Path)
    # 进程仍在写日志时读取会偶发失败，交给轮询重试
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

Write-Host "启动 $Exe（端口 $Port，集合 $Collection）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "队列: 共" $StartupTimeoutSeconds)) {
        throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $logPath）"
    }

    # 新建集合没有卡片，复习界面只会显示「已无待复习卡片」，那样验证不到卡片渲染
    if ((Read-Log $logPath) -match "队列: 共 0 张") {
        Write-Host "集合为空，写入一张测试卡片…"
        node tauri/tools/seed-card.mjs
        $cmdPath = Join-Path $OutDir "pycmd.dat"
        [System.IO.File]::WriteAllBytes(
            $cmdPath,
            [System.Text.Encoding]::UTF8.GetBytes("domDone")
        )
        curl.exe -s -o NUL -X POST -H "Content-Type: text/plain" `
            -H "Authorization: Bearer $ApiKey" --data-binary "@$cmdPath" `
            "http://127.0.0.1:$Port/_anki/pycmd"
        if (-not (Wait-LogPattern "渲染完成" 30)) {
            throw "补卡片后复习界面仍未渲染（详见 $logPath）"
        }
    }

    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "window.png") -WaitSeconds 20

    $env:SHOT_DIR = $OutDir
    node tauri/tools/visual-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
Write-Host "产物目录：$OutDir（window.png 为真实窗口渲染，visual-check.png 为 Playwright 渲染）"
if ($passed) {
    Write-Host "复习界面验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "复习界面验证失败" -ForegroundColor Red
exit 1
