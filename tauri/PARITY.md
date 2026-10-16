# 与原 Anki 的用户可见差异 —— 对齐清单

目标：**让用户觉得用起来和原 Anki 没有差异**。这里只列"用户能感觉到"的差异，
按感知顺序（第一印象 → 日常操作 → 长尾）排，逐项补。实现层面的差异（本地 HTTP 服务、
独立 workspace、`prefs21.db` 直读等）记在 `TAURI_HANDOVER.md`，不在这里。

标记：✅ 已对齐　⚠️ 部分对齐（下面写还差什么）　❌ 未做

每项都要有：**差距**（用户看到什么不同）→ **上游参考** → **验收方式**（怎么证明补齐了）。
补完一项就把这里的状态改掉，并在 `tauri/README.md` / `TAURI_HANDOVER.md` 里同步。

---

## 0. 本次核查（2026-10-16）

方法：**逐条读代码与验证脚本**核实（`ts/routes/**`、`tauri/src/**`、`tauri/tools/**`
对照上游 `qt/aqt/**`），**不采信旧标记**。上一版有 10 处状态与代码不符，已在下表更正；
清单没覆盖到的缺口新增为第 8 节。

核对当次跑通的回归：`eslint --max-warnings=0`（0）、`svelte-check`（0 errors / 0 warnings）、
`verify-decks.ps1`、`verify-reviewer.ps1`、`verify-browser.ps1`（**单独跑**均通过）。

**结论：第 1–7 节的 44 项里 ✅ 4 / ⚠️ 33 / ❌ 6**（首次核查时是 ✅ 3 / ⚠️ 33 / ❌ 8；
2026-10-16 先补了 3.3「偏好设置」→ ⚠️，同一轮的「数据安全三项」把 5.9 从 ❌ 做到 ✅）。
第 8 节那 12 项现在 **⚠️ 7 / ❌ 5**（8.2 多 profile、8.3 夜间模式、
8.4 界面语言已落地）。
另外第 8 节列了 12 项清单完全没覆盖的用户可见缺口。

**回归脚本不是顺序无关的**：三个 `verify-*.ps1` 共用 `out/tauri-test/collection.anki2`，
`verify-decks` 的「还原备份」会替换集合并留下失效的当前牌组，紧接着跑 `verify-browser`
就会首屏 0 结果、5 个开窗断言全挂（单独跑通过）。**跑回归要一个一个来**，见 §9。

---

## 1. 启动与主窗口（第一印象）

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 1.1 | ⚠️ | **牌组列表主界面** | 已有 `ts/routes/decks/`（启动默认页）：今日计数、折叠/展开、Shift 选牌组、点牌组 → 概览 → 学习、概览里的**牌组描述**（可编辑 + markdown）、选项、自定义学习、筛选牌组重建/清空、**取消埋藏**、齿轮菜单（重命名/选项/导出/删除）、**拖拽移动牌组**、底部三按钮、今日学习行。**还差**：概览计数取的是到期树，Qt 用 `sched.counts()` 并单列「埋藏差值 ±N」（有埋藏时概览数字大于实际可学，无提示）；没有 v1→v3 调度器升级提示条；概览缺「Reviews and Updates」共享来源链接；齿轮/文件导出的建议文件名用**叶子名**（子牌组导出文件名与 Qt 不同）；点整行任意处即进概览（Qt 只在点牌组名时进）；默认牌组的删除被禁用；今日学习行无任何断言 | `qt/aqt/deckbrowser.py` + `overview.py` |
| 1.2 | ⚠️ | 菜单栏 + 工具栏 | 菜单栏有了（牌组列表页顶部，File/Edit/View/Tools/Help）：文件=导入/导出/创建备份/**还原备份**/退出、编辑=撤销/重做、视图=全屏/三档缩放、工具=学习牌组/创建筛选牌组/检查数据库/检查媒体/空卡片/管理笔记类型/首选项、帮助=用户手册/支持 Anki/关于；快捷键 `Ctrl+E`/`Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+P`/`F11`/`Ctrl++`/`Ctrl+-`/`Ctrl+0`/`F`。工具栏两页都有（牌组/添加/浏览/统计/同步）。**还差**：**复习界面整条菜单栏没有**（复习中拿不到 导入/导出/备份/还原/检查数据库/管理笔记类型）；缺 Switch Profile(`Ctrl+Shift+P`)/Add-ons(`Ctrl+Shift+A`)/检查更新；同步链接只有文字，没有 Qt 的状态色与旋转图标；「关于」缺 logo/网站/复制调试信息/Python&Qt&Chromium 版本/贡献者名单；「学习牌组」不列筛选牌组（注释却写"筛选牌组也在内"）、无 `Ctrl+N`/上下键/`Ctrl+1..9`/回车提交；菜单不响应 `Ctrl+Q`/`Ctrl+Shift+I`/`/`/`F1` | `qt/aqt/toolbar.py:353-418`、`qt/aqt/forms/main.ui`、`qt/aqt/main.py:1425-1470` |
| 1.3 | ✅ | ~~底部状态栏~~ | **Qt v3 其实没有状态栏**：`forms/main.ui` 里没有 QStatusBar，`qt/aqt` 里也没有 `showMessage` 调用（`profiles.ui` 那个 statusbar 在 `main.py:336` 被设为不可见）。本项按"已对齐"处理 | `qt/aqt/forms/main.ui` |
| 1.4 | ⚠️ | 主窗口快捷键 | 牌组列表页已接 `a`/`b`/`t`/`s`/`c`/`o`/`r`/`e`/`u`/`y`(同步)/`F`(筛选牌组)/`F11`/`Ctrl+E`/`Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+P`/`Ctrl++`/`Ctrl+-`/`Ctrl+0`；复习页已照 `_shortcutKeys()` 接上所有有后端能力的键位（`assets/reviewer-keys.js`，宿主 `include_str!` 注入）：空格/回车（问题面显示答案；答案面按 `meta.spacebar_rates_card` 用焦点档或默认档 3，关闭则不动）、`1`–`4`（仅答案面，宿主另有 `answer_shown` 守卫）、`e`/`m`/`u`/`o`/`i`、`*`/`=`/`-`/`!`/`@`、`Ctrl+1..7`、`Ctrl+Alt+N`/`Ctrl+Alt+E`、`Ctrl+Del`、`Ctrl+Shift+D`、`Ctrl+Z`/`Ctrl+P`，以及主窗口全局的 `a`/`b`/`t`(`Shift+t`)/`d`/`s`/`y`。**还差**：复习页缺 `r`/`F5`/`5`/`6`/`7`/`v`/`Shift+v`（音频，见 2.3）、`Shift+A`（自动前进，2.4）、`Ctrl+Alt+I`（上一张卡片信息，2.2）、`F11`/`Ctrl+E`/`Ctrl+Shift+Z`/缩放三键；牌组列表缺 `d`（概览里按 d 回不到列表）、`Ctrl+Shift+I`、`/`、`Ctrl+Shift+N`、`Ctrl+Shift+A`、`Ctrl+Q`；两页都没有 `Ctrl+:`（调试控制台）。验证：`reviewer-keys-check.mjs` 按 31 组真实按键断言桥接命令（含输入框不抢键），另对真外壳断言评分守卫 | `qt/aqt/main.py:1189-1198`、`qt/aqt/reviewer.py:595-642` |
| 1.5 | ⚠️ | 启动行为 | 启动即牌组列表；**自动备份**（启动一次 + 每 5 分钟，`force=False`，`verify-decks.ps1` 断言产物是 >1KB 的 `.colpkg`）；启动自动同步线程；检查数据库/媒体/空卡片有入口；**还原备份**能用（6.2）。**还差**：界面语言已按 `meta["defaultLang"]` / 系统语言生效（见 8.4），只剩偏好设置里的语言下拉与首次运行的语言框；**主窗口几何不恢复**（固定 1200×820，`windows.rs` 的几何记忆只管 `windows::open` 开的窗口；「重置窗口大小」还需要 prefs21 的删键端点）；**profile 选择已实现**（`meta["last_loaded_profile_name"]` + 切换配置窗口，见 8.2）；缺「检查更新」 | `qt/aqt/main.py:setupProfile`、`_backup` |
| 1.6 | ⚠️ | 每日学习流程 | 队列空了会**回到概览并画上 Congratulations**：宿主在 `reviewer::start()` 里清会话（卡片/状态/计数一起清，否则底栏留着评分按钮、再按 `1`–`4` 会拿过期状态评分）+ `moveToState("overview")`（对照 `reviewer.py:257-259`），概览面板在没有可学卡片时改画恭喜内容（对照 `overview.py:190-209` 的 `_show_finished_screen`）—— 标题「恭喜！…」、`buildNextLearnMsg` 的「下次学习时间 / 剩余学习中卡片」（与 `/congrats` 页共用 `ts/routes/congrats/lib.ts`）、「今日复习上限已达」「今日新卡片上限已达」（`congratsInfo` 的 `reviewRemaining`/`newRemaining`）、埋藏卡片的「取消埋藏」链接（点了真的 `unburyDeck`）、「自定义学习」提示与牌组描述；选项/自定义学习/取消埋藏按钮照 Qt 的底栏在学完时也保留，「学习」按钮隐藏；`undo` 也按当前界面刷新（还在复习页就重取队列，已回概览/牌组列表就让页面重新加载，避免往没有 `_showQuestion` 的页面注入卡片）。**还差**：`/congrats` 路由本身仍然没有打开路径（内容画在概览面板里，因为本外壳的概览就是同一页的面板，见 1.1 的既有偏差）；时间盒到点的「继续 / 结束」对话框（见 2.6）；非 v3 调度器的回列表 + 警告不适用（rslib 只有 v3）。验证：`congrats-check.mjs`（自造一个「今天学完」的牌组 → 发 `study` → 断言两次导航与页面内容 → 点「取消埋藏」后卡片真的回到队列）| `qt/aqt/reviewer.py:257-259`、`qt/aqt/overview.py:190-209`、`ts/routes/congrats/` |

验收：启动外壳 → 截图与真 Anki 打开时的首屏逐项对比；快捷键表逐条按一遍（现在只能脚本级核对）。

## 2. 复习会话

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 2.1 | ⚠️ | **「更多」菜单 / 右键菜单** | 已有 **11 项 + 旗标子菜单**（不是文档原先写的 13 项）：埋藏/暂停的卡片与笔记、重置…、设置到期日…、选项、卡片信息、标记笔记、创建副本…、删除笔记 + 旗标 1-7。UI 与宿主动作都有真断言（菜单渲染/勾选态/子菜单悬停/`rev:flag:1`/表单 URL 编码/`rev:mark`/`rev:due`+undo）。**还差 8 项**（对照 Qt 的 19 项）：上一张卡片信息 `Ctrl+Alt+I`、重播音频 `R`、暂停音频 `5`、`-5` 秒 `6`、`+5` 秒 `7`、录音 `Shift+V`、重播录音 `V`、自动前进 `Shift+A`（带勾选态）；旗标名写死 ftl 默认值，不读集合配置 `flagLabels`；真实菜单的项数/文案只有截图，无断言 | `qt/aqt/reviewer.py:998-1056`（`_contextMenu`） |
| 2.2 | ⚠️ | 卡片信息 / 上一张卡片信息 | 卡片信息窗口两处入口都有断言（复习菜单 `rev:cardInfo`、浏览器菜单 `cardInfo:<cid>`，都断言开窗）。**「上一张卡片信息」完全不存在**：`reviewer.rs` 的 Session 没有"看过的卡片"历史，菜单里也没这一项 | `qt/aqt/reviewer.py:1093-1097` |
| 2.3 | ⚠️ | 音频操作 | 队列播放/自动播放已实现（TTS 合成成 WAV 再入队、`extractAvTags` 有断言）。**还差**：卡面**不画播放按钮**（宿主从不把 `extractAvTags` 结果写回显示文本，也没有 `play:q:N`），卡上的 `[sound:x.mp3]` / `[anki:tts …]` **原样显示、点不了**；**答案面会把问题面音频再播一遍**（`partial_render:false` 让 FrontSide 的 `[sound:]` 留在答案 HTML 里），所以 `replayq` 无效、日志多一次「播放音频」；重播/暂停/前进后退 5 秒/录音 `Shift+V`/重播录音 `V` 全无（`Command` 只有 `Play`，没有 pause/seek） | `qt/aqt/reviewer.py:1043-1048`、`qt/aqt/sound.py:906-932` |
| 2.4 | ❌ | 自动前进 | 没有：`autoAdvance` / `secondsToShowQuestion` / `secondsToShowAnswer` / `questionAction` / `answerAction` / `waitForAudio` 在 `tauri/src` 里 0 引用（只有 rslib 的牌组配置定义）。缺 `Shift+A`、目标牌组秒数与动作、`waitForAudio`、焦点不在窗口时自动失效、tooltip | `qt/aqt/reviewer.py:408-527,1049-1054` |
| 2.5 | ❌ | `current_caller` | 音频队列只有「播放」，没有 caller 概念，也没有 `stop_and_clear_queue_if_caller` 的等价物。影响：编辑器关窗不会停掉自己发起的播放、播放来源不可区分（纯鼠标用户感知较弱） | `qt/aqt/sound.py:171-173`、`qt/aqt/editor.py:479` |
| 2.6 | ⚠️ | 计时器 / 时间盒 | 计时器与"下次间隔"已接（`show_timer` → `maxTime`、答后停表）；**时间盒已实现**：`Reviewer` 里存「开始时刻 + 已答卡片数」（对照 `collection.py:1239-1251` 的 `startTimebox()` / `timeboxReached()`），进入概览（`overview.py:98`）与开始复习（`main.py:1242`）时重新计时，每答一张与 `Preferences.reviewing.time_limit_secs`（对照 `col.conf["timeLim"]`，0 = 关闭）比较（按 Qt 的浮点语义：实际用时 > 上限），到点弹原生「继续 / 结束」框（`studying-continue` / `studying-finish`）——选「结束」回牌组列表（对照 `moveToState("deckBrowser")`），选「继续」重新计时（`reviewer.py:973-992`）。**还差**：计时器仍**没有任何自动断言**（`set-deck-timer.mjs` 不在任何 verify 脚本里，`visual-check.mjs` 把 maxTime 写死 0）；到点的原生框在自动化里靠 `ANKI_TIMEBOX_CHOICE` 钩子跳过，真对话框只能手工点一次。验证：`verify-timebox.ps1` + `timebox-check.mjs`（两种选择各起一次外壳） | `qt/aqt/reviewer.py:975-992`、`qt/aqt/overview.py:98`、`pylib/anki/collection.py:1239-1251` |

验收：`tauri/tools/verify-reviewer.ps1` 逐项断言；菜单项逐条点一遍并检查集合状态。
**注意**：`visual-check.mjs` 里的底栏 HTML 是手抄副本，会与 `reviewer.rs` 漂移。

## 3. 既有页面 / 窗口的接入（成本最低的一类）

这些页面**仓库里已经有**，只是外壳没有入口 —— 补的是"开窗 + 入口 + 关窗契约"。

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 3.1 | ⚠️ | **牌组选项** | 三个入口都有（牌组列表齿轮、复习菜单「选项」、概览 `O`），窗口能开。**还差**：① **筛选牌组点「选项」会开到报错窗口** —— Qt 对 dyn 牌组开 `FilteredDeckConfigDialog`，这里一律开 `deck-options/<did>`，而 rslib 对筛选牌组会直接报错，用户既看不到筛选条件也改不了；② ~~**× 关窗丢改动**~~ → **宿主已补拦截并断言**（`windows::DECK_OPTIONS_CLOSE_REQUEST_JS`：`CloseRequested` 时 `prevent_close` + 让页面跑 `anki.deckOptionsPendingChanges()`；页面里的确认框改成走宿主 `askUser`，测试钩子 `ANKI_CONFIRM_YES` 才管用）——`decks-check.mjs` 用 `close-window.ps1` 真发 WM_CLOSE，断言「交给页面判定 + 窗口关掉」；③ 标题少了牌组名（Qt 是 `actions-options-for(val=名字)`）；④ 只有「开窗」级断言，没有"页面加载成功 + 改动落库"断言 | `qt/aqt/deckoptions.py:62-67,125-134` |
| 3.2 | ⚠️ | **统计图** | 入口两处都有（牌组列表工具栏「统计」、复习页 `t`/`Shift+t`），页面 `graphs/` 默认 `initialSearch="deck:current"`（等于 Qt 的 `_selectedDeck()`，**按当前牌组打开已经成立**）。**还差**：① Qt 的 NewDeckStats 顶部有**牌组下拉**（换牌组 = 设当前牌组 + 刷新），这里换牌组只能靠页面里的「牌组/集合/自定义搜索」；② 没有 **Save PDF**（Qt 是原生按钮 + `printToPdf`）；③ 窗口标题用 `qt-misc-stats`（「统计」），Qt 用 `statistics-title`（「统计数据」）；④ 没有"请先选一个牌组"的兜底 | `qt/aqt/stats.py:52-66,95-123`、`forms/stats.ui:14` |
| 3.3 | ⚠️ | **偏好设置** | **已重写成真首选项页面**（`ts/routes/preferences/`）：七个分页照 `forms/preferences.ui`（显示 / 复习 / 编辑 / 同步 / 备份 / 第三方服务 / 实验功能）+ 底部的**帮助**与**关闭**；纯前端就能落库的都接了 —— 语言（`meta.defaultLang`，选项来自宿主 `languages` 命令，54 种）、主题（发 `theme:` 命令，宿主落库 + 广播）、界面缩放、上下工具栏隐藏、减少动效、极简模式、检查更新两项（只落库）、调度三项（`rollover`/`learnAheadSecs`/`timeLim`）、reviewer 四项、`renderLatex`、编辑五项、同步五项（含 `customSyncUrl` 连带清 `currentSyncUrl`）、备份四项。**还差**：AnkiWeb 的登录/登出还留在牌组列表工具栏（没搬进来）、「媒体同步日志」窗口、`answer_keys` 自定义答案键（pickle 字典键是整数，现有端点读不出来）、`resetWindowSizes`（需要 prefs21 删键端点）、视频驱动（WebView2 无此概念）、AnkiHub（addon 系统已移除）；「干扰项」这几项目前只落库、外壳还没消费。验证：`verify-prefs.ps1` + `prefs-check.mjs`（七页结构 + 每页代表控件的「改 → RPC 复查 → 还原」） | `qt/aqt/preferences.py:39-77`、`forms/preferences.ui` |
| 3.4 | ⚠️ | **导入** | 三条入口都通（牌组列表底部「导入文件」、主窗口拖入、`ANKI_OPEN_PATH` 顶掉对话框），扩展名分流正确，`.apkg` 有端到端断言（造夹具 → 页面导入 → RPC 复查牌组与笔记回来 → `importDone`）。**还差**：`.csv/.tsv/.txt` → `import-csv` 只有单测，没有页面级/落库断言；「拖到主窗口导入」只有 `drop_path` 单测，没有合成 `DragDropEvent` 的端到端；`.colpkg`/`collection.apkg`/`backup-*.apkg` 与 Mnemosyne `.db` 仍只提示不支持（见 6.7） | `qt/aqt/import_export/importing.py`、`qt/aqt/main.py:112-137` |
| 3.5 | ⚠️ | **图像遮挡** | 已核对：这个版本没有独立的遮挡窗口，「遮挡」按钮只是把编辑器里的 `ImageOcclusionPage` 切出来；「选图片」走宿主 `/openFilePicker`。断言覆盖「按钮出现 → 图片选择器 → 拷进媒体夹 → 掩码编辑器渲染出图片」。**没覆盖**：画遮罩并保存成遮挡笔记 | `ts/routes/editor/NoteEditor.svelte`、`ts/routes/image-occlusion/` |
| 3.6 | ⚠️ | **自定义学习 / 筛选牌组** | 自定义学习已实现（六个选项 + cram 四选一 + 标签包含/排除，走 `CustomStudy` RPC，有端到端断言）；筛选牌组编辑器也有（名字/搜索/顺序/限量/重排/即使为空也创建 → `addOrUpdateFilteredDeck`，有断言）。**还差**：第二筛选条件（`search_2`/`order_2`/`limit_2`）、预览间隔三个数字框、每个筛选条件旁的「搜索」按钮、hint 按钮；**FSRS 专用排序被无条件过滤**（Qt 在开着 FSRS 时要显示）；**编辑已有筛选牌组做不到**（唯一入口就是 3.1 那个会报错的「选项」）；TagLimit 用两个文本框代替标签选择框 | `qt/aqt/customstudy.py`、`qt/aqt/filtered_deck.py` |

验收：每个窗口：能开、能关、关窗契约（`deckOptionsRequireClose` 等）走通、改动落库。
**现状**：3.1/3.2/3.3 只有「命令发出/窗口打开」级断言，页面能否加载、改动能否落库、关窗契约都没有断言。

## 4. 编辑器

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 4.1 | ⚠️ | **「字段…」对话框** | 已实现（`ts/routes/fields/`）：增删改/重定位、字体（家族 + 字号）、描述、排序字段、RTL / HTML 默认 / 折叠 / 排除搜索，保存走 `updateNotetype`，取消会问一句，删除有守卫。**还差**：系统字体下拉（这里是文本框）、拖动重排、删除时提示「有 N 张笔记用到它」、字段间跳转按钮。**验证缺口**：只断言了字段数/首个字段名/添加后 +1/保存落库/还原，字体、RTL、排序字段、重定位、删除守卫 0 断言 | `qt/aqt/fields.py` |
| 4.2 | ⚠️ | **「卡片…」对话框** | 已实现（`ts/routes/cards/`）：三个编辑区、添加字段、翻转、卡片类型增删改重排、恢复默认、浏览器外观、牌组覆盖、复制信息、`RenderUncommittedCard` 预览（正/背面 + 填充空字段 + 夜间模式），保存走 `updateNotetype`。**还差**：帮助按钮、系统字体下拉、模板下拉里没有 Qt 的「序号: 名字: 字段摘要」、重排是数字框、复制信息后没有提示。**验证缺口**：翻转/删除/重排/恢复默认/浏览器外观/牌组覆盖/复制信息/夜间模式全无断言，`cards-check.mjs:175` 还是恒真断言 `check(..., true)`。**注**：`Ctrl+L` 在本外壳无效（`NotetypeButtons.svelte:71` 无条件发 `bridgeCommand("cards")`，宿主不处理），但**上游这版同样没处理 `cards`**（`qt/aqt/editor.py` 的 `_links` 只有 paste/cut/copy/saved）→ 属上游同款问题，不算迁移差异；`tauri/src/server/api.rs:75-77` 的注释说错了 | `qt/aqt/clayout.py` |
| 4.3 | ✅ | 编辑器「更多」菜单 | **不存在对齐对象**（已核对）：这一版编辑器工具栏整个迁到了 TS，按钮只有 笔记类型 / 齿轮选项 / 行内格式 / 块格式 / 模板 / 填空题 / 遮挡；`qt/aqt/editor.py` 里没有 `onMore`，`_links` 也只有 paste / cut / copy / saved | — |
| 4.4 | ⚠️ | **拖入文件（HTML5 drop）** | 已实现：`WindowSpec::html5_drops` → `disable_drag_drop_handler()`，编辑器/浏览器编辑区/预览/编辑当前都开了，主窗口刻意保留 Tauri 拦截用于 `.apkg` 拖入；断言覆盖"编辑器窗口关了拦截 + 主窗口没关"。**还差**：**浏览器「复制笔记」开的添加卡片窗口漏了 `.html5_drops(true)`**（`editor.rs` 的 `open_add_cards_with_note`）→ 该路径拖放又被 Tauri 顶掉；`drop-check.mjs` 只测图片分支，音频/URL/文本/HTML 四支无断言；「操作系统 → WebView2」这一步只能真机手工确认 | `qt/aqt/editor.py:120`、`qt/aqt/webview.py:550-552` |
| 4.5 | ⚠️ | 未保存拦截 / `EditorState` | 页面有等价物（600ms 去抖保存、`saveNow`、`shouldPromptBeforeClosing`、`closeAddCards`/`closeEditCurrent`），**但宿主只拦「添加卡片」**（标题栏 × 与 Alt+F4 会问、有断言）。**已补**：**编辑当前 / 浏览器编辑区的原生 × 先落库再关**（`windows::EDITOR_CLOSE_REQUEST_JS` → 页面 `closeEditorWindow()`；关浏览器先让编辑区落库再连带关闭，对照 `browser.py:415-425`），`verify-editor.ps1` / `verify-browser.ps1` 用 `close-window.ps1` 真发 WM_CLOSE 断言。**还差**：`editorState:` 命令宿主只记日志，没有 addon 回调语义 | `qt/aqt/editcurrent.py:45-46`、`qt/aqt/editor.py` 的 `EditorState` |

验收：编辑器验证脚本里加"改字段 → 保存 → 集合内容正确"的既有链路 + 新界面的人工/自动核对。

## 5. 浏览器

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 5.1 | ⚠️ | **查找重复** | 已实现（浏览器「更多」→ 页面内 `FindDuplicatesModal`）：字段下拉、可选筛选器、搜索、分组报告、点一组跳到那些笔记（切笔记模式 + `nid:` 搜索）、一键打 `browsing-duplicate` 标签，全部有断言。**更正**：判重不是调后端的 `col.find_dupes()`，而是在页面里用 `buildSearchString` + `searchNotes` + 逐条 `getNote` + `stripHtml` 重实现的（语义等价，但大集合是 N+1 次 RPC）。**还差**：Qt 是独立非模态对话框；搜索框无历史；点组之后没有「保留/删除」等后续动作 | `qt/aqt/browser/find_duplicates.py` |
| 5.2 | ⚠️ | **管理笔记类型** | 已实现（`ts/routes/notetypes/`）：列表「名字 [笔记数]」、添加两步走、重命名、删除（守护 + 两种确认文案）、「字段…」「卡片…」两个按钮走宿主端点，都有断言。入口暂时挂在浏览器窗口的「更多」菜单（上游是主窗口菜单 + 侧栏 Note Types 右键）。**还差**：Qt 的「选项」按钮（开老的 `ModelAdvanced`）。**验证缺口**：只验了「没人用」的删除文案分支，in-use 分支未验 | `qt/aqt/models.py` |
| 5.3 | ⚠️ | 导出的 CSV / 纯文本 | 已实现：笔记/卡片纯文本（`.txt`，制表符 CSV）、勾选项与初值照 `forms/exporting.ui`，有真文件头与内容断言。**更正**：牌组下拉**有**（牌组列表的齿轮导出会传 deck 范围），只是从浏览器打开时只有「选中的笔记 / 全部集合」。**还差**：没有独立"选列"对话框；没有逐个牌组导出；colpkg 无端到端断言 | `qt/aqt/import_export/exporting.py` |
| 5.4 | ⚠️ | 编辑区位置 | Qt 把编辑区嵌在浏览器窗口内（QSplitter）；外壳是独立窗口（`/editor/` 的 CSP 不允许 iframe），双击才打开 → 不能与结果表并排，分割位置记忆也随之丢失 | `qt/aqt/browser/browser.py:608-630` |
| 5.5 | ⚠️ | 侧栏 | **实际只有 2 个分区**（Decks + Note Types，外加「当前牌组/全部集合」两个按钮），Qt 是 **7 个**（Saved Searches / Today / Card State / Flags / Tags / Decks / Note Types）。**还差**：五个分区、折叠状态、修饰键组合搜索（Ctrl/Alt/Shift+点击）、节点右键菜单（所以「管理笔记类型」只能挪到「更多」）。**验证缺口**：只断言了"侧栏里有默认牌组" | `qt/aqt/browser/sidebar/tree.py` |
| 5.6 | ⚠️ | 加标签对话框 | 纯输入框，**没有补全、没有最近标签**（上游用 QCompleter + 最近标签列表）；标签分隔/校验规则也没对齐 | `qt/aqt/tagedit.py` |
| 5.7 | ⚠️ | 列设置的存储位置 | 列宽/顺序存在**外壳自己的 profile 配置**里（`tauriBrowserColumnWidths`，JSON），Qt 存 QHeaderView 的二进制状态 → 两边交替使用不通用（显示哪些列走集合配置，与 Qt 同键）。断言覆盖拖动换序/写回/不错位/卡片与笔记各一份，但**没有"重启后还在"的持久化断言** | `qt/aqt/utils.py:849-852` |
| 5.8 | ⚠️ | 立即评分 / 查找替换 / 取消暂停埋藏 | 立即评分拆成 4 个菜单项（有断言）；查找替换**缺字段下拉、缺"只看选中笔记"、缺历史**；取消暂停会顺带恢复埋藏（`restoreBuriedAndSuspendedCards` vs Qt 的 `unsuspend_cards`） | `qt/aqt/browser/browser.py` 对应段落 |
| 5.9 | ✅ | 导出到 profile 目录的拦截 | **已实现**：保存对话框那条路（`saveFileDialog`）在选到 profile 目录内时提示「Please choose a different export location.」并**重新弹框**（对照 `exporting.py:119-140` 的 `while True`）；`ANKI_SAVE_PATH` 钩子下直接拒绝（返回空串）并记日志，所以这条路径能端到端断言（`export-guard-check.mjs` + 单元测试）。**与上游的差异**：上游拒的是 `pm.base` 整棵树，这里拒**当前 profile 目录**（`collection.anki2` 所在处）—— 外壳的测试集合就在仓库 `out/` 下，用 base 会把所有测试导出路径一起拒掉 | `qt/aqt/import_export/exporting.py:119-140` |

验收：`tauri/tools/verify-browser.ps1` + `browser-check.mjs`，每项都要有真集合断言。
**现状**：浏览器主流程（列/排序/多选/笔记模式/批量动作/导出/查找重复/预览）断言很扎实，
但侧栏、加标签补全、列设置持久化、导出目录拦截是空的。

## 6. 系统集成与媒体

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 6.1 | ⚠️ | **同步** | 登录 / 同步入口（牌组列表工具栏 + 复习界面工具栏）/ 状态 / 集合同步 / 媒体同步 / 取消 / 自动同步线程都有，假同步服务器下的断言覆盖：真打 `/sync/hostKey`+`/sync/meta`、host key 落 profile、完整同步的「上传/下载」二选一、点下载打到 `/sync/download`、挂起时再点 = 取消、自动同步定时器发起 `/msync/*`。**还差**：① `syncCollection` 与媒体同步的**成功路径零覆盖**（假服务器只到 meta，之后全 500）；② 冲突只有上传/下载二选一，缺 Qt 的分支确认与"空集合"防误触，完成后也不刷新/不关其它窗口；③ 进度只有文字（Qt 有进度条 + Abort）；④ **关闭时同步缺失**（`quit` 直接 `app.exit(0)`）；⑤ 没有 `restoring_backup`/safeMode 守卫；⑥ **启动那一次整份同步从未被断言**（启动时没有凭据，日志只有"还没登录"） | `qt/aqt/sync.py`、`qt/aqt/main.py:1120-1129,1534-1569` |
| 6.2 | ⚠️ | 备份与恢复 | 三条都是真的：自动备份（启动 + 每 5 分钟，`force=False`，目录与文件名与桌面版一致）、手动「创建备份」、**还原备份**（原生对话框开在备份目录 + 原生确认框 + 关集合 → `importCollectionPackage` → 重开 → 回牌组列表），产物与"关集合→导入→重开"都有断言。**还差**：**没有「打开备份文件夹」**；退出时 Qt 还会做 `pragma quick_check` + 损坏警告，这里零收尾；还原后没有 Qt 的 `restoring_backup`（会禁用自动同步/备份）；原生对话框在自动化里都被钩子顶掉，只能手工 | `qt/aqt/main.py:441-477,716-745`、`qt/aqt/forms/profiles.ui:87` |
| 6.3 | ⚠️ | 检查数据库 / 检查媒体 / 空卡片 | 三个入口都在菜单里，真的调 RPC 并弹报告（断言了方法名与报告非空）。**还差**：① **检查媒体缺 Qt 的删除未使用 / 清空回收站 / 恢复回收站 / 给缺失媒体打标签 / 渲染 LaTeX / 查看文件**（`tauri/src` 里没有 `trash`/`restoreTrash` 的调用）；② 空卡片报告是 `@html` 渲染，但用户看到的是字面量 `[anki:nid:123]`（Qt 会变成可点链接并补 `.allempty` 样式），也没有"保留笔记"勾选与删除按钮；③ 空卡片只断言"弹了框"，报告内容未断言 | `qt/aqt/mediacheck.py:123-259`、`qt/aqt/emptycards.py:54-74` |
| 6.4 | ❌ | 录音格式 | 直接给 WAV（`hound`），Anki 用外部 `lame` 转 MP3（装了的话）。代价：约 5MB/分钟，且与桌面版录出的媒体格式不同 | `qt/aqt/sound.py` 的 `encode_audio` |
| 6.5 | ✅ | 原生取色器 | Windows 上用系统取色对话框（`ChooseColorW`，与 Qt 在 Windows 上用的是同一个），16 个自定义色槽写进 `customColorPickerPalette`（与上游同一把键）；页面侧 `<input type="color">` 的点击改走 `pickColour:<当前颜色>`，老宿主没有能力标记时照旧用自带取色器；端点与调色板落库都有断言。**注**：真正的 `ChooseColorW` 从未被自动触发（检查里用 `ANKI_PICK_COLOUR` 短路），只能手工点一次；非 Windows 没有等价实现（见 6.5 备注） | `qt/aqt/mediasrv.py:1122-1128` |
| 6.6 | ❌ | **addon（插件）支持** | 完全没有：`tauri/src` 里没有 addon 加载、没有 `gui_hooks`（只有 CSP 里留了 `_addons/` 前缀）。装了插件的用户会明显觉得不一样 | `qt/aqt/addons.py` |
| 6.7 | ❌ | 文件关联 | **名不副实**：① 双击 `.apkg` = 不存在（`tauri.conf.json` 没有 `fileAssociations`，上游 NSIS 会注册 apkg/colpkg/anki/addon）；② 命令行带文件启动 = 不存在（`tauri/src` 里没有 `env::args()`，只有 `ANKI_*` 环境变量）；③ 「拖到主窗口导入」只有 `drop_path` 单测，**没有任何端到端** | `qt/aqt/main.py` 命令行处理、`qt/installer/app/pyproject.toml:94-99` |
| 6.8 | ⚠️ | 配置同步 | 直接读写 `prefs21.db`（与桌面版同一批键，`syncKey`/`syncUser` 落库有断言）。**还差**：① 唯一的跨实现验证（`make-prefs21-fixture.py` + `check-prefs21.mjs`）**没接进任何 `verify-*.ps1`**，纯手工；② ~~**写的是 JSON null 而不是删键**~~ → **已改成删键**（`Prefs21::set` 遇到 `null` 走 `delete`，对照 `profiles.py:721-722` 的 `profile.pop(key, None)`；`verify-prefs.ps1` 用 `make-prefs21-fixture.py show` dump `_global` 断言键真的从 pickle 里消失）。**还差**：③ `meta.defaultLang` 从未被读（语言只看 `ANKI_LANG`，见 8.4） | `qt/aqt/profiles.py:721-722` |

验收：需要真机/真账号（同步）；其余可用 RPC + 界面断言。
**现状**：备份/还原、检查三项、取色器是真的；同步只验到"登录 + 取消 + 自动媒体同步请求"；
addon、文件关联、录音格式为零。

## 7. 打包、Android 与工程化

| # | 状态 | 项 | 差距 |
| --- | --- | --- | --- |
| 7.1 | ⚠️ | Windows 安装包 | **可分发目录已做实并验证**：`package.ps1` 打出 `anki-tauri.exe + web/ + sveltekit/`（与 `paths.rs` 的 `for_resource_dir()` 一致，实测 exe 43.9MB、web 25 文件、sveltekit 151 文件），`verify-package.ps1` 用 `ANKI_RESOURCE_DIR` 强制打包模式跑 release 产物 + 完整 `decks-check`；`tauri.conf.json` 的 bundle 配置（NSIS + 资源映射 + 图标 + currentUser 安装）也写好了。**还差**：① 安装器**从未生成过**（本机没有 `cargo-tauri`/`@tauri-apps/cli`/NSIS/WiX）；② 无代码签名；③ **没有 `bundle.fileAssociations`** → 即使装了安装包，"双击 .apkg" 仍不成立；④ 元数据不全（FileVersion 0.1.0、无 LegalCopyright/publisher/category）；⑤ **网络其实是通的**（实测 crates.io / api.github.com 都 200），卡的是工具链未安装与首次下载。详见 `tauri/PACKAGING.md` |
| 7.2 | ❌ | Android | 零行 Android 代码：没有 `tauri/gen/android/`、没有 `lib.rs`/`mobile_entry_point`，非 Windows 的路径推断走 `$HOME`（不是应用私有目录）。需要：`tauri android init`、四个 rustup android target、SAF 版导入导出、录音（oboe vs MediaRecorder）、`getFilesDir()`、localhost 明文 HTTP 放行；**必须真机/模拟器**。本机 SDK + NDK 28.2 + JDK 已就位，缺 rustup target（联网）、Tauri CLI、Gradle 发行包 |
| 7.3 | ⚠️ | 构建流程 | 静态资源本来有官方路径（`just build` → `out/sveltekit` + `qt/_aqt/data/web`，正是 `package.ps1` 用的两处），但 **`justfile` 里没有任何 tauri 配方，CI 里也没有外壳的 `cargo check/build`** —— 外壳改动目前完全没有自动回归。缺 `just tauri-build / tauri-run / tauri-package / tauri-verify` 与 descriptors → web/sveltekit → cargo build 的依赖链 |
| 7.4 | ⚠️ | `capabilities/` 与图标 | `tauri/capabilities/` 不存在、`gen/schemas/capabilities.json` 是空数组，但前端不经 IPC（没有 `#[tauri::command]`/`invoke`），**现在无害**，加插件时才需要。图标是真问题：`tauri/icons/icon.ico` 与 `qt/aqt/data/web/imgs/favicon.ico` 逐字节相同，且只有 1 帧 32×32；上游真图标 `qt/installer/app/resources/anki.ico` 有 7 帧。另缺 `.icns`/`.png`（Android/macOS 无图标可用） |

---

## 8. 清单原先没覆盖、但用户可见的缺口（本次核查新增）

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 8.1 | ⚠️ | **偏好设置对话框（8 个分页）** | 见 3.3：七个分页已重写并接通（语言 / 主题 / 缩放 / 调度 / reviewer / 编辑 / 同步 / 备份等约 30 个控件），每项都有「改 → RPC 复查 → 还原」的断言。**还差**：AnkiWeb 登录/登出入口、媒体同步日志窗口、自定义答案键（`answer_keys` 的整数键，需要先扩展 `prefs21.rs`）、`resetWindowSizes`（需要删键端点）、视频驱动与 AnkiHub（依赖已移除的子系统）；「干扰项」与「检查更新」只落库、没有消费方（页面上已标注） | `qt/aqt/preferences.py:39-77`、`forms/preferences.ui` |
| 8.2 | ⚠️ | **多 profile 管理** | **已实现**（`tauri/src/profiles.rs` + `ts/routes/profiles/`）：列出 / 打开 / 新建 / 重命名 / 删除 / 打开备份文件夹，名字校验照 `pylib/anki/utils.py:256-269`，删除至少留一个、目录**进回收站**（`SHFileOperationW` + `FOF_ALLOWUNDO`）；启动按 `meta["last_loaded_profile_name"]` 选 profile（`ANKI_BASE` / `ANKI_PROFILE` / `ANKI_COLLECTION` 三个环境变量用于隔离与测试）；入口在 文件 → 切换配置（`Ctrl+Shift+P`），窗口标题用 `profiles-profiles`（中文「账户」）。验证：`verify-profiles.ps1` + `profiles-check.mjs`（列表/新建/非法名/重命名/删除 + **真切换**：重启后新进程打开 Second 并建出集合）。**还差**：Qt 是关集合 + 重开、**同一个进程**换 profile，外壳改成**重启进程**（用户可见结果等价，差异记在这里）；没有「Downgrade & Quit」按钮；没有首次运行的语言框（`forms/setlang.ui`，见 8.4） | `qt/aqt/profiles.py:128,241-256,262-303,409-422,650-654`、`qt/aqt/main.py:321-437` |
| 8.3 | ⚠️ | **夜间模式 / 主题 / `standard_css`** | **已实现**（`tauri/src/theme.rs`）：`meta["theme"]`（0 跟随系统 / 1 浅色 / 2 深色；跟随系统时 Windows 读注册表 `AppsUseLightTheme`，与 Qt 同一个键）；三条注入通道都通了 —— ① 每个窗口在 document-start 打 `night-mode`/`night_mode`/`nightMode`（`bridge.rs`）、② legacy 页在服务端 HTML 上带 `<html class="night-mode" data-bs-theme>` + body class 并内联 `standard_css()`（字体/缩放/`--canvas`）、③ SvelteKit 页 URL 带 `#night`；卡片 body class 由宿主拼全（`reviewer.rs`，否则 `ts/reviewer/index.ts` 的整串覆盖会把夜间类清掉）；运行期切换（`theme:<0\|1\|2>` 桥接命令）会落库并广播到所有窗口，窗口底色一起换（防白闪）。验证：`verify-theme.ps1`（真实窗口亮度 52.7 → 238.8 → 52.7，含复习界面 52.2）+ `theme-check.mjs`（两条通道的 `--canvas` 取值）。**还差**：`uiScale` 只作用于 legacy 页（SvelteKit 页没注入，不缩放）；`reduce_motion` / `minimalist_mode` / 隐藏上下工具栏那几个 body class 开关没做；偏好设置里的主题下拉还没接（在 3.3 / 8.1 一起做） | `qt/aqt/theme.py:60-102,211-244`、`qt/aqt/webview.py:638-692,910-1037` |
| 8.4 | ⚠️ | **界面语言选择** | **启动链已实现**（`tauri/src/i18n_lang.rs`）：优先级 `ANKI_LANG`（测试钩子）> `meta["defaultLang"]`（桌面版偏好设置写的）> 系统语言（Windows `GetUserDefaultLocaleName`）> en-US；换算照 `lang_to_disk_lang()`（`zh_CN`→`zh-CN`、`de_DE`→`de`）；语言表直接嵌入 `pylib/anki/lang.py` 解析（rslib 没有语言列表 API，打包后 `pylib/` 不分发，所以编译期嵌入）。验证：`verify-i18n.ps1` 四个场景（meta=zh_CN / meta=en_US / 无 prefs21 跟随系统 / `ANKI_LANG` 覆盖），每个都断言宿主生效语言 + `i18nResources` 下发的语言与文案。**还差**：偏好设置里的语言下拉（改完要重启并提示 `preferences-please-restart-anki-to-complete-language`）与首次运行的语言框（`forms/setlang.ui`） | `pylib/anki/lang.py:22-162`、`qt/aqt/preferences.py:442-466`、`qt/aqt/profiles.py:473-504` |
| 8.5 | ❌ | **异常/错误对话框** | 后端异常只落 `tracing::error!` 与终端日志：没有 `show_exception` 那种对话框、没有 Copy debug info 按钮、没有错误提示文件、没有 panic hook → 用户看不到原因，也没有可粘贴的排障信息 | `qt/aqt/errors.py:31,177-208` |
| 8.6 | ❌ | **集合打不开 / 过新 / 损坏的启动提示** | 打开集合失败时只记日志并继续启动（`media_folder` 置空）→ 用户拿到一个功能残缺的窗口，既不知道原因，也没有「降级并退出」 | `qt/aqt/main.py:648-660,745`、`qt/aqt/profiles.py:224-227` |
| 8.7 | ❌ | **schema 改动「需要整合同步」确认框** | 改字段/卡片模板/笔记类型后，Qt 会弹「需要一次性整合同步」的确认；外壳完全没有（只有"删除卡片模板"这类确认） | `qt/aqt/main.py:1688-1705` |
| 8.8 | ❌ | **卡片中的视频播放** | `SoundOrVideo` 一律丢给音频解码（`rodio`），没有 mpv 那套 → 含 `[sound:clip.mp4]` 的牌组只显示空白，日志走"播放失败"分支 | `qt/aqt/sound.py:105-129`、`qt/aqt/mpv.py` |
| 8.9 | ⚠️ | **同步日志 / 媒体同步对话框** | 只有工具栏文字状态，没有 `MediaSyncDialog`（进度 + 日志 + 失败原因） | `qt/aqt/mediasync.py:113-149` |
| 8.10 | ⚠️ | **可取消的进度对话框** | 检查数据库/检查媒体/还原备份只有一个 `busy` 标志，没有进度、不能取消；宿主也没有进度通道 | `qt/aqt/progress.py:340`、`qt/aqt/taskman.py:27` |
| 8.11 | ❌ | **浏览器的侧栏显隐 / 缩放 / 布局 / 全屏 / `Ctrl+G`** | `ts/routes/browser/+page.svelte` 里 `filteredDeck`/`toggleSidebar`/`zoom`/`Ctrl+G` 全无命中 → 从搜索结果一键建筛选牌组的捷径没了，侧栏收不起来，浏览器内也不能缩放/改布局/全屏 | `qt/aqt/browser/browser.py:312-387,843-848` |
| 8.12 | ⚠️ | **自定义 URL scheme 白名单与放行确认** | 卡片里的非 http(s) 链接既不询问也不拦截，直接 `opener::open`；用户也无法维护放行列表 | `qt/aqt/url_schemes.py:18-60` |

已核查**不是**缺口的：TTS（`allTtsVoices`/`writeTtsStream` + 缓存，与上游 Windows 路径一致）；
FrontendService 的 27 个 RPC 都有落点；`.anki-json` 导入（上游自己也没挂进 `IMPORTERS`）。

## 9. 验证体系的缺口（文档里的「✅」就来自这些脚本）

跑回归的正确姿势：**一次一个脚本**（见 §0 的顺序耦合问题），且 `verify-browser` 之前先跑
`verify-reviewer` —— 后者在集合为空时会补一张卡片；紧跟 `verify-decks` 之后跑会因为它的
「还原备份」把集合整个换掉而首屏 0 结果（2026-10-16 复现两次）。当前断言强度分四档：

1. **恒真/无效断言**：`cards-check.mjs:175` 的 `check(..., true)`；`reviewer-menu-check.mjs:310`
   的「more 命令被接受」；`editor-check.mjs` 的字段级取色只剩 `console.log`。
2. **半覆盖**：`drop-check.mjs` 只测图片分支；`occlusion-check.mjs` 不存遮挡笔记；
   `fields-check.mjs` 不验字体/RTL/排序字段/重定位/删除守卫；`notetypes-check.mjs` 只验
   "没人用"的删除分支；`browser-check.mjs` 对侧栏只验"有默认牌组"。
3. **靠钩子顶掉原生交互**：`ANKI_SAVE_PATH`/`ANKI_PICK_COLOUR`/`ANKI_RESTORE_PATH`/`ANKI_BACKUP_NOW`
   → 真对话框（保存、确认、取色、还原）路径全靠手工。
4. **0 断言的主要功能**：计时器（时间盒已有 `timebox-check.mjs`）、上一张卡片信息、
   音频的重播·暂停·±5 秒·录音、埋藏/暂停/重置/副本/删除笔记的**按键与菜单路径**
   （`rev:` 动作只有旗标·标记·到期日有断言）、同步成功路径、TTS 播放链路、
   `prefs21` 桌面版↔外壳互操作、文件关联与命令行、`just`/CI（根本没有）。

另外两处**测试自身**的问题：`visual-check.mjs` 的底栏 HTML 是手抄副本（会与 `reviewer.rs` 漂移、
且把 maxTime 写死 0）；`verify-*.ps1` 与 `*-check.mjs` 共用同一份集合，脚本间有顺序依赖。

**2026-10-16 关窗契约那一轮发现并已修**：`decks-check.mjs` 的概览断言**有状态前提** —— 它断言
概览里有「学习」按钮，但概览在「今天没有可学卡片」时画的是 Congratulations（PARITY 1.6），
没有那个按钮。集合里没有到期卡片时 `verify-decks.ps1` 会从这一项开始连锁失败（实测：概览学习
按钮 → 齿轮选项没开窗 → 导出没走钩子 → 导入/还原备份/自动同步全挂）。**修法**：`decks-check.mjs`
给要打开概览的子牌组补一张新卡片（新卡今天到期）；`verify-decks.ps1` 现已全绿。

## 10. 推进顺序（建议）

1. ~~**首选项真页面（8.1）+ profile 管理（8.2）+ 夜间模式/`standard_css`（8.3）+ 语言（8.4）**~~
   —— **本轮已做**（2026-10-16，四个提交）：`ts/routes/preferences/` 七个分页、
   `tauri/src/profiles.rs` + `ts/routes/profiles/`（文件 → 切换配置 `Ctrl+Shift+P`）、
   `tauri/src/theme.rs`、`tauri/src/i18n_lang.rs`；四个一键脚本 `verify-prefs` /
   `verify-profiles` / `verify-theme` / `verify-i18n` 全绿。剩下的尾巴记在 3.3 / 8.1 / 8.2
   （媒体日志窗口、`answer_keys`、`resetWindowSizes`、「干扰项」的消费方等）
2. ~~**复习键盘（空格/回车/1–4 等，2.x 附）+ Congratulations 收尾（1.6）+ 时间盒（2.6）**~~
   —— **本轮已做**（2026-10-16，三个提交）：`tauri/assets/reviewer-keys.js` 照
   `_shortcutKeys()` 绑完整张可用表 + 宿主 `answer_shown` 守卫 + `meta.spacebar_rates_card`
   （`reviewer-keys-check.mjs`）；队列空了清会话 + 回概览，概览改画 Congratulations
   （`congrats-check.mjs`）；时间盒（`startTimebox` / `timeboxReached` + 「继续 / 结束」框，
   `verify-timebox.ps1` + `timebox-check.mjs` 两种选择各验一次）。剩下的尾巴记在 1.4 / 1.6 / 2.6。
3. **数据安全三项**：~~关窗契约（3.1 牌组选项 / 4.5 编辑当前与浏览器编辑区）~~、
   ~~导出目录拦截（5.9）~~、~~6.8 的写 null 改成删键~~ —— **2026-10-16 已做**（关窗契约：
   `close-window.ps1` 真发 WM_CLOSE，`verify-editor` / `verify-browser` / `decks-check` 各自断言；
   导出拦截：`saveFileDialog` 的守卫 + `export-guard-check.mjs`；写 null → 删键：
   `Prefs21::delete` + `verify-prefs.ps1` 的 pickle dump 断言）。**还差**：筛选牌组「选项」（下面那条）
4. **音频显示与语义（2.3）+ 自动前进（2.4）+ 上一张卡片信息（2.2）**
5. **同步成功率路径与冲突分支、进度条（6.1/8.9）**；**打包** fileAssociations + 签名 + 图标
   （7.1/7.4）；`just` + CI（7.3）；Android（7.2）

环境依赖：同步成功路径要**真 AnkiWeb 账号**；Android/录音/剪贴板/原生对话框要**真机**；
签名要**证书**；`tauri-cli`/NSIS/`rustup target`/Gradle 要**联网**（网络本身是通的）。

每补齐一项：更新本文件的标记 → 在 `tauri/README.md` 改差异表 → 补/改验证脚本 →
在 `TAURI_HANDOVER.md` 的 §6 与 §9 里同步进度。

> 遗留的文档同步工作：`tauri/README.md`（同步链接状态色、偏好设置、"拖入已通"、
> 启动整份同步、空卡片渲染等描述）与 `TAURI_HANDOVER.md` 的 §7 缺口列表仍按旧状态写，
> 需要按本文件的更正一起改。
