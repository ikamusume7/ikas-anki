// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 界面语言：语言列表 + 「用户设置 / 系统语言 → ftl 目录名」的换算（PARITY 1.5 / 8.4）。
//!
//! 对照 `pylib/anki/lang.py`：
//!
//! - `langs = sorted([(显示名, 代码), ...])` —— 偏好设置里语言下拉的选项就来自它。
//!   这里用 `include_str!` 直接嵌入那份 Python 源再解析：它是唯一的真源，在 Rust 里
//!   抄一份迟早会漂移，而打包后 `pylib/` 并不随包分发（字符串是编译期嵌进去的）。
//! - `lang_to_disk_lang()`：`zh_CN` → `zh-CN`（少数「语言_地区」组合在 ftl 目录名里
//!   带连字符），其余语言把地区部分去掉（`de_DE` → `de`）。
//! - `get_def_lang()`：用户设置优先 → 系统语言 → `en_US`。
//!
//! 生效的语言由 `main.rs` 在启动时喂给 `AppBackend::new()`。**改语言需要重启进程** ——
//! Qt 也一样（`preferences.py:461-463` 的 `pm.setLang()` + `preferences-please-restart-
//! anki-to-complete-language`）。

use std::sync::LazyLock;

use regex::Regex;

/// `pylib/anki/lang.py` 原文（编译期嵌入，打包后照样可用）。
const LANG_PY: &str = include_str!("../../pylib/anki/lang.py");

/// 语言下拉的一项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lang {
    /// 显示名，与 Qt 的下拉一致（例如「简体中文」）
    pub name: String,
    /// Anki 的规范代码（例如 `zh_CN`）
    pub code: String,
}

/// 全部可选语言，顺序照 `langs` 的 `sorted(...)`。
pub fn all() -> &'static [Lang] {
    static LANGS: LazyLock<Vec<Lang>> = LazyLock::new(parse_langs);
    LANGS.as_slice()
}

/// ftl 目录名保留「语言-地区」连字符的组合（照抄 `lang_to_disk_lang()` 的名单）。
const QUALIFIED: &[&str] = &[
    "en_GB", "ga_IE", "hy_AM", "nb_NO", "nn_NO", "pt_BR", "pt_PT", "sv_SE", "zh_CN", "zh_TW",
];

/// 用户设置 / 系统语言 → ftl 目录名。对照 `lang_to_disk_lang()`。
pub fn to_disk_lang(lang: &str) -> String {
    let matched = match_code(lang).unwrap_or_else(|| lang.trim().replace('-', "_"));
    if QUALIFIED.contains(&matched.as_str()) {
        return matched.replace('_', "-");
    }
    // 其余语言去掉地区部分（`de_DE` → `de`）
    match matched.split_once('_') {
        Some((base, _)) => base.to_owned(),
        None => matched,
    }
}

/// 把任意写法（`zh`、`zh-CN`、`zh_CN`）对到语言表里的规范代码。
///
/// 只有基名时取该语言的第一个地区变体 —— Qt 的 `compatMap` 就是干这个的
/// （`{"it": "it_IT", ...}`），这里用前缀匹配代替那张表。
fn match_code(lang: &str) -> Option<String> {
    let normalized = lang.trim().replace('-', "_");
    if normalized.is_empty() {
        return None;
    }
    if let Some(hit) = all()
        .iter()
        .find(|it| it.code.eq_ignore_ascii_case(&normalized))
    {
        return Some(hit.code.clone());
    }
    let base = normalized.split('_').next().unwrap_or(&normalized);
    all()
        .iter()
        .find(|it| it.code.split('_').next() == Some(base))
        .map(|it| it.code.clone())
}

/// 生效的语言：`user`（`meta["defaultLang"]`）→ 系统语言 → `en_US`。
pub fn resolve(user: Option<&str>) -> String {
    if let Some(code) = user.filter(|it| !it.trim().is_empty()).and_then(match_code) {
        return to_disk_lang(&code);
    }
    if let Some(code) = system_lang().as_deref().and_then(match_code) {
        return to_disk_lang(&code);
    }
    "en-US".to_owned()
}

/// 系统语言。Windows 直接问 `GetUserDefaultLocaleName`（BCP-47 形态，如 `zh-CN`）。
#[cfg(windows)]
pub fn system_lang() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;

    // `LOCALE_NAME_MAX_LENGTH` 是 85
    let mut buffer = [0u16; 85];
    let len = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if len <= 1 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..(len as usize - 1)]))
}

#[cfg(not(windows))]
pub fn system_lang() -> Option<String> {
    std::env::var("LANG")
        .ok()
        .map(|value| value.split('.').next().unwrap_or_default().to_owned())
        .filter(|value| !value.is_empty())
}

/// 解析 `langs = sorted([...])` 里的 `("显示名", "代码")` 对。
fn parse_langs() -> Vec<Lang> {
    let re = Regex::new(r#"[(（]\s*["']([^"']+)["']\s*,\s*["']([A-Za-z]{2,3}(?:_[A-Z]{2})?)["']\s*[)）]"#)
        .expect("语言表正则非法");
    // 只取 `langs = sorted(` 之后到列表结束（`\n)`）之前那一段，避免匹配到别处的元组
    let Some((_, tail)) = LANG_PY.split_once("langs = sorted(") else {
        return Vec::new();
    };
    let end = tail.find("\n)").unwrap_or(tail.len());
    re.captures_iter(&tail[..end])
        .map(|cap| Lang {
            name: cap[1].to_owned(),
            code: cap[2].to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 语言表必须真的解析出来（源文件格式变了要立刻发现）。
    #[test]
    fn parses_language_list() {
        let all = all();
        assert!(all.len() > 40, "只解析出 {} 种语言", all.len());
        assert!(
            all.iter()
                .any(|it| it.code == "zh_CN" && it.name == "简体中文"),
            "语言表里没有 zh_CN：{:?}",
            all.iter().find(|it| it.code == "zh_CN")
        );
        assert!(all.iter().any(|it| it.code == "en_US"));
    }

    /// 与 `pylib/anki/lang.py` 的 `lang_to_disk_lang()` 逐例对齐。
    #[test]
    fn disk_lang_matches_python() {
        assert_eq!(to_disk_lang("zh_CN"), "zh-CN");
        assert_eq!(to_disk_lang("zh-CN"), "zh-CN");
        assert_eq!(to_disk_lang("zh_TW"), "zh-TW");
        assert_eq!(to_disk_lang("en_US"), "en");
        assert_eq!(to_disk_lang("en_GB"), "en-GB");
        assert_eq!(to_disk_lang("de_DE"), "de");
        assert_eq!(to_disk_lang("pt_BR"), "pt-BR");
        // 只有基名时走前缀匹配（Qt 用 compatMap）
        assert_eq!(to_disk_lang("it"), "it");
        assert_eq!(to_disk_lang("ja"), "ja");
    }

    /// 用户设置优先，其次是系统语言，最后才是 en-US。
    #[test]
    fn resolve_priority() {
        assert_eq!(resolve(Some("zh_CN")), "zh-CN");
        assert_eq!(resolve(Some("de_DE")), "de");
        assert_eq!(resolve(Some("  ")), {
            let expected = system_lang()
                .as_deref()
                .and_then(match_code)
                .map(|code| to_disk_lang(&code))
                .unwrap_or_else(|| "en-US".to_owned());
            expected
        });
        let fallback = system_lang()
            .as_deref()
            .and_then(match_code)
            .map(|code| to_disk_lang(&code))
            .unwrap_or_else(|| "en-US".to_owned());
        assert_eq!(resolve(None), fallback);
    }
}
