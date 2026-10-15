// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 原生取色器（PARITY 6.5）。
//!
//! 上游的链路：编辑器里的 `<input type="color">` 在 Qt WebEngine 下弹的是 **Qt 的取色器**
//! （Windows 上就是系统的 `ChooseColor` 对话框），页面在 `click` / `change` 时调
//! `saveCustomColours`，mediasrv 把对话框的 16 个自定义色槽写进集合配置
//! `customColorPickerPalette`（`qt/aqt/mediasrv.py:1122-1128`）；下次页面用后端 RPC
//! `getCustomColours`（`rslib/src/collection/service.rs:68-75`）把调色板画出来。
//!
//! 本外壳里 WebView2 的 `<input type="color">` 是 **Chromium 自带**的取色器，宿主拿不到它的
//! 自定义色槽，所以换成宿主侧的原生对话框（`ChooseColorW`，与 Qt 在 Windows 上用的是同一个），
//! 页面通过 `pickColour:<当前颜色>` 桥接命令调用（`crate::platform::colours::pick`）。
//! 色槽存在**同一把集合配置键**里，因此两边读到的调色板是一致的。
//!
//! 测试钩子：`ANKI_PICK_COLOUR` 设了就不弹对话框，直接当作用户选了这个颜色
//! （原生取色器没法自动点，与 `ANKI_SAVE_PATH` / `ANKI_OPEN_PATH` 同一套办法）。

use std::sync::Mutex;

use anki_proto::collection::GetCustomColoursResponse;
use anki_proto::config::SetConfigJsonRequest;
use anki_proto::generic::Empty;
use prost::Message as _;

use crate::platform::HandlerResult;
use crate::state::AppState;

/// 上游写的那把集合配置键（`qt/aqt/mediasrv.py:1127`）。
const PALETTE_KEY: &str = "customColorPickerPalette";

/// 系统取色对话框的自定义色槽数量（`QColorDialog.customCount()` 同样是 16）。
const SLOTS: usize = 16;

/// 最近一次对话框里的 16 个色槽（`None` = 本次运行还没弹过）。
///
/// 对应 Qt 的 `QColorDialog.customColor(i)` —— 那也是进程内的状态，落库由
/// `saveCustomColours` 负责。
static DIALOG_SLOTS: Mutex<Option<Vec<u32>>> = Mutex::new(None);

/// 平台端点 `saveCustomColours`（`frontend.proto:63`）。
///
/// 上游把 Qt 对话框的色槽写进集合配置；这里把**宿主对话框**的色槽写进去
/// （没弹过对话框时就是配置里原来那份，等于空操作）。
pub fn save_custom_colours(state: &AppState) -> HandlerResult {
    let palette = dialog_palette(state);
    write_palette(state, &palette)?;
    Ok(Vec::new())
}

/// 「页面点了取色器」→ 弹原生对话框，返回 `#rrggbb`（用户取消返回 `None`）。
pub fn pick(state: &AppState, current: &str) -> Option<String> {
    if let Ok(hook) = std::env::var("ANKI_PICK_COLOUR") {
        if !hook.is_empty() {
            tracing::info!("ANKI_PICK_COLOUR 生效，跳过原生取色器: {hook}");
            remember(state, &hook);
            return Some(hook);
        }
    }
    pick_native(state, current)
}

/// 当前的 16 个色槽：弹过对话框就用对话框的，否则读集合配置。
fn dialog_palette(state: &AppState) -> Vec<String> {
    let stored = DIALOG_SLOTS.lock().expect("色槽被 poison").clone();
    let slots = stored.unwrap_or_else(|| slots_from_config(state));
    slots.iter().map(|slot| hex_from_colorref(*slot)).collect()
}

/// 集合配置里的调色板 → 色槽。
fn slots_from_config(state: &AppState) -> Vec<u32> {
    let mut slots = vec![0x00ff_ffffu32; SLOTS];
    for (index, colour) in read_palette(state).iter().take(SLOTS).enumerate() {
        slots[index] = colorref_from_hex(colour);
    }
    slots
}

/// 读集合配置里的调色板（与后端 `getCustomColours` 是同一把键）。
fn read_palette(state: &AppState) -> Vec<String> {
    match block_on(state.backend.call("getCustomColours", Empty {}.encode_to_vec())) {
        Ok(bytes) => GetCustomColoursResponse::decode(bytes.as_slice())
            .map(|response| response.colours)
            .unwrap_or_default(),
        Err(error) => {
            tracing::warn!("读取自定义颜色失败: {error}");
            Vec::new()
        }
    }
}

/// 写回调色板（`setConfigJson`，与上游 `col.set_config` 同一把键）。
fn write_palette(state: &AppState, colours: &[String]) -> Result<(), String> {
    let value_json = serde_json::to_vec(colours).map_err(|error| error.to_string())?;
    let request = SetConfigJsonRequest {
        key: PALETTE_KEY.to_string(),
        value_json,
        // 与 `preview.rs` 一致：这类界面状态不进撤销栈
        undoable: false,
    };
    block_on(state.backend.call("setConfigJson", request.encode_to_vec()))
        .map(|_| ())
        .map_err(|error| format!("写入自定义颜色失败: {error}"))
}

/// 记住刚选的颜色：塞进色槽的第一位，并立刻落库。
///
/// 上游是"页面下一次调 `saveCustomColours` 时落库"；这里选完就落库，最终状态一致
/// （而且钩子路径下没有"下一次点击"）。
fn remember(state: &AppState, colour: &str) {
    let value = colorref_from_hex(colour);
    let mut slots = DIALOG_SLOTS
        .lock()
        .expect("色槽被 poison")
        .clone()
        .unwrap_or_else(|| slots_from_config(state));
    if slots.first() != Some(&value) {
        slots.pop();
        slots.insert(0, value);
    }
    *DIALOG_SLOTS.lock().expect("色槽被 poison") = Some(slots.clone());
    let palette: Vec<String> = slots.iter().map(|slot| hex_from_colorref(*slot)).collect();
    if let Err(error) = write_palette(state, &palette) {
        tracing::warn!("{error}");
    }
}

#[cfg(windows)]
fn pick_native(state: &AppState, current: &str) -> Option<String> {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::Controls::Dialogs::{
        ChooseColorW, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW,
    };

    let slots = DIALOG_SLOTS
        .lock()
        .expect("色槽被 poison")
        .clone()
        .unwrap_or_else(|| slots_from_config(state));
    // `CHOOSECOLORW.lpCustColors` 要的是 `*mut COLORREF`（`COLORREF` 是 u32 的新类型）
    let mut dialog_slots: Vec<COLORREF> = slots.iter().map(|slot| COLORREF(*slot)).collect();
    let mut dialog = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        rgbResult: COLORREF(colorref_from_hex(current)),
        lpCustColors: dialog_slots.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    // 模态调用：一直转到用户关掉对话框
    let confirmed = unsafe { ChooseColorW(&mut dialog) }.as_bool();
    let updated: Vec<u32> = dialog_slots.iter().map(|slot| slot.0).collect();
    *DIALOG_SLOTS.lock().expect("色槽被 poison") = Some(updated);
    if !confirmed {
        tracing::info!("取色器：用户取消");
        return None;
    }
    let chosen = hex_from_colorref(dialog.rgbResult.0);
    remember(state, &chosen);
    Some(chosen)
}

/// 非 Windows 平台没有等价的系统对话框（Android 上页面自己有取色界面），
/// 桥接命令的能力标记也只在 Windows 打开，所以正常情况下走不到这里。
#[cfg(not(windows))]
fn pick_native(_state: &AppState, _current: &str) -> Option<String> {
    tracing::warn!("当前平台没有原生取色器");
    None
}

// 颜色换算：COLORREF 是 0x00BBGGRR，网页里是 #rrggbb ---------------------------

fn colorref_from_hex(colour: &str) -> u32 {
    let hex = colour.trim().trim_start_matches('#');
    let value = u32::from_str_radix(hex, 16).unwrap_or(0x00ff_ffff);
    let (r, g, b) = ((value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff);
    r | (g << 8) | (b << 16)
}

fn hex_from_colorref(colour: u32) -> String {
    let (r, g, b) = (colour & 0xff, (colour >> 8) & 0xff, (colour >> 16) & 0xff);
    format!("#{r:02x}{g:02x}{b:02x}")
}

/// 在阻塞线程里等一个异步调用（`platform::dispatch` 跑在 `spawn_blocking` 里，
/// 与 `platform::config` 的同一套办法）。
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(future)
}
