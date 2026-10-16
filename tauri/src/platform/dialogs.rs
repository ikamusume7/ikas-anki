// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 原生对话框与外部程序调用。
//!
//! 对照 mediasrv 的 `open_file_picker` / `ask_user` / `show_message_box` /
//! `open_link`（原实现基于 Qt）。

use std::path::Path;
use std::path::PathBuf;

use anki_proto::frontend::AskUserRequest;
use anki_proto::frontend::MessageBoxType;
use anki_proto::frontend::OpenFilePickerRequest;
use anki_proto::frontend::ShowMessageBoxRequest;
use anki_proto::generic::Bool;
use anki_proto::generic::String as GenericString;
use prost::Message as _;

use super::HandlerResult;
use crate::state::AppState;

/// 对应 mediasrv 的 `open_file_picker`，返回选中的路径（取消时为空串）。
pub fn open_file_picker(body: &[u8]) -> HandlerResult {
    let request = OpenFilePickerRequest::decode(body).map_err(|e| e.to_string())?;
    let picked = pick_file(&request.title, &request.extensions, &request.filter_description);
    let val = picked
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(GenericString { val }.encode_to_vec())
}

/// 弹一个"打开文件"对话框，返回选中的路径（取消返回 `None`）。
///
/// 端点（`open_file_picker`）与**宿主自己**的导入入口（`crate::importing`：牌组列表的
/// 「导入文件」按钮走的就是 Qt 的 `getFile()`，`importing.py:153-163`）共用这一份。
///
/// 测试钩子：设了 `ANKI_OPEN_PATH` 就直接返回它，不弹框 —— 原生"打开文件"对话框
/// 没法自动点（与 `ANKI_SAVE_PATH` 同一套办法，见下）。用它的场景是图像遮挡：
/// 编辑器页面的「选图片」走 `/openFilePicker`（`data-transfer.ts:455` 的
/// `openFilePickerForImageOcclusion`），`occlusion-check.mjs` 要能自动走完；
/// `import-check.mjs` 用它给"导入文件"按钮喂一个 `.apkg`。
pub fn pick_file(title: &str, extensions: &[String], filter_description: &str) -> Option<PathBuf> {
    pick_file_in(title, extensions, filter_description, None)
}

/// 同上，但可以指定**起始目录**（上游 `getFile(dir=...)`）。
///
/// 「文件 → 还原备份」用它把对话框开在 `<profile>/backups`
/// （`main.py:441-467` 的 `dir=self.pm.backupFolder()`）。注意这个变体**不**看
/// `ANKI_OPEN_PATH`：还原备份有自己的钩子（`ANKI_RESTORE_PATH`），两者指向的文件不同，
/// 共用会因为验证脚本里那两个用途打架。
pub fn pick_file_in(
    title: &str,
    extensions: &[String],
    filter_description: &str,
    directory: Option<&Path>,
) -> Option<PathBuf> {
    if directory.is_none() {
        if let Ok(hook) = std::env::var("ANKI_OPEN_PATH") {
            if !hook.is_empty() {
                tracing::info!("ANKI_OPEN_PATH 生效，跳过打开对话框: {hook}");
                return Some(PathBuf::from(hook));
            }
        }
    }

    let mut dialog = rfd::FileDialog::new();
    if !title.is_empty() {
        dialog = dialog.set_title(title);
    }
    if !extensions.is_empty() {
        let extensions: Vec<&str> = extensions.iter().map(String::as_str).collect();
        let label = if filter_description.is_empty() {
            "Files"
        } else {
            filter_description
        };
        dialog = dialog.add_filter(label, &extensions);
    }
    if let Some(directory) = directory {
        dialog = dialog.set_directory(directory);
    }

    dialog.pick_file()
}

/// 保存文件对话框（**外壳新增的端点**，上游没有）。
///
/// 上游的导出对话框是原生界面，直接用 `getSaveFile()` 拿路径
/// （`qt/aqt/import_export/exporting.py:119-140`）；网页里的对话框要问宿主，
/// 所以这里补一个。请求复用 `OpenFilePickerRequest`：
///
/// - `title`：对话框标题（导入导出都用 `actions-export`）；
/// - `extensions`：扩展名（用来生成默认文件名与过滤器）；
/// - `filter_description`：过滤器文案（如 "Anki Deck Package"）；
/// - `key`：**外壳约定 = 建议的文件名**（上游用它是"记住上次目录"的配置键，
///   本外壳没有这套机制，正好拿来传默认名；空则退回 `collection.<ext>`）。
///
/// 响应是 `generic.String`（取消时为空串）。
///
/// 测试钩子：设了 `ANKI_SAVE_PATH` 就直接返回它，不弹框 —— 原生对话框没法自动点
/// （对照外壳已有的 `ANKI_*` 环境变量用法）。指向**已存在的目录**时，返回
/// `<该目录>/<建议文件名>`，这样一个测试里可以连着导出多个文件。
pub fn save_file_dialog(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = OpenFilePickerRequest::decode(body).map_err(|e| e.to_string())?;
    let extension = request.extensions.first().cloned().unwrap_or_default();
    let suggested = if request.key.is_empty() {
        if extension.is_empty() {
            "collection".to_owned()
        } else {
            format!("collection.{extension}")
        }
    } else {
        request.key.clone()
    };

    if let Ok(hook) = std::env::var("ANKI_SAVE_PATH") {
        if !hook.is_empty() {
            let path = if std::path::Path::new(&hook).is_dir() {
                std::path::Path::new(&hook)
                    .join(&suggested)
                    .to_string_lossy()
                    .into_owned()
            } else {
                hook
            };
            tracing::info!("ANKI_SAVE_PATH 生效，跳过保存对话框: {path}");
            // 钩子也一样过导出目录守卫：自动化里没法重弹对话框，所以直接拒绝（返回空串），
            // 让「导出到 profile 目录」这条路径可以被端到端断言。
            if in_profile_dir(state, &path) {
                tracing::warn!("导出路径在 profile 目录内，拒绝并要求重选: {path}");
                return Ok(GenericString { val: String::new() }.encode_to_vec());
            }
            return Ok(GenericString { val: path }.encode_to_vec());
        }
    }

    // 对照 `exporting.py:119-140` 的 `get_out_path()`：`while True` 里弹保存框，
    // 选到 profile 目录内就提示换个位置再来一次。上游判的是 `pm.base` 整棵树，
    // 这里判**当前 profile 目录**（`collection.anki2` 所在处）—— 外壳的测试集合就在
    // 仓库的 `out/` 下面，用 base 会把所有测试导出路径一起拒掉，见 `tauri/PARITY.md` 5.9。
    loop {
        let mut dialog = rfd::FileDialog::new().set_file_name(&suggested);
        if !request.title.is_empty() {
            dialog = dialog.set_title(&request.title);
        }
        if !request.extensions.is_empty() {
            let extensions: Vec<&str> = request.extensions.iter().map(String::as_str).collect();
            let label = if request.filter_description.is_empty() {
                "Files"
            } else {
                &request.filter_description
            };
            dialog = dialog.add_filter(label, &extensions);
        }

        let picked = dialog.save_file();
        let val = picked
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        tracing::info!("保存对话框返回: {val}");
        if val.is_empty() || !in_profile_dir(state, &val) {
            return Ok(GenericString { val }.encode_to_vec());
        }
        tracing::warn!("导出路径在 profile 目录内，提示换个位置: {val}");
        // 上游是硬编码英文（`exporting.py:137`），这里保持一致
        warn("Please choose a different export location.");
    }
}

/// 路径是不是落在当前 profile 目录里（导出目录守卫，对照 `exporting.py:132-138`）。
fn in_profile_dir(state: &AppState, path: &str) -> bool {
    path_in_dir(path, state.profile_dir.as_path())
}

/// 同上，纯函数版（便于单测）：两边都做一次 `canonicalize`，文件可能还不存在
/// （保存对话框返回的是未来路径），那时退化成"规范化过的字面比较"（Windows 上大小写不敏感）。
fn path_in_dir(path: &str, dir: &std::path::Path) -> bool {
    let normalize = |p: &std::path::Path| {
        let text = std::fs::canonicalize(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .to_string_lossy()
            .replace('/', "\\");
        // Windows 的 `canonicalize` 返回带 `\\?\` 前缀的 verbatim 路径；不去掉它，
        // 拿用户给的字面路径永远比不中（这个坑是 `verify-decks` 的导出拦截断言抓出来的）
        let text = text.strip_prefix(r"\\?\").unwrap_or(&text).to_string();
        text.trim_end_matches('\\').to_lowercase()
    };
    let target = normalize(std::path::Path::new(path));
    let dir = normalize(dir);
    target == dir || target.starts_with(&format!("{dir}\\"))
}

#[cfg(test)]
mod test {
    use super::path_in_dir;

    #[test]
    fn export_guard_matches_profile_dir_only() {
        let dir = std::path::Path::new("C:\\Anki2\\User 1");
        // profile 目录内的都拒绝（含大小写与分隔符差异、未来才存在的文件）
        assert!(path_in_dir("C:\\Anki2\\User 1\\out.colpkg", dir));
        assert!(path_in_dir("c:/anki2/user 1/sub/out.apkg", dir));
        assert!(path_in_dir("C:\\Anki2\\User 1", dir));
        // 目录本身之外的同级/上级路径放行（上游拒的是整个 base，本外壳只拒 profile 目录）
        assert!(!path_in_dir("C:\\Anki2\\User 2\\out.colpkg", dir));
        assert!(!path_in_dir("C:\\Anki2\\out.colpkg", dir));
        assert!(!path_in_dir("D:\\exports\\out.colpkg", dir));
        // 前缀相同但不是子目录（`User 1x`）不能误判
        assert!(!path_in_dir("C:\\Anki2\\User 1x\\out.colpkg", dir));

        // 真实存在的目录会走 `canonicalize` 分支：Windows 上它返回 `\\?\` 前缀的
        // verbatim 路径，必须剥掉才能和字面路径比中（不然守卫永远不生效）
        let tmp = std::env::temp_dir();
        let inside = tmp.join("anki-export-guard-probe.colpkg");
        assert!(path_in_dir(&inside.to_string_lossy(), &tmp));
    }
}

/// 对应 mediasrv 的 `ask_user`：是/否确认框。
///
/// 测试钩子：`ANKI_CONFIRM_YES=1` 时直接返回「是」—— 原生模态框点不了，
/// 与 `ANKI_SAVE_PATH` / `ANKI_RESTORE_PATH` 同一套办法（见 `tauri/README.md`）。
pub fn ask_user(body: &[u8]) -> HandlerResult {
    let request = AskUserRequest::decode(body).map_err(|e| e.to_string())?;

    if std::env::var("ANKI_CONFIRM_YES").is_ok_and(|value| !value.is_empty() && value != "0") {
        tracing::info!("ANKI_CONFIRM_YES 生效，确认框直接返回「是」: {}", request.text);
        return Ok(Bool { val: true }.encode_to_vec());
    }

    let mut dialog = rfd::MessageDialog::new()
        .set_description(&request.text)
        .set_buttons(rfd::MessageButtons::YesNo);
    if let Some(title) = request.title.as_deref() {
        dialog = dialog.set_title(title);
    }

    let confirmed = matches!(dialog.show(), rfd::MessageDialogResult::Yes);
    Ok(Bool { val: confirmed }.encode_to_vec())
}

/// 对应 mediasrv 的 `show_message_box`。
pub fn show_message_box(body: &[u8]) -> HandlerResult {
    let request = ShowMessageBoxRequest::decode(body).map_err(|e| e.to_string())?;

    let level = match request.r#type() {
        MessageBoxType::Warning => rfd::MessageLevel::Warning,
        MessageBoxType::Critical => rfd::MessageLevel::Error,
        MessageBoxType::Info => rfd::MessageLevel::Info,
    };

    let mut dialog = rfd::MessageDialog::new()
        .set_description(&request.text)
        .set_level(level)
        .set_buttons(rfd::MessageButtons::Ok);
    if let Some(title) = request.title.as_deref() {
        dialog = dialog.set_title(title);
    }
    dialog.show();

    Ok(Vec::new())
}

/// 通用的是/否确认框（宿主侧，对照 Qt 的 `askUser`）。
///
/// 与 `ask_user`（页面发起的端点）的区别：这个是宿主在动作里弹的，例如复习菜单的
/// 「删除笔记」先问一次（对照 `reviewer.py:1037-1041` 的 `delete_notes`）。
pub fn confirm(message: &str) -> bool {
    matches!(
        rfd::MessageDialog::new()
            .set_title("Anki")
            .set_description(message)
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes
    )
}

/// 时间盒到点的「继续 / 结束」框（对照 `reviewer.py:983-988` 的 `askUserDialog`）。
///
/// 返回 `true` = 继续（Qt 里选「继续」会重新计时，选「结束」回牌组列表）。
/// 按钮文案来自 ftl（`studying-continue` / `studying-finish`）；Windows 上走
/// `OkCancelCustom` —— rfd 0.15 没有 `YesNoCustom`，而 OK/Cancel 分别返回
/// `Custom(第一个文案)` / `Custom(第二个文案)`（`rfd` 的 win_cid 后端如此映射）。
///
/// `ANKI_TIMEBOX_CHOICE=continue|finish` 可以跳过对话框（供端到端脚本用，
/// 与 `ANKI_CONFIRM_YES` 同一套路）。
pub fn ask_timebox(message: &str, continue_label: &str, finish_label: &str) -> bool {
    if let Ok(choice) = std::env::var("ANKI_TIMEBOX_CHOICE") {
        let proceed = !choice.eq_ignore_ascii_case("finish");
        let label = if proceed { continue_label } else { finish_label };
        tracing::info!("ANKI_TIMEBOX_CHOICE={choice} 生效，时间盒对话框返回「{label}」");
        return proceed;
    }

    let result = rfd::MessageDialog::new()
        .set_title("Anki")
        .set_description(message)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::OkCancelCustom(
            continue_label.to_owned(),
            finish_label.to_owned(),
        ))
        .show();
    matches!(result, rfd::MessageDialogResult::Custom(label) if label == continue_label)
}

/// 「放弃当前输入？」确认框，返回 `true` 表示放弃。
///
/// 对照 `qt/aqt/addcards.py:118-127`：关窗前宿主弹框，用户选 Discard 才真的关窗。
/// 文案用 `adding-discard-current-input`（Qt 用的是同一个 key，`ftl/core/adding.ftl:3`）。
pub fn confirm_discard_input(state: &AppState) -> bool {
    let tr = state.backend.i18n();
    matches!(
        rfd::MessageDialog::new()
            .set_title("Anki")
            .set_description(tr.adding_discard_current_input())
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes
    )
}

/// 宿主自己在流程中间弹的警告框（对照 Qt 的 `showWarning`）。
///
/// 与 `show_message_box` 的区别：那个是**页面发起的端点**（消息由页面给），这个是宿主
/// 在动作里判断出问题后直接弹，例如"选中的笔记不是同一种笔记类型，不能改笔记类型"
/// （`qt/aqt/changenotetype.py:91-93`）。
pub fn warn(message: &str) {
    tracing::info!("警告框: {message}");
    rfd::MessageDialog::new()
        .set_title("Anki")
        .set_description(message)
        .set_level(rfd::MessageLevel::Warning)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// 对应 mediasrv 的 `open_link`：交给系统默认程序处理。
pub fn open_link(body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    opener::open(&request.val).map_err(|e| e.to_string())?;
    Ok(Vec::new())
}
