# Tauri 外壳迁移 — 交接文档

> 面向接手本任务的工程师/Agent。本文假设你**没有**此前的对话上下文。
> 阅读顺序建议：本文 §1 → §2 → §3，然后按需查 §5/§6。
>
> 相关文档：
> - `TAURI_MIGRATION_PLAN.md` — 总体计划（M0~M5 里程碑、风险、验收标准）
> - `tauri/README.md` — 工程说明与完整验证记录

---

## 1. 一句话状态

在 `tauri/` 下新增了一个 Tauri v2 外壳，**复用现有的 `ts/` 前端与 `rslib/` 后端**，
在 Windows 上已经能跑通「打开集合 → 复习一张卡片 → 评分 → 下一张」的完整闭环，
并已接入编辑器外壳的「编辑当前卡片」与**「添加卡片」**两个窗口。

- **M0 完成**：骨架、方法表、HTTP 服务、窗口
- **M1 完成**：集合打开、静态资源、媒体、CSP 分级、legacy 页面机制、最小复习会话、底部工具栏
- **M2 完成**：mediasrv 的 32 个平台端点全部有实现 —— 剪贴板（含 `text/uri-list`）、
  文件选择、图片、原生对话框、profile / meta 配置（直接读写桌面版 `prefs21.db`）、
  音频播放与**录音**、**TTS**、**窗口管理（多窗口模型）**、复习状态同步
- **M3 进行中**：编辑器、浏览器、牌组列表、自定义学习都已可用。
  - 编辑器：「编辑当前卡片」（`mode=current`）、**「添加卡片」（`mode=add`，含「丢弃输入」
    确认）**、浏览器里的编辑区（`mode=browser`）、以及**图像遮挡**（内嵌在编辑器页面里的
    遮罩编辑器，见 `tauri/README.md` 的「图像遮挡」）。
  - 浏览器：**页面是新写的**（`ts/routes/browser/`），入口有复习界面 `b`、
    图表 `browserSearch`、`searchInBrowser` 端点；功能含搜索/排序/多选、笔记模式、
    列清单 / 列宽 / 列顺序、**完整的动作表**（加/移除标签、清除未使用标签、标记、旗标、
    暂停、埋藏、重置、设置到期日、重新定位、立即评分、改牌组、删除、复制笔记、
    卡片信息、更改笔记类型、查找替换、跳转、反选/全选/选中笔记）、撤销重做、
    **导出 apkg/colpkg**，以及**预览与卡片信息两个独立窗口**。
  - 牌组列表：**页面是新写的**（`ts/routes/decks/`），是主窗口的**启动默认页**；
    牌组树/今日计数、折叠、点牌组 → 概览 → 学习、齿轮菜单（重命名/选项/删除）、
    底部「获取共享牌组 / 创建牌组 / 导入文件」，配 `d`/`s` 快捷键与两处**顶部工具栏**
    （牌组/添加/浏览/统计）。
  - 既有页面接入：牌组选项、统计（`t`）、设置（`Ctrl+P`）、导入、图像遮挡都已通；
    概览里还能开**自定义学习**（`C` → 新写的 `ts/routes/custom-study/`）。
  - 编辑器的两个对话框（「字段…」/「卡片…」，都是新写的页面）：`ts/routes/fields/`、
    `ts/routes/cards/`。
  - 未做：主窗口菜单栏与同步、编辑已有筛选牌组、导出 CSV/纯文本、
    查找重复、管理笔记类型、启动行为（检查/备份）、Android
- **M4 未开始**：打包与 Android

**当前可直接验证**（六个一键脚本，详见 §2.4）：

| 脚本 | 覆盖 |
| --- | --- |
| `tauri/tools/verify-reviewer.ps1` | 复习界面：启动 → 补卡片 → 截真实窗口 → 布局断言 |
| `tauri/tools/verify-m2.ps1` | 平台能力：剪贴板文件列表 / TTS 合成 / 录音闭环 + 多窗口命令归属 |
| `tauri/tools/verify-editor.ps1` | 编辑器：「编辑当前卡片」开窗 → 载入 note → 截图 → 关窗 → 改字段保存 → **「字段…」/「卡片…」两个对话框** → **图像遮挡**（选图片 → 掩码编辑器渲染出图片） |
| `tauri/tools/verify-addcards.ps1` | 添加卡片：`add` 开窗 → 注入新建 note → 截图 → 默认值 / 添加落库 / 两条关闭契约 |
| `tauri/tools/verify-browser.ps1` | 浏览器：`browse` 开窗 → 截图 → 侧栏 / 表格 / 双击开编辑器 → `searchInBrowser` 复用路径 → 关闭连带 |
| `tauri/tools/verify-decks.ps1` | 牌组列表（启动默认页）：截图 → 折叠 / 概览 / 齿轮菜单 / 新建删除 → `d`/`s`/`study` 切页 |

---

## 2. 如何跑起来（可复现）

### 2.1 前置产物（只需一次）

```powershell
# 1) protoc 31.1（版本/哈希须与 build/ninja_gen/src/protobuf.rs 一致）
New-Item -ItemType Directory -Force -Path out/extracted/protoc | Out-Null
curl.exe -L -o out/protoc-31.1.zip `
  "https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-win64.zip"
Expand-Archive out/protoc-31.1.zip -DestinationPath out/extracted/protoc -Force

# 2) protobuf 描述符（rslib 与 tauri 的 build.rs 都依赖）
$env:CARGO_TARGET_DIR="out/rust"
cargo build -p anki_proto
# 产出 out/rslib/proto/descriptors.bin、out/ts/lib/generated/backend.ts、
#      out/pylib/anki/_backend_generated.py

# 3) 前端依赖与产物
corepack yarn install
$root = (Get-Location).Path
$protos = (Get-ChildItem proto/anki/*.proto).Name | ForEach-Object { "anki/$_" }
& "out/extracted/protoc/bin/protoc.exe" -I proto $protos `
  "--plugin=protoc-gen-es=$root/node_modules/.bin/protoc-gen-es.cmd" `
  "--es_out=$root/out/ts/lib/generated"
& "$root/node_modules/.bin/tsx.cmd" ts/tools/markpure.ts out/ts/lib/generated
Copy-Item ts/lib/generated/*.ts out/ts/lib/generated/ -Force   # 手写 helper

# i18n 的 TS 接口：由 anki_i18n 的 build script 在编译期经 STRINGS_TS 写出。
# 漏掉这一步，yarn build 会报 Could not load .../out/ts/lib/generated/ftl
$env:STRINGS_TS="$root/ts/lib/generated/ftl.ts"
cargo build -p anki_i18n
Copy-Item ts/lib/generated/ftl.ts out/ts/lib/generated/ -Force

corepack yarn build                                            # → out/sveltekit

# 4) legacy 界面产物（复习屏需要）
New-Item -ItemType Directory -Force -Path qt/_aqt/data/web/js, qt/_aqt/data/web/css | Out-Null
node ts/transform_ts.mjs qt/aqt/data/web/js/webview.ts qt/_aqt/data/web/js/webview.js
node ts/bundle_ts.mjs ts/reviewer/index_wrapper.ts qt/_aqt/data/web/js/reviewer.js
node ts/transform_ts.mjs qt/aqt/data/web/js/reviewer-bottom.ts qt/_aqt/data/web/js/reviewer-bottom.js
$sass = (Resolve-Path "node_modules/.bin/sass.cmd").Path
& $sass -s compressed -I . -I node_modules qt/aqt/data/web/css/webview.scss -- qt/_aqt/data/web/css/webview.css
& $sass -s compressed -I . -I node_modules ts/reviewer/reviewer.scss -- qt/_aqt/data/web/css/reviewer.css
& $sass -s compressed -I . -I node_modules qt/aqt/data/web/css/toolbar-bottom.scss -- qt/_aqt/data/web/css/toolbar-bottom.css
& $sass -s compressed -I . -I node_modules qt/aqt/data/web/css/reviewer-bottom.scss -- qt/_aqt/data/web/css/reviewer-bottom.css
Copy-Item qt/aqt/data/web/imgs/* qt/_aqt/data/web/imgs/ -Recurse -Force
```

### 2.2 构建与运行

```powershell
cd tauri
cargo build
.\target\debug\anki-tauri.exe
```

> 可执行文件在 **`tauri/target/debug/anki-tauri.exe`**（`tauri/` 是独立 workspace，
> `cd tauri; cargo build` 用的是它自己的 `target/`）。四个验证脚本的 `-Exe` 默认值就是
> 这个路径，直接跑即可。
>
> （本文档早前写的 `out/rust/debug/anki-tauri.exe` 是错的：`out/rust` 是 §2.1 里
> `CARGO_TARGET_DIR=out/rust` 编译 `anki_proto` / `anki_i18n` 的产物目录，里面没有外壳
> 二进制。）

### 2.3 调试用环境变量

| 变量 | 默认 | 用途 |
| --- | --- | --- |
| `ANKI_API_PORT` | 0（随机） | 固定端口，便于 curl 验证 |
| `ANKI_COLLECTION` | `%APPDATA%/Anki2/User 1/collection.anki2` | 指定集合；不存在则创建 |
| `ANKI_START_PAGE` | `graphs` | `reviewer` 走 legacy 复习页；其余是 SvelteKit 路由 |
| `ANKI_API_KEY` | 随机 | 固定 API 密钥 |
| `ANKI_LANG` | `en-US` | 界面语言，`zh_CN` / `zh-CN` 均可 |

### 2.4 冒烟测试（已脚本化）

```powershell
pwsh -File tauri/tools/verify-reviewer.ps1   # 复习界面 + 布局断言
pwsh -File tauri/tools/verify-m2.ps1         # 平台能力 + 多窗口命令归属
pwsh -File tauri/tools/verify-editor.ps1     # 编辑器窗口 + 改字段保存
pwsh -File tauri/tools/verify-addcards.ps1   # 添加卡片窗口 + 添加落库 + 关闭契约
pwsh -File tauri/tools/verify-browser.ps1    # 浏览器窗口 + 表格 + 编辑器联动
```

五条都会自己启动外壳、用隔离集合、跑断言、结束进程。三个前置：

- **浏览器页面是 SvelteKit 产物**：改了 `ts/routes/browser/` 必须重跑
  `corepack yarn build`（§2.1 第 3 步），否则外壳发出去的是旧页面；

- Playwright 的 Chromium 内核要装过（`node node_modules/playwright/cli.js install
  chromium-headless-shell`），否则 `visual-check.mjs` / `editor-check.mjs` /
  `addcards-check.mjs` 会报 `Executable doesn't exist`；
- `verify-m2.ps1` 的**录音**断言需要本机有可用的输入设备，没有麦克风时会以
  `recordAudio → HTTP 500 找不到可用的录音设备` 失败（其余断言照常通过）。
  没设备时 `recordAudio` 会**一直挂着**，所以 `m2-check.mjs` 里那条请求必须立刻
  挂一个 `.catch()`（否则 node 按 unhandled rejection 自杀，还会带出 libuv 的
  `!(handle->flags & UV_HANDLE_CLOSING)` 断言 —— 看着像被测程序崩了，其实是脚本
  自己的问题）；收尾靠脚本末尾的 `process.exit()`，**不要去 `abort()`** 那条挂在
  半路的请求（Windows 上同样会触发那个断言）。

复习界面那条期望看到：

```text
界面语言: zh-CN
bridge 命令[main]: domDone
domDone: 复习模式=true
队列: 共 N 张（新 x / 学习中 y / 复习 z）
渲染完成: 问题 NNN 字节 / 答案 NNN 字节 / 样式 NNN 字节
布局检查通过：底部栏贴住窗口底部、卡片内边距正常、底部栏按钮齐全且在窗口内
```

想手工看日志的话：

```powershell
$env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
$env:ANKI_COLLECTION="$PWD/out/tauri-test/collection.anki2"
$env:ANKI_START_PAGE="reviewer"
$p = Start-Process -FilePath tauri/target/debug/anki-tauri.exe -PassThru `
     -RedirectStandardOutput out/run.log -RedirectStandardError out/run.err
Start-Sleep 18
Get-Content out\run.log -Encoding utf8 | Select-String "队列|渲染完成|底部栏|界面语言"
Stop-Process -Id $p.Id -Force
```

> 可执行文件在 `tauri/target/debug/anki-tauri.exe`（见 §2.2 的说明）。
>
> `out/tauri-test/` 是隔离的测试数据目录，**不会污染真实 profile**。
> 里面已有 `collection.anki2` 与 `collection.media/`。
>
> 脚本用 `Stop-Process -Force` 结束进程，可能打断 `prefs21.db` 的写入；读路径已按
> 读写方式打开以便恢复热日志（见 §5.19），但仍不建议在真实 profile 上这么做。

### 2.5 手工触发桥接命令（无需点击 UI）

```powershell
$pf = "out\pycmd.dat"
[System.IO.File]::WriteAllBytes($pf, [System.Text.Encoding]::UTF8.GetBytes("ans"))
curl.exe -s -X POST -H "Content-Type: text/plain" `
  -H "Authorization: Bearer test-key-123" --data-binary "@$pf" `
  "http://127.0.0.1:40001/_anki/pycmd"
```

`ans` → 显示答案；`ease1`..`ease4` → 评分并进入下一张。

> 桥接命令**按来源窗口分流**（见 §5.16）。注入脚本会自动带
> `X-Anki-Window: <窗口标签>`；手工 curl 不带这个头时按主窗口处理，
> 所以上面这些复习命令照常可用。要让录音窗口保存，得显式加
> `-H "X-Anki-Window: recorder"`。

---

## 3. 架构与关键决策

### 3.1 为什么保留本地 HTTP 服务，而不是用 Tauri IPC

前端 `ts/` 的所有资源都用 `/_anki/...` **绝对路径**引用，API 调用也是 `POST /_anki/{method}`：

```19:22:ts/lib/generated/post.ts
        const inputBytes = input.toBinary();
        const path = `/_anki/${method}`;
        const outputBytes = await postProtoInner(path, inputBytes, opChangesType, signal);
```

因此**在 Rust 侧用 axum 复刻 `qt/aqt/mediasrv.py` 的 HTTP 形态**，可以让那 250 个
Svelte 组件零改动运行。这是整个方案的核心取舍。

### 3.2 无 FFI

Tauri 的 Rust 侧与 rslib 同进程，直接 `use anki::...`。入口只有一个函数：

```text
Backend::run_service_method(service: u32, method: u32, input: bytes) -> Result<bytes, bytes>
```

方法名 → 索引的映射由 `tauri/build.rs` 调用 `anki_proto_gen::get_services()` 生成，
**与 rslib 内部分发使用同一来源**，保证索引不漂移。

### 3.3 数据流

```text
WebView (系统 WebView2)
  │  加载 http://127.0.0.1:<port>/...
  │  · SvelteKit 页面：/graphs、/editor ...
  │  · legacy 页面：/_anki/legacyPageData?id=N（复习屏走这条）
  │
  ├─ HTTP + protobuf ──> axum（tauri/src/server/）
  │                        ├─ 方法表命中 → rslib
  │                        ├─ 平台端点   → tauri/src/platform/
  │                        └─ 其余       → 404
  │
  └─ bridgeCommand/pycmd ──> POST /_anki/pycmd ──> tauri/src/server/pycmd.rs
```

### 3.4 注入脚本（`src/bridge.rs`）

页面加载前注入，做两件事，分别对应 Qt 的两个机制：

| 注入内容 | 对照 Qt |
| --- | --- |
| 包装 `window.fetch`，补 `Authorization` 头 | `AuthInterceptor`（Tauri 无等价请求拦截 API） |
| 提供 `window.bridgeCommand` / `window.pycmd` | QWebChannel 注入的同名函数 |
| `load` 事件后发 `domDone` | 连接成功后 `pycmd("domDone")` |

### 3.5 配置与桌面版共用 `prefs21.db`

`profile` / `meta` 配置（`getProfileConfigJson`、`setProfileConfigJson`、
`getMetaJson`、`setMetaJson`）直接读写桌面版的 `prefs21.db`（SQLite + Python
pickle），不再有第二份存储。

要点是**读取时保持 pickle 原值、只在返回前端那一步才转 JSON** —— 否则
`mainWindowGeom` 这类 `bytes` 项会在整体重写时被破坏。实现见
`src/platform/prefs21.rs`，验证方式与结论见 `tauri/README.md` 的「配置存储」。

### 3.6 窗口模型：一个窗口一个 `WebviewWindow`

**已定案**：多窗口 + 单实例 + 模态（禁用主窗口）+ 几何记忆，实现见
`tauri/src/windows.rs`；完整依据见 `tauri/README.md` 的「窗口模型」。要点：

- Qt 的这些窗口本来就是独立顶层窗口（牌组选项 / 导入还是 `ApplicationModal`），
  标题栏、任务栏项、几何、模态都是用户可感知的语义，单窗口 + 前端路由复现不了；
- 牌组选项 / 导入 / 首选项已经是独立的 SvelteKit 路由，多窗口是增量改动
  （前端零改动），主窗口本来就是「一个 URL」；
- **那 10 个降级端点里只有 4 个是被窗口模型挡住的**：`deckOptionsReady`、
  `deckOptionsRequireClose`、`importDone`、`importDialogRequireClose`。
  浏览器 / 添加卡片 / 编辑当前 / 字段 / 卡片布局在新版 Anki 里是**原生 Qt 界面**，
  仓库里没有对应的 Web 页面 —— 它们需要新的 Svelte 界面（M3），
  `saveCustomColours` 则需要原生取色器暴露调色板的能力。

窗口几何存在 profile 配置的 `tauriWindowGeometry:<label>` 下（JSON 数组），与 Qt 的
`{key}Geometry6.8`（`QWidget.saveGeometry()` 的二进制块）**刻意分开**：两边格式不可能
互通，复用键名只会互相破坏。

**命令按窗口分流**：注入脚本给每个请求带 `X-Anki-Window: <窗口标签>`，`pycmd.rs`
据此判断来源（复习命令只接受主窗口）。这不多余 —— 缺了它，任意窗口加载时发的
`domDone` 都会重启复习会话，详见 §5.16。

打开窗口的产品内入口还没实现（要经过牌组浏览器，而它是原生 Qt 界面），因此提供
`ANKI_OPEN_ROUTES=deck-options/1`（与 `ANKI_START_PAGE` 同类）供端到端验证。
（浏览器窗口本身已经有产品内入口了，见下面 §3.7；牌组选项 / 导入仍只能靠
`ANKI_OPEN_ROUTES`。）

### 3.7 浏览器：唯一"宿主 + 新写页面"的窗口

前几节的窗口都是**复用上游页面**（deck-options / editor / import-* 在 `ts/routes/`
里已经存在），浏览器不是：Qt 的浏览器整条链路都是原生 Qt 控件
（`qt/aqt/browser/`），所以页面是新写的（`ts/routes/browser/`，见 `tauri/README.md`
的「浏览器」）。四处与上游不同的决定，改动前务必知道：

1. **编辑区是独立窗口**（标签 `browser-editor`）：Qt 把编辑器嵌在浏览器窗口里，而
   `/editor/` 是不可信页面（CSP 带 `frame-ancestors 'none'`），iframe 会被 WebView
   直接拒绝。关闭浏览器窗口时会连带关掉它。
2. **搜索词由页面问、宿主答**：页面报到（`browserReady`）后宿主在**响应**里把首屏
   搜索词交回去；窗口已存在时改走 `eval` 注入 `loadSearch(...)`。这样两条路径都幂等，
   躲开了"注入时页面还没就绪"的时序问题。
3. **表格要先把列告诉后端**：读集合配置 `activeCols`（缺省 4 列）→
   `setActiveBrowserColumns` → 之后才能逐行 `browserRowForId`。少了中间那步，
   整张表格会空白并弹错误框（见 §5.21）。
4. **结果清单只有页面有，宿主持不了**：所以"预览的上一张/下一张"、编辑器的预览按钮
   都要绕一圈 —— 宿主 `eval` 页面的 `previewNeighbor(±1, cid)` / `previewCurrent()`，
   页面算出目标再回一条 `preview:<cid>`。Qt 也是这个形状（预览器问浏览器父窗口要
   前后卡片，`previewer.py:412-430`）。
5. **动作表本可以全在前端做，只有四处必须回宿主**：卡片信息（`cardInfo:<cid>` 开
   `card-info/<cid>` 窗口）、复制笔记（`addCopy:<nid>:<deckId>` 开「添加卡片」并注入
   该笔记）、更改笔记类型（`changeNoteType:<nid>,...` 开 `change-notetype/<id>` 窗口）、
   关闭（`close`）。其余动作（标记/旗标/暂停/埋藏/重置/到期日/重排/评分/
   标签/改牌组/删除/查找替换/跳转）都是页面直接调 RPC —— 后端已经有现成的方法，
   宿主不必中转。
6. **导出全在页面里做，宿主只补"选保存路径"**：对话框、选项、导出 RPC 都在
   `ts/routes/browser/` 里（对照 `browser.py:377` 与 `import_export/exporting.py`），
   只有"原生保存框"必须问宿主 —— 为此加了**外壳自己的端点** `saveFileDialog`
   （上游没有：Qt 的对话框直接调 `getSaveFile()`）。请求复用
   `OpenFilePickerRequest`，`key` 字段按外壳约定传建议文件名；`ANKI_SAVE_PATH`
   指向目录时直接返回 `<目录>/<建议文件名>`，好让验证脚本绕过原生对话框。
7. **"窗口感知的端点"复用现有页面**：`card-info/<cid>` 与 `change-notetype/<id>` 都是
   仓库里已有的路由，宿主只负责开窗与（必要时）关窗。其中 `changeNotetype` 必须由
   宿主接管：页面提交的请求里**没有 `note_ids`**（它不知道选了哪些笔记），宿主开窗时
   记下、请求到达时补上、成功后关窗（`changenotetype.py:68-85` +
   `mediasrv.py:772`）。凡是"页面只给局部信息、其余由宿主补齐"的端点，都该走这条路
   （也在 `server/api.rs` 里单独接，因为要 async 调后端 + 关窗）。

页面侧的其它要点（排序交给后端、批量动作的 id 映射、删除没有确认框、列清单写在
`activeCols`）见 `tauri/README.md` 的「浏览器」；`searchInBrowser` 端点在
`server/api.rs` 里单独处理（不在 `platform::dispatch`）：它要调异步 RPC
`buildSearchString`，而平台层跑在阻塞线程池上。

---

## 4. 已完成的工作清单

### 4.1 模块地图

```text
tauri/src/
├─ main.rs            解析路径 → 打开集合 → 起服务取端口 → 创建窗口
├─ backend.rs         rslib 桥接：open_collection / call(method) / run(service, method)
├─ method_table.rs    方法名 → 索引（build.rs 生成）
├─ bridge.rs          注入脚本（鉴权头 + 窗口身份 + bridgeCommand）
├─ legacy.rs          legacy 页面注册与 HTML 组装（复习页 / 录音页 / 预览页）
├─ main_window.rs     主窗口的入口与**页面切换**：牌组列表 ⇄ 复习页（`decks`/`overview`/
│                     `study`），以及统计（`graphs/`）与设置（`preferences/`），
│                     对应 Qt 的全局快捷键 `d`/`s`/`t`/`Ctrl+P`
├─ assets/            注入页面用的脚本：reviewer-menu.js（复习菜单/表单）、
│                     reviewer-toolbar.js（顶部工具栏）。**改了要重新 cargo build**
│                     （include_str! 编进二进制），见 §5.29
├─ browser.rs         浏览器窗口（开窗 + 首屏搜索 + 双击某行 → 打开编辑区 +
│                     预览/翻页的转发 + 卡片信息/更改笔记类型/复制笔记三个窗口）
├─ preview.rs         预览窗口（渲染问题/答案、翻面、前后卡片、重播音频）
├─ editor.rs          编辑器窗口（编辑当前 / 添加卡片 / 浏览器编辑区：开窗 +
│                     `editorReady` 后按窗口标签注入 `loadNote`）
├─ reviewer.rs        复习会话
├─ reviewer_menu.rs   复习界面的「更多」/右键菜单（宿主拼数据 + 执行动作；菜单脚本在
│                     `tauri/assets/reviewer-menu.js`，用 `include_str!` 引用）
├─ paths.rs           资源路径（开发/打包双模式）
├─ state.rs           共享状态
├─ windows.rs         窗口模型（多窗口 / 单实例 / 模态 / 几何记忆）
├─ platform/          平台端点（FrontendService，32 个上游端点 + 外壳新增的
│                     `saveFileDialog`）
│  ├─ audio.rs        playFile（rodio 队列播放）
│  ├─ clipboard.rs    readClipboard / writeClipboard
│  ├─ config.rs       get/setProfileConfigJson、get/setMetaJson
│  ├─ prefs21.rs      prefs21.db 读写（SQLite + pickle，与桌面版共用）
│  ├─ record.rs       recordAudio（cpal 采集 + WAV，会话挂在 state.recorder）
│  ├─ scheduling.rs   getSchedulingStatesWithContext / setSchedulingStates
│  ├─ dialogs.rs      openFilePicker / askUser / showMessageBox / openLink
│  │                  + **saveFileDialog**（外壳新增，导出对话框问保存路径用）
│  └─ media.rs        convertPastedImage / openMedia / showInMediaFolder
└─ server/
   ├─ mod.rs          路由、Host/Origin 中间件、服务启动
   ├─ api.rs          POST /_anki/{method}
   ├─ assets.rs       静态资源 + 媒体 + CSP 分级
   ├─ legacy.rs       GET /_anki/legacyPageData
   └─ pycmd.rs        POST /_anki/pycmd

tauri/tools/           验证脚本：verify-reviewer.ps1（复习界面）、verify-m2.ps1
                       （平台能力）、verify-editor.ps1（编辑当前卡片）、
                       verify-addcards.ps1（添加卡片）、verify-browser.ps1（浏览器）、
verify-decks.ps1（牌组列表）、
                       capture-window.ps1（截真实窗口，多窗口用 -TitleLike 选择）、
                       visual-check.mjs / m2-check.mjs / editor-check.mjs /
                       addcards-check.mjs / browser-check.mjs、seed-card.mjs、
                       set-tts-template.mjs、check-prefs21.mjs

ts/routes/decks/      新写的牌组列表页面（主窗口默认页）：+page.svelte（牌组树、计数、
                      折叠、概览面板、齿轮菜单、底部三按钮、快捷键 d/s/t/a/b）
ts/routes/cards/      新写的「卡片…」对话框（编辑器工具栏，上游是 Qt 的 clayout.py）：
                      +page.svelte（三块编辑区 + 卡片类型增删改重排 + 选项菜单 +
                      RenderUncommittedCard 预览）
ts/routes/browser/    新写的浏览器页面：+page.svelte（搜索/排序/多选/动作表/快捷键）、
                       Toolbar.svelte、Sidebar.svelte、ResultRow.svelte（逐行取数）、
                       ContextMenu.svelte（行右键菜单，`data-action` 供验证脚本点击）、
                       FormModal.svelte（到期日/重排/重置/查找替换共用的表单弹框）、
                       menu.ts（菜单与表单的类型）、DeckPickerModal.svelte、
                       TagsModal.svelte（加/移除标签）
```

### 4.2 安全策略（已对照 mediasrv 实现）

| 机制 | 验证 |
| --- | --- |
| Host 校验 | `Host: evil.com:1234` → 403 |
| Origin 校验 | `Origin: http://evil.com` → 403 |
| 内容类型 | `POST` 必须 `application/binary`，否则 403 |
| API 密钥 | 无密钥 → 403，正确 → 200 |
| 复习页白名单 | 放行 4 个端点 |
| CSP · 可信页面 | `frame-ancestors 'none'`，移除 SvelteKit 的 meta |
| CSP · 不可信页面 | 来源白名单 + script hash（`/editor` 已验证 hash 正确提取） |
| CSP · 媒体文件 | `default-src 'none'` + `sandbox` |
| 媒体文件名 | 只取 `basename`（对照 `AVPlayer.play_file` 的 SECURITY 注释），避免 `[sound:../../x]` 读到媒体目录之外 |

### 4.3 复习闭环（端到端验证过）

```text
bridge 命令: domDone
队列: 共 1 张（新 1 / 学习中 0 / 复习 0）
底部栏（458 字节）: ...
渲染完成: 问题 184 字节 / 答案 211 字节 / 样式 148 字节
bridge 命令: ans
bridge 命令: ease3
队列: 共 1 张（新 0 / 学习中 1 / 复习 0）   ← 评分确实更新了调度状态
```

底部栏按钮文本走 `anki_i18n`，中英文均已验证。

---

## 5. 踩过的坑（**请务必先读这一节**）

这些是实际卡住过的问题，重复踩会浪费大量时间。

### 5.1 `get_services()` 会把 FrontendService 方法复制进 BackendFrontendService

`tauri/build.rs` **必须**过滤掉 `BackendFrontendService`，否则平台端点会被
转给 rslib 并得到 `InvalidServiceIndex`：

```rust
let backend_services = backend_services
    .into_iter()
    .filter(|service| service.name != "BackendFrontendService");
```

这与 `rslib/rust_interface.rs` 的过滤保持一致。

### 5.2 Collection 侧服务的方法是「私有委托」，外部只能走 `run_service_method`

`SchedulerService` / `CardRenderingService` 等在 `impl Backend` 上只生成了
**私有的**同名委托方法。所以：

- ❌ `backend.get_queued_cards(...)` → 报「方法私有」
- ❌ `<Backend as BackendSchedulerService>::get_queued_cards(...)` → 方法不在该 trait 上
- ✅ `backend.run_service_method(service, method, bytes)` —— 这也正是 `rsbridge` 的做法

`tauri/src/backend.rs::call(method, input)` 封装了它；`reviewer.rs` 用方法名调用。

### 5.3 `run_service_method` 是阻塞调用

Python 绑定里用 `py.detach` 释放 GIL 才不卡解释器。Rust 侧必须放进
`tokio::task::spawn_blocking`，否则会阻塞 tokio 运行时、让 WebView 的请求全部排队。

### 5.4 静态资源有两个目录，产物在带下划线的那个

| 路径 | 角色 |
| --- | --- |
| `qt/_aqt/data/web/` | **构建产物**（运行时实际读取） |
| `qt/aqt/data/web/` | 源目录（`.ts` / `.scss` 原文） |
| `out/sveltekit/` | SvelteKit 产物（开发期回退） |

`assets.rs` 按「产物目录 → 源目录 → out/sveltekit」顺序查找。

### 5.5 SvelteKit 的 CSP meta 必须移除

SvelteKit 为静态托管写入了严格 CSP，本地 WebView 会因此拒绝执行脚本。
可信页面需剥离该 meta，不可信页面则保留并叠加自己的策略。

### 5.6 语言代码有两种格式

- `ftl/` 目录名用**连字符**：`zh-CN`
- Anki 设置里用**下划线**：`zh_CN`

`main.rs` 统一 `replace('_', "-")`。用错格式会静默回退到英文。

### 5.7 `bridgeCommand` 是前端交互的结构性依赖

`window.bridgeCommand` 由 Qt 的 QWebChannel 注入。没有它，复习界面的所有按钮、
编辑器快捷键都会抛 `TypeError`。`src/bridge.rs` 的垫片是必修项，不是可选优化。

### 5.8 测试脚手架本身会骗人（两次）

1. **漏 `cargo build`** —— 有一次验证「鉴权失败」，排查后发现测试命令没编译，
   跑的是旧二进制（用随机密钥）。
2. **用长度做断言** —— 中英文底部栏 HTML **都是 458 字节**，一度以为翻译没生效。
   实际是 `Edit+More+Shortcut key` 与 `编辑+更多+快捷键` 在 UTF-8 下恰好等长。
   **打印内容，不要只比长度。**

### 5.9 PowerShell 与 .NET 的当前目录可能不一致

`[System.IO.File]::WriteAllBytes("a.bin")` 与 `curl --data-binary "@a.bin"`
可能解析到不同目录。**写/读临时文件一律用绝对路径。**

### 5.10 部分工具会拦截「无显式编码」的文件写入

`Set-Content` / `Get-Content` 在未指定编码时会被阻止。改用
`[System.IO.File]::WriteAllBytes` + `Encoding.UTF8.GetBytes`，或 `-Encoding utf8`。

同理，删除文件前先 `Test-Path`：某些删除钩子会对「路径不存在」直接报错终止脚本，
`-ErrorAction SilentlyContinue` 也压不住。

### 5.11 前端构建还缺一个环节：`ftl.ts`

本文档 §2.1 原来漏了它。`ts/` 的 `@generated` 别名指向 `out/ts/lib/generated`，
而其中的 `ftl.ts`（i18n 的 TS 接口）由 `anki_i18n` 的 build script 在**编译期**
通过 `STRINGS_TS` 环境变量写出：

```powershell
$env:STRINGS_TS="$PWD/ts/lib/generated/ftl.ts"
cargo build -p anki_i18n
Copy-Item ts/lib/generated/ftl.ts out/ts/lib/generated/ -Force
```

漏掉它，`yarn build` 会报 `Could not load .../out/ts/lib/generated/ftl`。
注意 `ts/bundle_ts.mjs`（打包 `reviewer.js`）还依赖 `.svelte-kit/tsconfig.json`，
因此 legacy 产物必须在 `yarn build` **之后**再生成，否则报
`Could not resolve "@tslib/bridgecommand"`。

### 5.12 共用一个页面 ⇒ 底部栏 CSS 会污染卡片页面

Qt 把底部工具栏放在独立 WebView（`qt/aqt/reviewer.py:358`），所以
`toolbar-bottom.css` / `reviewer-bottom.css` 的 `body` 规则只作用于底部栏。
本方案是两个界面共用一个页面，实测后果：

- `reviewer-bottom.css` 的 `body { margin: 0; padding: 0 }` 盖掉了 `reviewer.scss`
  的 `body { margin: 20px }` → 卡片内容紧贴窗口边缘；
- `toolbar-bottom.css` 的 `body { overflow: hidden }` → 长卡片无法滚动；
- 底部栏留在文档流里，**不会**停在窗口底部，而是悬在卡片下方。

修法见 `src/legacy.rs` 的 `REVIEWER_ISOLATION_CSS`：不加载 `toolbar-bottom.css`，
并把 `#bottom-container` 固定到窗口底部、`body` 恢复 20px 内边距。用
`tools/visual-check.mjs` 可以直接看到 `body` 的 computed margin 与
`#bottom-container` 的位置。

### 5.13 窗口截图会因 DPI 虚拟化而骗人

在缩放 >100% 的屏幕上，未声明 DPI 感知的进程调用 `GetWindowRect` 拿到的是被
虚拟化的偏小坐标，而 GDI 按物理像素拷贝同样大小的区域 —— 截图里窗口「只截到
一部分、右侧按钮消失」，看起来极像布局缺陷（本机 175% 缩放下只截到窗口的 2/3）。

`tools/capture-window.ps1` 在抓图前调用 `SetProcessDpiAwareness(2)` 解决。
**先确认截图工具本身正确，再下「布局坏了」的结论。**

### 5.14 `serde-pickle` 的 API 与直觉不同

- `None` 是 `Value::None`（没有 `Null`）；
- dict 的键是 `HashableValue`，不是 `Value`；
- 写回要用 `serde_pickle::value_to_vec(&Value, SerOptions)`；`to_vec` 要求
  `T: Serialize`，而 `Value` 并未实现它。

另外**不要在读写过程中把 pickle 转成 JSON**：`mainWindowGeom` 是 `bytes`，
整体重写时会被破坏。正确做法是保持 pickle 原值，只在返回前端那一步才转换。

### 5.15 自定义调度的状态是 proto JSON，写错字段会直接报错

前端 `ts/reviewer/answering.ts` 把调度状态转成 **proto JSON** 交给用户脚本，回写时
用 `SchedulingStates.fromJson()`。因此：

- 字段名是 camelCase（`scheduledDays`），不是 proto 里的 snake_case；
- `SchedulingState` 是嵌套 oneof，正确路径是
  `states.good.normal.learning.scheduledSecs` 这种；写成 `states.good.scheduledDays`
  会让 `fromJson` 因「未知键」抛错；
- 抛错会中断整条链路，但注入脚本结尾的 `.finally()` 仍然上报 `statesMutated` ——
  宿主只看到「已完成」，却没有任何状态被提交。排查时很容易卡在"看起来跑通了"。

为此本外壳在注入脚本里额外加了 `.catch()`，把错误经 `stateMutationError:` 回传并
写进日志（Qt 只用 `evalWithCallback` 检测语法错误，运行期异常会静默丢失）。

### 5.16 多窗口下桥接命令必须带窗口身份

注入脚本在**每个**窗口加载时都会发 `domDone`，而所有窗口共用一条
`/_anki/pycmd` 通道。没有窗口身份时，「打开牌组选项」会被当成复习界面启动 ——
复习会话被无端重启（实测：取队列日志出现了两次）。

修法：注入脚本给每个内部请求带 `X-Anki-Window: <窗口标签>`，`server/pycmd.rs`
据此分流（复习相关命令只接受主窗口，其他窗口只处理关窗与录音命令）。
`tools/verify-m2.ps1` 会断言「附加窗口的 `domDone` 被忽略」且「取队列只出现一次」。

### 5.17 legacy 页面缺 charset：中文全成乱码

Qt 的 `legacy_page_data` 用 `Response(html, mimetype="text/html")`，Werkzeug 会自动
补上 `charset=utf-8`；我们复刻时发了裸 `text/html`，于是录音窗口的中文标签被浏览器
按系统默认编码（中文 Windows 是 GBK）解析成「褰曢煶涓�」这类乱码 ——
**布局是对的、只是字全错**，很容易误判成字体或渲染问题。

修法：`server/legacy.rs` 显式发 `text/html; charset=utf-8`，`legacy::render` 里再补
`<meta charset="utf-8">`。新增 legacy 页面时不要绕开 `render()`。

### 5.18 与 rslib 同名的端点必须让平台层接管

Qt 的 `mediasrv.post_handler_list` 会**盖过**后端服务（`_handle_dynamic_request` 先查
处理器表，`mediasrv.py:1343`），而本外壳的服务端是"先查方法表" —— 于是两边都有的名字
会被 rslib 接走，语义与 Qt 不一致。

踩到的例子：`getConfigJson`。Qt 的实现缺键返回 `null`（`mediasrv.py:834-838`），
rslib 的同名方法对缺失的键直接报错。编辑器要读 `renderMathjax` / `shrinkEditorImages` /
`closeHTMLTags` 这些布尔项，而新建集合的 `col.conf` 里根本没有它们（默认值在 rslib 的
注册表里，不落库），结果是：

1. 页面弹出错误框（`post.ts` 对失败的 RPC 会 `alert`）；
2. **`alert()` 阻塞页面的 JS 事件循环** → `loadNote` 的 promise 永远不落地 →
   宿主注入的 `.then(() => pycmd("editorLoaded"))` 永远不执行。

现象极具误导性：窗口里字段**已经渲染出来了**（卡在 `loadNoteInner` 的后半段），
但宿主侧看不到任何完成信号。修法：`platform::SHADOWED_METHODS` 列出要接管的名字，
`server/api.rs` 查方法表前先跳过它们。

**排查方法**：遇到"页面上看着好好的、宿主却收不到回调"，先截图看有没有 `alert`。

### 5.19 只读连接救不了热日志（强杀进程的代价）

`prefs21.db` 的读路径原先用 `SQLITE_OPEN_READ_ONLY` 打开。如果上一次写入被中途打断
（验证脚本的 `Stop-Process -Force`、断电），文件旁边会留下 `prefs21.db-journal`，
需要 SQLite 先做恢复 —— 只读连接做不到，`prepare` 就会失败：
`attempt to write a readonly database`。

后果是**所有 profile / meta 读取一起挂**（`getProfileConfigJson`、`getMetaJson`），
编辑器首屏要读 `getMeta(tagsCollapsed)`，于是又回到 5.18 那条链路：弹错误框、卡住页面。

修法：读路径也用读写方式打开（Qt 那边就是普通 `sqlite3.connect()`）。已并入
`Prefs21::connect()`，读写两条路径共用。

### 5.20 Playwright 抓到的响应体可能是空的（不是服务端的错）

写「添加卡片」的检查时踩到的：`page.waitForResponse("**/_anki/addNote")` 拿到的响应是
`HTTP 200` + **0 字节**，`response.body()` 也是空 —— 看起来像后端没返回内容。实际上：

- 页面自己拿到了正常响应（`addNote` 之后表单被 `loadNote` 重置、note 也确实进了集合）；
- 同一个请求用 curl 直发，响应是 29 字节的正常 `AddNoteResponse`。

结论：**别用"读响应体"来断言这类成功**。改成两条更硬的证据：

1. 用 `page.route(..., route.continue())` 拦下请求，读**请求体**（`postDataBuffer()`，
   能直接 `AddNoteRequest.fromBinary` 解出页面交上来的字段）；
2. 回到集合里查（`searchNotes` → `getNote`），断言 note 真的存在、内容正确，再删掉。

顺带两个同源的小坑：`route.request().postDataBuffer()` 在请求体**为空**（例如
`Bool{val:false}`，protobuf 零字节）时返回 `null`，必须先兜底成空 `Uint8Array`；
`Get-Content -Raw` 读**空日志文件**返回 `$null`，`[regex]::Matches($null, …)` 会抛异常
（PowerShell 里 `-match` 不会，所以老脚本没暴露这个问题）。

### 5.21 浏览器的表格：缺 `setActiveBrowserColumns` 会整张空白

写浏览器页面时踩到的：`browserRowForId` 直接回 500 `Active browser columns not set.`
（`rslib/src/browser_table.rs:276-285`），表现出来是**表格整张空白**。

正确顺序（对照 `pylib/anki/collection.py:890-896` 的 `load_browser_card_columns()`）：

1. 读集合配置 `activeCols`（缺省 `["noteFld","template","cardDue","deck"]`，
   见 `pylib/anki/browser.py:33`）；
2. `setActiveBrowserColumns({vals: [...]})` —— 它只是把列清单塞进
   `CollectionState.active_browser_columns`（`rslib/src/search/service/mod.rs:130-133`），
   **不落库**，所以每次开页面都要重新设；
3. 再 `browserRowForId`（逐行；后端没有批量接口）。

另外：这个 500 会被 `post.ts` 的默认 `alertOnError` 变成 `alert()`，**卡住页面的 JS
事件循环** —— 与 §5.18 同一个坑。浏览器页面对所有 RPC 都显式传了
`{ alertOnError: false }`，把错误留在表格里显示。

### 5.22 文案显示成 `missing-key: xxx` = 页面没加载那个 i18n 模块

新写的页面必须在 `setupI18n({ modules: [...] })` 里列全用到的**模块**（模块 = ftl
文件名）：浏览器页面一开始只加载了 BROWSING / DECKS / NOTETYPES，工具栏上的
`actions-delete` / `actions-preview` 就直接渲染成 `missing-key: actions-delete`。
现象很容易漏掉 —— 自动化断言抓不到（它们按 id/选择器定位，不看文案），只有截图才看得出来。

对照 `ts/routes/editor/base.ts` 的 `editorModules` 按需加：`ModuleName.ACTIONS`、
`ModuleName.EDITING` 等。另外 Rust 侧要读 `ftl/qt` 的文案（`qt-misc-*`）时，类型化方法
不存在，得用 `I18n::translate`（见 §7.4）。

### 5.23 虚拟列表会复用行组件，行数据必须跟着 id 重新取

`VirtualTable` 的行是 `{#each slice as index (index)}` —— **以行号为 key**。搜索换了一次
结果（"2 行 → 1 行"这种）之后，行号 0 对应的卡片已经变了，但 Svelte 复用同一个
`ResultRow` 实例，只在 `onMount` 里取数的写法就会一直显示**上一次搜索的内容**。

现象很有迷惑性：搜索框里是新词、状态栏的总数也对，只有表格内容是旧的（`verify` 的
「搜到了新建的测试 note」就是这么抓出来的）。修法：把取数搬到 `$: if (loadedId !== id)`
的响应式语句里，并在返回时再确认一次 `loadedId === cardId`（期间又换卡片就丢弃）。

### 5.24 给浏览器加动作时的两个小坑

1. **`BrowserRow` 不带 id**：它只有 `cells` / `color` / 字体（`proto/anki/search.proto`
   的 `BrowserRow`）。行的 id 一直都在结果列表里（卡片模式 cid、笔记模式 nid），
   所以「当前行的 note」要 `getCard` 反查、「当前行的卡片」要 `cardsOfNote` 展开；
   旗标 / 暂停 / 埋藏可以从 `color` 推（`flag = color - FLAG_RED + 1`），但**标记要从
   笔记标签里读**（既标记又有旗标时颜色只显示其中一个）。
2. **`qt` 仓库的文案在 TS 侧没有类型化函数**：生成器只给核心仓库生成函数
   （`rslib/i18n/write_strings.rs` 的 `is_core()`），而 `qt-accel-*`（Undo / Redo /
   Info / Set Due Date / Find…）都住在 `ftl/qt/qt-accel.ftl`。要用就得
   `import { translate } from "@generated/ftl-helpers"` 按 **key** 取，并把
   `ModuleName.QT_ACCEL` 加进 `setupI18n`（否则拿到的是 `missing key: ...`）。
   Rust 侧同理：`I18n::translate`（§7.4 把它放开为 `pub`）。文案里的 `&` 是 Qt 的
   助记符，网页上要替换掉。

### 5.27 三件复习菜单相关的坑

1. **会话里的 `card` 是快照**：`Reviewer` 的 `session().card` 是显示这张卡时取回来的，
   菜单改过旗标之后它不会变 —— 拿它算 toggle（"同旗标再点一次 = 清除"）必然判错，
   勾选态也会永远停在最初。凡是要读**会被自己改掉**的字段，都重新 `getCard`
   （`reviewer_menu::fresh_flag`）。
2. **宿主注入的脚本怎么测**：菜单画在页面里、但脚本由宿主 `eval` 注入 WebView2，
   Playwright 看不到那份 DOM。做法是把脚本放进 `tauri/assets/reviewer-menu.js`、
   宿主用 `include_str!` 引用，验证脚本**读同一个文件**塞进空页面测
   （渲染 / 勾选态 / 子菜单 / 表单回传 / 右键）——"测的不是跑的那份"就避免了。
   没被覆盖的是"注入本身"，那部分靠 `verify-reviewer.ps1` 截真实窗口的菜单来兜底。
3. **node 退出时的 libuv 断言**（`!(handle->flags & UV_HANDLE_CLOSING)`）：
   脚本里对本地服务发过 `fetch` 之后直接 `process.exit()`，keep-alive 的 socket
   句柄会在退出时触发它 —— 看起来像被测程序崩了，其实是被自己脚手的连接。给请求加上
   `Connection: close` 就干净了（`reviewer-menu-check.mjs`）。同一现象在
   `m2-check.mjs` 里也见过（那里是挂着的 `recordAudio`）。

### 5.26 页面用的 i18n 模块要漏一个，连文件名都会坏

`setupI18n({ modules })` 是按页面**用到**的文案列模块的（见 `+page.svelte` 的
`onMount`）。漏掉哪个模块，那个 key 就渲染成 `missing key: <key>` —— 界面上只是难看，
但**如果这个字符串被拿去做别的事就会出事**：导出对话框用
`tr.exportingCollection()` 拼建议文件名，没加载 `EXPORTING` 模块时文件名成了
`missing key: exporting-collection-2026-…`，里面那个**冒号在 Windows 上非法**，
于是"保存路径"是个无效路径、导出必然失败，而日志里只有一行
`ANKI_SAVE_PATH 生效，跳过保存对话框: …missing key: …`。

两个教训：
- 新页面/新对话框的文案，先确认对应 `ModuleName` 已加到 `setupI18n`；
- 验证脚本里加一条"界面上没有 `missing key`"的断言（`browser-check.mjs` 的
  「导出对话框的文案已本地化」就是干这个的）—— 比事后看日志快得多。

### 5.25 Svelte 不追踪函数体里的响应式变量（行高亮不刷新）

`ResultRow` 的行高亮原来是 `selected={isSelected(id)}`：`isSelected` 里读的 `selected`
**不算模板表达式的依赖**（Svelte 只看表达式里直接引用的变量），于是"只改 `selected`、
不动别的状态"时高亮不刷新 —— Ctrl+A 全选、反选都能复现：状态栏的「已选取 N 张」和
`browserTitle` 都对，只有表格里的高亮是旧的。

修法：传数组让表达式直接引用它（`selectedIds={selected}`，组件内
`class:selected={selectedIds.includes(id)}`）。

顺带记两条同时踩到的验证经验：

- 断言要**同时看页面状态与 DOM**（状态栏计数 + `browserTitle` 是前者，`tr.selected`
  是后者），只看一个会漏掉这类"状态对了但没画出来"的 bug；
- 虚拟列表 `VirtualTable` 会在首尾插**撑高度的空 `<tr>`**（只有一个没有 `title` 的
  `<td>`），`browser-check.mjs` 里数行要过滤掉它们（`tbody tr:has(td[title])`），
  否则会多算一行。

### 5.28 主窗口切页：先撤复习态，再导航

给主窗口加第二个页面（牌组列表 `/decks/`）时最容易踩的坑：切页是**整页导航**
（`window.location.href`，与卡片信息窗口换卡片同一招），而 `domDone` 是**每个页面
都会发**的（`bridge.rs` 的初始化脚本在每次导航后重跑）。`pycmd.rs` 的 `domDone` 分支
只看 `reviewer_active`：它是**进程级、只在启动时写过一次**的原子标志。所以：

- 从复习页切到牌组列表（`d` / `overview`）**必须先把 `reviewer_active` 置回 false**
  （`main_window.rs::show_decks_page`），否则牌组列表的 `domDone` 会照着复习页的流程
  去 `reviewer::start`，往不存在的 `#qa` / `#bottom-container` 注入 DOM；
- 切回复习页（`study`）则要先置 true，再导航 —— 顺序不能反，`domDone` 可能比你的
  下一条语句先到。

另外两条同源的小事：

- 复习页是 legacy 页面（`legacy::reviewer_page(port)` 每调一次就注册一份新 HTML），
  所以 URL 用 `OnceLock` 缓存；启动页就是复习界面时由 `main.rs` 先 `remember_reviewer_url`
  （否则同一个复习页会被注册两次）。
- 新页面要记得加进 `server/assets.rs` 的 `SVELTEKIT_ROUTES` 白名单，展示用户内容
  （牌组名）的还要加进 `UNTRUSTED_SVELTEKIT_ROUTES`（多一份 script hash 校验）。

还有两条前端小坑：`addDeck(Deck)` 的 `kind` 是**必填 oneof**（缺了后端报
`missing kind`）；SvelteKit 会把 `/decks/?deck=1` 规范化成 `/decks?deck=1`，
验证脚本的正则两种都要接受。

### 5.29 注入文件的两个坑：模板字符串里的反引号、`include_str!` 要重建

给复习界面加顶部工具栏（`assets/reviewer-toolbar.js`）时连踩三次，都值得记住：

1. **间距要落在真正放链接的那一层**。第一版照抄 Qt 的 `_body` 结构
   （`.header > .toolbar > a`），却把 `display:flex; gap` 写在 `.header` 上 ——
   `.header` 只有一个孩子（`.toolbar`），`justify-content:center` 生效、`gap` 不生效，
   四个链接挤成一串。**只断言 CSS 属性是查不出来的**（`getComputedStyle(...).gap`
   照样返回 `20px`），得断言相邻元素的 `getBoundingClientRect()` 差值
   （`reviewer-menu-check.mjs` 里就是这么改的）。另外这里改用 `margin` 而不是 `gap`：
   不受 Chromium 版本影响，语义也更贴（间距属于链接自己）。
2. **模板字符串里不能出现反引号**。这份脚本用 `` const CSS = `...` `` 装样式，
   我在其中的 CSS 注释里写了 `` `gap` ``，直接提前结束模板串 ⇒ 整个脚本语法错误 ⇒
   工具栏**整个消失**（而宿主那边照旧"注入成功"）。写这种文件时顺手 `node --check`
   一下最省事。
3. **`include_str!` 的内容改了必须重新 `cargo build`**：只改 `assets/*.js` 而不重建，
   跑的还是旧二进制里那份（我因此白看了两张"没变化"的截图）。验证脚本读的是**源文件**，
   所以那种情况下 `reviewer-menu-check.mjs` 会**通过**而真实窗口不对 —— 这类"测的不是
   跑的那份"要用「改完就重建」+ 真实窗口截图兜底（§5.27 第 2 条的另一面）。

顺带修掉一个隐患：宿主每次 `start()` 都会 `eval` 整份脚本，顶层的 `const` 第二次会
被判重复声明（抛 SyntaxError）。文件整体包了一层 IIFE，`window.installToolbar` 仍挂在
全局，验证脚本照旧能用。

### 5.31 「函数体里的响应式变量」还会再咬一次（自定义学习页）

写 `ts/routes/custom-study/` 时又踩了 §5.25 的坑，而且是**升级版**：选项文案依赖后端
`customStudyDefaults`（「可用的新卡片：N」），我写成 `function options()` 里读 `defaults`、
模板里直接调 —— `defaults` 回来之后提示**不刷新**（Svelte 只追踪表达式里直接引用的
变量，看不见函数体里读的那些）。

第一反应是把调用挪到响应式变量上：`$: opts = options();` —— **照样不管用**：Svelte 看的是
这条语句里出现的变量，`options` 只是个函数名，`defaults` 依然不可见。真正管用的是
**把它变成显式参数**：`$: opts = options(defaults);`（函数签名收 `source`），这样
`defaults` 出现在语句里，赋值就会重跑。

一句话总结：**想让 Svelte 跟着变，就得让变量出现在响应式语句里** —— 要么直接写在模板
表达式里，要么当参数传进去，别指望它穿透函数体。检查脚本里也留了一条断言
（`custom-study-check.mjs` 的「第一项旁边显示了可用新卡片数」）守着这个坑。

### 5.30 图像遮挡：先核对"有没有界面"，再动手

`tauri/PARITY.md` 原来写着「编辑器里的『遮挡』按钮要走 `qt/aqt/image_occlusion.py`」——
**这个仓库里根本没有那个文件**，也没有 `dialogs.py` 里的 Occlusion 对话框。这个版本的
遮挡编辑器是**内嵌在编辑器页面**里的（`NoteEditor.svelte:1470-1488` 渲染
`ImageOcclusionPage`），工具栏的 `image-occlusion-button` 只切一个 store；Qt 侧对应的是
`editor.py:500-535` 的 `setup_mask_editor`（同样往页面里 eval）。所以"接入"根本不需要
开窗口，只需要一条 `openFilePicker` 端点 —— 这类"清单上的臆测"最好先花五分钟核对上游，
省得照着不存在的文件设计。**做之前先 grep 上游文件是否存在。**

同一件事上还有三个小经验：

1. **add 模式与 edit 模式的取图路径不同**：add 走 `getImageForOcclusion({path})`（读文件），
   edit 走 `getImageOcclusionNote(noteId)`（读**已保存**的笔记）。编辑已有笔记时刚换的图
   还没落库 ⇒ 掩码编辑器拿到空图（`data:image/png;base64,`，22 字符）。这是上游的时序，
   与本外壳无关；检查脚本因此走 add 模式。
2. **断言"图片真的渲染了"要卡住长度**：空图与真图的 src 都通过 `startsWith("data:image/")`
   —— 只有长度能区分（空 ~22 字符，100x80 纯色 PNG ~360 字符）。另外 1x1 的测试图太小，
   索性用 Chromium 现截一张 100x80 的图当 fixture（`occlusion-check.mjs` 里就这么做的）。
3. **`ioMaskEditorVisible` 的初值不一定是 false**：遮挡笔记类型下页面可能一进来就是掩码
   编辑器状态，检查脚本"点一下工具栏按钮"会把它**关掉**。要先看状态、缺了才点（上面的
   检查脚本里就是这么写的）。顺带：点工具栏的「重置」会弹原生 `confirm`，Playwright 要
   `page.on("dialog", d => d.accept())` 才能过。

---

## 6. 剩余工作

按建议优先级排序。

### 6.1 复习界面收尾（价值最高，改动集中）

**已完成**：底部栏回到窗口底部、卡片内容恢复 20px 内边距与滚动（原先底部栏悬在
卡片下方、内容紧贴窗口边缘，根因见 §5.12），并补上了由 `--bottom-bar-height`
驱动的卡片区留白。

| 项 | 说明 |
| --- | --- |
| ~~**计时与时间盒**~~ | **已完成**：`showQuestion(button, maxTime)` 的 `maxTime` 取自牌组 `timer` / `maxTaken`；`showAnswer` 传 `stopTimerOnAnswer` |
| ~~**按钮下次间隔**~~ | **已完成**：用 `DescribeNextStates` 在按钮上标注「<1分 / 10分 / 1天 / 3天」 |
| ~~**按钮数量**~~ | **无需改动**：v3 下 Qt 的 `answerButtons()` 恒返回 4（2/3 个是 v2 遗留，v3 对 v2 是 assert 拒绝） |
| ~~**`more`（更多菜单）**~~ | **已实现**：底部栏「更多」与卡片右键 → 13 项菜单 + 旗标子菜单 + 重置/到期日表单（`reviewer_menu.rs` + `assets/reviewer-menu.js`）。差上一张卡片信息、音频 5 项、自动前进，见 `tauri/PARITY.md` 的 2.2-2.4 |
| ~~**`edit`**~~ | **已完成**：打开「编辑当前卡片」窗口。编辑器外层窗口全在 Web 里，宿主只需开窗 + `editorReady` 后注入 `loadNote`，见 `tauri/README.md` 的「编辑器」 |
| ~~**`more`**~~ | **已实现**（复习界面）：13 项菜单 + 旗标子菜单 + 两个表单对话框，见 `tauri/README.md` 的「复习界面」；编辑器里的 `more` 仍未做 |
| ~~**撤销**~~ | **已完成**：`undo` 桥接命令 + 页面内快捷键（`Ctrl+Z` / `Cmd+Z` / 裸 `u`），撤销后重新取队列 |
| ~~**音频自动播放队列**~~ | **已完成**：`audio.rs` 改为队列播放（对照 `AVPlayer.play_tags`），显示问题/答案时按牌组配置自动播放 |
| ~~**类型作答**~~ | **已完成**：问题面插入 `#typeans` 输入框、答案面用 `CompareAnswer` 生成对比；输入值由桥接垫片附在 `ans:` 后面回传 |
| ~~**计数显示**~~ | **已完成**：对照 `Reviewer._remaining()`，「显示答案」按钮上显示「新 + 学习中 + 复习」，当前队列加下划线 |
| ~~**界面状态同步**~~ | **已完成**：`getSchedulingStatesWithContext` / `setSchedulingStates` + `cardStateCustomizer` 注入，见 `platform/scheduling.rs` |
| ~~**顶部工具栏**~~ | **已完成**：牌组 / 添加 / 浏览 / 统计 + 快捷键提示，宿主注入 `assets/reviewer-toolbar.js`。差同步链接与 add-ons 托盘（`qt/aqt/toolbar.py:353-418`），见 `tauri/README.md` 的「顶部工具栏」 |

### 6.2 桥接命令扩充

目前覆盖 `domDone` / `ans` / `ans:<typed>` / `ease1`..`ease4` / `undo` /
`statesMutated` / `stateMutationError:` / `edit` / `add` / `browse` /
`browserSearch:<query>` / `preview`（编辑器的预览按钮）/ `more`（复习界面的「更多」
与卡片右键）/ `rev:<动作>[:<参数>]`（复习菜单里点的一项）/ `stats`（统计）/
`preferences`（设置）/ `decks`（牌组列表）/ `overview`（当前牌组的概览）/
`ioImageLoaded:<path|nid>`（编辑器的遮挡图片已载入，只记日志，对照 `editor.py:364`）/
`study`（开始复习）/ `opts:<did>`（牌组选项窗口）/ `import`（导入窗口）/
`shared`（打开共享牌组页）/ `editorReady` / `close`
（主窗口），以及非主窗口的 `browserReady` / `browserTitle:<notesMode>:<selected>:<total>` /
`editCard:<cid>` / `editNote:<nid>` / `preview:<cid>` / `cardInfo:<cid>` /
`addCopy:<nid>:<deckId>` / `changeNoteType:<nid>,...`、预览窗口的
`previewNext` / `previewPrev` /
`previewReplay` / `previewBothSides:<0|1>`、`saveRecording` / `cancelRecording`
与编辑器的状态跟踪命令。
待实现清单（都会打 debug 日志）：`more`、`updateToolbar`、`repaintNeeded`、
`toggleSticky`、`saveTags`、`paste`、`cutOrCopy` 等。

> 注意 `js/reviewer/index.ts` 里的 `isLegacy` 分支：新路径走 RPC，
> 旧路径走 `bridgeCommand`。优先确认某个命令是否真的还需要。

### 6.3 M2 收尾情况（已完成）

**M2 已完成**：mediasrv 的 32 个平台端点全部有实现（M3 期间又加了外壳自己的
`saveFileDialog`，见 §3.7）。逐项收尾情况：

- ~~**录音**~~ —— **已实现**：`cpal` 采集 + `hound` 写 WAV，录音窗口是宿主自建的
  legacy 页面（红点 + 计时 + 保存/取消），关闭窗口等同取消。与 Qt 的差异见
  `tauri/README.md` 的「录音」（Qt 会调外部 `lame` 转 MP3；取消时 Qt 不会回复
  请求方，我们会返回空路径）。**Android 未验证**：Qt/AnkiDroid 用 `MediaRecorder`，
  这里统一用 `cpal`（Android 走 oboe），需真机确认。
- ~~**窗口模型**~~ —— **已定案并实现**（见 §3.6）：多窗口 + 单实例 + 模态 + 几何记忆，
  `deckOptionsReady` / `deckOptionsRequireClose` / `importDone` /
  `importDialogRequireClose` 已可用；**其余端点也都收口了**（M3）：`closeAddCards` /
  `closeEditCurrent` 见 §6.4，`searchInBrowser` 把 `SearchNode` 翻成搜索词再开/复用
  浏览器窗口（§3.7，实现在 `server/api.rs`），`openFieldsDialog` / `openCardsDialog`
  各自开着新写的 `ts/routes/fields/` 与 `ts/routes/cards/`（见 §6.4）。
  只剩 `saveCustomColours` 卡在原生取色器能力（`rfd` 没有暴露自定义调色板的 API）。
- ~~**剪贴板按任意 MIME 读取**~~ —— **已实现**：`text/uri-list` 在 Windows 上读
  剪贴板的 `CF_HDROP`（编辑器「复制文件 → 粘贴成媒体」依赖它），图片与文本仍走
  `arboard`。仍未覆盖的是**拖入文件**（HTML5 drop），属 M3 的编辑器链路。
- ~~`getConfigJson` / `setConfigJson`~~ —— 这两个由 rslib 的 `ConfigService` 直接
  服务，实测可用（`getAllConfig` → 200），从来不是缺口。

### 6.4 M3 / M4

- **M3**：编辑器（`ts/routes/editor/`，89 个文件，依赖剪贴板/图片/录音）、
  导入导出（**Android 必须走 SAF**）、图像遮挡。
  **进度**：编辑器外壳、浏览器、牌组列表、图像遮挡都已完成；导入导出里 apkg/colpkg
  已接（浏览器动作表），还差导出 CSV/纯文本与导入的端到端验证。
  - **已完成**：编辑器外壳的两个窗口。
    - 「编辑当前卡片」（`mode=current`）：`tools/verify-editor.ps1`；
    - 「添加卡片」（`mode=add`）：`tools/verify-addcards.ps1` —— 含默认牌组/笔记类型
      （页面自己算，宿主只给 `reviewerCardId` + `initial:true`）、点「添加」真的落库、
      以及「有输入 ⇒ 弹确认框 / 无输入 ⇒ 直接关」两条关闭契约（标题栏 × 也会被拦到这条
      路径上）。
    - 两个窗口共用同一个页面，**注入参数必须按窗口标签分流**；改动前请先读
      §5.18（同名端点接管）、§5.20（别读 Playwright 响应体）与 `tauri/README.md`
      的「编辑器」。
  - **已完成**：**浏览器窗口**（`tauri/src/browser.rs` + 新写的
    `ts/routes/browser/` + `tools/verify-browser.ps1`）。入口是复习界面 `b`、
    图表 `browserSearch:<query>`、`searchInBrowser` 端点；功能含排序、多选、
    笔记模式、列清单 / 列宽、加标签 / 改牌组 / 删除 + 撤销重做；编辑区因 CSP 限制
    改成独立窗口（`browser-editor`）。改动前先读 §3.7 与 §5.21 / §5.23，以及
    `tauri/README.md` 的「浏览器」。
  - **已完成**：**预览窗口**（`tauri/src/preview.rs` + `legacy::preview_page`）：
    问题/答案、上一张/下一张（清单在浏览器页面手里）、重播音频；关闭浏览器时连带关掉。
  - **已完成**：浏览器动作表（`browser.py:301-401` 的绝大部分）——
    加/移除标签、清除未使用标签、标记、旗标、暂停、埋藏、重置、设置到期日、
    重新定位、立即评分、改牌组、删除、复制笔记、卡片信息、**更改笔记类型**、
    查找替换、跳转、反选/全选/选中笔记。菜单栏挪成了**行右键菜单 + 工具栏「更多」**
    （同一份表，`data-action` 供验证脚本定位）；卡片信息与更改笔记类型都复用已有的
    `card-info/<cid>` / `change-notetype/<id>` 路由，后者的保存由宿主补 `note_ids`
    并关窗（见 §3.7）。
  - **已完成**：列清单 / 列宽 / **列顺序拖动**（拖列头写回 `activeCols`；列宽按卡片/笔记
    分开存），以及行高亮「只改选中集合也要刷新」的修复（Svelte 响应式坑，见 §5.25）。
  - **已完成**：**导出**（apkg / colpkg）—— 对话框与导出 RPC 都在页面里，
    宿主只补一个 `saveFileDialog` 端点（见 §3.7 第 6 条）；验证脚本会真的导出一份
    `.apkg` 并检查它是合法 zip。
  - **已完成**：**牌组列表（主窗口默认页）** —— 新写的 `ts/routes/decks/`
    + `tools/verify-decks.ps1`：牌组树与今日计数、折叠、点牌组 → 概览（`?deck=`，
    同一页面的面板）→ 学习、齿轮菜单（重命名 / 选项 / 删除）、底部
    「获取共享牌组 / 创建牌组 / 导入文件」、工具栏、`d`/`s` 快捷键。
    页面切换在 `main_window.rs`（整页导航 + `reviewer_active` 的开关顺序，
    **切页前先读 §5.28**）；启动默认页从 `graphs` 改成了 `decks`（与 Qt 一致）。
  - **已完成**：**「字段…」对话框**（4.1）—— 编辑器工具栏的「字段」按钮（前端端点
    `openFieldsDialog`）→ 新写的 `ts/routes/fields/`：字段增删/改名/重新定位、
    编辑字体、描述、排序字段与四个勾选；保存走 `updateNotetype`（整份 notetype）。
    宿主在注入 `loadNote` 时记「窗口 → 笔记类型」（平台端点拿不到来源窗口）。
    验证见 `tools/fields-check.mjs`（由 `verify-editor.ps1` 调用）。
    差：系统字体下拉、拖动重排、删除时「有 N 张笔记用到它」的提示。
  - **已完成**：**「卡片…」对话框**（4.2）—— 编辑器工具栏的「卡片」/ `Ctrl+L`（前端端点
    `openCardsDialog`，上游 `qt/aqt/clayout.py` 有 884 行）→ 新写的 `ts/routes/cards/`：
    正面/背面/样式三块编辑区、添加字段、翻转、卡片类型增删改重排、恢复默认、
    浏览器外观、牌组覆盖、复制信息，预览走 `RenderUncommittedCard`（**未保存**的改动
    立刻可见）；保存走 `updateNotetype`（整份 notetype）。宿主在 `editor.rs` 里除了
    「窗口 → 笔记类型」还记了「窗口 → 那张 note」，再带上复习卡的 `ord`（`?notetype=&ord=&note=`）。
    验证见 `tools/cards-check.mjs`（由 `verify-editor.ps1` 调用）。
    差：帮助按钮、系统字体下拉、下拉里没有 Qt 的字段摘要、复制后没有提示。
  - **已完成**：**自定义学习**（3.6）—— 概览里的「自定义学习」（`C`）→ 新写的
    `ts/routes/custom-study/`（六个选项 + cram 四选一 + 标签包含/排除，走
    `CustomStudy` RPC；确定后宿主把主窗口导航到当前牌组的概览）。验证见
    `tools/custom-study-check.mjs`（由 `verify-decks.ps1` 调用）。
    差：`TagLimit` 标签对话框（这里是两个文本框）与「编辑已有筛选牌组」。
  - **已完成**：**图像遮挡**（3.5）—— 核对后发现**这个版本没有独立的遮挡窗口**：
    遮挡编辑器内嵌在编辑器页面里（`ImageOcclusionPage`），由工具栏按钮切换；
    外壳只需放行「选图片」的 `/openFilePicker`（另加了一个测试钩子 `ANKI_OPEN_PATH`）。
    验证见 `tools/occlusion-check.mjs`（由 `verify-editor.ps1` 调用）。
  - **已完成**：复习界面的**顶部工具栏**（牌组 / 添加 / 浏览 / 统计 + 快捷键提示，
    宿主注入 `assets/reviewer-toolbar.js`；差同步链接与 add-ons 托盘，见 §5.29 与
    `tauri/README.md` 的「顶部工具栏」）。顺带核对：**Qt v3 主窗口并没有状态栏**
    （`forms/main.ui` 里没有 QStatusBar），`tauri/PARITY.md` 的 1.3 已按"已对齐"处理。
  - 下一步：导出的 **CSV / 纯文本格式**（要先选列，是另一套对话框）、
    查找重复（原生对话框 + 卡片渲染，`find_duplicates.py` 114 行）、管理笔记类型。
    这些都是"缺界面 / 缺 UI 入口"，没有架构上的障碍。
- **M4**：Windows 打包（**废弃整套 Briefcase 流程**，改 `tauri build`；
  签名对象从 `Anki.exe` 换成 Tauri 产物）+ Android（`tauri android init/build`，
  需处理 `INTERNET` 权限、cleartext、`getFilesDir()`、NDK 交叉编译）。
  详见 `TAURI_MIGRATION_PLAN.md` §8。

### 6.5 工程化

- 把 §2.1 的手工构建步骤接入 `just` 流程。
- 补 `tauri/capabilities/` 目录。
- 替换图标（现为 `favicon.ico` 副本）。

---

## 7. 已知妥协与限制

### 7.1 有意为之

- **保留本地 HTTP 服务**：为了前端零改动。代价是多一个本地监听端口。
- **独立 workspace**：不污染 Anki 构建与 CI，代价是两套依赖树，且用不上 Anki 的测试体系。
- **未用 Tauri 插件系统**：页面是外部 URL，`@tauri-apps/api` 的 IPC 不可用。
- **浏览器页面是新写的**：上游的浏览器是原生 Qt 界面，没有可复用的 Web 页面，
  因此 `ts/routes/browser/` 属于本工程新增（其余窗口都是复用 `ts/routes/` 里的既有页面）。
  功能面已基本对齐（搜索 / 排序 / 多选 / 笔记模式 / 列清单·列宽·列顺序 / 完整动作表 /
  导出 apkg·colpkg），仍缺导出里的 CSV·纯文本格式、查找重复、管理笔记类型与
  「字段…/卡片…」对话框；逐项差异见 `tauri/README.md` 的「浏览器」。

### 7.2 曾经的妥协与解决情况（不是遗漏，已在文档标注）

- ~~10 个窗口管理类端点接受请求但不产生副作用~~ —— **已解决 6/10**：多窗口模型下
  6 个都已落地（4 个转发到对应窗口 + `closeAddCards` 的「放弃输入？」确认 +
  `closeEditCurrent` 直接关窗），另 4 个（`searchInBrowser` / `openFieldsDialog` /
  `openCardsDialog` / `saveCustomColours`）不是窗口模型能解决的，缺的是 Web 界面或
  原生取色器能力，见 §6.3。
- ~~配置存储用本地 JSON 文件，与 Anki 的 `prefs21.db` 不互通~~ —— **已解决**：
  现在直接读写 `prefs21.db`，与桌面版共享（见 §3.5）。
- ~~PNG「质量」语义与 Qt 不等价~~ —— **已解决**：Qt 对 PNG 传的 `quality` 其实
  **不生效**（handler 的默认 `compression = 50` 优先，按 `(50*9)/91` 得到 zlib
  level 4），现在显式用 `CompressionType::Level(4)` 对齐。
- ~~音频：无变速、无队列~~ —— **已解决**：`playFile` 与复习界面都走队列播放
  （对照 `AVPlayer.play_tags`），显示问题/答案时按牌组配置自动播放。Qt 本身也没有
  播放变速（`sound.py` 里没有 speed），不存在「缺少变速」。
- ~~TTS 标签未支持~~ —— **已解决**：走 rslib 的 `writeTtsStream`，与 Qt 在 Windows
  上实际用的 `WindowsRTTTSFilePlayer` 是同一路径（SAPI 那个 rank 更低）。仍未实现
  的是 `current_caller` 区分（editor / reviewer 的打断语义），属 M5 音频质量项。

### 7.3 尚未验证

- ~~没有做过视觉验证~~ —— **已补**：`tauri/tools/verify-reviewer.ps1` 会截取
  WebView2 的真实渲染并做布局断言，首次运行就查出两个真实缺陷（见 §5.12）；
  录音窗口（`tools/verify-m2.ps1` 配合人工截图）也查出过 charset 乱码（见 §5.17）。
  尚未人工核对卡片样式细节、深色模式与多语言排版。
- 未跑 Anki 的 Playwright e2e。
- **录音**：Windows 上已端到端验证（22 秒录音产出 2,107,244 字节的 48 kHz WAV）。
  **Android 未验证**（那里 Qt 用 `MediaRecorder`，本方案统一用 `cpal`）。
- **「添加卡片」的确认框只验证到「停住」**：`verify-addcards.ps1` 断言了
  `val=true` 时会进入确认流程且窗口保持打开，但**不会**自动化地去点掉那个原生模态框
  （点不掉就没法继续跑），「选放弃 ⇒ 关窗」这一半只能人工点一次。
- **浏览器的动作覆盖**：`verify-browser.ps1` 验证了加/移除标签、改牌组、删除（+ 撤销）、
  标记、旗标（含清除）、暂停/埋藏（含恢复）、重置、设置到期日、重新定位、立即评分、
  清除未使用标签、选中笔记、查找替换（真的改了字段）、卡片信息、复制笔记、
  更改笔记类型（真的改了类型）、导出（真的产出 .apkg）、列顺序拖动与两套列宽，
  以及 Home/Ctrl+N/End 跳转与反选/全选。**没验证**：导出到 profile 目录的拦截
  （上游有提示，本方案没做）、查找重复 / 管理笔记类型 / CSV 导出（都没实现）。
  「放弃输入」确认框里"选放弃 ⇒ 关窗"那一半也只能人工点一次（原生模态框）。
- **预览的「仅背面」只验证了开关与配置落库**：断言了 `previewBothSides` 打开后切到
  答案面、关闭后回到问题面，但没有断言「问题与答案同时显示」的排版细节。
- 检查脚本跑完会把集合里的 `previewBothSides` 留成 `false`（它先打开再关掉），
  如果之前是打开的，需要手工再打开。

### 7.4 对上游的改动

两处，都是**纯增量、无行为影响**：

1. `rslib/proto/src/lib.rs` 加了一行 `protobuf!(frontend, "frontend");`，
   用于暴露 `frontend.proto` 的消息类型（其 service 仍被 rust_interface 过滤）。
2. `rslib/i18n/src/lib.rs` 的 `I18n::translate` 从私有改为 `pub`（并补了文档注释）。
   原因：生成的类型化方法只覆盖核心字符串（`Translation::is_core()` 排除了 `qt` /
   `mobile` 仓库），而外壳复刻 Qt 对话框时要用 `qt-misc-*` 这类文案
   （预览窗口的「仅背面」就是 `qt-misc-back-side-only`）。字符串表本身包含全部模块，
   只是缺一个公开入口。

---

## 8. 关键文件索引

| 想看什么 | 去哪里 |
| --- | --- |
| 总体计划、里程碑、风险 | `TAURI_MIGRATION_PLAN.md` |
| 工程说明与完整验证记录 | `tauri/README.md` |
| 原实现（对照用） | `qt/aqt/mediasrv.py`、`qt/aqt/reviewer.py`、`qt/aqt/webview.py` |
| 方法表生成 | `tauri/build.rs` + `rslib/proto_gen/src/lib.rs` |
| 服务分发（索引来源） | `rslib/rust_interface.rs`、`rslib/src/services.rs` |
| 前端 API 入口 | `ts/lib/generated/post.ts` |
| 复习屏前端 | `ts/reviewer/`（`index.ts` 是主逻辑，`index_wrapper.ts` 挂 window） |
| 底部栏前端 | `qt/aqt/data/web/js/reviewer-bottom.ts` |
| CSP / 页面策略 | `qt/aqt/mediasrv.py`（搜 `_CSP`、`untrusted_sveltekit`） |
| 配置存储（`prefs21.db`） | `tauri/src/platform/prefs21.rs`、`qt/aqt/profiles.py` |
| 窗口模型 | `tauri/src/windows.rs`、`qt/aqt/__init__.py` 的 `DialogManager` |
| 录音与 TTS | `tauri/src/platform/record.rs`、`tauri/src/platform/audio.rs`、`qt/aqt/sound.py`、`qt/aqt/tts.py` |
| 编辑器窗口（编辑当前 / 添加卡片 / 浏览器编辑区） | `tauri/src/editor.rs`、`ts/routes/editor/`、`qt/aqt/editor.py`（宿主契约）、`qt/aqt/addcards.py`（关闭确认） |
| 浏览器窗口 | `tauri/src/browser.rs`、`ts/routes/browser/`（新写）、`qt/aqt/browser/browser.py` + `table/model.py` + `sidebar/tree.py` + `find_and_replace.py`（对照用）、`qt/aqt/forms/browser.ui`（动作表与快捷键） |
| 导出（apkg / colpkg） | `ts/routes/browser/ExportDialog.svelte` + `+page.svelte`（对话框与导出 RPC）、`tauri/src/platform/dialogs.rs` 的 `save_file_dialog`（外壳新增端点）、`qt/aqt/import_export/exporting.py`（对照用） |
| 更改笔记类型 | `tauri/src/browser.rs`（开窗）、`tauri/src/server/api.rs`（补 `note_ids` + 关窗）、`ts/routes/change-notetype/`（**既有页面**）、`qt/aqt/changenotetype.py`（对照用） |
| 复习菜单 | `tauri/src/reviewer_menu.rs` + `tauri/assets/reviewer-menu.js`（宿主拼数据、页面画菜单）、`qt/aqt/reviewer.py:998-1056`（对照用）、`tauri/tools/reviewer-menu-check.mjs`（UI + 宿主动作） |
| 统计 / 设置窗口 | `tauri/src/main_window.rs`（开窗）、`ts/routes/graphs/` 与 `ts/routes/preferences/`（**既有页面**）、`qt/aqt/main.py:1331,1459`（对照用） |
| 用户可见差异清单（对齐路线图） | `tauri/PARITY.md` |
| 预览窗口 | `tauri/src/preview.rs`、`tauri/src/legacy.rs` 的 `preview_page`、`qt/aqt/browser/previewer.py` |
| 验证脚本与用法 | `tauri/tools/`（`verify-reviewer.ps1` 复习界面 / `verify-m2.ps1` 平台能力 / `verify-editor.ps1` 编辑当前 / `verify-addcards.ps1` 添加卡片 / `verify-browser.ps1` 浏览器）、`tauri/README.md` 的「验证工具」 |

---

## 9. 下一步建议

M1 / M2 已收尾（32 个平台端点全部有实现），M3 的三个窗口都能用了
（「编辑当前卡片」「添加卡片」「浏览器」，都有一键脚本）。建议：

1. **先跑六个一键脚本确认现状**：`tauri/tools/verify-reviewer.ps1`（复习界面 + 布局
   断言）、`verify-m2.ps1`（平台能力 + 多窗口命令归属，录音那项需要有麦克风）、
   `verify-editor.ps1`（编辑当前卡片 + 改字段保存）、`verify-addcards.ps1`
   （添加卡片 + 添加落库 + 关闭契约）、`verify-browser.ps1`（浏览器 + 表格 + 编辑区
   联动）、`verify-decks.ps1`（牌组列表 + 主窗口切页）。它们是「改完马上确认没坏」的入口。
2. **对齐清单在 `tauri/PARITY.md`**：那里按"用户感知顺序"列着还差什么（主窗口/牌组列表、
   既有页面接入、字段/卡片对话框、浏览器余项、系统集成、打包与 Android），每项带
   上游参考与验收方式；**补完一项就更新那份清单**。当前进度：复习界面的「更多」菜单
   与顶部工具栏、3.1-3.6「既有页面接入」（牌组选项 / 统计 / 设置 / 导入 / 图像遮挡 /
   自定义学习）、
   1.1 牌组列表、1.2 的两条工具栏都已补（1.3 状态栏核对下来 Qt v3 并没有，已按对齐处理）。
   下一步按清单走：**4.2「卡片…」对话框**（模板/样式/预览，上游约 980 行，最大的一块）、
   1.x 的收尾（牌组描述/拖拽排序、启动行为）、以及浏览器余项（CSV 导出、查找重复、
   管理笔记类型）。「卡片…」的入口与「字段…」同源（`openCardsDialog` 端点），
   目前 `platform/mod.rs` 里仍是"忽略"分支。
   `y`（同步）与 `Ctrl+E`（导出）要等 6.x 的同步与主窗口菜单栏。
   做之前先读
   `tauri/README.md` 的「编辑器」「浏览器」「复习界面」「牌组列表」：宿主契约都不复杂，
   但踩坑不少（§5.18 - §5.30）。
   注意有些"新窗口"不必新写界面 —— `card-info/<cid>`、`change-notetype/<id>`
   这类**仓库里已有的路由**直接复用即可（§3.7 第 7 条）；反过来，"原生对话框"这类
   宿主独有能力（保存路径、消息框）则要按 §3.7 第 6 条补端点。
3. 若只做小步：`current_caller`（editor / reviewer 的 TTS 打断语义）与
   拖入文件（HTML5 drop）属音频 / 编辑器细节；工程化侧还有把 §2.1 的构建步骤
   接进 `just`、补 `tauri/capabilities/`、换掉占位图标。`more` 命令、卡片布局与
   预览器则同样卡在"缺界面"上。
