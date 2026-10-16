# 一键验证时间盒（PARITY 2.6）：两次启动外壳，分别选「结束」与「继续」。
#
# 为什么两次：时间盒到点的「继续 / 结束」是宿主在流程中间弹的原生框，自动化里用测试钩子
# `ANKI_TIMEBOX_CHOICE` 跳过（与 `ANKI_CONFIRM_YES` 同一套路），而钩子是**环境变量**，
# 只能一个进程一种选择。两次都跑同一个 `timebox-check.mjs`，由它按钩子决定断言哪条路径。
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过。
#
# 用法：
#   pwsh -File tauri/tools/verify-timebox.ps1
#   pwsh -File tauri/tools/verify-timebox.ps1 -Lang en-US

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40001,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/timebox-check",
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

function Count-Log {
    param([string]$Path, [string]$Pattern)
    try { $text = Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop } catch { return 0 }
    # 空文件时 `-Raw` 可能给 $null，`[regex]::Matches` 会直接报参数为空
    if ([string]::IsNullOrEmpty($text)) { return 0 }
    return ([regex]::Matches($text, $Pattern)).Count
}

# 跑一次外壳 + 一次检查，返回检查的退出码
function Invoke-TimeboxRun {
    param([string]$Choice, [string]$LogPath)

    foreach ($path in @($LogPath, "$LogPath.err")) {
        if (Test-Path $path) { Remove-Item $path -Force }
    }
    Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

    $env:ANKI_API_PORT = "$Port"
    $env:ANKI_API_KEY = $ApiKey
    $env:ANKI_COLLECTION = (Join-Path $root $Collection)
    # 起始页用复习界面：队列非空时直接开始复习，队列为空时宿主会回概览
    # （PARITY 1.6），两种情况下检查里的 `study` 都能进复习页
    $env:ANKI_START_PAGE = "reviewer"
    $env:ANKI_LANG = $Lang
    $env:ANKI_TIMEBOX_CHOICE = $Choice
    $env:ANKI_LOG = $LogPath
    $env:SHOT_DIR = $OutDir

    Write-Host "启动 $Exe（时间盒选择：$Choice）…"
    $proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
        -RedirectStandardOutput $LogPath -RedirectStandardError "$LogPath.err" -NoNewWindow -PassThru
    try {
        # 外壳起来即可：日志里出现取队列（`reviewer::start`）说明服务与集合都就绪
        $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
        while ((Get-Date) -lt $deadline) {
            if ((Count-Log -Path $LogPath -Pattern "队列: 共") -gt 0) { break }
            Start-Sleep -Milliseconds 500
        }
        if ((Count-Log -Path $LogPath -Pattern "队列: 共") -eq 0) {
            throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $LogPath）"
        }

        # `Out-Host` 很关键：函数的**所有管道输出**都会变成返回值，node 的输出如果走管道，
        # `$code` 就会变成一个数组（里面混着检查输出与退出码），判断必然失败
        & node tauri/tools/timebox-check.mjs | Out-Host
        return $LASTEXITCODE
    } finally {
        Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
        Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
    }
}

$passed = $true
foreach ($choice in @("finish", "continue")) {
    Write-Host ""
    Write-Host "== 时间盒：$choice ==" -ForegroundColor Cyan
    $code = Invoke-TimeboxRun -Choice $choice -LogPath (Join-Path $OutDir "run-$choice.log")
    if ($code -ne 0) { $passed = $false }
}

Write-Host ""
Write-Host "产物目录：$OutDir（timebox-finish.png 为「结束」后的牌组列表，timebox-continue.png 为继续复习）"
if ($passed) {
    Write-Host "时间盒验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "时间盒验证失败" -ForegroundColor Red
exit 1
