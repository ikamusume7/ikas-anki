// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `POST /_anki/{method}` 分发。
//!
//! 两类端点：
//! 1. rslib 服务方法（`METHOD_TABLE` 由 build.rs 生成）；
//! 2. 外壳实现的平台端点（`FrontendService`，见 `crate::platform`）。
//!
//! 对照 mediasrv 的 `_extract_collection_post_request`、`raw_backend_request`
//! 与 `_check_dynamic_request_permissions`。

use anki_proto::notetypes::ChangeNotetypeRequest;
use anki_proto::notetypes::NotetypeId;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use prost::Message as _;

use crate::method_table;
use crate::platform;
use crate::state::AppState;

/// 复习界面允许访问的端点（对照 mediasrv 的白名单）。
/// 复习页可能包含第三方 JS，因此不开放完整 API。
const REVIEWER_ENDPOINTS: &[&str] = &[
    "getSchedulingStatesWithContext",
    "setSchedulingStates",
    "i18nResources",
    "congratsInfo",
];

pub async fn handle_rpc(
    State(state): State<AppState>,
    Path(method): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = check_post_permission(&state, &headers, &method) {
        return response;
    }

    // 0. 需要**异步 RPC** 的平台端点：浏览器要把 `SearchNode` 翻成搜索词
    //    （`buildSearchString`），而 `platform::dispatch` 跑在阻塞线程池上，
    //    所以这条单独放在这里，不落进下面的平台分支。
    if method == "searchInBrowser" {
        return match crate::browser::search_in_browser(&state, &body).await {
            // 对照 `frontend.proto` 的 `generic.Empty`：响应体为空
            Ok(()) => ([(header::CONTENT_TYPE, "application/binary")], Vec::new()).into_response(),
            Err(error) => {
                tracing::warn!("searchInBrowser 失败: {error:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
            }
        };
    }

    // 「字段…」对话框：编辑器工具栏的「字段」按钮（`NotetypeButtons.svelte:43` 调
    // 的就是这个端点）。不带参数 —— 对着编辑器窗口当前的笔记类型开窗（对照
    // `mediasrv.py:1096-1106` 的 `open_fields_dialog`）。
    if method == "openFieldsDialog" {
        return match crate::main_window::open_fields_for_current(&state).await {
            Ok(()) => ([(header::CONTENT_TYPE, "application/binary")], Vec::new()).into_response(),
            Err(error) => {
                tracing::warn!("打开字段对话框失败: {error:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
            }
        };
    }

    // 「卡片…」对话框：编辑器工具栏的「卡片」按钮 / `Ctrl+L`（`NotetypeButtons.svelte:61`
    // 调的就是这个端点）。同样不带参数，对着编辑器窗口当前的笔记类型开窗（对照
    // `mediasrv.py:1109-1116` 的 `open_cards_dialog`）。
    if method == "openCardsDialog" {
        return match crate::main_window::open_cards_for_current(&state).await {
            Ok(()) => ([(header::CONTENT_TYPE, "application/binary")], Vec::new()).into_response(),
            Err(error) => {
                tracing::warn!("打开卡片对话框失败: {error:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
            }
        };
    }

    // 「管理笔记类型」窗口的两个按钮：对**指定的**笔记类型开「字段…」「卡片…」窗口。
    //
    // 上游 Qt 是 `models.py:238/243` 的 `onFields` / `onCards`（直接 `FieldDialog(self, m)` /
    // `CardLayout(self, m)`），页面这边没有窗口能力，所以照 `openFieldsDialog` /
    // `openCardsDialog` 的样子各开一条端点，只是这次把笔记类型 id 放在请求体里
    // （`anki.notetypes.NotetypeId`）。
    if method == "openFieldsDialogForNotetype" || method == "openCardsDialogForNotetype" {
        let notetype_id = match NotetypeId::decode(body.as_ref()) {
            Ok(input) => input.ntid,
            Err(error) => {
                return (StatusCode::BAD_REQUEST, format!("{method} 解析失败: {error}"))
                    .into_response()
            }
        };
        let opened = if method == "openFieldsDialogForNotetype" {
            crate::main_window::open_fields_for_notetype(&state, notetype_id).await
        } else {
            crate::main_window::open_cards_for_notetype(&state, notetype_id).await
        };
        return match opened {
            Ok(()) => ([(header::CONTENT_TYPE, "application/binary")], Vec::new()).into_response(),
            Err(error) => {
                tracing::warn!("{method} 失败: {error:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}")).into_response()
            }
        };
    }

    // 0.5 「更改笔记类型」：补上 note_ids + RPC 跑完由**宿主**关窗。
    //
    // 上游是窗口感知的端点：`mediasrv.py:772` 发现请求来自 `ChangeNotetypeDialog`
    // 就调 `dialog.save(data)` —— 而 `save()` 做的正是两件事（`changenotetype.py:68-85`）：
    // 把宿主记住的 `note_ids` 补进请求（**页面不知道选了哪些笔记**，它只给字段映射），
    // 然后执行操作、成功即关窗。这里照做。
    // 因为 `backend.call` 是 async、`platform::dispatch` 跑在阻塞线程池上，
    // 所以和 `searchInBrowser` 一样单独接一条。
    if method == "changeNotetype" {
        let mut input = match ChangeNotetypeRequest::decode(body.as_ref()) {
            Ok(input) => input,
            Err(error) => {
                return (StatusCode::BAD_REQUEST, format!("changeNotetype 解析失败: {error}"))
                    .into_response()
            }
        };
        if input.note_ids.is_empty() {
            match state.browser.pending_change_notetype() {
                Some(note_ids) => input.note_ids = note_ids,
                None => {
                    tracing::warn!("changeNotetype 请求没有 note_ids，且宿主也没有记录（窗口没开着？）")
                }
            }
        }
        return match state.backend.call(&method, input.encode_to_vec()).await {
            Ok(output) => {
                state.browser.clear_pending_change_notetype();
                crate::windows::close(&state, crate::windows::CHANGE_NOTETYPE);
                ([(header::CONTENT_TYPE, "application/binary")], output).into_response()
            }
            // 失败时不关窗：用户可以改完映射再保存一次
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
        };
    }

    // 1. rslib 服务方法。
    //
    // 例外：与平台层**同名**的端点由平台层接管 —— 对照 Qt，`post_handler_list` 里的
    // 处理器会盖过后端服务（`_handle_dynamic_request` 先查处理器表）。若不做这个例外，
    // `getConfigJson` 之类两边都有的名字会被 rslib 接走，语义与 Qt 不一致
    // （Qt 缺键返回 null，rslib 报错）。见 `platform::SHADOWED_METHODS`。
    if !platform::SHADOWED_METHODS.contains(&method.as_str()) {
        if let Some((service, index)) = method_table::lookup(&method) {
            return match state.backend.run(service, index, body.to_vec()).await {
                Ok(output) => {
                    ([(header::CONTENT_TYPE, "application/binary")], output).into_response()
                }
                // rslib 的错误是已编码的 BackendError protobuf，原样回传给前端
                Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
            };
        }
    }

    // 2. 平台端点：原生对话框与剪贴板是阻塞调用，放到阻塞线程池
    let blocking_state = state.clone();
    let blocking_method = method.clone();
    let blocking_body = body.to_vec();

    match tokio::task::spawn_blocking(move || {
        platform::dispatch(&blocking_method, &blocking_state, &blocking_body)
    })
    .await
    {
        Ok(Some(Ok(output))) => {
            ([(header::CONTENT_TYPE, "application/binary")], output).into_response()
        }
        Ok(Some(Err(message))) => (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
        Ok(None) => {
            tracing::debug!("未注册的接口: {method}");
            (StatusCode::NOT_FOUND, format!("unknown method: {method}")).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "platform task panicked",
        )
            .into_response(),
    }
}

/// 对照 mediasrv 的 `_check_dynamic_request_permissions`。
fn check_post_permission(
    state: &AppState,
    headers: &HeaderMap,
    method: &str,
) -> Result<(), Response> {
    // 内容类型必须是 binary，避免其他来源的透明请求
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    if content_type != "application/binary" {
        tracing::warn!("拒绝非 binary 内容类型的请求: {method}");
        return Err((StatusCode::FORBIDDEN, "unexpected content type").into_response());
    }

    if has_api_access(state, headers) || REVIEWER_ENDPOINTS.contains(&method) {
        return Ok(());
    }

    tracing::warn!("拒绝未授权的 API 访问: {method}");
    Err((StatusCode::FORBIDDEN, "unauthorized").into_response())
}

/// 对照 mediasrv 的 `_have_api_access`。
fn has_api_access(state: &AppState, headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {}", state.api_key);
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}
