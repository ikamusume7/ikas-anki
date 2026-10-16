// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 最小复习会话。
//!
//! 对照 Qt 的 `aqt/reviewer.py`：**取队列 → 渲染 → 注入 WebView → 处理评分**。
//! 与 Qt 保持同一个约定：卡片 HTML 交给 `js/reviewer.js` 提供的
//! `_showQuestion(q, a, bodyclass)` / `_showAnswer(a, bodyclass)` 全局函数。
//!
//! 当前实现的是主链路（显示问题 → 显示答案 → 四档评分 → 下一张），以及计时器、
//! 撤销、自动播放与音频队列、类型作答（`type:`）、队列计数显示、
//! `getSchedulingStatesWithContext` 界面状态同步、顶部/底部工具栏与「更多」菜单。
//!
//! **未实现**（见 `tauri/PARITY.md`）：
//! - **键盘答题**：只绑了 `Ctrl+Z`/`Ctrl+P` 与裸 `u`/`a`/`b`/`t`/`d`/`s`，
//!   Qt `_shortcutKeys` 里的空格 / 回车显示答案、`1`–`4` 评分等一个都没绑；
//! - **答完队列的收尾**：只注入一句提示，没有 Congratulations 页与概览跳转；
//! - **时间盒**（`timeboxReached` 与「继续 / 结束」对话框）；
//! - **音频**：卡面不画播放按钮（没有 `play:q:N`）、没有重播 / 暂停 / ±5 秒 / 录音，
//!   答案面还会把问题面音频再播一遍（`partial_render` 的差异）。

use std::sync::LazyLock;
use std::sync::Mutex;

use regex::Regex;

use anki_proto::card_rendering::av_tag;
use anki_proto::card_rendering::rendered_template_node;
use anki_proto::card_rendering::AvTag;
use anki_proto::card_rendering::CompareAnswerRequest;
use anki_proto::card_rendering::ExtractAvTagsRequest;
use anki_proto::card_rendering::ExtractAvTagsResponse;
use anki_proto::card_rendering::ExtractClozeForTypingRequest;
use anki_proto::cards::Card;
use anki_proto::notes::Note;
use anki_proto::notes::NoteId;
use anki_proto::notetypes::NotetypeId;
use anki_proto::card_rendering::RenderCardResponse;
use anki_proto::card_rendering::RenderExistingCardRequest;
use anki_proto::card_rendering::RenderedTemplateNode;
use anki_proto::collection::OpChanges;
use anki_proto::collection::OpChangesAfterUndo;
use anki_proto::deck_config::deck_config::Config as DeckSettings;
use anki_proto::deck_config::DeckConfig;
use anki_proto::deck_config::DeckConfigId;
use anki_proto::decks::deck;
use anki_proto::decks::Deck;
use anki_proto::decks::DeckId;
use anki_proto::generic::StringList;
use anki_proto::scheduler::card_answer::Rating;
use anki_proto::scheduler::CardAnswer;
use anki_proto::scheduler::GetQueuedCardsRequest;
use anki_proto::scheduler::QueuedCards;
use anki_i18n::I18n;
use anki_proto::scheduler::SchedulingStates;
use anki_proto::config::Preferences;
use anki_proto::generic::Empty;
use anki_proto::generic::Json as GenericJson;
use anki_proto::generic::String as GenericString;
use anki_proto::scheduler::queued_cards::Queue;
use anki_proto::scheduler::SchedulingContext;
use serde_json::json;

use crate::platform::audio;
use crate::state::AppState;

#[derive(Default)]
pub struct Reviewer {
    inner: Mutex<Session>,
}

#[derive(Default)]
struct Session {
    card_id: Option<i64>,
    /// 当前卡片（类型作答需要它的 note 与模板序号）
    card: Option<Card>,
    /// 类型作答（`[[type:Field]]`）的运行时状态；无类型作答时为 `None`
    type_answer: Option<TypeAnswer>,
    /// 当前卡片的牌组配置：计时器、自动播放、作答后是否停表等都取自这里
    deck_settings: Option<DeckSettings>,
    /// 复习界面的显示开关（对照 `col.conf` 的 `dueCounts` / `estTimes`）
    ui: ReviewerUi,
    /// 队列计数：新 / 学习中 / 复习
    counts: [u32; 3],
    /// 当前卡片属于哪个队列（0=新 1=学习中 2=复习），用于给对应计数加下划线
    queue_index: usize,
    states: Option<SchedulingStates>,
    /// 当前卡片的调度上下文，`getSchedulingStatesWithContext` 需要
    context: Option<SchedulingContext>,
    /// 自定义调度的变更密钥；整个复习会话保持不变（对照 Qt 在构造时生成一次）
    state_mutation_key: Option<String>,
    /// 用户的 `cardStateCustomizer`；为空表示没有自定义调度
    state_mutation_js: Option<String>,
    /// 本张卡的自定义调度是否已完成（完成后才显示评分按钮）
    states_mutated: bool,
    shown_at: Option<std::time::Instant>,
}

/// 类型作答的运行时状态（对照 Qt 的 `typeCorrect` / `_combining` / `_ignore_case`
/// 与字段的字体设置）。
#[derive(Clone)]
struct TypeAnswer {
    /// 正确答案（`[[type:Field]]` 指向的字段内容，必要时已按 cloze 处理）
    correct: String,
    /// 是否忽略组合字符（修饰符 `nc` 关闭）
    combining: bool,
    /// 是否忽略大小写（修饰符 `ci` 开启）
    ignore_case: bool,
    font: String,
    size: u32,
}

/// 对照 `col.conf` 里控制复习界面显示的两项配置。
///
/// 手写 `Default`：Anki 的默认值是两项都开启（`dueCounts` / `estTimes`），
/// 读取失败时应当按默认显示，而不是静默隐藏。
#[derive(Clone, Copy)]
struct ReviewerUi {
    show_remaining_counts: bool,
    show_intervals: bool,
}

impl Default for ReviewerUi {
    fn default() -> Self {
        Self {
            show_remaining_counts: true,
            show_intervals: true,
        }
    }
}

impl Reviewer {
    fn session(&self) -> std::sync::MutexGuard<'_, Session> {
        self.inner.lock().expect("复习会话已中毒")
    }

    /// 当前正在复习的卡片。
    ///
    /// `crate::editor` 用它实现「编辑当前卡片」（对照 Qt 的 `mw.reviewer.card`）。
    pub(crate) fn current_card(&self) -> Option<Card> {
        self.session().card.clone()
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

    let card = queued_card.card.as_ref();
    let card_id = card.map(|card| card.id).unwrap_or(0);
    // 对照 `Card.current_deck_id()`：筛选牌组里的卡片沿用原牌组的配置
    let deck_id = card
        .map(|card| {
            if card.original_deck_id != 0 {
                card.original_deck_id
            } else {
                card.deck_id
            }
        })
        .unwrap_or(0);

    // 对照 `V3CardInfo.counts()`：队列计数 + 当前卡片所在队列的下标
    let counts = [queued.new_count, queued.learning_count, queued.review_count];
    let queue_index = match queued_card.queue() {
        Queue::New => 0,
        Queue::Learning => 1,
        Queue::Review => 2,
    };

    // 牌组配置一次查好放进会话：显示问题/答案与自动播放都要用它
    let deck_settings = match read_deck_settings(state, deck_id).await {
        Ok(settings) => settings,
        Err(error) => {
            tracing::warn!("读取牌组配置失败，按默认值处理: {error:#}");
            None
        }
    };

    // 复习界面的显示开关（剩余计数 / 按钮间隔）同样每张卡开始时读一次
    let ui = match read_reviewer_ui(state).await {
        Ok(ui) => ui,
        Err(error) => {
            tracing::warn!("读取复习界面开关失败，按默认值处理: {error:#}");
            ReviewerUi::default()
        }
    };

    // 自定义调度（对照 `Reviewer.show()` 读 `cardStateCustomizer`）。
    // 没有自定义脚本时不需要等待，直接视为已完成。
    let state_mutation_js = match read_state_mutation_js(state).await {
        Ok(js) => js,
        Err(error) => {
            tracing::warn!("读取 cardStateCustomizer 失败: {error:#}");
            None
        }
    };
    let states_mutated = state_mutation_js.is_none();

    {
        let mut session = state.reviewer.session();
        session.card_id = Some(card_id);
        session.card = card.cloned();
        session.type_answer = None;
        session.deck_settings = deck_settings;
        session.ui = ui;
        session.counts = counts;
        session.queue_index = queue_index;
        session.states = queued_card.states.clone();
        session.context = queued_card.context.clone();
        session.state_mutation_js = state_mutation_js;
        session.states_mutated = states_mutated;
        // 对照 Qt 的 `_state_mutation_key`：整个复习会话保持不变
        if session.state_mutation_key.is_none() {
            session.state_mutation_key = Some(rand::random::<u64>().to_string());
        }
        session.shown_at = Some(std::time::Instant::now());
    }

    install_toolbar(state);

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
    sync_bottom_bar_height(state);
    install_reviewer_shortcuts(state);
    crate::reviewer_menu::install(state);

    show_question(state).await;
}

/// 撤销上一次操作（对照 Qt 的 `mw.undo()` → `operations/collection.py:undo()`）。
///
/// 撤销会改变卡片的调度状态（通常是把上一张卡放回队列），所以完成后必须重新
/// 取队列并重新渲染。
pub async fn undo(state: &AppState) {
    match call::<_, OpChangesAfterUndo>(state, "undo", &Empty {}).await {
        Ok(_) => tracing::debug!("已撤销上一步操作"),
        Err(error) => {
            // 没有可撤销的操作时 rslib 也会返回错误，属于正常情况
            tracing::warn!("撤销失败: {error:#}");
            return;
        }
    }
    start(state).await;
}

/// 复习界面的快捷键。
///
/// Qt 用 QShortcut 注册（`Reviewer._shortcutKeys()` 里的 `u`，以及主窗口的
/// Ctrl+Z、全局 `a` 与 `b`），单页方案里改为在页面内监听键盘。这里只装撤销、
/// 添加卡片与浏览器，且裸键会避开输入框（类型作答时 `u` / `a` / `b` 都是普通字符）。
fn install_reviewer_shortcuts(state: &AppState) {
    eval(
        state,
        r#"(function () {
    if (window.__ankiReviewerShortcuts) {
        return;
    }
    window.__ankiReviewerShortcuts = true;
    function inTextInput(target) {
        if (!target) {
            return false;
        }
        var name = target.tagName;
        return name === "INPUT" || name === "TEXTAREA" || target.isContentEditable === true;
    }
    document.addEventListener("keydown", function (event) {
        if (event.altKey) {
            return;
        }
        if (event.ctrlKey || event.metaKey) {
            if (event.key === "z" || event.key === "Z") {
                event.preventDefault();
                window.pycmd("undo");
            } else if (event.key === "p" || event.key === "P") {
                // 对照 `forms/main.ui` 的 `actionPreferences`（Ctrl+P）→ `onPrefs()`
                event.preventDefault();
                window.pycmd("preferences");
            }
            return;
        }
        if (inTextInput(event.target)) {
            return;
        }
        if (event.key === "u" || event.key === "U") {
            event.preventDefault();
            window.pycmd("undo");
        } else if (event.key === "a" || event.key === "A") {
            // 对照 Qt 主窗口的全局快捷键 `a` → `mw.onAddCard()`（`main.py:1183`）
            event.preventDefault();
            window.pycmd("add");
        } else if (event.key === "b" || event.key === "B") {
            // 对照 Qt 主窗口的全局快捷键 `b` → `mw.onBrowse()`
            event.preventDefault();
            window.pycmd("browse");
        } else if (event.key === "t" || event.key === "T") {
            // 对照 Qt 主窗口的全局快捷键 `t` / `Shift+t` → `mw.onStats()`（`main.py:1185`）
            event.preventDefault();
            window.pycmd("stats");
        } else if (event.key === "d" || event.key === "D") {
            // 对照 `d` → `moveToState("deckBrowser")`（`main.py:1181`）：回牌组列表
            event.preventDefault();
            window.pycmd("decks");
        } else if (event.key === "s" || event.key === "S") {
            // 对照 `s` → `onStudyKey()`（`main.py:1240-1245`）：复习状态下进概览
            event.preventDefault();
            window.pycmd("overview");
        }
    });
})();"#,
    );
}

/// 显示答案（对照 Qt 的 `Reviewer._showAnswer`）。
pub async fn show_answer(state: &AppState, typed_answer: Option<String>) {
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
    // 对照 `_mungeQA` → `typeAnsAnswerFilter`：把 `[[type:Field]]` 换成答案对比
    let answer_html = nodes_to_html(&rendered.answer_nodes);
    let type_answer = state.reviewer.session().type_answer.clone();
    let answer_html = apply_type_answer_answer(
        state,
        &answer_html,
        typed_answer.as_deref(),
        type_answer.as_ref(),
    )
    .await;

    // 对照 Qt `_showAnswer`：自动播放答案面音频
    autoplay_audio(state, false, &answer_html).await;

    eval(
        state,
        &format!(
            "_showAnswer({}, '', {})",
            js_string(&with_css(&rendered.css, &answer_html)),
            // bodyclass 为空字符串时前端不会覆盖
            js_string(""),
        ),
    );

    // 对照 Qt 的 `_showEaseButtons`：把「显示答案」换成四档评分按钮，
    // 第二个参数是牌组的 `stopTimerOnAnswer`（作答后是否停表）
    //
    // 对照 Qt `_showEaseButtons`：自定义调度未完成时先等它（Qt 是 50ms 后重试）
    wait_for_states_mutation(state).await;

    // 先取出 states 再 await：MutexGuard 不能跨 await 持有
    let (states, show_intervals) = {
        let session = state.reviewer.session();
        (session.states.clone(), session.ui.show_intervals)
    };
    let next_intervals = match states {
        Some(states) => match describe_next_states(state, &states).await {
            Ok(labels) => labels,
            Err(error) => {
                tracing::warn!("获取下次间隔失败: {error:#}");
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let buttons = ease_buttons(state.backend.i18n(), &next_intervals, show_intervals);
    let stop_timer = state
        .reviewer
        .session()
        .deck_settings
        .as_ref()
        .map(|settings| settings.stop_timer_on_answer)
        .unwrap_or(false);
    tracing::debug!("评分按钮: stopTimerOnAnswer={stop_timer} 间隔={next_intervals:?}");
    eval(
        state,
        &format!("showAnswer({}, {});", js_string(&buttons), stop_timer),
    );
    // 换成评分按钮后底部栏宽度/换行可能变化
    sync_bottom_bar_height(state);
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

    let question_html = nodes_to_html(&rendered.question_nodes);
    // 对照 `_mungeQA` → `typeAnsQuestionFilter`：把 `[[type:Field]]` 换成输入框。
    // 先把 card 取出来再 await：MutexGuard 不能跨 await 持有。
    let card = state.reviewer.session().card.clone();
    let (question_html, type_answer) = match card {
        Some(card) => apply_type_answer_question(state, &card, &question_html).await,
        None => (question_html, None),
    };
    state.reviewer.session().type_answer = type_answer;

    // 对照 Qt `_showQuestion`：先按牌组配置自动播放问题面音频，再注入渲染结果
    autoplay_audio(state, true, &question_html).await;

    let question = with_css(&rendered.css, &question_html);
    // 答案侧用于预加载图片，第二个参数不能为空
    let answer = with_css(&rendered.css, &nodes_to_html(&rendered.answer_nodes));

    tracing::debug!(
        "渲染完成: 问题 {} 字节 / 答案 {} 字节 / 样式 {} 字节",
        question.len(),
        answer.len(),
        rendered.css.len()
    );

    // 对照 Qt `_showQuestion` 里的 `_run_state_mutation_hook()`
    run_state_mutation(state);

    eval(
        state,
        &format!(
            "_showQuestion({}, {}, {})",
            js_string(&question),
            js_string(&answer),
            js_string(""),
        ),
    );

    // 对照 Qt 的 `_showAnswerButton` 与计时器：`showQuestion(button, maxTime)`。
    // maxTime 为 0 时前端会清空计时显示（即牌组未开启计时器）。
    let remaining = remaining_html(state);
    tracing::debug!("剩余计数: {remaining}");
    let button = show_answer_button(state.backend.i18n(), &remaining);
    let max_time = max_time_secs(state);
    tracing::debug!("计时器: maxTime={max_time} 秒");
    eval(
        state,
        &format!("showQuestion({}, {});", js_string(&button), max_time),
    );
    sync_bottom_bar_height(state);
}

/// 把底部栏高度写进 CSS 变量，供卡片区留白使用（见 `legacy::REVIEWER_ISOLATION_CSS`）。
///
/// 对照 Qt：底部栏是独立控件，留白由 Qt 的布局负责；本方案两个界面共用一个
/// 页面，因此由脚本把实测高度告诉样式表，而不是在 CSS 里写死数值。
/// 顶部工具栏的链接（对照 `toolbar.py:353-389` 的 `_centerLinks`）。
///
/// 顺序与文案都和 Qt 一致：牌组 / 添加 / 浏览 / 统计 / **同步**，提示是各自的全局快捷键
/// （`actions-shortcut-key`）。同步链接走宿主的 `sync` 命令，状态用
/// `window.__ankiSyncStatus()` 写回工具栏 —— 目前只有文字，没有 Qt 的状态色与旋转图标；
/// 左右两个托盘（add-ons 用）也没有（`tauri/PARITY.md` 1.2 / 6.1）。
fn toolbar_payload(tr: &I18n) -> serde_json::Value {
    json!({
        "hostId": "top-toolbar",
        "links": [
            {
                "id": "decks",
                "cmd": "decks",
                "label": tr.actions_decks(),
                "tip": tr.actions_shortcut_key("D"),
            },
            {
                "id": "add",
                "cmd": "add",
                "label": tr.actions_add(),
                "tip": tr.actions_shortcut_key("A"),
            },
            {
                "id": "browse",
                "cmd": "browse",
                // 浏览 / 统计住在 ftl/qt/qt-misc.ftl（没有类型化函数）
                "label": tr.translate("qt-misc-browse", None),
                "tip": tr.actions_shortcut_key("B"),
            },
            {
                "id": "stats",
                "cmd": "stats",
                "label": tr.translate("qt-misc-stats", None),
                "tip": tr.actions_shortcut_key("T"),
            },
            // 同步（`toolbar.py:410-419`）：点了发 `sync` 给宿主，宿主跑完用
            // `window.__ankiSyncStatus()` 把状态写回工具栏
            {
                "id": "sync",
                "cmd": "sync",
                "label": tr.translate("qt-misc-sync", None),
                "tip": tr.actions_shortcut_key("Y"),
            },
        ]
    })
}

/// 顶部工具栏的渲染脚本（放成文件，验证脚本读同一份，见 §5.27）。
const TOOLBAR_JS: &str = include_str!("../assets/reviewer-toolbar.js");

/// 注入顶部工具栏（对照 Qt 主窗口那条常驻工具栏；`legacy.rs` 的复习页留了容器）。
fn install_toolbar(state: &AppState) {
    let payload = toolbar_payload(state.backend.i18n());
    eval(
        state,
        &format!("{TOOLBAR_JS}\ninstallToolbar({payload});"),
    );
    tracing::debug!("顶部工具栏已注入");
}

fn sync_bottom_bar_height(state: &AppState) {
    eval(
        state,
        "document.documentElement.style.setProperty('--bottom-bar-height', \
         document.getElementById('bottom-container').offsetHeight + 'px');",
    );
}

pub(crate) async fn render_card(
    state: &AppState,
    card_id: i64,
) -> anyhow::Result<RenderCardResponse> {
    let request = RenderExistingCardRequest {
        card_id,
        browser: false,
        partial_render: false,
    };
    call(state, "renderExistingCard", &request).await
}

/// 显示问题/答案时自动播放卡片音频。
///
/// 对照 Qt 的 `Reviewer._showQuestion` / `_showAnswer`：
/// `if c.autoplay(): av_player.play_tags(c.question_av_tags())`。
/// 是否播放由牌组配置的 `disable_autoplay` 决定（默认播放）。
async fn autoplay_audio(state: &AppState, question_side: bool, html: &str) {
    let tags = match extract_av_tags(state, html, question_side).await {
        Ok(tags) => tags,
        Err(error) => {
            tracing::warn!("提取音频标签失败: {error:#}");
            return;
        }
    };
    if tags.is_empty() {
        return;
    }

    // 对照 `Card.autoplay()`：看牌组配置的 `disable_autoplay`，默认播放
    let autoplay = state
        .reviewer
        .session()
        .deck_settings
        .as_ref()
        .map(|settings| !settings.disable_autoplay)
        .unwrap_or(true);
    if !autoplay {
        tracing::debug!("牌组关闭了自动播放，跳过 {} 个音频标签", tags.len());
        return;
    }

    play_av_tags(state, tags).await;
}

/// 把 av 标签变成实际音频文件并排队播放。
///
/// reviewer（自动播放 / 重播）与预览窗口共用；对照 `AVPlayer.play_tags`。
pub(crate) async fn play_av_tags(state: &AppState, tags: Vec<AvTag>) {
    let mut files = Vec::new();
    for tag in tags {
        match tag.value {
            Some(av_tag::Value::SoundOrVideo(name)) => match audio::media_path(state, &name) {
                Ok(path) => files.push(path),
                Err(error) => tracing::warn!("{error}"),
            },
            // TTS：合成成 WAV 再入队，对照 Qt 的 `WindowsRTTTSFilePlayer`
            Some(av_tag::Value::Tts(tts)) => match audio::tts_file(state, &tts).await {
                Ok(path) => files.push(path),
                Err(error) => tracing::warn!("TTS 合成失败: {error:#}"),
            },
            None => {}
        }
    }

    if files.is_empty() {
        return;
    }
    tracing::debug!("播放 {} 个音频文件", files.len());
    if let Err(error) = audio::play_files(files) {
        tracing::warn!("播放失败: {error}");
    }
}

/// 对照 pylib 的 `Card.question_av_tags()` / `answer_av_tags()`：
/// 从渲染结果里提取 `[sound:...]` 与 `{{tts ...}}` 标签。
pub(crate) async fn extract_av_tags(
    state: &AppState,
    text: &str,
    question_side: bool,
) -> anyhow::Result<Vec<AvTag>> {
    let request = ExtractAvTagsRequest {
        text: text.to_string(),
        question_side,
    };
    let response: ExtractAvTagsResponse = call(state, "extractAvTags", &request).await?;
    Ok(response.av_tags)
}

/// 读当前卡片所属牌组的配置。
///
/// 对照 `Collection.decks.config_dict_for_deck_id()`：计时器、自动播放、
/// `stopTimerOnAnswer` 等都取自这里。筛选牌组没有自己的配置，返回 `None`，
/// 调用方按默认值处理。
pub(crate) async fn read_deck_settings(
    state: &AppState,
    deck_id: i64,
) -> anyhow::Result<Option<DeckSettings>> {
    if deck_id == 0 {
        return Ok(None);
    }
    let deck: Deck = call(state, "getDeck", &DeckId { did: deck_id }).await?;
    let Some(deck::Kind::Normal(normal)) = deck.kind else {
        tracing::debug!("牌组 {deck_id} 不是普通牌组，按默认配置处理");
        return Ok(None);
    };
    let config: DeckConfig = call(
        state,
        "getDeckConfig",
        &DeckConfigId {
            dcid: normal.config_id,
        },
    )
    .await?;
    Ok(config.config)
}

/// 对照 `Reviewer._showQuestion` 传给前端的 `maxTime`（单位秒）：
/// 牌组开启计时器时是 `card.time_limit() / 1000`，否则传 0（前端不显示计时）。
fn max_time_secs(state: &AppState) -> u32 {
    let session = state.reviewer.session();
    let Some(settings) = session.deck_settings.as_ref() else {
        return 0;
    };
    if settings.show_timer {
        settings.cap_answer_time_to_secs
    } else {
        0
    }
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

/// 对照 `Reviewer._showAnswerButton()`：按钮上带剩余队列计数（`_remaining()`）。
fn show_answer_button(tr: &I18n, remaining: &str) -> String {
    let label = tr.studying_show_answer();
    let key = tr.actions_shortcut_key(tr.studying_space());
    format!(
        r#"<table cellpadding="0"><tr><td class="stat2" align="center"><button title="{key}" id="ansbut" onclick='pycmd("ans");'>{label}<span class="stattxt">{remaining}</span></button></td></tr></table>"#
    )
}

/// 对照 `Reviewer._remaining()`：底部栏的「新 + 学习中 + 复习」剩余计数。
///
/// 当前卡片所在队列的那一项加下划线（Qt 用 `<u>` 标记）。
/// 关闭（`dueCounts` 为 false）或读不到计数时返回空字符串。
fn remaining_html(state: &AppState) -> String {
    let session = state.reviewer.session();
    if !session.ui.show_remaining_counts {
        return String::new();
    }
    let counts = session.counts;
    let current = session.queue_index;
    let cell = |index: usize, class: &str| {
        let value = counts[index];
        if index == current {
            format!(r#"<span class="{class}"><u>{value}</u></span>"#)
        } else {
            format!(r#"<span class="{class}">{value}</span>"#)
        }
    };
    format!(
        "{} + {} + {}",
        cell(0, "new-count"),
        cell(1, "learn-count"),
        cell(2, "review-count")
    )
}

/// 对照 `Reviewer._answerButtons()`。
///
/// 与 Qt 的差异：
///
/// - Qt 按 `ProfileManager.get_answer_key(i)` 取快捷键作为 title（默认就是 1..4），
///   这里同样用 1..4；
/// - Qt 会给默认按钮加 `id="defease"`，仓库里没有任何 CSS/JS 依赖它，故不加。
///
/// `show_intervals` 对应 `col.conf["estTimes"]`（`_buttonTime()` 的开关）。
fn ease_buttons(tr: &I18n, next_intervals: &[String], show_intervals: bool) -> String {
    let mut out = String::from(r#"<table cellpadding="0"><tr>"#);
    for (index, (ease, label)) in [
        (1, tr.studying_again()),
        (2, tr.studying_hard()),
        (3, tr.studying_good()),
        (4, tr.studying_easy()),
    ]
    .into_iter()
    .enumerate()
    {
        // 对照 `_buttonTime()`：`v3_labels[i - 1]`
        let due = match next_intervals.get(index) {
            Some(interval) if show_intervals && !interval.is_empty() => {
                format!(r#"<span class="nobold">{interval}</span>"#)
            }
            _ => String::new(),
        };
        let ease_num = ease.to_string();
        let key = tr.actions_shortcut_key(&ease_num);
        out.push_str(&format!(
            r#"<td align="center"><button title="{key}" data-ease="{ease}" onclick='pycmd("ease{ease}");'>{label}{due}</button></td>"#
        ));
    }
    out.push_str("</tr></table>");
    out
}

/// 读复习界面的显示开关（对照 `col.conf` 的 `dueCounts` / `estTimes`）。
async fn read_reviewer_ui(state: &AppState) -> anyhow::Result<ReviewerUi> {
    let prefs: Preferences = call(state, "getPreferences", &Empty {}).await?;
    let reviewing = prefs.reviewing.unwrap_or_default();
    Ok(ReviewerUi {
        show_remaining_counts: reviewing.show_remaining_due_counts,
        show_intervals: reviewing.show_intervals_on_buttons,
    })
}

/// 读 collection 配置里的 `cardStateCustomizer`（用户自定义调度脚本）。
///
/// 对照 `Reviewer.show()` 的 `self.mw.col.get_config("cardStateCustomizer")`。
async fn read_state_mutation_js(state: &AppState) -> anyhow::Result<Option<String>> {
    let request = GenericString {
        val: "cardStateCustomizer".to_string(),
    };
    let response: GenericJson = call(state, "getConfigJson", &request).await?;
    let value: serde_json::Value = serde_json::from_slice(&response.json)?;
    Ok(match value {
        serde_json::Value::String(js) if !js.trim().is_empty() => Some(js),
        _ => None,
    })
}

/// 对照 `Reviewer._run_state_mutation_hook()`。
///
/// 注入的脚本与 Qt 的 `RUN_STATE_MUTATION` 等价：调用前端的
/// `anki.mutateNextCardStates(key, …)`，完成后通过
/// `bridgeCommand('statesMutated')` 通知宿主，宿主据此放行评分按钮。
fn run_state_mutation(state: &AppState) {
    let script = {
        let session = state.reviewer.session();
        let (Some(js), Some(key)) = (
            session.state_mutation_js.as_deref(),
            session.state_mutation_key.as_deref(),
        ) else {
            return;
        };
        // 与 Qt 的 `RUN_STATE_MUTATION` 等价，额外把失败原因回传给宿主：
        // 前端的 mutation 在 WebView 里跑，出错时如果只吞掉，宿主只能看到
        // 「已完成」却没有任何状态被提交。
        format!(
            "anki.mutateNextCardStates({}, async (states, customData, ctx) => {{\n{}\n}})\
             .catch((error) => bridgeCommand('stateMutationError:' + ((error && error.stack) || String(error))))\
             .finally(() => bridgeCommand('statesMutated'));",
            js_string(key),
            js
        )
    };
    tracing::debug!("已注入自定义调度脚本（key 已设置）");
    eval(state, &script);
}

/// 等自定义调度完成（对照 Qt 在 `_showEaseButtons` 里的 50ms 重试）。
///
/// 超时后仍然继续：宁可用当前状态显示按钮，也不要让复习卡住。
async fn wait_for_states_mutation(state: &AppState) {
    for _ in 0..20 {
        if state.reviewer.session().states_mutated {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tracing::warn!("等待自定义调度超时，按当前状态继续");
}

/// 前端完成自定义调度后（`bridgeCommand('statesMutated')`）调用。
pub fn mark_states_mutated(state: &AppState) {
    tracing::debug!("自定义调度已完成");
    state.reviewer.session().states_mutated = true;
}

/// 对照 `mediasrv.get_scheduling_states_with_context()`。
pub fn states_with_context(state: &AppState) -> Option<(SchedulingStates, SchedulingContext)> {
    let session = state.reviewer.session();
    Some((session.states.clone()?, session.context.clone()?))
}

/// 对照 `Reviewer.set_scheduling_states()`。
///
/// key 不匹配时忽略：前端可能提交一个已经过期的变更，用它覆盖当前状态会让卡片
/// 按错误的调度信息作答。
pub fn apply_states_mutation(state: &AppState, key: &str, states: SchedulingStates) -> bool {
    let mut session = state.reviewer.session();
    if session.state_mutation_key.as_deref() != Some(key) {
        tracing::warn!("忽略过期的调度状态变更（key 不匹配）");
        return false;
    }
    session.states = Some(states);
    tracing::debug!("已接受前端提交的调度状态");
    true
}

/// `[[type:Field]]` 标记（对照 `Reviewer.typeAnsPat`）。
static TYPE_ANSWER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[type:(.+?)\]\]").expect("type 标记正则无效"));

/// 对照 `Reviewer.typeAnsQuestionFilter()`：把 `[[type:Field]]` 换成输入框。
///
/// 支持 Qt 的三个修饰符：`cloze`（填空题取指定空）、`nc`（不忽略组合字符）、
/// `ci`（忽略大小写）。返回替换后的 HTML 与本次作答的运行时状态。
async fn apply_type_answer_question(
    state: &AppState,
    card: &Card,
    html: &str,
) -> (String, Option<TypeAnswer>) {
    let Some(captures) = TYPE_ANSWER_PATTERN.captures(html) else {
        return (html.to_string(), None);
    };
    let raw = captures
        .get(1)
        .map(|found| found.as_str())
        .unwrap_or_default();
    // 字段名不含冒号，因此最后一个冒号之前都是修饰符
    let (modifiers, field_name) = match raw.rfind(':') {
        Some(index) => (&raw[..index], &raw[index + 1..]),
        None => ("", raw),
    };
    let mut cloze = false;
    let mut combining = true;
    let mut ignore_case = false;
    for modifier in modifiers.split(':').filter(|part| !part.is_empty()) {
        match modifier {
            "cloze" => cloze = true,
            "nc" => combining = false,
            "ci" => ignore_case = true,
            other => tracing::debug!("忽略未知的 type 修饰符: {other}"),
        }
    }

    let field = match read_type_answer_field(state, card, field_name, cloze).await {
        Ok(field) => field,
        Err(error) => {
            tracing::warn!("读取类型作答字段失败: {error:#}");
            None
        }
    };
    let Some((correct, font, size)) = field else {
        // 字段不存在或为空：对照 Qt，去掉标记即可
        tracing::debug!("字段 {field_name} 不存在或为空，移除 type 标记");
        return (TYPE_ANSWER_PATTERN.replace_all(html, "").into_owned(), None);
    };

    let input = format!(
        "<center><input type=text id=typeans onkeypress=\"_typeAnsPress();\" \
         style=\"font-family: '{font}'; font-size: {size}px;\"></center>"
    );
    let replaced = TYPE_ANSWER_PATTERN
        .replace(html, input.as_str())
        .into_owned();
    tracing::debug!("类型作答: 字段={field_name} 已插入输入框");
    (
        replaced,
        Some(TypeAnswer {
            correct,
            combining,
            ignore_case,
            font,
            size,
        }),
    )
}

/// 读 `[[type:Field]]` 指向的答案与字段样式（对照 Qt 遍历 `note_type()["flds"]`）。
///
/// 字段的字体与字号只存在于 legacy JSON 里（proto 的 `Notetype.Field` 不含样式），
/// 所以用 `getNotetypeLegacy` 一次取回字段名与样式。
async fn read_type_answer_field(
    state: &AppState,
    card: &Card,
    field_name: &str,
    cloze: bool,
) -> anyhow::Result<Option<(String, String, u32)>> {
    let note: Note = call(state, "getNote", &NoteId { nid: card.note_id }).await?;
    let legacy: GenericJson = call(
        state,
        "getNotetypeLegacy",
        &NotetypeId {
            ntid: note.notetype_id,
        },
    )
    .await?;
    let notetype: serde_json::Value = serde_json::from_slice(&legacy.json)?;
    let fields = notetype
        .get("flds")
        .and_then(|flds| flds.as_array())
        .ok_or_else(|| anyhow::anyhow!("notetype JSON 里没有 flds"))?;
    let Some(index) = fields
        .iter()
        .position(|field| field.get("name").and_then(|name| name.as_str()) == Some(field_name))
    else {
        return Ok(None);
    };

    let mut correct = note.fields.get(index).cloned().unwrap_or_default();
    if cloze {
        // 对照 Qt 的 `_contentForCloze`：`card.ord + 1` 就是 cloze 序号
        let request = ExtractClozeForTypingRequest {
            text: correct.clone(),
            ordinal: card.template_idx + 1,
        };
        let extracted: GenericString = call(state, "extractClozeForTyping", &request).await?;
        correct = extracted.val;
    }
    if correct.is_empty() {
        return Ok(None);
    }

    let style = &fields[index];
    let font = style
        .get("font")
        .and_then(|font| font.as_str())
        .unwrap_or("Arial")
        .to_string();
    let size = style.get("size").and_then(|size| size.as_u64()).unwrap_or(20) as u32;
    Ok(Some((correct, font, size)))
}

/// 对照 `Reviewer.typeAnsAnswerFilter()`：把 `[[type:Field]]` 换成答案对比结果。
async fn apply_type_answer_answer(
    state: &AppState,
    html: &str,
    typed: Option<&str>,
    type_answer: Option<&TypeAnswer>,
) -> String {
    let Some(type_answer) = type_answer else {
        // 本张卡没有类型作答（或字段为空）：去掉残留标记
        return TYPE_ANSWER_PATTERN.replace_all(html, "").into_owned();
    };

    // 对照 Qt：先把分隔线拿出来，稍后放回，让对比结果出现在分隔线之后
    let without_hr = html.replace("<hr id=answer>", "");
    let had_hr = without_hr.len() != html.len();

    let request = CompareAnswerRequest {
        expected: type_answer.correct.clone(),
        provided: typed.unwrap_or_default().to_string(),
        combining: type_answer.combining,
        ignore_case: type_answer.ignore_case,
    };
    let comparison: GenericString = match call(state, "compareAnswer", &request).await {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!("生成答案对比失败: {error:#}");
            return TYPE_ANSWER_PATTERN.replace_all(html, "").into_owned();
        }
    };

    let mut replacement = format!(
        "<div style=\"font-family: '{}'; font-size: {}px\">{}</div>",
        type_answer.font, type_answer.size, comparison.val
    );
    if had_hr {
        replacement = format!("<hr id=answer>{replacement}");
    }

    // 对照 Qt：若移走分隔线后已没有 type 标记（`{{FrontSide}}` 的情形），
    // 保持原文，避免分隔线被挪到错误位置
    if had_hr && !TYPE_ANSWER_PATTERN.is_match(&without_hr) {
        return html.to_string();
    }
    TYPE_ANSWER_PATTERN
        .replace_all(&without_hr, replacement.as_str())
        .into_owned()
}

/// 对照 `Scheduler.describe_next_states()`：四档评分的下次间隔标签。
async fn describe_next_states(
    state: &AppState,
    states: &SchedulingStates,
) -> anyhow::Result<Vec<String>> {
    let labels: StringList = call(state, "describeNextStates", states).await?;
    Ok(labels.vals)
}

/// 对照 `TemplateRenderOutput.question_and_style()`：样式与内容一起注入。
pub(crate) fn with_css(css: &str, html: &str) -> String {
    format!("<style>{css}</style>{html}")
}

/// 把结构化渲染结果拼成 HTML。
///
/// 对照 `pylib/anki/template.py` 的 `PartiallyRenderedCard.nodes_from_proto`。
pub(crate) fn nodes_to_html(nodes: &[RenderedTemplateNode]) -> String {
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
pub(crate) fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

/// 复习界面的注入目标是主窗口。
fn eval(state: &AppState, js: &str) {
    crate::windows::eval(state, crate::windows::MAIN, js);
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}
