# 一键验证「添加卡片」窗口（M3 的 AddCards，`editor/?mode=add`）。
#
# 串起这几件事：
#   1. 用隔离集合启动外壳（ANKI_START_PAGE=reviewer），必要时补一张卡片
#   2. 发复习界面的 `add` 桥接命令（等价于 `a` 快捷键）→ 断言窗口打开且**真的注入了新建 note**
#   3. 截取窗口真实渲染
#   4. 跑 addcards-check.mjs：默认牌组/笔记类型、填字段、点「添加」落库、
#      「有输入 ⇒ 要确认 / 无输入 ⇒ 直接关」的页面侧判定
#   5. 宿主侧关闭契约：`closeAddCards(val=false)` 必须直接关窗；
#      `closeAddCards(val=true)` 必须停在「放弃输入？」确认框上、窗口保持打开
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-addcards.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40026,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/addcards-check",
    [int]$StartupTimeoutSeconds = 60
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
Set-Location $root

$exePath = Join-Path $root $Exe
if (-not (Test-Path $exePath)) {
    throw "找不到可执行文件：$exePath（请先在 tauri/ 下 cargo build）"
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$logPath = Join-Path $OutDir "run.log"
$errPath = Join-Path $OutDir "run.err"
foreach ($path in @($logPath, $errPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "reviewer"
$env:ANKI_LANG = $Lang

function Read-Log {
    param([string]$Path)
    try { return (Get-Content $Path -Raw -Encoding utf8 -ErrorAction Stop) } catch { return "" }
}

function Count-Log {
    param([string]$Pattern)
    $text = Read-Log $logPath
    # 日志文件刚创建时是空的：`Get-Content -Raw` 会返回 $null，`[regex]::Matches` 不接受
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

# 同一个模式会出现多次（例如第二次开窗），所以要按次数等
function Wait-LogCount {
    param([string]$Pattern, [int]$Minimum, [int]$TimeoutSeconds)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $found = Count-Log -Pattern $Pattern
        if ($found -ge $Minimum) { return $true }
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# 桥接命令必须带窗口身份（见 TAURI_HANDOVER.md §5.16）；复习界面的命令来自主窗口
function Send-Bridge {
    param([string]$Window, [string]$Command)
    $cmdPath = Join-Path $OutDir "cmd.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::ASCII.GetBytes($Command))
    curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
}

function Send-Rpc {
    param([string]$Method, [byte[]]$Body = @())
    $bodyPath = Join-Path $OutDir "rpc.bin"
    [System.IO.File]::WriteAllBytes($bodyPath, $Body)
    curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: application/binary" `
        -H "Authorization: Bearer $ApiKey" --data-binary "@$bodyPath" `
        "http://127.0.0.1:$Port/_anki/$Method"
}

# 会阻塞的 RPC：`closeAddCards(val=true)` 要一直等到用户点掉确认框才返回。
# 用 `--max-time` 让请求方到点自己放弃 —— 断开请求不会让服务端的确认框消失
# （确认框在阻塞线程里），因此仍然可以断言「窗口保持打开」。
function Send-RpcAndGiveUp {
    param([string]$Method, [byte[]]$Body = @(), [int]$TimeoutSeconds = 8)
    $bodyPath = Join-Path $OutDir "rpc-slow.bin"
    [System.IO.File]::WriteAllBytes($bodyPath, $Body)
    # 非零退出码（28 = 超时）是预期行为
    & curl.exe -s -o NUL --max-time $TimeoutSeconds -X POST `
        -H "Content-Type: application/binary" -H "Authorization: Bearer $ApiKey" `
        --data-binary "@$bodyPath" "http://127.0.0.1:$Port/_anki/$Method" 2>&1 | Out-Null
}

# `generic.Bool` 的 protobuf 编码：字段 1 + varint
$boolFalse = [byte[]]@(0x08, 0x00)
$boolTrue = [byte[]]@(0x08, 0x01)
$windowTitle = if ($Lang -like "zh*") { "添加" } else { "Add" }

Write-Host "启动 $Exe（端口 $Port，集合 $Collection）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "队列: 共" $StartupTimeoutSeconds)) {
        throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $logPath）"
    }
    # 新建集合没有卡片，add 模式拿不到「home deck」的输入（也没有可复习的卡）
    if ((Read-Log $logPath) -match "队列: 共 0 张") {
        Write-Host "集合为空，写入一张测试卡片…"
        node tauri/tools/seed-card.mjs
        Send-Bridge -Window "main" -Command "domDone" | Out-Null
        if (-not (Wait-LogPattern "渲染完成" 30)) {
            throw "补卡片后复习界面仍未渲染（详见 $logPath）"
        }
    }

    Write-Host "发送 `add` 桥接命令（等价于复习界面的 `a` 快捷键）…"
    $status = Send-Bridge -Window "main" -Command "add"
    if ($status -ne "200") { throw "add 命令未被接受：HTTP $status" }

    # 关键断言：只有「窗口打开 + 注入成功 + 页面真的走完 loadNote」三者都成立才会有这一行
    if (-not (Wait-LogPattern "编辑器已载入 note\[add-cards\]" 40)) {
        throw "添加卡片窗口没有载入新建 note（详见 $logPath）"
    }
    Write-Host "添加卡片窗口已载入新建 note" -ForegroundColor Green

    $log = Read-Log $logPath
    foreach ($pattern in @("打开窗口: add-cards", "向编辑器注入: loadNote")) {
        if ($log -notmatch [regex]::Escape($pattern)) {
            Write-Host "缺少日志：$pattern" -ForegroundColor Red
            $passed = $false
        }
    }
    if ($log -match "编辑器载入 note 失败") {
        Write-Host "编辑器报错：$($Matches[0])" -ForegroundColor Red
        $passed = $false
    }

    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "addcards.png") `
        -WaitSeconds 20 -TitleLike $windowTitle

    Write-Host "验证默认值 / 添加落库 / 页面侧关闭判定…"
    node tauri/tools/addcards-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 宿主侧关闭契约之一：`val=false`（没有未保存输入）→ 直接关窗
    $closesBefore = Count-Log "关闭窗口: add-cards"
    $status = Send-Rpc -Method "closeAddCards" -Body $boolFalse
    if ($status -ne "200") { throw "closeAddCards 未被接受：HTTP $status" }
    if (-not (Wait-LogCount "关闭窗口: add-cards" ($closesBefore + 1) 20)) {
        Write-Host "val=false 时窗口没有关闭" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "val=false：窗口直接关闭" -ForegroundColor Green
    }

    # 宿主侧关闭契约之二：`val=true`（有未保存输入）→ 停在确认框上，窗口必须保持打开
    Send-Bridge -Window "main" -Command "add" | Out-Null
    if (-not (Wait-LogCount "打开窗口: add-cards" 2 20)) {
        throw "第二次打开添加卡片窗口失败（详见 $logPath）"
    }
    Send-RpcAndGiveUp -Method "closeAddCards" -Body $boolTrue
    if (-not (Wait-LogPattern "add-cards 有未保存输入" 20)) {
        Write-Host "val=true 时没有进入确认流程" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "val=true：停在「放弃输入？」确认框上（脚本结束时会连带结束进程）" -ForegroundColor Green
    }
    Start-Sleep -Seconds 2
    if ((Count-Log "关闭窗口: add-cards") -ne ($closesBefore + 1)) {
        Write-Host "确认框还没选，窗口却被关掉了" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "确认框期间窗口保持打开" -ForegroundColor Green
    }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
Write-Host "产物目录：$OutDir（addcards.png 为添加卡片窗口的真实渲染）"
if ($passed) {
    Write-Host "添加卡片窗口验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "添加卡片窗口验证失败" -ForegroundColor Red
exit 1
