// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Anki 的 Tauri v2 外壳。
//!
//! 启动顺序：**解析资源路径 → 打开集合 → 启动本地服务取端口 → 创建窗口**。
//! 之所以放在 Tauri 的 `setup` 里，是因为需要 `AppHandle` 才能定位打包后的
//! 资源目录。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autosync;
mod backend;
mod backup;
mod bridge;
mod browser;
mod editor;
mod i18n_lang;
mod importing;
mod legacy;
mod main_window;
mod method_table;
mod paths;
mod platform;
mod preview;
mod profiles;
mod reviewer;
mod reviewer_menu;
mod restore;
mod server;
mod state;
mod theme;
mod windows;

use anyhow::Context;
use anyhow::Result;
use tauri::AppHandle;
use tauri::WebviewUrl;
use tauri::WebviewWindowBuilder;

use crate::backend::AppBackend;
use crate::paths::AppPaths;
use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
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

    // 界面语言（PARITY 1.5 / 8.4）。优先级：
    //   `ANKI_LANG`（测试钩子）> `meta["defaultLang"]`（桌面版偏好设置写的）>
    //   系统语言 > `en-US`。换算法则见 `crate::i18n_lang`（对照 `pylib/anki/lang.py`）。
    // 改语言需要重启进程，与 Qt 一致（`preferences.py:461-463`）。
    let lang = match std::env::var("ANKI_LANG") {
        Ok(value) if !value.trim().is_empty() => i18n_lang::to_disk_lang(&value),
        _ => i18n_lang::resolve(stored_default_lang(&paths).as_deref()),
    };
    tracing::info!("界面语言: {lang}");
    let backend = AppBackend::new(vec![lang], String::new(), false)
        .context("初始化 rslib 后端失败")?;

    let profile = paths.profile();
    tracing::info!(
        "配置: {}（目录 {}）",
        profile
            .dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        profile.dir.display()
    );
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
        // 自动同步（PARITY 6.1）：启动时一次 + 之后每 5 分钟检查媒体同步，
        // 开关与间隔都在 profile 配置里，见 `crate::autosync`。
        autosync::spawn(state.clone());
    }

    // 主题 / 夜间模式（PARITY 8.3）：`meta["theme"]`（0 跟随系统 / 1 浅色 / 2 深色）。
    // 跟随系统时 Windows 读注册表 `AppsUseLightTheme`，与 Qt 同一个键；见 `crate::theme`。
    let (theme, _ui_scale) = theme::read_prefs(&state.profile_dir);
    state.set_night_mode(theme::night_mode(theme));
    tracing::info!(
        "主题: theme={} night={}",
        theme.as_i64(),
        state.night_mode()
    );

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
        let id = legacy::reviewer_page(port, &theme::style(&state));
        tracing::info!("已注册复习页: id={id}");
        let url = format!("http://127.0.0.1:{port}/_anki/legacyPageData?id={id}");
        // 记下来：`d`（牌组列表）之后再点牌组要切回复习页时直接复用这一份
        main_window::remember_reviewer_url(&url);
        url
    } else {
        // SvelteKit 页面靠 URL 尾部的 `#night` 自己 checkNightMode()（`nightmode.ts:6-12`）；
        // 与 `?cb=` 的顺序要照着上游：query 在前、hash 在后（`webview.py:953-957`）。
        format!(
            "http://127.0.0.1:{port}/{page}{}",
            theme::url_suffix(&state)
        )
    };
    tracing::info!("窗口将加载: {url}");

    let parsed: tauri::Url = url.parse()?;
    let main_window = WebviewWindowBuilder::new(app, windows::MAIN, WebviewUrl::External(parsed))
        .title("Anki")
        .inner_size(1200.0, 820.0)
        // 窗口底色随主题（对照 `webview.py:991` 的 `setBackgroundColor(CANVAS)`，避免首帧白闪）
        .background_color(theme::window_color(&state))
        // 主窗口**保留** Tauri 的拖放拦截：这样 Rust 侧能拿到拖入文件的**真实路径**
        // （`WindowEvent::DragDrop`），才能像 Qt 那样"把 .apkg 拖到主窗口就导入"
        // （`main.py:112-137`，见 `crate::importing::on_drop`）。代价是页面的 HTML5 drop
        // 收不到（WebView2 的拖放处理器被 Tauri 顶掉了）—— 上游 `reviewer.py:355` 虽然
        // 开着 `allow_drops`，但复习界面里本来也没什么可拖进去的东西，权衡下来选路径。
        // 需要 HTML5 drop 的是编辑器那几类窗口，它们走 `WindowSpec::html5_drops`。
        // 在任何页面脚本之前注入：补鉴权头 + 提供 bridgeCommand，
        // 并把窗口标签交给脚本（`/_anki/pycmd` 据此判断命令来自哪个窗口）
        .initialization_script(bridge::script(&api_key, windows::MAIN, state.night_mode()))
        .build()?;

    // 拖入文件 → 导入（对照 `main.py:112-137`）
    let state_for_drop = state.clone();
    main_window.on_window_event(move |event| {
        if let tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event {
            tracing::info!(
                "主窗口拖入 {} 个文件: {}",
                paths.len(),
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Err(error) = importing::on_drop(&state_for_drop, paths) {
                tracing::warn!("处理拖入文件失败: {error:#}");
            }
        }
    });
    tracing::info!("主窗口: 保留 Tauri 的拖放拦截（拖入文件交给 crate::importing）");

    // 命令行带文件启动（双击 `.apkg` / `anki file.apkg`，对照 `main.py` 的命令行处理）。
    // 与拖入走**同一套**接受规则与同一个入口（`importing::on_drop` → 同一个路由页），
    // 差别只是来源。延迟一下再导：窗口此刻还在加载起始页，立刻导航会被它覆盖掉。
    let cli_paths = cli_file_paths(std::env::args_os().skip(1));
    if !cli_paths.is_empty() {
        tracing::info!(
            "命令行传入文件: {}",
            cli_paths
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let state_for_cli = state.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            if let Err(error) = importing::on_drop(&state_for_cli, &cli_paths) {
                tracing::warn!("处理命令行传入的文件失败: {error:#}");
            }
        });
    }

    open_extra_windows(&state);

    Ok(())
}

/// 命令行里**存在且像文件**的参数（对照 `main.py` 的命令行处理）。
///
/// 只收真实存在的文件：Tauri/WebView2 自己也会塞一些以 `-` 开头的开关，路径里也可能
/// 有已经失效的文件（上游在文件不存在时会提示"请用 文件→导入"，这里直接忽略）。
fn cli_file_paths<I>(args: I) -> Vec<std::path::PathBuf>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    args.into_iter()
        .map(std::path::PathBuf::from)
        .filter(|path| path.is_file())
        .collect()
}

/// `prefs21.db` 的 `_global` 行里 `meta["defaultLang"]`（桌面版偏好设置里选的语言，
/// 形如 `zh_CN`）。文件不存在、没有这一项、或值不是字符串时返回 `None`，由调用方
/// 退回系统语言。
fn stored_default_lang(paths: &AppPaths) -> Option<String> {
    let prefs = Prefs21::locate(&paths.profile().dir)?;
    match prefs.get(Store::Meta, "defaultLang") {
        Ok(Some(serde_json::Value::String(lang))) => Some(lang),
        _ => None,
    }
}

/// 开发/测试入口：把需要窗口的页面直接开出来
/// （`ANKI_OPEN_ROUTES=deck-options/1,preferences`）。
///
/// 牌组选项 / 导入 / 首选项现在都有产品内入口了（齿轮菜单与复习菜单的「选项」、牌组列表
/// 底部的「导入文件」、菜单的 工具 → 首选项），这个入口保留下来是为了**不依赖 UI 路径**
/// 就能验证「多窗口 + 模态 + 关闭 Hook」这条链路（与 `ANKI_START_PAGE` 同类）。
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

#[cfg(test)]
mod test {
    use super::cli_file_paths;
    use std::ffi::OsString;

    #[test]
    fn cli_paths_keep_existing_files_only() {
        let dir = std::env::temp_dir();
        let file = dir.join("anki-cli-args-test.apkg");
        std::fs::write(&file, b"x").unwrap();
        let missing = dir.join("anki-cli-args-missing.apkg");
        let _ = std::fs::remove_file(&missing);
        let args: Vec<OsString> = vec![
            // 不存在的文件
            missing.clone().into_os_string(),
            // Tauri/WebView2 自己的开关
            OsString::from("--some-flag"),
            // 存在的文件
            file.clone().into_os_string(),
            // 目录（不算可导入）
            dir.clone().into_os_string(),
        ];
        assert_eq!(cli_file_paths(args), vec![file.clone()]);
        // 一个都没有时返回空，不会去动导入流程
        assert!(cli_file_paths(Vec::<OsString>::new()).is_empty());
        let _ = std::fs::remove_file(&file);
    }
}