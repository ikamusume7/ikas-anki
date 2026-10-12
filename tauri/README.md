# Anki Tauri 外壳

`TAURI_MIGRATION_PLAN.md` 的实现。一个通过本地 HTTP 服务驱动既有 Svelte 前端与
rslib 后端的 Tauri v2 外壳。

> **接手本工程请先读 `../TAURI_HANDOVER.md`** —— 它包含状态速览、可复现的
> 构建/验证步骤、关键决策、以及一份「踩过的坑」清单。

**当前进度**：M0 完成；**M1 完成**（含 legacy 页面机制与最小复习会话）；
M2 平台端点完成；另已补齐**安全策略**（API 密钥 / 来源校验 / CSP 分级）与
**`bridgeCommand` 通道**。剩余项见「尚未实现」。

## 设计要点

- **无 FFI**：Tauri 的 Rust 侧与 rslib 同进程，直接 `use anki::...` 调用
  `Backend::run_service_method`，不需要任何绑定层。
- **前端零改动**：保留了 `qt/aqt/mediasrv.py` 的 HTTP 形态，前端（`ts/`）的
  `/_anki/...` 绝对路径与 `POST /_anki/{method}` 调用方式完全不变。
- **索引一致性**：方法名 → `(service, method)` 的映射由 `build.rs` 复用
  `anki_proto_gen::get_services()` 生成，与 rslib 内部分发代码使用同一来源。

## 目录结构

```text
tauri/
├─ build.rs              # 生成 method_table.rs
├─ tauri.conf.json
├─ src/
│  ├─ main.rs            # 解析路径 → 打开集合 → 起服务取端口 → 创建窗口
│  ├─ backend.rs         # rslib 桥接（open_collection / call / run）
│  ├─ method_table.rs    # 方法名 → (service, method) 索引，供 HTTP 与内部共用
│  ├─ bridge.rs          # 注入到 WebView 的脚本（鉴权头 + bridgeCommand）
│  ├─ legacy.rs          # legacy 页面注册与 HTML 组装（对照 stdHtml）
│  ├─ reviewer.rs        # 复习会话：取队列 → 渲染 → 注入 → 评分 → 下一张
│  ├─ paths.rs           # 资源目录（开发/打包双模式）与 profile 路径
│  ├─ state.rs           # 共享状态（媒体目录、profile 目录、API 密钥、复习会话）
│  ├─ platform/          # M2：FrontendService 端点的外壳实现
│  │  ├─ mod.rs          # 端点分发
│  │  ├─ audio.rs        # 音频播放
│  │  ├─ clipboard.rs    # 剪贴板
│  │  ├─ config.rs       # profile / meta 配置存储
│  │  ├─ dialogs.rs      # 原生对话框与打开链接
│  │  └─ media.rs        # 图片转换与媒体目录
│  └─ server/
│     ├─ mod.rs          # axum 路由、Host/Origin 中间件、服务启动
│     ├─ api.rs          # POST /_anki/{method} → rslib 或 platform
│     ├─ assets.rs       # 静态资源 + 媒体文件 + CSP 分级
│     ├─ legacy.rs       # GET /_anki/legacyPageData
│     └─ pycmd.rs        # POST /_anki/pycmd（桥接命令）
└─ web/                  # frontendDist 占位目录
```

## 环境变量

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `ANKI_API_PORT` | `0`（随机） | 本地服务端口，便于调试与 e2e |
| `ANKI_COLLECTION` | `%APPDATA%/Anki2/User 1/collection.anki2` | 要打开（或创建）的集合 |
| `ANKI_START_PAGE` | `graphs` | 窗口加载的 SvelteKit 页面 |
| `ANKI_API_KEY` | 随机生成 | 固定本机 API 密钥，仅用于调试与 e2e |
| `ANKI_LANG` | `en-US` | 界面语言；接受 `zh_CN` 或 `zh-CN`（统一转成 FTL 的连字符形式） |

## 构建步骤

### 1. protoc（版本 31.1）

```powershell
New-Item -ItemType Directory -Force -Path out/extracted/protoc | Out-Null
curl.exe -L -o out/protoc-31.1.zip `
  "https://github.com/protocolbuffers/protobuf/releases/download/v31.1/protoc-31.1-win64.zip"
Expand-Archive out/protoc-31.1.zip -DestinationPath out/extracted/protoc -Force
```

### 2. protobuf 描述符（rslib 与 tauri 的 build.rs 都依赖）

```powershell
$env:CARGO_TARGET_DIR="out/rust"
cargo build -p anki_proto
```

产出 `out/rslib/proto/descriptors.bin`、`out/ts/lib/generated/backend.ts`、
`out/pylib/anki/_backend_generated.py`。

### 3. 前端产物

```powershell
corepack yarn install

# 生成 protobuf 的 TS 模块（等价于构建系统的 ts:generated:proto 目标）
$root = (Get-Location).Path
$protos = (Get-ChildItem proto/anki/*.proto).Name | ForEach-Object { "anki/$_" }
& "out/extracted/protoc/bin/protoc.exe" -I proto $protos `
  "--plugin=protoc-gen-es=$root/node_modules/.bin/protoc-gen-es.cmd" `
  "--es_out=$root/out/ts/lib/generated"
& "$root/node_modules/.bin/tsx.cmd" ts/tools/markpure.ts out/ts/lib/generated

# 复制手写 helper（构建系统的 ts:generated:src 目标）
Copy-Item ts/lib/generated/*.ts out/ts/lib/generated/ -Force

corepack yarn build      # 产出 out/sveltekit
```

### 3b. legacy 界面所需产物

复习屏、卡片布局等界面需要 esbuild / sass 的产物，输出到构建产物目录
`qt/_aqt/data/web/`：

```powershell
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

> 这些步骤在正式构建系统里由 `ts:generated:proto` / `ts:generated:src` /
> SvelteKit 构建目标完成；此处手工执行是因为尚未接入 `just` 流程。

### 4. 构建并运行

```powershell
cd tauri
cargo build
.\target\debug\anki-tauri.exe
```

## 已验证行为

### M0

| 检查项 | 结果 |
| --- | --- |
| `cargo check` / `cargo build` | 通过 |
| 程序启动（rslib 初始化 + 端口绑定 + 窗口创建） | 通过 |
| `POST /_anki/<未注册方法>` | `404`（方法表未命中） |

### M1

| 检查项 | 结果 | 说明 |
| --- | --- | --- |
| 打开集合 | 通过 | 集合不存在时自动创建 |
| `GET /_anki/readyz` | `200` | 反映真实集合状态，未打开时返回 `503` |
| `POST /_anki/getDeckNames` | `200` | 返回真实数据（新建集合含默认牌组 `Default`） |
| `GET /graphs`、`GET /editor` | `200` | SvelteKit 页面路由正确返回 `index.html` |
| `GET /_app/immutable/...` | `200` | 前端 chunk 可加载 |
| SvelteKit 的 CSP meta | 已移除 | 否则严格策略会拦截本地加载 |
| 响应头 `Content-Security-Policy` | `frame-ancestors 'none'` | 对照 mediasrv 的 `TRUSTED_PAGE_CSP` |
| 媒体文件 `GET /<name>` | 返回内容 | 走集合的 `collection.media` |
| 目录穿越 `GET /../Cargo.toml` | `403` | `safe_join` 生效 |
| 不存在的媒体 | `404` | 预期 |

### M2（第一批）

平台端点位于 `src/platform/`。这些端点对应 `FrontendService`，在 rslib 中
被显式过滤，桌面版由 Qt 实现，这里用跨平台 Rust 库补齐。

| 端点 | 实现 | 验证结果 |
| --- | --- | --- |
| `getProfileConfigJson` / `setProfileConfigJson` | 本地 JSON 文件 | 往返成功（`null` → `42`） |
| `getMetaJson` / `setMetaJson` | 同上 | 同一实现 |
| `writeClipboard` / `readClipboard` | `arboard` | 往返成功（写入 `clip-ok` 并读回） |
| `convertPastedImage` | `image` | png / jpeg 重编码 |
| `openFilePicker` | `rfd` 原生对话框 | 按扩展名过滤 |
| `askUser` / `showMessageBox` | `rfd` 原生对话框 | 是/否与提示框 |
| `openLink` | `opener` | 交给系统默认程序 |
| `openMedia` / `showInMediaFolder` | `opener::reveal` | 在文件管理器中定位 |
| `playFile` | `rodio` + 常驻音频线程 | 播放成功；不存在的文件返回 500 |

单窗口模型下暂时**显式降级**的端点（接受请求但不产生副作用，会记录 debug 日志）：
`closeAddCards`、`closeEditCurrent`、`importDone`、`importDialogRequireClose`、
`deckOptionsRequireClose`、`deckOptionsReady`、`openFieldsDialog`、
`openCardsDialog`、`searchInBrowser`、`saveCustomColours`。

> 关键实现细节：`anki_proto_gen::get_services()` 会把 `FrontendService` 的方法
> 复制进 `BackendFrontendService` 的 `delegating_methods`。因此 `build.rs`
> 必须把 `BackendFrontendService` 过滤掉（与 `rust_interface.rs` 一致），
> 否则这些请求会被转给 rslib 并得到 `InvalidServiceIndex`。

## 安全策略

对照 mediasrv 逐项实现：

| 机制 | 实现 | 验证 |
| --- | --- | --- |
| Host 校验 | 中间件拒绝非本机 Host | `Host: evil.com:1234` → 403 |
| Origin 校验 | 拒绝非本机来源 | `Origin: http://evil.com` → 403 |
| 内容类型校验 | `POST /_anki/*` 必须是 `application/binary` | `text/plain` → 403 |
| API 密钥 | 内部请求需带 `Authorization: Bearer <key>` | 无密钥 → 403，正确 → 200 |
| 复习页白名单 | 仅放行 4 个端点（`getSchedulingStatesWithContext` 等） | — |
| CSP · 可信页面 | `frame-ancestors 'none'`，并移除 SvelteKit 的 meta | `/graphs` 已验证 |
| CSP · 不可信页面 | 带来源白名单 + SvelteKit script hash | `/editor` 已验证，hash 正确提取 |
| CSP · 媒体文件 | `default-src 'none'` + `sandbox`，禁止脚本执行 | 已验证 |

## 桥接通道

`src/bridge.rs` 在页面脚本之前注入（`initialization_script`），做两件事：

1. **包装 `window.fetch`**，为 `/_anki/` 请求补上 `Authorization` 头
   —— 对照 Qt 的 `AuthInterceptor`（Tauri 没有等价的请求拦截 API）。
2. **提供 `window.bridgeCommand` / `window.pycmd`**，把命令 POST 到
   `/_anki/pycmd` —— 对照 Qt 通过 QWebChannel 注入的同名函数，语义保持
   「命令字符串 + 可选回调」。

> **当前覆盖**：`domDone`、`ans`、`ease1`..`ease4`、`close`。
> 其余命令（`edit`、`more`、`updateToolbar`、`repaintNeeded`、`browserSearch`、
> 编辑器快捷键等）会记录日志并返回 `null`。也就是说前端不再因缺少函数而
> 抛异常，但对应功能尚未生效。完整清单见「尚未实现」。

## 复习界面

复习屏走的是 **legacy 页面机制**（对照 Qt 的 `stdHtml` + `set_page_html`）：

1. 宿主组装完整 HTML（`<base>` + css/js 链接 + `#qa` 容器）并注册到服务端；
2. WebView 访问 `/_anki/legacyPageData?id=N` 取回该页面；
3. 卡片 HTML 由 `js/reviewer.js` 提供的 `_showQuestion` / `_showAnswer` 注入；
4. 用户的评分操作经 `pycmd` 回到宿主。

以 `ANKI_START_PAGE=reviewer` 启动。会话逻辑在 `src/reviewer.rs`：
**取队列 → 渲染 → 注入 → 评分 → 下一张**。

### 验证结果

| 步骤 | 证据 |
| --- | --- |
| 页面与资源加载 | `legacyPageData`、`js/reviewer.js`、`css/reviewer.css` 等全部 200 |
| 注入脚本 | 页面加载后自动发出 `domDone` |
| `bridgeCommand` 垫片 | `reviewer.js` 成功调用 `updateToolbar`、`repaintNeeded` |
| 取队列 | `队列: 共 1 张（新 1 / 学习中 0 / 复习 0）` |
| 卡片渲染 | `渲染完成: 问题 184 字节 / 答案 211 字节 / 样式 148 字节` |
| 显示答案 | `pycmd("ans")` 被处理 |
| 评分 | `pycmd("ease3")` 后队列变为 `新 0 / 学习中 1` —— 调度状态确实更新 |
| 底部栏 | 注入成功，按钮文本随语言变化（`Edit`/`More` ↔ `编辑`/`更多`） |

### 底部工具栏

Qt 把底部栏放在**独立的 WebView**（`mw.bottomWeb`）里；这里放在同一页面的
`#bottom-container`，由 `crate::reviewer` 注入。按钮调用全局 `pycmd`，
因此不需要跨 WebView 的消息传递。

对照关系：

| Qt | 这里 |
| --- | --- |
| `Reviewer._bottomHTML()` | `reviewer::bottom_bar_html()` |
| `Reviewer._showAnswerButton()` | `reviewer::show_answer_button()` |
| `Reviewer._answerButtons()` | `reviewer::ease_buttons()` |
| `js/reviewer-bottom.js` 的 `showQuestion` / `showAnswer` | 直接 eval 调用同一份脚本 |

按钮文本通过 `anki_i18n` 获取（对照 Qt 的 `tr.xxx()`），已用中英文验证。

## 尚未实现

- **桥接命令处理**：`/_anki/pycmd` 目前只覆盖复习主链路（`domDone`、`ans`、
  `ease1`..`ease4`、`close`）。工具栏动作、编辑器快捷键、`browserSearch`、
  `updateToolbar`、`repaintNeeded` 等命令会记录日志并返回 `null`。
- **复习界面余项**：
  - 底部栏已实现「编辑 / 更多 + 显示答案 + 四档评分」，但**计时与时间盒未接入**
    （`showQuestion` 的 maxTime 固定传 0）、按钮上不显示下次间隔、
    评分按钮固定四个（未按牌组配置显示 2/3 个）。
  - `edit` / `more` 命令目前是 no-op。
  - 撤销、自动播放与音频队列、类型作答（`type:` 过滤器）、学习/复习计数显示、
    `getSchedulingStatesWithContext` 等界面状态同步、卡片布局与预览器。
- **M2 余项**：
  - **录音**（`recordAudio`）：播放已完成，录音尚缺。Windows 需要 `cpal`
    采集并编码为 wav，Android 需要 `MediaRecorder`，是 M2 剩余风险最高的一项。
  - **窗口模型**：上表列出的那批降级端点要真正实现，需先确定 Tauri 外壳的
    窗口模型 —— 多 WebView（贴近 Qt 原有语义）还是单窗口 + 前端路由接管。
    这是架构决策，不只是编码工作。
  - **复习状态类**：`getSchedulingStatesWithContext` / `setSchedulingStates`
    依赖 reviewer 的调度状态，需要随复习界面一起重新设计。
  - `getConfigJson` / `setConfigJson`（集合级配置，与 profile/meta 不同）。
- **剪贴板限制**：`arboard` 只暴露文本与图片，无法像 Qt 那样按任意 MIME
  读取，因此 `text/uri-list`（拖入的文件路径）暂未支持。
- **M3**：导入导出、图像遮挡、Android 的 SAF 适配。
- **M4**：Windows MSI 打包与签名；Android 的 `tauri android init/build`。

## 已知限制

- **静态资源是手工构建的**：`out/sveltekit` 与 `qt/_aqt/data/web/` 的产物目前
  靠 README 里的命令手动生成，尚未接入 `just` 流程。
- **图标**：暂用 `qt/aqt/data/web/imgs/favicon.ico` 的副本，非多尺寸图标集。
- **打包配置**：`bundle.active = false`，尚未验证真实安装包。
- **`capabilities/` 目录不存在**：`tauri-build` 会引用它，当前不影响编译，
  但打包前需要补上。
- **Android 未验证**：路径解析里的 `user_data` 需要改为 `getFilesDir()`
  （`paths.rs` 里已留 TODO）。
- **本工程是独立 workspace**：不参与 Anki 原有的构建与 CI，依赖树独立。
