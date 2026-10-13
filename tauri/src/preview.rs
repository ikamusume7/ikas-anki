// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 预览窗口（对照 `qt/aqt/browser/previewer.py`）。
//!
//! Qt 的预览器是**独立顶层窗口**（`previewer.py:51` 的
//! `super().__init__(None, Qt.WindowType.Window)`）：上面是 reviewer 的 WebView，
//! 下面是 QDialogButtonBox（重播音频 / 仅背面 / 上一张 / 下一张）。
//! 这里沿用同样的形态，但按钮放在**页面里**（我们没有原生工具栏），
//! 通过 `pycmd` 回传（见 `legacy::preview_page`）。
//!
//! 渲染与音频都复用 reviewer 的代码路径：`renderExistingCard(browser=false)` +
//! `_showQuestion` / `_showAnswer`；音频走 `AVPlayer.play_tags` 的对应实现。
//!
//! 与 Qt 的差异：
//!
//! - 「上一张/下一张」不做禁用：卡片清单在浏览器页面手里，宿主没有它，
//!   到边界时点击无效果（Qt 会禁用按钮）；
//! - 快捷键保留 `R`（重播）、`B`（仅背面）、`←`/`→`（前后）、`Space`/`Enter`
//!   （显示答案 → 下一张）、`Esc` / `Ctrl+Shift+P`（关闭）。
//!
//! 「仅背面」的文案走的是 `qt-misc-back-side-only`：它属于 `ftl/qt` 仓库，
//! `anki_i18n` 的**类型化方法**只覆盖核心字符串（`Translation::is_core()` 排除了
//! `qt` / `mobile`），所以这里用底层的 `I18n::translate(key, None)` 取（字符串表本身
//! 包含全部模块，见 `rslib/i18n/write_strings.rs`）。

use std::sync::Mutex;

use anki_proto::cards::Card;
use anki_proto::cards::CardId;
use anyhow::Result;
use prost::Message as _;
use tauri::Manager as _;

use crate::browser;
use crate::legacy;
use crate::reviewer;
use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// 预览会话（窗口关闭时清空）。
#[derive(Default)]
pub struct Preview {
    inner: Mutex<Option<Session>>,
}

struct Session {
    card_id: i64,
    /// `false` = 问题面，`true` = 答案面（对照 `previewer.py` 的 `_state`）
    showing_answer: bool,
    /// 「仅背面」：问题与答案一起显示（bool 配置 `previewBothSides`）
    both_sides: bool,
    /// 卡片所属牌组是否允许自动播放（`Card.autoplay()`）
    autoplay: bool,
    question_html: String,
    answer_html: String,
    css: String,
}

impl Preview {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Session>> {
        self.inner.lock().expect("预览状态已中毒")
    }

    fn card_id(&self) -> Option<i64> {
        self.lock().as_ref().map(|session| session.card_id)
    }

    /// 关上预览窗口时调用（`windows.rs` 的 `Destroyed`）。
    pub fn clear(&self) {
        *self.lock() = None;
    }
}

/// 打开/切换预览（对照 `browser.py:853-865` 的 `onTogglePreview`）：
/// 同一个卡片再点一次是**关窗**，换一张卡片则是更新内容。
pub async fn toggle(state: &AppState, card_id: i64) -> Result<()> {
    if state.preview.card_id() == Some(card_id)
        && state.app.get_webview_window(windows::PREVIEW).is_some()
    {
        tracing::debug!("再次请求预览同一张卡片，关闭预览窗口");
        windows::close(state, windows::PREVIEW);
        return Ok(());
    }
    show(state, card_id).await
}

/// 显示某张卡片（问题面），必要时先把窗口开出来。
async fn show(state: &AppState, card_id: i64) -> Result<()> {
    let session = render(state, card_id).await?;
    let both_sides = session.both_sides;
    *state.preview.lock() = Some(session);
    tracing::info!("预览卡片: card={card_id}");
    // 窗口已存在说明页面早就加载完了，可以直接注入；否则等它报到（`domDone`）再注入 ——
    // 刚创建窗口就 eval 有可能会落空（与编辑器同一个坑）
    let existed = state.app.get_webview_window(windows::PREVIEW).is_some();
    ensure_window(state)?;
    // 「仅背面」沿用配置：打开时就是答案面（对照 `previewer.py:221-225`）
    if both_sides {
        set_showing_answer(state, true);
    }
    if existed {
        inject(state);
        autoplay(state).await;
    }
    Ok(())
}

/// 「仅背面」开关（对照 `previewer.py:278-293`）：写 bool 配置后重绘。
pub async fn set_both_sides(state: &AppState, value: bool) -> Result<()> {
    // 键名与 rslib 的 `BoolKey::PreviewBothSides` 一致
    // （`rslib/src/config/bool.rs`，strum 转成 camelCase）。
    // 走通用的 JSON 配置接口，免得从外面构造 BoolKey 的枚举值。
    let request = anki_proto::config::SetConfigJsonRequest {
        key: "previewBothSides".to_owned(),
        value_json: serde_json::to_vec(&value)?,
        undoable: false,
    };
    if let Err(error) = state
        .backend
        .call("setConfigJson", request.encode_to_vec())
        .await
    {
        tracing::warn!("写入 previewBothSides 失败: {error}");
    }

    tracing::info!("预览仅背面: {value}");
    if let Some(session) = state.preview.lock().as_mut() {
        session.both_sides = value;
        if !value {
            session.showing_answer = false;
        }
    }
    inject(state);
    if value {
        autoplay(state).await;
    }
    Ok(())
}

/// 预览页报到（`domDone`）：这时才注入内容。
pub async fn on_ready(state: &AppState) {
    inject(state);
    autoplay(state).await;
}

/// 上一张 / 下一张（对照 `previewer.py:344-362`）：
/// 问题面时 `next` 是「显示答案」，答案面时 `prev` 是「回到问题」。
pub async fn next(state: &AppState) {
    if showing_answer(state) {
        navigate(state, 1);
    } else {
        set_showing_answer(state, true);
        inject(state);
        autoplay(state).await;
    }
}

pub async fn prev(state: &AppState) {
    if showing_answer(state) {
        set_showing_answer(state, false);
        inject(state);
    } else {
        navigate(state, -1);
    }
}

/// 重播当前面的音频（对照 `previewer.py:124-134` 的 `_on_replay_audio`）。
pub async fn replay_audio(state: &AppState) {
    let Some((html, question_side)) = current_side_html(state) else {
        return;
    };
    match reviewer::extract_av_tags(state, &html, question_side).await {
        Ok(tags) => reviewer::play_av_tags(state, tags).await,
        Err(error) => tracing::warn!("提取音频标签失败: {error:#}"),
    }
}

/// 关掉预览窗口（浏览器窗口关闭时连带调用）。
pub fn close(state: &AppState) {
    state.preview.clear();
    windows::close(state, windows::PREVIEW);
}

// 内部 ####################################################################

fn showing_answer(state: &AppState) -> bool {
    state
        .preview
        .lock()
        .as_ref()
        .map(|session| session.showing_answer || session.both_sides)
        .unwrap_or(false)
}

fn set_showing_answer(state: &AppState, value: bool) {
    if let Some(session) = state.preview.lock().as_mut() {
        session.showing_answer = value;
    }
}

/// 当前这一面的 HTML（连同它是问题面还是答案面）。
fn current_side_html(state: &AppState) -> Option<(String, bool)> {
    let guard = state.preview.lock();
    let session = guard.as_ref()?;
    let showing = session.showing_answer || session.both_sides;
    Some((
        if showing {
            session.answer_html.clone()
        } else {
            session.question_html.clone()
        },
        !showing,
    ))
}

/// 请浏览器页面算出邻居再回一条 `preview:<cid>`。
fn navigate(state: &AppState, offset: i32) {
    let Some(card_id) = state.preview.card_id() else {
        return;
    };
    browser::request_neighbor(state, offset, card_id);
}

fn ensure_window(state: &AppState) -> Result<()> {
    let page = legacy::preview_page(state.port(), state.backend.i18n());
    windows::open(
        state,
        &WindowSpec::new(windows::PREVIEW, format!("_anki/legacyPageData?id={page}"))
            // 对照 `previewer.py:77` 的 `setWindowTitle(tr.actions_preview())`
            .title(state.backend.i18n().actions_preview())
            .size(700.0, 620.0),
    )?;
    Ok(())
}

/// 渲染卡片（`browser=false`，与 Qt 的 `card.question()`/`card.answer()` 一致）。
async fn render(state: &AppState, card_id: i64) -> Result<Session> {
    let rendered = reviewer::render_card(state, card_id).await?;
    let card = load_card(state, card_id).await?;
    let settings = reviewer::read_deck_settings(state, card.deck_id)
        .await
        .unwrap_or(None);
    Ok(Session {
        card_id,
        showing_answer: false,
        both_sides: matches!(
            state.backend.collection_config("previewBothSides").await,
            Some(serde_json::Value::Bool(true))
        ),
        autoplay: settings.map(|it| !it.disable_autoplay).unwrap_or(true),
        // 对照 `previewer.py:201-204`：预览里把 `[[type:Field]]` 删掉，不生成输入框
        question_html: strip_type_ans(&reviewer::nodes_to_html(&rendered.question_nodes)),
        answer_html: strip_type_ans(&reviewer::nodes_to_html(&rendered.answer_nodes)),
        css: rendered.css,
    })
}

/// 把当前面注入页面。问题面时答案也一起给（用于预加载图片，与 reviewer 一致）。
fn inject(state: &AppState) {
    let guard = state.preview.lock();
    let Some(session) = guard.as_ref() else {
        return;
    };
    let showing = session.showing_answer || session.both_sides;
    let answer = reviewer::with_css(&session.css, &session.answer_html);
    let js = if showing {
        format!("_showAnswer({}, '');", reviewer::js_string(&answer))
    } else {
        let question = reviewer::with_css(&session.css, &session.question_html);
        format!(
            "_showQuestion({}, {}, '');",
            reviewer::js_string(&question),
            reviewer::js_string(&answer)
        )
    };
    // 只在同一把锁里读完数据，避免把 guard 带进 eval
    let js = format!(
        "{js} previewSetState({}, {});",
        showing as u8, session.both_sides as u8
    );
    drop(guard);
    tracing::debug!("预览注入: 面={}", if showing { "答案" } else { "问题" });
    windows::eval(state, windows::PREVIEW, &js);
}

/// 自动播放（对照 `previewer.py:244-262`）：由牌组配置的 `disable_autoplay` 决定。
async fn autoplay(state: &AppState) {
    let allowed = state
        .preview
        .lock()
        .as_ref()
        .map(|session| session.autoplay)
        .unwrap_or(false);
    if !allowed {
        return;
    }
    let Some((html, question_side)) = current_side_html(state) else {
        return;
    };
    match reviewer::extract_av_tags(state, &html, question_side).await {
        Ok(tags) => reviewer::play_av_tags(state, tags).await,
        Err(error) => tracing::warn!("提取音频标签失败: {error:#}"),
    }
}

/// 对照 `previewer.py:201-204` 的 `type_ans_preview_filter`。
fn strip_type_ans(html: &str) -> String {
    static TYPE_ANS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\[\[type:[^\]]+\]\]").expect("正则无效"));
    TYPE_ANS.replace_all(html, "").into_owned()
}

async fn load_card(state: &AppState, card_id: i64) -> Result<Card> {
    let bytes = state
        .backend
        .call("getCard", CardId { cid: card_id }.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(Card::decode(bytes.as_slice())?)
}
