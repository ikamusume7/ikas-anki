# Anki → Tauri v2 迁移计划

| 项目 | 内容 |
| --- | --- |
| 目标 | 用一套代码同时产出 Windows 与 Android 客户端 |
| 技术栈 | Tauri v2 + 现有 Svelte 前端（`ts/`）+ 现有 Rust 核心（`rslib/`） |
| 状态 | 执行中（M0 / M1 完成，M2 主体完成 —— 详见 `TAURI_HANDOVER.md`） |
| 日期 | 2026-10-12 |
| **交接文档** | **`TAURI_HANDOVER.md`** —— 接手本任务前请先读它 |

---

## 1. 概述

本计划描述如何把 Anki 桌面端迁移到 **Tauri v2**，使同一份代码库能够同时构建 **Windows** 与 **Android** 客户端。

核心思路是：**复用 `rslib`（Rust 核心）与 `ts`（Svelte 前端），只替换「外壳」与「本地服务层」**。

- **保留**：`rslib/`（全部业务逻辑）、`proto/`（接口定义）、`ts/`（250 个 Svelte 组件）、`ftl/`（翻译）
- **重写**：`qt/aqt/mediasrv.py`（本地 HTTP 服务）、`pylib/`（Python 胶水层）、`qt/aqt/`（PyQt GUI）
- **移除**：Python 插件系统（已确认可以去掉）

---

## 2. 目标与范围

### 2.1 目标

1. 一套 `tauri/` 代码，`tauri build` 出 Windows 安装包，`tauri android build` 出 APK/AAB。
2. 前端 `ts/` **零改动**即可运行。
3. `rslib` 业务逻辑**零改动**（仅新增交叉编译配置）。
4. Windows 与 Android 功能对等（复习、编辑、导入导出、同步、媒体）。

### 2.2 范围内

- Tauri 应用骨架与窗口管理
- Rust 侧本地 HTTP 服务（复刻 mediasrv）
- 平台能力实现（文件选择、音频、剪贴板、对话框等）
- 双端打包与签名流水线

### 2.3 范围外

- Python 插件系统兼容（已确认放弃）
- iOS / Linux / macOS（本计划只覆盖 Win + Android，但架构不阻碍后续扩展）
- 卡片渲染引擎重写（沿用 `rslib/src/card_rendering`）

---

## 3. 现状分析

### 3.1 现有分层

| 层 | 位置 | 技术 |
| --- | --- | --- |
| Web 前端 | `ts/` | Svelte 5 + SvelteKit（静态导出） |
| 桌面 GUI | `qt/aqt/` | PyQt6 |
| 本地 HTTP 服务 | `qt/aqt/mediasrv.py` | **Flask + waitress** |
| Python 胶水 | `pylib/anki/` | Python |
| PyO3 桥 | `pylib/rsbridge/` | Rust（97 行） |
| Rust 核心 | `rslib/` | Rust |
| 接口定义 | `proto/anki/` | Protobuf |

### 3.2 决定架构的六个事实

**事实 1：Rust 核心的对外接口极简。**

`pylib/rsbridge/lib.rs` 全部内容不到 100 行，实质只有两个入口：

```text
open_backend(init_msg: bytes) -> Backend
Backend::run_service_method(service: u32, method: u32, input: bytes) -> Result<bytes>
```

`rslib/src/services.rs` 通过 `include!(concat!(env!("OUT_DIR"), "/backend.rs"))` 引入由 `build.rs` 自动生成的服务分发代码（生成逻辑见 `rslib/rust_interface.rs`）。

> **结论**：Tauri 的 Rust 侧可以直接 `use anki::backend::Backend`，**无需任何 FFI 层**。

**事实 2：前端与后端通过 HTTP + protobuf 通信。**

API 调用的唯一入口是生成文件 `ts/lib/generated/post.ts`：

```ts
const inputBytes = input.toBinary();
const path = `/_anki/${method}`;
const outputBytes = await postProtoInner(path, inputBytes, opChangesType, signal);
```

> **结论**：只要本地服务实现了 `POST /_anki/{method}`，前端 API 层无需改动。

**事实 3：前端是纯静态 SPA。**

`ts/svelte.config.js` 使用 `@sveltejs/adapter-static`，产物为 `out/sveltekit`（含 `index.html` fallback）。

> **结论**：产物可被任何 WebView 加载。

**事实 4：mediasrv 是 Python/Flask，且职责高度混合。**

见 3.3 节。

**事实 5：Rust 侧已有的同步服务与 GUI 无关。**

`rslib/src/sync/http_server/mod.rs` 只提供 `/sync`、`/msync`、`/health`，可原样复用。

**事实 6：rslib 已具备移动端适配基础。**

```rust
// rslib/src/storage/sqlite.rs
// Android has no /tmp folder, and fails in the default config.
#[cfg(target_os = "android")]
db.pragma_update(None, "temp_store", &"memory")?;
```

```rust
// rslib/src/collection/backup.rs
// created time unsupported on Android
#[cfg(target_os = "android")]
{ meta.modified().ok() }
```

且 `rslib/src/ankidroid/` 目录说明 AnkiDroid（Kotlin/Android）已直接消费该核心。

### 3.3 mediasrv 职责分解

`qt/aqt/mediasrv.py`（约 1386 行）同时承担五类职责：

| # | 职责 | 说明 |
| --- | --- | --- |
| 1 | 静态资源 | `/_anki/sveltekit/**`、`/_app/**`、`css/`、`js/`、`js/vendor/`、字体 |
| 2 | 后端 API 代理 | `POST /_anki/{method}` 转发到 Rust |
| 3 | 媒体文件 | 集合 `media.dir()` 下的图片/音频/视频 |
| 4 | 平台交互 | 32 个端点（见 7.1） |
| 5 | 安全策略 | CSP 头、Origin/Host 校验、API Key |

其中职责 1~3、5 可在 Rust 中一比一复刻；职责 4 必须在每个平台上重新实现。

---

## 4. 架构决策

### 4.1 候选方案对比

| 方案 | 前端改动 | rslib 复用 | Android | 说明 |
| --- | --- | --- | --- | --- |
| **A. 本地 HTTP server（选定）** | 零 | 直接 | 可行（AnkiDroid 已验证） | 在 Tauri 进程内用 axum 复刻 mediasrv |
| B. Tauri IPC + asset protocol | 中（需改路径与 post.ts） | 直接 | 可行 | 更「Tauri 原生」，但需改动前端资源路径 |
| C. 保留 Python mediasrv | 零 | 需 PyO3 | **不可行** | Android 无法部署 CPython + Flask |

### 4.2 选定方案：A

**理由：**

1. 前端所有资源使用 `/_anki/...` 绝对路径，保留 HTTP 层可**完全避免前端改动**。
2. 静态资源、API、媒体文件**同源**，CSP 与安全模型可原样保留。
3. Android 上该模式已被 AnkiDroid 验证（WebView 加载 `http://127.0.0.1:<port>`）。
4. 后续若需切换 IPC，只需改 `ts/lib/generated/post.ts` 的生成模板即可平滑演进（M5）。

**代价：** Android 上需要 `INTERNET` 权限并允许 cleartext 访问 localhost。

---

## 5. 目标架构

```text
┌──────────────── Tauri v2 App（Win / Android 同一套代码）──────────────────┐
│                                                                            │
│  WebView（Windows: WebView2 ｜ Android: System WebView）                    │
│     加载 http://127.0.0.1:<port>/_anki/pages/<page>.html                    │
│         │                                                                  │
│         │ HTTP + protobuf（与现状完全一致）                                  │
│         ▼                                                                  │
│  Rust 侧（Tauri 后端进程）                                                  │
│   ├─ axum router        ← 复刻 mediasrv 的 5 类路由                          │
│   ├─ anki crate         ← 直接调用 Backend::run_service_method               │
│   ├─ Tauri commands     ← 平台能力（文件/音频/剪贴板/对话框）                  │
│   └─ sync http_server   ← 复用 rslib/src/sync/http_server                    │
└────────────────────────────────────────────────────────────────────────────┘
```

### 5.1 目录结构建议

```text
tauri/
├─ Cargo.toml              # 依赖 anki = { path = "../rslib" }
├─ tauri.conf.json         # 窗口、bundle、Android 配置
├─ src/
│  ├─ main.rs              # 启动流程：init_backend → 起 server → 开窗口
│  ├─ server/
│  │  ├─ mod.rs            # axum router 组装
│  │  ├─ api.rs            # POST /_anki/{method}
│  │  ├─ assets.rs         # 静态资源 + SvelteKit fallback
│  │  ├─ media.rs          # 媒体文件
│  │  ├─ csp.rs            # CSP 头（迁移 mediasrv.py 常量）
│  │  └─ platform.rs       # 32 个平台交互端点
│  ├─ backend.rs           # Backend 封装（含 spawn_blocking）
│  └─ commands.rs          # Tauri IPC 命令
├─ src-tauri/gen/android/  # tauri android init 生成
└─ frontend/               # 指向 out/sveltekit 的构建产物
```

### 5.2 关键代码骨架

**依赖（`tauri/Cargo.toml`）**

```toml
[dependencies]
anki = { path = "../rslib" }
anki_proto = { path = "../rslib/proto" }
axum.workspace = true
tokio.workspace = true
prost.workspace = true
tauri = { version = "2", features = [] }
```

**后端封装（`tauri/src/backend.rs`）**

```rust
use anki::backend::{init_backend, Backend};
use std::sync::Arc;

pub struct AppBackend(Arc<Backend>);

impl AppBackend {
    pub fn new(preferred_langs: Vec<String>, server: bool) -> anyhow::Result<Self> {
        let init = anki_proto::backend::BackendInit { preferred_langs, server };
        let msg = prost::Message::encode_to_vec(&init);
        let backend = init_backend(&msg).map_err(anyhow::Error::msg)?;
        Ok(Self(Arc::new(backend)))
    }

    /// run_service_method 是阻塞调用，必须放在阻塞线程池中执行。
    pub async fn run(&self, service: u32, method: u32, input: Vec<u8>) -> Result<Vec<u8>, Vec<u8>> {
        let backend = self.0.clone();
        tokio::task::spawn_blocking(move || backend.run_service_method(service, method, &input))
            .await
            .expect("blocking task panicked")
    }
}
```

**API 路由（`tauri/src/server/api.rs`）**

```rust
async fn handle_rpc(
    State(app): State<AppState>,
    Path(method): Path<String>,
    body: Bytes,
) -> Response {
    let Some((service, idx)) = app.method_table.lookup(&method) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match app.backend.run(service, idx, body.to_vec()).await {
        Ok(out) => ([(header::CONTENT_TYPE, "application/binary")], out).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err).into_response(),
    }
}
```

> `method_table` 需要从 `proto` 的 service/method 索引构建，与 `rslib/rust_interface.rs` 中生成的索引保持一致（同一份 proto 描述符，可通过 `anki_proto_gen::get_services` 复用）。

**启动流程（`tauri/src/main.rs`）**

```rust
fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let backend = AppBackend::new(langs(), true)?;
            // 1. 先起本地服务，取得端口
            let port = tauri::async_runtime::block_on(start_server(backend))?;
            // 2. 再创建加载该端口的窗口
            let url = format!("http://127.0.0.1:{port}/_anki/pages/");
            create_main_window(app, &url)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**前端 API 层（M5 可选优化，`ts/lib/generated/post.ts` 生成模板）**

```ts
// 从 fetch 切换为 Tauri invoke
import { invoke } from "@tauri-apps/api/core";

const inputBytes = input.toBinary();
const outputBytes = await invoke<Uint8Array>("rpc", { method, inputBytes });
return outputType.fromBinary(outputBytes);
```

---

## 6. 里程碑

### M0 — 骨架打通（1~2 周）· 已完成

> 实现见 `tauri/`，验证记录见 `tauri/README.md`。

**目标**：Windows 上启动 Tauri 窗口，从 Rust 侧打开真实集合并渲染页面。

| # | 任务 | 产物 |
| --- | --- | --- |
| 1 | 创建 `tauri/` 工程，依赖 `anki` crate | `Cargo.toml`、`main.rs` |
| 2 | 实现 `POST /_anki/{method}` → `run_service_method` | `server/api.rs` |
| 3 | 实现静态资源路由（SvelteKit + `qt/aqt/data/web`） | `server/assets.rs` |
| 4 | 实现媒体文件路由 | `server/media.rs` |
| 5 | 窗口先起 server 后加载 `http://127.0.0.1:<port>/_anki/pages/` | `main.rs` |

**退出标准**：Windows 上打开 `.anki2`，WebView 中看到牌组列表。

**风险**：`run_service_method` 是阻塞调用（`rsbridge` 中使用 `py.detach` 释放 GIL），必须 `spawn_blocking`。

---

### M1 — 只读复习链路（2~4 周）· Rust 侧已完成

> 已完成：集合打开、静态资源与媒体文件服务、CSP 分级、**legacy 页面机制**、
> **最小复习会话**（取队列 → 渲染 → 注入 → 评分 → 下一张，已端到端验证）。
> 待办：计时与时间盒、撤销、音频队列、类型作答、界面状态同步。

**目标**：能完成一次复习（显示卡片 + 评分），不支持编辑。

| # | 任务 |
| --- | --- |
| 1 | 按 `mediasrv.py` 的 `exposed_backend_list` 补齐 API 白名单 |
| 2 | 卡片渲染接入 `rslib::card_rendering`，媒体经媒体路由加载 |
| 3 | 迁移 CSP 逻辑（见 7.2） |
| 4 | 实现复习页专用白名单端点：`getSchedulingStatesWithContext`、`setSchedulingStates`、`i18nResources`、`congratsInfo` |
| 5 | 音频播放最小实现（见 M2） |

**退出标准**：Windows 上完整复习一张卡片（front/back/四个评分按钮），音频可播。

---

### M2 — 平台能力补齐（2~3 周）· 主体完成

> 已完成：剪贴板、文件选择、图片转换、原生对话框、profile/meta 配置存储、
> 音频播放（见 `tauri/src/platform/`）；窗口管理类端点显式降级为无副作用。
> 待办：录音；窗口模型（多 WebView 或路由接管）；复习状态类端点。
>
> **计划外补充**（原计划未预见，已一并完成）：
> - **`bridgeCommand` / `pycmd` 通道**：前端交互的结构性依赖。Qt 通过
>   QWebChannel 注入同名全局函数，Tauri 侧改为「注入脚本 + HTTP 端点」。
>   通道已打通，但命令处理器目前仅覆盖 `domDone` / `close`。
> - **安全策略**：API 密钥、Host / Origin 校验、CSP 分级。
> - **路径解析**：原先依赖编译期路径，现改为开发期用源码树、打包后用
>   Tauri 资源目录。

**目标**：实现 mediasrv 的全部 32 个平台交互端点（清单见 7.1）。

**退出标准**：编辑器中可插入图片（文件选择）、录音、播放、粘贴剪贴板内容；对话框正常。

---

### M3 — 编辑器与导入导出（3~4 周）

| # | 任务 |
| --- | --- |
| 1 | 富文本编辑器（`ts/routes/editor/`，89 文件）依赖的剪贴板/图片/录音全部打通 |
| 2 | 导入导出：`.apkg` / `.colpkg` / CSV（逻辑在 rslib，文件访问需适配各端） |
| 3 | 图像遮挡（`ts/routes/image-occlusion/`，54 文件） |
| 4 | Android 上文件选择改走 SAF，处理 `content://` URI |

**退出标准**：双端可导入 `.apkg`、导出 `.colpkg`、编辑并保存笔记。

---

### M4 — 双端打包（2~3 周）

见第 8 节。

**退出标准**：同一份 `tauri/` 代码产出 Windows 安装包与 Android APK，两端可打开同一集合。

---

### M5 — 打磨（持续）

- API 从 HTTP 迁移到 Tauri `invoke`（仅需改 `post.ts` 生成模板）
- 音频/视频质量对齐（变速、自动播放队列、后台播放）
- 同步、媒体检查、统计图表回归

---

## 7. 关键工作项清单

### 7.1 平台交互端点（必须逐项实现）

来源：`qt/aqt/mediasrv.py` 的 `post_handler_list`。

| 端点 | Windows | Android |
| --- | --- | --- |
| `openFilePicker` | `tauri-plugin-dialog` | SAF / dialog 插件 |
| `recordAudio` | 自研或第三方 | `MediaRecorder` |
| `playFile` | `rodio` 或系统 API | `MediaPlayer` / ExoPlayer |
| `readClipboard` / `writeClipboard` | `tauri-plugin-clipboard-manager` | 同左 |
| `askUser` / `showMessageBox` | `tauri-plugin-dialog` | 同左 |
| `openLink` | `tauri-plugin-opener` | Intent |
| `openMedia` / `showInMediaFolder` | opener + 定位文件 | Intent / Share |
| `convertPastedImage` | `image` crate | 同左 |
| `openFieldsDialog` / `openCardsDialog` | 由前端路由接管 | 同左 |
| `closeAddCards` / `closeEditCurrent` | 由窗口管理接管 | 同左 |
| `saveCustomColours` | 自定义 | 自定义 |
| `congratsInfo` | 由前端状态接管 | 同左 |
| `getDeckConfigsForUpdate` / `updateDeckConfigs` | 直接调 Rust | 直接调 Rust |
| `getSchedulingStatesWithContext` / `setSchedulingStates` | 直接调 Rust | 直接调 Rust |
| `changeNotetype` / `importDone` / `importDialogRequireClose` / `searchInBrowser` / `deckOptionsRequireClose` / `deckOptionsReady` | 由窗口/路由管理接管 | 同左 |
| `getProfileConfigJson` / `setProfileConfigJson` / `getMetaJson` / `setMetaJson` / `getConfigJson` / `setConfigJson` | 迁移到 Rust 侧配置存储 | 同左 |

> 建议：M0~M1 阶段先用「最小可用实现」打通链路，M2 再逐项完善质量。

### 7.2 安全策略（必须原样迁移）

`mediasrv.py` 中的以下常量与校验逻辑必须迁移到 Rust：

| 项 | 位置 | 说明 |
| --- | --- | --- |
| `UNTRUSTED_MEDIA_CSP` | 第 75 行 | 用户媒体文件的 CSP（禁止脚本） |
| `TRUSTED_PAGE_CSP` | 第 104 行 | 可信页面 CSP |
| `_untrusted_sveltekit_content_security_policy` | 第 148 行 | SvelteKit 页面的 CSP（含 script hash） |
| `_sveltekit_render_script_hash` / `_strip_csp_meta` | 第 131 / 141 行 | 从 HTML 提取/剔除 CSP meta |
| `ensure_safe_path` | 第 347 行 | 目录穿越防护 |
| `is_localhost_origin` / Host 校验 | 第 365 / 494 行 | 拒绝非本地来源 |
| `_have_api_access`（Bearer API Key） | 第 1372 行 | 动态请求鉴权 |
| `get_sveltekit_route` / `is_untrusted_sveltekit_route` | 第 524 / 543 行 | 路由信任级别判定 |

> 遗漏任何一项都会导致编辑器或图像遮挡页面被浏览器拒绝，或引入安全漏洞。

### 7.3 路径与存储

- `rslib` **不负责**决定 AppData 目录，路径由上层通过 proto 传入（现状由 `pylib` 决定）。
- Windows：使用系统 Application Data 目录。
- Android：使用 `getFilesDir()` / `getExternalFilesDir()`，由 Tauri 侧取值后传入 Rust。

### 7.4 rslib 交叉编译

```toml
# .cargo/config.toml（Android）
[target.aarch64-linux-android]
linker = "<NDK>/toolchains/llvm/prebuilt/<host>/bin/aarch64-linux-android24-clang"
```

- `rusqlite` 的 `bundled` 特性会编译 C 版 SQLite，需要 NDK clang。
- `reqwest` 在移动端统一启用 `rustls` 特性（仓库已有 `rustls` / `native-tls` 两个 feature 开关）。
- `rslib/process` 在 Android 不可用（无法 spawn 子进程），依赖它的功能（如部分 LaTeX 流程）需禁用或替换。

---

## 8. 双端打包

### 8.1 Windows

- `tauri build` 产出 `.msi` / `.exe`
- **现有 Briefcase 流程（`qt/tools/build_installer.py` + `qt/installer/windows-template`）整体废弃**，需新写流水线
- 签名复用现有 Azure Trusted Signing 配置，但签名对象由 `Anki.exe` 改为 Tauri 产物
- 需更新 `.github/workflows/release.yml` 中的 `build-and-sign-windows` job

### 8.2 Android

环境：JDK 17 + Android SDK + NDK

```bash
tauri android init
tauri android dev      # 调试
tauri android build    # 产出 APK/AAB
```

必须处理的清单：

1. `AndroidManifest.xml` 添加 `android.permission.INTERNET`（本地 HTTP 服务需要）
2. 允许 cleartext：`android:usesCleartextTraffic="true"` 或 network security config（否则 `http://127.0.0.1` 被拦截）
3. `minSdkVersion` ≥ 24
4. `abiFilters`：至少 `arm64-v8a`，可选 `x86_64` 供模拟器
5. 数据库与媒体目录改用 `getFilesDir()`
6. 文件导入导出走 SAF，处理 `content://` URI
7. 音频播放改用 Android 原生（`qt/aqt/sound.py` 与 `mpv.py` 完全不可用）

---

## 9. 明确不需要做的事

- 不修改 `rslib`（除交叉编译配置与少量平台分支）
- 不修改 `proto`（TS 代码生成已存在）
- 不重写 `ts/` 的 250 个 Svelte 组件
- 不做 FFI 层（Tauri 的 Rust 侧直接依赖 crate）
- 不兼容 Python 插件系统

---

## 10. 风险与应对

| # | 风险 | 影响 | 应对 |
| --- | --- | --- | --- |
| 1 | Android 系统 WebView 版本碎片化 | SvelteKit 产物可能无法运行 | 实测最低目标版本；必要时调整 `vite.config.ts` 的 `build.target` |
| 2 | Android 音频/视频需完全重写 | 工作量被低估 | M0 就做音频 PoC，不要拖到 M2 |
| 3 | rslib 的 Android 交叉编译首次配置 | 阻塞进度 | 参考 AnkiDroid 现有配置；提前验证 `rusqlite` bundled |
| 4 | `run_service_method` 阻塞主线程 | UI 卡顿 | 一律 `spawn_blocking` |
| 5 | CSP 迁移遗漏 | 页面被浏览器拒绝 | 按 7.2 清单逐项核对，配合现有 e2e 测试 |
| 6 | 本地 HTTP server 在 Android 上的后台限制 | 同步/长任务中断 | 同步走前台服务或由 rslib 的 tokio 直接发起 |
| 7 | 打包流水线需从零重写 | 发布延迟 | M4 前先手动产出可安装包验证 |

---

## 11. 验收标准

| 里程碑 | 验收方式 |
| --- | --- |
| M0 | Windows 上打开集合，WebView 显示牌组列表 |
| M1 | Windows 上完成一次完整复习，音频可播 |
| M2 | 双端均可选文件、录音、播放、复制粘贴、弹对话框 |
| M3 | 双端均可导入 `.apkg`、编辑笔记并保存 |
| M4 | 同一 commit 产出 Windows 安装包与 Android APK，均能打开同一集合 |
| M5 | 复习、编辑、同步、统计在双端无功能缺失 |

建议在 M1 起引入自动化回归：复用现有 `ts/tests/e2e/` 的 Playwright 用例思路，改为驱动 Tauri 的 WebView。

---

## 12. 附录

### 12.1 现状关键文件索引

| 用途 | 路径 |
| --- | --- |
| Rust 核心 | `rslib/src/` |
| 服务分发生成 | `rslib/rust_interface.rs`、`rslib/src/services.rs` |
| PyO3 桥（FFI 参考） | `pylib/rsbridge/lib.rs` |
| 本地 HTTP 服务（需复刻） | `qt/aqt/mediasrv.py` |
| 同步服务（可复用） | `rslib/src/sync/http_server/` |
| 前端 API 入口 | `ts/lib/generated/post.ts` |
| 前端构建配置 | `ts/svelte.config.js`、`ts/vite.config.ts` |
| 接口定义 | `proto/anki/*.proto` |
| 构建系统 | `build/configure/src/`、`justfile` |

### 12.2 参考先例

- **AnkiDroid**：Kotlin + WebView，直接消费 `rslib`（见 `rslib/src/ankidroid/`），验证了「Rust 核心 + 多壳」模式与 `127.0.0.1` 本地服务模式在 Android 上的可行性。

### 12.3 第一步行动项

**立即执行 M0 的最小验证**：用 axum 实现 `POST /_anki/{method}` 与 SvelteKit 静态资源路由，在 Tauri 窗口中加载 `deck-options` 页面（依赖最少）。

该验证可确认核心假设：**Tauri Rust 侧直连 `rslib` + 前端零改动**。
