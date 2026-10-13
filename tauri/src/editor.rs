// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 编辑器窗口（`mode=current` 编辑当前卡片 / `mode=add` 添加卡片）。
//!
//! 新版编辑器的**外层窗口完全在 Web 里**：牌组选择器、notetype 按钮、保存/关闭按钮
//! 都由 `ts/routes/editor` 自己画（旧版 `aqt/forms/addcards.ui` 里的那些 Qt 控件现在
//! 只被 `addcards_legacy.py` 使用）。宿主因此只需要做两件事，对照 `qt/aqt/editor.py`：
//!
//! 1. 开一个窗口加载 `editor/?mode=<add|current|browser>`（`editor.py:117-119`）；
//! 2. 收到页面的 `editorReady` 之后，把 note 的**身份**注入进去：
//!    `require("anki/ui").loaded.then(() => loadNote({...}))`（`editor.py:416-447`）。
//!
//! 第 2 步是关键：编辑器**不会**自己去问后端「当前是哪张卡」。它拿到 `nid` 之后才用
//! RPC（`getNote` / `newNote` / `getNotetype` / `getFieldNames`…）把内容取回来
//! （`ts/routes/editor/NoteEditor.svelte:986-1230`）。少了这一步窗口会一直空白 ——
//! 而且不报任何错，因为页面本身是"正常"挂载完成的。
//!
//! 两种模式的注入参数不同（见 `tauri/README.md` 的「编辑器」）：
//!
//! | 模式 | 注入 | 来源 |
//! | --- | --- | --- |
//! | `current` | 当前复习卡的 `nid` / `notetypeId` / `reviewerCardId` | `qt/aqt/editcurrent.py` |
//! | `add` | 全部留空，只给 `reviewerCardId`（供页面算「home deck」）+ `initial: true` | `qt/aqt/addcards.py:76-83` |
//!
//! `add` 模式下**默认牌组与笔记类型由页面自己算**（`defaultsForAdding` /
//! `defaultDeckForNotetype`，`NoteEditor.svelte:1026-1045`），宿主不要去推断；但
//! `initial: true` 是硬门槛 —— 缺了它页面会退到 `notetypeChooser.getSelected()`
//! 那条路，拿不到「新建 note」。

use anki_proto::notes::Note;
use anki_proto::notes::NoteId;
use anyhow::Context;
use anyhow::Result;
use prost::Message as _;
use serde_json::json;
use tauri::Manager as _;

use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// 打开「编辑当前卡片」窗口。
///
/// 对应复习界面底部栏的「编辑」按钮（`pycmd('edit')`）与 Qt 的 `mw.onEditCurrent()`。
pub async fn open_current_card(state: &AppState) -> Result<()> {
    // 对照 `qt/aqt/editcurrent.py:29-31`：这个窗口编辑的就是复习界面正在显示的那张卡
    let card = state
        .reviewer
        .current_card()
        .context("当前没有正在复习的卡片，无法编辑")?;

    // 先用「note 还在不在」把错误挡在开窗之前，避免开出一个空白窗口
    let note = load_note(state, card.note_id).await?;
    tracing::info!(
        "打开编辑器: card={} note={} notetype={}",
        card.id,
        note.id,
        note.notetype_id
    );

    windows::open(
        state,
        &WindowSpec::new(windows::EDIT_CURRENT, "editor/?mode=current")
            // 对照 `qt/aqt/editcurrent.py:26` 的 `self.setWindowTitle(tr.editing_edit_current())`
            .title(state.backend.i18n().editing_edit_current())
            .size(800.0, 620.0),
    )
    .map_err(|error| anyhow::anyhow!("{error:#}"))
}

/// 打开「添加卡片」窗口（`editor/?mode=add`）。
///
/// 对照 `qt/aqt/addcards.py` 的 `NewAddCards` + `mw.onAddCard`：窗口标题取
/// `tr.actions_add()`（`addcards.py:34`），非模态，几何记忆与单实例由 `windows` 负责。
/// 这里对应复习界面的 `a` 快捷键（Qt 的全局快捷键表 `main.py:1183`）。
pub fn open_add_cards(state: &AppState) -> Result<()> {
    windows::open(
        state,
        &WindowSpec::new(windows::ADD_CARDS, "editor/?mode=add")
            .title(state.backend.i18n().actions_add())
            .size(800.0, 620.0),
    )
    .map_err(|error| anyhow::anyhow!("{error:#}"))
}

/// 打开「添加卡片」并**载入某张已有笔记的内容**（浏览器的「复制笔记」，
/// 对照 `browser.py:828-834` 的 `on_create_copy` → `AddCards.set_note(note, deck_id)`）。
///
/// Qt 传的是 `mid` + `deck_id` + `original_note_id`（`addcards.py:76-83`）：
/// 页面据此把那张笔记的字段/标签填进"新笔记"，牌组用给定的那个。
pub fn open_add_cards_with_note(
    state: &AppState,
    note_id: i64,
    notetype_id: i64,
    deck_id: i64,
) -> Result<()> {
    let args = json!({
        "nid": note_id,
        "notetypeId": notetype_id,
        "deckId": deck_id,
        "originalNoteId": note_id,
        "reviewerCardId": null,
        "focusTo": 0,
        "initial": true,
    });
    tracing::info!("复制笔记到添加卡片窗口: note={note_id} deck={deck_id}");

    // 与浏览器编辑区同一套逻辑：先记下参数，窗口已存在就直接注入，否则等它报到
    state.browser.set_pending_add_copy(args.clone());
    let existed = state.app.get_webview_window(windows::ADD_CARDS).is_some();
    windows::open(
        state,
        &WindowSpec::new(windows::ADD_CARDS, "editor/?mode=add")
            .title(state.backend.i18n().actions_add())
            .size(800.0, 620.0),
    )?;
    if existed {
        inject_note(state, windows::ADD_CARDS, &args);
    }
    Ok(())
}

/// 页面就绪（`editorReady`）后把要编辑的 note 注入进去。
///
/// 对照 `editor.py` 的 `_set_ready()` + `load_note()`。注意 Qt 是在窗口构造时就调用
/// `set_note`，由 `@on_editor_ready` 装饰器排队，等页面报到后才真正 eval —— 这里等价于
/// 在收到 `editorReady` 时再读一次当前卡片。
///
/// 三个编辑器窗口用的是**同一个页面**，所以必须按窗口标签分流：`add-cards` 注入的是
/// 「新建 note」、`browser-editor` 注入的是浏览器选中的那张 note，注入错了页面会直接
/// 进入错误的状态（甚至报错，例如 `mode=browser` 缺 `notetypeId`）。
pub async fn on_ready(state: &AppState, window: &str) {
    if window == windows::ADD_CARDS {
        // 浏览器点「复制笔记」时会把要复制的 note 参数先记下（见
        // `open_add_cards_with_note`）；没有的话就是一张空白的新笔记
        match state.browser.take_pending_add_copy() {
            Some(args) => inject_note(state, window, &args),
            None => inject_new_note(state, window),
        }
        return;
    }
    if window == windows::BROWSER_EDITOR {
        match state.browser.take_pending_note() {
            Some(args) => inject_note(state, window, &args),
            // 浏览器会先记下参数再开窗，理论上到不了这里
            None => tracing::warn!("浏览器编辑器已就绪，但没有待注入的 note"),
        }
        return;
    }

    let Some(card) = state.reviewer.current_card() else {
        tracing::warn!("编辑器已就绪，但当前没有正在复习的卡片，无法注入 note");
        return;
    };
    let note = match load_note(state, card.note_id).await {
        Ok(note) => note,
        Err(error) => {
            tracing::warn!("编辑器取 note 失败: {error:#}");
            return;
        }
    };

    // `reviewerCardId` 用来反查 home deck（`NoteEditor.svelte:1019-1022`）；
    // `initial: true` 与 Qt 的 `load_args` 一致（`editor.py:435-441`）。
    let args = json!({
        "nid": note.id,
        "notetypeId": note.notetype_id,
        "reviewerCardId": card.id,
        "focusTo": 0,
        "initial": true,
    });

    inject_note(state, window, &args);
}

/// 注入「新建一张 note」。
///
/// 对照 `addcards.py:76-83` 的 `load_new_note()` —— Qt 那边 `nid` / `notetypeId` /
/// `deckId` 全为 `None`，默认值由页面自己调 `defaultsForAdding()` 取；唯一的输入是
/// `reviewerCardId`（有正在复习的卡时，页面用它所在的牌组当默认牌组，`addcards.py:49`）。
fn inject_new_note(state: &AppState, window: &str) {
    let reviewer_card_id = state.reviewer.current_card().map(|card| card.id);
    let args = json!({
        "nid": null,
        "notetypeId": null,
        "deckId": null,
        "originalNoteId": null,
        "reviewerCardId": reviewer_card_id,
        "focusTo": 0,
        "initial": true,
    });
    inject_note(state, window, &args);
}

/// 把 `loadNote(...)` 注入到指定窗口。
///
/// 必须等 `anki/ui` 就绪：`loadNote` 是页面挂到 `globalThis` 上的
/// （`NoteEditor.svelte:1303-1360`），而 `anki/ui`.loaded 是页面自己 resolve 的
/// 信号（`ts/lib/tslib/ui.ts`）。这也是 Qt 用 `require("anki/ui").loaded.then(...)`
/// 包一层的原因。
///
/// 结果回传：对照 `editor.py:443` 的 `evalWithCallback(..., oncallback)` ——
/// Qt 也是靠回调确认 loadNote 跑完了。没有它，"注入成功但没生效"只能靠肉眼看窗口
/// （`loadNote` 取不到 note 时页面只会报个错，宿主完全不知情）。
pub(crate) fn inject_note(state: &AppState, window: &str, args: &serde_json::Value) {
    let js = format!(
        "require(\"anki/ui\").loaded\
         .then(() => loadNote({args}))\
         .then(() => pycmd(\"editorLoaded\"))\
         .catch((error) => pycmd(\"editorLoadError:\" + error));"
    );
    tracing::debug!("向编辑器注入: loadNote({args})");
    windows::eval(state, window, &js);
}

/// 取 note。编辑器自己也是这么取的（`NoteEditor.loadNoteInner` 里的 `getNote`）。
pub(crate) async fn load_note(state: &AppState, note_id: i64) -> Result<Note> {
    let request = NoteId { nid: note_id };
    let bytes = state
        .backend
        .call("getNote", request.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(Note::decode(bytes.as_slice())?)
}
