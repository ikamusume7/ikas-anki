// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 浏览器窗口（`ts/routes/browser`）。
//!
//! ## 与上游的关系（重要）
//!
//! Qt 的浏览器**整个都是原生 Qt 界面**：搜索框、笔记表格、侧边栏、以及嵌在同一个
//! 窗口里的编辑区（`qt/aqt/browser/browser.py:130`、`608-630`）。仓库里没有对应的
//! Web 页面（`ts/routes/` 只有 card-info / change-notetype / congrats / deck-options /
//! editor / graphs / image-occlusion / import-* / preferences），所以这里的页面是
//! **新写的**（与 deck-options / editor 那种"复用上游页面"不同）。两处刻意的差异：
//!
//! 1. **编辑区不内嵌**：Qt 把编辑器塞进浏览器窗口的下半部分；而 `/editor/` 是不可信
//!    页面（CSP 带 `frame-ancestors 'none'`，见 `server/assets.rs`），iframe 会被
//!    WebView 直接拒绝。这里改成**独立窗口**（标签 `browser-editor`），双击表格里
//!    的一行打开。关掉浏览器窗口时会连带关掉它。
//! 2. **不读 `defaultSearchText` 配置**：Qt 的默认搜索是
//!    `defaultSearchText`，没配置才退回 `deck:current`（`browser.py:566-576`）。
//!    这里先用后者（读配置要么走 profile 配置端点，要么加一次 RPC，属后续增量）。
//!
//! ## 宿主契约（与编辑器同构）
//!
//! 1. 宿主开窗加载 `browser/`；
//! 2. 页面挂载完 → `pycmd("browserReady")`，**宿主在响应里把首屏搜索词交回去**
//!    （`browserReady` 的响应体是 `{"search": "..."}`）；
//! 3. 窗口已存在时不再开窗，而是直接 `eval` 注入 `loadSearch("...")`。
//!
//! 第 2 步是必须的：页面自己不知道要搜什么。把搜索词放在**响应**里而不是"就绪后宿主
//! 注入"，是为了躲开"注入时页面还没就绪"的时序问题 —— 窗口刚创建时 eval 有可能落空。

use std::sync::Mutex;

use anki_proto::cards::Card;
use anki_proto::cards::CardId;
use anki_proto::generic::String as GenericString;
use anki_proto::search::SearchNode;
use anyhow::Result;
use prost::Message as _;
use serde_json::json;
use serde_json::Value;
use tauri::Manager as _;

use crate::editor;
use crate::platform::dialogs;
use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// `defaultSearchText` 配置为空时的默认搜索，对照 `qt/aqt/browser/browser.py:572`
/// 的 `SearchNode(deck="current")`。
const DEFAULT_SEARCH: &str = "deck:current";

/// 浏览器窗口存在期间的"待办"。
///
/// 放在 `AppState` 里而不是模块级 static：与 `reviewer` / `recorder` 一致，
/// 也让以后的多集合场景不会串味。
#[derive(Default)]
pub struct Browser {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// 最近一次请求的搜索词：窗口不存在时先记下，等页面报到时由它取走
    search: Option<String>,
    /// 待注入到 `browser-editor` 的 `loadNote` 参数（双击一行时设上）
    pending_note: Option<Value>,
    /// 待注入到「添加卡片」窗口的参数（「复制笔记」用，见 `editor::open_add_cards_with_note`）
    pending_add_copy: Option<Value>,
    /// 「更改笔记类型」窗口作用的笔记 id（页面提交的请求里**没有** note_ids，
    /// 由宿主补上，见 `server/api.rs`；对照 `changenotetype.py:82`）
    pending_change_notetype: Option<Vec<i64>>,
}

impl Browser {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("浏览器状态已中毒")
    }

    fn remember_search(&self, search: &str) {
        self.lock().search = Some(search.to_owned());
    }

    /// 页面报到（`browserReady`）时取走的搜索词。
    pub fn current_search(&self) -> String {
        self.lock()
            .search
            .clone()
            .unwrap_or_else(|| DEFAULT_SEARCH.to_owned())
    }

    fn set_pending_note(&self, args: Value) {
        self.lock().pending_note = Some(args);
    }

    pub fn take_pending_note(&self) -> Option<Value> {
        self.lock().pending_note.take()
    }

    pub fn set_pending_add_copy(&self, args: Value) {
        self.lock().pending_add_copy = Some(args);
    }

    pub fn take_pending_add_copy(&self) -> Option<Value> {
        self.lock().pending_add_copy.take()
    }

    pub fn set_pending_change_notetype(&self, note_ids: Vec<i64>) {
        self.lock().pending_change_notetype = Some(note_ids);
    }

    /// 取一份拷贝（保存失败时页面可能重试，所以不在这里清掉）。
    pub fn pending_change_notetype(&self) -> Option<Vec<i64>> {
        self.lock().pending_change_notetype.clone()
    }

    pub fn clear_pending_change_notetype(&self) {
        self.lock().pending_change_notetype = None;
    }
}

/// 打开（或聚焦）浏览器窗口，并让它执行 `search`（`None` 表示用默认搜索）。
///
/// 是 `async` 的原因：默认搜索要读集合配置 `defaultSearchText`
/// （`browser.py:566-573`），而读配置得走一次 RPC。
pub async fn open(state: &AppState, search: Option<&str>) -> Result<()> {
    let search = match search {
        Some(search) => search.to_owned(),
        None => default_search(state).await,
    };
    state.browser.remember_search(&search);

    // `windows::open` 对已存在的窗口只聚焦，所以先问一次：已存在说明页面早就报到了，
    // 必须直接把搜索词注入进去，否则窗口会停在旧结果上。
    let existed = state.app.get_webview_window(windows::BROWSER).is_some();
    windows::open(
        state,
        &WindowSpec::new(windows::BROWSER, "browser/")
            .title(state.backend.i18n().browsing_sidebar())
            .size(1100.0, 760.0),
    )?;
    if existed {
        inject_search(state, &search);
    }
    Ok(())
}

/// `FrontendService::SearchInBrowser`：请求体是 `search.SearchNode`（一个筛选条件），
/// 响应为空。
///
/// 对照 `qt/aqt/mediasrv.py:755-762` 的 `search_in_browser()`：把节点翻成搜索字符串，
/// 再打开浏览器。调用方见 `ts/routes/import-page/lib.ts` 与编辑器的「重复」链接。
///
/// 这个端点**不在** `platform::dispatch` 里：它要调异步 RPC（`buildSearchString`），
/// 而平台层跑在阻塞线程池上（见 `server/api.rs` 的调用点）。
pub async fn search_in_browser(state: &AppState, body: &[u8]) -> Result<()> {
    let node = SearchNode::decode(body).map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let search = build_search_string(state, node).await?;
    tracing::info!("在浏览器里搜索: {search}");
    open(state, Some(&search)).await
}

/// 没指定搜索时用的默认值。
///
/// 对照 `browser.py:566-573` 的 `_default_search()`：先看集合配置
/// `defaultSearchText`，非空就用它，否则 `deck:current`。
async fn default_search(state: &AppState) -> String {
    match state.backend.collection_config("defaultSearchText").await {
        Some(Value::String(text)) if !text.trim().is_empty() => {
            tracing::debug!("默认搜索取自 defaultSearchText: {text}");
            text
        }
        _ => DEFAULT_SEARCH.to_owned(),
    }
}

/// 把 `SearchNode` 翻成搜索字符串。对照 Qt 的 `col.build_search_string(...)`：
/// 引号与转义规则都在 rslib 里，宿主与页面都不该自己拼。
pub async fn build_search_string(state: &AppState, node: SearchNode) -> Result<String> {
    let bytes = state
        .backend
        .call("buildSearchString", node.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    let built = GenericString::decode(bytes.as_slice())?;
    Ok(built.val)
}

/// 双击表格里的一行 → 用编辑器打开这张卡片的 note。
///
/// 对照 Qt：浏览器把当前选中的卡片交给内嵌编辑器
/// （`browser.py:653-658` 的 `editor.set_note(self.card.note(), focusTo=...)`）。
/// 我们这边编辑区是独立窗口，所以先解析出 note 身份，再走编辑器的注入契约。
///
/// `notetypeId` 必须一起给：`mode=browser` 不渲染笔记类型选择器，页面在缺它时会退回
/// `notetypeChooser.getSelected()` 并报错（与 `mode=current` 的原因相同）。
pub async fn edit_card(state: &AppState, card_id: i64) -> Result<()> {
    let card = load_card(state, card_id).await?;
    open_editor_for_note(state, card.note_id).await
}

/// 笔记模式下的双击：选中的一行就是 note id，不需要再经卡片反查。
pub async fn edit_note(state: &AppState, note_id: i64) -> Result<()> {
    open_editor_for_note(state, note_id).await
}

async fn open_editor_for_note(state: &AppState, note_id: i64) -> Result<()> {
    let note = editor::load_note(state, note_id).await?;
    let args = json!({
        "nid": note.id,
        "notetypeId": note.notetype_id,
        "focusTo": 0,
        "initial": true,
    });
    tracing::info!("浏览器打开编辑器: note={}", note.id);

    state.browser.set_pending_note(args.clone());
    let existed = state.app.get_webview_window(windows::BROWSER_EDITOR).is_some();
    windows::open(
        state,
        &WindowSpec::new(windows::BROWSER_EDITOR, "editor/?mode=browser")
            .title(state.backend.i18n().studying_edit())
            .size(800.0, 620.0),
    )?;
    if existed {
        // 窗口已存在 = 页面早已就绪：直接注入（否则要等它下次报到）
        editor::inject_note(state, windows::BROWSER_EDITOR, &args);
    }
    Ok(())
}

/// 预览窗口点了上一张/下一张：**清单在浏览器页面手里**，所以把请求转过去，
/// 由页面算出邻居再回一条 `preview:<cid>`（见 `ts/routes/browser` 的 `previewNeighbor`）。
pub fn request_neighbor(state: &AppState, offset: i32, card_id: i64) {
    if state.app.get_webview_window(windows::BROWSER).is_none() {
        tracing::debug!("浏览器窗口已关闭，预览无法切换卡片");
        return;
    }
    let js = format!("previewNeighbor({offset}, {card_id});");
    windows::eval(state, windows::BROWSER, &js);
}

/// 编辑器里的「预览」按钮：把请求转给浏览器页面，由它把**当前那一行**报回来
/// （对照 `browser.py:611-614`：编辑器的 `preview` 链接指向浏览器的 `onTogglePreview`）。
pub fn request_preview(state: &AppState) {
    if state.app.get_webview_window(windows::BROWSER).is_none() {
        tracing::debug!("浏览器窗口不存在，预览请求被忽略");
        return;
    }
    windows::eval(state, windows::BROWSER, "previewCurrent();");
}

/// 打开「更改笔记类型」窗口（`browser.py:1241-1252` 的 `onChangeNoteType` →
/// `changenotetype.change_notetype_dialog`）。
///
/// 两个对齐点：
///
/// 1. Qt 先 `col.models.get_single_notetype_of_notes(note_ids)` —— **选中的笔记必须是
///    同一种笔记类型**，否则弹警告并且不开窗（`changenotetype.py:88-93`）。后端有
///    现成的 `getSingleNotetypeOfNotes`（`notes.proto:31`），错误文本已经可读，
///    这里照搬"报错就不开窗"。
/// 2. 页面是已有的 SvelteKit 动态路由 `change-notetype/<oldId>[/<newId>]`
///    （`ts/routes/change-notetype/[...notetypeIds]/+page.ts`，单个 id 时新旧相同，
///    用户随后在页面里选新类型），宿主只要把 URL 指对 —— 和卡片信息窗口同一个套路。
///    窗口是**应用级模态**（`changenotetype.py:46`），关闭时机见 `server/api.rs`
///    对 `changeNotetype` 的接管。
pub async fn open_change_notetype(state: &AppState, note_ids: Vec<i64>) -> Result<()> {
    if note_ids.is_empty() {
        return Ok(());
    }
    let request = anki_proto::notes::NoteIds {
        note_ids: note_ids.clone(),
    };
    let notetype_id = match state
        .backend
        .call("getSingleNotetypeOfNotes", request.encode_to_vec())
        .await
    {
        Ok(bytes) => match anki_proto::notetypes::NotetypeId::decode(bytes.as_slice()) {
            Ok(id) => id.ntid,
            Err(error) => {
                tracing::warn!("解析 NotetypeId 失败: {error}");
                return Ok(());
            }
        },
        Err(error) => {
            // 对照 `changenotetype.py:91-93` 的 `showWarning(str(e))`：混合笔记类型时
            // 后端给的就是可读文本（"笔记类型不一致"之类）。
            // 弹框是阻塞调用，放到阻塞线程池上（这里是 async 上下文，不能占着 worker）
            let message = error.clone();
            tauri::async_runtime::spawn_blocking(move || dialogs::warn(&message));
            tracing::info!("不能更改笔记类型: {error}");
            return Ok(());
        }
    };

    tracing::info!("打开更改笔记类型窗口: notetype={notetype_id} 笔记 {} 张", note_ids.len());
    // 页面提交的请求里没有 note_ids（它不知道选了哪些笔记），保存时由宿主补上
    state.browser.set_pending_change_notetype(note_ids);
    windows::open(
        state,
        &WindowSpec::new(
            windows::CHANGE_NOTETYPE,
            &format!("change-notetype/{notetype_id}"),
        )
        // 对照 `changenotetype.py:60` 的 `tr.browsing_change_notetype()`
        .title(state.backend.i18n().browsing_change_notetype())
        .size(800.0, 800.0)
        // 对照 `changenotetype.py:46` 的 `setWindowModality(ApplicationModal)`
        .modal(true),
    )?;
    Ok(())
}

/// 打开「卡片信息」窗口（`browser.py:799-803` → `aqt/card_info.py:62` 的
/// `web.load_sveltekit_page(f"card-info/{card_id}")`）。
///
/// `card-info/<cid>` 是**已有的 SvelteKit 路由**（`ts/routes/card-info/[cardId]`），
/// 页面自己在 load 里取 `cardStats`，所以宿主只要开窗并把 URL 指对。
/// Qt 的 `CardInfoDialog` 是单例（复用同一个对话框），这里也用固定标签 —— 已经开着
/// 但看的是另一张卡时，直接让它跳过去（整页导航，页面会在 load 里重新取数）。
pub fn open_card_info(state: &AppState, card_id: i64) -> Result<()> {
    tracing::info!("打开卡片信息: card={card_id}");
    let existed = state.app.get_webview_window(windows::CARD_INFO).is_some();
    windows::open(
        state,
        &WindowSpec::new(windows::CARD_INFO, &format!("card-info/{card_id}/"))
            // 对照 `actions-card-info`（Qt 的 `CardInfoDialog` 用的就是它）
            .title(state.backend.i18n().actions_card_info())
            .size(600.0, 700.0),
    )?;
    if existed {
        let js = format!("window.location.href = \"/card-info/{card_id}/\";");
        windows::eval(state, windows::CARD_INFO, &js);
    }
    Ok(())
}

/// 注入搜索：页面若还没挂载好就自己忽略（它会在报到时把搜索词取走）。
fn inject_search(state: &AppState, search: &str) {
    let quoted = serde_json::to_string(search).unwrap_or_else(|_| "\"\"".to_owned());
    let js = format!(
        "if (typeof window.loadSearch === \"function\") {{ window.loadSearch({quoted}); }}"
    );
    tracing::debug!("向浏览器注入搜索: {search}");
    windows::eval(state, windows::BROWSER, &js);
}

/// 取卡片（只需要 `note_id`）。
async fn load_card(state: &AppState, card_id: i64) -> Result<Card> {
    let request = CardId { cid: card_id };
    let bytes = state
        .backend
        .call("getCard", request.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(Card::decode(bytes.as_slice())?)
}
