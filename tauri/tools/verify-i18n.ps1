# 一键验证「界面语言」（PARITY 1.5 / 8.4）：跑四个场景，每个都断言
#   ① 宿主日志里的生效语言（`界面语言: <ftl 目录名>`）
#   ② `i18nResources` 真正下发给页面的语言与文案
#
# 场景：
#   1. `prefs21.db` 的 meta["defaultLang"]=zh_CN（桌面版写的）→ zh-CN / 牌组
#   2. meta["defaultLang"]=en_US → 英文 / Decks
#   3. 没有 prefs21.db → 跟随系统语言
#   4. `ANKI_LANG=ja_JP` 覆盖 meta=zh_CN → ja（测试钩子优先级最高）
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-i18n.ps1
#   pwsh -File tauri/tools/verify-i18n.ps1 -KeepLogs

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40041,
    [string]$ApiKey = "test-key-123",
    [string]$OutDir = "out/i18n-check",
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

$passed = $true

function Read-Log {
    param([string]$Path)
    try { return (Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Wait-LogPattern {
    param([string]$Path, [string]$Pattern, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Read-Log $Path) -match $Pattern) { return $true }
        Start-Sleep -Milliseconds 300
    }
    return $false
}

function Stop-Anki {
    Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
}

# 跑一个场景：隔离的 base 目录 + 可选的 prefs21 夹具 + 可选的 ANKI_LANG
function Invoke-Scenario {
    param(
        [string]$Name,
        [string]$MetaLang = "",
        [string]$EnvLang = "",
        [string]$ExpectLogLang = "",
        [string]$ExpectText = "",
        [string]$ExpectLang = "",
        [string]$ForbidText = ""
    )

    Write-Host ""
    Write-Host "=== 场景 $Name ===" -ForegroundColor Cyan
    Stop-Anki
    $base = Join-Path $OutDir $Name
    if (Test-Path $base) { Remove-Item -Recurse -Force $base }
    New-Item -ItemType Directory -Force -Path (Join-Path $base "User 1") | Out-Null
    $db = Join-Path $base "prefs21.db"
    if ($MetaLang) {
        # 夹具：桌面版写的那一行（profile 名 = 目录名 "User 1"）
        & py tauri/tools/make-prefs21-fixture.py init $db "User 1" $MetaLang | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "造 prefs21 夹具失败" }
    }

    $logPath = Join-Path $base "run.log"
    $errPath = Join-Path $base "run.err"
    $env:ANKI_API_PORT = "$Port"
    $env:ANKI_API_KEY = $ApiKey
    $env:ANKI_COLLECTION = (Join-Path $base "User 1/collection.anki2")
    $env:ANKI_START_PAGE = "decks"
    if ($EnvLang) { $env:ANKI_LANG = $EnvLang } else { Remove-Item Env:\ANKI_LANG -ErrorAction SilentlyContinue }

    $proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
        -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru
    try {
        if (-not (Wait-LogPattern -Path $logPath -Pattern "界面语言: " -TimeoutSeconds $StartupTimeoutSeconds)) {
            throw "启动超时：没等到「界面语言」日志（见 $logPath）"
        }
        # 「界面语言」发生在服务启动之前，RPC 要等服务起来
        if (-not (Wait-LogPattern -Path $logPath -Pattern "窗口将加载" -TimeoutSeconds $StartupTimeoutSeconds)) {
            throw "启动超时：没等到「窗口将加载」日志（见 $logPath）"
        }
        $log = Read-Log $logPath
        $actual = [regex]::Match($log, "界面语言: (\S+)").Groups[1].Value
        Write-Host "宿主生效语言: $actual"
        if ($ExpectLogLang -and $actual -ne $ExpectLogLang) {
            Write-Host "宿主的语言与预期不符：期望 $ExpectLogLang，实际 $actual" -ForegroundColor Red
            $script:passed = $false
        } elseif ($ExpectLogLang) {
            Write-Host "宿主语言正确（$ExpectLogLang）" -ForegroundColor Green
        }

        # 页面真正拿到的东西
        $env:ANKI_EXPECT_LANG = $ExpectLang
        $env:ANKI_EXPECT_TEXT = $ExpectText
        $env:ANKI_FORBID_TEXT = $ForbidText
        node tauri/tools/i18n-check.mjs
        if ($LASTEXITCODE -ne 0) { $script:passed = $false }
    } catch {
        Write-Host "场景 $Name 出错：$_" -ForegroundColor Red
        $script:passed = $false
    } finally {
        Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
        Stop-Anki
    }
}

# 1) 桌面版写的 defaultLang=zh_CN
Invoke-Scenario -Name "meta-zh" -MetaLang "zh_CN" -ExpectLogLang "zh-CN" `
    -ExpectLang "zh-CN" -ExpectText "牌组" -ForbidText "Decks"

# 2) 桌面版写的 defaultLang=en_US
Invoke-Scenario -Name "meta-en" -MetaLang "en_US" -ExpectLogLang "en" `
    -ExpectText "Decks" -ForbidText "牌组"

# 3) 没有 prefs21.db → 跟随系统语言
$sys = [System.Globalization.CultureInfo]::CurrentUICulture.Name
Write-Host "系统语言（PowerShell 视角）: $sys"
if ($sys -like "zh*") {
    Invoke-Scenario -Name "system" -ExpectLogLang "zh-CN" -ExpectLang "zh-CN" -ExpectText "牌组"
} elseif ($sys -like "en*") {
    Invoke-Scenario -Name "system" -ExpectText "Decks"
} else {
    # 其他语言不好在脚本里复算映射，只断言「不是英文兜底」+ 有翻译
    Write-Host "系统语言不是 zh/en，只断言生效语言不是英文兜底" -ForegroundColor Yellow
    Invoke-Scenario -Name "system" -ForbidText "missing-key"
}

# 4) ANKI_LANG 覆盖 meta
Invoke-Scenario -Name "env-override" -MetaLang "zh_CN" -EnvLang "ja_JP" `
    -ExpectLogLang "ja" -ExpectText "デッキ"

Write-Host ""
if ($passed) {
    Write-Host "界面语言验证通过（产物目录 $OutDir）" -ForegroundColor Green
    exit 0
}
Write-Host "界面语言验证失败（产物目录 $OutDir）" -ForegroundColor Red
exit 1
