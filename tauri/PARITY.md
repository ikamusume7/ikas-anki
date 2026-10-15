# 与原 Anki 的用户可见差异 —— 对齐清单

目标：**让用户觉得用起来和原 Anki 没有差异**。这里只列"用户能感觉到"的差异，
按感知顺序（第一印象 → 日常操作 → 长尾）排，逐项补。实现层面的差异（本地 HTTP 服务、
独立 workspace、`prefs21.db` 直读等）记在 `TAURI_HANDOVER.md`，不在这里。

标记：✅ 已对齐　⚠️ 部分对齐（下面写还差什么）　❌ 未做

每项都要有：**差距**（用户看到什么不同）→ **上游参考** → **验收方式**（怎么证明补齐了）。
补完一项就把这里的状态改掉，并在 `tauri/README.md` / `TAURI_HANDOVER.md` 里同步。

---

## 1. 启动与主窗口（第一印象）

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 1.1 | ⚠️ | **牌组列表主界面** | 已新写 `ts/routes/decks/`（启动默认页）：今日计数、折叠/展开、点牌组 → 概览 → 学习（概览面板里还有**牌组描述**（可编辑 + markdown 渲染）、选项、自定义学习、筛选牌组的**重建/清空**、**取消埋藏**，快捷键 `o`/`r`/`e`/`c`/`u` 照 `overview.py:126-133`）、齿轮菜单（重命名/选项/**导出**/删除）、**拖拽移动牌组**（拖到另一行 → 变成它的子牌组，拖到顶部那条 → 回顶层；`reparentDecks`）、底部三按钮、今日学习行。**差**：默认牌组的删除保护（本外壳禁用删除，上游删掉会重建）、概览计数的「埋藏差值」那一列、同层内的手动排序（上游其实也没有，顺序由后端按名字排） | `qt/aqt/deckbrowser.py` + `overview.py` |
| 1.2 | ⚠️ | 菜单栏 + 工具栏 | **菜单栏有了**（牌组列表页顶部，对照 `main.ui` 的 File/Edit/View/Tools/Help）：文件=导入/导出/创建备份/退出、编辑=撤销/重做、**视图=全屏/放大/缩小/重置缩放**、工具=**学习牌组**/**创建筛选牌组**/检查数据库/检查媒体/空卡片/管理笔记类型/首选项、帮助=用户手册/支持 Anki/**关于（版本信息）**，快捷键 `Ctrl+E`/`Ctrl+Z`/`Ctrl+Shift+Z`/`Ctrl+P`/`F11`/`Ctrl++`/`Ctrl+-`/`Ctrl+0`/`F`；**工具栏**两页都有了（牌组/添加/浏览/统计 + 快捷键提示）。**差**：菜单栏只在牌组列表页（复习界面是 legacy 页，仍是顶部工具栏），缺 Switch Profile / 检查更新 / add-ons / 同步链接（含状态色/旋转图标）；「关于」没有 Python/Qt/Chromium 版本与贡献者名单 | `qt/aqt/toolbar.py:353-418`、`qt/aqt/forms/main.ui`、`qt/aqt/main.py:1425-1470` |
| 1.3 | ✅ | ~~底部状态栏~~ | **Qt v3 其实没有状态栏**：`forms/main.ui` 里没有 QStatusBar，也没有 `showMessage` 调用 —— 同步状态在工具栏的同步链接上、计数在复习界面底部栏上（两者我们都已复刻，同步除外）。本项按"已对齐"处理 | `qt/aqt/forms/main.ui`、`qt/aqt/toolbar.py:421-430` |
| 1.4 | ⚠️ | 主窗口快捷键 | 已接 `u`/`a`/`b`/`t`/`d`/`s`/`Ctrl+P`/`Ctrl+E`（导出）/`Ctrl+Z`/`Ctrl+Shift+Z`/`F11`（全屏）/`Ctrl++`/`Ctrl+-`/`Ctrl+0`（缩放）；**差** `y`（同步）、`Ctrl+:`（调试控制台） | `qt/aqt/main.py:1189-1198` |
| 1.5 | ✅ | 启动行为 | **自动备份已做**（PARITY 6.2）：启动时尝试一次 + 之后每 5 分钟一次，都是 `force=False`、由后端按 `BackupLimits` 节流（启动那一次是替代 Qt 的"关闭时备份"，因为外壳没有优雅关闭的保证）；**打开上次的牌组**本来就一致（主窗口启动在牌组列表、当前牌组高亮）；**检查数据库 / 检查媒体 / 空卡片**有菜单入口（工具菜单，6.3）；**还原备份**也能用了（6.2） | `qt/aqt/main.py:setupProfile`、`_backup` |
| 1.6 | ⚠️ | 每日学习流程 | 复习完的「Congratulations」页面由复习页面自己渲染（`congratsInfo` 已接入）；但"明天再来"的等待时间/提前学习按钮要与 Qt 对齐核对 | `qt/aqt/reviewer.py:930-992` |

验收：启动外壳 → 截图与真 Anki 打开时的首屏逐项对比；快捷键表逐条按一遍。

## 2. 复习会话

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 2.1 | ✅ | **「更多」菜单 / 右键菜单** | 已补：13 项 + 旗标子菜单（埋藏/暂停的卡片与笔记、重置…、设置到期日…、选项、卡片信息、标记笔记、创建副本…、删除笔记）；差的三项见 2.2-2.4 | `qt/aqt/reviewer.py:998-1056`（`_contextMenu`）+ `_linkHandler` 的 `more` |
| 2.2 | ⚠️ | 卡片信息 / 上一张卡片信息 | 卡片信息窗口已有（浏览器与复习都要能开）；**上一张卡片信息**需要维护"看过的卡片"历史 | `qt/aqt/reviewer.py:1093-1097` |
| 2.3 | ⚠️ | 音频操作 | 队列播放、自动播放已对齐；缺**重播 / 暂停 / 前进后退 5 秒 / 录音（Shift+V）/ 重播录音（V）** 这些菜单项与快捷键 | `qt/aqt/reviewer.py:1043-1048`、`qt/aqt/sound.py` |
| 2.4 | ❌ | 自动前进 | Anki 的「自动前进」（`Shift+A`，答完题 N 秒后自动显示答案）没有 | `qt/aqt/reviewer.py:1049-1054`、`profile` 的 `autoAdvance`/`autoAdvanceDelay` |
| 2.5 | ❌ | `current_caller` | TTS/音频的「谁在播」语义（编辑器播放时打断复习界面音频）没有区分 | `TAURI_HANDOVER.md` §7.2 |
| 2.6 | ⚠️ | 时间盒 / 计时器 | 计时器与"下次间隔"已对齐；时间盒到点的提示对话框（继续 / 结束）要与 Qt 核对 | `qt/aqt/reviewer.py:975-992` |

验收：`tauri/tools/verify-reviewer.ps1` 里逐项断言；菜单项逐条点一遍并检查集合状态。

## 3. 既有页面 / 窗口的接入（成本最低的一类）

这些页面**仓库里已经有**，只是外壳没有入口 —— 补的是"开窗 + 入口 + 关窗契约"。

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 3.1 | ⚠️ | **牌组选项** | 已能开（复习界面「更多」→「选项」）；还差**牌组列表上的 Options 入口**（等 1.1） | `qt/aqt/deckoptions.py`、`ts/routes/deck-options/[...deckId]` |
| 3.2 | ⚠️ | **统计图** | 已能开（主窗口 `t` / `Shift+t`，见「复习界面」）；差**按当前牌组打开**（Qt 传 `_selectedDeck()`，这里是整个集合，页面里可换）与工具栏入口 | `qt/aqt/stats.py`、`main.py:1331` |
| 3.3 | ⚠️ | **偏好设置** | 已能开（主窗口 `Ctrl+P`）；Qt 的对话框底部有 Close 按钮，这里用窗口的 ×（页面自己没有关闭按钮） | `qt/aqt/preferences.py`、`main.py:1459` |
| 3.4 | ⚠️ | **导入** | **三条入口都通**（对照 Qt）：牌组列表底部「导入文件」（`pycmd("import")` → `importing::prompt_and_import`）、主窗口拖入文件（`WindowEvent::DragDrop` → `importing::on_drop`，`main.py:112-137`，存在的文件才收、`.colpkg` 排除、一次只导一个）、以及 `ANKI_OPEN_PATH` 顶掉对话框后的选文件路径。宿主先弹**原生文件对话框**（`importing.py:153-163`），再按扩展名挑页面：`.apkg`/`.zip` → `import-anki-package/<百分号编码的路径>`（`import_dialog.py:68`）、`.csv`/`.tsv`/`.txt` → `import-csv/<…>`、其他 → 弹 `importing-unable-to-import-filename`。端到端验证在 `verify-decks.ps1`（`import-check.mjs` 造 `.apkg` 夹具 → 页面导入 → RPC 复查）。**差**：整体替换集合（`.colpkg`、`collection.apkg`、`backup-*.apkg`）与 Mnemosyne `.db` 只提示不支持；`.apkg` 双击打开见 6.7 | `qt/aqt/import_export/importing.py`、`qt/aqt/deckbrowser.py:399`、`qt/aqt/main.py:112-137` |
| 3.5 | ⚠️ | **图像遮挡** | 已核对并验证：**这个版本没有独立的遮挡窗口**，「遮挡」按钮只是把编辑器页面里的 `ImageOcclusionPage`（掩码编辑器）切出来；「选图片」走宿主的 `/openFilePicker`。`tools/occlusion-check.mjs` 覆盖了「按钮出现 → 图片选择器 → 拷进媒体夹 → 掩码编辑器渲染出图片」。**没覆盖**：画遮罩并保存成遮挡笔记 | `ts/routes/editor/NoteEditor.svelte:795-815`、`ts/routes/image-occlusion/` |
| 3.6 | ⚠️ | **自定义学习 / 筛选牌组** | **自定义学习已实现**：概览里的「自定义学习」（快捷键 `C`）→ 新写的 `ts/routes/custom-study/`（六个选项 + cram 的四选一 + 标签包含/排除，走 `CustomStudy` RPC，验证见 `tools/custom-study-check.mjs`）。**筛选牌组编辑器也有了**：工具 → 创建筛选牌组（快捷键 `F`）→ 名字 / 搜索 / 顺序（`filteredDeckOrderLabels`）/ 限量 / 重排 / 即使为空也创建 → `addOrUpdateFilteredDeck`。**差**：`TagLimit` 标签选择对话框（这里是两个文本框）、筛选牌组对话框的第二个筛选条件 / 预览间隔（again·hard·good 秒数）/ 「搜索」按钮（浏览器的搜索编辑器）/ FSRS 专用排序 | `qt/aqt/customstudy.py`、`qt/aqt/filtered_deck.py`、`qt/aqt/overview.py:284-292` |

验收：每个窗口：能开、能关、关窗契约（`deckOptionsRequireClose` 等）走通、改动落库。

## 4. 编辑器

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 4.1 | ⚠️ | **「字段…」对话框** | **已实现**：新写的 `ts/routes/fields/`（编辑器工具栏的「字段」按钮 → 前端端点 `openFieldsDialog` → 宿主对当前编辑器窗口的笔记类型开窗）。字段增删/改名/重新定位、编辑字体（家族+字号）、描述、排序字段、RTL / HTML 默认 / 折叠 / 排除搜索都在，保存走 `updateNotetype`，取消会问一句。**差**：系统字体下拉（这里是文本框）、拖动重排、删除时提示「有 N 张笔记用到它」、字段间跳转按钮 | `qt/aqt/fields.py`（要新写界面） |
| 4.2 | ⚠️ | **「卡片…」对话框** | **已实现**：新写的 `ts/routes/cards/`（编辑器工具栏的「卡片」/ `Ctrl+L` → 前端端点 `openCardsDialog` → 宿主对着编辑器窗口的笔记类型 + 那张 note + 当前卡片的 `ord` 开窗）。三个编辑区（正面/背面/样式）、添加字段、翻转、卡片类型增删改重排、恢复默认、浏览器外观、牌组覆盖、复制信息、`RenderUncommittedCard` 预览（正/背面 + 填充空字段 + 夜间模式）都在，保存走 `updateNotetype`。**差**：帮助按钮（HelpPage）、系统字体下拉（这里是文本框）、下拉里没有 Qt 的「序号: 名字: 字段摘要」、卡片类型重排是数字框、复制信息后没有提示 | `qt/aqt/clayout.py`（884 行，要新写界面） |
| 4.3 | — | 编辑器「更多」菜单 | **不存在**（已核对）：这一版编辑器工具栏整个迁到了 TS（`ts/routes/editor/editor-toolbar/EditorToolbar.svelte:98-131`，按钮只有 笔记类型 / 齿轮选项 / 行内格式 / 块格式 / 模板 / 填空题 / 遮挡），`qt/aqt/editor.py` 里没有 `onMore`、`_links` 也只有 `paste` / `cut` / `copy` / `saved` 四项；旧版那个「更多」菜单已经拆成齿轮（`OptionsButtons.svelte`）与两个按钮，没有要对齐的东西 | — |
| 4.4 | ⚠️ | **拖入文件（HTML5 drop）** | **已实现**：页面链路本来就是上游 TS 的（`data-transfer.ts` 的 `handleDrop`，图片 / 音频 / URL / 文本 / HTML 五支 + `addMediaFile` / `convertPastedImage`），宿主补的是「别挡路」——`WindowSpec::html5_drops` → `disable_drag_drop_handler()`（Tauri 默认会顶掉 WebView2 的拖放处理器，页面收不到 HTML5 drop）。**差**：「操作系统 → WebView2」那一步只能靠真机手工确认（自动化里只断言宿主日志），以及「拖 `.apkg` 到主窗口导入」（6.7） | `qt/aqt/editor.py:120` 的 `allow_drops`、`webview.py:550-552`、`main.py:112-137` |
| 4.5 | ⚠️ | 未保存拦截 / `EditorState` | 字段是即时保存的（600ms 去抖），Qt 另有"未保存"状态与关窗拦截；影响 addon 与极端场景 | `qt/aqt/editor.py` 的 `EditorState` |

验收：编辑器验证脚本里加"改字段 → 保存 → 集合内容正确"的既有链路 + 新界面的人工/自动核对。

## 5. 浏览器

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 5.1 | ⚠️ | **查找重复** | **已实现**：浏览器「更多」菜单 → 页面内的 `FindDuplicatesModal.svelte`——「搜索位置」字段下拉、「可选筛选器」、搜索、分组报告（「N 条笔记: 值」）、点一组跳到那些笔记（切笔记模式 + `nid:…` 搜索）、「Tag Duplicates」一键打 `browsing-duplicate` 标签；判重复刻 `col.find_dupes()`（`collection.py:744-778`）。验证见 `browser-check.mjs` 第 8.8 节。**差**：Qt 是独立非模态对话框（报告画在 QWebView 里），这里是页面内模态框；搜索框没有历史记录；逐条 `getNote` 取字段值，大集合慢（上游直接读 SQL） | `qt/aqt/browser/find_duplicates.py`、`qt/aqt/forms/finddupes.ui` |
| 5.2 | ⚠️ | **管理笔记类型** | **已实现**：新写的 `ts/routes/notetypes/` —— 列表「名字 [笔记数]」（`getNotetypeNamesAndCounts`）、添加两步走（6 种原厂类型 / 克隆现有类型 → 问名字）、重命名、删除（守卫与两种确认文案照 `models.py:189-198`），「字段…」「卡片…」两个按钮走宿主端点 `openFieldsDialogForNotetype` / `openCardsDialogForNotetype`。**入口**因本外壳还没有菜单栏，暂时挂在浏览器窗口的「更多」菜单（上游是主窗口菜单 `main.py:1357` 与浏览器侧栏 Note Types 节点的右键菜单 `tree.py:924-928`）；验证见 `tools/notetypes-check.mjs`（由 `verify-browser.ps1` 调用）。**差**：Qt 的「选项」按钮（开老的 `ModelAdvanced`，内容就是字段/卡片两块） | `qt/aqt/models.py` |
| 5.3 | ⚠️ | 导出的 CSV / 纯文本 | **已实现**：导出对话框的「笔记纯文本 / 卡片纯文本」（`.txt`，制表符分隔的 CSV，后端 `exportNoteCsv` / `exportCardCsv`），勾选项（HTML / 标签 / 牌组名 / 笔记类型 / GUID）与初值照 `forms/exporting.ui`；验证见 `browser-check.mjs`（真的导出，并按文件头与内容断言）。**差**：没有独立的"选列"对话框（列由勾选项决定，Qt 也只按勾选项）、没有牌组下拉（只有「选中的笔记 / 全部集合」）、没有逐个牌组导出 | `qt/aqt/import_export/exporting.py` 的 `NoteCsvExporter`/`CardCsvExporter`、`forms/exporting.ui` |
| 5.4 | ⚠️ | 编辑区位置 | Qt 把编辑区嵌在浏览器窗口内；外壳是独立窗口（`/editor/` 的 CSP 不允许 iframe），双击才打开 | `qt/aqt/browser/browser.py:608-630` |
| 5.5 | ⚠️ | 侧栏 | 4 个分区（Qt 7 个 + 折叠状态 + 修饰键组合搜索） | `qt/aqt/browser/sidebar/tree.py` |
| 5.6 | ⚠️ | 加标签对话框 | 没有补全与最近标签 | `qt/aqt/tagedit.py` |
| 5.7 | ⚠️ | 列设置的存储位置 | 列宽/顺序存在**外壳自己的 profile 配置**里（Qt 存 QHeaderView 的二进制状态）→ 两边交替使用时不通用 | `qt/aqt/utils.py:saveHeader` |
| 5.8 | ⚠️ | 立即评分 / 查找替换 / 取消暂停埋藏 | 菜单替代对话框、查找替换少了字段下拉与"只看选中笔记"、取消暂停会顺带恢复埋藏 | `qt/aqt/browser/browser.py` 对应段落 |
| 5.9 | ⚠️ | 导出到 profile 目录的拦截 | 上游会拒绝并提示换个位置 | `qt/aqt/import_export/exporting.py:132-138` |

验收：`tauri/tools/verify-browser.ps1` + `browser-check.mjs`，每项都要有真集合断言。

## 6. 系统集成与媒体

| # | 状态 | 项 | 差距 | 上游参考 |
| --- | --- | --- | --- | --- |
| 6.1 | ⚠️ | **同步** | **登录 / 同步入口 / 状态 / 媒体同步 / 取消 / 自动同步有了**：工具栏最右的「同步」（`Y` 键）→ 未登录弹登录框 → `syncLogin` → `syncKey`/`syncUser` 存进 **profile 配置**（`prefs21.db`，与桌面版同一批键）→ `syncStatus` → `syncCollection` →（`syncMedia` 开着时）`syncMedia` + 轮询 `mediaSyncStatus`；需要完整同步时给「上传 / 下载」二选一；同步中再点一次 = 取消（`abortSync`/`abortMediaSync`）；宿主侧还有**自动同步**线程（`tauri/src/autosync.rs`：启动时整份同步一次 + 之后按 `autoSyncMediaMinutes`（默认 15 分钟）做媒体同步，开关 `autoSync`）。验证：`decks-check.mjs` 5i/5j（假同步服务器）。**差**：「AnkiWeb 冲突」的完整处理（只做了上传/下载入口）、复习界面工具栏上的同步链接、同步进度条（只有文字）、关闭时同步（外壳没有优雅关闭的钩子） | `qt/aqt/sync.py`、`qt/aqt/main.py:1120-1129,1534-1569`、`rslib/src/sync` |
| 6.2 | ✅ | 备份与恢复 | **自动备份**（`tauri/src/backup.rs`：启动时 + 每 5 分钟，`force=False`，目录与文件名与桌面版一致 `<profile>/backups/backup-*.colpkg`，份数由后端瘦身）、**手动「创建备份」入口**（菜单 文件 → 创建备份）、**还原备份**（文件 → 还原备份…：原生文件对话框默认开在备份目录 + 原生确认框 + 宿主关集合 → `importCollectionPackage` → 重开集合 → 回牌组列表）。**差**：没有「打开备份文件夹」这一项 | `qt/aqt/main.py:_backup`、`qt/aqt/main.py:441-477`、`qt/aqt/profiles.py` |
| 6.3 | ⚠️ | 检查数据库 / 检查媒体 / 空卡片 | **有入口了**：牌组列表的菜单「工具」→ 检查数据库 / 检查媒体 / 空卡片，各自调 RPC 并弹出报告框（对照 `qt/aqt/dbcheck.py` / `mediacheck.py` / `emptycards.py`）。**差**：空卡片报告这里只显示文本（上游会渲染卡片列表并给「删除」按钮）、检查媒体的「删除未使用文件 / 恢复回收站」按钮 | Anki 主窗口 Tools 菜单 |
| 6.4 | ❌ | 录音格式 | 直接给 WAV；Anki 用外部 `lame` 转 MP3（装了的话） | `qt/aqt/sound.py` 的 `encode_audio` |
| 6.5 | ✅ | 原生取色器 | **已实现**：Windows 上用系统取色对话框（`ChooseColorW`，与 Qt 在 Windows 上用的是同一个），16 个自定义色槽写进集合配置 `customColorPickerPalette`（与上游 `mediasrv.save_custom_colours` 同一把键，页面用后端的 `getCustomColours` 读）。页面侧 `<input type="color">` 的点击改走 `pickColour:<当前颜色>` 桥接命令（老宿主没有能力标记 `window.__ankiHostColourPicker` 时照旧用自带取色器）。**差**：非 Windows 平台没有等价的原生对话框（Android 上仍是页面自带的） | `qt/aqt/mediasrv.py:1122-1128` |
| 6.6 | ❌ | **addon（插件）支持** | 完全没有。装了插件的用户会明显觉得不一样 | `qt/aqt/addons.py` |
| 6.7 | ⚠️ | 文件关联 | **拖入文件已通**：主窗口保留 Tauri 的拖放拦截，`DragDropEvent` → `importing::on_drop`（`main.py:112-137`）；**差**双击 `.apkg` 打开（要 Windows 文件关联，属 M4 打包）与命令行带文件启动 | `qt/aqt/main.py` 的命令行参数处理 |
| 6.8 | ⚠️ | 配置同步 | 直接读写 `prefs21.db`（已对齐）；但列宽这类"外壳自己的"配置不互通（见 5.7） | —— |

验收：需要真机/真账号（同步）；其余可用 RPC + 界面断言。

## 7. 打包、Android 与工程化

| # | 状态 | 项 | 差距 |
| --- | --- | --- | --- |
| 7.1 | ⚠️ | Windows 安装包 | **可分发目录已做并验证**：`tauri/tools/package.ps1` 打出 `anki-tauri.exe + web/ + sveltekit/`（与 `paths.rs` 的 `for_resource_dir()` 一致），`verify-package.ps1` 用 `ANKI_RESOURCE_DIR` 强制打包模式跑 release 产物 + 完整 `decks-check`。`tauri.conf.json` 的 bundle 配置（NSIS + 资源映射 + 图标）也写好了。**差**：真正生成 NSIS/MSI 需要联网下载 Tauri CLI 与 WiX/NSIS（本环境没有）；代码签名未做。详见 `tauri/PACKAGING.md` |
| 7.2 | ❌ | Android | `tauri android init/build` 未做；录音（`cpal`/oboe vs `MediaRecorder`）、SAF、路径（`getFilesDir()`）都要过；需要 Android SDK/NDK 与 Gradle 依赖下载 |
| 7.3 | ❌ | 构建流程 | 静态资源仍靠手工命令，未接 `just` |
| 7.4 | ❌ | `capabilities/` 与图标 | 目录缺失、图标是 favicon 副本 |

---

## 推进顺序（建议）

1. ~~**2.1 复习界面「更多」菜单**~~ —— **已补**（13 项 + 旗标子菜单 + 两个表单对话框；
   页面画菜单、宿主拼数据与执行动作，见 `tauri/README.md` 的「复习界面」）
2. **3.1–3.6 既有页面接入**（牌组选项 / 统计 / 偏好 / 导入 / 遮挡 / 自定义学习）——
仓库里都有页面，只差入口，性价比最高。**六项都已补**：牌组选项（复习菜单
「选项」+ 牌组列表的齿轮）、统计（主窗口 `t`）、设置（主窗口 `Ctrl+P`）、
导入（牌组列表底部的「导入文件」+ 主窗口拖入）、图像遮挡（编辑器里的「遮挡」按钮，
这个版本是内嵌的掩码编辑器，没有独立窗口）、自定义学习（新写的
`ts/routes/custom-study/`）；验证分别在 `verify-editor.ps1` / `verify-decks.ps1`
3. **1.1–1.6 主窗口与牌组列表** —— **1.1 已补**（`ts/routes/decks/`，启动默认页；
   概览做成了同页面板，`d`/`s` 快捷键一并接上；齿轮里也补了**导出**）、
   **1.2 的两条工具栏都有了**（复习界面由宿主注入 `assets/reviewer-toolbar.js`）、
   **1.3 经核对 Qt v3 并没有状态栏**（已按对齐处理）。剩下的：1.2 的同步链接与菜单栏、
   1.4 的 `y`/`Ctrl+E`、1.1 的牌组描述与拖拽排序、1.5 启动行为（检查/备份/打开上次的牌组）、
   1.6 收尾核对
4. **4.1–4.4 编辑器** —— 4.1/4.2 两个对话框与 4.4 拖入文件都补完了（验证都在
   `verify-editor.ps1`）；**4.3「更多」菜单经核对上游并不存在**（工具栏已迁到 TS，
   `editor.py` 没有 `onMore`），剩下的只有 4.5 未保存拦截 / `EditorState`
5. **~~5.1–5.3 浏览器余项~~** —— **全部补完**：5.1 查找重复（`FindDuplicatesModal.svelte`）、
   5.2 管理笔记类型（`ts/routes/notetypes/`）、5.3 CSV / 纯文本导出（`ExportDialog.svelte`）；
   验证都在 `verify-browser.ps1`（`browser-check.mjs` / `notetypes-check.mjs`）。
   浏览器只剩 5.5–5.9 那几条小差异（侧栏分区、加标签补全、列设置存储、导出目录拦截…）
6. **6.x 系统集成**（同步、备份、addon 是硬骨头）
7. **7.x 打包与 Android**

每补齐一项：更新本文件的标记 → 在 `tauri/README.md` 改差异表 → 补/改验证脚本 →
在 `TAURI_HANDOVER.md` 的 §6 与 §9 里同步进度。
