# 一键验证「浏览器」窗口（M3 新写的 `ts/routes/browser`）。
#
# 串起这几件事：
#   1. 用隔离集合启动外壳（ANKI_START_PAGE=reviewer），必要时补一张卡片
#   2. 发复习界面的 `browse` 桥接命令（等价于 `b` 快捷键）→ 断言窗口打开 + 页面报到
#      + 首屏搜索真的出了结果（`browserTitle:0:...`，N>0）
#   3. 跑 browser-check.mjs：侧栏、列头、排序、多选、笔记模式、列清单、
#      加标签 / 改牌组 / 删除 + 撤销、双击开编辑器、预览
#   4. 预览窗口：翻面、上一张/下一张（经由浏览器页面算邻居）、重播音频、截图
#   5. `searchInBrowser` 与图表 `browserSearch` 的复用路径（窗口已存在 ⇒ 只注入不再开窗）
#   6. 关闭浏览器 → 断言连带的编辑区窗口与预览窗口也被关掉
#
# 前置：已按 TAURI_HANDOVER.md §2 构建过（`cd tauri; cargo build`），
#       且新页面已在 `out/sveltekit` 里（改了 `ts/routes/browser/` 要重跑 `yarn build`）。
#
# 用法：
#   pwsh -File tauri/tools/verify-browser.ps1

[CmdletBinding()]
param(
    [string]$Exe = "tauri/target/debug/anki-tauri.exe",
    [int]$Port = 40027,
    [string]$ApiKey = "test-key-123",
    [string]$Collection = "out/tauri-test/collection.anki2",
    [string]$Lang = "zh-CN",
    [string]$OutDir = "out/browser-check",
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
# 上一次导出的产物（文件名带时间戳，会越攒越多）
Get-ChildItem $OutDir -File -Include "*.apkg", "*.colpkg" -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue

Get-Process -Name "anki-tauri" -ErrorAction SilentlyContinue | Stop-Process -Force

$env:ANKI_API_PORT = "$Port"
$env:ANKI_API_KEY = $ApiKey
$env:ANKI_COLLECTION = (Join-Path $root $Collection)
$env:ANKI_START_PAGE = "reviewer"
$env:ANKI_LANG = $Lang
# 导出检查的测试钩子：原生"保存文件"对话框没法自动点，直接给个目录，
# 文件名用页面建议的那个（`platform/dialogs.rs` 的 `save_file_dialog`）
$env:ANKI_SAVE_PATH = (Join-Path $root "out/browser-check")

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

# 同一个模式会出现多次（例如第二次开窗 / 第二张预览卡片），所以要按次数等
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

# 桥接命令必须带窗口身份（见 TAURI_HANDOVER.md §5.16）
function Send-Bridge {
    param([string]$Window, [string]$Command)
    $cmdPath = Join-Path $OutDir "cmd.dat"
    [System.IO.File]::WriteAllBytes($cmdPath, [System.Text.Encoding]::ASCII.GetBytes($Command))
    curl.exe -s -o NUL -w "%{http_code}" -X POST -H "Content-Type: text/plain" `
        -H "Authorization: Bearer $ApiKey" -H "X-Anki-Window: $Window" `
        --data-binary "@$cmdPath" "http://127.0.0.1:$Port/_anki/pycmd"
}

$browseTitle = if ($Lang -like "zh*") { "浏览" } else { "Browse" }
$previewTitle = if ($Lang -like "zh*") { "预览" } else { "Preview" }
$cardInfoTitle = if ($Lang -like "zh*") { "卡片信息" } else { "Card Info" }

Write-Host "启动 $Exe（端口 $Port，集合 $Collection）…"
$proc = Start-Process -FilePath $exePath -WorkingDirectory $root `
    -RedirectStandardOutput $logPath -RedirectStandardError $errPath -NoNewWindow -PassThru

$passed = $true
try {
    if (-not (Wait-LogPattern "队列: 共" $StartupTimeoutSeconds)) {
        throw "启动超时：$StartupTimeoutSeconds 秒内没有看到取队列日志（详见 $logPath）"
    }
    if ((Read-Log $logPath) -match "队列: 共 0 张") {
        Write-Host "集合为空，写入一张测试卡片…"
        node tauri/tools/seed-card.mjs
        Send-Bridge -Window "main" -Command "domDone" | Out-Null
        if (-not (Wait-LogPattern "渲染完成" 30)) {
            throw "补卡片后复习界面仍未渲染（详见 $logPath）"
        }
    }

    Write-Host "发送 `browse` 桥接命令（等价于复习界面的 `b` 快捷键）…"
    $status = Send-Bridge -Window "main" -Command "browse"
    if ($status -ne "200") { throw "browse 命令未被接受：HTTP $status" }

    if (-not (Wait-LogPattern "打开窗口: browser ->" 30)) {
        throw "浏览器窗口没有打开（详见 $logPath）"
    }
    if (-not (Wait-LogPattern "bridge 命令\[browser\]: browserReady" 30)) {
        throw "浏览器页面没有报到（browserReady）"
    }
    Write-Host "浏览器窗口已打开并报到" -ForegroundColor Green
    # `browserTitle:<notesMode>:<selected>:<total>` —— 总数不为 0 才算首屏搜索出结果
    if (-not (Wait-LogPattern "bridge 命令\[browser\]: browserTitle:0:\d+:[1-9]" 30)) {
        Write-Host "首屏搜索没有结果（browserTitle:0:0:0）" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "首屏搜索已出结果" -ForegroundColor Green
    }

    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "browser.png") `
        -WaitSeconds 20 -TitleLike $browseTitle

    Write-Host "验证侧栏 / 排序 / 多选 / 笔记模式 / 列清单 / 批量动作 / 预览…"
    node tauri/tools/browser-check.mjs
    if ($LASTEXITCODE -ne 0) { $passed = $false }

    # 双击一行 → 宿主开编辑区窗口并注入 note（浏览器自己不做编辑器）
    if (-not (Wait-LogPattern "打开窗口: browser-editor ->" 20)) {
        Write-Host "双击行后没有打开编辑区窗口" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "编辑器已载入 note\[browser-editor\]" 20)) {
        Write-Host "编辑区窗口没有载入 note" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "双击行 → 编辑区窗口载入 note" -ForegroundColor Green
    }

    # 卡片信息（行右键菜单 → `cardInfo:<cid>`）：复用已有的 `card-info/<cid>` 路由
    if (-not (Wait-LogPattern "打开窗口: card-info ->" 20)) {
        Write-Host "右键菜单点「卡片信息」没有开窗" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "卡片信息窗口已打开" -ForegroundColor Green
        & (Join-Path $PSScriptRoot "capture-window.ps1") `
            -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "card-info.png") `
            -WaitSeconds 20 -TitleLike $cardInfoTitle
    }

    # 复制笔记（菜单 → `addCopy:<nid>:<deckId>`）：开「添加卡片」并载入那张笔记
    if (-not (Wait-LogPattern "复制笔记到添加卡片窗口: note=" 20)) {
        Write-Host "菜单点「复制笔记」没有开添加卡片窗口" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "编辑器已载入 note\[add-cards\]" 20)) {
        Write-Host "复制笔记后添加卡片窗口没有载入内容" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "复制笔记 → 添加卡片窗口载入该笔记" -ForegroundColor Green
    }

    # 更改笔记类型（菜单 → 宿主开窗口 → 页面里保存后**宿主关窗**）：
    # 窗口是既有路由 `change-notetype/<oldId>`，保存那条 RPC 由 `server/api.rs` 接管
    if (-not (Wait-LogPattern "打开窗口: change-notetype ->" 20)) {
        Write-Host "菜单点「更改笔记类型」没有开窗" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "bridge 命令\[change-notetype\]: domDone" 20)) {
        Write-Host "更改笔记类型窗口没有渲染出页面" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "关闭窗口: change-notetype" 20)) {
        Write-Host "保存后没有关掉更改笔记类型窗口" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "更改笔记类型：开窗 → 页面保存 → 宿主关窗" -ForegroundColor Green
    }

    # 导出（`saveFileDialog` 端点 + 导出 RPC）：产物在 out/browser-check/*.apkg
    if (-not (Wait-LogPattern "ANKI_SAVE_PATH 生效，跳过保存对话框" 20)) {
        Write-Host "导出没有走保存路径钩子" -ForegroundColor Red
        $passed = $false
    } else {
        $apkg = Get-ChildItem (Join-Path $root "out/browser-check") -Filter "*.apkg" -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (-not $apkg) {
            Write-Host "导出没有产出 .apkg 文件" -ForegroundColor Red
            $passed = $false
        } elseif ($apkg.Length -lt 1kb) {
            Write-Host "导出的 .apkg 太小：$($apkg.Length) 字节" -ForegroundColor Red
            $passed = $false
        } else {
            Write-Host "导出 apkg → $($apkg.Name)（$([int]($apkg.Length / 1kb)) KB）" -ForegroundColor Green
        }
    }

    # 预览：浏览器里点「预览」→ 独立窗口渲染问题面
    if (-not (Wait-LogPattern "打开窗口: preview ->" 20)) {
        Write-Host "点预览后没有打开预览窗口" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "预览注入: 面=问题" 20)) {
        Write-Host "预览窗口没有渲染问题面" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览窗口已渲染问题面" -ForegroundColor Green
    }

    & (Join-Path $PSScriptRoot "capture-window.ps1") `
        -ProcessName "anki-tauri" -OutFile (Join-Path $OutDir "preview.png") `
        -WaitSeconds 20 -TitleLike $previewTitle

    # 预览的上一张/下一张由**浏览器页面**算邻居（清单在它手里）。这里先用一次
    # `browserSearch` 让真实窗口的结果列表是新鲜的，否则它可能停在别的搜索上。
    $injectionsBefore = Count-Log "向浏览器注入搜索"
    Send-Bridge -Window "main" -Command "browserSearch:deck:current" | Out-Null
    if (-not (Wait-LogCount "向浏览器注入搜索" ($injectionsBefore + 1) 20)) {
        Write-Host "刷新浏览器结果列表失败" -ForegroundColor Red
        $passed = $false
    }

    # 预览的翻面与前后卡片（前后卡片经由浏览器页面算邻居）
    $status = Send-Bridge -Window "preview" -Command "previewNext"
    if ($status -ne "200") { throw "previewNext 未被接受：HTTP $status" }
    if (-not (Wait-LogPattern "预览注入: 面=答案" 20)) {
        Write-Host "预览没有翻到答案面" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览：问题面 → 答案面" -ForegroundColor Green
    }

    $cardsBefore = Count-Log "预览卡片: card="
    Send-Bridge -Window "preview" -Command "previewNext" | Out-Null
    if (-not (Wait-LogCount "预览卡片: card=" ($cardsBefore + 1) 20)) {
        Write-Host "预览没有切到下一张卡片" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览：答案面 → 下一张卡片" -ForegroundColor Green
    }
    Send-Bridge -Window "preview" -Command "previewPrev" | Out-Null
    if (-not (Wait-LogCount "预览卡片: card=" ($cardsBefore + 2) 20)) {
        Write-Host "预览没有切回上一张卡片" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览：下一张卡片 → 上一张卡片" -ForegroundColor Green
    }
    # 重播音频：没有音频标签时应当安静地什么都不做（日志里不该出现警告）
    Send-Bridge -Window "preview" -Command "previewReplay" | Out-Null
    Start-Sleep -Milliseconds 800
    if ((Read-Log $logPath) -match "提取音频标签失败") {
        Write-Host "预览重播音频时报错" -ForegroundColor Red
        $passed = $false
    }

    # 「仅背面」（勾选框 / B 键）：打开时直接显示答案面，配置落库
    Send-Bridge -Window "preview" -Command "previewBothSides:1" | Out-Null
    if (-not (Wait-LogPattern "预览仅背面: true" 20)) {
        Write-Host "预览「仅背面」没有生效" -ForegroundColor Red
        $passed = $false
    } elseif (-not (Wait-LogPattern "预览注入: 面=答案" 20)) {
        Write-Host "「仅背面」没有切到答案面" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览：仅背面（问题+答案）" -ForegroundColor Green
    }
    Send-Bridge -Window "preview" -Command "previewBothSides:0" | Out-Null
    if (-not (Wait-LogPattern "预览仅背面: false" 20)) {
        Write-Host "预览「仅背面」没有关掉" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "预览：回到只看问题" -ForegroundColor Green
    }

    # `searchInBrowser` 走的是"窗口已存在"的复用路径：只注入，不再开窗
    $log = Read-Log $logPath
    if ($log -notmatch "在浏览器里搜索: nid:") {
        Write-Host "searchInBrowser 没有把 SearchNode 翻成搜索词" -ForegroundColor Red
        $passed = $false
    } elseif ($log -notmatch "向浏览器注入搜索: nid:") {
        Write-Host "searchInBrowser 没有复用已打开的窗口（没有注入）" -ForegroundColor Red
        $passed = $false
    } elseif (([regex]::Matches($log, "打开窗口: browser ->")).Count -ne 1) {
        Write-Host "searchInBrowser 又开了一个浏览器窗口" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "searchInBrowser：复用已打开的窗口并注入搜索" -ForegroundColor Green
    }

    # 图表点柱子 → `browserSearch:<query>`（`GraphsPage.svelte` 发的命令，
    # 对照 `qt/aqt/stats.py:132-136`）。这是一条真正的产品内入口。
    $graphInjections = Count-Log "向浏览器注入搜索: deck:current"
    $status = Send-Bridge -Window "main" -Command "browserSearch:deck:current"
    if ($status -ne "200") { throw "browserSearch 命令未被接受：HTTP $status" }
    if (-not (Wait-LogCount "向浏览器注入搜索: deck:current" ($graphInjections + 1) 20)) {
        Write-Host "browserSearch 没有把搜索词交给浏览器" -ForegroundColor Red
        $passed = $false
    } else {
        Write-Host "browserSearch：图表 → 浏览器搜索" -ForegroundColor Green
    }

    # 关掉浏览器 → 编辑区窗口与预览窗口都是它带出来的，应一起关掉
    $status = Send-Bridge -Window "browser" -Command "close"
    if ($status -ne "200") { throw "close 命令未被接受：HTTP $status" }
    if (-not (Wait-LogPattern "关闭窗口: browser" 20)) {
        throw "浏览器窗口没有关闭（详见 $logPath）"
    }
    foreach ($dependent in @("browser-editor", "preview", "card-info")) {
        if (-not (Wait-LogPattern "关闭窗口: $dependent" 20)) {
            Write-Host "关闭浏览器时没有连带关掉 $dependent 窗口" -ForegroundColor Red
            $passed = $false
        } else {
            Write-Host "关闭浏览器连带关掉 $dependent 窗口" -ForegroundColor Green
        }
    }
} catch {
    Write-Host "验证过程出错：$_" -ForegroundColor Red
    $passed = $false
} finally {
    Get-Process -Id $proc.Id -ErrorAction SilentlyContinue | Stop-Process -Force
}

Write-Host ""
Write-Host "产物目录：$OutDir（browser.png / preview.png 为真实渲染）"
if ($passed) {
    Write-Host "浏览器窗口验证通过" -ForegroundColor Green
    exit 0
}
Write-Host "浏览器窗口验证失败" -ForegroundColor Red
exit 1
