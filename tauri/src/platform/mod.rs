// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `FrontendService` 的平台端点实现。
//!
//! 这些端点在 rslib 中并不存在 —— `rslib/rust_interface.rs` 在生成分发代码时
//! 显式过滤掉了 `FrontendService` 与 `BackendFrontendService`。桌面版由 Qt 层
//! （`qt/aqt/mediasrv.py` 的 `post_handler_list`）实现，本模块用跨平台 Rust
//! 库补齐，从而使前端无需改动即可使用编辑器。
//!
//! 前端通过 `POST /_anki/{method}` 调用这些端点，请求体与响应体都是 protobuf。

mod audio;
mod clipboard;
mod config;
mod dialogs;
mod media;

use crate::state::AppState;

/// 端点处理结果：成功时返回 protobuf 响应体，失败时返回错误文本。
pub type HandlerResult = Result<Vec<u8>, String>;

/// 分发平台端点；返回 `None` 表示该方法不由本模块处理。
///
/// 注意：这些实现都是阻塞的（原生对话框、剪贴板），调用方必须放在
/// 阻塞线程池中执行。
pub fn dispatch(method: &str, state: &AppState, body: &[u8]) -> Option<HandlerResult> {
    let result = match method {
        // 剪贴板
        "readClipboard" => clipboard::read(body),
        "writeClipboard" => clipboard::write(body),

        // 编辑器媒体
        "convertPastedImage" => media::convert_pasted_image(body),
        "openFilePicker" => dialogs::open_file_picker(body),
        "openMedia" => media::open_media(state, body),
        "showInMediaFolder" => media::show_in_media_folder(state, body),

        // 对话框与外部程序
        "askUser" => dialogs::ask_user(body),
        "showMessageBox" => dialogs::show_message_box(body),
        "openLink" => dialogs::open_link(body),

        // 音频播放
        "playFile" => audio::play_file(state, body),

        // profile / meta 配置存储
        "getProfileConfigJson" => config::get_profile_config_json(state, body),
        "setProfileConfigJson" => config::set_profile_config_json(state, body),
        "getMetaJson" => config::get_meta_json(state, body),
        "setMetaJson" => config::set_meta_json(state, body),

        // 单窗口模型下没有对应窗口的端点。
        //
        // Qt 版本用它们关闭独立窗口或通知模态状态变化；当前外壳只有一个窗口，
        // 因此接受请求但不产生副作用 —— 这是显式降级而非遗漏。
        // 需要随多窗口 / 路由接管方案重新设计，见 README 的「M2 余项」。
        "closeAddCards"
        | "closeEditCurrent"
        | "importDone"
        | "importDialogRequireClose"
        | "deckOptionsRequireClose"
        | "deckOptionsReady"
        | "openFieldsDialog"
        | "openCardsDialog"
        | "searchInBrowser"
        | "saveCustomColours" => {
            tracing::debug!("单窗口下忽略该端点: {method}");
            Ok(Vec::new())
        }

        _ => return None,
    };
    Some(result)
}
