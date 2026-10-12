// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! legacy 页面机制。
//!
//! 对照 Qt 的 `AnkiWebView.stdHtml` + `mediasrv.set_page_html`：
//! 宿主把完整 HTML 注册到服务端，再让 WebView 访问
//! `/_anki/legacyPageData?id=N`。这样做是为了绕开 QWebEngine `setHtml` 的
//! 大小限制，并让服务端统一设置 CSP。
//!
//! 复习屏、卡片布局、预览器等界面都走这条路径，因此它是这些界面的共同前提。

use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::LazyLock;
use std::sync::Mutex;

/// 页面 id 计数。Qt 用 `id(webview)` 作为 key，这里改用递增整数。
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

static PAGES: LazyLock<Mutex<HashMap<u32, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// 注册一个 legacy 页面，返回可用于 `legacyPageData?id=` 的 id。
pub fn register(html: String) -> u32 {
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    PAGES.lock().expect("页面表已中毒").insert(id, html);
    id
}

pub fn get(id: u32) -> Option<String> {
    PAGES.lock().expect("页面表已中毒").get(&id).cloned()
}

/// 组装完整的 legacy 页面。
///
/// 对照 `AnkiWebView.stdHtml`：
/// - `<base>` 指向本地服务（对照 `mw.baseHTML()`）；
/// - css / js 通过 `/_anki/` 下的绝对 URL 引用；
/// - 脚本放在 body 开头，与 Qt 版本一致。
///
/// 未移植的部分：`standard_css()`（动态字体、缩放、主题色）。这些目前由
/// `css/webview.css` 与 `css/reviewer.css` 的默认值兜底。
pub fn render(port: u16, title: &str, body: &str, css: &[&str], js: &[&str]) -> String {
    let base = format!("http://127.0.0.1:{port}/");

    let mut head = format!("<base href=\"{base}\">\n");
    for sheet in css {
        head.push_str(&format!(
            "    <link rel=\"stylesheet\" type=\"text/css\" href=\"{base}_anki/{sheet}\">\n"
        ));
    }

    let mut scripts = String::new();
    for script in js {
        scripts.push_str(&format!(
            "<script src=\"{base}_anki/{script}\"></script>\n"
        ));
    }

    format!(
        "<!doctype html>\n\
         <html dir=\"ltr\">\n\
         <head>\n\
         \x20   <title>{title}</title>\n\
         {head}</head>\n\
         <body>\n\
         {scripts}{body}</body>\n\
         </html>\n"
    )
}

/// 对照 `Reviewer.revHtml()` + `_initWeb()`。
///
/// 卡片内容不在这里：Qt 通过 `web.eval("_showQuestion(...)")` 把渲染结果交给
/// `js/reviewer.js` 注入 `#qa`，这里保持同样的约定。
///
/// 与 Qt 的差异：Qt 把底部工具栏放在**独立的 WebView** 里（`mw.bottomWeb`），
/// 这里放在同一页面。`#bottom-container` 的内容由 `crate::reviewer` 注入，
/// 其中的按钮调用全局的 `pycmd`，因此跨 WebView 的消息传递不需要了。
pub fn reviewer_page(port: u16) -> u32 {
    let body = r#"<div id="_mark" hidden>&#x2605;</div>
<div id="_flag" hidden>&#x2691;</div>
<div id="qa" dir="auto"></div>
<div id="bottom-container"></div>
"#;
    let html = render(
        port,
        "Anki",
        body,
        &[
            "css/webview.css",
            "css/reviewer.css",
            "css/toolbar-bottom.css",
            "css/reviewer-bottom.css",
        ],
        // reviewer-bottom.js 提供全局的 showQuestion / showAnswer（底部栏更新）
        &["js/webview.js", "js/reviewer.js", "js/reviewer-bottom.js"],
    );
    register(html)
}
