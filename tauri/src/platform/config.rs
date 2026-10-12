// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! profile / meta 配置存储。
//!
//! 对照 mediasrv 的 `get_profile_config_json` / `set_profile_config_json` /
//! `get_meta_json` / `set_meta_json`（原实现读写 Anki 的 `prefs21.db`）。
//!
//! 外壳用一个 JSON 文件保存，语义与前端一致（按 key 存任意 JSON 值）。
//! 注意：这与桌面版的 `prefs21.db` 不共享，profile 目录切换后需要重新配置。

use std::path::Path;
use std::path::PathBuf;

use anki_proto::frontend::SetSettingJsonRequest;
use anki_proto::generic::Json as GenericJson;
use anki_proto::generic::String as GenericString;
use prost::Message as _;
use serde_json::Map;
use serde_json::Value;

use super::HandlerResult;
use crate::state::AppState;

const PROFILE_STORE: &str = "profile";
const META_STORE: &str = "meta";

pub fn get_profile_config_json(state: &AppState, body: &[u8]) -> HandlerResult {
    get_json(&state.profile_dir, PROFILE_STORE, body)
}

pub fn set_profile_config_json(state: &AppState, body: &[u8]) -> HandlerResult {
    set_json(&state.profile_dir, PROFILE_STORE, body)
}

pub fn get_meta_json(state: &AppState, body: &[u8]) -> HandlerResult {
    get_json(&state.profile_dir, META_STORE, body)
}

pub fn set_meta_json(state: &AppState, body: &[u8]) -> HandlerResult {
    set_json(&state.profile_dir, META_STORE, body)
}

fn store_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("tauri-{name}.json"))
}

fn load(dir: &Path, name: &str) -> Map<String, Value> {
    let Ok(bytes) = std::fs::read(store_path(dir, name)) else {
        return Map::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save(dir: &Path, name: &str, map: &Map<String, Value>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(map).map_err(|e| e.to_string())?;
    std::fs::write(store_path(dir, name), bytes).map_err(|e| e.to_string())
}

fn get_json(dir: &Path, name: &str, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    let value = load(dir, name)
        .get(&request.val)
        .cloned()
        .unwrap_or(Value::Null);
    Ok(GenericJson {
        json: serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    }
    .encode_to_vec())
}

fn set_json(dir: &Path, name: &str, body: &[u8]) -> HandlerResult {
    let request = SetSettingJsonRequest::decode(body).map_err(|e| e.to_string())?;
    let value: Value =
        serde_json::from_slice(&request.value_json).map_err(|e| e.to_string())?;

    let mut map = load(dir, name);
    map.insert(request.key, value);
    save(dir, name, &map)?;
    Ok(Vec::new())
}
