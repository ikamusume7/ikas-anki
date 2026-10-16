// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 静态资源与媒体文件服务。
//!
//! 对照 `qt/aqt/mediasrv.py` 的 `_extract_internal_request`、`_extract_request`、
//! `_handle_builtin_file_request`、`get_sveltekit_route` 以及整套 CSP 策略。

use std::path::Path;
use std::path::PathBuf;
use std::sync::LazyLock;

use axum::extract::State;
use axum::http::header::CONTENT_SECURITY_POLICY;
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::http::HeaderValue;
use axum::http::Method;
use axum::http::StatusCode;
use axum::http::Uri;
use axum::response::IntoResponse;
use axum::response::Response;
use regex::Regex;

use crate::state::AppState;

/// 需要走 SvelteKit 的页面路由（对照 mediasrv 的 `get_sveltekit_route`）。
const SVELTEKIT_ROUTES: &[&str] = &[
    "graphs",
    "congrats",
    "card-info",
    "change-notetype",
    "deck-options",
    "import-anki-package",
    "import-csv",
    "import-page",
    "image-occlusion",
    "preferences",
    "editor",
    // 本外壳新写的页面（上游的浏览器是原生 Qt 界面），见 `crate::browser`
    "browser",
    // 本外壳新写的页面（上游的牌组列表也是原生 Qt 界面），见 `crate::main_window`
    "decks",
    // 自定义学习（上游是 Qt 对话框）
    "custom-study",
    // 字段对话框（上游是 Qt 对话框）
    "fields",
    // 卡片模板对话框（上游是 Qt 对话框）
    "cards",
    // 「管理笔记类型」对话框（上游是 Qt 的 Models 对话框）
    "notetypes",
    // 「切换配置」窗口（上游是 Qt 的 ProfileManager 窗口），见 `crate::profiles`
    "profiles",
];

/// 展示用户内容的路由，需要更严格的 CSP（对照 mediasrv 的
/// `is_untrusted_sveltekit_route`）。
///
/// `browser` 不在上游的名单里（上游没有这个页面），但它同样在展示集合内容
/// （表格里的每一格都是卡片字段，`rslib/src/browser_table.rs` 转成的纯文本），
/// 因此按不可信页面处理，代价只是多一份 script hash 校验。`decks` 同理：牌组名也是
/// 用户内容（同样是纯文本渲染）。
const UNTRUSTED_SVELTEKIT_ROUTES: &[&str] = &[
    "editor",
    "image-occlusion",
    "browser",
    "decks",
    "custom-study",
    // 字段名与描述都是用户内容
    "fields",
    // 模板与样式也是用户内容（模板里可以放任意 HTML/JS）
    "cards",
    // 笔记类型的名字是用户内容
    "notetypes",
];

/// 对照 mediasrv 的 `TRUSTED_PAGE_CSP`。
const TRUSTED_PAGE_CSP: &str = "frame-ancestors 'none'";

/// 对照 mediasrv 的 `UNTRUSTED_MEDIA_CSP`：集合媒体是不可信的用户内容，
/// 不允许其作为活动文档执行。
const UNTRUSTED_MEDIA_CSP: &str = concat!(
    "default-src 'none'; ",
    "script-src 'none'; ",
    "connect-src 'none'; ",
    "object-src 'none'; ",
    "frame-src 'none'; ",
    "child-src 'none'; ",
    "base-uri 'none'; ",
    "form-action 'none'; ",
    "style-src 'self' 'unsafe-inline'; ",
    "img-src 'self'; ",
    "font-src 'self'; ",
    "media-src 'self'; ",
    "sandbox allow-same-origin",
);

/// 对照 mediasrv 的 `_SVELTEKIT_CSP_META_RE`；第一个捕获组是 script hash。
static SVELTEKIT_CSP_META: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"<meta http-equiv="content-security-policy" content="script-src 'self' ('sha256-[A-Za-z0-9+/=]+')">"#,
    )
    .expect("CSP 正则无效")
});

pub async fn favicon(State(state): State<AppState>) -> Response {
    serve_asset(&state, "imgs/favicon.ico", None).await
}

pub async fn handle_fallback(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
) -> Response {
    if method != Method::GET {
        return (StatusCode::METHOD_NOT_ALLOWED, "仅支持 GET").into_response();
    }

    let decoded = percent_decode(uri.path());
    let path = decoded.trim_start_matches('/');
    let head = path.split('/').next().unwrap_or_default();

    // 内置资源、SvelteKit 页面路由（editor / graphs / deck-options 等）
    if path.is_empty()
        || path.starts_with("_anki/")
        || path.starts_with("_app/")
        || SVELTEKIT_ROUTES.contains(&head)
    {
        let (relative, route) = rewrite(path);
        return serve_asset(&state, &relative, route).await;
    }

    // 其余按集合媒体文件处理（对照 mediasrv 的 LocalFileRequest）
    serve_media(&state, path).await
}

/// 从 `qt/aqt/data/web` 提供内置资源。
async fn serve_asset(state: &AppState, relative: &str, route: Option<&str>) -> Response {
    let relative = sveltekit_fallback(relative);

    // 依次尝试：构建产物目录 → 源目录
    let mut roots = vec![state.paths.web.as_path()];
    if let Some(source) = state.paths.web_source.as_deref() {
        roots.push(source);
    }
    for root in roots {
        if let Some(path) = safe_join(root, &relative) {
            if let Some(response) = read_asset(&path, route, state).await {
                return response;
            }
        }
    }

    // 开发期回退：前端产物可能还停留在 out/sveltekit
    if let Some(rest) = relative.strip_prefix("sveltekit/") {
        if let Some(path) = safe_join(&state.paths.sveltekit, rest) {
            if let Some(response) = read_asset(&path, route, state).await {
                return response;
            }
        }
    }

    tracing::debug!("未找到静态资源: {relative}");
    (StatusCode::NOT_FOUND, format!("not found: {relative}")).into_response()
}

/// 从集合的媒体目录提供文件。
async fn serve_media(state: &AppState, relative: &str) -> Response {
    let Some(folder) = state.media_folder.as_ref() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "集合尚未打开").into_response();
    };

    let Some(path) = safe_join(folder.as_ref().as_path(), relative) else {
        return (StatusCode::FORBIDDEN, "非法路径").into_response();
    };

    let Some(bytes) = read_bytes(&path).await else {
        // 媒体缺失是常态（卡片引用了未下载的文件），保持静默
        return (StatusCode::NOT_FOUND, format!("not found: {relative}")).into_response();
    };

    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
        headers.insert(CONTENT_TYPE, value);
    }
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(UNTRUSTED_MEDIA_CSP),
    );
    (headers, bytes).into_response()
}

async fn read_bytes(path: &Path) -> Option<Vec<u8>> {
    tokio::fs::read(path).await.ok()
}

/// 读取资源并按页面信任级别设置 CSP。
async fn read_asset(path: &Path, route: Option<&str>, state: &AppState) -> Option<Response> {
    let bytes = read_bytes(path).await?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();

    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_str(mime.as_ref()).ok()?);

    if mime.essence_str() != "text/html" {
        return Some((headers, bytes).into_response());
    }

    // 对照 mediasrv `_handle_builtin_file_request`：
    // - 不可信页面保留 SvelteKit 写入的 CSP meta（含 script hash），
    //   并叠加一份带来源白名单的策略；
    // - 可信页面则移除该 meta，只保留 frame-ancestors。
    if route.is_some_and(is_untrusted_route) {
        let hash = sveltekit_script_hash(&bytes);
        let csp = untrusted_page_csp(state.port(), hash.as_deref());
        if let Ok(value) = HeaderValue::from_str(&csp) {
            headers.insert(CONTENT_SECURITY_POLICY, value);
        }
        return Some((headers, bytes).into_response());
    }

    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(TRUSTED_PAGE_CSP),
    );
    let body = if path.ends_with("index.html") {
        strip_sveltekit_csp(&bytes)
    } else {
        bytes
    };
    Some((headers, body).into_response())
}

fn is_untrusted_route(route: &str) -> bool {
    UNTRUSTED_SVELTEKIT_ROUTES.contains(&route)
}

/// 对照 mediasrv 的 `_untrusted_sveltekit_content_security_policy`。
fn untrusted_page_csp(port: u16, script_hash: Option<&str>) -> String {
    let mut sources = String::new();
    for path in ["_anki/", "_app/", "_addons/"] {
        sources.push_str(&format!("http://127.0.0.1:{port}/{path} "));
    }
    if let Some(hash) = script_hash {
        sources.push_str(hash);
    }
    format!(
        "script-src {}; form-action 'none'; frame-ancestors 'none'",
        sources.trim_end()
    )
}

fn sveltekit_script_hash(html: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(html);
    SVELTEKIT_CSP_META
        .captures(&text)
        .map(|captures| captures[1].to_string())
}

fn strip_sveltekit_csp(html: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(html);
    SVELTEKIT_CSP_META.replace_all(&text, "").into_owned().into_bytes()
}

/// 把请求路径重写为相对于 `qt/aqt/data/web` 的路径，并返回命中的页面路由。
///
/// 对照 mediasrv `_extract_internal_request` 的重写规则。
fn rewrite(path: &str) -> (String, Option<&'static str>) {
    let head = path.split('/').next().unwrap_or_default();

    if let Some(route) = SVELTEKIT_ROUTES
        .iter()
        .find(|route| **route == head)
        .copied()
    {
        return (format!("sveltekit/_app/{path}"), Some(route));
    }
    if let Some(rest) = path.strip_prefix("_app/") {
        return (format!("sveltekit/_app/{rest}"), None);
    }
    if let Some(rest) = path.strip_prefix("_anki/") {
        // 仅顶层的 legacy 引用需要重映射
        if !rest.contains('/') {
            if let Some(stem) = rest.strip_suffix(".css") {
                return (format!("css/{stem}.css"), None);
            }
            if let Some(stem) = rest.strip_suffix(".js") {
                let vendored = matches!(stem, "jquery" | "jquery-ui" | "plot");
                let mapped = if vendored {
                    format!("js/vendor/{stem}.js")
                } else {
                    format!("js/{stem}.js")
                };
                return (mapped, None);
            }
        }
        return (rest.to_string(), None);
    }
    (path.to_string(), None)
}

/// SvelteKit 的前端路由回退：非 immutable 资源一律返回 index.html。
fn sveltekit_fallback(path: &str) -> String {
    if path.starts_with("sveltekit/") && !path.contains("immutable") {
        return "sveltekit/index.html".to_string();
    }
    path.to_string()
}

/// 拼接路径并阻止目录穿越。
///
/// 对照 mediasrv 的 `ensure_safe_path`。
fn safe_join(base: &Path, relative: &str) -> Option<PathBuf> {
    let mut out = PathBuf::from(base);
    for part in relative.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        // 拒绝父目录、Windows 盘符与反斜杠
        if part == ".." || part.contains('\\') || part.contains(':') {
            tracing::warn!("拒绝可疑路径: {relative}");
            return None;
        }
        out.push(part);
    }
    Some(out)
}

fn percent_decode(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .into_owned()
}
