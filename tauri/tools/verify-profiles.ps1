# 一键验证「多 profile 管理 + 切换」（PARITY 1.5 / 8.2）：
#   1. 隔离 base 启动 → 打开「切换配置」窗口（菜单命令 `switchProfile`）→ 截真实窗口
#   2. `profiles-check.mjs`：列表 / 当前项 / 新建 / 非法名 / 重命名 / 删除（走宿主命令复查）
#   3. **真切换**：发 `profile:{"action":"switch",...}` → 宿主重启进程 →
#      在新进程的日志里等 `配置: <新名字>`，并断言新的 profile 目录里建出了集合
#
# 前置：`cd tauri; cargo build` + `corepack yarn build`。
#
# 用法：
#   pwsh -File tauri/tools/verify-profiles.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40071,
    [string]$ApiKey = "test-key-123",
    [string]$OutDir = "out/profiles-check",
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

# 隔离 base：两个 profile，切换有目标
$baseDir = Join-Path $OutDir "base"
if (Test-Path $baseDir) { Remove-Item -Recurse -Force $baseDir }
New-Item -ItemType Directory -Force -Path (Join-Path $baseDir "User 1") | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $baseDir "Second") | Out-Null
$db = Join-Path $baseDir "prefs21.db"
& py tauri/tools/make-prefs21-fixture.py init $db "User 1" "zh_CN" | Out-Null
# 第二个 profile 行（夹具脚本只写一个）：用同样的 SQLite + pickle 方式补一行
$addProfile = @'
import pickle, sqlite3, sys
db, name = sys.argv[1], sys.argv[2]
conn = sqlite3.connect(db)
conn.execute("insert or ignore into profiles values (?, ?)", (name, pickle.dumps({}, protocol=4)))
conn.commit()
print("added", name)
'@
$script = Join-Path $OutDir "add-profile.py"
[System.IO.File]::WriteAllText($script, $addProfile, [System.Text.UTF8Encoding]::new($false))
& py $script $db "Second" | Out-Null

$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) { if (Test-Path $path) { Remove-Item $path -Force } }

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
# 用 ANKI_BASE 隔离整个 base（prefs21.db + 各 profile 目录）。
# **不要**用 ANKI_COLLECTION：它优先级高于 meta，会把重启后的子进程也钉在同一个集合上，
# 那样就测不到「按 last_loaded_profile_name 选 profile」。
$env:ANKI_BASE = $baseDir
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = "zh-CN"
# 原生确认框点不了：删除 profile 的确认直接返回「是」
$env:ANKI_CONFIRM_YES = "1"
Remove-Item Env:\ANKI_COLLECTION -ErrorAction SilentlyContinue
Remove-Item Env:\ANKI_PROFILE -ErrorAction SilentlyContinue

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

function Invoke-Profile {
    param([string]$Command)
    $cmdPath = Join-Path $OutDir "profile.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::UTF8.GetBytes($Command))
    return (curl.exe -s -X POST -H "Content-Type: text/plain" `
            -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: main" `
            --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd")
}

$passed = $true
Write-Host "启动外壳（隔离 base，配置 User 1）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru
try {
    if (-not (Wait-LogPattern -Pattern "配置: User 1" $StartupTimeoutSeconds)) {
        throw "启动超时或配置不对（见 $logPath）"
    }
    Write-Host "启动配置是 User 1" -ForegroundColor Green

    # 菜单里的入口：切换配置窗口
    Send-Bridge "switchProfile"
    if (-not (Wait-LogPattern -Pattern "打开窗口: profiles ->" 20)) {
        Write-Host "「切换配置」窗口没有打开" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "切换配置窗口已打开" -ForegroundColor Green
        Start-Sleep -Seconds 3
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "profiles.png") `
            -WaitSeconds 20 -TitleLike "账户"
    }

    # 页面级检查（新建 / 非法名 / 重命名 / 删除）
    node tauri/tools/profiles-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 真切换：宿主会重启进程，新进程继承我们的 stdout（同一个日志文件）
    Write-Host "切到 Second（宿主会重启）…"
    $before = (Read-Log).Length
    Send-Bridge 'profile:{"action":"switch","name":"Second"}'
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    $switched = $false
    while ((Get-Date) -lt $deadline) {
        $text = Read-Log
        if ($text.Length -gt $before -and $text -match "配置: Second") { $switched = $true; break }
        Start-Sleep -Milliseconds 500
    }
    if (-not $switched) {
        Write-Host "切换后没有看到新进程加载 Second（见 $logPath）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "重启后打开的是 Second" -ForegroundColor Green
        $collection = Join-Path $baseDir "Second/collection.anki2"
        if (-not (Wait-LogPattern -Pattern "配置: Second" 5)) { $passed = $false }
        Start-Sleep -Seconds 2
        if (Test-Path $collection) {
            Write-Host "Second 的集合已创建（$collection）" -ForegroundColor Green
        } else {
            Write-Host "Second 的集合没有被创建" -ForegroundColor Red
            $passed = $false
        }
        # 新进程也能正常应答
        $list = Invoke-Profile 'profile:{"action":"list"}'
        if ($list -match '"current":"Second"') {
            Write-Host "新进程报告的当前配置是 Second" -ForegroundColor Green
        } else {
            Write-Host "新进程报告的当前配置不对：$list" -ForegroundColor Red
            $passed = $false
        }
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
    Write-Host "多 profile 验证通过（产物目录 $OutDir）" -ForegroundColor Green
    exit 0
}
Write-Host "多 profile 验证失败（产物目录 $OutDir）" -ForegroundColor Red
exit 1
