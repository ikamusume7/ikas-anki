// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 原生对话框与外部程序调用。
//!
//! 对照 mediasrv 的 `open_file_picker` / `ask_user` / `show_message_box` /
//! `open_link`（原实现基于 Qt）。

use anki_proto::frontend::AskUserRequest;
use anki_proto::frontend::MessageBoxType;
use anki_proto::frontend::OpenFilePickerRequest;
use anki_proto::frontend::ShowMessageBoxRequest;
use anki_proto::generic::Bool;
use anki_proto::generic::String as GenericString;
use prost::Message as _;

use super::HandlerResult;

/// 对应 mediasrv 的 `open_file_picker`，返回选中的路径（取消时为空串）。
pub fn open_file_picker(body: &[u8]) -> HandlerResult {
    let request = OpenFilePickerRequest::decode(body).map_err(|e| e.to_string())?;

    let mut dialog = rfd::FileDialog::new();
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

    let picked = dialog.pick_file();
    let val = picked
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();

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

/// 对应 mediasrv 的 `open_link`：交给系统默认程序处理。
pub fn open_link(body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    opener::open(&request.val).map_err(|e| e.to_string())?;
    Ok(Vec::new())
}
