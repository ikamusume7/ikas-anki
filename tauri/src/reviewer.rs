// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 最小复习会话。
//!
//! 对照 Qt 的 `aqt/reviewer.py`：**取队列 → 渲染 → 注入 WebView → 处理评分**。
//! 与 Qt 保持同一个约定：卡片 HTML 交给 `js/reviewer.js` 提供的
//! `_showQuestion(q, a, bodyclass)` / `_showAnswer(a, bodyclass)` 全局函数。
//!
//! 当前实现的是主链路（显示问题 → 显示答案 → 四档评分 → 下一张）。
//! **未实现**：计时器与时间盒、撤销、自动播放与音频队列、类型作答（`type:`）、
//! 学习/复习队列计数显示、`getSchedulingStatesWithContext` 等界面状态同步。

use std::sync::Mutex;

use anki_proto::card_rendering::rendered_template_node;
use anki_proto::card_rendering::RenderCardResponse;
use anki_proto::card_rendering::RenderExistingCardRequest;
use anki_proto::card_rendering::RenderedTemplateNode;
use anki_proto::collection::OpChanges;
use anki_proto::scheduler::card_answer::Rating;
use anki_proto::scheduler::CardAnswer;
use anki_proto::scheduler::GetQueuedCardsRequest;
use anki_proto::scheduler::QueuedCards;
use anki_i18n::I18n;
use anki_proto::scheduler::SchedulingStates;
use tauri::Manager as _;

use crate::state::AppState;

#[derive(Default)]
pub struct Reviewer {
    inner: Mutex<Session>,
}

#[derive(Default)]
struct Session {
    card_id: Option<i64>,
    states: Option<SchedulingStates>,
    shown_at: Option<std::time::Instant>,
}

impl Reviewer {
    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.inner.lock().expect("复习会话已中毒")
    }
}

/// 开始（或继续）复习：取下一张卡片并显示问题。
pub async fn start(state: &AppState) {
    let request = GetQueuedCardsRequest {
        fetch_limit: 1,
        intraday_learning_only: false,
    };
    let queued: QueuedCards = match call(state, "getQueuedCards", &request).await {
        Ok(queued) => queued,
        Err(error) => {
            tracing::error!("取队列失败: {error:#}");
            show_message(state, &format!("无法取回卡片：{error}"));
            return;
        }
    };

    tracing::debug!(
        "队列: 共 {} 张（新 {} / 学习中 {} / 复习 {}）",
        queued.cards.len(),
        queued.new_count,
        queued.learning_count,
        queued.review_count
    );

    let Some(queued_card) = queued.cards.first() else {
        show_message(state, "本队列已无待复习卡片。");
        return;
    };

    let card_id = queued_card.card.as_ref().map(|card| card.id).unwrap_or(0);
    {
        let mut session = state.reviewer.session();
        session.card_id = Some(card_id);
        session.states = queued_card.states.clone();
        session.shown_at = Some(std::time::Instant::now());
    }

    // 底部栏只需注入一次（幂等，重复注入也无副作用）
    let bar = bottom_bar_html(state.backend.i18n());
    tracing::debug!("底部栏（{} 字节）: {}", bar.len(), bar);
    eval(
        state,
        &format!(
            "document.getElementById('bottom-container').innerHTML = {};",
            js_string(&bar)
        ),
    );

    show_question(state).await;
}

/// 显示答案（对照 Qt 的 `Reviewer._showAnswer`）。
pub async fn show_answer(state: &AppState) {
    let Some(card_id) = state.reviewer.session().card_id else {
        return;
    };
    let rendered: RenderCardResponse = match render_card(state, card_id).await {
        Ok(rendered) => rendered,
        Err(error) => {
            tracing::error!("渲染答案失败: {error:#}");
            return;
        }
    };
    eval(
        state,
        &format!(
            "_showAnswer({}, '', {})",
            js_string(&with_css(&rendered.css, &nodes_to_html(&rendered.answer_nodes))),
            // bodyclass 为空字符串时前端不会覆盖
            js_string(""),
        ),
    );

    // 对照 Qt 的 `_showEaseButtons`：把「显示答案」换成四档评分按钮
    let buttons = ease_buttons(state.backend.i18n());
    eval(state, &format!("showAnswer({}, true);", js_string(&buttons)));
}

/// 提交评分（`ease` 为 1..4），随后显示下一张卡片。
pub async fn answer(state: &AppState, ease: u32) {
    let (card_id, current, next) = {
        let session = state.reviewer.session();
        let Some(card_id) = session.card_id else {
            return;
        };
        let Some(states) = session.states.as_ref() else {
            return;
        };
        let (current, next) = match ease {
            1 => (states.current.clone(), states.again.clone()),
            2 => (states.current.clone(), states.hard.clone()),
            3 => (states.current.clone(), states.good.clone()),
            _ => (states.current.clone(), states.easy.clone()),
        };
        (card_id, current, next)
    };

    let (Some(current), Some(next)) = (current, next) else {
        tracing::warn!("卡片缺少调度状态，无法评分");
        return;
    };

    let rating = match ease {
        1 => Rating::Again,
        2 => Rating::Hard,
        3 => Rating::Good,
        _ => Rating::Easy,
    };

    let milliseconds_taken = state
        .reviewer
        .session()
        .shown_at
        .map(|at| at.elapsed().as_millis() as u32)
        .unwrap_or_default();

    let answer = CardAnswer {
        card_id,
        current_state: Some(current),
        new_state: Some(next),
        rating: rating as i32,
        answered_at_millis: now_millis(),
        milliseconds_taken,
    };

    if let Err(error) = call::<_, OpChanges>(state, "answerCard", &answer).await {
        tracing::error!("评分失败: {error:#}");
        return;
    }

    start(state).await;
}

async fn show_question(state: &AppState) {
    let Some(card_id) = state.reviewer.session().card_id else {
        return;
    };
    let rendered: RenderCardResponse = match render_card(state, card_id).await {
        Ok(rendered) => rendered,
        Err(error) => {
            tracing::error!("渲染问题失败: {error:#}");
            show_message(state, &format!("渲染卡片失败：{error}"));
            return;
        }
    };

    let question = with_css(&rendered.css, &nodes_to_html(&rendered.question_nodes));
    // 答案侧用于预加载图片，第二个参数不能为空
    let answer = with_css(&rendered.css, &nodes_to_html(&rendered.answer_nodes));

    tracing::debug!(
        "渲染完成: 问题 {} 字节 / 答案 {} 字节 / 样式 {} 字节",
        question.len(),
        answer.len(),
        rendered.css.len()
    );

    eval(
        state,
        &format!(
            "_showQuestion({}, {}, {})",
            js_string(&question),
            js_string(&answer),
            js_string(""),
        ),
    );

    // 对照 Qt 的 `_showAnswerButton`
    let button = show_answer_button(state.backend.i18n());
    eval(state, &format!("showQuestion({}, 0);", js_string(&button)));
}

async fn render_card(state: &AppState, card_id: i64) -> anyhow::Result<RenderCardResponse> {
    let request = RenderExistingCardRequest {
        card_id,
        browser: false,
        partial_render: false,
    };
    call(state, "renderExistingCard", &request).await
}

/// 通过方法名调用 rslib 服务，并把响应解码为具体类型。
async fn call<Req, Resp>(state: &AppState, method: &str, request: &Req) -> anyhow::Result<Resp>
where
    Req: prost::Message,
    Resp: prost::Message + Default,
{
    let bytes = state
        .backend
        .call(method, request.encode_to_vec())
        .await
        .map_err(anyhow::Error::msg)?;
    Resp::decode(bytes.as_slice()).map_err(|error| anyhow::anyhow!("解析响应失败: {error}"))
}

/// 对照 `Reviewer._bottomHTML()`。
///
/// Qt 把它渲染在独立的底部 WebView 中；这里放在同一页面的
/// `#bottom-container` 里。按钮通过全局的 `pycmd` 回传命令。
fn bottom_bar_html(tr: &I18n) -> String {
    let edit = tr.studying_edit();
    let more = tr.studying_more();
    let edit_key = tr.actions_shortcut_key("E");
    let more_key = tr.actions_shortcut_key("M");
    format!(
        r#"<center id="outer">
<table id="innertable" width="100%" cellspacing="0" cellpadding="0">
<tr>
<td align="start" valign="top" class="stat"><button title="{edit_key}" onclick="pycmd('edit');">{edit}</button></td>
<td align="center" valign="top" id="middle"></td>
<td align="end" valign="top" class="stat"><button title="{more_key}" onclick="pycmd('more');">{more} <span id="time" class="stattxt"></span></button></td>
</tr>
</table>
</center>"#
    )
}

/// 对照 `Reviewer._showAnswerButton()`。
fn show_answer_button(tr: &I18n) -> String {
    let label = tr.studying_show_answer();
    let key = tr.actions_shortcut_key(tr.studying_space());
    format!(
        r#"<table cellpadding="0"><tr><td class="stat2" align="center"><button title="{key}" id="ansbut" onclick='pycmd("ans");'>{label}</button></td></tr></table>"#
    )
}

/// 对照 `Reviewer._answerButtons()`。
///
/// 简化之处：Qt 会根据牌组配置与调度器的 `answerButtons()` 决定显示 2/3/4 个
/// 按钮，并在按钮上标注下次间隔（`describe_next_states`）。这里固定四个按钮、
/// 不显示间隔。
fn ease_buttons(tr: &I18n) -> String {
    let mut out = String::from(r#"<table cellpadding="0"><tr>"#);
    for (ease, label) in [
        (1, tr.studying_again()),
        (2, tr.studying_hard()),
        (3, tr.studying_good()),
        (4, tr.studying_easy()),
    ] {
        out.push_str(&format!(
            r#"<td align="center"><button data-ease="{ease}" onclick='pycmd("ease{ease}");'>{label}</button></td>"#
        ));
    }
    out.push_str("</tr></table>");
    out
}

/// 对照 `TemplateRenderOutput.question_and_style()`：样式与内容一起注入。
fn with_css(css: &str, html: &str) -> String {
    format!("<style>{css}</style>{html}")
}

/// 把结构化渲染结果拼成 HTML。
///
/// 对照 `pylib/anki/template.py` 的 `PartiallyRenderedCard.nodes_from_proto`。
fn nodes_to_html(nodes: &[RenderedTemplateNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node.value.as_ref() {
            Some(rendered_template_node::Value::Text(text)) => out.push_str(text),
            Some(rendered_template_node::Value::Replacement(replacement)) => {
                out.push_str(&replacement.current_text);
            }
            None => {}
        }
    }
    out
}

fn show_message(state: &AppState, message: &str) {
    eval(
        state,
        &format!(
            "_showQuestion({}, '', '')",
            js_string(&format!("<p>{message}</p>"))
        ),
    );
}

/// 把任意字符串安全地嵌入 JS 源码。
fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn eval(state: &AppState, js: &str) {
    let Some(window) = state.app.get_webview_window("main") else {
        tracing::warn!("找不到主窗口，无法注入脚本");
        return;
    };
    if let Err(error) = window.eval(js) {
        tracing::warn!("注入脚本失败: {error}");
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
