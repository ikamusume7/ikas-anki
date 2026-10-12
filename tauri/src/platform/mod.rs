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

pub(crate) mod audio;
mod clipboard;
mod config;
mod dialogs;
mod media;
pub(crate) mod prefs21;
pub(crate) mod record;
mod scheduling;

use crate::state::AppState;
use crate::windows;

/// 端点处理结果：成功时返回 protobuf 响应体，失败时返回错误文本。
pub type HandlerResult = Result<Vec<u8>, String>;

/// 与 rslib 服务**同名**、但必须由平台层接管的端点。
///
/// 对照 Qt：`mediasrv.post_handler_list` 里的处理器会盖过后端服务
/// （`_handle_dynamic_request` 先查处理器表，`qt/aqt/mediasrv.py:1343`）。
/// 本外壳的服务端是"先查方法表"，因此这些名字要显式列出来。
///
/// - `getConfigJson`：Qt 缺键时返回 `null`（`mediasrv.py:834-838`），
///   而 rslib 的同名方法对缺失的键直接报错。编辑器要读 `renderMathjax` 这类布尔项，
///   新建集合里并没有它们（默认值在 rslib 的注册表里，不落库），
///   于是会弹出错误框、并把页面的 JS 事件循环卡在 `alert()` 上。
///
/// 同为处理器表成员但**不**需要接管的：
///
/// - `congratsInfo` / `getDeckConfigsForUpdate`：Qt 的实现只是转手调用后端，语义相同；
/// - `updateDeckConfigs` / `changeNotetype`：Qt 额外做了界面副作用
///   （前者成功后关掉牌组选项窗口，后者把数据交给原生对话框保存），
///   这些属于后续增量，暂未接管。
pub const SHADOWED_METHODS: &[&str] = &["getConfigJson"];

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

        // 音频：播放与录音
        "playFile" => audio::play_file(state, body),
        // 长阻塞：一直等到用户点「保存/取消」。调用方已在阻塞线程池里
        "recordAudio" => record::record(state),

        // 集合级配置。Qt 在 mediasrv 里自己实现（缺键时返回 null 而非报错），
        // 因此这里**不能**落到 rslib 的同名方法上，见 `config.rs`
        "getConfigJson" => config::get_collection_config_json(state, body),

        // profile / meta 配置存储
        "getProfileConfigJson" => config::get_profile_config_json(state, body),
        "setProfileConfigJson" => config::set_profile_config_json(state, body),
        "getMetaJson" => config::get_meta_json(state, body),
        "setMetaJson" => config::set_meta_json(state, body),

        // 复习界面的调度状态同步（自定义调度）
        "getSchedulingStatesWithContext" => scheduling::get_states_with_context(state, body),
        "setSchedulingStates" => scheduling::set_states(state, body),

        // 窗口管理（模型见 `crate::windows`，决策依据见 README 的「窗口模型」）。
        //
        // 这些端点由**页面**发起：WebView 关不掉宿主窗口，只能请求宿主代劳。
        // 窗口不存在时 `windows::close` 只记日志 —— 这是正确行为，不是降级。
        "closeAddCards" => {
            windows::close(state, windows::ADD_CARDS);
            Ok(Vec::new())
        }
        "closeEditCurrent" => {
            windows::close(state, windows::EDIT_CURRENT);
            Ok(Vec::new())
        }
        "deckOptionsRequireClose" => {
            windows::close(state, windows::DECK_OPTIONS);
            Ok(Vec::new())
        }
        "importDialogRequireClose" => {
            windows::close(state, windows::IMPORT);
            Ok(Vec::new())
        }
        // 页面报到，对照 `DeckOptionsDialog.set_ready()`
        "deckOptionsReady" => {
            windows::mark_ready(windows::DECK_OPTIONS);
            Ok(Vec::new())
        }
        // 导入完成后把导入窗口由模态转为非模态，对照 `mediasrv.import_done`
        "importDone" => {
            windows::set_modal(state, windows::IMPORT, false);
            Ok(Vec::new())
        }

        // 仍未实现，但**不是窗口模型挡住的**：它们服务的界面在新版 Anki 里是原生
        // Qt 实现，仓库里没有对应的 Web 页面（详见 `crate::windows` 的模块注释）。
        //   - 浏览器 / 字段 / 卡片布局：需要新的 Svelte 界面（M3）
        //   - saveCustomColours：需要能暴露自定义调色板的原生取色器，`rfd` 无此 API
        "searchInBrowser" | "openFieldsDialog" | "openCardsDialog" | "saveCustomColours" => {
            tracing::warn!("该端点需要尚未实现的界面或能力，暂时忽略: {method}");
            Ok(Vec::new())
        }

        _ => return None,
    };
    Some(result)
}
