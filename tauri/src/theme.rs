// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 主题与夜间模式（PARITY 8.3）。
//!
//! 对照上游：
//!
//! - 偏好存在 `prefs21.db` 的 `_global` 行里，键 `theme`，取值是 `Theme` 枚举
//!   （`qt/aqt/theme.py:53-56`：0 跟随系统 / 1 浅色 / 2 深色；缺键当 0）；
//! - 是否夜间由 `theme_manager._determine_night_mode()`（`theme.py:211-222`）决定：
//!   跟随系统时 Windows 读注册表 `AppsUseLightTheme`（0 = 深色）；
//! - 生效值有**三条注入通道**（`qt/aqt/webview.py`）：
//!   1. 每个窗口在 document-start 由 `crate::bridge` 打上 `night-mode`（html）与
//!      `night_mode` / `nightMode`（body），避免首帧白闪；
//!   2. legacy 页在服务端拼 HTML 时就带 `<html class="night-mode" data-bs-theme>` 与
//!      body class，并内联 `standard_css()`（`webview.py:638-692`）；
//!   3. SvelteKit 页靠 URL 尾部的 `#night` 让页面自己 `checkNightMode()`
//!      （`ts/lib/tslib/nightmode.ts:6-12`）。
//!
//! 运行期切换（偏好设置里的主题下拉）由 `set_theme()` 复刻
//! `webview.py:989-1013` 的 `on_theme_did_change()`：遍历所有窗口 eval 同一段 JS。
//!
//! 为什么 CSS 变量能直接生效：`ts/lib/sass/_root-vars.scss:23,45` 定义了
//! `:root.night-mode` 的第二套变量，`qt/aqt/data/web/css/webview.css` 里也带着同一份，
//! 所以只要 `<html>` 上有 `night-mode`，颜色就整体切换。

use std::path::Path;

use serde_json::json;
use tauri::Manager as _;

use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
use crate::state::AppState;
// 注意：**不要**在这里 `use crate::windows;` —— 那会把 `windows` 这个名字遮住，
// 而本文件还要用 `windows` crate 的 `Win32::System::Registry`（见 `system_dark_mode`）。
// 宿主模块都走全路径 `crate::windows::…`。

/// `colors.CANVAS`（`ts/lib/sass/_vars.scss` 的 canvas 色板，构建产物里也是这两个值）。
pub const CANVAS_LIGHT: &str = "#f5f5f5";
pub const CANVAS_DARK: &str = "#2c2c2c";

/// 界面 `theme` 偏好。对照 `qt/aqt/theme.py:53-56` 的 `Theme` 枚举。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    FollowSystem,
    Light,
    Dark,
}

impl Theme {
    /// `meta["theme"]` → 枚举（缺键 / 越界都当「跟随系统」）。
    pub fn from_meta(value: Option<i64>) -> Self {
        match value {
            Some(1) => Self::Light,
            Some(2) => Self::Dark,
            _ => Self::FollowSystem,
        }
    }

    pub fn as_i64(self) -> i64 {
        match self {
            Self::FollowSystem => 0,
            Self::Light => 1,
            Self::Dark => 2,
        }
    }
}

/// 这个偏好下到底是不是夜间。对照 `theme.py:211-222`。
pub fn night_mode(theme: Theme) -> bool {
    match theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::FollowSystem => system_dark_mode(),
    }
}

/// 系统是否处于深色模式。
///
/// Windows：注册表 `HKCU\...\Themes\Personalize` 的 `AppsUseLightTheme`（0 表示深色），
/// 与 Qt 的 `get_windows_dark_mode()`（`theme.py:340-359`）读的是同一个键。
/// 其他平台目前返回 `false`（Qt 在 Linux 上走 dbus / gsettings，未移植）。
#[cfg(windows)]
pub fn system_dark_mode() -> bool {
    use windows::core::w;
    use windows::Win32::System::Registry::RegGetValueW;
    use windows::Win32::System::Registry::HKEY_CURRENT_USER;
    use windows::Win32::System::Registry::RRF_RT_REG_DWORD;

    let mut light: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(std::ptr::from_mut(&mut light).cast()),
            Some(std::ptr::from_mut(&mut size)),
        )
    };
    status.is_ok() && light == 0
}

#[cfg(not(windows))]
pub fn system_dark_mode() -> bool {
    false
}

/// 一个页面用到的主题相关样式（legacy 页需要在服务端拼 HTML 时用到）。
#[derive(Clone, Debug)]
pub struct Style {
    pub night: bool,
    /// `meta["uiScale"]`（默认 1.0）。对照 Qt 的 `pm.uiScale()`。
    pub ui_scale: f64,
    /// 界面字体（Windows 上是 `qt-misc-segoe-ui`，其余平台留空表示不覆盖）。
    pub font_family: String,
}

impl Style {
    /// body 上的类名。对照 `theme.py:163-191` 的 `body_class()`：
    /// 平台类恒有，夜间追加 `nightMode` / `night_mode`；`fancy` 表示非极简模式。
    pub fn body_class(&self) -> String {
        let mut classes = vec![platform_class().to_owned(), "fancy".to_owned()];
        if self.night {
            classes.push("nightMode".to_owned());
            classes.push("night_mode".to_owned());
        }
        classes.join(" ")
    }

    /// 卡片页面的 body class。对照 `theme_manager.body_classes_for_card_ord(ord)`：
    /// 前面再加上 `card cardN`（`ord` 从 0 开始，类名从 1 开始）。
    ///
    /// **必须传全**：`ts/reviewer/index.ts` 里是 `document.body.className = bodyclass`
    /// （整串覆盖），传空串会把页面上打好的类全清掉。
    pub fn body_classes_for_card_ord(&self, ord: u32) -> String {
        format!("card card{} {}", ord + 1, self.body_class())
    }

    /// 对照 `AnkiWebView.standard_css()`（`webview.py:638-692`）。
    ///
    /// 只保留与网页有关的部分：`--canvas` 的深浅两套、字体、系统字号（配合
    /// `:root.system-font-size`）与 `zoom`。Qt 里那些按 DPI 算字号、macOS 专属按钮样式
    /// 的细节没有等价物，这里用 `uiScale` 代替。
    pub fn standard_css(&self) -> String {
        let font_size = 16.0 * self.ui_scale;
        let mut css = format!(
            ":root {{ --canvas: {CANVAS_LIGHT}; }}\n\
             :root[class*=night-mode] {{ --canvas: {CANVAS_DARK}; }}\n"
        );
        if !self.font_family.is_empty() {
            css.push_str(&format!(
                "html {{ font-family: \"{family}\"; }}\n\
                 button {{ font-family: \"{family}\"; }}\n",
                family = self.font_family
            ));
        }
        css.push_str(&format!(
            ":root.system-font-size, :root.night-mode.system-font-size {{ \
             font-size: {font_size}px; --bs-body-font-size: {font_size}px; \
             --font-size: {font_size}px; }}\n"
        ));
        css.push_str(&format!(
            "body {{ zoom: {zoom}; background-color: var(--canvas); }}\n",
            zoom = self.ui_scale
        ));
        css
    }

    /// 窗口底色（对照 `webview.py:991` 的 `page().setBackgroundColor(CANVAS)`，
    /// 用来避免换主题时的白闪）。
    pub fn canvas_rgba(&self) -> (u8, u8, u8, u8) {
        if self.night {
            (0x2c, 0x2c, 0x2c, 0xff)
        } else {
            (0xf5, 0xf5, 0xf5, 0xff)
        }
    }
}

/// 平台类名，对照 `theme.py` 的 `isWin` / `isMac` / `isLin`。
fn platform_class() -> &'static str {
    if cfg!(windows) {
        "isWin"
    } else if cfg!(target_os = "macos") {
        "isMac"
    } else {
        "isLin"
    }
}

/// 读 profile 配置里的主题与界面缩放（缺键时分别是「跟随系统」与 1.0）。
pub fn read_prefs(profile_dir: &Path) -> (Theme, f64) {
    let Some(prefs) = Prefs21::locate(profile_dir) else {
        return (Theme::FollowSystem, 1.0);
    };
    let theme = Theme::from_meta(
        prefs
            .get(Store::Meta, "theme")
            .ok()
            .flatten()
            .and_then(|v| v.as_i64()),
    );
    let ui_scale = prefs
        .get(Store::Meta, "uiScale")
        .ok()
        .flatten()
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0)
        // 对照 `profiles.py:535-537`：读的时候下钳到 1
        .max(1.0);
    (theme, ui_scale)
}

/// 现算一份当前生效的样式（宿主各处拼页面/建窗口时用）。
pub fn style(state: &AppState) -> Style {
    let (_, ui_scale) = read_prefs(&state.profile_dir);
    Style {
        night: state.night_mode(),
        ui_scale,
        font_family: font_family(state),
    }
}

/// 界面字体。Windows 上与 Qt 一样用 `qt-misc-segoe-ui` 的文案（`webview.py:644-648`）。
fn font_family(state: &AppState) -> String {
    if cfg!(windows) {
        state
            .backend
            .i18n()
            .translate("qt-misc-segoe-ui", None)
            .into_owned()
    } else {
        String::new()
    }
}

/// 窗口背景色（建窗时用）。对照 `webview.py:991`。
pub fn window_color(state: &AppState) -> tauri::utils::config::Color {
    let (r, g, b, a) = style(state).canvas_rgba();
    tauri::utils::config::Color(r, g, b, a)
}

/// 运行期切换主题：落库 → 更新内存 → 广播到所有窗口。
///
/// 对照 `preferences.py:420-421` → `main.py:1171-1173` 的 `mw.set_theme()`。
pub fn set_theme(state: &AppState, value: i64) -> anyhow::Result<bool> {
    let theme = Theme::from_meta(Some(value));
    if let Some(prefs) = Prefs21::locate(state.profile_dir.as_path()) {
        prefs
            .set(Store::Meta, "theme", &json!(theme.as_i64()))
            .map_err(|error| anyhow::anyhow!("写入 theme 失败: {error}"))?;
    }
    let night = night_mode(theme);
    state.set_night_mode(night);
    broadcast(state);
    tracing::info!(
        "主题: theme={} night={night}",
        theme.as_i64()
    );
    Ok(night)
}

/// 把当前夜间状态推给所有已打开的窗口。对照 `webview.py:989-1013`。
pub fn broadcast(state: &AppState) {
    let night = state.night_mode();
    let css = toggle_script(night);
    let (r, g, b, a) = if night {
        (0x2c, 0x2c, 0x2c, 0xff)
    } else {
        (0xf5, 0xf5, 0xf5, 0xff)
    };
    for (label, window) in state.app.webview_windows() {
        // 先换窗口底色，避免切换瞬间白闪（对照 `webview.py:991`）
        if let Err(error) =
            window.set_background_color(Some(tauri::utils::config::Color(r, g, b, a)))
        {
            tracing::debug!("设置窗口 {label} 底色失败: {error}");
        }
        crate::windows::eval(state, &label, &css);
    }
}
/// 切换类名的那段 JS（`broadcast` 与新建窗口时都会用到）。
pub fn toggle_script(night: bool) -> String {
    format!(
        r#"(function () {{
    var doc = document.documentElement;
    if (doc) {{
        if ({night}) {{ doc.dataset.bsTheme = "dark"; doc.classList.add("night-mode"); }}
        else {{ doc.dataset.bsTheme = "light"; doc.classList.remove("night-mode"); }}
    }}
    var body = document.body;
    if (body) {{
        if ({night}) {{ body.classList.add("night_mode"); body.classList.add("nightMode"); }}
        else {{ body.classList.remove("night_mode"); body.classList.remove("nightMode"); }}
    }}
}})();"#
    )
}

/// 建窗时在 **document-start** 注入的那段脚本（`crate::bridge::script` 用）。
///
/// 与 `toggle_script` 的区别：这里还要补平台类，而且 document-start 时
/// `document.documentElement` / `document.body` 可能都还不存在 —— 这两种情况都要兜住，
/// 否则「首帧正常、出卡变浅色」（`ts/reviewer/index.ts` 会整串覆盖 body.className）。
pub fn window_init_script(night: bool) -> String {
    format!(
        "\nwindow.__ankiNightMode = {flag};\n\
         (function () {{\n\
         \x20   function apply() {{\n\
         \x20       var doc = document.documentElement;\n\
         \x20       if (doc) {{\n\
         \x20           doc.dataset.bsTheme = {theme};\n\
         \x20           doc.classList.{doc_op}(\"night-mode\");\n\
         \x20       }}\n\
         \x20       var body = document.body;\n\
         \x20       if (body) {{\n\
         \x20           body.classList.add(\"{platform}\");\n\
         \x20           body.classList.{body_op}(\"night_mode\");\n\
         \x20           body.classList.{body_op}(\"nightMode\");\n\
         \x20       }}\n\
         \x20       return !!(doc && body);\n\
         \x20   }}\n\
         \x20   if (!apply()) {{ document.addEventListener(\"DOMContentLoaded\", apply); }}\n\
         }})();\n",
        flag = night,
        theme = if night { "\"dark\"" } else { "\"light\"" },
        doc_op = if night { "add" } else { "remove" },
        body_op = if night { "add" } else { "remove" },
        platform = platform_class(),
    )
}

/// 夜里窗口打开时给 SvelteKit 页面用的 URL 后缀（对照 `webview.py:948-965`）。
pub fn url_suffix(state: &AppState) -> &'static str {
    if state.night_mode() {
        "#night"
    } else {
        ""
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_from_meta() {
        assert_eq!(Theme::from_meta(None), Theme::FollowSystem);
        assert_eq!(Theme::from_meta(Some(0)), Theme::FollowSystem);
        assert_eq!(Theme::from_meta(Some(1)), Theme::Light);
        assert_eq!(Theme::from_meta(Some(2)), Theme::Dark);
        assert_eq!(Theme::from_meta(Some(99)), Theme::FollowSystem);
        // 明暗与偏好一致（Light/Dark 不依赖系统）
        assert!(!night_mode(Theme::Light));
        assert!(night_mode(Theme::Dark));
    }

    #[test]
    fn body_classes_include_platform_and_night() {
        let light = Style { night: false, ui_scale: 1.0, font_family: String::new() };
        let class = light.body_class();
        assert!(class.contains(platform_class()));
        assert!(!class.contains("nightMode"));
        let dark = Style { night: true, ui_scale: 1.0, font_family: String::new() };
        let class = dark.body_class();
        assert!(class.contains("nightMode") && class.contains("night_mode"));
        // 卡片类必须带全（前端会整串覆盖 body.className）
        assert_eq!(dark.body_classes_for_card_ord(2), format!("card card3 {class}"));
    }

    #[test]
    fn standard_css_carries_both_canvases() {
        let style = Style { night: true, ui_scale: 1.5, font_family: "Segoe UI".to_owned() };
        let css = style.standard_css();
        assert!(css.contains(CANVAS_LIGHT) && css.contains(CANVAS_DARK));
        assert!(css.contains("zoom: 1.5"));
        assert!(css.contains("font-size: 24px"));
        assert!(css.contains("color: var(--canvas)") || css.contains("background-color: var(--canvas)"));
        assert_eq!(style.canvas_rgba(), (0x2c, 0x2c, 0x2c, 0xff));
    }
}
