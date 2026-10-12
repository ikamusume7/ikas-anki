// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 编辑器窗口。
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

use anki_proto::notes::Note;
use anki_proto::notes::NoteId;
use anyhow::Context;
use anyhow::Result;
use prost::Message as _;
use serde_json::json;

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
            .title("编辑当前")
            .size(800.0, 620.0),
    )
    .map_err(|error| anyhow::anyhow!("{error:#}"))
}

/// 页面就绪（`editorReady`）后把要编辑的 note 注入进去。
///
/// 对照 `editor.py` 的 `_set_ready()` + `load_note()`。注意 Qt 是在窗口构造时就调用
/// `set_note`，由 `@on_editor_ready` 装饰器排队，等页面报到后才真正 eval —— 这里等价于
/// 在收到 `editorReady` 时再读一次当前卡片。
pub async fn on_ready(state: &AppState, window: &str) {
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

    // 必须等 `anki/ui` 就绪：`loadNote` 是页面挂到 `globalThis` 上的
    // （`NoteEditor.svelte:1303-1360`），而 `anki/ui`.loaded 是页面自己 resolve 的
    // 信号（`ts/lib/tslib/ui.ts`）。这也是 Qt 用 `require("anki/ui").loaded.then(...)`
    // 包一层的原因。
    //
    // 结果回传：对照 `editor.py:443` 的 `evalWithCallback(..., oncallback)` ——
    // Qt 也是靠回调确认 loadNote 跑完了。没有它，"注入成功但没生效"只能靠肉眼看窗口
    // （`loadNote` 取不到 note 时页面只会报个错，宿主完全不知情）。
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
async fn load_note(state: &AppState, note_id: i64) -> Result<Note> {
    let request = NoteId { nid: note_id };
    let bytes = state
        .backend
        .call("getNote", request.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(Note::decode(bytes.as_slice())?)
}
