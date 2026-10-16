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
//!
//! 3. **窗口身份**：多窗口模型下（见 `crate::windows`）同一个进程里有多个页面
//!    在发桥接命令，而 `POST /_anki/pycmd` 是一条共用的通道。Qt 里每个 WebView
//!    有自己的 `AnkiWebPage` 对象，命令天然归属该窗口；这里改为让脚本在每个
//!    请求上带 `X-Anki-Window: <窗口标签>`，宿主据此判断命令来自哪个窗口。
//!    没有这个头，任意页面加载时发的 `domDone` 都会被当成复习界面启动。

/// 占位符在注入时替换为真实的 API 密钥与窗口标签。
const SCRIPT: &str = r#"
(function () {
    var API_KEY = "__API_KEY__";
    var WINDOW_LABEL = "__WINDOW_LABEL__";
    var originalFetch = window.fetch;

    function isInternal(url) {
        return typeof url === "string" && url.indexOf("/_anki/") === 0;
    }

    function internalHeaders(extra) {
        return Object.assign(
            {
                Authorization: "Bearer " + API_KEY,
                "X-Anki-Window": WINDOW_LABEL,
            },
            extra,
        );
    }

    // 1) 为内部请求补上鉴权头与窗口身份
    window.fetch = function (input, init) {
        try {
            var url = typeof input === "string" ? input : (input && input.url) || "";
            if (isInternal(url)) {
                init = init || {};
                init.headers = internalHeaders(init.headers);
            }
        } catch (error) {
            console.error("bridge: failed to attach auth header", error);
        }
        return originalFetch.call(this, input, init);
    };

    // 2) 提供 bridgeCommand / pycmd
    window.bridgeCommand = window.pycmd = function (arg, callback) {
        // 类型作答：把输入框内容附在命令后面（外壳侧的适配，前端代码不用改）。
        // 对照 Qt：那边用 evalWithCallback("getTypedAnswer();") 取回输入值。
        if (arg === "ans") {
            var input = document.getElementById("typeans");
            if (input && typeof input.value === "string") {
                arg = "ans:" + input.value;
            }
        }
        originalFetch("/_anki/pycmd", {
            method: "POST",
            headers: internalHeaders({ "Content-Type": "text/plain" }),
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

pub fn script(api_key: &str, window_label: &str, night: bool) -> String {
    let mut script = SCRIPT
        .replace("__API_KEY__", api_key)
        .replace("__WINDOW_LABEL__", window_label);
    // 宿主能力标记：Windows 上 `<input type="color">` 换成宿主侧的原生取色器
    // （`crate::platform::colours`）。页面据此把点击交给 `pickColour:` 桥接命令；
    // Qt 那边没有这个标记，照旧用 Qt 自己的取色器。见 PARITY 6.5。
    if cfg!(windows) {
        script.push_str("\nwindow.__ankiHostColourPicker = true;\n");
    }
    // 主题：在 document-start 就打上平台类与夜间类。对照 `qt/aqt/webview.py:919-935`
    // 的注入（那边是页面加载完成后 eval 一段同样的类名操作）——早注入是为了避免首帧白闪。
    script.push_str(&crate::theme::window_init_script(night));
    script
}
