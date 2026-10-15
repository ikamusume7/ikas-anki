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
pub(crate) mod colours;
mod config;
pub(crate) mod dialogs;
mod media;
pub(crate) mod prefs21;
pub(crate) mod record;
mod scheduling;

use anki_proto::generic::Bool;
use prost::Message as _;

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
        // 外壳新增：网页里的导出对话框要问宿主一个保存路径（见 `dialogs::save_file_dialog`）
        "saveFileDialog" => dialogs::save_file_dialog(body),
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

        // 原生取色器的调色板（上游是 mediasrv 的 `save_custom_colours`，
        // `mediasrv.py:1122-1128`）；读那半边是后端的 `getCustomColours`
        "saveCustomColours" => colours::save_custom_colours(state),

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
        "closeAddCards" => close_add_cards(state, body),
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

        // 仍未实现，但**不是窗口模型挡住的**：它服务的界面在新版 Anki 里是原生
        // Qt 实现，仓库里没有对应的 Web 页面（详见 `crate::windows` 的模块注释）。
        //   - saveCustomColours：需要能暴露自定义调色板的原生取色器，`rfd` 无此 API

        // 不在这里的：`searchInBrowser`、`openFieldsDialog`、`openCardsDialog` ——
        // 它们都要调异步 RPC（前者查集合、后两者要 await `getNotetype` 才能算出标题），
        // 所以走 `server/api.rs`。
        "saveCustomColours" => {
            tracing::warn!("该端点需要尚未实现的界面或能力，暂时忽略: {method}");
            Ok(Vec::new())
        }

        _ => return None,
    };
    Some(result)
}

/// 关闭「添加卡片」窗口。对照 `qt/aqt/addcards.py:107-127` 的
/// `_close_if_user_wants_to_discard_changes`。
///
/// 请求体是 `generic.Bool`，也就是页面的 `shouldPromptBeforeClosing()` 判定结果
/// （`NoteEditor.svelte:458-480`：所有字段都为空/`<br>` 才算「没有输入」）。弹框必须
/// 由宿主来做 —— Web 侧整个目录没有 `confirm`，页面只是把这个布尔值交上来。
///
/// 按钮文案的取舍：Qt 的「Discard」用的是 Qt 自带的标准按钮翻译，仓库里没有对应的
/// ftl key（`adding-keep-editing` 只覆盖另一个按钮），所以这里用系统默认的是/否，
/// 文案取 `adding-discard-current-input`。语义一致，这不是遗漏。
fn close_add_cards(state: &AppState, body: &[u8]) -> HandlerResult {
    let has_pending_input = Bool::decode(body)
        .map_err(|error| error.to_string())?
        .val;
    if has_pending_input {
        tracing::info!("add-cards 有未保存输入，等待用户确认是否放弃");
        if !dialogs::confirm_discard_input(state) {
            tracing::info!("用户选择继续编辑，add-cards 保持打开");
            return Ok(Vec::new());
        }
        tracing::info!("用户选择放弃输入");
    }
    windows::close(state, windows::ADD_CARDS);
    Ok(Vec::new())
}
