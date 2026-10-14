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
//! `d` → 牌组列表、`s` → 概览、`t`/`Shift+t` → 统计、`Ctrl+P` → 设置；
//! 还没接的是 `Ctrl+:`（Python 调试控制台）与 `y`（同步，见 `tauri/PARITY.md` 6.1）。

use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use anyhow::Result;

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
            let id = legacy::reviewer_page(port);
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
    state.reviewer_active.store(true, Ordering::SeqCst);
    let url = reviewer_url(state);
    navigate(state, &url);
    Ok(())
}

/// 打开牌组选项窗口（牌组列表的齿轮菜单 → `opts:<did>`）。
///
/// 与复习菜单里的「选项」是同一个页面（`deck-options/<did>`），见 `reviewer_menu.rs`。
pub fn open_deck_options(state: &AppState, deck_id: i64) -> Result<()> {
    tracing::info!("打开牌组选项: deck={deck_id}");
    windows::open(
        state,
        &WindowSpec::new(windows::DECK_OPTIONS, &format!("deck-options/{deck_id}"))
            .title(state.backend.i18n().translate("actions-options", None))
            .size(900.0, 700.0),
    )
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
    let notetype = load_notetype(state, notetype_id).await?;
    tracing::info!("打开字段对话框: notetype={notetype_id}");
    windows::open(
        state,
        &WindowSpec::new(windows::FIELDS, &format!("fields/?notetype={notetype_id}"))
            // 对照 `fields.py:50` 的 `tr.fields_fields_for(val=notetype name)`
            .title(
                state
                    .backend
                    .i18n()
                    .fields_fields_for(&notetype.name),
            )
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
    tracing::info!("打开卡片对话框: notetype={notetype_id} ord={ord} note={note_id:?}");
    windows::open(
        state,
        &WindowSpec::new(windows::CARDS, &route)
            .title(
                state
                    .backend
                    .i18n()
                    .card_templates_card_types_for(&notetype.name),
            )
            .size(1000.0, 720.0),
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
/// 页面是既有的 `graphs/`。Qt 会把**当前选中的牌组**传进去（`_selectedDeck()`），
/// 这里打开的是整个集合的视图（页面里可以自己换牌组/搜索词）。
pub fn open_stats(state: &AppState) -> Result<()> {
    tracing::info!("打开统计图窗口");
    windows::open(
        state,
        &WindowSpec::new(windows::STATS, "graphs/")
            // 对照 DeckStats 对话框标题（`qt-misc-stats`）
            .title(state.backend.i18n().translate("qt-misc-stats", None))
            .size(900.0, 760.0),
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
