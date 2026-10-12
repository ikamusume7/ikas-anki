// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Anki 的 Tauri v2 外壳。
//!
//! 启动顺序：**解析资源路径 → 打开集合 → 启动本地服务取端口 → 创建窗口**。
//! 之所以放在 Tauri 的 `setup` 里，是因为需要 `AppHandle` 才能定位打包后的
//! 资源目录。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backend;
mod bridge;
mod legacy;
mod method_table;
mod paths;
mod platform;
mod reviewer;
mod server;
mod state;

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
    }

    // 起始页面可通过 ANKI_START_PAGE 指定（editor / graphs / deck-options ...）。
    // 特例：`reviewer` 走 legacy 页面机制，加载 /_anki/legacyPageData。
    let page = std::env::var("ANKI_START_PAGE").unwrap_or_else(|_| "graphs".to_string());
    if page == "reviewer" {
        // 只有复习界面才会在 domDone 时开始取卡
        state
            .reviewer_active
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    // 注入脚本需要 API 密钥，先取出再移交状态
    let api_key = state.api_key.to_string();
    let port = server::spawn(state).context("启动本地 HTTP 服务失败")?;

    let url = if page == "reviewer" {
        let id = legacy::reviewer_page(port);
        tracing::info!("已注册复习页: id={id}");
        format!("http://127.0.0.1:{port}/_anki/legacyPageData?id={id}")
    } else {
        format!("http://127.0.0.1:{port}/{page}")
    };
    tracing::info!("窗口将加载: {url}");

    let parsed: tauri::Url = url.parse()?;
    WebviewWindowBuilder::new(app, "main", WebviewUrl::External(parsed))
        .title("Anki")
        .inner_size(1200.0, 820.0)
        // 在任何页面脚本之前注入：补鉴权头 + 提供 bridgeCommand
        .initialization_script(bridge::script(&api_key))
        .build()?;

    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("anki_tauri=debug,info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}
