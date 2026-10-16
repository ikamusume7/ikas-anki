# 起一个**真的**本地同步服务器（仓库自带的 `anki-sync-server`）。
#
# 为什么需要它：PARITY 6.1 的缺口之一是"`syncCollection` 与媒体同步的成功路径零覆盖"——
# 现有检查用的是只实现 `/sync/hostKey` + `/sync/meta` 的假服务器（decks-check 5i），
# 之后一路 500。真 AnkiWeb 账号在自动化里拿不到，但仓库里就有一个可跑的同步服务器：
# `rslib/sync/main.rs`（`SYNC_BASE`/`SYNC_HOST`/`SYNC_PORT`/`SYNC_USER1=用户名:密码`）。
#
# 用法：
#   pwsh -File tauri/tools/sync-server.ps1 -Port 40011 -Base out/tauri-test/sync-server
#   pwsh -File tauri/tools/sync-server.ps1 -Stop
#
# 退出码：0 = 服务器已就绪（或已停止）；1 = 启动失败/健康检查超时。

[CmdletBinding()]
param(
    [int]$Port = 40011,
    [string]$Base = "out/tauri-test/sync-server",
    [string]$User = "tauri",
    [string]$Password = "tauri-pass",
    [string]$LogFile = "out/tauri-test/sync-server.log",
    [int]$TimeoutSeconds = 30,
    [switch]$Stop
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$pidFile = Join-Path $root "out/tauri-test/sync-server.pid"
$basePath = if ([System.IO.Path]::IsPathRooted($Base)) { $Base } else { Join-Path $root $Base }

if ($Stop) {
    # 先按 pid 文件杀，再兜底按进程名（自动化里可能被上一次运行留下）
    if (Test-Path $pidFile) {
        $saved = (Get-Content $pidFile -Raw).Trim()
        if ($saved) {
            Get-Process -Id ([int]$saved) -ErrorAction SilentlyContinue | Stop-Process -Force
        }
        Remove-Item $pidFile -Force -ErrorAction SilentlyContinue
    }
    Get-Process -Name "anki-sync-server" -ErrorAction SilentlyContinue | Stop-Process -Force
    Write-Host "同步服务器已停止"
    exit 0
}

$binary = Join-Path $root "target/debug/anki-sync-server.exe"
if (-not (Test-Path $binary)) {
    Write-Host "没有 $binary —— 先跑：cargo build -p anki-sync-server" -ForegroundColor Red
    exit 1
}

New-Item -ItemType Directory -Force -Path $basePath | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $LogFile) | Out-Null

# 端口被占（上一次没清干净）就先收掉
Get-Process -Name "anki-sync-server" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 300

$env:SYNC_BASE = $basePath
$env:SYNC_HOST = "127.0.0.1"
$env:SYNC_PORT = "$Port"
$env:SYNC_USER1 = "$User`:$Password"
$env:RUST_LOG = "anki=info"

$logPath = if ([System.IO.Path]::IsPathRooted($LogFile)) { $LogFile } else { Join-Path $root $LogFile }
$process = Start-Process -FilePath $binary -WorkingDirectory $root -PassThru -NoNewWindow `
    -RedirectStandardOutput $logPath -RedirectStandardError "$logPath.err"
Set-Content -Path $pidFile -Value $process.Id -NoNewline

$deadline = (Get-Date).AddSeconds($TimeoutSeconds)
while ((Get-Date) -lt $deadline) {
    try {
        $response = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/health" -UseBasicParsing -TimeoutSec 2
        if ($response.StatusCode -eq 200) {
            Write-Host "同步服务器已就绪: http://127.0.0.1:$Port （pid $($process.Id)，数据 $basePath）"
            exit 0
        }
    } catch {
        Start-Sleep -Milliseconds 400
    }
}

Write-Host "同步服务器没起来（$TimeoutSeconds 秒内 /health 没返回 200），日志见 $logPath" -ForegroundColor Red
Get-Content $logPath -ErrorAction SilentlyContinue | Select-Object -Last 10 | ForEach-Object { Write-Host "  $_" }
exit 1
