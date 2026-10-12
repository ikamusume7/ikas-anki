// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 复习界面的调度状态同步端点。
//!
//! 对照 mediasrv 的 `get_scheduling_states_with_context` /
//! `set_scheduling_states`（原实现转发给 `aqt.mw.reviewer` 的
//! `get_scheduling_states` / `get_scheduling_context` / `set_scheduling_states`）。
//!
//! 前端 `ts/reviewer/answering.ts` 用它们实现自定义调度：读取当前卡的调度状态与
//! 上下文，用用户的 `cardStateCustomizer` 脚本改写，再写回。写回时要带上前端拿到的
//! key，宿主用它丢弃过期的变更。

use anki_proto::frontend::SchedulingStatesWithContext;
use anki_proto::frontend::SetSchedulingStatesRequest;
use prost::Message as _;

use super::HandlerResult;
use crate::reviewer;
use crate::state::AppState;

pub fn get_states_with_context(state: &AppState, _body: &[u8]) -> HandlerResult {
    let Some((states, context)) = reviewer::states_with_context(state) else {
        return Err("复习会话尚未开始".to_string());
    };
    Ok(SchedulingStatesWithContext {
        states: Some(states),
        context: Some(context),
    }
    .encode_to_vec())
}

pub fn set_states(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = SetSchedulingStatesRequest::decode(body).map_err(|error| error.to_string())?;
    let Some(states) = request.states else {
        return Err("请求里没有调度状态".to_string());
    };
    // key 不匹配时静默忽略（对照 Qt），避免过期变更覆盖新状态
    reviewer::apply_states_mutation(state, &request.key, states);
    Ok(Vec::new())
}
