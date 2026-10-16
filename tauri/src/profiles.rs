// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 多 profile 管理（PARITY 1.5 / 8.2）。
//!
//! 对照 `qt/aqt/profiles.py` 的 `ProfileManager` 与 `main.py:321-437` 的 Profiles 窗口：
//!
//! - **列出**：`select name from profiles where name != '_global'`（`profiles.py:163-172`）；
//! - **新建**：只插一行、**不建目录**（`:241-250`），目录由「打开」时的集合创建；
//! - **重命名**：先改 `profiles.name`，再改目录名（`:262-303`）；目录已存在时拒绝
//!   （`profiles-folder-already-exists`），非法字符按 `pylib/anki/utils.py:256-269` 校验；
//! - **删除**：整个 profile 目录进回收站 + 删行（`:252-256`），且**至少保留一个**；
//! - **切换**：写 `meta["last_loaded_profile_name"]`（`:650-654`）。
//!
//! ## 与 Qt 的差异（记在 `tauri/PARITY.md` 8.2）
//!
//! Qt 切换 profile 是**关集合 → 重开集合**、同一个进程里换；外壳改成**重启进程**
//! （`AppHandle::restart()`，重启后由 `paths.rs` 读 `last_loaded_profile_name` 决定开哪个）。
//! 用户可见结果等价（两边都会关掉所有窗口、重新打开集合），换来的是不必把
//! `AppState` 里的 `profile_dir` / `media_folder` / 集合开关状态全部改成可变。
//!
//! 命令统一走一条桥接命令 `profile:<json>`（避免为每个动作加一条），
//! 返回值是 JSON：成功 `{"ok":true,...}`，失败 `{"ok":false,"error":"<代号>"}`，
//! 代号由页面映射成 ftl 文案（`name-exists` / `invalid-char` / `folder-exists` /
//! `last-profile` / `missing` / 其他 IO 错误原文）。

use std::path::Path;
use std::path::PathBuf;

use serde_json::json;
use serde_json::Value as Json;

use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
use crate::state::AppState;

/// 文件名里不允许的字符（对照 `pylib/anki/utils.py:256-269` 的 `invalid_filename`）。
const INVALID_CHARS: &[char] = &[':', '*', '?', '"', '<', '>', '|', '/', '\\'];
/// 保留名（对照 `main.py:380-381` 的 `addons21`）。
const RESERVED: &[&str] = &["addons21"];

/// profile 的 base 目录（`prefs21.db` 与各 profile 目录都在这里）。
pub fn base_dir(state: &AppState) -> PathBuf {
    state
        .profile_dir
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 当前 profile 名（就是 profile 目录名，如 `User 1`）。
pub fn current_name(state: &AppState) -> String {
    state
        .profile_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn prefs(state: &AppState) -> Prefs21 {
    Prefs21::for_meta(&base_dir(state))
}

fn profile_dir(state: &AppState, name: &str) -> PathBuf {
    base_dir(state).join(name)
}

/// 名字校验；返回不合法的那一项（供页面映射文案）。
fn validate(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("invalid-char".to_owned());
    }
    if let Some(character) = name.chars().find(|it| INVALID_CHARS.contains(it)) {
        tracing::debug!("profile 名含非法字符 {character}");
        return Err("invalid-char".to_owned());
    }
    if name.starts_with('.') {
        return Err("invalid-char".to_owned());
    }
    if RESERVED.iter().any(|it| it.eq_ignore_ascii_case(name)) {
        return Err("name-exists".to_owned());
    }
    Ok(())
}

/// profile 列表（含当前那个）的 JSON。
pub fn list_json(state: &AppState) -> String {
    let current = current_name(state);
    match prefs(state).list_names() {
        Ok(names) => json!({ "ok": true, "current": current, "profiles": names }).to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// 处理一条 `profile:<json>` 命令。
pub fn handle_command(state: &AppState, payload: &str) -> String {
    let request: Json = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(error) => {
            return json!({ "ok": false, "error": format!("非法参数: {error}") }).to_string()
        }
    };
    let action = request.get("action").and_then(Json::as_str).unwrap_or("");
    let name = request.get("name").and_then(Json::as_str).unwrap_or("");
    match action {
        "list" => list_json(state),
        "create" => result(create(state, name)),
        "rename" => {
            let to = request.get("to").and_then(Json::as_str).unwrap_or("");
            result(rename(state, name, to))
        }
        "delete" => result(delete(state, name)),
        "switch" => result(switch(state, name)),
        "open_backups" => result(open_backups(state)),
        other => json!({ "ok": false, "error": format!("未知动作: {other}") }).to_string(),
    }
}

fn result(outcome: Result<(), String>) -> String {
    match outcome {
        Ok(()) => json!({ "ok": true }).to_string(),
        Err(error) => json!({ "ok": false, "error": error }).to_string(),
    }
}

/// 新建 profile（只插一行）。
pub fn create(state: &AppState, name: &str) -> Result<(), String> {
    let name = name.trim();
    validate(name)?;
    if !prefs(state).create_row(name)? {
        return Err("name-exists".to_owned());
    }
    tracing::info!("新建配置: {name}");
    Ok(())
}

/// 重命名 profile：先改行、再改目录（目录可能还不存在 —— 没打开过的 profile
/// 只有一行，`profiles.py:262-303` 同样是 `if os.path.exists(...)` 才改名）。
pub fn rename(state: &AppState, from: &str, to: &str) -> Result<(), String> {
    let from = from.trim();
    let to = to.trim();
    validate(to)?;
    if from.eq_ignore_ascii_case(to) {
        return Err("name-exists".to_owned());
    }

    let names = prefs(state).list_names()?;
    if names.iter().any(|it| it.eq_ignore_ascii_case(to)) {
        return Err("name-exists".to_owned());
    }
    if !names.iter().any(|it| it == from) {
        return Err("missing".to_owned());
    }

    let old_dir = profile_dir(state, from);
    let new_dir = profile_dir(state, to);
    if new_dir.exists() {
        return Err("folder-exists".to_owned());
    }

    if !prefs(state).rename_row(from, to)? {
        return Err("missing".to_owned());
    }
    if old_dir.exists() {
        // 只有大小写差异时 Windows 的 `rename` 会失败，先挪到临时目录再挪回来
        let result = if old_dir.file_name().map(|n| n.to_string_lossy().to_lowercase())
            == new_dir.file_name().map(|n| n.to_string_lossy().to_lowercase())
        {
            let temp = profile_dir(state, &format!("{to}-temp"));
            std::fs::rename(&old_dir, &temp).and_then(|()| std::fs::rename(&temp, &new_dir))
        } else {
            std::fs::rename(&old_dir, &new_dir)
        };
        if let Err(error) = result {
            // 目录没改成，把行改回去，避免「行与目录不一致」
            let _ = prefs(state).rename_row(to, from);
            tracing::warn!("重命名配置目录失败: {error}");
            return Err(format!("{error}"));
        }
    }
    // 被重命名的正是当前 profile 时，让下次启动也指向新名字
    let current = current_name(state);
    if current == from {
        let _ = prefs(state).set(Store::Meta, "last_loaded_profile_name", &json!(to));
    }
    tracing::info!("重命名配置: {from} → {to}");
    Ok(())
}

/// 删除 profile：目录进回收站 + 删行；至少保留一个。
pub fn delete(state: &AppState, name: &str) -> Result<(), String> {
    let name = name.trim();
    let names = prefs(state).list_names()?;
    if names.len() < 2 {
        return Err("last-profile".to_owned());
    }
    if !names.iter().any(|it| it == name) {
        return Err("missing".to_owned());
    }
    let dir = profile_dir(state, name);
    if dir.exists() {
        if let Err(error) = trash(&dir) {
            tracing::warn!("删除配置目录失败（将直接删）: {error}");
            std::fs::remove_dir_all(&dir).map_err(|error| format!("{error}"))?;
        }
    }
    prefs(state).delete_row(name)?;
    tracing::info!("删除配置: {name}");

    // 删掉的正是当前 profile：切到剩下的第一个（重启后生效），否则窗口还开着、
    // 集合却已经没有对应的 profile 了
    if name.eq_ignore_ascii_case(&current_name(state)) {
        if let Some(next) = prefs(state).list_names()?.into_iter().next() {
            prefs(state)
                .set(Store::Meta, "last_loaded_profile_name", &json!(next))
                .map_err(|error| format!("写入 last_loaded_profile_name 失败: {error}"))?;
            tracing::info!("当前配置已被删除，重启到 {next}");
            state.app.restart();
        }
    }
    Ok(())
}

/// 删除目录：**进回收站**（对照 Qt 的 `send_to_trash`），失败时由调用方直接删。
#[cfg(windows)]
fn trash(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;

    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::SHFileOperationW;
    use windows::Win32::UI::Shell::SHFILEOPSTRUCTW;
    use windows::Win32::UI::Shell::FOF_ALLOWUNDO;
    use windows::Win32::UI::Shell::FOF_NOCONFIRMATION;
    use windows::Win32::UI::Shell::FOF_SILENT;
    use windows::Win32::UI::Shell::FO_DELETE;

    // `SHFileOperationW` 的路径必须是**双 NUL 结尾**的宽字符串
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.push(0);
    from.push(0);

    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        pTo: PCWSTR::null(),
        // FOF_ALLOWUNDO 就是「进回收站」；另外两个关掉确认框与进度条
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0) as u16,
        lpszProgressTitle: PCWSTR::null(),
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code != 0 {
        return Err(format!("SHFileOperationW 返回 {code}"));
    }
    if operation.fAnyOperationsAborted.as_bool() {
        return Err("回收站操作被中止".to_owned());
    }
    Ok(())
}

#[cfg(not(windows))]
fn trash(_path: &Path) -> Result<(), String> {
    Err("未实现：非 Windows 平台没有回收站调用".to_owned())
}

/// 切换 profile：写 `meta["last_loaded_profile_name"]` 后**重启进程**。
pub fn switch(state: &AppState, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.eq_ignore_ascii_case(&current_name(state)) {
        return Ok(());
    }
    let names = prefs(state).list_names()?;
    if !names.iter().any(|it| it == name) {
        return Err("missing".to_owned());
    }
    prefs(state)
        .set(Store::Meta, "last_loaded_profile_name", &json!(name))
        .map_err(|error| format!("写入 last_loaded_profile_name 失败: {error}"))?;

    // 测试钩子：`ANKI_NO_RESTART=1` 时只落库、不重启（切换会换掉整个进程，
    // 自动化里没法在同一个进程里继续断言）
    if std::env::var("ANKI_NO_RESTART").is_ok_and(|value| !value.is_empty() && value != "0") {
        tracing::info!("ANKI_NO_RESTART 生效：已写入 last_loaded_profile_name={name}，不重启");
        return Ok(());
    }

    tracing::info!("切换到配置 {name}，重启进程（对照 Qt 的关集合 + 重开）");
    // 不返回：Tauri 会启动新进程并退出当前进程（新进程读 last_loaded_profile_name）
    state.app.restart();
}

/// 打开当前 profile 的备份目录（`<profile>/backups`）。
///
/// 对照 Qt 的「打开备份文件夹」入口（`qt/aqt/main.py:441` 一带；PARITY 6.2 的缺口）。
pub fn open_backups(state: &AppState) -> Result<(), String> {
    let dir = state.profile_dir.join("backups");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| format!("创建备份目录失败: {error}"))?;
    }
    opener::open(&dir).map_err(|error| format!("打开备份目录失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_profile_names() {
        assert!(validate("User 1").is_ok());
        assert!(validate("学习").is_ok());
        assert!(validate("").is_err());
        assert!(validate(".").is_err());
        assert!(validate(".hidden").is_err());
        assert!(validate("a/b").is_err());
        assert!(validate("a:b").is_err());
        assert!(validate("addons21").is_err());
    }
}
