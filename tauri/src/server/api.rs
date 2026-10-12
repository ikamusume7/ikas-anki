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

use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;

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
