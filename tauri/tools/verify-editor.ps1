# 一键验证「编辑当前卡片」窗口（编辑器外壳的第一块）。
#
# 串起四件事：
#   1. 用隔离集合启动外壳（ANKI_START_PAGE=reviewer），必要时补一张卡片
#   2. 发复习界面的 `edit` 桥接命令（底部栏「编辑」按钮就是它）
#   3. 断言编辑器窗口打开、并**真的载入了 note**（靠注入脚本回传的 editorLoaded）
#   4. 截取窗口真实渲染，再走一次关闭流程
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build` →
#       `tauri/target/debug/anki-tauri.exe`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-editor.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40021,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/editor-check",
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
foreach ($path in @($logPath, $errPath)) {
    if (Test-Path $path) { Remove-Item $path -Force }
}

Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "reviewer"
$env:ANKI_LANG = $Lang

# 图像遮挡的「选图片」要弹原生的打开对话框（没法自动点），所以造一张真图片并让
# 宿主直接返回它（`platform/dialogs.rs` 的 `ANKI_OPEN_PATH`，与 `ANKI_SAVE_PATH` 同类）。
# 1x1 的透明 PNG，够 `addMediaFromPath` 拷进媒体夹。
$occlusionFixture = Join-Path $OutDir "occlusion-fixture.png"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
[System.IO.File]::WriteAllBytes(
    $occlusionFixture,
    [Convert]::FromBase64String(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFAAH/q842iQAAAABJRU5ErkJggg=="
    )
)
$env:ANKI_OPEN_PATH = $occlusionFixture

function Read-Log {
    param([string]$Path)
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
    param([string]$Method, [string]$Body = "")
    $bodyPath = Join-Path $OutDir "rpc.bin"
    [System.IO.File]::WriteAllBytes($bodyPath, [byte[]]::new(0))
    curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: application/binary" `
        -H "Authorization: Bearer $ApiKey" --data-binary "@$bodyPath" `
        "http://127.0.0.1:$Port/_anki/$Method"
}

Write-Host "启动 $Exe（端口 $Port，集合 $Collection）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "队列: 共" $StartupTimeoutSeconds)) {
        throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $logPath）"
    }
    # 新建集合没有卡片，复习界面不会显示卡片，「编辑当前」也就无从谈起
    if ((Read-Log $logPath) -match "队列: 共 0 张") {
        Write-Host "集合为空，写入一张测试卡片…"
        node tauri/tools/seed-card.mjs
        Send-Bridge -Window "main" -Command "domDone" | Out-Null
        if (-not (Wait-LogPattern "渲染完成" 30)) {
            throw "补卡片后复习界面仍未渲染（详见 $logPath）"
        }
    }

    Write-Host "发送 `edit` 桥接命令（等价于底部栏的「编辑」按钮）…"
    $status = Send-Bridge -Window "main" -Command "edit"
    if ($status -ne "200") { throw "edit 命令未被接受：HTTP $status" }

    # 关键断言：注入脚本回传的「已载入 note」。
    # 只有窗口打开、注入成功、页面真的取到 note 三者都成立才会有这一行。
    if (-not (Wait-LogPattern "编辑器已载入 note" 40)) {
        throw "编辑器没有载入 note（详见 $logPath）"
    }
    Write-Host "编辑器已载入 note" -ForegroundColor Green

    $log = Read-Log $logPath
    foreach ($pattern in @("打开编辑器: card=", "打开窗口: edit-current", "向编辑器注入: loadNote")) {
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
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "editor.png") `
        -WaitSeconds 20 -TitleLike "编辑当前"

    # 「字段…」对话框：编辑器工具栏的「字段」按钮发的前端端点就是 openFieldsDialog
    # （`NotetypeButtons.svelte:43`；宿主对着编辑器窗口当前的笔记类型开窗）
    Write-Host "验证「字段…」对话框…"
    $status = Send-Rpc -Method "openFieldsDialog"
    if ($status -ne "200") { throw "openFieldsDialog 未被接受：HTTP $status" }
    if (-not (Wait-LogPattern "打开窗口: fields ->" 20)) {
        Write-Host "字段对话框没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "字段对话框窗口已打开" -ForegroundColor Green
        Start-Sleep -Seconds 1
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "fields-window.png") `
            -WaitSeconds 20
    }
    node tauri/tools/fields-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 「卡片…」对话框：编辑器工具栏的「卡片」按钮 / `Ctrl+L` 发的前端端点就是
    # openCardsDialog（`NotetypeButtons.svelte:61`）。宿主对着编辑器窗口当前的笔记类型
    # 开窗，并把**正在编辑的那张 note** 与当前卡片的 ord 一起带过去（预览用）
    Write-Host "验证「卡片…」对话框…"
    $status = Send-Rpc -Method "openCardsDialog"
    if ($status -ne "200") { throw "openCardsDialog 未被接受：HTTP $status" }
    if (-not (Wait-LogPattern "打开窗口: cards ->" 20)) {
        Write-Host "卡片对话框没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "卡片对话框窗口已打开" -ForegroundColor Green
        Start-Sleep -Seconds 2
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "cards-window.png") `
            -WaitSeconds 20
    }
    $env:SHOT_DIR = $OutDir
    node tauri/tools/cards-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 关闭流程：编辑器的「关闭」按钮就是发这条 RPC（NoteEditor.svelte 的 closeEditCurrent）
    $status = Send-Rpc -Method "closeEditCurrent"
    if ($status -ne "200") { throw "closeEditCurrent 未被接受：HTTP $status" }
    if (-not (Wait-LogPattern "关闭窗口: edit-current" 20)) {
        Write-Host "关闭窗口的日志没有出现" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "关闭流程正常" -ForegroundColor Green
    }

    # 改字段 → 保存：这一步要真的敲键盘，所以走 Playwright（见脚本注释）
    Write-Host "验证「改字段 → 保存」链路…"
    node tauri/tools/editor-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 图像遮挡：只有遮挡笔记类型才显示工具栏按钮；选图片走宿主的 /openFilePicker
    # （本版本没有独立的遮挡窗口，遮罩编辑器就内嵌在编辑器页面里）
    Write-Host "验证图像遮挡（内嵌遮罩编辑器）…"
    $env:SHOT_DIR = $OutDir
    node tauri/tools/occlusion-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }
    # 页面把图片拷进了媒体夹，后端没暴露删除接口，这里从磁盘清掉
    $mediaFile = Join-Path (Split-Path $env:ANKI_COLLECTION -Parent) `
        "collection.media/occlusion-fixture.png"
    if (Test-Path $mediaFile) { Remove-Item $mediaFile -Force -ErrorAction SilentlyContinue }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
Write-Host "产物目录：$OutDir（editor.png 为编辑器窗口的真实渲染）"
if ($passed) {
    Write-Host "编辑器窗口验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "编辑器窗口验证失败" -ForegroundColor Red
exit 1
