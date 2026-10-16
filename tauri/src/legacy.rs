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

use anki_i18n::I18n;

use crate::theme::Style;

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
/// - 脚本放在 body 开头，与 Qt 版本一致；
/// - `<html>` / `<body>` 带主题类，并内联 `standard_css()`（字体、缩放、`--canvas`）——
///   对照 `webview.py:638-692` 与 `730-755`；样式顺序也照上游：webview.css → standard_css
///   → 各页面的隔离样式。
pub fn render(
    port: u16,
    title: &str,
    body: &str,
    css: &[&str],
    js: &[&str],
    style: &Style,
    extra_css: &str,
) -> String {
    let base = format!("http://127.0.0.1:{port}/");

    let mut head = format!("<base href=\"{base}\">\n");
    for sheet in css {
        head.push_str(&format!(
            "    <link rel=\"stylesheet\" type=\"text/css\" href=\"{base}_anki/{sheet}\">\n"
        ));
    }
    // `standard_css()` 紧跟 webview.css（两者都在 <link> 之后、隔离样式之前），
    // 与上游的层叠顺序一致（`webview.py:716-724`）
    head.push_str(&format!(
        "    <style>{}</style>\n",
        style.standard_css()
    ));
    // 各页面的隔离样式放在最后：同优先级下后者生效，用于隔离共用页面带来的污染
    if !extra_css.is_empty() {
        head.push_str(&format!("    <style>{extra_css}</style>\n"));
    }

    let mut scripts = String::new();
    for script in js {
        scripts.push_str(&format!(
            "<script src=\"{base}_anki/{script}\"></script>\n"
        ));
    }

    // 夜间：`<html class="night-mode" data-bs-theme="dark">`（`webview.py:732-746`）
    let html_attrs = if style.night {
        " class=\"night-mode\" data-bs-theme=\"dark\""
    } else {
        " data-bs-theme=\"light\""
    };

    format!(
        "<!doctype html>\n\
         <html dir=\"ltr\"{html_attrs}>\n\
         <head>\n\
         \x20   <meta charset=\"utf-8\">\n\
         \x20   <title>{title}</title>\n\
         {head}</head>\n\
         <body class=\"{body_class}\">\n\
         {scripts}{body}</body>\n\
         </html>\n",
        body_class = style.body_class()
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
pub fn reviewer_page(port: u16, style: &Style) -> u32 {
    // `#top-toolbar` 由 `crate::reviewer` 在会话开始时填内容（`install_toolbar()`），
    // 对照 Qt 主窗口那条常驻工具栏（`qt/aqt/toolbar.py`）
    let body = r#"<div id="top-toolbar"></div>
<div id="_mark" hidden>&#x2605;</div>
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
            // 仅保留底部栏自己的样式；`toolbar-bottom.css` 在 Qt 里也属于底部
            // WebView（`qt/aqt/toolbar.py`、`reviewer.py:360`），它只带来
            // `body { overflow: hidden }`，会让长卡片无法滚动，故不加载。
            "css/reviewer-bottom.css",
        ],
        // reviewer-bottom.js 提供全局的 showQuestion / showAnswer（底部栏更新）
        &["js/webview.js", "js/reviewer.js", "js/reviewer-bottom.js"],
        style,
        REVIEWER_ISOLATION_CSS,
    );
    register(html)
}

/// 录音对话框（对照 `qt/aqt/sound.py` 的 `RecordDialog`）。
///
/// Qt 用 QDialog + QTimer 显示计时，另有保存/取消两个按钮；这里用宿主自建的页面，
/// 按钮通过 `bridgeCommand` 回传决定（`saveRecording` / `cancelRecording`）。
///
/// `secs_placeholder` 会被填进翻译串的秒数位置，再由页面脚本按帧替换 —— Qt 是每帧
/// 重新调 `tr.media_recordingtime(secs=...)`，而这里的页面是纯静态 HTML。
pub fn recorder_page(port: u16, i18n: &I18n, secs_placeholder: &str, style: &Style) -> u32 {
    let label = i18n.media_recordingtime(secs_placeholder);
    let save = i18n.actions_save();
    let cancel = i18n.actions_cancel();

    let body = format!(
        r#"<div id="recorder">
  <div id="dot"></div>
  <div id="label">{label}</div>
  <div id="buttons">
    <button id="save">{save}</button>
    <button id="cancel">{cancel}</button>
  </div>
</div>
<script>
(function () {{
    var placeholder = "{secs_placeholder}";
    var label = document.getElementById("label");
    var template = label.innerHTML;
    var started = Date.now();
    function tick() {{
        var secs = ((Date.now() - started) / 1000).toFixed(1);
        label.innerHTML = template.split(placeholder).join(secs);
    }}
    tick();
    setInterval(tick, 100);
    document.getElementById("save").addEventListener("click", function () {{
        pycmd("saveRecording");
    }});
    document.getElementById("cancel").addEventListener("click", function () {{
        pycmd("cancelRecording");
    }});
}})();
</script>
"#
    );
    register(render(port, "Anki", &body, &[], &[], style, RECORDER_CSS))
}

/// 预览窗口的页面（对照 `qt/aqt/browser/previewer.py:145-160`）。
///
/// Qt 的预览器用 reviewer 的 WebView + reviewer.js 渲染，按钮在原生
/// QDialogButtonBox 里；这里按钮放进页面（`#preview-buttons` 固定到底部），
/// 通过 `pycmd` 回传给 `crate::preview`。
///
/// 按钮文案：重播音频取自 `actions-replay-audio`（核心 ftl）；
/// 上一张/下一张沿用 Qt 的 `<` / `>` 字形（Qt 用的是箭头符号，没有文案）。
pub fn preview_page(port: u16, i18n: &I18n, style: &Style) -> u32 {
    let replay = i18n.actions_replay_audio();
    // 「仅背面」的文案在 `ftl/qt` 仓库里，类型化方法只覆盖核心字符串，
    // 所以用底层的 `translate`（字符串表本身包含全部模块）
    let back_side_only = i18n.translate("qt-misc-back-side-only", None);
    let body = format!(
        r#"<div id="_mark" hidden>&#x2605;</div>
<div id="_flag" hidden>&#x2691;</div>
<div id="qa" dir="auto"></div>
<div id="preview-buttons">
  <button id="prev" title="">&lt;</button>
  <button id="next" title="">&gt;</button>
  <button id="replay">{replay}</button>
  <label id="both-label">
    <input type="checkbox" id="both" /> {back_side_only}
  </label>
</div>
<script>
(function () {{
    // 对照 `previewer.py:267-275` 的注入函数名：reviewer.js 提供 _showQuestion/_showAnswer
    window.previewSetState = function (showingAnswer, bothSides) {{
        document.body.dataset.state = showingAnswer ? "answer" : "question";
        document.getElementById("both").checked = !!bothSides;
    }};
    document.getElementById("both").addEventListener("change", function (event) {{
        pycmd("previewBothSides:" + (event.target.checked ? 1 : 0));
    }});
    document.getElementById("prev").addEventListener("click", function () {{
        pycmd("previewPrev");
    }});
    document.getElementById("next").addEventListener("click", function () {{
        pycmd("previewNext");
    }});
    document.getElementById("replay").addEventListener("click", function () {{
        pycmd("previewReplay");
    }});
    document.addEventListener("keydown", function (event) {{
        // 焦点在按钮上时，Space/Enter 会先被浏览器解释成"点击"，交给 click 处理器，
        // 否则同一次按键会发两条命令
        var onButton =
            document.activeElement && document.activeElement.tagName === "BUTTON";
        if (event.key === "r" || event.key === "R") {{
            pycmd("previewReplay");
        }} else if (event.key === "b" || event.key === "B") {{
            // 对照 `previewer.py:107-115` 的「仅背面」快捷键
            var both = document.getElementById("both");
            both.checked = !both.checked;
            pycmd("previewBothSides:" + (both.checked ? 1 : 0));
        }} else if (event.key === "ArrowLeft") {{
            pycmd("previewPrev");
        }} else if (event.key === "ArrowRight") {{
            event.preventDefault();
            pycmd("previewNext");
        }} else if ((event.key === "Enter" || event.key === " ") && !onButton) {{
            event.preventDefault();
            pycmd("previewNext");
        }} else if (
            event.key === "Escape" ||
            (event.ctrlKey && event.shiftKey && (event.key === "p" || event.key === "P"))
        ) {{
            pycmd("close");
        }}
    }});
}})();
</script>
"#
    );
    let html = render(
        port,
        "Anki",
        &body,
        &["css/webview.css", "css/reviewer.css"],
        &["js/webview.js", "js/reviewer.js"],
        style,
        PREVIEW_ISOLATION_CSS,
    );
    register(html)
}

/// 预览页的隔离样式：语义与 reviewer 页一致，只是把按钮栏固定到底部。
const PREVIEW_ISOLATION_CSS: &str = r#"
body {
    /* reviewer.scss 的值 */
    margin: 20px;
    padding-bottom: 52px;
    overflow-y: auto;
}

#preview-buttons {
    position: fixed;
    inset: auto 0 0 0;
    display: flex;
    gap: 8px;
    justify-content: center;
    align-items: center;
    padding: 8px;
    background: var(--canvas);
    border-top: 1px solid var(--border-subtle);
}

#preview-buttons button {
    min-width: 44px;
    margin: 0;
    padding: 4px 10px;
}

#both-label {
    display: flex;
    gap: 4px;
    align-items: center;
    white-space: nowrap;
}
"#;

/// 录音对话框的样式。只依赖系统颜色（`canvas` / `canvastext`），
/// 这样浅色与深色主题下都能跟原生对话框接近。
const RECORDER_CSS: &str = r#"
html, body { height: 100%; }
body {
    margin: 0;
    background: canvas;
    color: canvastext;
    font-family: system-ui, sans-serif;
}
#recorder {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 14px;
    height: 100%;
    box-sizing: border-box;
    padding: 16px;
}
#dot {
    width: 16px;
    height: 16px;
    border-radius: 50%;
    background: #d33;
    animation: pulse 1s ease-in-out infinite;
}
@keyframes pulse {
    50% { opacity: 0.25; }
}
#label { text-align: center; line-height: 1.5; }
#buttons { display: flex; gap: 8px; }
button { min-width: 88px; padding: 4px 12px; }
"#;

/// 单页面方案特有的隔离样式。
///
/// Qt 把底部工具栏放在**独立的 WebView** 里（`qt/aqt/reviewer.py:358`），它加载的
/// `reviewer-bottom.css` 里有下面这条规则：
///
/// ```css
/// body { margin: 0; padding: 0; }
/// ```
///
/// 在 Qt 里这只作用于底部栏自己的 body，但本方案两个界面共用一个页面，
/// 于是它盖掉了 `reviewer.scss` 的 `body { margin: 20px }` —— 卡片内容会紧贴
/// 窗口左上角（已由截图与 DOM 度量证实）。同理，底部栏在 Qt 里是占据窗口底部的
/// 独立控件，共用一个页面后需要显式固定，否则它会留在文档流里、悬在卡片下方。
///
/// 因此这里把页面级语义恢复成 `webview.css` / `reviewer.css` 的，并把底部栏
/// 固定到窗口底部。底部栏高度由 `crate::reviewer` 在注入后写入
/// `--bottom-bar-height`，避免写死数值。
const REVIEWER_ISOLATION_CSS: &str = r#"
body {
    /* reviewer.scss 的值，被 reviewer-bottom.css 的 body 规则盖掉了 */
    margin: 20px;
    /* 底部栏固定在窗口底部，为卡片区留出等高的空间，避免内容被遮住 */
    padding-bottom: var(--bottom-bar-height, 80px);
    /* 顶部工具栏同理（`crate::reviewer::install_toolbar` 注入后写入真实高度） */
    padding-top: var(--top-bar-height, 2.4rem);
    /* 长卡片需要能滚动（toolbar-bottom.css 的 overflow: hidden 已不再加载） */
    overflow-y: auto;
}

#top-toolbar {
    /* 对照 Qt：主窗口的工具栏是内容之上的一条独立控件 */
    position: fixed;
    inset: 0 0 auto 0;
    background: var(--canvas);
}

#bottom-container {
    /* 对照 Qt：底部栏是占据窗口底部的独立控件 */
    position: fixed;
    inset: auto 0 0 0;
    background: var(--canvas);
    overflow: hidden;
}

/* `reviewer-bottom.css` 还给 button 设了 min-width/margin（Qt 里只影响底部栏），
   这里把卡片内的按钮复位成 `reviewer.scss` 的语义 */
#qa button {
    min-width: revert;
    margin: 1em 0.5em;
}
"#;
