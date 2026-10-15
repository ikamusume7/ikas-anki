// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! `POST /_anki/pycmd`：桥接命令通道。
//!
//! 对照 Qt 侧 `AnkiWebPage.onBridgeCmd`。请求体是命令字符串
//! （如 `ans`、`ease3`、`close`、`domDone`），响应是 JSON 文本，会传给前端的回调。
//!
//! 已处理的命令集中在复习主链路、编辑器与浏览器；其余命令（工具栏动作、
//! `updateToolbar`、`repaintNeeded` 等）依赖尚未迁入的 Qt 逻辑，会记录日志并返回 `null`。
//!
//! **命令按来源窗口分流**：注入脚本会在每个请求上带 `X-Anki-Window`（见
//! `crate::bridge`）。Qt 里每个 WebView 有自己的 `AnkiWebPage`，命令天然归属该
//! 窗口；这里必须显式判断，否则任意窗口加载时发的 `domDone` 都会重启复习会话。

use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use serde_json::json;

use crate::browser;
use crate::editor;
use crate::importing;
use crate::main_window;
use crate::platform::record;
use crate::preview;
use crate::reviewer;
use crate::reviewer_menu;
use crate::state::AppState;
use crate::windows;

/// 来源窗口的请求头，由注入脚本写入。
const WINDOW_HEADER: &str = "x-anki-window";

pub async fn handle(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !has_api_access(&state, &headers) {
        tracing::warn!("拒绝未授权的桥接命令");
        return (StatusCode::FORBIDDEN, "unauthorized").into_response();
    }

    let window = source_window(&headers);
    let command = String::from_utf8_lossy(&body).trim().to_string();
    tracing::debug!("bridge 命令[{window}]: {command}");

    let value = dispatch(&state, &window, &command)
        .await
        .unwrap_or_else(|| "null".to_string());
    ([(header::CONTENT_TYPE, "application/json")], value).into_response()
}

/// 命令来自哪个窗口。老版本注入脚本不带这个头时按主窗口处理。
fn source_window(headers: &HeaderMap) -> String {
    headers
        .get(WINDOW_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or(windows::MAIN)
        .to_owned()
}

/// 返回 `Some(json)` 表示要把该 JSON 文本回传给前端回调。
async fn dispatch(state: &AppState, window: &str, command: &str) -> Option<String> {
    // 编辑器注入结果的回传与窗口无关（主窗口直接跑编辑器时也会发）
    if handle_editor_report(window, command) {
        return None;
    }

    // 与窗口无关的命令：编辑器/浏览器/主窗口里都能点调色板（PARITY 6.5），
    // 所以放在按窗口分发**之前**。`pickColour:<#rrggbb>` → 原生取色器（或测试钩子）。
    if let Some(current) = command.strip_prefix("pickColour:") {
        // 原生对话框是模态阻塞调用，得照 `platform::dispatch` 那套放到阻塞线程池里
        let current = current.to_string();
        let pick_state = state.clone();
        return match tokio::task::spawn_blocking(move || {
            crate::platform::colours::pick(&pick_state, &current)
        })
        .await
        {
            Ok(Some(colour)) => Some(colour),
            Ok(None) => Some(String::new()),
            Err(_) => {
                tracing::warn!("取色器任务 panic");
                Some(String::new())
            }
        };
    }

    // 非主窗口只处理与它自己相关的命令（关窗、录音、编辑器），
    // 复习相关的命令一律不接受 —— 见文件头关于 `domDone` 的说明。
    if window != windows::MAIN {
        return dispatch_secondary(state, window, command).await;
    }

    match command {
        // 页面就绪。仅当主窗口承载复习界面时才触发复习，
        // 否则加载 graphs 之类的页面也会被当成复习开始。
        "domDone" => {
            let active = state.reviewer_active.load(Ordering::SeqCst);
            tracing::debug!("domDone: 复习模式={active}");
            if active {
                reviewer::start(state).await;
            }
            None
        }
        // 显示答案（对照 Qt 的 `ans`）。
        // 类型作答时桥接脚本会把输入框内容附在命令后面：`ans:<用户输入>`
        "ans" => {
            reviewer::show_answer(state, None).await;
            None
        }
        command if command.starts_with("ans:") => {
            let typed = command.trim_start_matches("ans:").to_string();
            reviewer::show_answer(state, Some(typed)).await;
            None
        }
        // 四档评分（对照 Qt 的 `ease1`..`ease4`）
        "ease1" | "ease2" | "ease3" | "ease4" => {
            let ease = command
                .trim_start_matches("ease")
                .parse::<u32>()
                .unwrap_or(3);
            reviewer::answer(state, ease).await;
            None
        }
        // 撤销（对照 Qt 的 `u` 快捷键与全局 Ctrl+Z）
        "undo" => {
            reviewer::undo(state).await;
            None
        }
        // 自定义调度完成（对照 Qt 的 `statesMutated`）。宿主据此放行评分按钮。
        "statesMutated" => {
            reviewer::mark_states_mutated(state);
            None
        }
        // 自定义调度脚本抛错（本外壳特有的上报，Qt 用 evalWithCallback 检测语法错误）
        command if command.starts_with("stateMutationError:") => {
            tracing::warn!(
                "自定义调度脚本失败: {}",
                command.trim_start_matches("stateMutationError:")
            );
            None
        }
        // 牌组列表（对照 Qt 的 `d` → `moveToState("deckBrowser")`，`main.py:1181`）
        "decks" => {
            if let Err(error) = main_window::show_decks(state) {
                tracing::warn!("切到牌组列表失败: {error:#}");
            }
            None
        }
        // 当前牌组的概览（对照 Qt 的 `s` 在复习状态 → `moveToState("overview")`，
        // `main.py:1240-1245`）
        "overview" => {
            if let Err(error) = main_window::show_overview(state) {
                tracing::warn!("切到牌组概览失败: {error:#}");
            }
            None
        }
        // 开始复习（Overview 的「学习」按钮 / `s` 在概览里）
        "study" => {
            if let Err(error) = main_window::start_study(state) {
                tracing::warn!("开始复习失败: {error:#}");
            }
            None
        }
        // 牌组的齿轮菜单 → 选项（`deckbrowser.py:293-296` 的 `opts:<did>`）
        command if command.starts_with("opts:") => {
            match command.trim_start_matches("opts:").parse::<i64>() {
                Ok(deck_id) => {
                    if let Err(error) = main_window::open_deck_options(state, deck_id) {
                        tracing::warn!("打开牌组选项失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 opts 命令: {command}（{error}）"),
            }
            None
        }
        // 概览里的「自定义学习」（对照 Qt 的 `overview.py:116-117` → `onStudyMore()`；
        // 命令名沿用 Qt 的 `customStudy`）
        command if command.starts_with("customStudy:") => {
            match command.trim_start_matches("customStudy:").parse::<i64>() {
                Ok(deck_id) => {
                    if let Err(error) = main_window::open_custom_study(state, deck_id) {
                        tracing::warn!("打开自定义学习失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 customStudy 命令: {command}（{error}）"),
            }
            None
        }
        // 自定义学习窗口里点了确定：后端已把当前牌组切成筛选牌组，
        // 所以把主窗口导航到它的概览并关掉对话框（Qt 只关对话框）
        "customStudyDone" => {
            if let Err(error) = main_window::show_overview(state) {
                tracing::warn!("切到牌组概览失败: {error:#}");
            }
            windows::close(state, window);
            None
        }
        // 牌组列表底部：导入文件（`deckbrowser.py:399` → `mw.onImport()` →
        // `prompt_for_file_then_import()`：**先弹文件对话框**，再按文件名开对应的页面，
        // 见 `crate::importing`）与获取共享牌组（`deckbrowser.py:396` → `openLink(...)`）
        "import" => {
            if let Err(error) = importing::prompt_and_import(state) {
                tracing::warn!("导入失败: {error:#}");
            }
            None
        }
        "shared" => {
            if let Err(error) = main_window::open_shared_decks() {
                tracing::warn!("打开共享牌组页失败: {error:#}");
            }
            None
        }
        // 主窗口菜单「文件 → 创建备份」（`main.py:1575-1586`：立刻、强制、等它写完）与
        // 「文件 → 退出」（`main.py:1439`）。备份目录在宿主侧（`<profile>/backups`），
        // 所以由宿主执行，页面只发命令。
        "backup" => {
            crate::backup::create_now(state).await;
            None
        }
        "quit" => {
            tracing::info!("菜单 → 退出");
            state.app.exit(0);
            None
        }
        // 文件 → 还原备份（`main.py:441-477`）：选 .colpkg → 确认 → 关集合 → 导入 → 重开
        "restoreBackup" => {
            if let Err(error) = crate::restore::prompt_and_restore(state).await {
                tracing::warn!("还原备份失败: {error:#}");
            }
            None
        }
        // 视图菜单：全屏（`main.py:1484-1495`）与缩放（`main.py:1462-1470`，±0.1 / 重置 1.0）。
        // 切的是**发命令的那个窗口**（Qt 切的是当前活动窗口）。
        "fullscreen" => {
            main_window::toggle_full_screen(state, window);
            None
        }
        command if command.starts_with("zoom:") => {
            main_window::zoom(state, window, command.trim_start_matches("zoom:"));
            None
        }
        // 帮助 → 关于（`onAbout()` → `aqt/about.py`）。上游的对话框里还有 Python/Qt/
        // Chromium 版本，外壳只报自己的版本 + Anki 版本，页面据此渲染弹框。
        "about" => Some(
            json!({
                "anki": anki::version::version(),
                "buildhash": anki::version::buildhash(),
                "shell": env!("CARGO_PKG_VERSION"),
                "platform": format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH),
            })
            .to_string(),
        ),
        // 统计图（对照 Qt 主窗口的全局快捷键 `t` / `Shift+t` → `onStats()`，
        // `main.py:1185`），页面是既有的 `graphs/`
        "stats" => {
            if let Err(error) = main_window::open_stats(state) {
                tracing::warn!("打开统计图失败: {error:#}");
            }
            None
        }
        // 偏好设置（对照 `main.py:1459` 的 `actionPreferences`，快捷键 `Ctrl+P`），
        // 页面是既有的 `preferences/`
        "preferences" => {
            if let Err(error) = main_window::open_preferences(state) {
                tracing::warn!("打开偏好设置失败: {error:#}");
            }
            None
        }
        // 复习界面的「更多」与卡片右键（对照 Qt 的 `more` → `showContextMenu()`，
        // `reviewer.py:682-683`）。菜单由宿主拼数据、页面画，见 `crate::reviewer_menu`。
        "more" => {
            reviewer_menu::show(state).await;
            None
        }
        // 复习菜单里点的一项（`rev:<动作>[:<参数>]`）
        command if command.starts_with("rev:") => {
            reviewer_menu::handle(state, command).await;
            None
        }
        // 编辑当前卡片（对照 Qt 复习界面底部栏的 `edit` → `mw.onEditCurrent()`）
        "edit" => {
            if let Err(error) = editor::open_current_card(state).await {
                tracing::warn!("打开编辑器失败: {error:#}");
            }
            None
        }
        // 添加卡片（对照 Qt 主窗口的全局快捷键 `a` → `mw.onAddCard()`）。
        // Qt 的入口在主窗口工具栏上，本外壳还没有牌组浏览器/工具栏，所以先由
        // 复习页面的 `a` 快捷键触发（见 `reviewer.rs` 的 `install_reviewer_shortcuts`）。
        "add" => {
            if let Err(error) = editor::open_add_cards(state) {
                tracing::warn!("打开添加卡片窗口失败: {error:#}");
            }
            None
        }
        // 打开浏览器（对照 Qt 主窗口的全局快捷键 `b` → `mw.onBrowse()`，
        // `main.py` 的快捷键表；默认搜索 `deck:current` 见 `crate::browser`）。
        "browse" => {
            if let Err(error) = browser::open(state, None).await {
                tracing::warn!("打开浏览器窗口失败: {error:#}");
            }
            None
        }
        // 点图表的柱子 → 看对应的卡片（对照 `qt/aqt/stats.py:132-136` 的
        // `_on_bridge_cmd`：`browserSearch:<query>` → 打开浏览器并搜索，
        // 由 `ts/routes/graphs/GraphsPage.svelte:25-27` 发出）。
        // 这里不再过 `buildSearchString`：图表给的就是搜索词（与 Qt 的
        // `browser.search_for(query)` 一致）。
        command if command.starts_with("browserSearch:") => {
            let query = command.trim_start_matches("browserSearch:").trim();
            if let Err(error) = browser::open(state, Some(query)).await {
                tracing::warn!("图表打开浏览器失败: {error:#}");
            }
            None
        }
        // 主窗口直接加载编辑器时（`ANKI_START_PAGE=editor`）也会发这条，
        // 与编辑器窗口走同一套注入逻辑
        "editorReady" => {
            editor::on_ready(state, window).await;
            None
        }
        // 编辑器（`mode=browser`）里的预览按钮：转给浏览器页面，由它报当前行
        "preview" => {
            browser::request_preview(state);
            None
        }
        // 对照 Qt 在 load finished 时绑定的 Esc 键行为
        "close" => {
            windows::close(state, window);
            None
        }
        other => {
            tracing::debug!("尚未处理的 bridge 命令: {other}");
            None
        }
    }
}

/// 编辑器注入结果的回传（见 `crate::editor::on_ready`）。
///
/// 对照 Qt 的 `evalWithCallback`：只有拿到回调才知道 `loadNote` 真的跑完了。
/// 返回 `true` 表示这条命令已被消费。
fn handle_editor_report(window: &str, command: &str) -> bool {
    if command == "editorLoaded" {
        tracing::info!("编辑器已载入 note[{window}]");
        return true;
    }
    if let Some(error) = command.strip_prefix("editorLoadError:") {
        tracing::warn!("编辑器载入 note 失败[{window}]: {error}");
        return true;
    }
    false
}

/// 主窗口之外的窗口发来的命令。对照 Qt：每个窗口有自己的 `AnkiWebPage`，
/// 只会收到自己页面发出的命令。
async fn dispatch_secondary(state: &AppState, window: &str, command: &str) -> Option<String> {
    match command {
        // 页面就绪：与复习无关的窗口忽略即可（正是这一步要避免误触发复习）。
        // 例外：预览窗口的内容是在它报到之后才注入的（刚开窗就 eval 会落空）。
        "domDone" => {
            if window == windows::PREVIEW {
                preview::on_ready(state).await;
            } else {
                tracing::debug!("忽略 {window} 的 domDone");
            }
            None
        }
        "close" => {
            windows::close(state, window);
            None
        }
        // 录音对话框的「保存 / 取消」（对照 `RecordDialog.accept` / `reject`）
        "saveRecording" => {
            state.recorder.finish(record::Outcome::Save);
            None
        }
        "cancelRecording" => {
            state.recorder.finish(record::Outcome::Discard);
            None
        }
        // 编辑器就绪：把当前卡片的 note 注入进去（对照 `editor.py` 的 `_set_ready`）。
        // 少了这一步编辑器窗口会一直空白，而且不报错 —— 页面自己是"正常"加载完的。
        "editorReady" => {
            editor::on_ready(state, window).await;
            None
        }
        // 「管理笔记类型」窗口（Qt 的 `Models` 对话框）。上游的入口是主窗口菜单
        // （`main.py:1357`）与浏览器侧栏 Note Types 的右键菜单（`tree.py:924-928`）；
        // 本外壳还没有菜单栏（PARITY 1.2），所以挂在浏览器的「更多」菜单里
        "notetypes" => {
            if let Err(error) = main_window::open_notetypes(state) {
                tracing::warn!("打开「管理笔记类型」窗口失败: {error:#}");
            }
            None
        }
        // 浏览器页面就绪：**由宿主决定首屏搜索**（页面自己不知道要搜什么）。
        // 把搜索词放在响应里，而不是"就绪后宿主再注入"，是为了躲开时序问题：
        // 窗口刚创建时 eval 有可能落空，而页面报到一定会到。
        "browserReady" => Some(json!({ "search": state.browser.current_search() }).to_string()),
        // 浏览器窗口标题：`browserTitle:<notesMode>:<selected>:<total>`，对照
        // `browser.py:549-559` 的 `updateTitle()`（页面只报数字，文案在宿主本地化）。
        // 卡片模式与笔记模式用的是两条不同的文案（`browsing-window-title` /
        // `browsing-window-title-notes`）。
        command if command.starts_with("browserTitle:") => {
            let (notes_mode, selected, total) = parse_title_counts(command);
            let i18n = state.backend.i18n();
            let title = if notes_mode {
                i18n.browsing_window_title_notes(selected, total)
            } else {
                i18n.browsing_window_title(selected, total)
            };
            let title = anki_i18n::without_unicode_isolation(&title);
            windows::set_title(state, window, &title);
            None
        }
        // 双击表格里的一行 → 用编辑器打开这张卡片的 note（对照 Qt 把选中的卡片
        // 交给内嵌编辑器，`browser.py:653-658`）。卡片 id 由页面给，note 由宿主解析。
        command if command.starts_with("editCard:") => {
            match command.trim_start_matches("editCard:").parse::<i64>() {
                Ok(card_id) => {
                    if let Err(error) = browser::edit_card(state, card_id).await {
                        tracing::warn!("浏览器打开编辑器失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 editCard 命令: {command}（{error}）"),
            }
            None
        }
        // 笔记模式下的双击：这一行本身就是 note id（`browser_row_for_id` 在笔记模式
        // 收的是 nid），不需要再经卡片反查
        command if command.starts_with("editNote:") => {
            match command.trim_start_matches("editNote:").parse::<i64>() {
                Ok(note_id) => {
                    if let Err(error) = browser::edit_note(state, note_id).await {
                        tracing::warn!("浏览器打开编辑器失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 editNote 命令: {command}（{error}）"),
            }
            None
        }
        // 更改笔记类型（`changeNoteType:<nid>,...`，对照 `browser.py:1241-1252`）：
        // 窗口与保存行为见 `browser::open_change_notetype` 与 `server/api.rs`
        command if command.starts_with("changeNoteType:") => {
            match parse_change_notetype(command) {
                Some(note_ids) => {
                    if let Err(error) = browser::open_change_notetype(state, note_ids).await {
                        tracing::warn!("打开更改笔记类型窗口失败: {error:#}");
                    }
                }
                None => tracing::warn!("非法的 changeNoteType 命令: {command}"),
            }
            None
        }
        // 卡片信息（`cardInfo:<cid>`，对照 `browser.py:799-803` 的 `showCardInfo`）
        command if command.starts_with("ioImageLoaded:") => {
            // 对照 Qt 的 `editor.py:364-372`：编辑器把「遮挡图片已载入」告诉宿主，
            // Qt 用它触发 `editor_mask_editor_did_load_image` 钩子（给 add-ons）；
            // 桌面端自己不做别的事（遮罩编辑器就在页面里），这里只记一条日志。
            tracing::debug!("编辑器报告遮挡图片已载入: {command}");
            None
        }
        command if command.starts_with("cardInfo:") => {
            match command.trim_start_matches("cardInfo:").parse::<i64>() {
                Ok(card_id) => {
                    if let Err(error) = browser::open_card_info(state, card_id) {
                        tracing::warn!("打开卡片信息窗口失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 cardInfo 命令: {command}（{error}）"),
            }
            None
        }
        // 复制笔记（`addCopy:<nid>:<deckId>`，对照 `browser.py:828-834`）：
        // 开「添加卡片」并把这张笔记的内容填进去
        command if command.starts_with("addCopy:") => {
            match parse_add_copy(command) {
                Some((note_id, deck_id)) => {
                    // 笔记类型由宿主查（页面只知道 nid 与 deck）
                    match editor::load_note(state, note_id).await {
                        Ok(note) => {
                            if let Err(error) = editor::open_add_cards_with_note(
                                state,
                                note_id,
                                note.notetype_id,
                                deck_id,
                            ) {
                                tracing::warn!("打开添加卡片窗口失败: {error:#}");
                            }
                        }
                        Err(error) => tracing::warn!("复制笔记取 note 失败: {error:#}"),
                    }
                }
                None => tracing::warn!("非法的 addCopy 命令: {command}"),
            }
            None
        }
        // 浏览器里点了预览（`preview:<cid>`）：已开同一张则关窗，否则打开/切换
        // （对照 `browser.py:853-865` 的 `onTogglePreview`）。
        command if command.starts_with("preview:") => {
            match command.trim_start_matches("preview:").parse::<i64>() {
                Ok(card_id) => {
                    if let Err(error) = preview::toggle(state, card_id).await {
                        tracing::warn!("打开预览窗口失败: {error:#}");
                    }
                }
                Err(error) => tracing::warn!("非法的 preview 命令: {command}（{error}）"),
            }
            None
        }
        // 编辑器里的预览按钮（`PreviewButton.svelte:26-28` 发的裸 `preview`）。
        // 对照 Qt：浏览器的 `editor._links["preview"] = onTogglePreview`（`browser.py:611-614`），
        // 用的是浏览器当前选中的卡片 —— 这份清单在浏览器页面手里，所以转过去问它。
        "preview" => {
            browser::request_preview(state);
            None
        }
        // 预览窗口的按钮与快捷键（对照 `previewer.py:344-362`）
        "previewNext" => {
            preview::next(state).await;
            None
        }
        "previewPrev" => {
            preview::prev(state).await;
            None
        }
        "previewReplay" => {
            preview::replay_audio(state).await;
            None
        }
        // 预览窗口的「仅背面」勾选（对照 `previewer.py:107-115`）
        command if command.starts_with("previewBothSides:") => {
            let value = command.trim_start_matches("previewBothSides:") == "1";
            if let Err(error) = preview::set_both_sides(state, value).await {
                tracing::warn!("切换仅背面失败: {error:#}");
            }
            None
        }
        // 编辑器的状态跟踪命令。Qt 用它们维护 `currentField` / `EditorState`
        // （`editor.py:341-361`），本外壳既没有 addon 也没有"未保存"拦截，只记日志。
        // 注意字段的**保存**不在这里：页面自己去调 `updateNote`（`NoteEditor.svelte:431-435`），
        // 与 Qt 一致；`saved` 只是 `saveNow()` 完成后的通知（`editor.py:545`）。
        command
            if command.starts_with("key")
                || command.starts_with("focus")
                || command.starts_with("blur")
                || command.starts_with("editorState")
                || command == "saved" =>
        {
            tracing::debug!("编辑器命令[{window}]: {command}");
            None
        }
        other => {
            tracing::debug!("窗口 {window} 发来的未处理命令: {other}");
            None
        }
    }
}

/// 解析 `changeNoteType:<nid>,<nid>,...`（浏览器「更改笔记类型」）。
fn parse_change_notetype(command: &str) -> Option<Vec<i64>> {
    let note_ids: Vec<i64> = command
        .trim_start_matches("changeNoteType:")
        .split(',')
        .filter_map(|id| id.trim().parse().ok())
        .collect();
    (!note_ids.is_empty()).then_some(note_ids)
}

/// 解析 `addCopy:<nid>:<deckId>`（浏览器「复制笔记」）。
fn parse_add_copy(command: &str) -> Option<(i64, i64)> {
    let mut parts = command.trim_start_matches("addCopy:").split(':');
    let note_id = parts.next()?.parse().ok()?;
    let deck_id = parts.next()?.parse().ok()?;
    Some((note_id, deck_id))
}

/// 解析 `browserTitle:<notesMode>:<selected>:<total>`；格式不对时按 0 处理
/// （只是标题，不值得报错）。
fn parse_title_counts(command: &str) -> (bool, i32, i32) {
    let mut parts = command.trim_start_matches("browserTitle:").split(':');
    let notes_mode = parts.next().map(|it| it == "1").unwrap_or(false);
    let selected = parts.next().and_then(|it| it.parse().ok()).unwrap_or(0);
    let total = parts.next().and_then(|it| it.parse().ok()).unwrap_or(0);
    (notes_mode, selected, total)
}

fn has_api_access(state: &AppState, headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {}", state.api_key);
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}
