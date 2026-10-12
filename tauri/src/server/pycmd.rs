// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `POST /_anki/pycmd`：桥接命令通道。
//!
//! 对照 Qt 侧 `AnkiWebPage.onBridgeCmd`。请求体是命令字符串
//! （如 `ans`、`ease3`、`close`、`domDone`），响应是 JSON 文本，会传给前端的回调。
//!
//! 已处理的命令集中在复习主链路与外壳自身；其余命令（工具栏动作、编辑器
//! 快捷键、`browserSearch` 等）依赖尚未迁入的 Qt 逻辑，会记录日志并返回 `null`。
//!
//! **命令按来源窗口分流**：注入脚本会在每个请求上带 `X-Anki-Window`（见
//! `crate::bridge`）。Qt 里每个 WebView 有自己的 `AnkiWebPage`，命令天然归属该
//! 窗口；这里必须显式判断，否则任意窗口加载时发的 `domDone` 都会重启复习会话。

use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;

use crate::editor;
use crate::platform::record;
use crate::reviewer;
use crate::state::AppState;
use crate::windows;

/// 来源窗口的请求头，由注入脚本写入。
const WINDOW_HEADER: &str = "x-anki-window";

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !has_api_access(&state, &headers) {
        tracing::warn!("拒绝未授权的桥接命令");
        return (StatusCode::FORBIDDEN, "unauthorized").into_response();
    }

    let window = source_window(&headers);
    let command = String::from_utf8_lossy(&body).trim().to_string();
    tracing::debug!("bridge 命令[{window}]: {command}");

    let value = dispatch(&state, &window, &command)
        .await
        .unwrap_or_else(|| "null".to_string());
    ([(header::CONTENT_TYPE, "application/json")], value).into_response()
}

/// 命令来自哪个窗口。老版本注入脚本不带这个头时按主窗口处理。
fn source_window(headers: &HeaderMap) -> String {
    headers
        .get(WINDOW_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or(windows::MAIN)
        .to_owned()
}

/// 返回 `Some(json)` 表示要把该 JSON 文本回传给前端回调。
async fn dispatch(state: &AppState, window: &str, command: &str) -> Option<String> {
    // 编辑器注入结果的回传与窗口无关（主窗口直接跑编辑器时也会发）
    if handle_editor_report(window, command) {
        return None;
    }

    // 非主窗口只处理与它自己相关的命令（关窗、录音、编辑器），
    // 复习相关的命令一律不接受 —— 见文件头关于 `domDone` 的说明。
    if window != windows::MAIN {
        return dispatch_secondary(state, window, command).await;
    }

    match command {
        // 页面就绪。仅当主窗口承载复习界面时才触发复习，
        // 否则加载 graphs 之类的页面也会被当成复习开始。
        "domDone" => {
            let active = state.reviewer_active.load(Ordering::SeqCst);
            tracing::debug!("domDone: 复习模式={active}");
            if active {
                reviewer::start(state).await;
            }
            None
        }
        // 显示答案（对照 Qt 的 `ans`）。
        // 类型作答时桥接脚本会把输入框内容附在命令后面：`ans:<用户输入>`
        "ans" => {
            reviewer::show_answer(state, None).await;
            None
        }
        command if command.starts_with("ans:") => {
            let typed = command.trim_start_matches("ans:").to_string();
            reviewer::show_answer(state, Some(typed)).await;
            None
        }
        // 四档评分（对照 Qt 的 `ease1`..`ease4`）
        "ease1" | "ease2" | "ease3" | "ease4" => {
            let ease = command
                .trim_start_matches("ease")
                .parse::<u32>()
                .unwrap_or(3);
            reviewer::answer(state, ease).await;
            None
        }
        // 撤销（对照 Qt 的 `u` 快捷键与全局 Ctrl+Z）
        "undo" => {
            reviewer::undo(state).await;
            None
        }
        // 自定义调度完成（对照 Qt 的 `statesMutated`）。宿主据此放行评分按钮。
        "statesMutated" => {
            reviewer::mark_states_mutated(state);
            None
        }
        // 自定义调度脚本抛错（本外壳特有的上报，Qt 用 evalWithCallback 检测语法错误）
        command if command.starts_with("stateMutationError:") => {
            tracing::warn!(
                "自定义调度脚本失败: {}",
                command.trim_start_matches("stateMutationError:")
            );
            None
        }
        // 编辑当前卡片（对照 Qt 复习界面底部栏的 `edit` → `mw.onEditCurrent()`）
        "edit" => {
            if let Err(error) = editor::open_current_card(state).await {
                tracing::warn!("打开编辑器失败: {error:#}");
            }
            None
        }
        // 主窗口直接加载编辑器时（`ANKI_START_PAGE=editor`）也会发这条，
        // 与编辑器窗口走同一套注入逻辑
        "editorReady" => {
            editor::on_ready(state, window).await;
            None
        }
        // 对照 Qt 在 load finished 时绑定的 Esc 键行为
        "close" => {
            windows::close(state, window);
            None
        }
        other => {
            tracing::debug!("尚未处理的 bridge 命令: {other}");
            None
        }
    }
}

/// 编辑器注入结果的回传（见 `crate::editor::on_ready`）。
///
/// 对照 Qt 的 `evalWithCallback`：只有拿到回调才知道 `loadNote` 真的跑完了。
/// 返回 `true` 表示这条命令已被消费。
fn handle_editor_report(window: &str, command: &str) -> bool {
    if command == "editorLoaded" {
        tracing::info!("编辑器已载入 note[{window}]");
        return true;
    }
    if let Some(error) = command.strip_prefix("editorLoadError:") {
        tracing::warn!("编辑器载入 note 失败[{window}]: {error}");
        return true;
    }
    false
}

/// 主窗口之外的窗口发来的命令。对照 Qt：每个窗口有自己的 `AnkiWebPage`，
/// 只会收到自己页面发出的命令。
async fn dispatch_secondary(state: &AppState, window: &str, command: &str) -> Option<String> {
    match command {
        // 页面就绪：与复习无关的窗口忽略即可（正是这一步要避免误触发复习）
        "domDone" => {
            tracing::debug!("忽略 {window} 的 domDone");
            None
        }
        "close" => {
            windows::close(state, window);
            None
        }
        // 录音对话框的「保存 / 取消」（对照 `RecordDialog.accept` / `reject`）
        "saveRecording" => {
            state.recorder.finish(record::Outcome::Save);
            None
        }
        "cancelRecording" => {
            state.recorder.finish(record::Outcome::Discard);
            None
        }
        // 编辑器就绪：把当前卡片的 note 注入进去（对照 `editor.py` 的 `_set_ready`）。
        // 少了这一步编辑器窗口会一直空白，而且不报错 —— 页面自己是"正常"加载完的。
        "editorReady" => {
            editor::on_ready(state, window).await;
            None
        }
        // 编辑器的状态跟踪命令。Qt 用它们维护 `currentField` / `EditorState`
        // （`editor.py:341-361`），本外壳既没有 addon 也没有"未保存"拦截，只记日志。
        // 注意字段的**保存**不在这里：页面自己去调 `updateNote`（`NoteEditor.svelte:431-435`），
        // 与 Qt 一致；`saved` 只是 `saveNow()` 完成后的通知（`editor.py:545`）。
        command
            if command.starts_with("key")
                || command.starts_with("focus")
                || command.starts_with("blur")
                || command.starts_with("editorState")
                || command == "saved" =>
        {
            tracing::debug!("编辑器命令[{window}]: {command}");
            None
        }
        other => {
            tracing::debug!("窗口 {window} 发来的未处理命令: {other}");
            None
        }
    }
}

fn has_api_access(state: &AppState, headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {}", state.api_key);
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}
