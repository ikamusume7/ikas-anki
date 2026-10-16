# 一键验证「主题 / 夜间模式」（PARITY 8.3）：
#   1. 用 meta["theme"]=2 启动 → 日志 `主题: theme=2 night=true` + 真实窗口截图**明显变暗**
#   2. 发 `theme:1` → 日志切到浅色 + 截图**明显变亮**（运行期切换链路）
#   3. 再切回 2 → 又变暗
#   4. 以复习界面启动（legacy 页 + 卡片 body class）→ 深色依然生效
#   5. `theme-check.mjs`：页面级断言两条注入通道（`#night` 与 document-start 注入）与
#      CSS 变量 `--canvas` 的深浅取值
#
# 亮度判据：整屏按 20px 网格采样求平均亮度（0–255）。深色主题下画布是 #2c2c2c（≈44），
# 浅色是 #f5f5f5（≈245），两者差距极大，用 100 / 150 两个阈值足够稳。
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-theme.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40051,
    [string]$ApiKey = "test-key-123",
    [string]$OutDir = "out/theme-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$exePath = Join-Path $root $Exe
if (-not (Test-Path $exePath)) {
    throw "找不到可执行文件：$exePath（请先按 TAURI_HANDOVER.md §2 构建）"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$baseDir = Join-Path $OutDir "base"
if (Test-Path $baseDir) { Remove-Item -Recurse -Force $baseDir }
New-Item -ItemType Directory -Force -Path (Join-Path $baseDir "User 1") | Out-Null
# 夹具：桌面版写的 theme=2（深色）；defaultLang 用中文，日志与文案好认
& py tauri/tools/make-prefs21-fixture.py init (Join-Path $baseDir "prefs21.db") "User 1" "zh_CN" | Out-Null
if ($LASTEXITCODE -ne 0) { throw "造 prefs21 夹具失败" }
# 夹具脚本不写 theme，这里补上（用同一个 SQLite + pickle 格式）
$setTheme = @'
import pickle, sqlite3, sys
db, value = sys.argv[1], int(sys.argv[2])
conn = sqlite3.connect(db)
row = conn.execute("select cast(data as blob) from profiles where name='_global'").fetchone()
data = pickle.loads(row[0])
data["theme"] = value
conn.execute("update profiles set data=? where name='_global'", (pickle.dumps(data, protocol=4),))
conn.commit()
print("theme =", value)
'@
$script = Join-Path $baseDir "set-theme.py"
[System.IO.File]::WriteAllText($script, $setTheme, [System.Text.UTF8Encoding]::new($false))
& py $script (Join-Path $baseDir "prefs21.db") 2 | Out-Null

$passed = $true
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) { if (Test-Path $path) { Remove-Item $path -Force } }

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

# 整屏平均亮度（0–255）：按 20px 网格采样
function Get-MeanLuminance {
    param([string]$Path)
    $bitmap = [System.Drawing.Bitmap]::FromFile($Path)
    try {
        $sum = 0.0
        $count = 0
        for ($x = 0; $x -lt $bitmap.Width; $x += 20) {
            for ($y = 0; $y -lt $bitmap.Height; $y += 20) {
                $color = $bitmap.GetPixel($x, $y)
                $sum += 0.2126 * $color.R + 0.7152 * $color.G + 0.0722 * $color.B
                $count++
            }
        }
        return [math]::Round($sum / [math]::Max($count, 1), 1)
    } finally {
        $bitmap.Dispose()
    }
}

function Send-Bridge {
    param([string]$Command, [string]$Window = "main")
    $cmdPath = Join-Path $OutDir "bridge.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::UTF8.GetBytes($Command))
    curl.exe -s -o NUL -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
}

function Capture {
    param([string]$OutFile, [string]$TitleLike = "")
    # 注意：不要用 $args，它是 PowerShell 的自动变量
    $capture = @{
        ProcessName = "anki-tauri"
        OutFile     = $OutFile
        WaitSeconds = 20
    }
    if ($TitleLike) { $capture.TitleLike = $TitleLike }
    & (Join-Path $PSScriptRoot "capture-window.ps1") @capture | Out-Null
}

function Stop-Anki {
    Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
}

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $baseDir "User 1/collection.anki2")
$env:ANKI_LANG = "zh-CN"
$env:ANKI_START_PAGE = "decks"

Stop-Anki
Write-Host "启动外壳（theme=2，起始页 decks）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru
try {
    if (-not (Wait-LogPattern -Pattern "主题: theme=2 night=true" $StartupTimeoutSeconds)) {
        throw "启动失败：没等到深色主题日志（见 $logPath）"
    }
    Write-Host "深色主题已生效（meta theme=2）" -ForegroundColor Green
    if (-not (Wait-LogPattern -Pattern "窗口将加载" 20)) { throw "窗口没起来" }
    Start-Sleep -Seconds 3
    Capture -OutFile (Join-Path $OutDir "dark.png")
    $dark = Get-MeanLuminance -Path (Join-Path $OutDir "dark.png")
    Write-Host "深色截图平均亮度: $dark"
    if ($dark -ge 100) { Write-Host "深色主题下画面不够暗" -ForegroundColor Red; $passed = $false }

    # 运行期切到浅色（对照 preferences.py → mw.set_theme）
    Send-Bridge "theme:1"
    if (-not (Wait-LogPattern -Pattern "主题: theme=1 night=false" 20)) {
        Write-Host "运行期切换主题没有生效" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "运行期切到浅色" -ForegroundColor Green
        Start-Sleep -Seconds 2
        Capture -OutFile (Join-Path $OutDir "light.png")
        $light = Get-MeanLuminance -Path (Join-Path $OutDir "light.png")
        Write-Host "浅色截图平均亮度: $light"
        if ($light -le 150) { Write-Host "浅色主题下画面不够亮" -ForegroundColor Red; $passed = $false }
        if ($light -le $dark) { Write-Host "浅色没有比深色更亮（切换无效？）" -ForegroundColor Red; $passed = $false }
    }

    # 再切回深色
    Send-Bridge "theme:2"
    if (-not (Wait-LogPattern -Pattern "主题: theme=2 night=true" 20)) {
        Write-Host "切回深色没有生效" -ForegroundColor Red
        $passed = $false
    } else {
        Start-Sleep -Seconds 2
        Capture -OutFile (Join-Path $OutDir "dark-again.png")
        $again = Get-MeanLuminance -Path (Join-Path $OutDir "dark-again.png")
        Write-Host "切回深色后平均亮度: $again"
        if ($again -ge 100) { Write-Host "切回深色后画面不够暗" -ForegroundColor Red; $passed = $false }
    }

    # 页面级断言（两条注入通道 + CSS 变量）
    $env:ANKI_SHOT_DIR = $OutDir
    node tauri/tools/theme-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
    Stop-Anki
}

# 复习界面（legacy 页）在深色下也要生效：卡片 body class 由宿主拼全（PARITY 8.3 的坑）
Write-Host ""
Write-Host "检查复习界面（legacy 页）在深色下的表现…"
$env:ANKI_START_PAGE = "reviewer"
$reviewerBase = Join-Path $OutDir "reviewer"
if (Test-Path $reviewerBase) { Remove-Item -Recurse -Force $reviewerBase }
New-Item -ItemType Directory -Force -Path (Join-Path $reviewerBase "User 1") | Out-Null
& py tauri/tools/make-prefs21-fixture.py init (Join-Path $reviewerBase "prefs21.db") "User 1" "zh_CN" | Out-Null
& py $script (Join-Path $reviewerBase "prefs21.db") 2 | Out-Null
$env:ANKI_COLLECTION = (Join-Path $reviewerBase "User 1/collection.anki2")
$logPath2 = Join-Path $OutDir "reviewer.log"
$proc2 = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath2 -RedirectStandardError (Join-Path $OutDir "reviewer.err") -NoNewWindow -PassThru
try {
    $deadline = (Get-Date).AddSeconds($StartupTimeoutSeconds)
    $ok = $false
    while ((Get-Date) -lt $deadline) {
        try { $text = Get-Content $logPath2 -Raw -Encoding utf8 } catch { $text = "" }
        if ($text -match "主题: theme=2 night=true" -and $text -match "队列: 共") { $ok = $true; break }
        Start-Sleep -Milliseconds 300
    }
    if (-not $ok) { throw "复习界面没起来（见 $logPath2）" }
    if ($text -match "队列: 共 0 张") {
        Write-Host "集合为空，补一张卡片…"
        node tauri/tools/seed-card.mjs | Out-Null
        Send-Bridge "domDone"
    }
    Start-Sleep -Seconds 4
    Capture -OutFile (Join-Path $OutDir "reviewer-dark.png")
    $reviewer = Get-MeanLuminance -Path (Join-Path $OutDir "reviewer-dark.png")
    Write-Host "复习界面深色截图平均亮度: $reviewer"
    if ($reviewer -ge 120) {
        Write-Host "复习界面在深色下不够暗（卡片 body class 可能被覆盖了）" -ForegroundColor Red
        $passed = $false
    }
} catch {
    Write-Host "复习界面检查出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc2.Id -ErrorAction SilentlyContinue | Stop-Process -Force
    Stop-Anki
}

Write-Host ""
if ($passed) {
    Write-Host "主题 / 夜间模式验证通过（产物目录 $OutDir）" -ForegroundColor Green
    exit 0
}
Write-Host "主题 / 夜间模式验证失败（产物目录 $OutDir）" -ForegroundColor Red
exit 1
