// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 复习界面顶部工具栏（对照 Qt 主窗口的 TopWebView，`qt/aqt/toolbar.py:353-389`）。
//
// 宿主注入：`src/reviewer.rs` 的 `install_toolbar()`（`include_str!` 引用本文件，
// 再把 `toolbar_payload()` 生成的 JSON 交给 `installToolbar()`）。
//
// 为什么放成文件：Qt 那份 HTML 在独立的 QWebEngineView 里，注入到 WebView2 的脚本
// 没法从 Playwright 直接看。验证脚本读**同一个文件**塞进空页面测渲染与点击
// （同 `reviewer-menu.js`，见 TAURI_HANDOVER.md §5.27），避免"测的不是跑的那份"。
//
// 与 Qt 的差异：
// - **同步链接有了**（`toolbar.py:410-419`）：点了发 `sync` 给宿主，宿主跑完用
//   `window.__ankiSyncStatus()` 把状态写进工具栏；Qt 那边的状态色与旋转图标这里只有文字；
// - 左右两个托盘（add-ons 挂载点）没做；
// - Qt 的工具栏只在鼠标靠近顶部时隐藏（全屏时用 `hide_if_allowed`），这里常驻。

// 整个文件包一层 IIFE：宿主每次会话开始都会 `eval` 这份脚本体（`reviewer::start`），
// 裸的顶层 `const` 第二次会被判「重复声明」并抛 SyntaxError（虽然工具栏已经画好了）。
(function () {
const TOOLBAR_CSS = `
#top-toolbar {
    position: fixed;
    inset: 0 0 auto 0;
    z-index: 10;
    background: var(--canvas, #fff);
    border-bottom: 1px solid var(--border, #d8d8d8);
}

#top-toolbar .header {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0.35rem 0.75rem;
}

#top-toolbar .hitem {
    color: var(--fg-link, #0a58ca);
    cursor: pointer;
    font-size: 0.95rem;
    text-decoration: none;
}

/* 用 margin 而不是 flex 的 gap：链接是内层 .toolbar 的子元素，间距要落在它们身上；
   上游的 css/toolbar.css 在这个仓库里没有（见 TAURI_HANDOVER.md §5.29） */
#top-toolbar .hitem + .hitem {
    margin-left: 1.25rem;
}

#top-toolbar .hitem:hover {
    text-decoration: underline;
}

/* 同步状态（宿主用 window.__ankiSyncStatus() 往这里写） */
#top-toolbar #sync-status {
    margin-left: 1.25rem;
    color: var(--fg-faint, #666);
    font-size: 0.85rem;
}
`;

/** 把工具栏写进宿主给的容器，并把高度同步给卡片区（`--top-bar-height`）。 */
window.installToolbar = function (data) {
    const host = document.getElementById(data.hostId || "top-toolbar");
    if (!host) {
        return false;
    }
    // 幂等：重复注入（重新报到、页面重载后宿主再注入）不叠加
    if (host.dataset.ready === "1") {
        return true;
    }
    host.dataset.ready = "1";

    const style = document.createElement("style");
    style.textContent = TOOLBAR_CSS;
    document.head.appendChild(style);

    const header = document.createElement("div");
    header.className = "header";
    const center = document.createElement("div");
    center.className = "toolbar";
    for (const link of data.links || []) {
        const item = document.createElement("a");
        item.className = "hitem";
        item.id = link.id;
        item.href = "#";
        item.textContent = link.label;
        item.setAttribute("aria-label", link.label);
        if (link.tip) {
            item.title = link.tip;
        }
        item.onclick = () => {
            window.pycmd(link.cmd);
            return false;
        };
        center.appendChild(item);
    }
    header.appendChild(center);
    host.appendChild(header);

    // 同步状态行（宿主 `sync` 命令跑完会调 `window.__ankiSyncStatus(text)`）
    const status = document.createElement("span");
    status.id = "sync-status";
    center.appendChild(status);
    window.__ankiSyncStatus = (text) => {
        status.textContent = text || "";
    };

    // 与底部栏同一套办法（`reviewer.rs::sync_bottom_bar_height`）：卡片区靠
    // `body { padding-top: var(--top-bar-height) }` 让开，不写死高度
    document.documentElement.style.setProperty(
        "--top-bar-height",
        `${host.offsetHeight}px`,
    );
    return true;
};
})();
