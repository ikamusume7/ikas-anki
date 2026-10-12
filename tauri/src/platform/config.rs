// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! profile / meta 配置存储。
//!
//! 对照 mediasrv 的 `get_profile_config_json` / `set_profile_config_json` /
//! `get_meta_json` / `set_meta_json`：它们读写的是 `aqt.mw.pm.profile` / `pm.meta`，
//! 也就是桌面版的 `prefs21.db`。本模块直接操作同一份文件（见 `super::prefs21`），
//! 因此外壳与桌面版共享设置 —— 不再有第二份存储。
//!
//! 注意与 Qt 版一致的两个行为：
//!
//! - 项不存在时返回 `null`（`profile.get()` 得到 `None`，序列化成 `null`），
//!   而不是报错；
//! - 取到的值无法用 JSON 表示时（例如 `mainWindowGeom` 是 `bytes`）返回错误 ——
//!   Qt 版用 orjson 序列化同样会失败，这里保持相同的语义，而不是静默返回 `null`。

use anki_proto::frontend::SetSettingJsonRequest;
use anki_proto::generic::Empty;
use anki_proto::generic::Json as GenericJson;
use anki_proto::generic::String as GenericString;
use prost::Message as _;
use serde_json::Value;

use super::prefs21::Prefs21;
use super::prefs21::Store;
use super::HandlerResult;
use crate::state::AppState;

pub fn get_profile_config_json(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|error| error.to_string())?;
    let value = prefs21(state)?.get(Store::Profile, &request.val)?;
    json_response(value.unwrap_or(Value::Null))
}

pub fn set_profile_config_json(state: &AppState, body: &[u8]) -> HandlerResult {
    set_json(state, Store::Profile, body)
}

pub fn get_meta_json(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|error| error.to_string())?;
    let value = prefs21(state)?.get(Store::Meta, &request.val)?;
    json_response(value.unwrap_or(Value::Null))
}

pub fn set_meta_json(state: &AppState, body: &[u8]) -> HandlerResult {
    set_json(state, Store::Meta, body)
}

/// 读**集合级**配置。对照 mediasrv 的 `get_config_json`（`qt/aqt/mediasrv.py:834-838`）：
/// **缺键时返回 `null`**，而不是报错。
///
/// 为什么不直接用 rslib 的同名方法：rslib 的 `getConfigJson` 对缺失的键会返回错误
/// （`No such value: 'xxx'`），而 Qt 在 mediasrv 里自己实现了这个端点，用
/// `except KeyError: null` 兜底。差别会真的咬人 —— 新建集合的 `col.conf` 里没有
/// `renderMathjax` / `shrinkEditorImages` / `closeHTMLTags` 这些布尔项（它们的默认值
/// 在 rslib 的注册表里，不落库），编辑器一打开就会因为读不到而弹错误框，
/// 而且 `alert()` 会阻塞页面的 JS 事件循环。
pub fn get_collection_config_json(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|error| error.to_string())?;

    // 没有「取单个键且不报错」的接口，只能整体取出再挑（集合配置本身很小）
    let all = block_on(state.backend.call("getAllConfig", Empty {}.encode_to_vec()))
        .map_err(|error| format!("读取集合配置失败: {error}"))?;
    let raw = GenericJson::decode(all.as_slice()).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_slice(&raw.json).map_err(|error| error.to_string())?;

    json_response(value.get(&request.val).cloned().unwrap_or(Value::Null))
}

/// 在阻塞线程里等一个异步调用。
///
/// `platform::dispatch` 跑在 `spawn_blocking` 里（见 `server/api.rs`），不在异步
/// 上下文中，因此这里可以直接 `block_on`，不会与运行时的任务互相饿死。
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Handle::current().block_on(future)
}

fn set_json(state: &AppState, store: Store, body: &[u8]) -> HandlerResult {
    let request = SetSettingJsonRequest::decode(body).map_err(|error| error.to_string())?;
    let value: Value =
        serde_json::from_slice(&request.value_json).map_err(|error| error.to_string())?;
    prefs21(state)?.set(store, &request.key, &value)?;
    Ok(Vec::new())
}

/// 打开当前 profile 的 `prefs21.db`。
fn prefs21(state: &AppState) -> Result<Prefs21, String> {
    Prefs21::locate(state.profile_dir.as_path())
        .ok_or_else(|| format!("无法从 profile 目录推断 prefs21.db: {}", state.profile_dir.display()))
}

fn json_response(value: Value) -> HandlerResult {
    Ok(GenericJson {
        json: serde_json::to_vec(&value).map_err(|error| error.to_string())?,
    }
    .encode_to_vec())
}
