// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `POST /_anki/pycmd`：桥接命令通道。
//!
//! 对照 Qt 侧 `AnkiWebPage.onBridgeCmd`。请求体是命令字符串
//! （如 `ans`、`ease3`、`close`、`domDone`），响应是 JSON 文本，会传给前端的回调。
//!
//! 已处理的命令集中在复习主链路与外壳自身；其余命令（工具栏动作、编辑器
//! 快捷键、`browserSearch` 等）依赖尚未迁入的 Qt 逻辑，会记录日志并返回 `null`。

use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use tauri::Manager as _;

use crate::reviewer;
use crate::state::AppState;

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !has_api_access(&state, &headers) {
        tracing::warn!("拒绝未授权的桥接命令");
        return (StatusCode::FORBIDDEN, "unauthorized").into_response();
    }

    let command = String::from_utf8_lossy(&body).trim().to_string();
    tracing::debug!("bridge 命令: {command}");

    let value = dispatch(&state, &command)
        .await
        .unwrap_or_else(|| "null".to_string());
    ([(header::CONTENT_TYPE, "application/json")], value).into_response()
}

/// 返回 `Some(json)` 表示要把该 JSON 文本回传给前端回调。
async fn dispatch(state: &AppState, command: &str) -> Option<String> {
    match command {
        // 页面就绪。仅当当前窗口承载复习界面时才触发复习，
        // 否则加载 graphs 之类的页面也会被当成复习开始。
        "domDone" => {
            let active = state.reviewer_active.load(Ordering::SeqCst);
            tracing::debug!("domDone: 复习模式={active}");
            if active {
                reviewer::start(state).await;
            }
            None
        }
        // 显示答案（对照 Qt 的 `ans`）
        "ans" => {
            reviewer::show_answer(state).await;
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
        // 对照 Qt 在 load finished 时绑定的 Esc 键行为
        "close" => {
            if let Some(window) = state.app.get_webview_window("main") {
                if let Err(error) = window.close() {
                    tracing::warn!("关闭窗口失败: {error}");
                }
            }
            None
        }
        other => {
            tracing::debug!("尚未处理的 bridge 命令: {other}");
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
