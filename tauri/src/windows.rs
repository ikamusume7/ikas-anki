// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 窗口模型：**一个窗口一个 `WebviewWindow`**（多窗口），对齐 Qt 的 `DialogManager`。
//!
//! ## 为什么是多窗口，而不是「单窗口 + 前端路由接管」
//!
//! 1. Qt 里这些窗口本来就是**独立顶层窗口**（浏览器/添加卡片/编辑当前是
//!    `QMainWindow(None, Window)`，牌组选项/导入是 `QDialog(mw, Window)`），
//!    而且牌组选项与导入是 `ApplicationModal`。标题栏、任务栏项、窗口几何记忆、
//!    模态都是用户可感知的语义，单窗口 + 路由要自己重写一遍窗口管理器。
//! 2. 牌组选项 / 导入 / 首选项在新版 Anki 里已经是**独立的 SvelteKit 路由**
//!    （`ts/routes/`），`WebviewWindowBuilder` 直接加载该 URL 即可，前端零改动；
//!    单窗口 + 路由反而要把 legacy 复习页与 SvelteKit 页面塞进同一个文档。
//! 3. 主窗口当前就是「一个 URL」（legacy 复习页或某个 SvelteKit 路由），扩展成
//!    多窗口是增量改动，不需要动已有的注入与安全策略。
//!
//! ## 需要澄清：「窗口模型」只解锁其中一半
//!
//! 那 10 个降级端点里，只有 4 个是被窗口模型挡住的（`deckOptionsReady`、
//! `deckOptionsRequireClose`、`importDone`、`importDialogRequireClose`）——
//! 它们服务的页面（`ts/routes/deck-options`、`ts/routes/import-*`）**已经存在**，
//! 只是没有人能打开它们。
//!
//! 其余 6 个**不是窗口模型的问题**：浏览器 / 添加卡片 / 编辑当前 / 字段 / 卡片布局
//! 在当代 Anki 里是**原生 Qt 界面**，仓库里没有对应的 Web 页面（`ts/routes/` 只有
//! card-info / change-notetype / congrats / deck-options / editor / graphs /
//! image-occlusion / import-* / preferences）。它们要么需要新的 Svelte 界面（M3），
//! 要么需要能暴露自定义调色板的原生取色器（`saveCustomColours`）。
//!
//! ## 语义对齐
//!
//! - **单实例**：窗口以标签唯一标识，已存在时只聚焦 —— 对照 `DialogManager`
//!   「ensures that only one copy of the window is open at once」。
//! - **模态**：Tauri 没有 `setModal`，用「禁用主窗口」模拟 `ApplicationModal`，
//!   窗口销毁时恢复（否则主窗口会永久禁用，属于必须兜底的失败模式）。
//! - **几何记忆**：对照 Qt 的 `saveGeom` / `restoreGeom` 存在 profile 配置里。
//!   Qt 存的是 `QWidget.saveGeometry()` 的二进制块，两边格式不可能互通，
//!   因此用**自己的键名**存 JSON，互不干扰。

use std::collections::HashSet;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use serde_json::json;
use tauri::Manager as _;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;
use tauri::WindowEvent;

use crate::bridge;
use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
use crate::state::AppState;

/// 主窗口标签。
pub const MAIN: &str = "main";
/// 编辑当前卡片（Qt 的 `NewEditCurrent`）。
pub const EDIT_CURRENT: &str = "edit-current";
/// 添加卡片（Qt 的 `NewAddCards`）。
pub const ADD_CARDS: &str = "add-cards";
/// 牌组选项（Qt 的 `DeckOptionsDialog`）。
pub const DECK_OPTIONS: &str = "deck-options";
/// 导入（Qt 的 `ImportDialog`）。
pub const IMPORT: &str = "import";
/// 录音对话框（Qt 的 `RecordDialog`）。
pub const RECORDER: &str = "recorder";

/// 窗口几何的存储键前缀。刻意与 Qt 的键名（`{key}Geometry6.8`）区分开：
/// Qt 存的是二进制 Qt 几何块，我们存 JSON，两边不能互相解析。
const GEOMETRY_PREFIX: &str = "tauriWindowGeometry:";

/// 打开一个窗口所需的参数。
pub struct WindowSpec {
    /// Tauri 窗口标签；同名窗口已存在时只聚焦
    pub label: String,
    pub title: String,
    /// 相对根路径，例如 `deck-options/1`（SvelteKit 路由）或
    /// `_anki/legacyPageData?id=2`（宿主自建的 legacy 页面）
    pub route: String,
    pub width: f64,
    pub height: f64,
    /// 打开期间禁用主窗口，模拟 Qt 的 `ApplicationModal`
    pub modal: bool,
    /// 是否始终置顶（录音对话框需要，否则会被编辑窗口盖住）
    pub always_on_top: bool,
}

impl WindowSpec {
    /// 用默认尺寸构造。标题默认取标签（产品内入口落地后应传入本地化标题）。
    pub fn new(label: impl Into<String>, route: impl Into<String>) -> Self {
        let label = label.into();
        Self {
            title: label.replace('-', " "),
            label,
            route: route.into(),
            width: 1000.0,
            height: 760.0,
            modal: false,
            always_on_top: false,
        }
    }

    #[must_use]
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    #[must_use]
    pub fn size(mut self, width: f64, height: f64) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    #[must_use]
    pub fn always_on_top(mut self, always_on_top: bool) -> Self {
        self.always_on_top = always_on_top;
        self
    }
}

/// 打开窗口；同名窗口已存在时只聚焦。
///
/// 对照 `DialogManager.open()`：同名的窗口只会存在一个实例。
pub fn open(state: &AppState, spec: &WindowSpec) -> Result<()> {
    if spec.label == MAIN {
        anyhow::bail!("主窗口由启动流程创建，不能通过这里打开");
    }

    let app = &state.app;
    if let Some(existing) = app.get_webview_window(&spec.label) {
        tracing::debug!("窗口已存在，聚焦而不新建: {}", spec.label);
        existing.set_focus().ok();
        return Ok(());
    }

    let url = format!("http://127.0.0.1:{}/{}", state.port(), spec.route);
    let parsed: tauri::Url = url
        .parse()
        .with_context(|| format!("非法窗口地址: {url}"))?;
    tracing::info!("打开窗口: {} -> {url}", spec.label);

    let mut builder = WebviewWindowBuilder::new(app, spec.label.as_str(), WebviewUrl::External(parsed))
        .title(&spec.title)
        .inner_size(spec.width, spec.height)
        .always_on_top(spec.always_on_top)
        // 每个窗口都要注入：补鉴权头 + 提供 bridgeCommand；
        // 同时把窗口标签交给脚本，供 `/_anki/pycmd` 判断命令来自哪个窗口
        .initialization_script(bridge::script(&state.api_key, &spec.label));

    if let Some(geometry) = load_geometry(state, &spec.label) {
        builder = builder
            .position(f64::from(geometry.x), f64::from(geometry.y))
            .inner_size(geometry.width, geometry.height);
    }

    // 模态窗口挂在主窗口下，保证它始终浮在主窗口之上
    if spec.modal {
        if let Some(main) = app.get_webview_window(MAIN) {
            builder = builder.parent(&main).context("设置父窗口失败")?;
        }
    }

    let window = builder
        .build()
        .with_context(|| format!("创建窗口失败: {}", spec.label))?;

    if spec.modal {
        set_modal(state, &spec.label, true);
    }

    // 关闭时：记住几何 + 解除模态。
    //
    // `CloseRequested` 时窗口还在，能读到几何；`Destroyed` 时窗口已从管理器里移除，
    // 但它是**唯一保证会触发**的事件，所以模态恢复挂在它上面（否则用户用标题栏的
    // 关闭按钮关掉模态窗口后，主窗口会永久处于禁用状态）。
    let app_handle = app.clone();
    let label = spec.label.clone();
    let state_for_event = state.clone();
    window.on_window_event(move |event| match event {
        WindowEvent::CloseRequested { .. } => {
            let Some(window) = app_handle.get_webview_window(&label) else {
                return;
            };
            if let (Ok(position), Ok(size)) = (window.outer_position(), window.inner_size()) {
                save_geometry(&state_for_event, &label, position.x, position.y, size.width, size.height);
            }
        }
        WindowEvent::Destroyed => release_modal(&state_for_event, &label),
        _ => {}
    });

    Ok(())
}

/// 关闭窗口。
///
/// 对照 Qt 的 `closeAddCards` / `closeEditCurrent` / `deckOptionsRequireClose` /
/// `importDialogRequireClose`：**发起关闭的是页面里的前端代码，而窗口由宿主管理**，
/// WebView 自己关不掉宿主窗口，所以必须绕一圈请求宿主。窗口不存在时只记日志 ——
/// 这不是遗漏，而是正确行为（页面在则窗口在）。
pub fn close(state: &AppState, label: &str) {
    match state.app.get_webview_window(label) {
        Some(window) => {
            tracing::debug!("关闭窗口: {label}");
            // 兜底：万一个别平台不发 Destroyed，也不能让主窗口一直禁用
            release_modal(state, label);
            if let Err(error) = window.close() {
                tracing::warn!("关闭窗口 {label} 失败: {error}");
            }
        }
        None => tracing::debug!("窗口不存在，忽略关闭请求: {label}"),
    }
}

/// 往指定窗口注入脚本。
///
/// 对照 Qt 的 `web.eval(...)`：宿主驱动页面的唯一通道（例如复习界面靠它把渲染结果
/// 交给 `_showQuestion`，编辑器靠它注入 `loadNote`）。多窗口下**必须指定窗口** ——
/// Qt 每个 WebView 有自己的 `eval`，这里只有一个共用方法，写错对象就会把脚本注入到
/// 别的窗口。
pub fn eval(state: &AppState, label: &str, js: &str) {
    let Some(window) = state.app.get_webview_window(label) else {
        tracing::warn!("找不到窗口 {label}，无法注入脚本");
        return;
    };
    if let Err(error) = window.eval(js) {
        tracing::warn!("向 {label} 注入脚本失败: {error}");
    }
}

/// 打开或解除某个窗口的模态（对照 Qt 的 `ApplicationModal` 与导入完成后的
/// 「转非模态」：`mediasrv.import_done` 会 hide → setWindowModality(NonModal) → show）。
pub fn set_modal(state: &AppState, label: &str, modal: bool) {
    if modal {
        *modal_window().lock().expect("模态状态已中毒") = Some(label.to_owned());
        tracing::debug!("进入模态: {label}（主窗口已禁用）");
        set_main_enabled(state, false);
    } else {
        release_modal(state, label);
    }
}

/// 窗口标签 → 模态窗口（同一时刻只可能有一个）。
fn modal_window() -> &'static Mutex<Option<String>> {
    static MODAL: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
    &MODAL
}

fn release_modal(state: &AppState, label: &str) {
    let mut modal = modal_window().lock().expect("模态状态已中毒");
    if modal.as_deref() != Some(label) {
        return;
    }
    *modal = None;
    drop(modal);
    tracing::debug!("解除模态: {label}");
    set_main_enabled(state, true);
}

fn set_main_enabled(state: &AppState, enabled: bool) {
    let Some(main) = state.app.get_webview_window(MAIN) else {
        return;
    };
    if let Err(error) = main.set_enabled(enabled) {
        tracing::warn!("切换主窗口可用状态失败: {error}");
        return;
    }
    if enabled {
        main.set_focus().ok();
    }
}

/// 页面报到（对照 `deckOptionsReady` → `DeckOptionsDialog.set_ready()`）。
///
/// Qt 用它拦掉「页面就绪之前到达的关闭请求」，本外壳的关闭请求由页面自己发起、
/// 不存在这种时序问题，因此这里只记录日志。
pub fn mark_ready(label: &str) {
    tracing::debug!("窗口页面已就绪: {label}");
}

/// 从路由推断窗口标签（路由首段），供开发/测试入口使用。
pub fn label_for_route(route: &str) -> String {
    route
        .split('/')
        .find(|segment| !segment.is_empty())
        .unwrap_or("window")
        .to_owned()
}

/// 已打开的窗口标签，用于诊断与测试。
pub fn open_labels(state: &AppState) -> HashSet<String> {
    let mut labels = HashSet::new();
    for window in state.app.webview_windows().keys() {
        labels.insert(window.clone());
    }
    labels
}

struct Geometry {
    x: i32,
    y: i32,
    width: f64,
    height: f64,
}

fn geometry_key(label: &str) -> String {
    format!("{GEOMETRY_PREFIX}{label}")
}

fn load_geometry(state: &AppState, label: &str) -> Option<Geometry> {
    let prefs = Prefs21::locate(state.profile_dir.as_path())?;
    let value = prefs.get(Store::Profile, &geometry_key(label)).ok()??;
    let items = value.as_array()?;
    let number = |index: usize| items.get(index)?.as_i64();
    Some(Geometry {
        x: number(0)? as i32,
        y: number(1)? as i32,
        // 过小的尺寸多半是记录损坏，退回默认值而不是开一个看不见的窗口
        width: number(2)?.max(300) as f64,
        height: number(3)?.max(200) as f64,
    })
}

fn save_geometry(
    state: &AppState,
    label: &str,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) {
    let Some(prefs) = Prefs21::locate(state.profile_dir.as_path()) else {
        return;
    };
    let value = json!([x, y, width, height]);
    if let Err(error) = prefs.set(Store::Profile, &geometry_key(label), &value) {
        tracing::warn!("保存窗口几何失败（{label}）: {error}");
    }
}
