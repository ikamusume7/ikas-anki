// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Anki 的 Tauri v2 外壳。
//!
//! 启动顺序：**解析资源路径 → 打开集合 → 启动本地服务取端口 → 创建窗口**。
//! 之所以放在 Tauri 的 `setup` 里，是因为需要 `AppHandle` 才能定位打包后的
//! 资源目录。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod backup;
mod bridge;
mod browser;
mod editor;
mod importing;
mod legacy;
mod main_window;
mod method_table;
mod paths;
mod platform;
mod preview;
mod reviewer;
mod reviewer_menu;
mod server;
mod state;
mod windows;

use anyhow::Context;
use anyhow::Result;
use tauri::AppHandle;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;

use crate::backend::AppBackend;
use crate::paths::AppPaths;
use crate::state::AppState;

fn main() -> Result<()> {
    init_tracing();

    tauri::Builder::default()
        .setup(|app| {
            bootstrap(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .context("Tauri 应用启动失败")
}

fn bootstrap(app: &AppHandle) -> Result<()> {
    let paths = AppPaths::resolve(app)?;

    // 界面语言。TODO: 应像 aqt 那样探测系统语言，目前用环境变量覆盖、默认英文。
    //
    // 注意：`ftl/` 下的目录用连字符（`zh-CN`），而 Anki 的设置里用下划线
    // （`zh_CN`），这里统一转成连字符形式。
    let lang = std::env::var("ANKI_LANG")
        .unwrap_or_else(|_| "en-US".to_string())
        .replace('_', "-");
    tracing::info!("界面语言: {lang}");
    let backend = AppBackend::new(vec![lang], String::new(), false)
        .context("初始化 rslib 后端失败")?;

    let profile = paths.profile();
    tracing::info!("集合路径: {}", profile.collection.display());
    if let Some(parent) = profile.collection.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建 profile 目录 {}", parent.display()))?;
    }

    // 桌面版由 Python 侧打开集合；这里由外壳直接打开。
    // 打开失败不阻止启动，便于在窗口内观察错误。
    let opened = match backend.open_collection(&profile) {
        Ok(()) => {
            tracing::info!("集合已打开");
            true
        }
        Err(error) => {
            tracing::error!("打开集合失败: {error:#}");
            false
        }
    };

    let media_folder = opened.then(|| profile.media_folder.clone());
    let state = AppState::new(
        app.clone(),
        backend,
        paths,
        profile.dir.clone(),
        media_folder,
    );
    if opened {
        state.mark_collection_open();
        // 自动备份（PARITY 1.5 / 6.2）：启动时尝试一次 + 之后每 5 分钟一次。
        // 是否真的落盘由后端按 `BackupLimits` 判断，见 `crate::backup`。
        backup::spawn(state.clone());
    }

    // 起始页面可通过 ANKI_START_PAGE 指定（editor / graphs / deck-options ...）。
    // 特例：`reviewer` 走 legacy 页面机制，加载 /_anki/legacyPageData。
    // 默认是**牌组列表**（Qt 的主窗口启动就在 deckBrowser 状态，`main.py:93`）。
    let page = std::env::var("ANKI_START_PAGE").unwrap_or_else(|_| "decks".to_string());
    if page == "reviewer" {
        // 只有复习界面才会在 domDone 时开始取卡
        state
            .reviewer_active
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    // 注入脚本需要 API 密钥，先取出再移交状态
    let api_key = state.api_key.to_string();
    // `spawn` 会持有状态，后续还要用它打开附加窗口，因此传克隆
    let port = server::spawn(state.clone()).context("启动本地 HTTP 服务失败")?;

    let url = if page == "reviewer" {
        let id = legacy::reviewer_page(port);
        tracing::info!("已注册复习页: id={id}");
        let url = format!("http://127.0.0.1:{port}/_anki/legacyPageData?id={id}");
        // 记下来：`d`（牌组列表）之后再点牌组要切回复习页时直接复用这一份
        main_window::remember_reviewer_url(&url);
        url
    } else {
        format!("http://127.0.0.1:{port}/{page}")
    };
    tracing::info!("窗口将加载: {url}");

    let parsed: tauri::Url = url.parse()?;
    let main_window = WebviewWindowBuilder::new(app, windows::MAIN, WebviewUrl::External(parsed))
        .title("Anki")
        .inner_size(1200.0, 820.0)
        // 主窗口**保留** Tauri 的拖放拦截：这样 Rust 侧能拿到拖入文件的**真实路径**
        // （`WindowEvent::DragDrop`），才能像 Qt 那样"把 .apkg 拖到主窗口就导入"
        // （`main.py:112-137`，见 `crate::importing::on_drop`）。代价是页面的 HTML5 drop
        // 收不到（WebView2 的拖放处理器被 Tauri 顶掉了）—— 上游 `reviewer.py:355` 虽然
        // 开着 `allow_drops`，但复习界面里本来也没什么可拖进去的东西，权衡下来选路径。
        // 需要 HTML5 drop 的是编辑器那几类窗口，它们走 `WindowSpec::html5_drops`。
        // 在任何页面脚本之前注入：补鉴权头 + 提供 bridgeCommand，
        // 并把窗口标签交给脚本（`/_anki/pycmd` 据此判断命令来自哪个窗口）
        .initialization_script(bridge::script(&api_key, windows::MAIN))
        .build()?;

    // 拖入文件 → 导入（对照 `main.py:112-137`）
    let state_for_drop = state.clone();
    main_window.on_window_event(move |event| {
        if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
            if let Err(error) = importing::on_drop(&state_for_drop, paths) {
                tracing::warn!("处理拖入文件失败: {error:#}");
            }
        }
    });
    tracing::info!("主窗口: 保留 Tauri 的拖放拦截（拖入文件交给 crate::importing）");

    open_extra_windows(&state);

    Ok(())
}

/// 开发/测试入口：把需要窗口的页面直接开出来
/// （`ANKI_OPEN_ROUTES=deck-options/1,preferences`）。
///
/// 牌组选项 / 导入 / 首选项的产品内入口还没实现（在 Qt 里它们要经过主窗口的工具栏与
/// 牌组浏览器，而牌组浏览器是原生 Qt 界面）。这里提供一个与 `ANKI_START_PAGE` 同类的
/// 测试入口，让「多窗口 + 模态 + 关闭 Hook」这条链路可以端到端验证。
/// （编辑器与浏览器窗口现在有产品内入口了，见 `crate::editor` / `crate::browser`。）
fn open_extra_windows(state: &AppState) {
    let routes = std::env::var("ANKI_OPEN_ROUTES").unwrap_or_default();
    for route in routes.split(',').map(str::trim).filter(|it| !it.is_empty()) {
        let label = windows::label_for_route(route);
        // 只有牌组选项与导入在 Qt 里是 ApplicationModal
        let modal = matches!(label.as_str(), windows::DECK_OPTIONS | windows::IMPORT);
        let spec = windows::WindowSpec::new(label, route).modal(modal);
        if let Err(error) = windows::open(state, &spec) {
            tracing::warn!("打开附加窗口失败（{route}）: {error:#}");
        }
    }
    if !routes.is_empty() {
        tracing::info!("已打开的窗口: {:?}", windows::open_labels(state));
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("anki_tauri=debug,info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
