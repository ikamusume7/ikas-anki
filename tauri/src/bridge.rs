// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 注入到 WebView 的桥接脚本。
//!
//! 对应 Qt 侧的两件事：
//!
//! 1. **鉴权头**：`qt/aqt/webview.py` 的 `AuthInterceptor` 会给内部 URL
//!    自动附加 `Authorization: Bearer <APIKEY>`。Tauri 没有等价的请求拦截
//!    API，因此这里改为包装 `window.fetch`。
//!
//! 2. **`bridgeCommand` / `pycmd`**：Qt 通过 QWebChannel 注入同名全局函数
//!    （见 `qt/aqt/webview.py` 的 `_bridge_script`），前端大量依赖它触发
//!    宿主侧动作。这里用 HTTP 打通，语义保持「传入命令字符串 + 可选回调」。
//!
//! 脚本在页面任何脚本之前执行，因此 `window.fetch` 的补丁对所有代码生效。

/// 占位符在注入时替换为真实的 API 密钥。
const SCRIPT: &str = r#"
(function () {
    var API_KEY = "__API_KEY__";
    var originalFetch = window.fetch;

    function isInternal(url) {
        return typeof url === "string" && url.indexOf("/_anki/") === 0;
    }

    // 1) 为内部请求补上鉴权头
    window.fetch = function (input, init) {
        try {
            var url = typeof input === "string" ? input : (input && input.url) || "";
            if (isInternal(url)) {
                init = init || {};
                init.headers = Object.assign({}, init.headers, {
                    Authorization: "Bearer " + API_KEY,
                });
            }
        } catch (error) {
            console.error("bridge: failed to attach auth header", error);
        }
        return originalFetch.call(this, input, init);
    };

    // 2) 提供 bridgeCommand / pycmd
    window.bridgeCommand = window.pycmd = function (arg, callback) {
        originalFetch("/_anki/pycmd", {
            method: "POST",
            headers: {
                "Content-Type": "text/plain",
                Authorization: "Bearer " + API_KEY,
            },
            body: String(arg),
        })
            .then(function (response) {
                return response.text();
            })
            .then(function (text) {
                if (!callback || !text) {
                    return;
                }
                try {
                    callback(JSON.parse(text));
                } catch (error) {
                    callback(text);
                }
            })
            .catch(function (error) {
                console.error("bridgeCommand failed:", arg, error);
            });
        return false;
    };

    // 3) 页面就绪后通知宿主
    //
    // 对照 Qt 在 QWebChannel 连接成功后调用 `pycmd("domDone")`。
    // 这里必须等 load 事件，否则 reviewer.js 尚未执行完，_showQuestion 还不存在。
    function notifyReady() {
        window.pycmd("domDone");
    }
    if (document.readyState === "complete") {
        notifyReady();
    } else {
        window.addEventListener("load", notifyReady);
    }
})();
"#;

pub fn script(api_key: &str) -> String {
    SCRIPT.replace("__API_KEY__", api_key)
}
