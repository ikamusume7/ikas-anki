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
| `ANKI_START_PAGE` | `decks` | 窗口加载的 SvelteKit 页面（`decks` = 牌组列表，与 Qt 的启动状态一致；`reviewer` 是 legacy 复习页） |
| `ANKI_OPEN_PATH` | 空 | 测试钩子：设了它，「打开文件」对话框（`openFilePicker`）直接返回这个路径。原生对话框没法自动点，图像遮挡的「选图片」靠它跑端到端（`occlusion-check.mjs`） |
| `ANKI_API_KEY` | 随机生成 | 固定本机 API 密钥，仅用于调试与 e2e |
| `ANKI_LANG` | `en-US` | 界面语言；接受 `zh_CN` 或 `zh-CN`（统一转成 FTL 的连字符形式） |
| `ANKI_BACKUP_NOW` | 空 | 测试钩子：启动时**强制**备份一次并等它写完（默认间隔 30 分钟，脚本等不起），见「自动备份」 |
| `ANKI_RESTORE_PATH` | 空 | 测试钩子：路径，或 `newest`（= 备份目录里最新的 `backup-*.colpkg`）。设了它，「文件 → 还原备份」跳过原生文件对话框与确认框 |
| `ANKI_PICK_COLOUR` | 空 | 测试钩子：设了它，原生取色器不弹对话框、直接当用户选了这个 `#rrggbb`（见「原生取色器」） |
| `ANKI_SYNC_TICK_SECONDS` | 300 | 自动同步定时器的间隔（秒）。测试里设成 2，配合 profile 的 `autoSyncMediaMinutes=0`，几秒内就能看到一次自动媒体同步 |

## 构建步骤

> 打包与分发（可分发目录 / NSIS 安装包 / 签名 / Android）见 [`tauri/PACKAGING.md`](PACKAGING.md)。

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

原先「显式降级」的 10 个端点基本都补上了：

| 端点 | 现状 |
| --- | --- |
| `deckOptionsReady` / `deckOptionsRequireClose` / `importDone` / `importDialogRequireClose` | **已实现**：就绪 / 关闭 / 模态变化都转发给对应窗口 |
| `closeAddCards` / `closeEditCurrent` | **已实现**：两个窗口也都开得出来（裸 `a` / 复习界面的「编辑」） |
| `searchInBrowser` | **已实现**：浏览器的「在本窗口中搜索」（要 await 一次集合查询，见 `server/api.rs`） |
| `openFieldsDialog` / `openCardsDialog` | **已实现**：新写的 `ts/routes/fields/` 与 `ts/routes/cards/`（同样要 await `getNotetype` 算标题），入口是编辑器工具栏的「字段…」「卡片…」 |
| `saveCustomColours` | **仍未实现**：需要能暴露自定义调色板的原生取色器，`rfd` 没有该 API |

窗口的**产品内入口**也补齐了：复习界面的「编辑 / 更多」、主窗口的快捷键与工具栏、
牌组列表的齿轮菜单、编辑器工具栏的两个对话框按钮。`ANKI_OPEN_ROUTES=deck-options/1,preferences`
（与 `ANKI_START_PAGE` 同类的开发/测试入口）仍然保留，用来直接把某个窗口开出来做验证。

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

## 自动备份

对照 `qt/aqt/main.py` 的备份逻辑（PARITY 1.5 / 6.2），实现只有 `tauri/src/backup.rs`
一个文件：

| Qt | 本外壳 |
| --- | --- |
| **关闭时** `col.create_backup(force=False)`（`main.py:726-733`），退出前 `await_backup_completion()`（`main.py:608`） | 外壳没有"优雅关闭"的保证（验证脚本是 `Stop-Process -Force`，真实使用也可能被强杀），改成**启动时先尝试一次** —— 上一次会话的改动因此能被保住 |
| **每 5 分钟**一次（`main.py:1543-1549` 的定时器） | 同左（`backup::spawn` 起一个后台线程，5 分钟一轮） |
| 是否真的落盘交给后端（`BackupLimits`：默认 **30 分钟**间隔 + 集合自上次备份后有改动，`rslib/src/collection/backup.rs:28-48`） | 同左（两个触发点都是 `force=False`，所以实际频率与 Qt 一致） |
| 备份目录 `<profile>/backups`，文件名 `backup-<日期>-<时间>.colpkg`，份数由 `BackupLimits` 自动瘦身 | 同左（目录、文件名、瘦身都由后端负责） |
| 还原：Tools → Open Backup（`main.py:441-477`） | **已实现**：文件 → 还原备份… → 原生文件对话框（默认开在 `<profile>/backups`）→ 原生确认框 → 宿主 **关集合 → `importCollectionPackage` → 重开集合** → 关掉其它窗口、主窗口回牌组列表。`import_collection_package` 要求集合是**关着**的（`rslib/src/backend/import_export.rs:28` 的 `lock_closed_collection`），顺序不能反 |

测试钩子：`ANKI_BACKUP_NOW=1` 在启动时**强制**备份一次并等它写完（默认间隔 30 分钟，
脚本等不起）；`ANKI_RESTORE_PATH`（路径，或 `newest` = 备份目录里最新的那份）让「文件 →
还原备份」跳过原生文件对话框与确认框 —— 两者都自动点不了。`verify-decks.ps1` 用这两个钩子
断言 `out/tauri-test/backups/backup-*.colpkg` 存在、>1KB 且是合法 zip（`PK`），以及还原后
「新的牌组消失、备份里就有的牌组还在」。平时不设 `ANKI_BACKUP_NOW` 时，日志里是
`备份检查: 现在还不需要备份` —— 说明后端的节流生效。

## 原生取色器

编辑器工具栏的「文字颜色 / 高亮颜色」调色板（PARITY 6.5）。上游的链路：`<input type="color">`
在 Qt WebEngine 下弹的是 Qt 的取色器（Windows 上就是系统的 `ChooseColor` 对话框），页面在
`click`/`change` 时调 `saveCustomColours`，mediasrv 把对话框的 16 个自定义色槽写进集合配置
`customColorPickerPalette`（`qt/aqt/mediasrv.py:1122-1128`），下次页面用后端 RPC
`getCustomColours` 把调色板画出来。

WebView2 的 `<input type="color">` 是 **Chromium 自带**的取色器，宿主拿不到它的色槽，所以
本外壳换成宿主侧的 `ChooseColorW`（与 Qt 在 Windows 上用的是同一个对话框）：

| 组件 | 作用 |
| --- | --- |
| `tauri/src/platform/colours.rs` | `pickColour:<当前颜色>` → 弹原生对话框（带 16 个色槽，选完塞进第一位）；平台端点 `saveCustomColours` → 把色槽写进集合配置 |
| `tauri/src/bridge.rs` | Windows 上注入 `window.__ankiHostColourPicker = true`（宿主能力标记） |
| `ColorPicker.svelte` | 有这个标记时，点击不弹 Chromium 的取色器，改发 `pickColour:`，回调里写回颜色并补发 `input`/`change` |

读的那半边没有改动：`getCustomColours` 是后端 RPC，一直可用。测试钩子
`ANKI_PICK_COLOUR` 让 `pickColour` 不弹对话框直接返回该颜色 —— 系统对话框没法自动点，
`verify-editor.ps1` 用它断言"宿主钩子生效"与"色进了自定义调色板"。

> **没被自动化稳定覆盖的部分**：颜色**落到字段上**那一步走的是编辑器工具栏的
> `surrounder`，而它只在这张卡的选区 active 时才生效（工具栏那颗按钮本身就是 disabled
> 状态）。检查脚本里没法稳定复现选中态，所以只断言到"宿主侧取到了颜色并写进调色板"
> 这一层 —— 这条字段级断言在本轮之前的一次运行（队列里是另一张卡）里是过的，之后随
> fixture 漂移丢掉了，故降级成日志输出。

**差异**：非 Windows 平台没有等价的原生对话框（Android 上仍是页面自带的取色器）；
原生对话框没有挂到主窗口上（不是模态子窗口）。

## 同步

工具栏最右的「同步」（快捷键 `Y`，对照 `toolbar.py:410-419` 的同步链接）：

| 步骤 | 实现 |
| --- | --- |
| 未登录 → 登录框 | 邮箱 + 密码 + 「自定义同步服务器」（对照 `aqt/sync.py` 的 `SyncLoginDialog`）；`syncLogin` → `syncKey`/`syncUser` 写进 **profile 配置**（`prefs21.db`，键名与 `qt/aqt/profiles.py:660-725` 一致） |
| 已登录 → 同步 | `syncStatus` → `syncCollection`（`syncMedia` 取自 profile 的 `syncMedia`，默认 true）；服务端要求完整同步（`schema` 时间戳与本地不同，`rslib/src/sync/collection/meta.rs:68-74`）时弹「上传 / 下载」二选一 → `fullUploadOrDownload`（`/sync/upload` 或 `/sync/download`） |
| 媒体同步 | 集合同步成功后（且 `syncMedia` 开着）`syncMedia` 在后台跑 + 轮询 `mediaSyncStatus`，把后端本地化好的「Checked / Added / Removed」显示出来（最多轮询 5 分钟） |
| 取消 | 同步中再点一次「同步」→ `abortSync` + `abortMediaSync`（对照上游进度对话框的 Abort）；被取消的请求当作正常结束，显示「媒体同步已中止」 |
| 自动同步（宿主侧） | `tauri/src/autosync.rs`：启动时整份同步一次（上游的 on-open；外壳没有优雅关闭钩子，所以没有 on-close）+ 之后每 5 分钟检查一次，距上次媒体同步超过 `autoSyncMediaMinutes`（默认 15）才做**媒体**同步。开关与凭据都读 profile 配置（`autoSync` / `syncKey` / `customSyncUrl` / `syncMedia`） |
| 状态显示 | 按钮的 `data-sync-state`（off / idle / syncing / error）+ 列表下方的 `#sync-status` 文案（后端错误原文；个别情况下后端不带消息，页面上就只剩一个 `Error:` —— 已知小瑕疵） |
| 复习界面的同步 | 顶部工具栏的「同步」链接（`reviewer-toolbar.js`）→ 宿主的 `sync` 命令（`pycmd.rs` → `autosync::sync_now`）：有凭据就跑一次集合 + 媒体同步、状态用 `window.__ankiSyncStatus()` 写进工具栏；没凭据就提示一句并跳到牌组列表页（登录框在那儿） |

**验证**（`decks-check.mjs` 5i/5j）：检查脚本自己起一个**假同步服务器**（实现
`/sync/hostKey` 与 `/sync/meta`；同步协议的响应是 zstd 压的 JSON + `anki-original-size`
头，`meta` 的 `ts` 必须是当前时间、`media_usn` 不能缺、`scm` 与本地不同就会走**完整同步**
分支），把 `customSyncUrl` 指向它，然后走「点同步 → 登录 → 完整同步二选一 → 点下载 →
同步中取消」，并另外验证**自动同步**（`verify-decks.ps1` 把 `ANKI_SYNC_TICK_SECONDS`
设成 2 秒，脚本在 profile 里放一份凭据 + `autoSyncMediaMinutes=0` → 断言几秒后宿主真的
打到了 `/msync/begin`，日志里也有「自动同步: 开始媒体同步」）：

```
PASS  登录请求打到了同步服务器 — /sync/hostKey / /sync/meta
PASS  host key 存进了 profile 配置（syncKey） — "fake-host-key"
PASS  需要完整同步时弹出上传/下载二选一 — 上传到 AnkiWeb
PASS  点「下载」打到了 /sync/download
PASS  同步中按钮进入 syncing 状态 / 取消后回到 idle 状态
PASS  自动同步定时器发起了媒体同步（/msync/…） — /msync/begin ×4
```

**差**：「AnkiWeb 冲突」的完整处理（只做了上传/下载入口与冲突文案）、进度条（只有文字
状态）、关闭时同步（没有优雅关闭钩子）；媒体同步的 **happy path** 与
`syncCollection` 的正常合并没有自动化覆盖（假服务器只实现到 `meta`，跑通整条集合/媒体
协议太重）。

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

**拖入文件**（HTML5 drop）不走剪贴板这套 —— 页面读的是 `dataTransfer`，见「编辑器」的
「拖入文件（HTML5 drop）」。

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

快捷键：Qt 用 QShortcut 注册（复习界面的 `u` 与主窗口的 `Ctrl+Z` / `a`），单页方案改为
在页面内监听 `keydown`（`src/reviewer.rs` 的 `install_reviewer_shortcuts`）：

- `Ctrl+Z` / `Cmd+Z` → 撤销；
- 裸 `u` → 撤销，裸 `a` → 打开「添加卡片」窗口（对照主窗口的全局快捷键 `a` →
  `mw.onAddCard()`，`main.py:1183`）；两者都在 `INPUT` / `TEXTAREA` /
  `contenteditable` 内不生效（类型作答时 `u` / `a` 是普通字符）。

验证：评分一张卡（队列 `学习中 1` → `学习中 0`）后执行 `undo`，日志出现
`已撤销上一步操作`，队列回到 `学习中 1`。

未实现：重做（`Ctrl+Shift+Z`）与撤销状态显示（没有菜单栏）。

### 「更多」菜单（= 右键菜单）

底部栏的「更多」按钮与卡片右键都会打开它，对照 Qt 的 `showContextMenu()`
（`reviewer.py:682-683` 的 `more` 链接 → `reviewer.py:998-1056` 的 `_contextMenu`）。
菜单内容与 Qt 一致：旗标子菜单（Ctrl+1..7、当前旗标打勾）、埋藏卡片、重置卡片…、
设置到期日…、暂停卡片、选项、卡片信息、标记笔记、埋藏笔记、暂停笔记、创建副本…、
删除笔记 —— 每项都带 Qt 的快捷键提示。

分工（`src/reviewer_menu.rs` + `assets/reviewer-menu.js`）：

- **宿主**拼菜单数据（文案走 i18n、勾选态读**现取**的卡片/笔记）并执行动作；
- **页面**只负责画菜单与表单，点完回一条 `rev:<动作>[:<参数>]` 桥接命令。

这样页面里没有第二份状态，"菜单勾选态和实际不符"不会发生。几个细节：

- 「重置卡片…」「设置到期日…」要输入，页面里画了一个最小表单（Qt 是 QDialog）；
  确认后回 `rev:forget:<恢复位置>,<重置计数>` / `rev:due:<URL 编码的日期表达式>`；
- 「选项」开既有的 `deck-options/<deckId>` 路由，「卡片信息」开 `card-info/<cid>`，
  「创建副本…」开「添加卡片」并载入当前笔记；
- 「删除笔记」用宿主的原生确认框先问一次（对照 Qt 的 `delete_notes`）。

未做：**上一张卡片信息**（要维护"看过的卡片"历史）、音频的 **重播 / 暂停 / ±5 秒**
（播放器要支持 seek）、**自动前进**（`Shift+A`）。这三项列在 `tauri/PARITY.md`。

验证：`tauri/tools/reviewer-menu-check.mjs`（由 `verify-reviewer.ps1` 调用）——
菜单脚本本身在 Playwright 里跑（渲染、勾选态、子菜单悬停、表单回传、右键发 `more`），
宿主动作对着真外壳发 `rev:*` 并用 RPC 复查（旗标设置/清除、`marked` 标签加/去、
到期日改动 + `undo` 还原）；`verify-reviewer.ps1` 另外断言菜单里开出的
「牌组选项」「卡片信息」两个窗口，并截真实窗口里的菜单（`out/reviewer-check/reviewer-menu.png`）。

### 顶部工具栏（= Qt 主窗口的 TopWebView）

复习界面顶部那条常驻工具栏，对照 `qt/aqt/toolbar.py:353-389` 的 `_centerLinks`：
**牌组 / 添加 / 浏览 / 统计**，每项带自己的全局快捷键提示（`actions-shortcut-key`
生成，例如「快捷键：D」），点了发 `pycmd('decks')` / `add` / `browse` / `stats`。

- 渲染脚本是 `tauri/assets/reviewer-toolbar.js`（宿主 `include_str!` 引用，见
  `reviewer.rs::install_toolbar`），页面侧的容器是 `legacy.rs` 复习页里的
  `#top-toolbar`；高度由脚本写进 `--top-bar-height`，卡片区靠
  `body { padding-top }` 让开 —— 与底部栏同一套办法
  （`reviewer.rs::sync_bottom_bar_height`）。
- 验证脚本读**同一个文件**塞进空页面跑（渲染、间距、点击回传、重复注入不叠加），
  见 §5.27 的做法与 `reviewer-menu-check.mjs`；`verify-reviewer.ps1` 另外截真实窗口
  （`out/reviewer-check/reviewer-toolbar.png`）。

差异：

| 项 | Qt | 本外壳 |
| --- | --- | --- |
| 同步链接 | 有（状态色 + 旋转图标，`toolbar.py:410-418`） | 没有（未实现同步，见 `tauri/PARITY.md` 6.1） |
| 左右两个托盘 | add-ons 的挂载点（`_left_tray_content` / `_right_tray_content`） | 没有 |
| 隐藏行为 | 全屏/鼠标离开时 `hide_if_allowed` 自动隐藏 | 常驻 |
| 实现方式 | 独立 QWebEngineView（`TopWebView`） | 同页面里的固定条（宿主注入） |

牌组列表页也有同一条工具栏（那是 SvelteKit 页面，自己画，见「牌组列表」）。

### 全局入口：统计与设置

主窗口（复习界面所在的那个窗口）的另外两个入口，对照 Qt 的全局快捷键表
（`main.py:1189-1198` 与 `forms/main.ui`）：

| 快捷键 | 打开的窗口 | 上游 |
| --- | --- | --- |
| `t` / `Shift+t` | **统计**（既有的 `graphs/` 页面） | `main.py:1185` → `onStats()`（DeckStats 对话框） |
| `Ctrl+P` | **设置**（既有的 `preferences/` 页面） | `forms/main.ui` 的 `actionPreferences` → `onPrefs()` |

两个都是独立窗口（Qt 是 QDialog），宿主只负责开窗与标题（`src/main_window.rs`）。
差异：

- **统计**：Qt 会把**当前选中的牌组**传进去（`_selectedDeck()`），这里打开的是整个
  集合的视图（页面里可以自己换牌组/搜索词）；
- **设置**：Qt 对话框底部有 Close 按钮，这里用窗口的 ×（页面自己不带关闭按钮）。

未接的全局快捷键（同一张表）：`Ctrl+:`（Python 调试控制台）与 `y`（同步，见
`tauri/PARITY.md` 6.1）；`d`/`s` 已接（见下面的「牌组列表」）。

验证：`verify-reviewer.ps1` 里用桥接命令发 `stats` / `preferences`，断言两个窗口都
打开，并截真实渲染（`out/reviewer-check/graphs.png`、`preferences.png`）。

## 牌组列表（主窗口）

M3 的**启动默认页**（`ts/routes/decks/`）。Qt 的主窗口是个状态机
（`main.py:93-94` 的 `MainWindowState`），本外壳用两个页面覆盖它：

| Qt 状态 | 本外壳 | 上游 |
| --- | --- | --- |
| `deckBrowser`（牌组列表，启动默认） | `/decks/` | `qt/aqt/deckbrowser.py` |
| `overview`（牌组概览） | `/decks/?deck=<id>`（同一页面里的一块面板） | `qt/aqt/overview.py` |
| `review`（复习） | 复习页（legacy 页面），`/decks/` 点「学习」切过去 | `qt/aqt/reviewer.py` |

页面本身（牌组树、计数、齿轮菜单、概览面板）直接调 RPC：`deckTree`（`now` 必填，计数来自
`newCount`/`learnCount`/`reviewCount`）、`setDeckCollapsed`、`addDeck`、`renameDeck`、
`removeDecks`、`getCurrentDeck`、`setCurrentDeck`、`studiedToday`；概览面板另外用
`getDeck`（描述 + 是否筛选牌组）、`renderMarkdown`（markdown 描述）、`congratsInfo`
（有没有埋藏的卡片）、`updateDeck`（存描述）、`unburyDeck`（取消埋藏）、
`rebuildFilteredDeck` / `emptyFilteredDeck`（筛选牌组的重建 / 清空）、
`reparentDecks`（拖拽移动牌组：`newParent = 0` 表示顶层）。
**只有四件事回宿主**：

| 页面动作 | 桥接命令 | 宿主做的事 |
| --- | --- | --- |
| 点牌组名（`open:<did>`） | 页面先 `setCurrentDeck`，再整页导航 | — |
| 概览「学习」 | `study` | 主窗口切到复习页（`reviewer_active=true` + 导航） |
| 齿轮 → 选项 / 底部「导入文件」 | `opts:<did>` / `import` | 开牌组选项窗口 / 先弹原生文件对话框，再按扩展名开导入窗口（模态），见「导入」 |
| 底部「获取共享牌组」 | `shared` | 交给系统浏览器打开 ankiweb 共享牌组页 |
| 工具栏「添加 / 浏览 / 统计」 | `add` / `browse` / `stats` | 复用已有入口 |

主窗口的**页面切换**（`src/main_window.rs`）用整页导航，与卡片信息窗口换卡片同一招
（`window.location.href`）。这里有个坑：**切页前必须把 `reviewer_active` 置回 false**
（`decks`/`overview` 命令里做了）—— 否则牌组列表的 `domDone` 会照着复习页的流程去
`reviewer::start`，往不存在的 `#qa` 注入 DOM。复习页的 legacy 页面只在第一次需要时
注册（`main_window.rs` 用 `OnceLock` 缓存 URL；启动页就是复习界面时由 `main.rs` 记下）。

### 菜单栏（File / Edit / Tools / Help）

Qt 的主窗口菜单栏是原生的、在所有状态下都在；本外壳把它画在**牌组列表页顶部**
（`main.ui` 的 `menuCol` / `menuEdit` / `menuTools` / `menuHelp`），文案用
`qt-accel-*`（去掉 `&` 助记符）：

| 菜单 | 项 | 实现 |
| --- | --- | --- |
| 文件 | 导入… / 导出…(Ctrl+E) / 创建备份 / 还原备份… / 退出 | `import` / 导出对话框（范围=当前牌组）/ `backup` / `restoreBackup` / `quit` 五条桥接命令（后三条是宿主侧：备份目录、关集合导入重开、退出都只有宿主能做） |
| 编辑 | 撤销(Ctrl+Z) / 重做(Ctrl+Shift+Z) | `undo` / `redo` RPC + 刷新 |
| 工具 | 学习牌组 / 创建筛选牌组(F) / 检查数据库 / 检查媒体 / 空卡片 / 管理笔记类型 / 首选项(Ctrl+P) | 学习牌组 = 一个「筛选框 + 完整牌组名列表 + 添加 / 学习」弹框（上游 `aqt/studydeck.py`）：确定后设为当前牌组并进概览，`添加` 新建的牌组也直接进概览；创建筛选牌组 = `aqt/filtered_deck.py` 的等价物（名字 / 搜索 / 顺序 / 限量 / 重排 / 即使为空也创建 → `addOrUpdateFilteredDeck`，顺序标签来自 `filteredDeckOrderLabels`）；三个 `check*` RPC 的结果弹框（`{@html}` 渲染后端给的报告）+ `notetypes` / `preferences` 桥接命令 |
| 视图 | 全屏(F11) / 放大(Ctrl++) / 缩小(Ctrl+-) / 重置缩放(Ctrl+0) | `fullscreen` / `zoom:in|out|reset` 桥接命令 → 宿主 `set_fullscreen` / `set_zoom`（±0.1，钳在 0.3–3；Qt 也不持久化缩放） |
| 帮助 | 用户手册 / 支持 Anki / 关于 | `helpPageLink(INDEX)` → `openLink`；捐赠链接 = `aqt.appDonate`；关于 = `about` 桥接命令返回 JSON（Anki 版本来自 `anki::version`，外壳版本是 `CARGO_PKG_VERSION`），页面渲染成弹框 |

**差异**：菜单栏只在牌组列表页（复习界面是 legacy 页，那里仍是顶部工具栏，功能重合）；
还原备份用的是原生文件对话框 + 原生确认框（上游同款），还原后会关掉其它窗口；
缺 Switch Profile、检查更新、add-ons、「打开备份文件夹」；「关于」比上游少了
Python / Qt / Chromium 版本与贡献者名单；筛选牌组对话框没有第二个筛选条件、预览间隔
（again/hard/good 秒数）、「搜索」按钮（浏览器的搜索编辑器）与 FSRS 专用排序
（可提取性升/降序）。

### 与 Qt 的差异（有意为之）

| 项 | Qt | 本外壳 |
| --- | --- | --- |
| 概览 | 独立页面（`moveToState("overview")`） | 同一页面的面板（`?deck=<id>`），少一次状态切换 |
| 概览底部按钮 | 选项 / 自定义学习 / 重建·清空（筛选牌组）/ 取消埋藏 / 描述（`overview.py:283-314`） | 都在：`O` 选项、`C` 自定义学习、筛选牌组显示 `R` 重建 / `E` 清空、有埋藏卡片时显示 `U` 取消埋藏、「描述」开编辑弹框；快捷键 `o/r/e/c/u` 也照 `overview.py:126-133` |
| 牌组描述 | `DeckDescriptionDialog`（约 79 行）：markdown 勾选 + 纯文本框 | 同一个弹框思路（`Modal` + `textarea` + markdown 勾选），保存走 `updateDeck`；概览里按 `renderMarkdown` 渲染 |
| 取消埋藏的三选一 | 两种埋藏都有时弹 `askUserDialog`（手动 / 计划 / 全部） | 同左（`#unbury-user` / `#unbury-sched` / `#unbury-all`），否则直接 ALL |
| 概览的计数 | `sched.counts()` + 按牌组到期树算出的**埋藏差值**（`-3` 之类，`overview.py:228-266`） | 直接用牌组到期树的 `newCount`/`learnCount`/`reviewCount`；**没做**埋藏差值那一列 |
| 牌组描述里的链接 | 在 WebView 里直接可点 | 同左（`{@html}` 渲染；本页是不可信路由，CSP 的 `script-src` 挡住内联脚本/事件处理器） |
| 牌组导出 | 齿轮菜单里有「导出」 | **已实现**：齿轮 → 导出 → 与本页共用的导出对话框，范围默认「该牌组」（`limit = deckId`）、建议文件名用牌组名；验证见 `decks-check.mjs`（导出笔记纯文本 → 读回文件断言只有这个牌组的笔记） |
| 拖拽移动牌组 | jQuery UI 拖放（`deckbrowser.ts:6-32`）：拖到另一个牌组上 → 变成它的子牌组，拖到顶部那条 `top-level-drag-row` → 回到顶层 | 同左（HTML5 拖放 + 页面内的 `drag:<source>,<target>` 语义 → `reparentDecks`；目标 0 = 顶层）；后端只拒绝「目标必须是叶子节点」（筛选牌组不能当父牌组），错误显示在列表下方 |
| 拖拽排序（同一层内调顺序） | 其实没有：Qt 也只支持改父级，同层顺序由后端按名字排 | 同左 |
| 默认牌组删除保护 | 默认牌组也能删（删了会重建） | **默认牌组的「删除」被禁用** |
| 菜单栏 | 原生 `QMenuBar`，所有状态都在（File/Edit/Tools/Help） | 画在牌组列表页顶部（见上面「菜单栏」一节）：文件/编辑/工具/帮助，能实现的项都接了；复习界面没有它（那里只有顶部工具栏） |
| 工具栏 | 主窗口的 TopWebView，所有页面都在 | 两边都有：牌组列表页自己画，复习界面由宿主注入（`assets/reviewer-toolbar.js`）；同步按钮只在牌组列表页 |
| 同步链接 | 有（带状态/旋转图标，`toolbar.py:410-419`） | 有了，两处都有：牌组列表工具栏最右的「同步」（`Y` 键，页面实现，未登录时弹登录框）与**复习界面顶部工具栏的同步链接**（宿主 `sync` 命令，状态用 `window.__ankiSyncStatus()` 写回工具栏）；见「同步」 |
| 「今日学习」一行 | `statistics-studied-today` | 同一个 key（内容是后端算好的，按 HTML 渲染） |

### 已验证

`tauri/tools/verify-decks.ps1`（+ `decks-check.mjs`）：以 `ANKI_START_PAGE=decks` 启动
→ 断言**启动时创建了备份**（`ANKI_BACKUP_NOW=1` 钩子，产物是合法 zip，见「自动备份」）
→ 截首屏真实窗口（`out/decks-check/decks.png`）→ Playwright 走一遍：表头与三列计数、
当前牌组高亮、折叠/展开（写回 `studyCollapsed`）、点牌组进概览（URL + 当前牌组 +
「学习」按钮）、概览「学习」发出 `study`、齿轮菜单（`opts:<did>` / 重命名 /
**导出**：范围默认该牌组、文件名用牌组名、导出的笔记纯文本里只有这个牌组的笔记 / 删除）、
**概览面板**（描述渲染 + 「描述」弹框改写并保存 + markdown 渲染、埋藏后出现「取消埋藏」
并点掉它）、**拖拽移动牌组**（拖到另一个牌组上 → 名字变成 `父::子`；拖到顶部那条 → 回顶层）、
**菜单栏**（五个菜单渲染；文件→创建备份真的产出新 `.colpkg`、文件→导出… 与 `Ctrl+E` 打开
导出对话框且范围默认当前牌组、视图→三档缩放与 `F11`、工具→检查数据库/检查媒体/空卡片
各自调 RPC 并弹出报告、**工具→学习牌组**筛选框按空格分词匹配 + 选中后进该牌组概览 +
「添加」新建后也进概览、**工具→创建筛选牌组**（快捷键 `F`）顺序下拉来自
`filteredDeckOrderLabels`、构建后 `getDeck` 确认是筛选牌组且搜索条件落库、编辑→撤销发出
`undo`、文件→导入… 发出 `import`、帮助→关于显示版本；退出与两个帮助链接只断言存在）、
新建牌组与删除、底部按钮、工具栏 → 宿主断言牌组选项窗口、导入窗口、导出走了
`ANKI_SAVE_PATH` 钩子、概览「学习」真的切到复习页，以及 `d` / `s` / `study` 三个桥接
命令的来回切页（切到复习页后还会取队列）。筛选牌组的「重建 / 清空」与快捷键 `r`
由 `custom-study-check.mjs` 覆盖（它先建出一个筛选牌组）。
检查会临时建/删牌组，并还原当前牌组；失败留下的临时牌组用
`tauri/tools/cleanup-test-decks.mjs` 清掉。

## 自定义学习（筛选牌组）

牌组概览里的「自定义学习」（对照 Qt 概览底部的「C」，`overview.py:284-292`；快捷键 `c`
也接上了）。窗口是新写的 `ts/routes/custom-study/`，对照 `qt/aqt/customstudy.py`：

| 项 | 上游 | 本外壳 |
| --- | --- | --- |
| 六个选项（提升新卡片/复习上限、复习忘记的、提前复习、预览新卡片、按状态或标签） | `customstudy.ui:19-60` + `onRadioChange`（每项换文案与范围） | 同左，文案逐项对齐（含「可用的新卡片：N」这类提示） |
| 数字框 | `QSpinBox`（范围 see `customstudy.py:109-134`） | `<input type=number>`（同样的上下限与默认值，默认值取后端 `customStudyDefaults`） |
| cram 的四个卡片范围 | `QListWidget cardType`（`customstudy.ui:110-135`） | `<select>` 四选一 |
| 标签包含 / 排除 | 弹 `TagLimit` 对话框（`taglimit.ui`） | **两个文本框**（空格分隔，简化） |
| 确定后 | `CustomStudyRequest` → 后端建/复用「Custom Study Session」并把当前牌组切过去；Qt 只关对话框 | 同左，另外请宿主把主窗口导航到当前牌组的概览（否则用户得自己回去看） |

宿主只做两件事：`customStudy:<did>` 开窗（`main_window.rs::open_custom_study`）、
`customStudyDone` 时导航主窗口 + 关对话框（`server/pycmd.rs`）。**没做**：编辑已有筛选牌组
（`filtered_deck.py` 的 `FilteredDeckConfigDialog`）、概览里的「重建 / 清空筛选牌组 /
取消埋藏」。

验证：`tauri/tools/custom-study-check.mjs`（由 `verify-decks.ps1` 调用）—— 断言六个选项 /
cram 的展开与四选一 / 确定后 `customStudyDone` / RPC 复查「多了一个筛选牌组且它就是当前
牌组」，最后删掉它并把当前牌组切回去；`out/decks-check/custom-study.png` 是页面截图。

## 编辑器

M3 的两块：**「编辑当前卡片」窗口**（复习界面底部栏的「编辑」按钮 → `pycmd('edit')`）
与**「添加卡片」窗口**（复习界面的 `a` 快捷键 → `pycmd('add')`，对照 Qt 主窗口的全局
快捷键 `main.py:1183`）。

现状：两个窗口都能开窗、载入 note（编辑当前 = 当前复习卡的 note；添加 = 一张空的新
note）、改字段、保存、关闭；添加窗口还能真的把 note 加进集合。

### 宿主契约（关键）

新版编辑器的**外层窗口完全在 Web 里** —— 牌组 / notetype 选择器、工具按钮、关闭按钮
都由 `ts/routes/editor` 自己画（旧版 `aqt/forms/addcards.ui` 里的那些 Qt 控件，现在
只被 `addcards_legacy.py` 使用）。宿主只做两件事，对照 `qt/aqt/editor.py`：

1. 开一个窗口加载 `editor/?mode=<add|current|browser>`（`editor.py:117-119`）；
2. 收到页面的 `editorReady` 之后注入 `loadNote({...})`（`editor.py:416-447`）。

第 2 步是**必须**的：编辑器不会自己去问后端「当前是哪张卡」，它拿到 `nid` 之后才走 RPC
（`getNote` / `getNotetype` / `getFieldNames`…）取内容。少了这一步窗口会一直空白，
而且**不报错** —— 页面自己是"正常"挂载完成的。

注入结果会回传（`editorLoaded` / `editorLoadError:<msg>`），对照 Qt 用
`evalWithCallback` 拿回调（`editor.py:443`）：否则"注入了但没生效"只能靠肉眼看窗口。

**两个模式共用一个页面，所以注入参数必须按窗口标签分流**
（`src/editor.rs::on_ready`；`add-cards` 窗口注入"当前复习卡"的话，页面会直接进入编辑
那张卡的状态，而不是新建）：

| 模式 | 窗口标签 | 注入 |
| --- | --- | --- |
| `current` | `edit-current` | `nid` / `notetypeId` / `reviewerCardId` / `focusTo:0` / `initial:true`（`editcurrent.py`） |
| `add` | `add-cards` | `nid:null` / `notetypeId:null` / `deckId:null` / `originalNoteId:null` / `reviewerCardId:<当前复习卡或 null>` / `focusTo:0` / `initial:true`（`addcards.py:76-83`） |

`add` 模式的**默认牌组与笔记类型由页面自己算**（`defaultsForAdding()` /
`defaultDeckForNotetype()`，`NoteEditor.svelte:1026-1045`），宿主不要去推断；但
`initial: true` 是硬门槛 —— 缺了它页面会退到 `notetypeChooser.getSelected()` 那条路，
拿不到「新建 note」。`reviewerCardId` 是页面算「home deck」的输入（`addcards.py:49`）。

### 关闭「添加卡片」时要问过页面

页面**只判定、不弹框**：`closeAddCards()` 先 `saveNow()`，再把
`shouldPromptBeforeClosing()` 的结果（所有字段都为空 / `<br>` 才算"没有输入"）当成
`Bool` 交给宿主（`NoteEditor.svelte:458-480`；Web 侧整个目录没有 `confirm`）。
弹框与关窗都要宿主做，对照 `addcards.py:107-127` 的 `_close_if_user_wants_to_discard_changes`：

- `closeAddCards(val=true)` → `dialogs::confirm_discard_input`（文案
  `adding-discard-current-input`）→ 用户选「放弃」才关窗，否则窗口保持打开；
- **标题栏的 × / Alt+F4 也走同一条路**（`windows.rs` 拦截 `CloseRequested`，先
  `prevent_close()` 再 `eval("closeAddCards()")`，页面没挂载完时退化成直接请求关窗）——
  否则用户可以绕过确认框无声丢掉正在输入的内容。宿主自己发起的关闭用一个标记位区分，
  不然确认流程会拦到自己（死循环）。

按钮文案的取舍：Qt 的「Discard」用的是 Qt 自带的标准按钮翻译，仓库里没有对应 ftl key
（`adding-keep-editing` 只覆盖另一个按钮），所以这里用系统默认的是 / 否。

### 字段保存不经过宿主

字段内容由**页面自己**保存：敲键盘 600ms 去抖 → `bridgeCommand("key:<i>")` →
`updateNotes` RPC；字段失焦时立即保存（`NoteEditor.svelte:1508-1524`）。
宿主侧的 `key:` / `focus:` / `blur:` / `editorState:` / `saved` 只记日志
（Qt 用它们维护 `currentField` 与 `EditorState`，服务于 addon 与"未保存"拦截，
本外壳还没有这类需求）。`add` 模式下 `updateCurrentNote()` 直接 return（`mode !== "add"`
的判断），所以「添加」之前不会有 `updateNotes`。

### 已验证

`tauri/tools/verify-editor.ps1`：开窗 → 断言 `editorLoaded` → 截图 → 用
`closeEditCurrent` 关窗；再由 `editor-check.mjs` 走 Playwright：注入 `loadNote` →
**真的敲键盘** → 失焦 → 用 RPC 复查集合内容（`getNote`）确认落库，最后还原。

```
PASS  note 已渲染出字段 — 2 个可编辑字段
PASS  字段修改已保存到集合 — 正面现在是「…（已编辑25017）」
PASS  已还原测试笔记
```

`tauri/tools/verify-addcards.ps1`（+ `addcards-check.mjs`）：`add` 命令开窗 → 断言注入的
是"新建 note" → 截图 → Playwright 验默认牌组/笔记类型、**真的填字段并点「添加」**、
再用 `searchNotes` 回查集合（新增的 note 确实在，内容正确，随后删掉）→ 最后用 RPC 验
宿主侧的两条关闭契约（`val=false` 直接关、`val=true` 停在确认框上且窗口保持打开）。

```
PASS  牌组选择器已渲染 — 「Default   选择牌组     Default」
PASS  新建 note 已渲染出字段 — 2 个可编辑字段
PASS  有输入时页面报告需要确认 — closeAddCards(val=true)
PASS  「添加」按钮存在 — 按钮序号 3
PASS  点击「添加」发出了 addNote — 1 次请求
PASS  addNote 带着字段内容 — 「TauriProbe43753」
PASS  新增的 note 已落库 — 搜到 1 条
PASS  无输入时页面报告可直接关闭 — closeAddCards(val=false)
```

> 检查「添加是否落库」走的是**拦请求 + 查集合**，不是读 Playwright 抓到的响应体：
> 实测那条 `addNote` 响应在 Playwright 里是 `200` + **0 字节**（`response.body()` 空），
> 而同一个请求用 curl 直发能拿到 29 字节的正常响应 —— 页面本身是成功的（表单随后被
> 重置、note 也确实进了集合）。踩过一次，别再照抄"读响应体"的写法。

### 尚未实现

- ~~`more`（更多菜单）~~ —— **已实现**：复习界面底部栏的「更多」与卡片右键会打开菜单，见「复习界面」的「更多」菜单。
- ~~「字段…」「卡片…」按钮~~ —— **已实现**：`openFieldsDialog` / `openCardsDialog` 两个
  端点各自对着编辑器窗口当前的东西开窗（见下面两节）。

### 「字段…」对话框

编辑器工具栏的「字段」按钮（`NotetypeButtons.svelte:43`）在非 legacy 路径下发的是前端端点
**`openFieldsDialog`**（不是桥接命令），端点不带参数 —— Qt 那边对着**编辑器窗口当前的笔记类型**
开（`mediasrv.py:1096-1106`）。所以宿主在注入 `loadNote` 时记一份「窗口 → 笔记类型」
（`editor.rs::remember_notetype`），端点处理放在 `server/api.rs`（要 await 一次 `getNotetype`
才能算出标题 `fields-fields-for`），窗口与标题在 `main_window.rs::open_fields_for_current`。

页面 `ts/routes/fields/` 对照 `qt/aqt/fields.py`：

| 项 | 上游 | 本外壳 |
| --- | --- | --- |
| 字段列表 + 添加/重命名/删除/重新定位 | `fields.ui:43-64`，改的是内存里的 notetype | 同左（弹框输入名字 / 位置；删除守卫：≥2 字段、`preventDeletion`） |
| 编辑字体 | `QFontComboBox` + `QSpinBox`（5-300），写 `Field.Config.fontName/fontSize` | 文本框 + 数字框（**差**：系统字体下拉） |
| 描述 / 排序字段 / RTL / HTML 默认 / 折叠 / 排除搜索 | `fields.ui:96-181` | 同左 |
| 保存 / 取消 | `accept` 一次性写回整份 notetype；`reject` 有改动先问一句 | 同左（`updateNotetype` / `card-templates-discard-changes`） |

验证：`tauri/tools/fields-check.mjs`（由 `verify-editor.ps1` 调用）—— 载入字段列表 →
添加字段 → 保存 → 用 RPC 复查真的落库 → 用检查开始时留的**原始字节**原样还原
（`updateNotetype` 收的就是 `getNotetype` 的响应），并截图 `out/editor-check/fields-window.png`。

### 「卡片…」对话框

编辑器工具栏的「卡片」按钮 / `Ctrl+L`（`NotetypeButtons.svelte:61`）走同一个套路，端点是
**`openCardsDialog`**（`mediasrv.py:1109-1116` 的 `open_cards_dialog`）。端点同样不带参数，
但 Qt 那边要三样东西（`editor.py:312-332` 的 `onCardLayout`）：

| 东西 | Qt | 本外壳 |
| --- | --- | --- |
| 笔记类型 | `window.editor.note.mid` | `editor::current_notetype_and_note`（注入 `loadNote` 时记「窗口 → 笔记类型/note」） |
| 预览用的 note | 编辑器当前那张；新建笔记时是空 `Note(col, mid)` | 同左（`?note=`；没有就由页面造一张空笔记） |
| 初始卡片序号 | 当前卡片的 `ord`（`editor.py:315`），无卡取 0 | 同左（只有「编辑当前」窗口对应复习卡；新建笔记取 0） |

页面 `ts/routes/cards/` 对照 `qt/aqt/clayout.py`（**884 行**）：

| 项 | 上游 | 本外壳 |
| --- | --- | --- |
| 骨架 | 顶部「卡片类型: [下拉] [选项 ▾]」；左边三个**单选**页（正面 / 背面 / 样式，不是页签）；右边预览；底部按钮行 | 同左 |
| 编辑器 | 纯文本 `QTextEdit`（`setAcceptRichText(False)`） | `<textarea>` |
| 添加字段 | 弹「字段 + 字体 + 字号」→ 追加 `<div style='font-family: …'>{{字段}}</div>` | 同左（**差**：字体是文本框，Qt 是系统字体下拉） |
| 翻转 | 按 `<hr id=answer>` 切开背模板再互换 | 同左 |
| 预览 | `note.ephemeral_card(ord, custom_note_type=…, custom_template=…)` → `RenderUncommittedCard`；正/背面 + 填充空字段 + 夜间模式 | 同左（iframe + `srcdoc`；媒体相对路径靠 `<base>` 指到本地服务） |
| 卡片类型 | 下拉切换；菜单里增/删/改/重排（改内存后整体保存） | 同左（重排是弹数字框，Qt 也是 `getOnlyText`；**差**：Qt 的下拉每项是「序号: 名字: 正面字段 -> 背面字段」的摘要（`_summarizedName`），这里只显示名字） |
| 选项菜单 | 恢复默认 / 增删改重排 / 牌组覆盖 / 复制信息到剪贴板 / 浏览器外观 | 同左（**差**：复制后没有 "copied to clipboard" 提示 —— 那个 ftl key 本仓库里已经不存在了，`clayout.py:780` 引用的 `about_copied_to_clipboard` 是坏的；牌组覆盖用下拉，Qt 是带补全的 `TagEdit`） |
| 保存 / 取消 | 整体 `update_notetype_legacy`；`reject` 有改动先问「保存 / 放弃 / 取消」 | `updateNotetype`；取消时只问「放弃吗」 |
| 帮助 | `HelpPage.TEMPLATES` | **没做**（要接帮助页） |

两个上游行为值得记一下：

- 「添加卡片类型」复制的是**当前**模板（`clayout.py:707-711`），保存前必须把正面改得不一样，
  否则后端直接拒绝（"The front side is identical to card template N"）—— Qt 那边同样存不进去；
- 删除卡片类型的确认框里带卡片张数：Qt 用 `template_use_count`（Python 侧），本外壳用一次
  `card:<序号> note:"<笔记类型>"` 搜索代替。

验证：`tauri/tools/cards-check.mjs`（由 `verify-editor.ps1` 调用）—— 载入模板与样式 →
改正面模板，断言**未保存**的改动已经出现在预览里（`RenderUncommittedCard`）→ 添加卡片类型 →
保存 → 用 RPC 复查模板与卡片类型都落库 → 用原始字节还原，并截图
`out/editor-check/cards.png` 与真实窗口 `cards-window.png`。

### 「管理笔记类型」对话框

上游 Qt 的 `Models` 对话框（`qt/aqt/models.py:39`）有两个入口：主窗口菜单
「管理笔记类型」（`main.py:1357` → `Models(mw, mw, fromMain=True)`）与浏览器侧栏
Note Types 节点的右键菜单（`browser/sidebar/tree.py:924-928`）。本外壳还没有菜单栏
（PARITY 1.2），所以入口**暂时挂在浏览器窗口的「更多」菜单**里：
`ts/routes/browser/+page.svelte` 的 `manageNotetypes()` → `bridgeCommand("notetypes")`，
菜单项 `data-action="manageNotetypes"`（文案 `browsing-manage-note-types`）。

窗口由宿主开：`windows.rs` 的标签 `NOTETYPES`、`main_window.rs::open_notetypes`
（尺寸 560×640，标题取 `notetypes-note-types`），`server/pycmd.rs` 里
`"notetypes"` → `open_notetypes`。页面是新写的 `ts/routes/notetypes/`：

| 项 | 上游 | 本外壳 |
| --- | --- | --- |
| 列表 | `models.py:157-158`：每行「名字 [笔记数]」，数据来自 `getNotetypeNamesAndCounts` | 同左（文案 `browsing-note-count`）；双击 / `F2` 重命名、`Delete` 删除 |
| 添加 | 两步（`models.py:166-187` + `AddModel`）：先选原厂类型或克隆现有类型，再问名字 | 同左（6 种原厂类型 + 每个现有类型一条克隆项；落库走 `addOrUpdateNotetype`，`id=0` 即新增，与 Qt 的 `add_notetype_legacy` 同一条路） |
| 重命名 | `actions-new-name`（预填当前名字）→ `update_notetype_legacy` | `getNotetype` + `updateNotetype` |
| 删除 | `models.py:189-198`：只剩一个类型时只提示；有笔记在用 → 「连同卡片一起删」，否则 → 「删掉这个没人用的类型」 | 同左（`confirm()` 的文案就是那两个 ftl key） |
| 「字段…」「卡片…」 | 只在 `fromMain=True` 时给这两个按钮，各自开 `Fields` / `Layout` | 两个按钮都给，走宿主端点 `openFieldsDialogForNotetype` / `openCardsDialogForNotetype`（请求体 `NotetypeId{ntid}`）→ `fields/?notetype=<id>` / `cards/?notetype=<id>&ord=0`（复用上面两个对话框） |

差异：Qt 的「选项」按钮开的是老的 `ModelAdvanced`（内容就是字段/卡片两块），本外壳的
两个按钮已覆盖，没有照搬；入口位置见上（等主窗口菜单栏）。

> 原厂类型的 JSON 走 `getStockNotetypeLegacy`、克隆走 `getNotetypeLegacy`，新建时把
> `id` 置 0 —— `NotetypeSchema11.id` 是必填数字，既不能删也不能给 `null`（原厂 JSON
> 里干脆没有这个字段，两种错法分别报 `missing field id` 与 `StringOrInt`）。

验证：`tauri/tools/notetypes-check.mjs`（由 `verify-browser.ps1` 调用）—— 列表渲染
（数量与「名字 [笔记数]」）→ 两步添加原厂类型（RPC 复查 2 字段 / 1 模板）→ 克隆现有
类型 → 重命名 → 「字段…」「卡片…」两条端点 → 删除的确认文案是「没人用的类型」→
收尾把本次建的临时类型删干净；另截真实窗口 `notetypes.png` 与页面 `notetypes-page.png`。

### 拖入文件（HTML5 drop）

编辑器工具栏与字段都在页面上，拖入文件也一样：**上游 TS 早就实现了整套逻辑**
（`ts/routes/editor/rich-text-input/data-transfer.ts` 的 `handleDrop` → 图片 / 音频 /
URL / 文本 / HTML 五支），落回字段走 `pasteHTML`，媒体落库走 rslib 的 `addMediaFile`
/ `convertPastedImage` —— 宿主**一个端点都不用加**。

宿主唯一要做的是**别挡路**：Tauri 默认会顶掉 WebView2 自己的拖放处理器（好把事件发成
`DragDropEvent`），页面就收不到 HTML5 drop 了。`WindowSpec` 因此多了 `html5_drops`，
打开它就给窗口调 `disable_drag_drop_handler()`：

| 窗口 | 上游对应的地方 |
| --- | --- |
| `edit-current` / `add-cards` / `browser-editor` | `editor.py:120` 的 `self.web.allow_drops = True` |
| 主窗口（复习界面 / 牌组列表） | `reviewer.py:355` |
| `preview` | `previewer.py:158` |

（`clayout.py:371` 也给卡片布局的预览开了，不过我们这个页面的预览是 iframe，drop 由
外层页面处理，没有额外的开关。）

差异：开关一关，Rust 侧就收不到 `DragDropEvent` 了。**主窗口因此是例外** —— 它保留 Tauri
的拦截（`main.rs:129-133`），好让拖进来的文件拿到**真实路径**去走「拖 `.apkg` 就导入」
（见下面的「导入」）；编辑器那几类窗口才需要 HTML5 drop。

验证：`tauri/tools/drop-check.mjs`（由 `verify-editor.ps1` 调用）—— 用 Playwright 在
页面里**合成**一次带真 PNG 文件的 `drop`，断言图片插进字段（`<img src="paste-<哈希>.jpg">`）、
文件真的落进 `collection.media`、失焦保存后集合里的 note 也有它、页面无报错，最后还原
字段并删掉生成的媒体文件。**没覆盖**「操作系统 → WebView2」那一步（要真的按住鼠标拖），
那半只能靠 `verify-editor.ps1` 断言宿主日志「已关闭 Tauri 的文件拖放拦截」。

写这个检查时踩到的三个坑（别再踩）：

- `locator.dispatchEvent("drop", { dataTransfer })` 造出来的事件在页面里**读不到**
  `dataTransfer`（编辑器拿到 `undefined`，于是什么都不发生），得用
  `elementHandle.evaluate` 在页面里自己 `new DragEvent("drop", { dataTransfer })`；
- 字段在 **shadow DOM** 里，`page.evaluate` 里的 `document.querySelector` 找不到它，
  要用 Playwright 的 locator（它穿透 shadow DOM）；
- 顺手记一条：后端会**真的解码**图片（`convertPastedImage` → image crate），随手抄的
  1x1 PNG base64 很可能是坏的（原来的遮挡夹具就是，只是没人解码它才一直没暴露），
  挑一张 CRC 正确的再写进脚本。

## 导入（选文件 / 拖入 / 牌组列表按钮）

对照 Qt 的三条入口（`importing.py:153-163` 的 `getFile()`、`main.py:112-137` 的窗口
`dropEvent`、`deckbrowser.py:399` 的「导入文件」），**实现都在 `tauri/src/importing.rs`**：

| 入口 | 怎么进 | 做什么 |
| --- | --- | --- |
| 牌组列表底部「导入文件」 | 页面发 `pycmd("import")` | `prompt_and_import`：弹**原生文件对话框**（`rfd`），拿到路径后按扩展名挑页面 |
| 主窗口拖入文件 | Tauri 的 `WindowEvent::DragDrop`（主窗口**保留**拦截，才拿得到真实路径） | `on_drop`：存在的文件才收、目录跳过、`.colpkg` 排除（`main.py:131` 同样过滤）、一次只导一个，然后同上 |
| 其它调用方 | `importing::open_for_path`（例如测试里用 `ANKI_OPEN_PATH` 顶掉对话框） | 同上 |

挑页面的规则（`target_for_path` + `route_for`，与 Qt 的 `importing.py:137-146` 一一对应）：

| 文件 | 打开 |
| --- | --- |
| `.apkg`、`.zip` | `import-anki-package/<百分号编码的路径>`（`import_dialog.py:68` 就是这个 URL 形状） |
| `.csv`、`.tsv`、`.txt` | `import-csv/<…>` |
| 其他 | 弹 `importing-unable-to-import-filename` 提示框，不开窗 |

路径走**一个 URL 段**（catch-all 路由 `[...path]`）：`/` 保留，`\`、`:`、空格等都百分号
编码（`importing::encode_path`，与 JS 的 `quote` 默认行为一致），页面再 `decodeURIComponent`。
**已知差异**：整体替换集合的包（`.colpkg`、`collection.apkg`、`backup-*.apkg`）与 Mnemosyne
`.db` 只提示「不支持」，Qt 那边会走 `import_colpkg` / 重命名当前集合。

验证：`tauri/tools/import-check.mjs`（由 `verify-decks.ps1` 调用）—— 建临时牌组 + 卡片 →
`exportAnkiPackage` 导出成夹具（**牌组限定**）→ 删掉临时牌组与卡片 → 用 Playwright 打开
宿主会打开的那个 URL → 点「导入」→ 断言牌组与卡片真的进了集合、页面发过 `importDone`、
页面无报错 → 收尾删干净。宿主那一半（原生对话框 + 挑页面 + URL 编码）由
`verify-decks.ps1` 的日志断言覆盖（`打开窗口: import -> …import-anki-package/…import-fixture.apkg`），
纯逻辑（按扩展名挑页面、拖入时的过滤）由 `importing.rs` 的 `#[cfg(test)]` 单测覆盖。

两个坑记在这里：

- `ExportAnkiPackageRequest.limit` 是 `ExportLimit`，它**自己还有一层同名 oneof**：要写
  `limit: { limit: { case: "deckId", value: id } }`。少一层不报错，但会**静默导出整个集合**
  —— 夹具里连别的牌组与笔记都在，导回去会把集合搅乱（踩过）；
- 导入是**分步落库**的：牌组先出现、`note` 晚一步。判断"导入完成"要等**卡片**，等牌组会
  读到「0 张」（踩过）。
- 新集合的默认配置 `mergeNotetypes=false`，而**从同一份集合导出的 apkg 里那个笔记类型
  与集合里的是同一个（同 id）** —— 这个组合下导入只建牌组、**不带笔记**（`apkg/import/
  notes.rs:144` 那条分支；页面还传了 `alertOnError: false`，所以看着"没报错却什么都没
  导进来"）。检查脚本因此先像用户勾上「合并笔记类型」那样把配置开上，导入完还原。

### 图像遮挡（内嵌遮罩编辑器）

这个版本的 Anki **没有独立的遮挡窗口**：遮挡编辑器就是编辑器页面里的
`ImageOcclusionPage`（`MaskEditor` 的画布 + 形状工具），由工具栏的
`image-occlusion-button` 切换 —— 对照 `qt/aqt/editor.py:500-535` 的
`setup_mask_editor`（Qt 也是往页面里 eval `setupMaskEditorForNewNote/…`）。
所以外壳这一侧没有「开窗口」的活，宿主契约只有一条：

| 页面动作 | 走哪 | 宿主做什么 |
| --- | --- | --- |
| 工具栏「遮挡」按钮 | 页面内 `$ioMaskEditorVisible` | 不经过宿主；只有 `ioImageLoaded:` 会回传一条，宿主记日志（对照 `editor.py:364-372`） |
| 图片选择器「选图片」 | `/openFilePicker`（`data-transfer.ts:455`） | 弹原生文件对话框并返回路径；测试用 `ANKI_OPEN_PATH` 跳过 |
| 选完图 | `addMediaFromPath` + `getImageForOcclusion` | 都在后端，页面自己调 |

验证：`tauri/tools/occlusion-check.mjs`（由 `verify-editor.ps1` 调用）—— 用 Chromium
截一张真图片 → 在 add 模式的编辑器页面里断言「遮挡按钮出现 → 进图片选择器 →
`/openFilePicker` 被调用 → 图片拷进 `collection.media` → 掩码编辑器渲染出 data URL」，
并截图（`out/editor-check/occlusion-editor.png`）。**没覆盖**：在画布上画遮罩 →
保存成遮挡笔记（要真的拖拽 + 校验 `{{c1::…}}` 字段，值得另开一条检查）。

一个上游时序问题（与本外壳无关，记一下）：**编辑已有笔记**时换图片走
`getImageOcclusionNote(noteId)`（`mask-editor.ts:58-76`），读的是**已保存**的笔记 ——
页面刚把新图片写进字段、还没落库，就会拿到空图（`#image` 的 src 只有
`data:image/png;base64,`）。add 模式按文件路径取图，不受影响。

## 浏览器

M3 的另一块：**浏览器（Browse）窗口**。与编辑器不同，**这个页面是外壳自己新写的**
（`ts/routes/browser/`）—— Qt 的浏览器整条链路都是原生 Qt 控件（`qt/aqt/browser/`：
搜索框、笔记表格、侧边栏、菜单栏、嵌在同一个窗口里的编辑区），仓库里没有对应的 Web 页面。

入口：

- 复习界面按 `b`（对照 Qt 主窗口的全局快捷键 `b` → `mw.onBrowse()`，默认搜索
  `defaultSearchText`，没配置则 `deck:current`）；
- 图表点柱子 → `browserSearch:<query>`（由 `ts/routes/graphs/GraphsPage.svelte:25-27`
  发出，对照 `qt/aqt/stats.py:132-136`）；
- `searchInBrowser` 端点（导入页、编辑器的「重复」链接）→ 请求体是 `search.SearchNode`，
  宿主用 `buildSearchString` 翻成搜索词再开窗（对照 `mediasrv.py:755-762`）。

已实现的功能，以及各自对照的上游实现：

| 功能 | 上游 |
| --- | --- |
| 搜索（后端语法字符串） | `browser.py:502-524` `onSearchActivated` |
| **排序**（点列头，交给后端 `SortOrder.builtin`，落库 `sortType`/`sortBackwards`） | `table/table.py:492-516`、`collection.py:692-719` |
| **多选**（单击 / Ctrl+单击 / Shift+区间 / Ctrl+A / Esc） | QTableView 原生行为 + `table.py:351-407` |
| **加标签**（Ctrl+Shift+A，`addNoteTags`） | `browser.py:961-974` |
| **移除标签**（Ctrl+Alt+Shift+A，`removeNoteTags`） | `browser.py:976-988` |
| **清除未使用标签**（`clearUnusedTags`） | `browser.py:997-1000`、`aqt/operations/tag.py` |
| **标记**（Ctrl+K，`marked` 标签的加/删） | `browser.py:1083-1093` |
| **旗标**（Ctrl+1..Ctrl+7，`SetFlag`；同一旗标再按 = 清除） | `browser.py:1056-1077` |
| **暂停 / 埋藏**（Ctrl+J / Ctrl+Shift+J，`BuryOrSuspendCards`） | `browser.py:1006-1042` |
| **重置（Forget）**（Ctrl+Alt+N，弹框两个勾选项） | `aqt/forms/forget.ui`、`operations/scheduling.py:95-131` |
| **设置到期日**（Ctrl+Shift+D）+ **重新定位**（Ctrl+Shift+S） | `operations/scheduling.py:30-62,134-206` |
| **立即评分**（菜单里的四档） | `browser.py:1129-1167` |
| **卡片信息**（Ctrl+Shift+I，独立窗口，复用 `card-info/<cid>` 路由） | `browser.py:799-803`、`aqt/card_info.py:62` |
| **复制笔记**（Ctrl+Alt+E：开「添加卡片」并载入该笔记） | `browser.py:828-834` |
| **更改笔记类型**（菜单：开 `change-notetype/<id>` 窗口，页面里映射字段后保存） | `browser.py:1241-1252`、`aqt/changenotetype.py` |
| **导出**（Ctrl+Shift+E：牌组包 / 整个集合 / 笔记纯文本 / 卡片纯文本，问宿主要保存路径后调导出 RPC） | `browser.py:377`、`qt/aqt/import_export/exporting.py` |
| **查找重复**（菜单：「搜索位置」字段 + 可选筛选器 → 搜索 → 分组报告；点一组跳到那些笔记、可一键打标签） | `browser/find_duplicates.py`、`forms/finddupes.ui`、`collection.py:744-778` 的 `find_dupes()` |
| **查找替换**（Ctrl+Alt+F，`FindAndReplace`） | `aqt/browser/find_and_replace.py` |
| **改牌组**（Ctrl+D，`setDeck`） | `browser.py:918-949` |
| **删除**（Ctrl+Del / Delete，`removeNotes`，删的是**笔记**） | `browser.py:895-906` |
| **撤销/重做**（Ctrl+Z / Ctrl+Shift+Z） | 浏览器菜单的 Undo / Redo |
| **反选 / 全选 / 选中笔记**（Ctrl+Alt+S / Ctrl+Alt+A / 菜单） | `browser.py:308-309,1175-1183` |
| **跳转**（Home / End / Ctrl+N / Ctrl+P / Shift+Home / Shift+End） | `browser.py:379-397` |
| **笔记模式**（`browserTableShowNotesMode` + `activeNoteCols` + `searchNotes`） | `table/table.py:40,196-206` |
| **列清单**（表头右键勾选，立即写回 `activeCols`） | `table/state.py:137-148`、视图菜单 |
| **列宽拖动**（按模式存 profile 配置 `tauriBrowserColumnWidths`） | `table.py:326-335` + `utils.saveHeader` |
| **列顺序拖动**（拖列头换位，写回 `activeCols` 的顺序） | `table.py:391` 的 `sectionMoved`（Qt 把视图顺序记在表头状态里） |
| **预览**（问题/答案、上一张/下一张、重播音频、仅背面） | `browser/previewer.py`（见「预览窗口」） |
| 双击开编辑器 | `browser.py:653-660`（Qt 是内嵌编辑区，见差异表） |
| 侧栏（牌组树 / 笔记模板 / 当前牌组 / 全部集合） | `sidebar/tree.py` |
| 窗口标题（`browsing-window-title(-notes)`） | `browser.py:549-559` |

动作表在网页上没有菜单栏可用，所以放在**行右键菜单**（`ContextMenu.svelte`）与工具栏的
「更多」按钮里，每一项都带 Qt 的快捷键提示；快捷键本身由 `+page.svelte` 的 `onKeydown`
实现（表见 `qt/aqt/forms/browser.ui` 的 `shortcut` 属性）。菜单项带 `data-action`
（`actionToggle_Mark` 那样的名字），验证脚本按它点击，不去匹配本地化文案。

### 宿主契约

与编辑器同构，但**搜索词是"页面问、宿主答"**：

1. 页面挂载完 → `pycmd("browserReady")`，宿主在**响应**里交回首屏搜索词
   （`{"search": "deck:current"}`）；
2. 窗口已存在时宿主改走 `eval` 注入 `loadSearch("...")`；注入时页面若还没挂载好会自己
   忽略，因为它随后会在报到时把搜索词取走 —— 两条路径都幂等。

页面发给宿主的命令：`browserTitle:<notesMode>:<selected>:<total>`（宿主用
`browsing-window-title(-notes)` 本地化后设成窗口标题）、`editCard:<cid>` /
`editNote:<nid>`（双击一行）、`preview:<cid>`（预览，见下）。

### 表格数据

- **列清单**：先读集合配置 `activeCols`（缺省 `["noteFld", "template", "cardDue", "deck"]`，
  对照 `pylib/anki/browser.py:33`），再 `setActiveBrowserColumns` —— 少了这一步
  `browserRowForId` 会直接报 `Active browser columns not set.`
  （`rslib/src/browser_table.rs:276-285`），现象是整张表格空白 + 弹错误框。
- **排序**：只把「列 key + 方向」交给后端（`SortOrder.builtin`），前端不自己排；不可排的列
  （`sorting_cards == SORTING_NONE`）点了给 `browsing-sorting-on-this-column-is-not` 提示，
  同列再点只翻转方向 —— 全部对照 `table.py:492-516`。
- **行数据**：后端**没有批量接口**，只能逐行 `browserRowForId`。页面把这件事交给
  `ResultRow.svelte`，由虚拟列表按需挂载 —— 与 Qt 的 `QAbstractTableModel.data()`
  惰性取数是同一个思路。注意该组件**必须跟着 `id` 重新取数**（虚拟列表按行号复用组件实例，
  见 §5.23）。
- 单元格文本已经是纯文本（`rslib/src/browser_table.rs` 内部就调 `html_to_text_line`），
  页面直接显示，不做二次转换。
- **`BrowserRow` 只有 `cells` / `color` / 字体**，没有 card id / note id。行 id 一直都在
  结果列表里（卡片模式是 cid、笔记模式是 nid）：卡片模式下要拿 note 得先 `getCard`
  反查（`currentNoteId()`），笔记模式下要拿卡片得先 `cardsOfNote`（`currentCardId()`）。
  行颜色用于推旗标 / 暂停 / 埋藏（`BrowserRow.Color` → `flag = color - FLAG_RED + 1`）；
  标记状态必须查笔记的标签（既标记又有旗标时颜色只显示其中一个）。
- **列顺序就是 `activeCols` 的顺序**：后端 `browserRowForId` 返回的 `cells` 按送进
  `setActiveBrowserColumns` 的列顺序排，所以「显示顺序」与「取数顺序」必须同源 ——
  拖列头改的就是 `activeCols`（改完重新 `setActiveBrowserColumns` 并让每行重新取数，
  靠 `ResultRow` 的 `version` 属性）。Qt 的做法不同（模型顺序不变，视图顺序记在表头
  状态里），但观察到的行为一致。列宽则按模式分开存（`{cards, notes}`，等价于 Qt 的
  两个 `saveHeader` 键）。
- **导出只在页面里做**：对话框（`ExportDialog.svelte`）→ 问宿主要保存路径
  （**外壳新增的端点** `saveFileDialog`，请求复用 `OpenFilePickerRequest`，其中
  `key` 按外壳约定传建议文件名）→ 页面直接调 `exportAnkiPackage` /
  `exportCollectionPackage` / `exportNoteCsv` / `exportCardCsv`。宿主只提供"选路径"这
  一个能力（网页自己弹不出原生保存框）。
  测试钩子：`ANKI_SAVE_PATH` 指向目录时直接返回 `<目录>/<建议文件名>`，绕过原生对话框
  （原生框没法自动点；与 `ANKI_*` 其它变量同一套路）。
  四种格式与勾选项的对应、以及「笔记纯文本 / 卡片纯文本」落成什么文件，见下面
  「导出（四种格式）」。
  另外**上游的 `exporting-note-exported` 在核心仓库里没有文本**（en/zh 都是空的），
  所以 apkg 导出后状态栏是空的 —— 与 Qt 一致，不是漏翻译。
- **「更改笔记类型」是"窗口感知的端点"**：页面是既有的动态路由
  `change-notetype/<oldId>[/<newId>]`（`+page.ts` 从路径取新旧 id），它保存时只发
  **字段/模板映射**，请求里**没有 `note_ids`**（页面不知道选了哪些笔记）；宿主开窗时
  记下选中的笔记，收到 `changeNotetype` 请求时补上，成功后再关窗 —— 对照
  `changenotetype.py:68-85` 的 `save()` 与 `mediasrv.py:772` 的窗口分派。所以这条
  RPC 由 `server/api.rs` 单独接管（它要 async 调后端 + 关窗），不落进通用分发。
  窗口是**应用级模态**；选中的笔记必须同一种类型，否则弹警告且不开窗
  （`changenotetype.py:88-93`）。
- **行高亮要用数组传，不能传 `selected={isSelected(id)}`**：Svelte 只追踪模板表达式里
  直接引用的响应式变量，函数体里用到的 `selected` 它看不到 —— 于是「只改 `selected`、
  不动其它状态」时（Ctrl+A 全选、反选）高亮不会刷新。传 `selectedIds={selected}`
  让表达式直接引用数组即可。这条是验证脚本逼出来的：状态栏计数对、`browserTitle`
  也对，只有 DOM 的高亮是旧的。
- 失败一律带 `alertOnError: false`：搜索语法写错、行取不到（卡片刚被删）都只体现在
  表格里，绝不弹 `alert()` 卡死页面（§5.18 的教训）。

### 批量动作的三条映射规则

卡片模式下表格里是**卡片**，而批量动作的粒度不一样，转换只能靠 RPC（后端没有批量映射接口）：

| 动作 | 需要的 id | 怎么拿 |
| --- | --- | --- |
| 加标签 / 删除 | note id | 卡片模式下逐张 `getCard` 取 `noteId` 再去重（`cards.proto` 只有 `GetCard`） |
| 改牌组 | card id | 笔记模式下逐条 `cardsOfNote` 展开 |
| 预览 | card id | 笔记模式下取 `cardsOfNote` 的第一张 |

动作之后统一重跑当前搜索，并保留仍然存在的选中项（对照 `table.py:520-543` 的
`_restore_selection`）。**删除没有确认框** —— 上游就没有（`browser.py:895-906` 直接删），
靠通用 undo 兜底，所以这里的「删除」也是直接删 + 状态栏提示 + `Ctrl+Z` 可撤销。

### 导出（四种格式）

对话框是页面自己的（`ExportDialog.svelte`，浏览器与牌组列表共用），背后的逻辑在
`exporting.ts`（格式映射 / `ExportLimit` / 问宿主要保存路径 / 调导出 RPC）。
格式与选项照 `qt/aqt/import_export/exporting.py` 的 `ExportDialog`
（`exporter_changed()` 按格式显隐选项，`exporting.py:98-111`）：

两个入口：

| 入口 | 上游 | 范围下拉 |
| --- | --- | --- |
| 浏览器「更多」→ 导出 | `browser.py:377` 的 `_on_export_notes`（带选中的 nids） | 选中的笔记（N）/ 全部集合 |
| 牌组列表的齿轮 → 导出 | `deckbrowser.py:317-327` → `main.py:1398-1401` 的 `onExport(did=…)` | **该牌组**（默认）/ 全部集合 |

| 格式 | 扩展名 | 后端 RPC | 选项 |
| --- | --- | --- | --- |
| Anki 牌组包 | `.apkg` | `exportAnkiPackage` | 范围（选中的笔记 / 全部）、排程、牌组预设、媒体、旧版 |
| Anki 集合包 | `.colpkg` | `exportCollectionPackage` | 媒体、旧版（没有范围：`show_deck_list = False`） |
| 笔记纯文本 | `.txt` | `exportNoteCsv` | 范围、HTML、标签、牌组名、笔记类型、GUID |
| 卡片纯文本 | `.txt` | `exportCardCsv` | 范围、HTML |

- **勾选初值**照 `forms/exporting.ui`（`includeHTML` 另由 `setup()` 置 true，
  `exporting.py:88`）：排程 / 媒体 / HTML / 标签 = 开，牌组预设 / 牌组名 / 笔记类型 /
  GUID / 旧版 = 关；每次打开对话框都重置。
- **两个纯文本格式后端写的是制表符分隔的 CSV**
  （`rslib/src/import_export/text/csv/export.rs`）：头两行固定
  `#separator:tab` / `#html:<true|false>`，笔记导出再按勾选项补
  `#guid/#notetype/#deck/#tags column:<N>`（列号后端算），没有单独的表头行。
  **没有另做"选列"对话框** —— 列由勾选项决定（上游 Qt 也是这样）。
- **`limit` 是双层 oneof**：`{ limit: { case: "noteIds"|"wholeCollection"|"deckId", value } }`。
  三个导出 RPC 共用 `ExportLimit`，两个入口统一走 `exporting.ts` 的
  `limitFor(choice, context)`（用生成的类构造，写错类型编译期就报）。少嵌一层**不报错**，
  而是静默导出整个集合（踩过，见 `TAURI_HANDOVER.md` §5.33）—— `decks-check.mjs`
  就是用「别的牌组的笔记不该出现在文件里」把这条守住的。
- 导出后的提示照 Qt：apkg / 笔记文本用 `exporting-note-exported(count)`、卡片文本用
  `exporting-card-exported(count)`、colpkg 用 `exporting-collection-exported`（无计数）。
- **差异**：上游的范围下拉列着**所有牌组**（可以在这里换一个牌组导出，默认文件名取它）；
  这里只有两个入口各自的两个选项（浏览器：选中的笔记 / 全部集合；牌组列表：该牌组 /
  全部集合）；不做"导出到 profile 目录就拒绝"的拦截（PARITY 5.9）；保存路径走宿主的
  `saveFileDialog`，浏览器入口的建议文件名是 `集合-<时间戳>.<ext>`、牌组入口用牌组名。

### 查找重复

对照 `qt/aqt/browser/find_duplicates.py` 的 `FindDuplicatesDialog` + `forms/finddupes.ui`
（入口是浏览器 Edit 菜单里的 `qt-accel-find-duplicates`，`browser.py:354`）。这里是浏览器
页面里的一个模态框（`FindDuplicatesModal.svelte`），内容与上游一一对应：

| 项 | 上游 | 本外壳 |
| --- | --- | --- |
| 「搜索位置」 | 所有笔记类型的字段名并集，大小写不同只留第一次出现的写法，按小写排序（`find.py:60-67` + `find_duplicates.py:45-47`） | 同左（每次打开都重新取，笔记类型刚改过也准） |
| 「可选筛选器」 | 可编辑下拉 + 历史记录（`findDupesFind`） | 普通输入框（**差**：没有历史） |
| 判重 | `col.find_dupes(field, search)`（`collection.py:744-778`） | 同左算法，见下 |
| 报告 | `browsing-found-as-across-bs(part=browsing-group, whole=browsing-note-count)` + 每组一条链接「N 条笔记: 值」 | 同左（每组一个按钮，点击行为一样） |
| 点一组 | `search_for(link)` + `onNote()`：浏览器换 `nid:…` 搜索并切到笔记模式 | 同左（页面直接换搜索 + 切笔记模式） |
| Tag Duplicates | 给所有组里的笔记加 `browsing-duplicate` 标签（`add_tags_to_notes`） | 同左（`addNoteTags`，请求里的 `tags` 是**一个空格分隔的字符串**，不是数组） |

`find_dupes()` 的复刻（三步，全部用现有 RPC）：

1. 搜出「该字段非空」的笔记 —— 用 `buildSearchString` 的 `field_name` 节点（等价于
   `字段:_*`），可选筛选器与它 AND（`buildSearchString` 的 `group` 节点）；
2. 逐条 `getNote` 取字段值（按字段名在该笔记类型里的下标），再过一遍
   `stripHtml(mode=PRESERVE_MEDIA_FILENAMES)` —— 与上游 `strip_html_media` 是同一个后端方法；
3. 空值不算重复；同一个值出现 ≥2 次就是一组，**组的顺序按"第二次出现"排**（照上游的
   append 时机）。

差异：Qt 是挂在浏览器上的**独立非模态对话框**（报告画在 QWebView 里），这里是页面内的
模态框；上游的搜索框带历史记录；**后端没有批量取字段的 RPC**，所以是逐条 `getNote`
（大集合会慢，上游直接读 SQL）。

验证：`browser-check.mjs` 第 8.8 节 —— 造两条同值笔记 → 开框选字段/筛选器 → 搜索 →
断言只有一组且计数 2、报告首行有「组数 / 笔记数」→ 点那一组 → 断言浏览器换成
`nid:…` 且切到笔记模式 → 再开一次点「Tag Duplicates」→ RPC 复查两条笔记都多了同一个
标签 → 删掉这两条笔记并切回卡片模式。

### 预览窗口

对照 `qt/aqt/browser/previewer.py`：独立顶层窗口，内容走 reviewer 的渲染路径
（`renderExistingCard(browser=false)` + `_showQuestion`/`_showAnswer` + `card.question()` 的
同款 CSS），按钮在页面底部（`<` / `>` / 重播音频），快捷键 `R` / `←` / `→` /
`Space`/`Enter` / `Esc`（`Ctrl+Shift+P` 也可关闭）。

- 同一个卡片再点一次「预览」= **关窗**（对照 `browser.py:853-865` 的 `onTogglePreview`）；
- 「仅背面」（勾选框 / `B`）：问题与答案一起显示，开关落库到 bool 配置
  `previewBothSides`（`previewer.py:107-115,278-293`）。它的文案是
  `qt-misc-back-side-only`，属于 `ftl/qt` 仓库 —— 类型化的 i18n 方法只覆盖核心
  字符串，所以这里走 `I18n::translate`（外壳为此把该方法放开为 `pub`，见交接文档
  §7.4）；
- `[[type:Field]]` 会被删掉而不是生成输入框（对照 `previewer.py:201-204` 的
  `type_ans_preview_filter`）；
- **上一张/下一张的清单在浏览器页面手里**：宿主持有不了完整结果集，所以
  `previewNext`/`previewPrev` 会绕一圈 —— 宿主 `eval` 浏览器的 `previewNeighbor(±1, cid)`，
  页面算出邻居再回一条 `preview:<cid>`（对照 `previewer.py:412-430`：Qt 的预览器也是问
  浏览器父窗口要前后卡片）；
- 关闭浏览器窗口时预览窗口一起关掉（`browser.py` 的 `_close_previewer`）。

### 与 Qt 的差异（有意为之）

| 项 | Qt | 这里 |
| --- | --- | --- |
| 编辑区 | 嵌在浏览器窗口内（`browser.py:608-630`） | **独立窗口** `browser-editor`：`/editor/` 是不可信页面（CSP `frame-ancestors 'none'`），iframe 会被 WebView 拒绝；关闭浏览器时连带关掉它 |
| 打开编辑器 | 选中行即载入编辑区 | **双击**一行（单击只选中，否则每点一下都抢焦点） |
| 菜单栏 | Notes / Cards / View / Jumps 四个菜单 | 最常用的四个平铺成工具栏按钮，其余在**行右键菜单** + 工具栏「更多」里（同一份动作表）；因此 `browsing-delete-notes` 等菜单文案没用上 |
| 立即评分 | 弹出「重来 / 困难 / 良好 / 简单 + 取消」对话框 | 菜单里四项（少一次点击，触发的是同一个 `gradeNow`） |
| 取消暂停 / 取消埋藏 | 两个独立入口（`unsuspend_cards` / `unbury_cards`） | 都走「恢复被暂停/被埋藏的卡片」这一个 RPC（后端只有它），因此取消暂停也会顺带恢复埋藏 |
| 查找替换 | 对话框带「所有字段 / 标签 / 某字段」下拉框与「只看选中的笔记」 | 固定作用于选中笔记的全部字段（两个下拉/勾选简化掉了） |
| 对话框确认键 | Qt 自带的标准按钮（"OK"，仓库里没有核心 ftl key） | 用最贴合的可用文案：重置/重排用动作名，到期日用 `actions-save`，查找替换用对话框标题 |
| 卡片信息窗口 | `CardInfoDialog`（单例，Qt 对话框） | 独立窗口，**复用已有的 `card-info/<cid>` 路由**；同窗口再看另一张卡时整页导航过去 |
| 更改笔记类型 | 保存前 `confirm_schema_modification()`（牵涉 schema 时再确认一次） | 跳过这一步（本外壳没有这条确认链路）；其余一致：宿主补 `note_ids`、成功即关窗 |
| 导出 | 五种格式（牌组包 / 整个集合 / 笔记纯文本 / 卡片 CSV / 带列的笔记 CSV）+ 逐个牌组导出；原生保存对话框；导出到 profile 目录时会拒绝 | 四种格式都在（apkg / colpkg / 笔记纯文本 / 卡片纯文本）；范围只有「选中的笔记 / 全部集合」（没有牌组下拉）；纯文本的列由勾选项决定，没有独立的"选列"对话框；保存路径问宿主的 `saveFileDialog`（默认文件名 `集合-<时间戳>.<ext>`）；不做"导出到 profile 目录"的拦截。详见「导出（四种格式）」 |
| 查找重复 | 挂在浏览器上的独立非模态 `QDialog`，报告画在 QWebView 里；搜索框带历史 | 页面内的模态框，结果是一个列表；搜索框没历史；逐条 `getNote` 取字段值（后端没有批量接口），大集合会慢 |
| 列清单 | 「视图」菜单里逐项勾选 | 表头右键弹层（同一份 `activeCols`，落库时机一致） |
| 列宽 / 列顺序 | `saveHeader` 存 PM profile 的**二进制表头状态**（列宽 + 视图顺序） | profile 配置 `tauriBrowserColumnWidths`（JSON，`{cards, notes}` 各存列 key → px）；顺序直接落在 `activeCols` 里 |
| 侧栏 | 7 个分区 + 折叠状态 + 修饰键组合搜索 | 「当前牌组 / 全部集合 / 牌组树（带计数）/ 笔记模板」，点击即替换搜索词 |
| 批量动作 | 删除 / 改牌组 / 加标签 / 移除标签 / 清除未使用标签 / 标记 / 暂停 / 埋藏 / 旗标 / 重置 / 设置到期日 / 重新定位 / 立即评分 / 改笔记类型 / 查找替换 / 导出… | 都在（行右键菜单 + 工具栏「更多」）；缺的是**查找重复**，见「尚未实现」 |
| 加标签对话框 | `TagEdit`（补全 + 最近标签） | 一个输入框（空格分隔），补全与最近标签未做 |
| 预览 | 按钮在原生 QDialogButtonBox | 按钮在页面里；前后卡片不做禁用（清单在浏览器手里） |
| 表头排序指示 | QHeaderView 的箭头 | 列头里的 ▲/▼ |
| 窗口标题 | `browsing-window-title(-notes)`（含已选/总数） | 同（页面报数、宿主本地化） |

### 已验证

`tauri/tools/verify-browser.ps1`（+ `browser-check.mjs`）：`browse` 开窗 → 断言页面报到且
首屏搜索出结果 → 截真实窗口 → Playwright 走完整条链路（侧栏 / 排序 / 多选 / 笔记模式 /
列清单 / 加标签 / 改牌组 / 删除 + 撤销 / **行右键菜单的 38 项动作**：标记 / 旗标 / 暂停 /
埋藏 / 重置 / 到期日 / 重排 / 立即评分 / 清除未使用标签 / 选中笔记 / 卡片信息 / 复制笔记 /
查找替换，以及 Home·Ctrl+N·End 跳转与反选全选）→ 四种格式的导出（apkg / colpkg / 两种
纯文本）→ 查找重复（判重 / 点组跳转 / 一键打标签）→ 驱动预览窗口翻面与前后卡片并截图 →
开「管理笔记类型」窗口 + 跑 `notetypes-check.mjs`（列表 / 两步添加 / 重命名 / 删除 /
两条对话框端点）→ 验 `searchInBrowser` 与 `browserSearch` 的复用路径 → 关闭浏览器断言
编辑区、预览与卡片信息窗口连带关掉。产物里有 `browser.png` / `preview.png` /
`card-info.png` / `notetypes.png`（真实窗口）与 `notetypes-page.png`（页面）截图。

> 检查脚本开跑时会先把「笔记模式」这类**残留配置**归位（上一次跑崩了会把
> `browserTableShowNotesMode` 留在开启状态，后面的断言与宿主侧的 `previewNeighbor`
> 都建立在卡片模式上）—— 排查预览翻页失败时先看这一条。

```
PASS  侧栏已渲染牌组 — 当前牌组 / 全部集合 / Default 2 + 0 + 0 / Basic / …
PASS  列头 = activeCols 指定的列 — 排序字段 | 卡片模板 | 到期 | 牌组
PASS  点列头后 sortType 落库 — cardDue
PASS  再点一次翻转方向 — true → false
PASS  Ctrl+点击加选 — browserTitle:0:2:3
PASS  Ctrl+A 全选 — browserTitle:0:3:3（共 3 行）
PASS  笔记模式改用 searchNotes — 1 次
PASS  勾选一列后立即写回 activeCols — ["noteFld","template","cardDue","deck","cardMod"]
PASS  拖动列头 → 首列挪到末尾 / 新顺序写回 activeCols
PASS  单元格跟着列顺序走（没和表头错位）/ 拖动不会被当成点列头排序
PASS  列宽：卡片与笔记各存一份、切模式互不影响、切回来能恢复
PASS  加标签：能按新标签搜到 — 1 条
PASS  改牌组：卡片真的换到新牌组 — 1 → 1791860914411
PASS  删除发出了 removeNotes — 200
PASS  Ctrl+Z 撤销后 note 回来了
PASS  右键菜单已打开 — 38 项 / 菜单项带了 Qt 的快捷键提示
PASS  菜单 → 卡片信息发出 cardInfo:<cid> / 菜单 → 复制笔记发出 addCopy
PASS  Ctrl+K 加 marked 标签 / 再按一次取消 / marked 标签已清掉
PASS  Ctrl+3 设置绿色旗标 — flag=3 / 同一旗标再按一次 = 清除
PASS  Ctrl+J 暂停 / 再次 Ctrl+J 取消暂停 / Ctrl+Shift+J 埋藏
PASS  重置弹框有 2 个勾选项 / 重置发出 scheduleCardsAsNew
PASS  设置到期日发出 setDueDate(days=2) / 重新定位发出 sortCards
PASS  菜单 → 立即评分（良好）
PASS  Home/Ctrl+N/End 选中对应行、Ctrl+Alt+A 全选、Ctrl+Alt+S 反选（高亮与计数都对）
PASS  菜单 → 选中笔记（换成 nid: 搜索）
PASS  查找替换发出 findAndReplace / 查找替换真的改了字段
PASS  菜单 → 更改笔记类型发出 changeNoteType / 页面已渲染 / 真的改了笔记类型
      （宿主补 note_ids → 后端执行 → 关窗；见 verify 脚本的「开窗 → 页面保存 → 宿主关窗」）
PASS  菜单 → 管理笔记类型发出 notetypes（窗口已打开）
PASS  列表渲染出全部笔记类型 / 每项是「名字 [笔记数]」
PASS  添加（原厂 Basic → RPC 复查 2 字段 / 1 模板）/ 克隆现有类型 / 重命名生效
PASS  「字段…」打了 openFieldsDialogForNotetype / 「卡片…」打了 openCardsDialogForNotetype
PASS  删除确认框问的是「没人用的类型」/ 已清理临时笔记类型
PASS  导出对话框默认 apkg + 选中的笔记 / 文案已本地化 / 切到 colpkg 后只剩「含媒体 / 旧版支持」
PASS  导出勾选初值照 exporting.ui（排程开 / 牌组预设关）
PASS  切到「笔记纯文本」后只剩文本相关的选项 / 勾选初值照 exporting.ui（HTML·标签开）
PASS  笔记纯文本导出产出了制表符 CSV — 集合-2026-10-15@12-13-27.txt（#separator:tab / #tags column:3）
PASS  切到「卡片纯文本」后只剩范围 + HTML
PASS  卡片纯文本导出产出了制表符 CSV — 集合-2026-10-15@12-13-31.txt
PASS  已清理导出的文本文件
PASS  「查找重复」列出了笔记类型里的字段 / 重复报告只有造出来的那一组（计数 2）
PASS  报告首行给了「组数 / 笔记数」— 在 2 条笔记 中找到 1 种组合。
PASS  点一组 → 浏览器换成 nid: 搜索 / 点一组 → 切到笔记模式
PASS  「Tag Duplicates」给组里的笔记都打了新标签 — 复制 | 复制
PASS  已清理查找重复的测试笔记 / 查找重复之后回到卡片模式
PASS  导出 apkg 产出了合法文件 — 集合-2026-10-13@15-48-12.apkg 56633 字节 PK
      （verify 脚本：导出 apkg → 集合-….apkg（55 KB））
PASS  双击行发出了 editCard — editCard:1791798151921
PASS  预览发出了 preview:<cid> — preview:1791798151921
PASS  已删除临时牌组 — TauriTemp40804
预览窗口已渲染问题面
预览：问题面 → 答案面
预览：答案面 → 下一张卡片
预览：下一张卡片 → 上一张卡片
预览：仅背面（问题+答案）
预览：回到只看问题
searchInBrowser：复用已打开的窗口并注入搜索
browserSearch：图表 → 浏览器搜索
关闭浏览器连带关掉 browser-editor 窗口
关闭浏览器连带关掉 preview 窗口
```

检查脚本会真的改集合（加/删标签、改牌组、删笔记、建临时牌组、给卡片打旗标 / 暂停 /
埋藏 / 重置 / 改到期日 / 立即评分 / 查找替换），但**每一步都会还原或清理**：动作类的
改动紧随一个 `Ctrl+Z`（或再按一次同样的开关切回去）、临时 note 与临时牌组删掉、
`sortType`/`activeCols` 等配置写回原值，最后断言「已删除临时牌组」与「配置已还原」。

> 改了 `ts/routes/browser/` 之后**必须重跑 `corepack yarn build`**：这个页面是 SvelteKit
> 产物（`out/sveltekit`），外壳只负责把它当 `browser` 路由发出去
> （`server/assets.rs` 的 `SVELTEKIT_ROUTES`）。它按**不可信页面**处理（多一份 script
> hash 校验），理由见该文件的注释。

## 验证工具

`tauri/tools/` 下的脚本用来确认改动没有破坏界面与配置：

| 脚本 | 作用 |
| --- | --- |
| `verify-reviewer.ps1` | 一键：启动外壳 → 必要时补一张卡片 → 截真实窗口 → 跑布局断言。改完复习界面先跑它 |
| `verify-m2.ps1` | 一键：验证 M2 的平台能力（剪贴板文件列表、TTS 合成、录音闭环）并检查多窗口的命令归属。**录音那项需要本机有输入设备**，没有时会以 `recordAudio → HTTP 500 找不到可用的录音设备` 失败（其余断言照常通过） |
| `m2-check.mjs` | 上面那个脚本调用的实际检查（HTTP 层端到端断言）。注意：那条会一直挂着的 `recordAudio` 必须马上挂 `.catch()`、且收尾**不要** `abort()` —— 否则 node 会以 unhandled rejection 自杀并带出 libuv 的 `UV_HANDLE_CLOSING` 断言，看着像被测程序崩了 |
| `verify-editor.ps1` | 一键：验证「编辑当前卡片」窗口（开窗 → 载入 note → 截图 → 关窗 → 保存链路 → 「字段…」/「卡片…」两个对话框） |
| `editor-check.mjs` | 编辑器「改字段 → 保存」的检查：Playwright 真实敲键盘，再用 RPC 复查集合内容 |
| `fields-check.mjs` | `verify-editor.ps1` 调用的「字段…」检查：加字段 → 保存 → RPC 复查 → 原始字节还原 |
| `cards-check.mjs` | `verify-editor.ps1` 调用的「卡片…」检查：改模板 → **未保存**的改动出现在预览里 → 加卡片类型 → 保存 → RPC 复查 → 原始字节还原 |
| `drop-check.mjs` | `verify-editor.ps1` 调用的「拖入文件」检查：页面里合成带 PNG 的 drop → 字段出现 `<img src="paste-…">` → 真落进 `collection.media` → 保存后复查 → 还原 |
| `import-check.mjs` | `verify-decks.ps1` 调用的导入检查：造 `.apkg` 夹具 → 删掉临时牌组/卡片 → 页面导入 → RPC 复查牌组与卡片进库 → `importDone` → 收尾 |
| `verify-addcards.ps1` | 一键：验证「添加卡片」窗口（`add` 开窗 → 截图 → 默认值 / 添加落库 / 两条关闭契约） |
| `addcards-check.mjs` | 上面那个脚本调用的检查：Playwright 填字段 + 点「添加」，拦请求验内容、用 `searchNotes` 验落库并清理 |
| `verify-browser.ps1` | 一键：验证浏览器窗口（`browse` 开窗 → 截图 → 侧栏/表格/双击开编辑器 → 管理笔记类型窗口 → `searchInBrowser` 复用路径 → 关闭连带） |
| `browser-check.mjs` | 上面那个脚本调用的检查：Playwright 点侧栏、双击表格行、导出四种格式（含读回产出的 CSV）、查找重复（判重 / 跳转 / 打标签），并观察页面发往宿主的桥接命令 |
| `notetypes-check.mjs` | `verify-browser.ps1` 调用的「管理笔记类型」检查：列表渲染 / 两步添加（原厂 + 克隆，RPC 复查字段数）/ 重命名 / 删除确认文案 / 「字段…」「卡片…」两条端点，收尾把临时类型删干净 |
| `verify-decks.ps1` | 一键：验证牌组列表（`ANKI_START_PAGE=decks` 启动 → **自动备份产物** → 截图 → 折叠/概览/齿轮菜单（含**导出**）/拖拽/新建删除 → `d`/`s`/`study` 切页） |
| `decks-check.mjs` | 上面那个脚本调用的检查：Playwright 真的点齿轮、在弹框里输入、导出笔记纯文本并读回文件断言牌组范围，并用 RPC 复查当前牌组/牌组是否真的建删 |
| `cleanup-test-decks.mjs` | 清掉检查脚本**中途失败**留下的临时牌组（`TauriTemp*` / `TauriDecks*` 等） |
| `occlusion-check.mjs` | `verify-editor.ps1` 调用的图像遮挡检查：遮挡按钮 → 图片选择器 → 宿主 `/openFilePicker`（`ANKI_OPEN_PATH`）→ 媒体拷贝 → 掩码编辑器渲染出图片 |
| `custom-study-check.mjs` | `verify-decks.ps1` 调用的自定义学习检查：六个选项 / cram 四选一 / 确定后建出筛选牌组并切成当前牌组（用 RPC 复查并还原） |
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
  `ease1`..`ease4`、`undo`、`statesMutated`、`add`、`edit`、`browse`、`browserSearch:`、
  `stats`、`preferences`、`decks`、`overview`、`study`、`opts:<did>`、`import`、`shared`、
  `close`）、编辑器的 `editorReady` / `saved` / `key:`…、浏览器窗口的
  `browserReady` / `browserTitle:` / `editCard:` / `editNote:` / `preview:<cid>`、
  预览窗口的 `previewNext` / `previewPrev` / `previewReplay`，以及录音窗口的
  `saveRecording` / `cancelRecording`。`updateToolbar`、`repaintNeeded`、
  工具栏动作等命令会记录日志并返回 `null`。
- **复习界面余项**：
  - 底部栏已实现「编辑 / 更多 + 显示答案 + 四档评分」，并接入**计时器**
    （`showQuestion(button, maxTime)`，`maxTime` 取自牌组的 `timer` / `maxTaken`）、
    按钮上的**下次间隔**（`describeNextStates`）与牌组的 `stopTimerOnAnswer`。
    评分按钮固定四个 —— v3 调度器下 Qt 也是四个（`Scheduler.answerButtons()`
    恒返回 4，2/3 个按钮是 v2 的遗留）。
  - ~~牌组列表~~ —— **已实现**（新写的 SvelteKit 页面，启动默认页）：牌组树与今日
    计数、折叠、点牌组 → 概览 → 学习、齿轮菜单（重命名 / 选项 / 导出 / 删除）、底部
    「获取共享牌组 / 创建牌组 / 导入文件」、`d`/`s` 快捷键与工具栏；概览面板还有描述
    （可编辑 + markdown）、选项、自定义学习 / 筛选牌组的重建·清空、取消埋藏，牌组行可
    拖拽改父级，见「牌组列表」。
    **剩下的**：复习界面的工具栏、概览计数的「埋藏差值」那一列、菜单栏里
    Switch Profile / 全屏缩放 / 学习牌组 / 创建筛选牌组 / 检查更新 / add-ons / About
    这几项（RPC 或窗口能力还不具备）。
  - ~~`edit`~~ —— **已实现**：打开「编辑当前卡片」窗口，见「编辑器」。
  - ~~添加卡片~~ —— **已实现**：裸 `a` → `pycmd("add")` → `editor/?mode=add` 窗口，
    见「编辑器」（Qt 的入口在主窗口工具栏 / 全局快捷键 `a`）。
  - ~~浏览器~~ —— **已实现**（新写的 SvelteKit 页面）：裸 `b` → `pycmd("browse")`、
    图表 `browserSearch:<query>` 与 `searchInBrowser` 端点三个入口都通了；搜索、排序、
    多选、笔记模式、列清单 / 列宽 / 列顺序拖动、加标签 / 移除标签 / 清除未使用标签 / 标记 / 旗标 /
    暂停 / 埋藏 / 重置 / 设置到期日 / 重新定位 / 立即评分 / 改牌组 / 删除 + 撤销重做、
    复制笔记、卡片信息、查找替换、跳转、预览（独立窗口）、导出（四种格式）、查找重复都在，
    见「浏览器」。**剩下的**只是 PARITY 里那些小差异（侧栏分区、加标签补全、列设置存储、
    导出到 profile 目录的拦截），没有"整块没做"的功能了。
  - ~~`more` 命令~~ —— **已实现**（复习界面）：见「复习界面」的「更多」菜单；编辑器里的 `more` 仍未做。
  - ~~卡片布局~~ —— **已实现**：编辑器工具栏的「卡片…」（`openCardsDialog`），见「编辑器」。
  - ~~导出 CSV / 纯文本~~ —— **已实现**：`ExportDialog.svelte` 的「笔记纯文本 / 卡片纯文本」
    （`.txt`，后端 `exportNoteCsv` / `exportCardCsv`），见「导出（四种格式）」。
  - ~~查找重复~~ —— **已实现**：浏览器「更多」菜单 → `FindDuplicatesModal.svelte`
    （`find_dupes()` 的三步复刻：字段非空搜索 → 逐条取字段值 → 分组），见「查找重复」。
  - ~~管理笔记类型~~ —— **已实现**：新写的 `ts/routes/notetypes/`（列表 + 添加/重命名/删除/克隆
    + 「字段…」「卡片…」两个按钮），入口在浏览器窗口的「更多」菜单，见「管理笔记类型」对话框。
  - 音频的 `current_caller` 区分（editor / reviewer 的打断语义），属 M5 的音频质量项。
- **M2 已完成**：mediasrv 的 32 个平台端点全部有实现，详见上文各节。
  唯一没验证的是 **Android 上的录音**：这里统一用 `cpal`（Android 走 oboe），
  而 Qt/AnkiDroid 用的是 `MediaRecorder`，需要在真机确认。
- ~~拖入文件（HTML5 drop）~~ —— **已实现**：编辑器窗口见「编辑器」的「拖入文件（HTML5
  drop）」，主窗口拖入 `.apkg` 导入见「导入」；剩下的是「操作系统 → WebView2」那一步的
  真机确认与 `.apkg` 双击打开（PARITY 6.7）。
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
