# 一键验证「牌组列表」页面（主窗口默认页面，M3 新写的 `ts/routes/decks`）。
#
# 串起这几件事：
#   1. 用隔离集合启动外壳（ANKI_START_PAGE=decks，与 Qt 主窗口启动状态一致）
#   2. 截首屏真实窗口（decks.png）
#   3. 跑 decks-check.mjs：表头 / 折叠展开 / 点牌组进概览 / 概览「学习」/ 齿轮菜单
#      （选项、重命名、删除）/ 新建牌组 / 底部按钮 / 工具栏
#   4. 宿主断言：牌组选项窗口、导入窗口（`import-anki-package/<编码路径>`）、
#      概览「学习」切到复习页
#   5. `d` / `s` / `study` 桥接命令（等价于全局快捷键）让主窗口来回切页
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build`），
#       且新页面已在 `out/sveltekit` 里（改了 `ts/routes/decks/` 要重跑 `yarn build`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-decks.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40031,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/decks-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$exePath = Join-Path $root $Exe
if (-not (Test-Path $exePath)) {
    throw "找不到可执行文件：$exePath（请先在 tauri/ 下 cargo build）"
}

# 先收掉可能还在跑的旧实例：上一次运行留下的进程会占着 run.log（删除会失败）
Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "decks"
$env:ANKI_LANG = $Lang

# 「导入文件」按钮与 Qt 一样先弹**原生文件对话框**（`importing.py:153-163`），自动化里没法
# 点它，所以用 `ANKI_OPEN_PATH` 顶掉（与 verify-editor 给遮挡夹具用的是同一套办法）。
# 这个变量宿主在启动时就固定了，因此必须在 Start-Process 之前设好；指向的 .apkg 由下面的
# import-check.mjs 现造出来。
$fixtureApkg = [System.IO.Path]::GetFullPath((Join-Path $root "$OutDir/import-fixture.apkg"))
$env:ANKI_OPEN_PATH = $fixtureApkg
$env:ANKI_IMPORT_FIXTURE = $fixtureApkg

function Read-Log {
    param([string]$Path)
    try { return (Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Count-Log {
    param([string]$Pattern)
    $text = Read-Log $logPath
    if (-not $text) { return 0 }
    return ([regex]::Matches($text, $Pattern)).Count
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

function Wait-LogCount {
    param([string]$Pattern, [int]$Minimum, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Count-Log -Pattern $Pattern) -ge $Minimum) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# 桥接命令必须带窗口身份（见 TAURI_HANDOVER.md §5.16）
function Send-Bridge {
    param([string]$Window, [string]$Command)
    $cmdPath = Join-Path $OutDir "cmd.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::ASCII.GetBytes($Command))
    curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
}

Write-Host "启动 $Exe（端口 $Port，集合 $Collection，起始页 decks）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "窗口将加载: http://127.0.0.1:$Port/decks" $StartupTimeoutSeconds)) {
        throw "启动超时：没有看到主窗口加载牌组列表（详见 $logPath）"
    }
    if (-not (Wait-LogPattern "bridge 命令\[main\]: domDone" 30)) {
        throw "牌组列表页面没有报到（详见 $logPath）"
    }
    Write-Host "牌组列表已渲染" -ForegroundColor Green

    Start-Sleep -Seconds 2
    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "decks.png") `
        -WaitSeconds 20 -TitleLike "Anki"

    # 上一次跑崩了会留下临时牌组，而 decks-check 按名字前缀找自己那份 —— 留着会让它
    # 认错牌组（「折叠父牌组后子牌组消失」就是这么挂的）。开跑前先清一遍
    Write-Host "清理上次可能残留的临时牌组…"
    node tauri/tools/cleanup-test-decks.mjs | Out-Null

    # 导入链路（PARITY 3.4 + 6.7）：先造一份 .apkg 夹具并走一遍「页面导入」，
    # 这样下面 decks-check 点「导入文件」时 `ANKI_OPEN_PATH` 指向的文件是真实存在的
    Write-Host "验证导入链路（造夹具 + 页面端到端导入）…"
    $env:SHOT_DIR = $OutDir
    node tauri/tools/import-check.mjs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "import-check.mjs 有失败项" -ForegroundColor Red
        $passed = $false
    }

    Write-Host "跑 decks-check.mjs（Playwright）…"
    node tauri/tools/decks-check.mjs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "decks-check.mjs 有失败项" -ForegroundColor Red
        $passed = $false
    }

    # 宿主侧：菜单里的两个窗口 + 概览「学习」切页
    if (-not (Wait-LogPattern "打开窗口: deck-options ->" 20)) {
        Write-Host "齿轮 → 选项没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "齿轮 → 牌组选项窗口已打开" -ForegroundColor Green
    }
    if (-not (Wait-LogPattern "打开窗口: custom-study ->" 20)) {
        Write-Host "概览「自定义学习」没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "自定义学习窗口已打开" -ForegroundColor Green
    }

    # 自定义学习窗口本身：六个选项 / cram 的列表 / 确定后建出筛选牌组并切成当前牌组
    Write-Host "验证自定义学习窗口（Playwright）…"
    $env:SHOT_DIR = $OutDir
    node tauri/tools/custom-study-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 「导入文件」按钮：宿主先弹原生文件对话框（测试里被 `ANKI_OPEN_PATH` 顶掉），再按
    # 文件名挑页面 —— `.apkg` 应该开 `import-anki-package/<百分号编码的路径>`
    # （对照 `importing.py:137-146` 与 `import_dialog.py:68`）
    if (-not (Wait-LogPattern "打开窗口: import ->" 20)) {
        Write-Host "「导入文件」没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "导入窗口已打开" -ForegroundColor Green
    }
    $importLog = Read-Log $logPath
    if ($importLog -notmatch "import-anki-package/.*import-fixture\.apkg") {
        Write-Host "导入窗口没有开在 .apkg 对应的页面上（见 $logPath）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "导入窗口开在 import-anki-package/<编码路径> 上" -ForegroundColor Green
    }
    if (-not (Wait-LogCount -Pattern "主窗口导航到: http://127.0.0.1:$Port/_anki/legacyPageData" -Minimum 1 -TimeoutSeconds 20)) {
        Write-Host "概览「学习」没有切到复习页" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "概览「学习」切到了复习页" -ForegroundColor Green
    }

    # 全局快捷键（`main.py:1189-1198`）：d → 牌组列表、s → 概览、概览里 s/study → 复习
    Write-Host "发送 decks / overview / study 桥接命令…"
    Send-Bridge -Window "main" -Command "decks" | Out-Null
    if (-not (Wait-LogPattern "主窗口导航到: /decks/" 20)) {
        Write-Host "d（牌组列表）没有切回去" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "d 回到了牌组列表" -ForegroundColor Green
    }

    Send-Bridge -Window "main" -Command "overview" | Out-Null
    if (-not (Wait-LogPattern "主窗口导航到: /decks/\?deck=current" 20)) {
        Write-Host "s（概览）没有切到概览" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "s 切到了当前牌组的概览" -ForegroundColor Green
    }

    $beforeStudy = Count-Log -Pattern "主窗口导航到: http://127.0.0.1:$Port/_anki/legacyPageData"
    Send-Bridge -Window "main" -Command "study" | Out-Null
    if (-not (Wait-LogCount -Pattern "主窗口导航到: http://127.0.0.1:$Port/_anki/legacyPageData" -Minimum ($beforeStudy + 1) -TimeoutSeconds 20)) {
        Write-Host "study 没有开始复习" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "study 开始了复习" -ForegroundColor Green
    }
    # 复习页加载完会报到（domDone）→ 宿主持取队列（`reviewer::start`）。
    # 整个流程里只有这一次会取队列（起始页是牌组列表，不会自动开始复习）。
    if (-not (Wait-LogPattern "队列: 共" 30)) {
        Write-Host "复习页没有取到队列（详见 $logPath）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "复习页已开始取卡" -ForegroundColor Green
    }
} finally {
    if ($proc -and -not $proc.HasExited) {
        Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force
    }
    # 夹具是一次性的（import-check.mjs 刻意留着它，好让「导入文件」有东西可开）
    if (Test-Path $fixtureApkg) { Remove-Item $fixtureApkg -Force }
}

Write-Host ""
if ($passed) {
    Write-Host "牌组列表验证通过" -ForegroundColor Green
    exit 0
} else {
    Write-Host "牌组列表验证失败（详见 $OutDir）" -ForegroundColor Red
    exit 1
}
