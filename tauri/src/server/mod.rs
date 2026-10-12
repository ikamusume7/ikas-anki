// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 本地 HTTP 服务。
//!
//! 这是 `qt/aqt/mediasrv.py` 的 Rust 复刻版。之所以保留 HTTP 而不是改用
//! Tauri IPC，是因为前端（`ts/`）全部使用 `/_anki/...` 绝对路径加载资源与
//! 调用接口，保留 HTTP 层可以让那 250 个 Svelte 组件**零改动**运行。

pub mod api;
pub mod assets;
pub mod legacy;
pub mod pycmd;

use std::net::SocketAddr;

use axum::extract::Request;
use axum::extract::State;
use axum::http::header;
use axum::http::StatusCode;
use axum::middleware::from_fn;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::get;
use axum::routing::post;
use axum::Router;

use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/favicon.ico", get(assets::favicon))
        .route("/_anki/readyz", get(readyz))
        // 静态段优先于参数段，因此不会与下面的 {method} 冲突
        .route("/_anki/pycmd", post(pycmd::handle))
        .route("/_anki/legacyPageData", get(legacy::page_data))
        .route("/_anki/{method}", post(api::handle_rpc))
        .fallback(assets::handle_fallback)
        .layer(from_fn(reject_non_local))
        .with_state(state)
}

/// 对照 mediasrv `handle_request` 开头的 Host / Origin 校验。
///
/// 只接受本机来源，避免同机上的其他页面访问本地 API。
async fn reject_non_local(request: Request, next: Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok());

    match host {
        Some(host) if is_local_host(host) => {}
        other => {
            tracing::warn!("拒绝非本地 Host: {other:?}");
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }

    if let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    {
        if !is_local_origin(origin) {
            tracing::warn!("拒绝非本地 Origin: {origin}");
            return (StatusCode::FORBIDDEN, "forbidden").into_response();
        }
    }

    next.run(request).await
}

/// 对照 mediasrv 的 `_ALLOWED_ORIGIN_PREFIXES`：host 必须带端口。
fn is_local_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    ["127.0.0.1:", "localhost:", "[::1]:"]
        .iter()
        .any(|prefix| host.starts_with(prefix))
}

fn is_local_origin(origin: &str) -> bool {
    let origin = origin.to_ascii_lowercase();
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|host| {
        ["http://", "https://"].iter().any(|scheme| {
            let base = format!("{scheme}{host}");
            origin == base
                || origin.starts_with(&format!("{base}:"))
                || origin.starts_with(&format!("{base}/"))
        })
    })
}

/// 对应 mediasrv 的 `/_anki/readyz`：报告集合是否已打开。
async fn readyz(State(state): State<AppState>) -> StatusCode {
    if state.is_collection_open() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// 在后台线程启动服务，阻塞直到端口绑定成功并返回实际端口。
pub fn spawn(state: AppState) -> anyhow::Result<u16> {
    let (tx, rx) = std::sync::mpsc::channel::<SocketAddr>();
    let serving_state = state.clone();

    std::thread::Builder::new()
        .name("anki-mediasrv".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("构建 tokio 运行时失败");

            runtime.block_on(async move {
                // 对齐 mediasrv：允许通过 ANKI_API_PORT 指定固定端口，便于调试与 e2e。
                let desired_port: u16 = std::env::var("ANKI_API_PORT")
                    .ok()
                    .and_then(|port| port.parse().ok())
                    .unwrap_or(0);

                let listener = tokio::net::TcpListener::bind(("127.0.0.1", desired_port))
                    .await
                    .expect("绑定本地端口失败");
                let addr = listener.local_addr().expect("获取本地地址失败");
                tx.send(addr).expect("回传端口失败");

                tracing::info!("本地服务已启动: http://{addr}");
                axum::serve(listener, router(serving_state))
                    .await
                    .expect("HTTP 服务异常退出");
            });
        })?;

    let port = rx.recv()?.port();
    // CSP 里需要带上端口，因此绑定成功后回填
    state.set_port(port);
    Ok(port)
}
