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
│  │  ├─ audio.rs        # 音频队列播放（对照 AVPlayer）
│  │  ├─ clipboard.rs    # 剪贴板
│  │  ├─ config.rs       # profile / meta 配置端点
│  │  ├─ prefs21.rs      # prefs21.db 读写（SQLite + pickle）
│  │  ├─ dialogs.rs      # 原生对话框与打开链接
│  │  └─ media.rs        # 图片转换与媒体目录
│  └─ server/
│     ├─ mod.rs          # axum 路由、Host/Origin 中间件、服务启动
│     ├─ api.rs          # POST /_anki/{method} → rslib 或 platform
│     ├─ assets.rs       # 静态资源 + 媒体文件 + CSP 分级
│     ├─ legacy.rs       # GET /_anki/legacyPageData
│     └─ pycmd.rs        # POST /_anki/pycmd（桥接命令）
├─ tools/                # 验证脚本（截图 / 布局断言 / 造卡片 / 配置连通性）
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

# i18n 的 TS 接口（构建系统的 ts:generated:i18n 目标）。
# 它由 anki_i18n 的 build script 在**编译期**通过 STRINGS_TS 写出：
# 缺少这一步时 `yarn build` 会报
#   Could not load .../out/ts/lib/generated/ftl
# 因为 `@generated` 别名指向 out/ts/lib/generated，而 ftl.ts 必须先被生成。
$env:STRINGS_TS="$PWD/ts/lib/generated/ftl.ts"
cargo build -p anki_i18n
Copy-Item ts/lib/generated/ftl.ts out/ts/lib/generated/ -Force

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
| `getProfileConfigJson` / `setProfileConfigJson` | `prefs21.db`（与桌面版同一份） | 见「配置存储」 |
| `getMetaJson` / `setMetaJson` | 同上 | 同一实现 |
| `writeClipboard` / `readClipboard` | `arboard` + Windows 原生 `CF_HDROP` | 见「剪贴板」 |
| `convertPastedImage` | `image` | png / jpeg 重编码；PNG 用 zlib level 4 对齐 Qt（见下） |
| `openFilePicker` | `rfd` 原生对话框 | 按扩展名过滤 |
| `askUser` / `showMessageBox` | `rfd` 原生对话框 | 是/否与提示框 |
| `openLink` | `opener` | 交给系统默认程序 |
| `openMedia` / `showInMediaFolder` | `opener::reveal` | 在文件管理器中定位 |
| `playFile` | `rodio` + 常驻音频线程（队列） | 见「音频播放」 |
| `recordAudio` | `cpal` 采集 + `hound` 写 WAV | 见「录音」 |

> `convertPastedImage` 的 PNG 压缩级别：Qt 侧 `image.save(buffer, "png", 50)` 里那个
> `quality` 其实**不生效** —— `QImageWriter` 只设置了 `Quality`，而 PNG handler 的
> `compression` 默认值是 50（≥0）并优先，最终按 `(50 * 9) / 91 = 4` 得到
> **zlib level 4**。这里显式用 `CompressionType::Level(4)` 对齐（注意 `image` 的
> 默认档位是 `Fast`，不能依赖默认值）。JPEG 的 `quality = 80` 语义是直接对应的。

### 窗口模型

**决策：一个窗口一个 `WebviewWindow`（多窗口），对齐 Qt 的 `DialogManager`；
不采用「单窗口 + 前端路由接管」。** 实现见 `src/windows.rs`。

依据：

1. Qt 里这些窗口本来就是**独立顶层窗口**（浏览器/添加卡片/编辑当前是
   `QMainWindow(None, Window)`，牌组选项/导入是 `QDialog(mw, Window)`），而且
   牌组选项与导入是 `ApplicationModal`。标题栏、任务栏项、几何记忆、模态都是
   用户可感知的语义，单窗口 + 路由等于自己重写一遍窗口管理器。
2. 牌组选项 / 导入 / 首选项在新版 Anki 里已经是**独立的 SvelteKit 路由**
   （`ts/routes/`），`WebviewWindowBuilder` 直接加载该 URL 即可，前端零改动；
   主窗口当前本来就是「一个 URL」，扩展成多窗口是增量改动。

已实现的窗口语义：

| 语义 | 实现 | 对照 Qt |
| --- | --- | --- |
| 单实例 | 以窗口标签唯一标识，已存在时只聚焦 | `DialogManager` 的「同一窗口只保留一个实例」 |
| 模态 | 禁用主窗口，窗口销毁时恢复 | `ApplicationModal`（Tauri 没有 `setModal`） |
| 几何记忆 | 存进 profile 配置 `tauriWindowGeometry:<label>` | `saveGeom` / `restoreGeom` |
| 脚本注入 | 每个窗口都注入鉴权与 `bridgeCommand` | 每个 WebView 都过 QWebChannel |

> 几何用**自己的键名**、值为 JSON：Qt 存的是 `QWidget.saveGeometry()` 的二进制块，
> 两边格式不可能互通，复用键名只会互相破坏。

**命令必须归属窗口。** 注入脚本除了补鉴权头，还会给每个请求带上
`X-Anki-Window: <窗口标签>`（`src/bridge.rs`）。Qt 里每个 WebView 有自己的
`AnkiWebPage`，命令天然归属该窗口；而这里所有窗口共用一条 `/_anki/pycmd` 通道，
不显式区分就会出事 —— 注入脚本在每个窗口加载时都会发 `domDone`，缺了窗口身份，
「打开牌组选项」就会被当成复习界面启动，复习会话被无端重启。
`src/server/pycmd.rs` 因此把命令分流：复习相关命令只接受主窗口，其他窗口只处理
关窗与录音命令（`saveRecording` / `cancelRecording`）。

原先「显式降级」的 10 个端点现在分成三类：

| 端点 | 现状 |
| --- | --- |
| `deckOptionsReady` / `deckOptionsRequireClose` / `importDone` / `importDialogRequireClose` | **已实现**：就绪 / 关闭 / 模态变化都转发给对应窗口 |
| `closeAddCards` / `closeEditCurrent` | **关闭语义已实现**，但窗口本身还开不出来（见下） |
| `searchInBrowser` / `openFieldsDialog` / `openCardsDialog` | **仍未实现，而且不是窗口模型的问题**：浏览器 / 字段 / 卡片布局在新版 Anki 里是**原生 Qt 界面**，仓库里没有对应的 Web 页面（`ts/routes/` 只有 card-info / change-notetype / congrats / deck-options / editor / graphs / image-occlusion / import-* / preferences），需要新的 Svelte 界面（M3） |
| `saveCustomColours` | **仍未实现**：需要能暴露自定义调色板的原生取色器，`rfd` 没有该 API |

打开窗口的**产品内入口**也还没实现（打开牌组选项要经过牌组浏览器，而牌组浏览器是
原生 Qt 界面）。因此提供 `ANKI_OPEN_ROUTES=deck-options/1,preferences`——与
`ANKI_START_PAGE` 同类的开发/测试入口——把窗口直接开出来做端到端验证。

已验证（`ANKI_START_PAGE=reviewer` + `ANKI_OPEN_ROUTES=deck-options/1`）：

```
窗口将加载: http://127.0.0.1:40016/_anki/legacyPageData?id=1   # 主窗口
打开窗口: deck-options -> http://127.0.0.1:40016/deck-options/1
进入模态: deck-options（主窗口已禁用）
已打开的窗口: {"main", "deck-options"}
窗口页面已就绪: deck-options   # 页面自己 POST 回来 → 第二个窗口确实渲染并执行了 JS
关闭窗口: deck-options
解除模态: deck-options
```

截图确认牌组选项窗口完整渲染并显示集合里的真实配置；几何写进了 `prefs21.db` 的
`tauriWindowGeometry:deck-options`，同行的 `mainWindowGeom`（`bytes`）未被破坏。

> 关键实现细节：`anki_proto_gen::get_services()` 会把 `FrontendService` 的方法
> 复制进 `BackendFrontendService` 的 `delegating_methods`。因此 `build.rs`
> 必须把 `BackendFrontendService` 过滤掉（与 `rust_interface.rs` 一致），
> 否则这些请求会被转给 rslib 并得到 `InvalidServiceIndex`。

> **与 rslib 同名的端点由平台层接管**：Qt 的 `mediasrv.post_handler_list` 会**盖过**
> 后端服务（`_handle_dynamic_request` 先查处理器表，`mediasrv.py:1343`）。本外壳的
> 服务端是先查方法表，因此 `platform::SHADOWED_METHODS` 里显式列出要接管的名字 ——
> 目前只有 `getConfigJson`：Qt 缺键时返回 `null`（`mediasrv.py:834-838`），
> 而 rslib 的同名方法直接报错。差别会真的咬人：编辑器要读 `renderMathjax` 这类布尔项，
> 新建集合里没有它们，于是弹错误框、`alert()` 又把页面的 JS 事件循环卡住，
> `loadNote` 的 promise 永远不落地（详见「编辑器」与交接文档 §5.18）。

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

## 配置存储

`profile` / `meta` 配置（`getProfileConfigJson`、`setProfileConfigJson`、
`getMetaJson`、`setMetaJson`）直接读写**桌面版同一份 `prefs21.db`**，因此外壳与
桌面版共享设置。实现见 `src/platform/prefs21.rs`：

- 文件是 SQLite，表为 `profiles(name text primary key collate nocase, data blob not null)`；
- `_global` 行的 `data` 是 `ProfileManager.meta`；每个 profile 行的 `data` 是
  `ProfileManager.profile`，profile 名就是 profile 目录名（如 `User 1`）；
- `data` 是 `pickle.dumps(obj, protocol=4)`，用 `serde-pickle` 解析。读取时**保持
  pickle 原值**，只在返回给前端那一步才转 JSON —— 否则 `mainWindowGeom` 这类
  `bytes` 项会在整体重写时被破坏。

行为对齐 Qt：

| 情况 | 行为 |
| --- | --- |
| 项不存在 | 返回 `null`（`profile.get()` 得到 `None`） |
| 值无法用 JSON 表示（如 `bytes`） | 返回错误（Qt 用 orjson 序列化时同样会失败） |
| 数据库不存在 | 读取视为无配置；写入时按 `_loadMeta()` 的方式建表 |

已验证（`tools/make-prefs21-fixture.py` 造数据 + `tools/check-prefs21.mjs` 检查 + 再用 Python 读回）：

```powershell
py tauri/tools/make-prefs21-fixture.py init out/prefs21.db tauri-test
# 启动外壳（集合在 out/tauri-test/ 时，prefs21.db 落在 out/）
node tauri/tools/check-prefs21.mjs
py tauri/tools/make-prefs21-fixture.py show out/prefs21.db tauri-test
```


| 检查项 | 结果 |
| --- | --- |
| 读取桌面版写入的 `numBackups` / `searchHistory` / `syncMedia` / `meta.defaultLang` | 通过 |
| 缺失项返回 `null` | 通过 |
| `bytes` 项（`mainWindowGeom`）返回错误 | 通过（HTTP 500） |
| 写入 `importMode` / `meta.lastMsg` 后由 Python 读回 | 通过 |
| 重写后 `bytes` 项未被破坏 | 通过（仍为 `01020304ff`，其余键保留） |

> 早期版本把配置存在 profile 目录下的 `tauri-*.json` 里，与桌面版互不相通；
> 该实现已移除。

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

### 音频播放

`src/platform/audio.rs` 是 `qt/aqt/sound.py` 的 `AVPlayer` 的等价物：**队列**播放。
`play_tags` 的语义是「清空队列 → 打断当前播放 → 依次播完再播下一个」，因此：

- `playFile` 端点（编辑器里点击音频）与 `AVPlayer.play_file` 行为一致；
- 复习界面在显示问题/答案时自动播放卡片音频，对照
  `Reviewer._showQuestion` / `_showAnswer` 里的
  `if c.autoplay(): av_player.play_tags(c.question_av_tags())`：
  先从渲染结果用 `ExtractAvTags` 提取标签，再由 `getDeck` → `getDeckConfig`
  读牌组的 `disable_autoplay`（默认播放）。

文件名只取 `basename`（对照 `AVPlayer.play_file` 的 SECURITY 注释），避免
`[sound:../../x]` 这类卡片读到媒体目录之外的文件。

已验证：问题面自动播放 1 个音频、答案面 2 个（Basic 的答案模板含 `{{FrontSide}}`，
会带上问题面的 `[sound:]`，与 Qt 的 `answer_av_tags` 一致），日志按顺序出现两次
`播放音频`。

**TTS**（`{{tts en_US:Field}}`）已接入，走的是 rslib 的语音合成，与 Qt 在 Windows 上
实际使用的路径相同，见下面「TTS」。仍未实现的是 `current_caller` 区分
（Qt 用它决定「编辑器发起的播放是否打断复习界面的音频」，属 M5 的音频质量项）。

### TTS

`audio.rs::tts_file` 对照 `qt/aqt/tts.py` 的 `WindowsRTTTSFilePlayer`：

1. `allTtsVoices`（`validate: false`）取语音包列表；
2. 按 `TTSPlayer.voice_for_tag` 的规则挑一个：先按 `voices=` 里写的顺序找「同名 + 同语言」，
   都匹配不上时退回该语言下第一个可用的（`qt/aqt/tts.py:84-110`）；
3. `writeTtsStream` 合成到临时文件，再把该文件塞进音频队列。

为什么不用 SAPI 而走 rslib：Windows 上 `av_player` 会选 rank 最高的播放器，
`WindowsTTSPlayer`（SAPI）的 `default_rank = -1`，而 `WindowsRTTTSFilePlayer` 是默认的 0
（`qt/aqt/tts.py:493`、`:581`）—— 也就是说 **Qt 桌面版实际用的就是后端合成**。
好处是合成完全由 rslib 负责（`rslib/src/card_rendering/tts/windows.rs` 用 WinRT
`SpeechSynthesizer`），外壳不需要新依赖。

合成结果按「语音包 + 文本 + 语速」缓存（对照 Qt 的 `temp_file_for_tag_and_voice`），
同一张卡重复显示不会重复合成。

已验证：本机 `allTtsVoices` 返回 6 个语音包；`writeTtsStream` 产出 76 KB 的 WAV；
`extractAvTags` 能把 `[anki:tts lang=en_US]hello[/anki:tts]` 解析成 TTS 标签。
**复习界面里的完整链路也跑通了**：用 `tools/set-tts-template.mjs` 往 Basic 的正面
模板插入 `{{tts en_US:正面}}` 后重新渲染，日志按顺序出现
`TTS 已合成: …\tts-<hash>.wav` 与 `播放音频: …\tts-<hash>.wav`
（验证完记得用同一个脚本的 `remove` 还原模板，否则后续验证会依赖系统语音包）。

> 注意 `{{tts}}` 里的字段名是**本地化的**：zh-CN 下是「正面」而不是 `Front`，
> 写错会被 rslib 的模板校验直接拒绝（"未找到「Front」字段"）。

### 录音

`recordAudio` 是**长阻塞端点**：请求一直挂着，直到用户在录音窗口里点「保存/取消」
（Qt 也是这个语义：mediasrv 的 `record_audio` + `AsyncRequestHandler`）。分三块：

- `src/platform/record.rs`：`cpal` 采集 + `hound` 写 WAV，采集回调在音频线程上；
- `src/legacy.rs::recorder_page`：录音窗口页面（红点 + 计时 + 保存/取消），
  按钮通过 `bridgeCommand("saveRecording" / "cancelRecording")` 回传；
- `src/server/pycmd.rs`：把这两个命令转给 `Recorder::finish`。

与 Qt 的差异：

| 项 | Qt | 这里 |
| --- | --- | --- |
| 输出格式 | WAV → 外部 `lame` 转 MP3（没装 `lame` 就报错） | 直接给 WAV，不依赖外部可执行文件（一分钟约 5 MB） |
| 取消 | `RecordDialog.reject` 不回复请求方，HTTP 请求永远挂着 | 返回空路径，调用方至少能结束这次调用 |
| 界面 | QDialog（图标 + 计时 + Save/Cancel） | 宿主自建的 legacy 页面（同样的三件套） |
| 置顶 | 跟随父窗口 | `always_on_top`，避免被编辑窗口盖住 |

关掉录音窗口（标题栏的 ×）等同于取消：等待循环轮询窗口是否还在
（`wait_for_outcome`），否则录音会一直占着麦克风。停止采集前多等 0.5 秒
（对照 Qt 的 `QTimer.singleShot(500, ...)`），避免截断结尾。

已验证：录音窗口实时计时；`saveRecording` 后取回路径，产物是合法的
48 kHz / 16 位单声道 WAV（22 秒 = 2,107,244 字节）。

> 踩坑：legacy 页面必须带 `charset=utf-8`。Qt 那边由 Flask 自动补
> （`Response(html, mimetype="text/html")`，见 `qt/aqt/mediasrv.py:1355`），
> 这里最初漏了，于是含中文的录音窗口被浏览器按 GBK 解析成一堆乱码。
> 现在 `server/legacy.rs` 显式发 `text/html; charset=utf-8`，
> `legacy::render` 也补了 `<meta charset="utf-8">`。

### 剪贴板

编辑器只用到两种形态（见 `ts/routes/editor/rich-text-input/data-transfer.ts`）：

- `text/uri-list`：在资源管理器里「复制文件」，粘贴时变成集合媒体文件 —— 编辑器拿到
  路径后交给 `addMediaFromUrl`，rslib 支持 `file://`（`rslib/src/editor.rs:18`）；
- `image/*`：真正的图片数据（截图工具直接放图片）。

`arboard` 只暴露文本与图片，没有文件列表，因此 `text/uri-list` 在 Windows 上直接读
剪贴板的 `CF_HDROP`（资源管理器复制文件时写入的格式），转成 `file:///C:/...` 并按需
转义（空格 → `%20`，否则 rslib 的 `Url::parse` 会失败）。

已验证：往剪贴板放两个文件（其中一个名字带空格）后，`readClipboard` 返回正确的
`file://` 列表且空格被转义；写入方向覆盖 `image/*` 与 `text/*`。

**未覆盖**：拖入文件（HTML5 drop）。Qt 那边由 WebView 的 `dropEvent` 处理，而 WebView2
是否把 `text/uri-list` 暴露给页面的 `dataTransfer` 尚未验证 —— 属于编辑器链路（M3）。

### 计时与评分按钮

对照 `Reviewer._showQuestion` / `_showEaseButtons` / `_answerButtons`：

- `showQuestion(button, maxTime)` 的 `maxTime` 取牌组配置：`timer` 为真时是
  `maxTaken`（秒），否则传 0 —— 前端在 0 时不显示计时，非 0 时从 `0:00` 开始
  计时并在到达上限后变红。
- `showAnswer(buttons, stopTimer)` 的第二个参数是牌组的 `stopTimerOnAnswer`。
- 评分按钮上的间隔来自 `SchedulerService.DescribeNextStates`（返回四档的
  「<1 分 / 10 分 / 1 天 / 3 天」这类本地化文本）。
- 「显示答案」按钮上带剩余队列计数（对照 `_remaining()`）：`新 + 学习中 + 复习`，
  当前卡片所在队列的那一项加下划线。

两项显示开关取自 `getPreferences` 的 `Reviewing`（对应 `col.conf` 的
`estTimes` / `dueCounts`）：`show_intervals_on_buttons` 控制按钮上是否显示间隔
（对照 `_buttonTime()`），`show_remaining_due_counts` 控制是否显示剩余计数。
读取失败时按 Anki 的默认值（都开启）处理。

牌组配置与这两项开关都只在每张卡开始时查一次，结果存在复习会话里。
注意 `Card.current_deck_id()` 用的是 `original_deck_id or deck_id`，筛选牌组里的
卡片要沿用原牌组的配置，这里与之保持一致。

按钮数量固定四个，这不是简化：v3 调度器下 Qt 的 `Scheduler.answerButtons()` 就
返回 4（2/3 个按钮的分支只在 v2 下生效，而 v3 源码里对 v2 是 assert 拒绝的）。

### 自定义调度（界面状态同步）

`getSchedulingStatesWithContext` / `setSchedulingStates` 由
`src/platform/scheduling.rs` 实现，配合 `src/reviewer.rs` 的注入逻辑，复刻 Qt 的
`cardStateCustomizer`：

1. 每张卡开始时读 collection 配置 `cardStateCustomizer`；为空则跳过（视为已完成）；
2. 显示问题时注入
   `anki.mutateNextCardStates(key, async (states, customData, ctx) => { …用户脚本… })`；
3. 前端取状态 → 执行用户脚本 → 带 key 回写，宿主用 key 丢弃过期变更
   （对照 `Reviewer.set_scheduling_states`）；
4. 前端完成后 `bridgeCommand('statesMutated')`，宿主才放行评分按钮（对照 Qt 在
   `_showEaseButtons` 里的 50ms 重试；这里改为最多等 1 秒）。

与 Qt 的差异：注入脚本多了 `.catch(...)`，把脚本抛错经 `stateMutationError:`
回传宿主并写进日志。Qt 只用 `evalWithCallback` 检测语法错误，运行期异常会被静默
丢掉 —— 实测很容易误判成"链路没通"。

验证方式（`tools/set-state-customizer.mjs`）：写一个把「良好」改成 42 分钟的自定义
脚本，重取卡片后该按钮的间隔会从 `<10 分` 变成 `1.4 个月`，同时日志出现
`已接受前端提交的调度状态`。

### 类型作答（`{{type:Field}}`）

对照 Qt 的 `typeAnsFilter`（两个方向）：

- **问题面**：把 `[[type:Field]]` 换成
  `<input id="typeans" onkeypress="_typeAnsPress();">`（字号/字体取自字段设置），
  并记下正确答案与匹配选项。支持 `cloze` / `nc` / `ci` 三个修饰符。
- **答案面**：用 `CompareAnswer(expected, provided, combining, ignore_case)` 生成
  对比 HTML 替换标记；分隔线 `<hr id=answer>` 会被挪到对比结果之前。

取值方式与 Qt 不同但等价：Qt 用 `evalWithCallback("getTypedAnswer();")` 回读输入框，
Tauri 的 `eval` 没有返回值，所以由**桥接垫片**在 `pycmd("ans")` 时把 `#typeans` 的
内容附在命令后面（`ans:<用户输入>`），服务端按前缀解析 —— 前端代码不需要改。

字段的字体与字号只存在于 legacy JSON（proto 的 `Notetype.Field` 不含样式），因此
用 `getNotetypeLegacy` 一次取回字段名与样式。

验证：新建隔离集合 + 「问答题（输入答案）」卡片，问题面出现输入框（日志
`类型作答: 字段=背面 已插入输入框`），提交错误答案后答案面显示红底的输入、`↓`、
以及正确答案。

### 撤销与快捷键

`undo` 桥接命令对应 Qt 的 `mw.undo()` → `operations/collection.py:undo()`：
调用 rslib 的 `CollectionService.Undo`，然后**重新取队列** —— 撤销会改变卡片的调度
状态（通常是把上一张卡放回队列），所以必须重取并重渲染。

快捷键：Qt 用 QShortcut 注册（复习界面的 `u` 与主窗口的 `Ctrl+Z`），单页方案改为在
页面内监听 `keydown`（`src/reviewer.rs` 的 `install_reviewer_shortcuts`）：

- `Ctrl+Z` / `Cmd+Z` → 撤销；
- 裸 `u` → 撤销，但在 `INPUT` / `TEXTAREA` / `contenteditable` 内不生效
  （类型作答时 `u` 是普通字符）。

验证：评分一张卡（队列 `学习中 1` → `学习中 0`）后执行 `undo`，日志出现
`已撤销上一步操作`，队列回到 `学习中 1`。

未实现：重做（`Ctrl+Shift+Z`）与撤销状态显示（没有菜单栏）。

## 编辑器

M3 的第一步：**「编辑当前卡片」窗口**（复习界面底部栏的「编辑」按钮 → `pycmd('edit')`）。

现状：能开窗、能载入当前卡片的 note、能改字段并保存、能关闭。

### 宿主契约（关键）

新版编辑器的**外层窗口完全在 Web 里** —— 牌组 / notetype 选择器、工具按钮、关闭按钮
都由 `ts/routes/editor` 自己画（旧版 `aqt/forms/addcards.ui` 里的那些 Qt 控件，现在
只被 `addcards_legacy.py` 使用）。宿主只做两件事，对照 `qt/aqt/editor.py`：

1. 开一个窗口加载 `editor/?mode=<add|current|browser>`（`editor.py:117-119`）；
2. 收到页面的 `editorReady` 之后注入
   `loadNote({nid, notetypeId, reviewerCardId, focusTo, initial})`（`editor.py:416-447`）。

第 2 步是**必须**的：编辑器不会自己去问后端「当前是哪张卡」，它拿到 `nid` 之后才走 RPC
（`getNote` / `getNotetype` / `getFieldNames`…）取内容。少了这一步窗口会一直空白，
而且**不报错** —— 页面自己是"正常"挂载完成的。

注入结果会回传（`editorLoaded` / `editorLoadError:<msg>`），对照 Qt 用
`evalWithCallback` 拿回调（`editor.py:443`）：否则"注入了但没生效"只能靠肉眼看窗口。

### 字段保存不经过宿主

字段内容由**页面自己**保存：敲键盘 600ms 去抖 → `bridgeCommand("key:<i>")` →
`updateNotes` RPC；字段失焦时立即保存（`NoteEditor.svelte:1508-1524`）。
宿主侧的 `key:` / `focus:` / `blur:` / `editorState:` / `saved` 只记日志
（Qt 用它们维护 `currentField` 与 `EditorState`，服务于 addon 与"未保存"拦截，
本外壳还没有这类需求）。

### 已验证

`tauri/tools/verify-editor.ps1`：开窗 → 断言 `editorLoaded` → 截图 → 用
`closeEditCurrent` 关窗；再由 `editor-check.mjs` 走 Playwright：注入 `loadNote` →
**真的敲键盘** → 失焦 → 用 RPC 复查集合内容（`getNote`）确认落库，最后还原。

```
PASS  note 已渲染出字段 — 2 个可编辑字段
PASS  字段修改已保存到集合 — 正面现在是「…（已编辑25017）」
PASS  已还原测试笔记
```

### 尚未实现

- `more`（更多菜单）仍是 no-op：它要打开菜单，需要新的界面。
- 「字段…」「卡片…」按钮（`openFieldsDialog` / `openCardsDialog`）：新版 Anki 里是原生
  Qt 对话框，仓库里没有对应的 Web 页面，点了没反应。
- AddCards（`mode=add`）窗口：需要宿主注入"新建 note 的默认 deck / notetype"，
  以及关闭前的「丢弃修改」确认框（判定在 Web、弹框在宿主，`addcards.py:107-127`）。
- 浏览器（`mode=browser`）窗口：同样缺界面。

## 验证工具

`tauri/tools/` 下的脚本用来确认改动没有破坏界面与配置：

| 脚本 | 作用 |
| --- | --- |
| `verify-reviewer.ps1` | 一键：启动外壳 → 必要时补一张卡片 → 截真实窗口 → 跑布局断言。改完复习界面先跑它 |
| `verify-m2.ps1` | 一键：验证 M2 的平台能力（剪贴板文件列表、TTS 合成、录音闭环）并检查多窗口的命令归属 |
| `m2-check.mjs` | 上面那个脚本调用的实际检查（HTTP 层端到端断言） |
| `verify-editor.ps1` | 一键：验证「编辑当前卡片」窗口（开窗 → 载入 note → 截图 → 关窗 → 保存链路） |
| `editor-check.mjs` | 编辑器「改字段 → 保存」的检查：Playwright 真实敲键盘，再用 RPC 复查集合内容 |
| `capture-window.ps1` | 截取 WebView2 的真实渲染（PrintWindow / 屏幕两种方式）；多窗口模型下用 `-TitleLike` 指定要截哪个窗口 |
| `visual-check.mjs` | 用 Playwright 复现宿主注入，输出 DOM 度量并做布局断言（定位布局问题用） |
| `seed-card.mjs` | 通过 HTTP API 向测试集合写入一张卡片（可带自定义正反面） |
| `check-prefs21.mjs` | 验证配置端点确实读写了 `prefs21.db` |
| `make-prefs21-fixture.py` | 造一份「桌面版写入的」`prefs21.db` 供上面那个脚本检查 |
| `set-deck-timer.mjs` | 查看 / 开关测试牌组的计时器（验证计时与时间盒） |
| `set-state-customizer.mjs` | 设置 / 清除 `cardStateCustomizer`（验证状态同步） |
| `set-tts-template.mjs` | 往 Basic 正面模板插入 / 移除 `{{tts en_US:…}}`（验证 TTS 链路，用完请 `remove` 还原） |

两个在 Windows 上必须注意的点：

1. **截图前必须声明 DPI 感知**（脚本已做）。否则在缩放 >100% 的屏幕上
   `GetWindowRect` 返回被虚拟化的偏小坐标，而 GDI 按物理像素拷贝同样大小的区域，
   会得到「窗口只截到一部分、右侧按钮消失」的假象，极易误判成布局缺陷。
   实测 175% 缩放下只截到窗口的 2/3。
2. 造卡片时 note type 名是**本地化**的：`zh-CN` 下 `Basic` 叫「问答题」、
   `Cloze` 叫「填空题」、`Image Occlusion` 叫「图片遮盖」，且内置 id 不是 1。

## 尚未实现

- **桥接命令处理**：`/_anki/pycmd` 覆盖复习主链路（`domDone`、`ans`、
  `ease1`..`ease4`、`undo`、`statesMutated`、`close`）与录音窗口的
  `saveRecording` / `cancelRecording`。工具栏动作、编辑器快捷键、`browserSearch`、
  `updateToolbar`、`repaintNeeded` 等命令会记录日志并返回 `null`
  （其中多数依赖 M3 的界面）。
- **复习界面余项**：
  - 底部栏已实现「编辑 / 更多 + 显示答案 + 四档评分」，并接入**计时器**
    （`showQuestion(button, maxTime)`，`maxTime` 取自牌组的 `timer` / `maxTaken`）、
    按钮上的**下次间隔**（`describeNextStates`）与牌组的 `stopTimerOnAnswer`。
    评分按钮固定四个 —— v3 调度器下 Qt 也是四个（`Scheduler.answerButtons()`
    恒返回 4，2/3 个按钮是 v2 的遗留）。
  - ~~`edit`~~ —— **已实现**：打开「编辑当前卡片」窗口，见「编辑器」。
  - `more` 命令仍是 no-op：它要打开更多菜单，需要新的界面。
  - 卡片布局与预览器（同上：需要新的界面）。
  - 音频的 `current_caller` 区分（editor / reviewer 的打断语义），属 M5 的音频质量项。
- **M2 已完成**：mediasrv 的 32 个平台端点全部有实现，详见上文各节。
  唯一没验证的是 **Android 上的录音**：这里统一用 `cpal`（Android 走 oboe），
  而 Qt/AnkiDroid 用的是 `MediaRecorder`，需要在真机确认。
- **未覆盖的剪贴板路径**：拖入文件（HTML5 drop）依赖 WebView2 是否把
  `text/uri-list` 暴露给页面的 `dataTransfer`，属编辑器链路（M3）。
- **M3**：导入导出、图像遮挡、Android 的 SAF 适配。
- **M4**：Windows MSI 打包与签名；Android 的 `tauri android init/build`。

## 已知限制

- **单页面方案需要 CSS 隔离**：Qt 把底部工具栏放在独立的 WebView 里，因此
  `toolbar-bottom.css` / `reviewer-bottom.css` 的 `body` 规则只作用于底部栏。
  本方案两个界面共用一个页面，这些规则会污染卡片页面（`body { margin: 0 }` 会让
  卡片内容贴边、`overflow: hidden` 会让长卡片无法滚动、`button { margin: 9px }`
  会作用到卡片内的按钮），必须靠   `src/legacy.rs` 的 `REVIEWER_ISOLATION_CSS`
  修正。以后共用新的样式文件时要留意同类冲突。
- **legacy 页面必须带 `charset=utf-8`**：Qt 那边由 Flask 自动补，这里必须显式发
  （`server/legacy.rs`），否则含非 ASCII 文本的页面会被按系统编码解析成乱码。
  `legacy::render` 里的 `<meta charset="utf-8">` 是第二道保险；
  新增 legacy 页面时不要绕过 `render()`。
- **静态资源是手工构建的**：`out/sveltekit` 与 `qt/_aqt/data/web/` 的产物目前
  靠 README 里的命令手动生成，尚未接入 `just` 流程。
- **图标**：暂用 `qt/aqt/data/web/imgs/favicon.ico` 的副本，非多尺寸图标集。
- **打包配置**：`bundle.active = false`，尚未验证真实安装包。
- **`capabilities/` 目录不存在**：`tauri-build` 会引用它，当前不影响编译，
  但打包前需要补上。
- **Android 未验证**：路径解析里的 `user_data` 需要改为 `getFilesDir()`
  （`paths.rs` 里已留 TODO）。
- **本工程是独立 workspace**：不参与 Anki 原有的构建与 CI，依赖树独立。
