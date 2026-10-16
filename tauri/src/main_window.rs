// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 主窗口（复习界面所在的那个窗口）的入口与**页面切换**。
//!
//! Qt 的主窗口是一个状态机（`main.py:93-94` 的 `MainWindowState`）：
//! `deckBrowser`（牌组列表，启动默认）/ `overview`（牌组概览）/ `review`（复习）。
//! 本外壳用两个页面覆盖这三个状态：
//!
//! - `/decks/`（新写的 SvelteKit 页面，`ts/routes/decks/`）= 牌组列表；
//!   `/decks/?current=1` 或 `/decks/?deck=<id>` 时页面里显示该牌组的**概览**
//!   （牌组名 + New/Learning/To Review 计数 + 「学习」按钮，对照 `overview.py`）；
//! - 复习页（legacy 页面，`legacy::reviewer_page`）= 复习状态。
//!
//! 切换靠整页导航（`window.location.href`），与卡片信息窗口换卡片是同一招
//! （`browser.rs::open_card_info`）。**切页前必须更新 `reviewer_active`**：`domDone`
//! 只有它为真时才去启动复习（`server/pycmd.rs` 的 `domDone` 分支），否则牌组列表的
//! `domDone` 会往不存在的 `#qa` 注入 DOM。
//!
//! 主窗口全局快捷键（`main.py:1189-1198`）里与本文件相关的是：
//! `d` → 牌组列表、`s` → 概览、`t`/`Shift+t` → 统计、`Ctrl+P` → 设置。
//! 还没有的是 `Ctrl+:`（Python 调试控制台）；`y`（同步）只在牌组列表页接了，
//! 复习页另外还缺 `F11`/`Ctrl+E`/`Ctrl+Shift+Z`/缩放三键（见 `tauri/PARITY.md` 1.4）。

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::sync::OnceLock;

use anyhow::Result;
use prost::Message as _;
use tauri::Manager as _;

use anki_proto::decks::deck;
use anki_proto::decks::Deck;

use crate::legacy;
use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// 复习页的 URL。启动时如果就是复习界面，`main.rs` 会把它记进来（`remember_reviewer_url`），
/// 避免重复注册 legacy 页面；否则第一次切过去时再注册。
static REVIEWER_URL: OnceLock<String> = OnceLock::new();

/// 记下启动时注册好的复习页 URL（见 `main.rs`）。
pub fn remember_reviewer_url(url: &str) {
    let _ = REVIEWER_URL.set(url.to_string());
}

/// 复习页的 URL（没有就现注册一份；legacy 页面的 id 会一直有效）。
fn reviewer_url(state: &AppState) -> String {
    REVIEWER_URL
        .get_or_init(|| {
            let port = state.port();
            let id = legacy::reviewer_page(port, &crate::theme::style(state));
            tracing::info!("已注册复习页: id={id}");
            format!("http://127.0.0.1:{port}/_anki/legacyPageData?id={id}")
        })
        .clone()
}

/// 让主窗口导航到给定地址。
fn navigate(state: &AppState, url: &str) {
    tracing::info!("主窗口导航到: {url}");
    windows::eval(
        state,
        windows::MAIN,
        &format!("window.location.href = \"{url}\";"),
    );
}

/// 显示牌组列表（Qt 的 `d` → `moveToState("deckBrowser")`，`main.py:1181`）。
pub fn show_decks(state: &AppState) -> Result<()> {
    show_decks_page(state, None)
}

/// 显示当前牌组的概览（Qt 的 `s` 在复习状态 → `moveToState("overview")`，
/// `main.py:1240-1245`）。
///
/// 页面自己去问 `getCurrentDeck`（宿主不查，省一次 RPC）。
pub fn show_overview(state: &AppState) -> Result<()> {
    // 对照 `overview.py:98`：显示概览就重新开始计时（时间盒的起点在这里）
    state.reviewer.start_timebox();
    show_decks_page(state, Some("current".to_string()))
}

/// 打开牌组列表页面；`deck` 非空时页面里同时显示该牌组的概览。
fn show_decks_page(state: &AppState, deck: Option<String>) -> Result<()> {
    // 先撤掉复习态：牌组列表的 domDone 不该去启动复习（见文件头注释）
    state.reviewer_active.store(false, Ordering::SeqCst);
    let query = match deck {
        Some(deck) => format!("?deck={deck}"),
        None => String::new(),
    };
    navigate(state, &format!("/decks/{query}"));
    Ok(())
}

/// 开始复习（Qt 的 Overview「Study Now」/ `onStudyKey` 在概览里 → `moveToState("review")`）。
pub fn start_study(state: &AppState) -> Result<()> {
    // 对照 `main.py:1242` / `toolbar.py:447`：开始复习时重新计时
    state.reviewer.start_timebox();
    state.reviewer_active.store(true, Ordering::SeqCst);
    let url = reviewer_url(state);
    navigate(state, &url);
    Ok(())
}

/// 打开牌组选项窗口（牌组列表的齿轮菜单 → `opts:<did>`，复习菜单的「选项」也走这里）。
///
/// **筛选牌组走另一个对话框**：对照 `deckoptions.py:125-134` 的
/// `display_options_for_deck()`，dyn 牌组开的是 `FilteredDeckConfigDialog`，而不是牌组选项
/// 页（rslib 对筛选牌组的 `deck-options` 会直接报错，用户既看不到筛选条件也改不了）。
/// 本外壳复用牌组列表页里的筛选牌组编辑器：开一个 `?filtered=<id>` 的窗口，页面预填并弹出。
pub async fn open_deck_options(state: &AppState, deck_id: i64) -> Result<()> {
    let deck = read_deck(state, deck_id).await;
    let filtered = deck
        .as_ref()
        .and_then(|deck| deck.kind.as_ref())
        .map(|kind| matches!(kind, deck::Kind::Filtered(_)))
        .unwrap_or(false);
    let name = deck.map(|deck| deck.name).unwrap_or_default();

    if filtered {
        // 标题与上游一致：`actions-options-for(val=牌组名)`（`filtered_deck.py:192-193`）
        let title = state.backend.i18n().translate(
            "actions-options-for",
            Some(anki_i18n::tr_args! { "val" => name.clone() }),
        );
        tracing::info!("打开筛选牌组选项: deck={deck_id}（{name}）");
        return windows::open(
            state,
            &WindowSpec::new(windows::FILTERED_DECK, &format!("decks/?filtered={deck_id}"))
                .title(title)
                .size(720.0, 780.0),
        );
    }

    tracing::info!("打开牌组选项: deck={deck_id}");
    windows::open(
        state,
        &WindowSpec::new(windows::DECK_OPTIONS, &format!("deck-options/{deck_id}"))
            .title(state.backend.i18n().translate("actions-options", None))
            .size(900.0, 700.0),
    )
}

/// 读一个牌组（`getDeck`）。失败时返回 `None` —— 调用方按"不是筛选牌组"处理。
async fn read_deck(state: &AppState, deck_id: i64) -> Option<Deck> {
    let request = anki_proto::decks::DeckId { did: deck_id };
    match state
        .backend
        .call("getDeck", prost::Message::encode_to_vec(&request))
        .await
    {
        Ok(bytes) => match Deck::decode(bytes.as_slice()) {
            Ok(deck) => Some(deck),
            Err(error) => {
                tracing::warn!("解析 getDeck 结果失败: {error}");
                None
            }
        },
        Err(error) => {
            tracing::warn!("读牌组失败（按普通牌组处理）: {error}");
            None
        }
    }
}

/// 打开「自定义学习」窗口（概览里的「自定义学习」，对照 Qt 的 `customstudy.py`）。
///
/// 页面 `ts/routes/custom-study/` 自己调 `customStudy` RPC；成功后它会发
/// `customStudyDone`，由宿主把主窗口导航到当前牌组的概览并关掉这个对话框
/// （Qt 那边只关对话框、当前牌组已被后端切成筛选牌组，用户得自己回去看）。
pub fn open_custom_study(state: &AppState, deck_id: i64) -> Result<()> {
    tracing::info!("打开自定义学习: deck={deck_id}");
    windows::open(
        state,
        &WindowSpec::new(windows::CUSTOM_STUDY, &format!("custom-study/?deck={deck_id}"))
            // 对照 `customstudy.ui:14` 的对话框标题（`actions-custom-study`）
            .title(state.backend.i18n().translate("actions-custom-study", None))
            .size(560.0, 620.0),
    )
}

/// 打开「字段…」对话框（编辑器工具栏的「字段」按钮 → 前端端点 `openFieldsDialog`，
/// 对照 `qt/aqt/fields.py` 的 `FieldDialog`）。
///
/// 端点不带参数：Qt 那边对着**编辑器窗口当前的笔记类型**开（`mediasrv.py:1096-1106`
/// → `window.editor.onFields()`），所以这里从 `editor::last_notetype()` 取。
/// 标题是 `fields-fields-for`（"Fields for {notetype name}"），所以要读一次 notetype。
pub async fn open_fields_for_current(state: &AppState) -> Result<()> {
    let Some(notetype_id) = crate::editor::last_notetype() else {
        tracing::warn!("没有正在编辑的笔记类型，忽略「字段…」请求");
        return Ok(());
    };
    open_fields_for_notetype(state, notetype_id).await
}

/// 对着**指定的**笔记类型开「字段…」窗口。
///
/// 「管理笔记类型」窗口的「字段…」按钮走这条路（Qt 那边是
/// `models.py:238` 的 `onFields` → `FieldDialog(self, m)`）。
pub async fn open_fields_for_notetype(state: &AppState, notetype_id: i64) -> Result<()> {
    let notetype = load_notetype(state, notetype_id).await?;
    tracing::info!("打开字段对话框: notetype={notetype_id}");
    windows::open(
        state,
        &WindowSpec::new(windows::FIELDS, &format!("fields/?notetype={notetype_id}"))
            // 对照 `fields.py:50` 的 `tr.fields_fields_for(val=notetype name)`
            .title(state.backend.i18n().fields_fields_for(&notetype.name))
            .size(640.0, 720.0),
    )
}

/// 读一份 notetype（标题要用名字，所以开窗前必须拿到）。
async fn load_notetype(
    state: &AppState,
    notetype_id: i64,
) -> Result<anki_proto::notetypes::Notetype> {
    let bytes = state
        .backend
        .call(
            "getNotetype",
            prost::Message::encode_to_vec(&anki_proto::notetypes::NotetypeId {
                ntid: notetype_id,
            }),
        )
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    prost::Message::decode(bytes.as_slice()).map_err(|error| anyhow::anyhow!("{error}"))
}

/// 打开「卡片…」对话框（编辑器工具栏的「卡片」按钮 / `Ctrl+L` → 前端端点
/// `openCardsDialog`，对照 `qt/aqt/clayout.py` 的 `CardLayout`）。
///
/// 端点同样不带参数，但 Qt 那边要三样东西（`editor.py:312-332` 的 `onCardLayout`）：
/// 笔记类型、**编辑器窗口当前那张 note**（预览用，`clayout.py:536`）、以及初始卡片
/// 序号 = 正在复习的那张卡的 `ord`（`editor.py:315`，没有卡片时取 0）。
///
/// 三者都随 URL 传给页面（`?notetype=&ord=&note=`），页面自己去取数据 ——
/// 宿主只负责开窗（见 `tauri/README.md` 的「窗口模型」）。标题对照
/// `clayout.py:78` 的 `card_templates_card_types_for`（"Card Types for {val}"）。
pub async fn open_cards_for_current(state: &AppState) -> Result<()> {
    let Some((window, notetype_id, note_id)) = crate::editor::current_notetype_and_note() else {
        tracing::warn!("没有正在编辑的笔记类型，忽略「卡片…」请求");
        return Ok(());
    };
    let notetype = load_notetype(state, notetype_id).await?;

    // Qt 取的是「编辑器当前卡片」的 ord（`editor.py:315`）。本外壳的编辑器窗口只有
    // 编辑当前卡片时才知道自己对应哪张卡，所以从复习会话里取；新建笔记的那两个窗口
    // （`add-cards` / 浏览器编辑区）在 Qt 里也没有卡片对象（`Note(col, mid)`，ord=0）。
    let ord = if window == windows::EDIT_CURRENT {
        state
            .reviewer
            .current_card()
            .map(|card| card.template_idx)
            .unwrap_or(0)
    } else {
        0
    };

    let mut route = format!("cards/?notetype={notetype_id}&ord={ord}");
    if let Some(note_id) = note_id {
        route.push_str(&format!("&note={note_id}"));
    }
    open_cards(state, notetype_id, &notetype.name, &route)
}

/// 对着**指定的**笔记类型开「卡片…」窗口（没有 note：页面自己造一张空笔记，ord=0）。
///
/// 「管理笔记类型」窗口的「卡片…」按钮走这条路（Qt 是 `models.py:243` 的 `onCards`）。
pub async fn open_cards_for_notetype(state: &AppState, notetype_id: i64) -> Result<()> {
    let notetype = load_notetype(state, notetype_id).await?;
    open_cards(
        state,
        notetype_id,
        &notetype.name,
        &format!("cards/?notetype={notetype_id}&ord=0"),
    )
}

fn open_cards(
    state: &AppState,
    notetype_id: i64,
    notetype_name: &str,
    route: &str,
) -> Result<()> {
    tracing::info!("打开卡片对话框: notetype={notetype_id} route={route}");
    windows::open(
        state,
        &WindowSpec::new(windows::CARDS, route)
            .title(
                state
                    .backend
                    .i18n()
                    .card_templates_card_types_for(notetype_name),
            )
            .size(1000.0, 720.0),
    )
}

/// 打开「管理笔记类型」窗口（Qt 的 `Models` 对话框，`qt/aqt/models.py:39`）。
///
/// 上游有两个入口：主窗口菜单「管理笔记类型」（`main.py:1357` → `Models(mw, mw,
/// fromMain=True)`）与浏览器侧栏 Note Types 的右键菜单（`browser/sidebar/tree.py:924-928`）。
/// 主窗口菜单栏画在牌组列表页、浏览器侧栏还没有右键菜单，入口因此挂在**浏览器窗口**的
/// 「更多」菜单里 —— 差异记在 `tauri/PARITY.md` 5.2 / 5.5。
pub fn open_notetypes(state: &AppState) -> Result<()> {
    tracing::info!("打开「管理笔记类型」窗口");
    windows::open(
        state,
        &WindowSpec::new(windows::NOTETYPES, "notetypes/")
            // 对照 `models.ui:17` 的 windowTitle＝`notetypes-note-types`
            .title(state.backend.i18n().notetypes_note_types())
            .size(560.0, 640.0),
    )
}

/// 「获取共享牌组」（Qt 的 `deckbrowser.py:396` → `openLink(aqt.appShared + "decks")`）：
/// 交给系统默认浏览器打开 ankiweb 的共享牌组页（与 Qt 走 `open_link` 一样）。
pub fn open_shared_decks() -> Result<()> {
    const SHARED_DECKS: &str = "https://ankiweb.net/shared/decks";
    tracing::info!("打开共享牌组页: {SHARED_DECKS}");
    opener::open(SHARED_DECKS).map_err(|error| anyhow::anyhow!("{error:#}"))
}

/// 打开统计图窗口（`t` / `Shift+t` → `onStats()`，`main.py:1331`）。
///
/// 页面是既有的 `graphs/`，默认搜索词是 `deck:current`，所以看到的就是**当前牌组**
/// （牌组列表里选中的那个、复习界面里正在复习的那个），与 Qt 的 `_selectedDeck()` 同义。
///
/// **还差**（`tauri/PARITY.md` 3.2）：Qt 的对话框顶部有原生 `DeckChooser`（不用回主窗口
/// 就能换牌组）与「Save PDF」按钮；标题用的是 `statistics-title`（统计数据），而这里复用了
/// 工具栏那个 `qt-misc-stats`（统计）。
pub fn open_stats(state: &AppState) -> Result<()> {
    tracing::info!("打开统计图窗口");
    windows::open(
        state,
        &WindowSpec::new(windows::STATS, "graphs/")
            // 对照工具栏的「统计」（`qt-misc-stats`）；Qt 对话框标题是 `statistics-title`
            .title(state.backend.i18n().translate("qt-misc-stats", None))
            .size(900.0, 760.0),
    )
}

/// 打开「切换配置」窗口（Qt 的 `ProfileManager`，`main.py:321-345`）。
///
/// 标题用 `profiles-profiles`（Qt 的 `forms/profiles.ui` 是 "Anki"，
/// 但窗口列表里叫 Profiles 更好认，差异记在 PARITY 8.2）。
pub fn open_profiles(state: &AppState) -> Result<()> {
    tracing::info!("打开切换配置窗口");
    windows::open(
        state,
        &WindowSpec::new(windows::PROFILES, "profiles/")
            .title(state.backend.i18n().translate("profiles-profiles", None))
            .size(520.0, 480.0),
    )
}

/// 打开偏好设置窗口（`Ctrl+P` → `onPrefs()`，`main.py:1459` → `Preferences` 对话框）。
pub fn open_preferences(state: &AppState) -> Result<()> {
    tracing::info!("打开偏好设置窗口");
    windows::open(
        state,
        &WindowSpec::new(windows::PREFERENCES, "preferences/")
            .title(state.backend.i18n().translate("preferences-preferences", None))
            .size(760.0, 700.0),
    )
}

// 视图菜单 ####################################################################

/// 视图 → 全屏：切换**发起命令的那个窗口**的全屏状态
/// （`main.py:1484-1495` 的 `on_toggle_full_screen` 切的是当前活动窗口）。
pub fn toggle_full_screen(state: &AppState, label: &str) {
    let Some(window) = state.app.get_webview_window(label) else {
        tracing::warn!("找不到窗口 {label}，无法切换全屏");
        return;
    };
    let next = !window.is_fullscreen().unwrap_or(false);
    match window.set_fullscreen(next) {
        Ok(()) => tracing::info!("全屏: {next}"),
        Err(error) => tracing::warn!("切换全屏失败: {error}"),
    }
}

/// 视图 → 放大 / 缩小 / 重置缩放（`main.py:1462-1470`：`±0.1`，重置回 `1.0`）。
///
/// Qt 那边**不持久化**缩放（每次启动都是 1.0），这里也保持一致；WebView2 的缩放范围是
/// 0.25–5，这里再收窄到 0.3–3，避免按几下就变成不可用的尺寸。
///
/// 注意：Tauri 的 `WebviewWindow` 只有 `set_zoom`、**没有** getter，所以自己记一份当前值
/// （能改缩放的只有这个菜单）。
pub fn zoom(state: &AppState, label: &str, mode: &str) {
    let Some(window) = state.app.get_webview_window(label) else {
        tracing::warn!("找不到窗口 {label}，无法缩放");
        return;
    };
    let mut levels = zoom_levels()
        .lock()
        .expect("缩放表被 poison 了");
    let current = *levels.entry(label.to_string()).or_insert(1.0);
    let next = match mode {
        "in" => current + 0.1,
        "out" => current - 0.1,
        _ => 1.0,
    }
    .clamp(0.3, 3.0);
    match window.set_zoom(next) {
        Ok(()) => {
            levels.insert(label.to_string(), next);
            tracing::info!("缩放: {next:.1}");
        }
        Err(error) => tracing::warn!("缩放失败: {error}"),
    }
}

/// 每个窗口的缩放系数（见 `zoom`）。
fn zoom_levels() -> &'static Mutex<HashMap<String, f64>> {
    static LEVELS: OnceLock<Mutex<HashMap<String, f64>>> = OnceLock::new();
    LEVELS.get_or_init(|| Mutex::new(HashMap::new()))
}
