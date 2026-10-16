// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 复习界面的「更多」/ 右键菜单。
//!
//! 对照 `qt/aqt/reviewer.py:994-1088`：Qt 的 `showContextMenu()` 弹一个原生 QMenu，
//! 底部栏的 `more` 链接（`reviewer.py:682-683`）与卡片右键都会触发它。单页方案里
//! 菜单必须在 Web 里画，于是分工是：
//!
//! - **宿主**：拼菜单数据（文案走 i18n、勾选态读当前卡片/笔记）+ 执行动作；
//! - **页面**：只负责画菜单与表单、把点击回传成 `rev:<动作>[:<参数>]` 桥接命令。
//!
//! 这样页面里没有第二份状态，"菜单勾选态和实际不符"这类问题不会出现。
//!
//! 未做（列在 `tauri/PARITY.md` 的 2.1-2.4、2.6）：上一张卡片信息（要维护"看过的卡片"
//! 历史）、音频的 5 项（重播 / 暂停 / 前后 5 秒 / 录音 `Shift+V` / 重播录音 `V`，播放器要
//! 支持 seek 与录音）、自动前进（`Shift+A` 的自动显示答案，带勾选态）、时间盒；另外旗标名
//! 目前写死 ftl 默认值，不读集合配置 `flagLabels`。

use anki_i18n::tr_args;
use anki_proto::cards::SetFlagRequest;
use anki_proto::config::{config_key, OptionalStringConfigKey};
use anki_proto::notes::RemoveNotesRequest;
use anki_proto::scheduler::{
    bury_or_suspend_cards_request::Mode as BuryMode, schedule_cards_as_new_request::Context,
    BuryOrSuspendCardsRequest, ScheduleCardsAsNewDefaultsRequest, ScheduleCardsAsNewRequest,
    SetDueDateRequest,
};
use anki_proto::tags::NoteIdsAndTagsRequest;
use prost::Message as _;
use serde_json::json;

use crate::editor;
use crate::platform::dialogs;
use crate::state::AppState;
use crate::windows;

/// 标记用的标签名（对照 `pylib/anki/tags.py` 的 `MARKED_TAG`）。
const MARKED_TAG: &str = "marked";

/// 把菜单的样式与交互注入复习页面（与 `install_reviewer_shortcuts` 同时机）。
pub fn install(state: &AppState) {
    windows::eval(state, windows::MAIN, MENU_JS);
}

/// 打开菜单（底部栏的「更多」按钮与卡片右键都走这里）。
///
/// 对照 Qt 的 `showContextMenu()`：菜单内容每次都重新算（`_contextMenu()` 里就带了
/// `currentFlag`），所以旗标/标记的勾选态永远和当前卡片一致。
pub async fn show(state: &AppState) {
    let Some(card) = state.reviewer.current_card() else {
        tracing::warn!("复习界面还没有卡片，忽略「更多」菜单请求");
        return;
    };
    let tr = state.backend.i18n();

    // 勾选态：旗标从卡片的 `flags` 低位取（`Card.user_flag()` = `flags & 7`），
    // 标记要读笔记的标签。
    // 注意**必须现取**：会话里那份 `card` 是显示这张卡时的快照，菜单改过旗标之后
    // 它不会变，直接用会导致"勾选态永远是最初的"、而且 toggle 也判错。
    let flag = fresh_flag(state, card.id).await;
    let marked = note_is_marked(state, card.note_id).await;

    // 文案里带 "..." 的几项，上游用 `actions-with-ellipsis` 包一层
    let with_ellipsis = |key: &str| {
        tr.translate(
            "actions-with-ellipsis",
            Some(tr_args! { "action" => tr.translate(key, None).to_string() }),
        )
        .to_string()
    };

    let flag_items: Vec<serde_json::Value> = [
        ("actions-flag-red", 1),
        ("actions-flag-orange", 2),
        ("actions-flag-green", 3),
        ("actions-flag-blue", 4),
        ("actions-flag-pink", 5),
        ("actions-flag-turquoise", 6),
        ("actions-flag-purple", 7),
    ]
    .into_iter()
    .map(|(key, index)| {
        json!({
            "label": tr.translate(key, None),
            "shortcut": format!("Ctrl+{index}"),
            "checked": flag == index,
            "action": format!("flag:{index}"),
        })
    })
    .collect();

    let menu = json!({
        "items": [
            { "label": tr.translate("studying-flag-card", None), "submenu": flag_items },
            { "label": tr.translate("studying-bury-card", None), "shortcut": "-", "action": "buryCard" },
            { "label": with_ellipsis("actions-forget-card"), "shortcut": "Ctrl+Alt+N", "action": "forget" },
            { "label": with_ellipsis("actions-set-due-date"), "shortcut": "Ctrl+Shift+D", "action": "due" },
            { "label": tr.translate("actions-suspend-card", None), "shortcut": "@", "action": "suspendCard" },
            { "label": tr.translate("actions-options", None), "shortcut": "O", "action": "options" },
            { "label": tr.translate("actions-card-info", None), "shortcut": "I", "action": "cardInfo" },
            serde_json::Value::Null,
            { "label": tr.translate("studying-mark-note", None), "shortcut": "*", "checked": marked, "action": "mark" },
            { "label": tr.translate("studying-bury-note", None), "shortcut": "=", "action": "buryNote" },
            { "label": tr.translate("studying-suspend-note", None), "shortcut": "!", "action": "suspendNote" },
            { "label": with_ellipsis("actions-create-copy"), "shortcut": "Ctrl+Alt+E", "action": "copy" },
            { "label": tr.translate("studying-delete-note", None), "shortcut": "Ctrl+Del", "action": "deleteNote" },
        ],
        "labels": {
            "cancel": tr.translate("actions-cancel", None),
            "confirm": tr.translate("actions-save", None),
            "forgetTitle": tr.translate("actions-forget-card", None),
            "restorePosition": tr.translate("scheduling-restore-position", None),
            "resetCounts": tr.translate("scheduling-reset-counts", None),
            "dueTitle": tr.translate("actions-set-due-date", None),
            "duePrompt": tr.translate(
                "scheduling-set-due-date-prompt",
                Some(tr_args! { "cards" => 1 }),
            ),
        },
    });

    tracing::debug!("打开复习菜单: 旗标={flag} 已标记={marked}");
    windows::eval(state, windows::MAIN, &format!("showReviewerMenu({menu});"));
}

/// 执行菜单动作（`rev:<动作>[:<参数>]`）。
pub async fn handle(state: &AppState, command: &str) {
    let rest = command.trim_start_matches("rev:");
    let (action, param) = match rest.split_once(':') {
        Some((action, param)) => (action, param),
        None => (rest, ""),
    };
    let Some(card) = state.reviewer.current_card() else {
        tracing::warn!("复习界面没有卡片，忽略菜单动作: {command}");
        return;
    };
    let card_id = card.id;
    let note_id = card.note_id;

    match action {
        // 旗标：与当前旗标相同就清除（对照 `reviewer.py:1099-1108`）
        "flag" => {
            let desired: u32 = param.parse().unwrap_or(0);
            // 同上：旗标要现取（会话里的 `card` 是快照）
            let current = fresh_flag(state, card_id).await;
            let flag = if current == desired { 0 } else { desired };
            call(
                state,
                "setFlag",
                SetFlagRequest {
                    card_ids: vec![card_id],
                    flag,
                },
            )
            .await;
            tracing::info!("复习菜单: 旗标 {current} → {flag}");
        }
        "buryCard" | "suspendCard" => {
            let mode = if action == "buryCard" {
                BuryMode::BuryUser
            } else {
                BuryMode::Suspend
            };
            call(
                state,
                "buryOrSuspendCards",
                BuryOrSuspendCardsRequest {
                    card_ids: vec![card_id],
                    note_ids: Vec::new(),
                    mode: mode as i32,
                },
            )
            .await;
            advance(state, action).await;
        }
        "buryNote" | "suspendNote" => {
            let mode = if action == "buryNote" {
                BuryMode::BuryUser
            } else {
                BuryMode::Suspend
            };
            call(
                state,
                "buryOrSuspendCards",
                BuryOrSuspendCardsRequest {
                    card_ids: Vec::new(),
                    note_ids: vec![note_id],
                    mode: mode as i32,
                },
            )
            .await;
            advance(state, action).await;
        }
        "mark" => {
            let request = NoteIdsAndTagsRequest {
                note_ids: vec![note_id],
                tags: MARKED_TAG.to_owned(),
            };
            if note_is_marked(state, note_id).await {
                call(state, "removeNoteTags", request).await;
            } else {
                call(state, "addNoteTags", request).await;
            }
        }
        // 重置：第一次点击（无参数）先弹对话框，拿到勾选值后再走下面那条
        "forget" if param.is_empty() => {
            let (restore_position, reset_counts) =
                forget_defaults(state).await.unwrap_or((true, false));
            let spec = forget_dialog_spec(state, restore_position, reset_counts);
            windows::eval(state, windows::MAIN, &format!("showReviewerDialog({spec});"));
        }
        // 重置：`forget:<恢复位置 0|1>,<重置计数 0|1>`（对话框里两个勾选项）
        "forget" => {
            let mut parts = param.split(',');
            let restore_position = parts.next() == Some("1");
            let reset_counts = parts.next() == Some("1");
            call(
                state,
                "scheduleCardsAsNew",
                ScheduleCardsAsNewRequest {
                    card_ids: vec![card_id],
                    log: false,
                    restore_position,
                    reset_counts,
                    context: Some(Context::Reviewer as i32),
                },
            )
            .await;
            advance(state, "forget").await;
        }
        // 设置到期日：先弹输入框
        "due" if param.is_empty() => {
            let spec = due_dialog_spec(state);
            windows::eval(state, windows::MAIN, &format!("showReviewerDialog({spec});"));
        }
        // 设置到期日：`due:<days>`（形如 `3`、`1-5`、`2026-11-01`，语法由后端解析）
        "due" => {
            // 页面用 `encodeURIComponent` 传值（日期里可能有 `-`、空格等）
            let days = percent_encoding::percent_decode_str(param)
                .decode_utf8_lossy()
                .into_owned();
            if days.trim().is_empty() {
                return;
            }
            call(
                state,
                "setDueDate",
                SetDueDateRequest {
                    card_ids: vec![card_id],
                    days,
                    config_key: Some(OptionalStringConfigKey {
                        key: config_key::String::SetDueReviewer as i32,
                    }),
                },
            )
            .await;
            advance(state, "due").await;
        }
        // 牌组选项：开既有的 `deck-options/<deckId>` 路由（对照 `reviewer.py:1090-1092`）
        "options" => {
            if let Err(error) = open_deck_options(state, card.deck_id).await {
                tracing::warn!("打开牌组选项失败: {error:#}");
            }
        }
        // 卡片信息（对照 `reviewer.py:1096-1097` 的 `on_card_info`）
        "cardInfo" => {
            if let Err(error) = crate::browser::open_card_info(state, card_id) {
                tracing::warn!("打开卡片信息失败: {error:#}");
            }
        }
        // 创建副本：开「添加卡片」并载入当前笔记（对照 `reviewer.py:1032-1036`）
        "copy" => {
            match editor::load_note(state, note_id).await {
                Ok(note) => {
                    if let Err(error) = editor::open_add_cards_with_note(
                        state,
                        note_id,
                        note.notetype_id,
                        card.deck_id,
                    ) {
                        tracing::warn!("创建副本失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("创建副本取 note 失败: {error:#}"),
            }
        }
        // 删除笔记（对照 `reviewer.py:1037-1041`）：Qt 会先问一次
        "deleteNote" => {
            let message = state
                .backend
                .i18n()
                .translate("studying-delete-note", None)
                .to_string();
            let confirmed = {
                let message = message.clone();
                tauri::async_runtime::spawn_blocking(move || dialogs::confirm(&message))
                    .await
                    .unwrap_or(false)
            };
            if !confirmed {
                return;
            }
            call(
                state,
                "removeNotes",
                RemoveNotesRequest {
                    note_ids: vec![note_id],
                    card_ids: Vec::new(),
                },
            )
            .await;
            advance(state, "deleteNote").await;
        }
        other => tracing::warn!("未知的复习菜单动作: {other}"),
    }
}

/// 调后端 RPC（`getNote` 等只读查询不算动作，不记日志）。
async fn call<M: prost::Message>(state: &AppState, method: &str, request: M) {
    if let Err(error) = state.backend.call(method, request.encode_to_vec()).await {
        tracing::warn!("复习菜单 {method} 失败: {error}");
    }
}

/// 动作让卡片离开队列之后，重新取队列并渲染下一张（对照 Qt 的 `self.mw.reset()`）
async fn advance(state: &AppState, action: &str) {
    tracing::info!("复习菜单: {action} → 重新取队列");
    crate::reviewer::start(state).await;
}

/// 现取当前卡片的用户旗标（`flags & 7`）。
///
/// 会话里的 `card` 是显示这张卡时的快照 —— 菜单改过旗标之后它不会变，所以
/// 勾选态与 toggle 判断都必须重新问后端要（失败时按 0 处理，不让菜单打不开）。
async fn fresh_flag(state: &AppState, card_id: i64) -> u32 {
    let request = anki_proto::cards::CardId { cid: card_id };
    match state
        .backend
        .call("getCard", request.encode_to_vec())
        .await
    {
        Ok(bytes) => match anki_proto::cards::Card::decode(bytes.as_slice()) {
            Ok(card) => card.flags & 7,
            Err(error) => {
                tracing::warn!("解析 getCard 结果失败: {error}");
                0
            }
        },
        Err(error) => {
            tracing::warn!("读卡片失败: {error}");
            0
        }
    }
}

async fn note_is_marked(state: &AppState, note_id: i64) -> bool {
    let request = anki_proto::notes::NoteId { nid: note_id };
    match state
        .backend
        .call("getNote", request.encode_to_vec())
        .await
    {
        Ok(bytes) => match anki_proto::notes::Note::decode(bytes.as_slice()) {
            Ok(note) => note.tags.iter().any(|tag| tag == MARKED_TAG),
            Err(error) => {
                tracing::warn!("解析 getNote 结果失败: {error}");
                false
            }
        },
        Err(error) => {
            tracing::warn!("读笔记失败: {error}");
            false
        }
    }
}

/// 打开牌组选项窗口。
///
/// 直接复用 `main_window::open_deck_options`：**筛选牌组的卡片**要开筛选牌组编辑器
/// （对照 `deckoptions.py:125-134`），这里再写一份就会漏掉那条分支。
async fn open_deck_options(state: &AppState, deck_id: i64) -> anyhow::Result<()> {
    crate::main_window::open_deck_options(state, deck_id).await
}

/// 注入到复习页面的菜单与表单。
///
/// 只做"画"和"点"：菜单数据（文案 / 勾选态）与动作执行都在宿主侧，见模块注释。
/// 注入到复习页面的菜单与表单。
///
/// 脚本本体放在 `tauri/assets/reviewer-menu.js`（宿主与验证脚本
/// `tauri/tools/reviewer-menu-check.mjs` 读的是同一份，避免"测的不是跑的那份"）。
const MENU_JS: &str = include_str!("../assets/reviewer-menu.js");

/// 「重置卡片」的对话框参数（对照 `aqt/forms/forget.ui`：两个勾选项）。
fn forget_dialog_spec(
    state: &AppState,
    restore_position: bool,
    reset_counts: bool,
) -> serde_json::Value {
    let tr = state.backend.i18n();
    json!({
        "title": tr.translate("actions-forget-card", None),
        "action": "forget",
        "labels": dialog_labels(state),
        "fields": [
            { "kind": "check", "label": tr.translate("scheduling-restore-position", None), "value": restore_position },
            { "kind": "check", "label": tr.translate("scheduling-reset-counts", None), "value": reset_counts },
        ],
    })
}

/// 「设置到期日」的对话框参数（对照 `operations/scheduling.py:30-62` 的 `getText`）。
fn due_dialog_spec(state: &AppState) -> serde_json::Value {
    let tr = state.backend.i18n();
    json!({
        "title": tr.translate("actions-set-due-date", None),
        "action": "due",
        "labels": dialog_labels(state),
        "fields": [
            {
                "kind": "text",
                "label": tr.translate("scheduling-set-due-date-prompt", Some(tr_args! { "cards" => 1 })),
                "value": "",
            },
        ],
    })
}

/// 对话框按钮文案（Qt 用的是 Qt 自带的标准按钮，核心 ftl 里没有，与浏览器页同处理）。
fn dialog_labels(state: &AppState) -> serde_json::Value {
    let tr = state.backend.i18n();
    json!({
        "cancel": tr.translate("actions-cancel", None),
        "confirm": tr.translate("actions-save", None),
    })
}

/// 重置卡片的默认值（对照 `_forget_current_card` → `schedule_cards_as_new_defaults`）。
pub async fn forget_defaults(state: &AppState) -> Option<(bool, bool)> {
    let bytes = state
        .backend
        .call(
            "scheduleCardsAsNewDefaults",
            ScheduleCardsAsNewDefaultsRequest {
                // 这个字段在 proto 里不是 optional（`ScheduleCardsAsNewRequest.context` 才是）
                context: Context::Reviewer as i32,
            }
            .encode_to_vec(),
        )
        .await
        .ok()?;
    let response = anki_proto::scheduler::ScheduleCardsAsNewDefaultsResponse::decode(bytes.as_slice())
        .ok()?;
    Some((response.restore_position, response.reset_counts))
}
