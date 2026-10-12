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
在 Windows 上已经能跑通「打开集合 → 复习一张卡片 → 评分 → 下一张」的完整闭环。

- **M0 完成**：骨架、方法表、HTTP 服务、窗口
- **M1 完成**：集合打开、静态资源、媒体、CSP 分级、legacy 页面机制、最小复习会话、底部工具栏
- **M2 主体完成**：平台端点（剪贴板/文件选择/图片/对话框/配置/音频播放）、安全策略、`bridgeCommand` 通道
- **M3/M4 未开始**：编辑器、导入导出、Android

**当前可直接验证**：`ANKI_START_PAGE=reviewer` 启动后，会自动加载复习界面并显示一张真实卡片。

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

### 2.4 已验证的冒烟测试

```powershell
$env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
$env:ANKI_COLLECTION="D:\Workspace\ikas-anki\out\tauri-test\collection.anki2"
$env:ANKI_START_PAGE="reviewer"
$p = Start-Process -FilePath tauri\target\debug\anki-tauri.exe -PassThru `
     -RedirectStandardOutput out\run.log -RedirectStandardError out\run.err
Start-Sleep 18
Get-Content out\run.log -Encoding utf8 | Select-String "队列|渲染完成|底部栏|界面语言"
Stop-Process -Id $p.Id -Force
```

期望看到：

```text
界面语言: en-US
bridge 命令: domDone
domDone: 复习模式=true
队列: 共 N 张（新 x / 学习中 y / 复习 z）
底部栏（NNN 字节）: <center id="outer"> ...
渲染完成: 问题 NNN 字节 / 答案 NNN 字节 / 样式 NNN 字节
```

> `out/tauri-test/` 是隔离的测试数据目录，**不会污染真实 profile**。
> 里面已有 `collection.anki2` 与 `collection.media/`。

### 2.5 手工触发桥接命令（无需点击 UI）

```powershell
$pf = "out\pycmd.dat"
[System.IO.File]::WriteAllBytes($pf, [System.Text.Encoding]::UTF8.GetBytes("ans"))
curl.exe -s -X POST -H "Content-Type: text/plain" `
  -H "Authorization: Bearer test-key-123" --data-binary "@$pf" `
  "http://127.0.0.1:40001/_anki/pycmd"
```

`ans` → 显示答案；`ease1`..`ease4` → 评分并进入下一张。

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

---

## 4. 已完成的工作清单

### 4.1 模块地图

```text
tauri/src/
├─ main.rs            解析路径 → 打开集合 → 起服务取端口 → 创建窗口
├─ backend.rs         rslib 桥接：open_collection / call(method) / run(service, method)
├─ method_table.rs    方法名 → 索引（build.rs 生成）
├─ bridge.rs          注入脚本
├─ legacy.rs          legacy 页面注册与 HTML 组装
├─ reviewer.rs        复习会话
├─ paths.rs           资源路径（开发/打包双模式）
├─ state.rs           共享状态
├─ platform/          平台端点（FrontendService）
│  ├─ audio.rs        playFile（rodio + 常驻音频线程）
│  ├─ clipboard.rs    readClipboard / writeClipboard
│  ├─ config.rs       get/setProfileConfigJson、get/setMetaJson
│  ├─ dialogs.rs      openFilePicker / askUser / showMessageBox / openLink
│  └─ media.rs        convertPastedImage / openMedia / showInMediaFolder
└─ server/
   ├─ mod.rs          路由、Host/Origin 中间件、服务启动
   ├─ api.rs          POST /_anki/{method}
   ├─ assets.rs       静态资源 + 媒体 + CSP 分级
   ├─ legacy.rs       GET /_anki/legacyPageData
   └─ pycmd.rs        POST /_anki/pycmd
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

---

## 6. 剩余工作

按建议优先级排序。

### 6.1 复习界面收尾（价值最高，改动集中）

| 项 | 说明 |
| --- | --- |
| **计时与时间盒** | `showQuestion(middle, maxTime)` 的 `maxTime` 现在固定传 0；应对照 `card.time_limit()` |
| **按钮下次间隔** | Qt 用 `V3Scheduler.describe_next_states()` 在按钮上标注「10分钟/1天」。需要接调度状态格式化 |
| **按钮数量** | 现在固定 4 个；Qt 按牌组配置显示 2/3/4 个（`sched.answerButtons()`） |
| **`edit` / `more`** | 目前 no-op。需要打开浏览器/更多菜单 |
| **撤销** | rslib 已有 `undo` API（`Ctrl+Z`） |
| **音频自动播放队列** | 复习时自动播放卡片音频；`playFile` 已实现，缺队列调度 |
| **类型作答** | `type:` 过滤器链路 |
| **计数显示** | 底部栏的「新/学习中/复习」剩余计数（`Reviewer._remaining()`） |
| **界面状态同步** | `getSchedulingStatesWithContext` / `setSchedulingStates` |

### 6.2 桥接命令扩充

目前只覆盖 `domDone` / `ans` / `ease1`..`ease4` / `close`。
待实现清单（都会打 debug 日志）：`edit`、`more`、`updateToolbar`、
`repaintNeeded`、`browserSearch`、`toggleSticky`、`saveTags`、`saved`、
`paste`、`cutOrCopy`、`preview` 等。

> 注意 `js/reviewer/index.ts` 里的 `isLegacy` 分支：新路径走 RPC，
> 旧路径走 `bridgeCommand`。优先确认某个命令是否真的还需要。

### 6.3 M2 余项

- **录音**（`recordAudio`）：Windows 需 `cpal` 采集 + wav 编码；Android 需 `MediaRecorder`。**风险最高**。
- **窗口模型**：那 10 个降级端点（`closeAddCards`、`searchInBrowser`、
  `openFieldsDialog` 等）要真正实现，**需要先决定窗口模型**：
  多 WebView（贴近 Qt）还是单窗口 + 前端路由接管。这是架构决策。
- `getConfigJson` / `setConfigJson`（集合级配置）。
- **剪贴板限制**：`arboard` 无法按任意 MIME 读取，`text/uri-list`（拖入文件）未支持。

### 6.4 M3 / M4

- **M3**：编辑器（`ts/routes/editor/`，89 个文件，依赖剪贴板/图片/录音）、
  导入导出（**Android 必须走 SAF**）、图像遮挡。
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

### 7.2 显式降级（不是遗漏，已在文档标注）

- 10 个窗口管理类端点接受请求但不产生副作用。
- 配置存储用本地 JSON 文件（`tauri-profile.json`），**与 Anki 的 `prefs21.db` 不互通**。
- PNG「质量」语义与 Qt 不等价（Qt 传的是压缩级别）。
- 音频：无变速、无队列、未区分 editor/reviewer 模式。

### 7.3 尚未验证

- **没有做过视觉验证**：所有验证都在 HTTP 层 + 日志，**没有看到窗口的实际渲染效果**。
  不过 `reviewer.js` 会主动调用 `updateToolbar`，可证明它已加载执行。
- 未跑 Anki 的 Playwright e2e。
- **Android 一行未验证**。

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

---

## 9. 建议的第一步

如果要继续推进，建议按这个顺序：

1. **先跑一遍 §2.4 的冒烟测试**，确认环境完好、闭环仍通。
2. 挑 §6.1 里的**计时与时间盒**或**按钮下次间隔**——改动集中在
   `tauri/src/reviewer.rs`，不涉及架构决策，且能立刻验证。
3. 若打算碰窗口相关端点，**先与负责人确认窗口模型**（§6.3），不要边写边猜。
