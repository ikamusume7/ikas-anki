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
pub fn save_file_dialog(body: &[u8]) -> HandlerResult {
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
            return Ok(GenericString { val: path }.encode_to_vec());
        }
    }

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

    Ok(GenericString { val }.encode_to_vec())
}

/// 对应 mediasrv 的 `ask_user`：是/否确认框。
pub fn ask_user(body: &[u8]) -> HandlerResult {
    let request = AskUserRequest::decode(body).map_err(|e| e.to_string())?;

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
