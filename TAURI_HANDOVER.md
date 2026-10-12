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
并已接入「编辑当前卡片」的编辑器窗口。

- **M0 完成**：骨架、方法表、HTTP 服务、窗口
- **M1 完成**：集合打开、静态资源、媒体、CSP 分级、legacy 页面机制、最小复习会话、底部工具栏
- **M2 完成**：mediasrv 的 32 个平台端点全部有实现 —— 剪贴板（含 `text/uri-list`）、
  文件选择、图片、原生对话框、profile / meta 配置（直接读写桌面版 `prefs21.db`）、
  音频播放与**录音**、**TTS**、**窗口管理（多窗口模型）**、复习状态同步
- **M3 进行中**：编辑器外壳的第一步「编辑当前卡片」已可用（`src/editor.rs`）；
  导入导出、图像遮挡、Android 未开始
- **M4 未开始**：打包与 Android

**当前可直接验证**（三个一键脚本，详见 §2.4）：

| 脚本 | 覆盖 |
| --- | --- |
| `tauri/tools/verify-reviewer.ps1` | 复习界面：启动 → 补卡片 → 截真实窗口 → 布局断言 |
| `tauri/tools/verify-m2.ps1` | 平台能力：剪贴板文件列表 / TTS 合成 / 录音闭环 + 多窗口命令归属 |
| `tauri/tools/verify-editor.ps1` | 编辑器：「编辑当前卡片」开窗 → 载入 note → 截图 → 关窗 → 改字段保存 |

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
```

三条都会自己启动外壳、用隔离集合、跑断言、结束进程。复习界面那条期望看到：

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
$p = Start-Process -FilePath out/rust/debug/anki-tauri.exe -PassThru `
     -RedirectStandardOutput out/run.log -RedirectStandardError out/run.err
Start-Sleep 18
Get-Content out\run.log -Encoding utf8 | Select-String "队列|渲染完成|底部栏|界面语言"
Stop-Process -Id $p.Id -Force
```

> 可执行文件在 `out/rust/debug/anki-tauri.exe` —— 构建流程设了
> `CARGO_TARGET_DIR=out/rust`，不是 `tauri/target/`。
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

---

## 4. 已完成的工作清单

### 4.1 模块地图

```text
tauri/src/
├─ main.rs            解析路径 → 打开集合 → 起服务取端口 → 创建窗口
├─ backend.rs         rslib 桥接：open_collection / call(method) / run(service, method)
├─ method_table.rs    方法名 → 索引（build.rs 生成）
├─ bridge.rs          注入脚本（鉴权头 + 窗口身份 + bridgeCommand）
├─ legacy.rs          legacy 页面注册与 HTML 组装
├─ editor.rs          编辑器窗口（开窗 + `editorReady` 后注入 `loadNote`）
├─ reviewer.rs        复习会话
├─ paths.rs           资源路径（开发/打包双模式）
├─ state.rs           共享状态
├─ windows.rs         窗口模型（多窗口 / 单实例 / 模态 / 几何记忆）
├─ platform/          平台端点（FrontendService）
│  ├─ audio.rs        playFile（rodio 队列播放）
│  ├─ clipboard.rs    readClipboard / writeClipboard
│  ├─ config.rs       get/setProfileConfigJson、get/setMetaJson
│  ├─ prefs21.rs      prefs21.db 读写（SQLite + pickle，与桌面版共用）
│  ├─ record.rs       recordAudio（cpal 采集 + WAV，会话挂在 state.recorder）
│  ├─ scheduling.rs   getSchedulingStatesWithContext / setSchedulingStates
│  ├─ dialogs.rs      openFilePicker / askUser / showMessageBox / openLink
│  └─ media.rs        convertPastedImage / openMedia / showInMediaFolder
└─ server/
   ├─ mod.rs          路由、Host/Origin 中间件、服务启动
   ├─ api.rs          POST /_anki/{method}
   ├─ assets.rs       静态资源 + 媒体 + CSP 分级
   ├─ legacy.rs       GET /_anki/legacyPageData
   └─ pycmd.rs        POST /_anki/pycmd

tauri/tools/           验证脚本：verify-reviewer.ps1（复习界面）、verify-m2.ps1
                       （平台能力）、verify-editor.ps1（编辑器窗口）、
                       capture-window.ps1（截真实窗口，多窗口用 -TitleLike 选择）、
                       visual-check.mjs / m2-check.mjs / editor-check.mjs、
                       seed-card.mjs、set-tts-template.mjs、check-prefs21.mjs
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
| ~~**`edit`**~~ | **已完成**：打开「编辑当前卡片」窗口。编辑器外层窗口全在 Web 里，宿主只需开窗 + `editorReady` 后注入 `loadNote`，见 `tauri/README.md` 的「编辑器」 |
| **`more`** | 仍 no-op：要打开更多菜单，需要新的界面 |
| ~~**撤销**~~ | **已完成**：`undo` 桥接命令 + 页面内快捷键（`Ctrl+Z` / `Cmd+Z` / 裸 `u`），撤销后重新取队列 |
| ~~**音频自动播放队列**~~ | **已完成**：`audio.rs` 改为队列播放（对照 `AVPlayer.play_tags`），显示问题/答案时按牌组配置自动播放 |
| ~~**类型作答**~~ | **已完成**：问题面插入 `#typeans` 输入框、答案面用 `CompareAnswer` 生成对比；输入值由桥接垫片附在 `ans:` 后面回传 |
| ~~**计数显示**~~ | **已完成**：对照 `Reviewer._remaining()`，「显示答案」按钮上显示「新 + 学习中 + 复习」，当前队列加下划线 |
| ~~**界面状态同步**~~ | **已完成**：`getSchedulingStatesWithContext` / `setSchedulingStates` + `cardStateCustomizer` 注入，见 `platform/scheduling.rs` |

### 6.2 桥接命令扩充

目前只覆盖 `domDone` / `ans` / `ease1`..`ease4` / `close`。
待实现清单（都会打 debug 日志）：`edit`、`more`、`updateToolbar`、
`repaintNeeded`、`browserSearch`、`toggleSticky`、`saveTags`、`saved`、
`paste`、`cutOrCopy`、`preview` 等。

> 注意 `js/reviewer/index.ts` 里的 `isLegacy` 分支：新路径走 RPC，
> 旧路径走 `bridgeCommand`。优先确认某个命令是否真的还需要。

### 6.3 M2 收尾情况（已完成）

**M2 已完成**：mediasrv 的 32 个平台端点全部有实现。逐项收尾情况：

- ~~**录音**~~ —— **已实现**：`cpal` 采集 + `hound` 写 WAV，录音窗口是宿主自建的
  legacy 页面（红点 + 计时 + 保存/取消），关闭窗口等同取消。与 Qt 的差异见
  `tauri/README.md` 的「录音」（Qt 会调外部 `lame` 转 MP3；取消时 Qt 不会回复
  请求方，我们会返回空路径）。**Android 未验证**：Qt/AnkiDroid 用 `MediaRecorder`，
  这里统一用 `cpal`（Android 走 oboe），需真机确认。
- ~~**窗口模型**~~ —— **已定案并实现**（见 §3.6）：多窗口 + 单实例 + 模态 + 几何记忆，
  `deckOptionsReady` / `deckOptionsRequireClose` / `importDone` /
  `importDialogRequireClose` 已可用。剩下的 `searchInBrowser` / `openFieldsDialog` /
  `openCardsDialog` / `closeAddCards` / `closeEditCurrent` 卡在**缺少 Web 界面**
  （新版 Anki 里它们是原生 Qt 界面，需要新写 Svelte 页面，属 M3），
  `saveCustomColours` 卡在原生取色器能力。
- ~~**剪贴板按任意 MIME 读取**~~ —— **已实现**：`text/uri-list` 在 Windows 上读
  剪贴板的 `CF_HDROP`（编辑器「复制文件 → 粘贴成媒体」依赖它），图片与文本仍走
  `arboard`。仍未覆盖的是**拖入文件**（HTML5 drop），属 M3 的编辑器链路。
- ~~`getConfigJson` / `setConfigJson`~~ —— 这两个由 rslib 的 `ConfigService` 直接
  服务，实测可用（`getAllConfig` → 200），从来不是缺口。

### 6.4 M3 / M4

- **M3**：编辑器（`ts/routes/editor/`，89 个文件，依赖剪贴板/图片/录音）、
  导入导出（**Android 必须走 SAF**）、图像遮挡。
  - 已完成第一步：**「编辑当前卡片」窗口**（`tauri/src/editor.rs`、`tools/verify-editor.ps1`）。
    编辑器外层窗口全在 Web 里，宿主只需开窗 + `editorReady` 后注入 `loadNote`；
    改动前请先读 §5.18（同名端点接管）与 `tauri/README.md` 的「编辑器」。
  - 下一步：AddCards（`mode=add`，要注入默认 deck/notetype，并在关窗前弹「丢弃修改」确认）、
    浏览器（`mode=browser`）、「字段…」「卡片…」对话框（原生 Qt 界面，需新写 Svelte 页面）。
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

### 7.2 曾经的妥协与解决情况（不是遗漏，已在文档标注）

- ~~10 个窗口管理类端点接受请求但不产生副作用~~ —— **已解决一半**：多窗口模型下
  6 个已转发到对应窗口（含录音的保存/取消），另 4 个（`searchInBrowser` /
  `openFieldsDialog` / `openCardsDialog` / `saveCustomColours`）不是窗口模型能解决的，
  缺的是 Web 界面或原生取色器能力，见 §6.3。
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

### 7.4 对上游的改动

只有一处：`rslib/proto/src/lib.rs` 加了一行 `protobuf!(frontend, "frontend");`，
用于暴露 `frontend.proto` 的消息类型（其 service 仍被 rust_interface 过滤）。
纯增量，无行为影响。

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
| 编辑器窗口 | `tauri/src/editor.rs`、`ts/routes/editor/`、`qt/aqt/editor.py`（宿主契约） |
| 验证脚本与用法 | `tauri/tools/`（`verify-reviewer.ps1` 复习界面 / `verify-m2.ps1` 平台能力 / `verify-editor.ps1` 编辑器）、`tauri/README.md` 的「验证工具」 |

---

## 9. 下一步建议

M1 / M2 已收尾（32 个平台端点全部有实现），M3 的**编辑器外壳**开了个头
（「编辑当前卡片」窗口可用）。建议：

1. **先跑三个一键脚本确认现状**：`tauri/tools/verify-reviewer.ps1`（复习界面 + 布局
   断言）、`verify-m2.ps1`（平台能力 + 多窗口命令归属）、`verify-editor.ps1`
   （编辑器窗口 + 改字段保存）。它们是「改完马上确认没坏」的入口。
2. **继续 M3**：编辑器还缺 AddCards（`mode=add`）、浏览器（`mode=browser`）与
   「字段…/卡片…」对话框。做之前先读 `tauri/README.md` 的「编辑器」——
   宿主契约只有两步（开窗 + 注入 `loadNote`），但踩坑不少（§5.18 / §5.19）。
3. 若只做小步：`tauri/README.md` 的「尚未实现」里，`more` 命令、卡片布局与预览器
   都卡在"缺界面"上；`current_caller` 与拖入文件属音频 / 编辑器细节。
