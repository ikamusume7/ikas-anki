// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `GET /_anki/legacyPageData?id=N`。
//!
//! 对照 mediasrv 的 `legacy_page_data`：返回宿主先前注册的 HTML，
//! 并为其设置可信页面的 CSP。

use axum::extract::Query;
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use serde::Deserialize;

use crate::legacy;

#[derive(Deserialize)]
pub struct PageQuery {
    id: u32,
}

pub async fn page_data(Query(params): Query<PageQuery>) -> Response {
    let Some(html) = legacy::get(params.id) else {
        tracing::debug!("请求了不存在的 legacy 页面: {}", params.id);
        return (StatusCode::NOT_FOUND, "page not found").into_response();
    };

    // 这些页面由宿主自己组装，不含用户内容，因此按可信页面处理。
    // 编辑器使用的是另一套更严格的策略（见 assets.rs）。
    //
    // `charset=utf-8` 不能省：Qt 那边是 Flask 的 `Response(html, mimetype="text/html")`
    // 由 Werkzeug 自动补上的（见 `qt/aqt/mediasrv.py:1355`）。少了它，含中文的
    // 页面会被浏览器按系统默认编码（中文 Windows 是 GBK）解析成乱码 —— 录音对话框
    // 的中文标签就是这么变成乱码的。
    (
        [
            (CONTENT_TYPE, "text/html; charset=utf-8"),
            (CONTENT_SECURITY_POLICY, "frame-ancestors 'none'"),
        ],
        html,
    )
        .into_response()
}
