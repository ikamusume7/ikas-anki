# 一键验证复习界面：启动外壳 → 必要时补一张卡片 → 截真实窗口 → 跑布局断言。
#
# 这是「改完复习界面马上确认没坏」的入口。它把三件事串起来：
#   1. 用隔离集合启动 out/rust/debug/anki-tauri.exe（ANKI_START_PAGE=reviewer）
#   2. 截取 WebView2 的**真实渲染**（tauri/tools/capture-window.ps1）
#   3. 用 Playwright 复现宿主注入并断言布局（tauri/tools/visual-check.mjs）
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build` →
#       `tauri/target/debug/anki-tauri.exe`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-reviewer.ps1
#   pwsh -File tauri/tools/verify-reviewer.ps1 -Lang en-US

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40001,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/reviewer-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
# 截真实窗口时用的标题（对照 ftl：`qt-misc-stats` / `preferences-preferences`）
$statsTitle = if ($Lang -like "zh*") { "统计" } else { "Stats" }
$preferencesTitle = if ($Lang -like "zh*") { "设置" } else { "Preferences" }
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

# 发一条桥接命令（模拟页面里的 `pycmd`；`X-Anki-Window` 默认主窗口）
function Send-Bridge {
    param([string]$Command, [string]$Window = "main")
    $cmdPath = Join-Path $OutDir "bridge.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::UTF8.GetBytes($Command))
    curl.exe -s -o NUL -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
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

# 某个片段在日志里出现过几次（要判断"有没有**新的**一条"时必须用它：
# `Wait-LogPattern` 会被旧行骗过）
function Count-Log {
    param([string]$Pattern)
    return ([regex]::Matches((Read-Log $logPath), $Pattern)).Count
}

function Wait-LogCount {
    param([string]$Pattern, [int]$Minimum, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ((Count-Log $Pattern) -ge $Minimum) { return $true }
        Start-Sleep -Milliseconds 400
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
        # 队列空时宿主已经回到概览上的 Congratulations（PARITY 1.6），
        # 所以这里发 `study` 重新进复习页；在概览上发 `domDone` 会被忽略
        Send-Bridge "study"
        if (-not (Wait-LogPattern "渲染完成" 30)) {
            throw "补卡片后复习界面仍未渲染（详见 $logPath）"
        }
    }

    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "window.png") -WaitSeconds 20

    # 顶部工具栏（对照 Qt 主窗口的 TopWebView）：宿主在会话开始时注入
    if (-not (Wait-LogPattern "顶部工具栏已注入" 20)) {
        Write-Host "顶部工具栏没有注入" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "顶部工具栏已注入" -ForegroundColor Green
        Start-Sleep -Milliseconds 800
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "reviewer-toolbar.png") -WaitSeconds 20
    }

    $env:SHOT_DIR = $OutDir
    node tauri/tools/visual-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 复习界面的「更多」/右键菜单：宿主拼数据、页面画菜单
    # （脚本本体 `tauri/assets/reviewer-menu.js`，宿主用 include_str! 引用）
    Write-Host "检查「更多」菜单（UI 脚本 + 宿主动作）…"
    node tauri/tools/reviewer-menu-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 工具栏「同步」（`sync` 桥接命令）走了宿主那条路：凭证与假服务器由
    # reviewer-menu-check.mjs 准备
    if ((Read-Log $logPath) -match "复习界面工具栏: 同步") {
        Write-Host "工具栏「同步」走了宿主的 sync 命令" -ForegroundColor Green
    } else {
        Write-Host "没有看到工具栏同步的宿主日志（见 $logPath）" -ForegroundColor Red
        $passed = $false
    }

    # 菜单检查里的「设置到期日 → 撤销」会把队列清空一次（宿主按 PARITY 1.6 回了概览），
    # 撤销之后主窗口未必停在复习页上；后面要截菜单、开牌组选项/卡片信息两个窗口，
    # 都要求复习页有当前卡片 —— 所以重新进一次复习页（`study` 幂等）
    $renders = Count-Log "渲染完成"
    Send-Bridge "study"
    if (-not (Wait-LogCount "渲染完成" ($renders + 1) 30)) {
        Write-Host "重新进复习页后没有渲染（详见 $logPath）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "重新进复习页并渲染了卡片" -ForegroundColor Green
    }

    # 让真实窗口弹出菜单并截图（菜单是页面里的浮层，窗口截图就能看到）
    Send-Bridge "more"
    Start-Sleep -Milliseconds 900
    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "reviewer-menu.png") -WaitSeconds 20

    # 菜单里的两个窗口入口：牌组选项（既有 deck-options 页面）与卡片信息
    Send-Bridge "rev:options"
    Send-Bridge "rev:cardInfo"
    foreach ($pair in @(@("deck-options", "牌组选项"), @("card-info", "卡片信息"))) {
        if (-not (Wait-LogPattern "打开窗口: $($pair[0]) ->" 20)) {
            Write-Host "菜单里的「$($pair[1])」没有开窗" -ForegroundColor Red
            $passed = $false
        } else {
            Write-Host "菜单 → $($pair[1]) 窗口已打开" -ForegroundColor Green
        }
    }

    # 主窗口的另外两个全局入口（对照 `main.py:1189-1198` 的快捷键表与工具栏）：
    # `t` → 统计（既有 graphs 页面）、`Ctrl+P` → 设置（既有 preferences 页面）
    Send-Bridge "stats"
    if (-not (Wait-LogPattern "打开窗口: stats ->" 20)) {
        Write-Host "按 t（统计）没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "统计图窗口已打开" -ForegroundColor Green
        # 页面报到（domDone）之后还要拉图数据再画图，截早了会拍到空白窗口
        Start-Sleep -Seconds 4
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "graphs.png") `
            -WaitSeconds 20 -TitleLike $statsTitle
    }
    Send-Bridge "preferences"
    if (-not (Wait-LogPattern "打开窗口: preferences ->" 20)) {
        Write-Host "按 Ctrl+P（设置）没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "设置窗口已打开" -ForegroundColor Green
        Start-Sleep -Seconds 2
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "preferences.png") `
            -WaitSeconds 20 -TitleLike $preferencesTitle
    }

    # 复习键盘：页面脚本把按键翻成桥接命令（`tauri/assets/reviewer-keys.js`，宿主用
    # `include_str!` 引用），宿主再执行动作。第 1 半查映射（同一个脚本文件），
    # 第 2 半查宿主侧那条「答案面之外不评分」的守卫。
    # 放在最后：第 2 半会把队列里的卡片答掉，前面的「更多」菜单/卡片信息要用到它。
    Write-Host "检查复习键盘（按键映射 + 宿主守卫）…"
    $env:ANKI_LOG = $logPath
    node tauri/tools/reviewer-keys-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 答完队列的收尾：没有卡片时要回概览并画 Congratulations（不再是硬编码中文提示）。
    # 这个检查自造一个「今天学完」的临时牌组，所以不受前面几步留下的状态影响。
    Write-Host "检查答完队列的收尾（Congratulations）…"
    $env:ANKI_LOG = $logPath
    $env:SHOT_DIR = $OutDir
    node tauri/tools/congrats-check.mjs
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
