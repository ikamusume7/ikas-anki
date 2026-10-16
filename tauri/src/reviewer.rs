// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 最小复习会话。
//!
//! 对照 Qt 的 `aqt/reviewer.py`：**取队列 → 渲染 → 注入 WebView → 处理评分**。
//! 与 Qt 保持同一个约定：卡片 HTML 交给 `js/reviewer.js` 提供的
//! `_showQuestion(q, a, bodyclass)` / `_showAnswer(a, bodyclass)` 全局函数。
//!
//! 当前实现的是主链路（显示问题 → 显示答案 → 四档评分 → 下一张），以及计时器与
//! **时间盒**、撤销、自动播放与音频队列、类型作答（`type:`）、队列计数显示、
//! `getSchedulingStatesWithContext` 界面状态同步、顶部/底部工具栏、「更多」菜单、
//! 键盘答题（`assets/reviewer-keys.js`），以及队列空时的收尾（清会话 + 回概览画
//! Congratulations）。
//!
//! **未实现**（见 `tauri/PARITY.md`）：
//! - **音频**：卡面不画播放按钮（没有 `play:q:N`）、没有重播 / 暂停 / ±5 秒 / 录音，
//!   答案面还会把问题面音频再播一遍（`partial_render` 的差异）。因此 `_shortcutKeys()`
//!   里的 `r`/`F5`（重播）、`5`/`6`/`7`（暂停、±5 秒）、`v`/`Shift+v`（录音与重播录音）
//!   与 `Shift+A`（自动前进）都没有绑定。

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
use tauri::Manager as _;
use anki_proto::scheduler::SchedulingStates;
use anki_proto::config::Preferences;
use anki_proto::generic::Empty;
use anki_proto::generic::Json as GenericJson;
use anki_proto::generic::String as GenericString;
use anki_proto::scheduler::queued_cards::Queue;
use anki_proto::scheduler::SchedulingContext;
use serde_json::json;

use crate::platform::audio;
use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
use crate::state::AppState;

#[derive(Default)]
pub struct Reviewer {
    inner: Mutex<Session>,
    /// 时间盒：进入概览 / 开始复习时重新开始（对照 `Collection.startTimebox()`）
    timebox: Mutex<Option<Timebox>>,
}

/// 时间盒的运行时状态（对照 `collection.py:1239-1251` 的 `_startTime` / `_startReps`）。
///
/// Qt 把它挂在 Collection 上；本外壳挂在 `Reviewer` 上（同一个进程里只有一份复习会话）。
/// `cards` 就是 `sched.reps - _startReps`：本外壳自己数这段时间里答过几张卡。
#[derive(Clone, Copy)]
struct Timebox {
    started_at: std::time::Instant,
    cards: u32,
}

#[derive(Default)]
struct Session {
    card_id: Option<i64>,
    /// 上一张卡片（对照 Qt 的 `self.previous_card`）：菜单里的「上一张卡片信息」用它
    previous_card_id: Option<i64>,
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
    /// 是否已经显示答案（对照 Qt 的 `self.state == "answer"`）。
    ///
    /// Qt 的 `_answerCard()` 有 `if self.state != "answer": return` 守卫，
    /// 所以「问题面按 1」不会评分。评分按钮由 `show_answer()` 画出，这里同步记录。
    answer_shown: bool,
    /// 问题面 / 答案面的音频标签（按渲染顺序），`play:q:N` 与「重播音频」用
    question_tags: Vec<AvTag>,
    answer_tags: Vec<AvTag>,
    /// 自动前进的运行时开关（对照 Qt 的 `self.auto_advance_enabled`）。
    ///
    /// Qt 每次构造 reviewer 都重置为 `False`，只能靠 `Shift+A`/菜单打开 —— 牌组配置里那
    /// 几个秒数与动作只是"参数"，不决定开关（`qt/aqt` 里没有读 `autoAdvance` 的地方）。
    auto_advance: bool,
    /// 计时器的"代"：每次重排或清理都 +1；旧任务睡醒发现代对不上就自己退出
    /// （Qt 用 `QTimer.deleteLater()` 做同一件事）。
    auto_advance_generation: u64,
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

/// 对照 `col.conf` 里控制复习界面显示的两项配置，外加时间盒的上限。
///
/// 手写 `Default`：Anki 的默认值是剩余计数与按钮间隔都开启（`dueCounts` / `estTimes`），
/// 读取失败时应当按默认显示，而不是静默隐藏；时间盒默认关闭（`timeLim` 为 0）。
#[derive(Clone, Copy)]
struct ReviewerUi {
    show_remaining_counts: bool,
    show_intervals: bool,
    /// 时间盒上限（秒）；0 = 关闭（对照 `col.conf["timeLim"]`）
    time_limit_secs: u32,
}

impl Default for ReviewerUi {
    fn default() -> Self {
        Self {
            show_remaining_counts: true,
            show_intervals: true,
            time_limit_secs: 0,
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

    /// 上一张卡片 id（对照 Qt 的 `reviewer.previous_card`）。
    ///
    /// 复习菜单的「上一张卡片信息」（`Ctrl+Alt+I`）用它开卡片信息窗口，即使当前队列已经
    /// 空了也能用（Qt 里那个对话框对象一直留着）。
    pub(crate) fn previous_card_id(&self) -> Option<i64> {
        self.session().previous_card_id
    }

    /// 自动前进是否开着（菜单勾选态与 `Shift+A` 都用它）。
    pub(crate) fn auto_advance_enabled(&self) -> bool {
        self.session().auto_advance
    }

    /// 切换自动前进（对照 `reviewer.py:1219-1231` 的 `toggle_auto_advance`）。
    ///
    /// 返回切换后的状态；调用方负责提示与重新排定时器。
    pub(crate) fn toggle_auto_advance(&self) -> bool {
        let mut session = self.session();
        session.auto_advance = !session.auto_advance;
        // 换了一代：旧的计时任务作废（`auto_advance_if_enabled()` 会按当前面重排）
        session.auto_advance_generation += 1;
        session.auto_advance
    }

    /// 焦点丢失时自动前进要自己关掉（对照 `reviewer.py:428-434`）。
    pub(crate) fn disable_auto_advance(&self) {
        let mut session = self.session();
        if session.auto_advance {
            tracing::info!("窗口失去焦点，自动前进已关闭（对照 Qt 的同一行为）");
            session.auto_advance = false;
            session.auto_advance_generation += 1;
        }
    }

    /// 起一代新的计时器，返回它的代号（0 = 当前不需要计时）。
    fn arm_auto_advance(&self, seconds: f64) -> u64 {
        let mut session = self.session();
        session.auto_advance_generation += 1;
        if !session.auto_advance || seconds <= 0.0 {
            return 0;
        }
        session.auto_advance_generation
    }

    /// 这一代还是最新的吗（旧的 sleep 任务醒来看这个）。
    fn auto_advance_still_current(&self, generation: u64) -> bool {
        let session = self.session();
        session.auto_advance && session.auto_advance_generation == generation
    }

    /// 重新开始计时（对照 `col.startTimebox()`）。
    ///
    /// 调用点与 Qt 一致：显示概览（`overview.py:98`）与开始复习（`main.py:1242`）。
    pub(crate) fn start_timebox(&self) {
        *self.timebox.lock().expect("时间盒已中毒") = Some(Timebox {
            started_at: std::time::Instant::now(),
            cards: 0,
        });
    }

    /// 本段计时里答过一张卡（`sched.reps` 的等价计数）。
    fn count_timebox_card(&self) {
        if let Ok(mut guard) = self.timebox.lock() {
            if let Some(timebox) = guard.as_mut() {
                timebox.cards += 1;
            }
        }
    }

    /// 时间盒的 (已用时间, 已答卡片数)；没有在计时时为 `None`。
    ///
    /// 返回 `Duration` 而不是整秒：Qt 用浮点数比 `time.time() - _startTime > timeLim`
    /// （`collection.py:1248-1249`），边界上"1.5 秒 > 1 秒"要算到点。
    fn timebox_progress(&self) -> Option<(std::time::Duration, u32)> {
        self.timebox
            .lock()
            .ok()?
            .map(|timebox| (timebox.started_at.elapsed(), timebox.cards))
    }
}

/// 开始（或继续）复习：取下一张卡片并显示问题。
pub async fn start(state: &AppState) {
    // 对照 `Reviewer.nextCard()` 开头两行（`reviewer.py:249-250`）：
    // `previous_card = self.card; self.card = None` —— **在取下一张之前**就把当前这张
    // 记成"上一张"，所以答掉最后一张、队列空了之后「上一张卡片信息」照样能看。
    {
        let mut session = state.reviewer.session();
        if let Some(previous) = session.card_id {
            session.previous_card_id = Some(previous);
        }
    }

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
        // 对照 Qt 的 `Reviewer.nextCard()`（`reviewer.py:257-259`）：没有卡片就
        // `moveToState("overview")`，而概览在没有可学卡片时会渲染 Congratulations
        // （`overview.py:190-209` 的 `_show_finished_screen`，本外壳画在概览面板里）。
        tracing::info!("队列已空：清理复习会话并回到概览（恭喜页）");
        clear_session(state);
        if let Err(error) = crate::main_window::show_overview(state) {
            tracing::warn!("回到概览失败: {error:#}");
        }
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
        // 新卡片：还没显示答案（对照 Qt 在 `_showQuestion()` 里把 state 置回 "question"）
        session.answer_shown = false;
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

/// 队列空了：把会话清干净（对照 Qt 里 `self.card = None`）。
///
/// 不清的话底栏还留着上一张卡的评分按钮，再按 `1`–`4` 会拿过期状态去评分
/// （后端以 `card was modified` 报错，用户看到的是一个莫名其妙的失败）。
fn clear_session(state: &AppState) {
    let mut session = state.reviewer.session();
    session.card_id = None;
    session.card = None;
    session.type_answer = None;
    session.deck_settings = None;
    session.states = None;
    session.context = None;
    session.counts = [0, 0, 0];
    session.queue_index = 0;
    session.answer_shown = false;
    session.shown_at = None;
}

/// 时间盒到点了吗？到点就弹「继续 / 结束」（对照 `reviewer.py:973-992` 的
/// `check_timebox()`）。
///
/// 返回 `true` 表示**结束**：调用方不要再显示下一张卡（Qt 此时
/// `moveToState("deckBrowser")`，本外壳回牌组列表）。
/// 「继续」则重新计时（对照 `reviewer.py:991` 的 `col.startTimebox()`）。
async fn check_timebox(state: &AppState) -> bool {
    // 0 = 关闭（偏好设置的「复习」分页 →「时间盒」秒数，对照 `col.conf["timeLim"]`）
    let limit = state.reviewer.session().ui.time_limit_secs;
    if limit == 0 {
        tracing::debug!("时间盒未开启（timeLim=0）");
        return false;
    }
    let Some((elapsed, cards)) = state.reviewer.timebox_progress() else {
        return false;
    };
    // 与 Qt 一样用"实际已用时间 > 上限"比较（不是四舍五入后的秒数）
    if elapsed <= std::time::Duration::from_secs(u64::from(limit)) {
        tracing::debug!(
            "时间盒: 已用 {:.1} 秒 / 上限 {limit} 秒 / 已答 {cards} 张",
            elapsed.as_secs_f64()
        );
        return false;
    }

    // 对照 `reviewer.py:979`：用的是**上限**而不是实际用时
    let minutes = ((limit as f64) / 60.0).round() as u32;
    tracing::info!("时间盒到点: 已答 {cards} 张 / 约 {minutes} 分钟（上限 {limit} 秒）");
    let (message, continue_label, finish_label) = {
        let tr = state.backend.i18n();
        (
            // `minutes` 要的是 `impl Into<String>`（ftl 里是变量替换，不做数字格式化）
            tr.studying_card_studied_in_minute(cards, minutes.to_string())
                .to_string(),
            tr.studying_continue().to_string(),
            tr.studying_finish().to_string(),
        )
    };
    let message_for_log = message.clone();
    let proceed = tauri::async_runtime::spawn_blocking(move || {
        crate::platform::dialogs::ask_timebox(&message, &continue_label, &finish_label)
    })
    .await
    .unwrap_or(false);
    tracing::debug!("时间盒对话框: {message_for_log}");

    if proceed {
        tracing::info!("时间盒到点：继续（重新计时）");
        state.reviewer.start_timebox();
        return false;
    }

    tracing::info!("时间盒到点：结束（回牌组列表）");
    clear_session(state);
    if let Err(error) = crate::main_window::show_decks(state) {
        tracing::warn!("时间盒结束后回牌组列表失败: {error:#}");
    }
    true
}

/// 切换自动前进（`Shift+A` 与菜单里的「自动前进」，对照 `reviewer.py:1219-1231`）。
pub(crate) async fn toggle_auto_advance(state: &AppState) {
    let enabled = state.reviewer.toggle_auto_advance();
    let message = if enabled {
        state.backend.i18n().actions_auto_advance_activated()
    } else {
        state.backend.i18n().actions_auto_advance_deactivated()
    };
    tracing::info!("自动前进: {enabled}");
    toast(state, &message);
    // 对照 `auto_advance_if_enabled()`：按当前是问题面还是答案面重新排定
    if state.reviewer.session().answer_shown {
        auto_advance_to_question_if_enabled(state);
    } else {
        auto_advance_to_answer_if_enabled(state);
    }
}

/// 显示问题面之后的自动前进排定（对照 `reviewer.py:408-420` 的
/// `_auto_advance_to_answer_if_enabled`）。
///
/// 到点走 `_on_show_answer_timeout`：`questionAction` 是「显示答案」就直接显示，
/// 否则提示一句（Qt 是 tooltip，本外壳用一个小小的浮层）。
pub(crate) fn auto_advance_to_answer_if_enabled(state: &AppState) {
    let (enabled, seconds) = {
        let session = state.reviewer.session();
        let seconds = session
            .deck_settings
            .as_ref()
            .map(|settings| settings.seconds_to_show_question)
            .unwrap_or(0.0);
        (session.auto_advance, seconds)
    };
    if !enabled {
        // 没开也要换代：旧的计时任务作废（Qt 每次都 `_clear_auto_advance_timers()`）
        state.reviewer.arm_auto_advance(0.0);
        return;
    }
    let generation = state.reviewer.arm_auto_advance(seconds as f64);
    if generation == 0 {
        return;
    }
    tracing::debug!("自动前进: {seconds} 秒后显示答案");
    let state = state.clone();
    // `Box::pin` 是必须的：`show_question → 排定 → (到点) show_answer → 排定 → answer → start
    // → show_question` 会绕回自己，不装箱的话编译器算不出这个 future 的类型
    // （E0391 cycle detected）
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_secs_f64(seconds as f64)).await;
            if !state.reviewer.auto_advance_still_current(generation) {
                return;
            }
            on_show_answer_timeout(&state).await;
        });
    tauri::async_runtime::spawn(task);
}

/// 显示答案面之后的自动前进排定（对照 `reviewer.py:489-497`）。
pub(crate) fn auto_advance_to_question_if_enabled(state: &AppState) {
    let (enabled, seconds) = {
        let session = state.reviewer.session();
        let seconds = session
            .deck_settings
            .as_ref()
            .map(|settings| settings.seconds_to_show_answer)
            .unwrap_or(0.0);
        (session.auto_advance, seconds)
    };
    if !enabled {
        state.reviewer.arm_auto_advance(0.0);
        return;
    }
    let generation = state.reviewer.arm_auto_advance(seconds as f64);
    if generation == 0 {
        return;
    }
    tracing::debug!("自动前进: {seconds} 秒后动作（answerAction）");
    let state = state.clone();
    let task: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_secs_f64(seconds as f64)).await;
            if !state.reviewer.auto_advance_still_current(generation) {
                return;
            }
            on_show_question_timeout(&state).await;
        });
    tauri::async_runtime::spawn(task);
}

/// 问题面超时（对照 `reviewer.py:422-443` 的 `_on_show_answer_timeout`）。
async fn on_show_answer_timeout(state: &AppState) {
    if state.reviewer.session().card_id.is_none() {
        return;
    }
    let settings = state.reviewer.session().deck_settings.clone();
    let Some(settings) = settings else {
        return;
    };
    // `waitForAudio`：有音频在播就先不动（Qt 是 `av_player.current_player`）
    if settings.wait_for_audio && crate::platform::audio::is_playing() {
        tracing::debug!("自动前进: 还有音频在播，等下一轮");
        return;
    }
    if !main_window_focused(state) {
        state.reviewer.disable_auto_advance();
        return;
    }
    match settings.question_action() {
        anki_proto::deck_config::deck_config::config::QuestionAction::ShowReminder => {
            tracing::info!("自动前进: 到点提示（问题面）");
            toast(state, &state.backend.i18n().studying_question_time_elapsed());
        }
        // 默认（含非法值）：显示答案
        _ => {
            tracing::info!("自动前进: 到点显示答案");
            show_answer(state, None).await;
        }
    }
}

/// 答案面超时（对照 `reviewer.py:501-527` 的 `_on_show_question_timeout`）。
async fn on_show_question_timeout(state: &AppState) {
    if state.reviewer.session().card_id.is_none() {
        return;
    }
    let settings = state.reviewer.session().deck_settings.clone();
    let Some(settings) = settings else {
        return;
    };
    if settings.wait_for_audio && crate::platform::audio::is_playing() {
        tracing::debug!("自动前进: 还有音频在播，等下一轮");
        return;
    }
    if !main_window_focused(state) {
        state.reviewer.disable_auto_advance();
        return;
    }
    match settings.answer_action() {
        anki_proto::deck_config::deck_config::config::AnswerAction::AnswerAgain => {
            tracing::info!("自动前进: 到点评「重来」");
            answer(state, 1).await;
        }
        anki_proto::deck_config::deck_config::config::AnswerAction::AnswerHard => {
            tracing::info!("自动前进: 到点评「困难」");
            answer(state, 2).await;
        }
        anki_proto::deck_config::deck_config::config::AnswerAction::AnswerGood => {
            tracing::info!("自动前进: 到点评「良好」");
            answer(state, 3).await;
        }
        anki_proto::deck_config::deck_config::config::AnswerAction::ShowReminder => {
            tracing::info!("自动前进: 到点提示（答案面）");
            toast(state, &state.backend.i18n().studying_answer_time_elapsed());
        }
        // 默认（含非法值）：埋藏这张卡（对照 Qt 的 `else: self.bury_current_card()`）
        _ => {
            tracing::info!("自动前进: 到点埋藏卡片");
            crate::reviewer_menu::handle(state, "rev:buryCard").await;
        }
    }
}

/// 主窗口是否在前台（对照 `reviewer.py:428-434` 的 `focusWidget().window() != self.mw`）。
fn main_window_focused(state: &AppState) -> bool {
    // 测试钩子：自动化里终端才是前台窗口（Windows 不允许后台进程随便抢前台），
    // 这个钩子让"焦点判定"直接通过 —— 与 `ANKI_CONFIRM_YES` 同一套路。
    // 真实行为（窗口没有焦点就关掉自动前进）在日志里能看到：「窗口失去焦点，自动前进已关闭」
    if std::env::var("ANKI_AUTO_ADVANCE_IGNORE_FOCUS")
        .is_ok_and(|value| !value.is_empty() && value != "0")
    {
        return true;
    }
    state
        .app
        .get_webview_window(crate::windows::MAIN)
        .and_then(|window| window.is_focused().ok())
        .unwrap_or(false)
}

/// 复习页里的小浮层提示（Qt 的 `tooltip()`；本外壳没有原生 tooltip，用页面里的一个 div）。
fn toast(state: &AppState, message: &str) {
    eval(
        state,
        &format!(
            r#"(function () {{
    var el = document.getElementById("anki-toast");
    if (!el) {{
        el = document.createElement("div");
        el.id = "anki-toast";
        el.style.cssText = "position:fixed;left:50%;transform:translateX(-50%);bottom:calc(var(--bottom-bar-height, 4rem) + 1rem);background:rgba(0,0,0,0.8);color:#fff;padding:0.35rem 0.75rem;border-radius:0.3rem;font-size:0.9rem;z-index:99;opacity:0;transition:opacity 0.3s;pointer-events:none;";
        document.body.appendChild(el);
    }}
    el.textContent = {};
    el.style.opacity = "1";
    clearTimeout(window.__ankiToastTimer);
    window.__ankiToastTimer = setTimeout(function () {{ el.style.opacity = "0"; }}, 2500);
}})();"#,
            js_string(message)
        ),
    );
}

/// 撤销上一次操作（对照 Qt 的 `mw.undo()` → `operations/collection.py:undo()`）。
///
/// 撤销会改变卡片的调度状态（通常是把上一张卡放回队列），所以完成后必须重新取队列并
/// 重新渲染 —— 但**要按当前界面来**：还在复习页就重取重画；已经被顶回概览/牌组列表
/// （例如「设置到期日」把最后一张卡移到未来 → 队列空 → 自动回概览，再撤销）就让那个
/// 页面重新拉数据（对照 Qt 的 `mw.undo()` → `col.undo()` → `mw.reset()` → 按当前
/// 状态刷新）。
pub async fn undo(state: &AppState) {
    match call::<_, OpChangesAfterUndo>(state, "undo", &Empty {}).await {
        Ok(_) => tracing::debug!("已撤销上一步操作"),
        Err(error) => {
            // 没有可撤销的操作时 rslib 也会返回错误，属于正常情况
            tracing::warn!("撤销失败: {error:#}");
            return;
        }
    }
    if state.reviewer_active.load(std::sync::atomic::Ordering::SeqCst) {
        start(state).await;
    } else {
        // 页面（牌组列表 / 概览）自己重新取数据；复习会话在回到概览时已经清干净
        tracing::debug!("撤销时不在复习页，让当前页面重新加载");
        eval(state, "window.location.reload();");
    }
}

/// 复习界面的快捷键脚本（对照 Qt 的 `Reviewer._shortcutKeys()`）。
///
/// Qt 用 QShortcut 注册，单页方案里改为在页面内监听键盘 —— 脚本本体放在
/// `assets/reviewer-keys.js`，用 `include_str!` 引用，这样验证脚本能加载**同一份文件**
/// （与 `assets/reviewer-menu.js` 相同的做法）。
const KEYS_JS: &str = include_str!("../assets/reviewer-keys.js");

/// 安装键盘操作。
///
/// 每次会话开始都会刷新 `window.__ankiReviewerConfig`（脚本本身只装一次），
/// 这样在设置里改「空格评分」后，下一张卡片就生效。
fn install_reviewer_shortcuts(state: &AppState) {
    let spacebar_rates_card = read_spacebar_rates_card(state);
    tracing::debug!("键盘答题: spacebarRatesCard={spacebar_rates_card}");
    eval(
        state,
        &format!(
            "window.__ankiReviewerConfig = {{ spacebarRatesCard: {spacebar_rates_card} }};\n{KEYS_JS}"
        ),
    );
}

/// 读 `meta["spacebar_rates_card"]`（对照 `ProfileManager.spacebar_rates_card()`）。
///
/// 缺键时按桌面版的默认值 `True` 处理（`pylib/anki/profiles.py` 的 `defaultProfile`）。
fn read_spacebar_rates_card(state: &AppState) -> bool {
    let Some(prefs) = Prefs21::locate(state.profile_dir.as_path()) else {
        return true;
    };
    prefs
        .get(Store::Meta, "spacebar_rates_card")
        .ok()
        .flatten()
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

/// 当前卡片对应的 body class（对照 `theme_manager.body_classes_for_card_ord(ord)`）。
///
/// 必须整串传给 `_showQuestion` / `_showAnswer`：页面里是
/// `document.body.className = bodyclass`（整串覆盖），少一个类就会把页面上打好的
/// 平台类/夜间类清掉（表现出来就是「首帧正常、出卡变浅色」）。
fn card_body_class(state: &AppState) -> String {
    let ord = state
        .reviewer
        .session()
        .card
        .as_ref()
        // 卡片模板序号（pylib 里叫 `ord`，rslib 的 proto 字段名是 `template_idx`）
        .map(|card| card.template_idx)
        .unwrap_or(0);
    crate::theme::style(state).body_classes_for_card_ord(ord)
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

    // 对照 Qt `_showAnswer`：自动播放答案面音频（并记住这面的标签）
    let answer_tags = autoplay_audio(state, false, &answer_html).await;
    state.reviewer.session().answer_tags = answer_tags;
    // 对照 `av_refs_to_play_icons()`：把 `[anki:play:a:N]` 标记换成播放按钮
    let answer_html = av_refs_to_play_icons(&answer_html, 'a');
    let play_buttons = answer_html.matches("replay-button").count();
    if play_buttons > 0 {
        tracing::info!("答案面注入了 {play_buttons} 个播放按钮");
    }

    eval(
        state,
        &format!(
            "_showAnswer({}, '', {})",
            js_string(&with_css(&rendered.css, &answer_html)),
            // 对照 `reviewer.py:394-399`：bodyclass 必须带全（`ts/reviewer/index.ts:209`
            // 是 `document.body.className = bodyclass`，整串覆盖）
            js_string(&card_body_class(state)),
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
    // 评分按钮画出来之后才允许评分（对照 Qt 的 `self.state = "answer"`）
    state.reviewer.session().answer_shown = true;
    eval(
        state,
        &format!("showAnswer({}, {});", js_string(&buttons), stop_timer),
    );
    // 换成评分按钮后底部栏宽度/换行可能变化
    sync_bottom_bar_height(state);
    // 对照 `reviewer.py:489`：显示答案之后按需排定「自动前进到下一张/评分」
    auto_advance_to_question_if_enabled(state);
}

/// 提交评分（`ease` 为 1..4），随后显示下一张卡片。
pub async fn answer(state: &AppState, ease: u32) {
    let (card_id, current, next) = {
        let mut session = state.reviewer.session();
        // 对照 `_answerCard()` 的 `if self.state != "answer": return`：
        // 问题面按 1..4 不评分；提交中的重复按键（双击评分按钮）也在这里挡掉。
        if !session.answer_shown {
            tracing::warn!("尚未显示答案，忽略评分请求（ease{ease}）");
            return;
        }
        session.answer_shown = false;
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

    // 对照 `reviewer.py:566-570` 的 `_after_answering()`：先记一次时间盒的卡片数，
    // 再看时间盒是否到点（到点且选「结束」时不再显示下一张）
    state.reviewer.count_timebox_card();
    if check_timebox(state).await {
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
    let question_tags = autoplay_audio(state, true, &question_html).await;
    state.reviewer.session().question_tags = question_tags;
    let question_html = av_refs_to_play_icons(&question_html, 'q');
    let play_buttons = question_html.matches("replay-button").count();
    if play_buttons > 0 {
        tracing::info!("问题面注入了 {play_buttons} 个播放按钮");
    }

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
            // 对照 `reviewer.py:394-399` 的 `body_classes_for_card_ord(c.ord)`
            js_string(&card_body_class(state)),
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
    // 对照 `reviewer.py:406`：显示问题之后按需排定「自动前进到答案」
    auto_advance_to_answer_if_enabled(state);
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

/// 显示问题/答案时自动播放卡片音频，并把这面的音频标签记进会话
/// （`play:q:N` / `replayq` 要用）。
///
/// 对照 Qt 的 `Reviewer._showQuestion` / `_showAnswer`：
/// `if c.autoplay(): av_player.play_tags(c.question_av_tags())`。
/// 是否播放由牌组配置的 `disable_autoplay` 决定（默认播放）。
///
/// **答案面不再把问题面的音频再播一遍**：答案 HTML 里带着 `{{FrontSide}}`（含问题面的
/// 音频标记），Qt 那边 `card.answer_av_tags()` 是按答案面算的，所以问题面的那份不会重复。
/// 这里用"前缀跳过"近似：答案面的标签如果以问题面的标签列表开头，就把这一段去掉。
async fn autoplay_audio(state: &AppState, question_side: bool, html: &str) -> Vec<AvTag> {
    let mut tags = match extract_av_tags(state, html, question_side).await {
        Ok(tags) => tags,
        Err(error) => {
            tracing::warn!("提取音频标签失败: {error:#}");
            return Vec::new();
        }
    };
    if !question_side {
        let question_tags = state.reviewer.session().question_tags.clone();
        if !question_tags.is_empty() && tags.len() >= question_tags.len() {
            if tags[..question_tags.len()] == question_tags[..] {
                tracing::debug!(
                    "答案面跳过 {} 个已在问题面播过的音频标签（对照 `answer_av_tags()`）",
                    question_tags.len()
                );
                tags.drain(..question_tags.len());
            }
        }
    }
    if tags.is_empty() {
        return tags;
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
        return tags;
    }

    play_av_tags(state, tags.clone()).await;
    tags
}

/// 卡片 HTML 里的音频：上游渲染器会写成 `[anki:play:q:N]` 标记（`rslib/.../writer.rs:137`），
/// 本外壳拿到的渲染结果里还是原始的 `[sound:...]` / `[anki:tts ...]`，所以三种形式都认
/// （索引按出现顺序递增，与 `extractAvTags` 返回的标签顺序一致）。
static AV_REF_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<marker>\[anki:play:[qa]:\d+\])|\[sound:[^\]]+\]|\[anki:tts[^\]]*\]")
        .expect("音频标记正则")
});

/// 把音频标记换成播放按钮（对照 `sound.py:900-921` 的 `av_refs_to_play_icons()`）。
///
/// 点击后发 `play:q:N` / `play:a:N` 给宿主（见 `server/pycmd.rs` 的 `play:` 分支），
/// 索引对应 `extractAvTags` 那份列表。图标复用页面里的 `.replay-button` / `.playImage`
/// （`css/reviewer.css`）。
fn av_refs_to_play_icons(html: &str, side: char) -> String {
    let mut index = 0usize;
    AV_REF_PATTERN
        .replace_all(html, |captures: &regex::Captures| {
            let command = match captures.name("marker") {
                Some(marker) => marker
                    .as_str()
                    .trim_start_matches("[anki:")
                    .trim_end_matches(']')
                    .to_owned(),
                None => {
                    let command = format!("play:{side}:{index}");
                    index += 1;
                    command
                }
            };
            format!(
                r##"<a class="replay-button soundLink" href="#" onclick="pycmd('{command}'); return false;" draggable="false">
    <svg class="playImage" viewBox="0 0 64 64" version="1.1">
        <circle cx="32" cy="32" r="29" />
        <path d="M56.502,32.301l-37.502,20.101l0.329,-40.804l37.173,20.703Z" />
    </svg>
</a>"##
            )
        })
        .into_owned()
}

/// 播放某一面的第 `index` 个音频标签（`play:q:0` / `play:a:0`）。
///
/// 对照 `qt/aqt/sound.py:924-937` 的 `play_clicked_audio`。
pub(crate) async fn play_clicked_audio(state: &AppState, question_side: bool, index: usize) {
    let tag = {
        let session = state.reviewer.session();
        let tags = if question_side {
            &session.question_tags
        } else {
            &session.answer_tags
        };
        tags.get(index).cloned()
    };
    match tag {
        Some(tag) => {
            tracing::info!(
                "播放点击的音频: {}第 {index} 个",
                if question_side { "问题面" } else { "答案面" }
            );
            play_av_tags(state, vec![tag]).await;
        }
        None => tracing::warn!(
            "没有第 {index} 个{}音频标签",
            if question_side { "问题面" } else { "答案面" }
        ),
    }
}

/// 暂停 / 继续音频（`5` / 菜单「暂停音频」；对照 `reviewer.py:644-646` 的 `on_pause_audio`）。
pub(crate) fn pause_audio() {
    match crate::platform::audio::toggle_pause() {
        Ok(()) => tracing::info!("音频: 暂停/继续（对照 av_player.toggle_pause）"),
        Err(error) => tracing::warn!("音频暂停失败: {error}"),
    }
}

/// 前后跳 5 秒（`6` / `7` / 菜单里的两项；对照 `reviewer.py:648-656`）。
pub(crate) fn seek_relative(seconds: f32) {
    match crate::platform::audio::seek_relative(seconds) {
        Ok(()) => tracing::info!("音频: 跳到 {seconds:+.0} 秒"),
        Err(error) => tracing::warn!("音频跳转失败: {error}"),
    }
}

/// 重播当前这一面的音频（`r` / `F5` / 菜单里的「重播音频」）。
///
/// 对照 `reviewer.py:1043-1044` 的 `replayAudio`：问题面重播问题面的，答案面重播答案面的。
pub(crate) async fn replay_audio(state: &AppState) {
    let (answer_shown, tags) = {
        let session = state.reviewer.session();
        if session.answer_shown {
            (true, session.answer_tags.clone())
        } else {
            (false, session.question_tags.clone())
        }
    };
    if tags.is_empty() {
        tracing::info!(
            "没有音频可重播（{}）",
            if answer_shown { "答案面" } else { "问题面" }
        );
        return;
    }
    tracing::info!(
        "重播音频（{}，{} 个标签）",
        if answer_shown { "答案面" } else { "问题面" },
        tags.len()
    );
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

/// 读复习界面的显示开关（对照 `col.conf` 的 `dueCounts` / `estTimes`）与时间盒上限。
async fn read_reviewer_ui(state: &AppState) -> anyhow::Result<ReviewerUi> {
    let prefs: Preferences = call(state, "getPreferences", &Empty {}).await?;
    let reviewing = prefs.reviewing.unwrap_or_default();
    Ok(ReviewerUi {
        show_remaining_counts: reviewing.show_remaining_due_counts,
        show_intervals: reviewing.show_intervals_on_buttons,
        // 偏好设置「复习」分页的「时间盒」（分钟 → 秒由前端换算，这里就是秒）
        time_limit_secs: reviewing.time_limit_secs,
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

#[cfg(test)]
mod test {
    use super::av_refs_to_play_icons;

    #[test]
    fn av_refs_become_play_buttons() {
        // 上游渲染器写的标记形式（`rslib/.../writer.rs:137`）
        let marked = av_refs_to_play_icons("前[anki:play:q:0]中[anki:play:a:1]后", 'q');
        assert!(marked.contains("pycmd('play:q:0')"), "{marked}");
        assert!(marked.contains("pycmd('play:a:1')"), "{marked}");
        assert!(!marked.contains("[anki:play:"), "标记应该都被换掉了: {marked}");
        assert_eq!(marked.matches("replay-button").count(), 2);

        // 本外壳实际拿到的原始形式：`[sound:...]` / `[anki:tts ...]`，索引按出现顺序
        let raw = av_refs_to_play_icons(
            "[sound:a.mp3]和[anki:tts en_US:hi]以及[sound:b.mp3]<p>纯文本</p>",
            'q',
        );
        assert!(raw.contains("pycmd('play:q:0')"), "{raw}");
        assert!(raw.contains("pycmd('play:q:1')"), "{raw}");
        assert!(raw.contains("pycmd('play:q:2')"), "{raw}");
        assert_eq!(raw.matches("replay-button").count(), 3);
        assert!(raw.contains("<p>纯文本</p>"), "{raw}");

        // 没有音频的 HTML 原样返回
        assert_eq!(av_refs_to_play_icons("<p>纯文本</p>", 'q'), "<p>纯文本</p>");
    }
}