<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
牌组列表（主窗口的默认页面）。

对照 `qt/aqt/deckbrowser.py`：

- 表头与列：`decks-deck` / `actions-new` / `decks-learn-header` / `decks-review-header`
  （`deckbrowser.py:212-231`），计数来自牌组树的 `new_count` / `learn_count` /
  `review_count`（`deckbrowser.py:287-291`，0 时置灰 `zero-count`）；
- 牌组名是链接，点了会**把该牌组设为当前牌组并进入概览**（`deckbrowser.py:269-276`
  的 `open:<did>` → `_onOpen` → `set_current_deck` + `mw.onOverview()`）；
- 折叠箭头 → `collapse:<did>`（`deckbrowser.py:258`，写回集合的 `study_collapsed`）；
- Shift+点击整行 → `select:<did>`（`deckbrowser.py:248`，只切当前牌组）；
- 齿轮 → `opts:<did>` → 重命名 / 选项 / **导出** / 删除（`deckbrowser.py:293-296,309-327`；
  导出走 `mw.onExport(did=...)` → `ExportDialog(mw, did=did)`，范围默认是**这个牌组**，
  建议文件名也用牌组名 —— 复用浏览器那套 `ExportDialog.svelte` + `exporting.ts`）；
- **拖拽**：把一行拖到另一行上 → 变成它的子牌组；拖到列表顶部那条
  `top-level-drag-row` → 回到顶层（`deckbrowser.ts:6-32` 的 jQuery UI 拖放 →
  `deckbrowser.py:121-123` 的 `drag:source,target` → `reparentDecks`）。
- 底部按钮：获取共享牌组 / 创建牌组 / 导入文件（`deckbrowser.py:382-394`）；
- 底部还有一行「今日学习」（`deckbrowser.py:207-210` 的 `studied_today`）。

概览（`?deck=<id>`，Qt 的 Overview，`qt/aqt/overview.py`）本来在主窗口里是**另一个
页面**（`moveToState("overview")`）；本外壳把它做成同一个页面里的一块面板：牌组名、
New / Learning / To Review 计数与「学习」按钮（`overview.py:228-266` 的表格与
`studying-study-now`）。Qt 的牌组描述、自定义学习、取消埋藏、空牌组跳过等都没做，
见 README 的差异表。

顶部工具栏（还原 Qt 主窗口的 TopWebView，`qt/aqt/toolbar.py:353-389`）：牌组 / 添加 /
浏览 / 统计 —— **同步按钮没做**（本外壳没有同步，见 `tauri/PARITY.md` 6.1）。
-->
<script lang="ts">
    import { SetDeckCollapsedRequest, SetDeckCollapsedRequest_Scope } from "@generated/anki/decks_pb";
    import type { DeckTreeNode } from "@generated/anki/decks_pb";
    import {
        Deck,
        DeckId,
        DeckIds,
        Deck_Filtered,
        Deck_Filtered_SearchTerm,
        Deck_Filtered_SearchTerm_Order,
        GetDeckNamesRequest,
        RenameDeckRequest,
    } from "@generated/anki/decks_pb";
    import { Empty } from "@generated/anki/generic_pb";
    import {
        SyncAuth,
        SyncCollectionResponse_ChangesRequired,
        SyncStatusResponse_Required,
    } from "@generated/anki/sync_pb";
    import { HelpPageLinkRequest_HelpPage } from "@generated/anki/links_pb";
    import { UnburyDeckRequest_Mode } from "@generated/anki/scheduler_pb";
    import * as tr from "@generated/ftl";
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import {
        addDeck,
        addOrUpdateFilteredDeck,
        checkDatabase,
        checkMedia,
        congratsInfo,
        deckTree,
        emptyFilteredDeck,
        filteredDeckOrderLabels,
        fullUploadOrDownload,
        getCurrentDeck,
        getDeck,
        getDeckNames,
        getEmptyCards,
        getOrCreateFilteredDeck,
        helpPageLink,
        openLink,
        rebuildFilteredDeck,
        redo,
        removeDecks,
        renameDeck,
        renderMarkdown,
        reparentDecks,
        setCurrentDeck,
        setDeckCollapsed,
        studiedToday,
        syncCollection,
        syncLogin,
        syncStatus,
        unburyDeck,
        undo,
        updateDeck,
    } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { getProfileConfig, setProfileConfig } from "@tslib/profile";
    import { onMount } from "svelte";

    import Modal from "$lib/components/Modal.svelte";

    // 菜单是浏览器页面里那一个（行右键与工具栏「更多」共用）；它是通用组件，
    // 这里直接复用，省得再写一份定位/关闭逻辑
    import ContextMenu from "../browser/ContextMenu.svelte";
    import ExportDialog from "../browser/ExportDialog.svelte";
    import { performExport } from "../browser/exporting";
    import type { ExportChoice, MenuSection } from "../browser/menu";

    interface Row {
        node: DeckTreeNode;
        depth: number;
    }

    let rows: Row[] = [];
    /** 整棵树（含被折叠的子节点），概览面板按 id 找节点用 */
    let nodes = new Map<number, DeckTreeNode>();
    let currentDeckId = 0n;
    let studied = "";
    let busy = false;
    /** 概览面板要显示的牌组（`null` = 只显示列表） */
    let overview: DeckTreeNode | null = null;
    /** 概览的补充信息：`getDeck` 的完整对象（编辑描述要用）、渲染好的描述、埋藏标记 */
    let overviewDeck: Deck | null = null;
    let overviewDescription = "";
    let haveBuried = false;
    let haveSchedBuried = false;
    let haveUserBuried = false;
    /** 描述编辑弹框（对照 `deckdescription.py`） */
    let descriptionModal: Modal;
    let descriptionText = "";
    let descriptionMarkdown = false;
    /** 「取消埋藏」的三选一弹框（两种埋藏都有时才弹，`overview.py:152-175`） */
    let unburyModal: Modal;
    /** 拖拽移动牌组：源牌组、当前高亮的落点、失败提示 */
    let dragSource: bigint | null = null;
    let dropTarget: bigint | null = null;
    let dragStatus = "";
    /** 菜单栏：当前展开的菜单；工具菜单那几个报告用的弹框 */
    let openMenu: string | null = null;
    let reportModal: Modal;
    let reportTitle = "";
    let reportBody = "";
    /** 工具 → 学习牌组（`aqt/studydeck.py` 的 StudyDeck 对话框） */
    let studyModal: Modal;
    let studyFilter = "";
    let studyChoice = "";
    /** 候选牌组：**完整名字**（树节点的 `name` 只是叶子名）+ id */
    let studyEntries: { id: bigint; name: string }[] = [];
    let studyMatching: string[] = [];
    /** 「学习牌组」里点了「添加」：新建完直接进那个牌组的概览（上游 `success` 回调） */
    let createThenStudy = false;
    /** 工具 → 创建筛选牌组（`aqt/filtered_deck.py` 的 FilteredDeckConfigDialog） */
    let filteredModal: Modal;
    let filteredId = 0n;
    let filteredConfig: Deck_Filtered | null = null;
    let filteredName = "";
    let filteredSearch = "";
    let filteredLimit = 100;
    let filteredOrder: Deck_Filtered_SearchTerm_Order =
        Deck_Filtered_SearchTerm_Order.OLDEST_REVIEWED_FIRST;
    let filteredOrderChoices: { order: Deck_Filtered_SearchTerm_Order; label: string }[] = [];
    let filteredResched = false;
    /** 「即使为空也创建」勾选（上游 `decks_create_even_if_empty`） */
    let filteredAllowEmpty = false;
    /** 同步（PARITY 6.1）：登录弹框、状态机、全量同步的二选一 */
    let syncModal: Modal;
    let syncFullModal: Modal;
    let syncUser = "";
    let syncPassword = "";
    let syncEndpoint = "";
    let syncState: "off" | "idle" | "syncing" | "error" = "off";
    let syncMessage = "";
    /** 齿轮菜单的位置与目标牌组 */
    let gear: { x: number; y: number; node: DeckTreeNode } | null = null;

    let textModal: Modal;
    let confirmModal: Modal;
    /** 导出对话框（与浏览器「更多」→ 导出共用）；打开时记下是哪个牌组 */
    let exportDialog: ExportDialog;
    let exportDeck: { id: bigint; name: string } | null = null;
    /** 导出之后的提示（上游是一个 tooltip；牌组列表没有状态栏，就放在底部那一行） */
    let exportStatus = "";
    /** 文本弹框在做什么：新建牌组 / 重命名 */
    let textMode: "create" | "rename" = "create";
    let textValue = "";
    /** 重命名与删除的目标牌组 */
    let target: DeckTreeNode | null = null;

    $: textTitle = textMode === "create" ? tr.decksCreateDeck() : tr.actionsRename();
    $: textPrompt = textMode === "create" ? tr.decksNewDeckName() : tr.actionsRename();
    /** 当前概览的牌组是不是筛选牌组（决定显示「重建/清空」还是「自定义学习」） */
    $: overviewFiltered = overviewDeck?.kind.case === "filtered";
    // 「学习牌组」列表：随着候选列表更新与筛选框输入重算。
    // 注意这里**直接**引用 `studyEntries` —— Svelte 的响应式依赖是静态分析的，
    // 挪进函数里的话依赖会只剩 `studyFilter`（表现为：刚打开弹框时列表是空的）
    $: studyMatching = studyEntries
        .map((entry) => entry.name)
        .filter((name) => matchesStudyFilter(name, studyFilter));

    /** 取整棵树（`now` 必填：后端按它算「今天」的到期数）。 */
    async function loadTree(): Promise<void> {
        const now = BigInt(Math.floor(Date.now() / 1000));
        const root = await deckTree({ now });
        rows = [];
        nodes = new Map();
        for (const child of root.children) {
            flatten(child, 0);
        }
        // 概览面板跟着刷新（重新取一次，计数才是新的）
        if (overview) {
            overview = nodes.get(Number(overview.deckId)) ?? null;
        }
    }

    /**
     * 展平树。`collapsed` 是后端给的**当前状态**（Qt 同样读它，见
     * `deckbrowser.py:266`），所以被折叠的子节点不进 `rows`，但仍进 `nodes`。
     */
    function flatten(node: DeckTreeNode, depth: number): void {
        nodes.set(Number(node.deckId), node);
        rows = [...rows, { node, depth }];
        if (node.collapsed) {
            for (const child of node.children) {
                collect(child);
            }
            return;
        }
        for (const child of node.children) {
            flatten(child, depth + 1);
        }
    }

    /** 只登记、不显示（折叠的子节点）。 */
    function collect(node: DeckTreeNode): void {
        nodes.set(Number(node.deckId), node);
        for (const child of node.children) {
            collect(child);
        }
    }

    async function reload(): Promise<void> {
        // 模块要按本页面用到的文案来选，漏了哪个按钮就会显示 `missing-key: xxx`
        await Promise.all([
            loadTree(),
            getCurrentDeck(new Empty({}))
                .then((deck) => {
                    currentDeckId = deck.id;
                })
                .catch(() => {
                    currentDeckId = 0n;
                }),
            studiedToday(new Empty({}))
                .then((text) => {
                    studied = text.val;
                })
                .catch(() => {
                    studied = "";
                }),
        ]);
        await loadOverviewExtra();
    }

    /** 进入某个牌组的概览（Qt 的 `open:<did>`：设为当前牌组 + 切到概览）。 */
    async function openDeck(node: DeckTreeNode): Promise<void> {
        await setCurrentDeck(new DeckId({ did: node.deckId }));
        // 整页导航：与宿主切页（`main_window.rs`）保持同一套办法，页面重新初始化
        window.location.href = `/decks/?deck=${node.deckId}`;
    }

    /** Shift+点击：只把该牌组设为当前牌组（Qt 的 `select:<did>`）。 */
    async function selectDeck(node: DeckTreeNode): Promise<void> {
        await setCurrentDeck(new DeckId({ did: node.deckId }));
        currentDeckId = node.deckId;
    }

    async function toggleCollapse(node: DeckTreeNode): Promise<void> {
        await setDeckCollapsed(
            new SetDeckCollapsedRequest({
                deckId: node.deckId,
                collapsed: !node.collapsed,
                scope: SetDeckCollapsedRequest_Scope.REVIEWER,
            }),
        );
        await loadTree();
    }

    // 拖拽移动牌组 ############################################################

    /**
     * 上游是 jQuery UI 的 `draggable`/`droppable`（`deckbrowser.ts:6-32`）：放到某个牌组行上
     * 就变成它的子牌组，放到列表顶部那条 `top-level-drag-row` 上就回到顶层（目标 id 传 0）。
     * 这里用 HTML5 拖放，源牌组记在组件状态里（合成事件也能驱动，方便验证脚本）。
     */
    function onDragStart(node: DeckTreeNode, event: DragEvent): void {
        dragSource = node.deckId;
        dragStatus = "";
        if (event.dataTransfer) {
            event.dataTransfer.effectAllowed = "move";
            event.dataTransfer.setData("text/plain", String(node.deckId));
        }
    }

    function onDragOver(target: bigint, event: DragEvent): void {
        if (dragSource === null || dragSource === target) {
            return;
        }
        // 不 preventDefault 的话浏览器不会派发 drop
        event.preventDefault();
        if (event.dataTransfer) {
            event.dataTransfer.dropEffect = "move";
        }
        dropTarget = target;
    }

    function onDragLeave(target: bigint): void {
        if (dropTarget === target) {
            dropTarget = null;
        }
    }

    /** 放下：`reparentDecks`（`deckbrowser.py:121-123,360-363`）。`newParent = 0` 表示顶层。 */
    async function onDrop(target: bigint, event: DragEvent): Promise<void> {
        event.preventDefault();
        const source = dragSource;
        dragSource = null;
        dropTarget = null;
        if (source === null || source === target) {
            return;
        }
        busy = true;
        try {
            await reparentDecks({ deckIds: [source], newParent: target });
            await reload();
        } catch (problem) {
            // 后端只拒绝一种情况：目标必须是叶子节点（筛选牌组不能当父牌组）
            console.error("拖拽移动牌组失败", problem);
            dragStatus = String(problem);
        } finally {
            busy = false;
        }
    }

    function onDragEnd(): void {
        dragSource = null;
        dropTarget = null;
    }

    // 菜单栏（主窗口）##########################################################

    /** 取 Qt 的加速键文案（`qt-accel-*`）；`&` 是助记符，网页上要去掉。 */
    function qtAccel(key: string): string {
        return translateRaw(key).replace(/&/g, "");
    }

    /** 导出（`Ctrl+E` 与「文件 → 导出…」）：范围默认是**当前牌组**（`main.py:1398-1401`）。 */
    function exportCurrentDeck(): void {
        exportStatus = "";
        const node = nodes.get(Number(currentDeckId)) ?? null;
        const deck = node ? { id: node.deckId, name: node.name } : undefined;
        exportDeck = deck ?? null;
        exportDialog.show(deck ? { deck } : {});
    }

    /** 编辑 → 撤销 / 重做（`main.py:1448-1450`）。 */
    async function undoLast(kind: "undo" | "redo"): Promise<void> {
        busy = true;
        try {
            if (kind === "undo") {
                await undo(new Empty({}));
            } else {
                await redo(new Empty({}));
            }
            await reload();
        } catch (problem) {
            // 没有可撤销的东西时后端会报错（上游在状态栏提示）
            console.error("撤销/重做失败", problem);
        } finally {
            busy = false;
        }
    }

    function showReport(title: string, body: string): void {
        reportTitle = title;
        reportBody = body;
        reportModal.show();
    }

    /** 工具 → 检查数据库（`main.py:1710-1713` → `aqt/dbcheck.py`）。 */
    async function runDatabaseCheck(): Promise<void> {
        busy = true;
        try {
            const response = await checkDatabase(new Empty({}));
            const title = qtAccel("qt-accel-check-database");
            if (response.problems.length === 0) {
                showReport(title, `<p>${tr.databaseCheckRebuilt()}</p>`);
            } else {
                // 问题文案由后端本地化好（`database-check-*` 系列）
                showReport(
                    title,
                    `<ul>${response.problems.map((problem) => `<li>${problem}</li>`).join("")}</ul>`,
                );
            }
        } finally {
            busy = false;
        }
    }

    /** 工具 → 检查媒体（`main.py:1715-1719` → `aqt/mediacheck.py`）。 */
    async function runMediaCheck(): Promise<void> {
        busy = true;
        try {
            const response = await checkMedia(new Empty({}));
            showReport(qtAccel("qt-accel-check-media"), response.report);
        } finally {
            busy = false;
        }
    }

    /** 工具 → 空卡片（`main.py:1740-1743` → `aqt/emptycards.py`，上游在对话框里渲染卡片）。 */
    async function runEmptyCardsCheck(): Promise<void> {
        busy = true;
        try {
            const response = await getEmptyCards(new Empty({}));
            // 没有空卡片时后端给空报告（`emptycards.rs:78-88` 才往里写字），补一句文案
            showReport(
                tr.actionsEmptyCards(),
                response.report || `<p>${tr.emptyCardsNotFound()}</p>`,
            );
        } finally {
            busy = false;
        }
    }

    /** 帮助 → 用户手册：后端给出语言相关的链接（对照 `onDocumentation` → `openHelp(INDEX)`）。 */
    async function openGuide(): Promise<void> {
        const url = await helpPageLink({ page: HelpPageLinkRequest_HelpPage.INDEX });
        if (url.val) {
            await openLink({ val: url.val });
        }
    }

    /** 帮助 → 支持 Anki（对照 `onDonate` → `openLink(aqt.appDonate)`）。 */
    async function openDonate(): Promise<void> {
        await openLink({ val: "https://docs.ankiweb.net/manual/contrib" });
    }

    /** 视图 → 放大 / 缩小 / 重置：宿主侧 `set_zoom`（`main.py:1462-1470`）。 */
    function zoomView(mode: "in" | "out" | "reset"): void {
        bridgeCommand(`zoom:${mode}`);
    }

    // 同步（PARITY 6.1）#########################################################

    /**
     * 凭据存在 **profile 配置**里，与桌面版同一批键（`qt/aqt/profiles.py:660-725`）：
     * `syncKey`（host key，未登录时不存在）、`syncUser`、`customSyncUrl`、`syncMedia`。
     * 外壳的 profile 配置后端就是桌面版的 `prefs21.db`，所以两边看到的是同一份凭据。
     */
    async function loadSyncState(): Promise<void> {
        syncState = (await getProfileConfig("syncKey")) ? "idle" : "off";
    }

    async function currentSyncAuth(): Promise<SyncAuth | null> {
        const hkey: string | null = await getProfileConfig("syncKey");
        if (!hkey) {
            return null;
        }
        const endpoint: string | null = await getProfileConfig("customSyncUrl");
        return new SyncAuth({ hkey, endpoint: endpoint || undefined });
    }

    /** 工具栏「同步」/ `Y`：没登录先弹登录框，否则直接同步（`main.py:on_sync_button_clicked`）。 */
    function askSync(): void {
        void (async () => {
            if (syncState === "syncing") {
                return;
            }
            const auth = await currentSyncAuth();
            if (!auth) {
                await openSyncLogin();
                return;
            }
            await runSync(auth);
        })();
    }

    /** 登录弹框（对照 `aqt/sync.py` 的 `SyncLoginDialog`：邮箱 + 密码 + 自定义服务器）。 */
    async function openSyncLogin(): Promise<void> {
        syncUser = (await getProfileConfig("syncUser")) ?? "";
        syncEndpoint = (await getProfileConfig("customSyncUrl")) ?? "";
        syncPassword = "";
        syncMessage = "";
        syncModal.show();
    }

    /** 登录：`syncLogin` → `syncKey` / `syncUser` 落进 profile（`profiles.py:660-663`）。 */
    async function doSyncLogin(): Promise<void> {
        syncState = "syncing";
        syncMessage = "";
        try {
            const auth = await syncLogin({
                username: syncUser,
                password: syncPassword,
                endpoint: syncEndpoint || undefined,
            });
            await setProfileConfig("syncKey", auth.hkey);
            await setProfileConfig("syncUser", syncUser);
            if (syncEndpoint) {
                await setProfileConfig("customSyncUrl", syncEndpoint);
            }
            syncPassword = "";
            syncModal.hide();
            await runSync(
                new SyncAuth({
                    hkey: auth.hkey,
                    endpoint: auth.endpoint || syncEndpoint || undefined,
                }),
            );
        } catch (problem) {
            syncState = "error";
            syncMessage = String(problem);
        }
    }

    /** 同步：`syncStatus` → `syncCollection`（`aqt/sync.py` 的 `sync()`）。 */
    async function runSync(auth: SyncAuth): Promise<void> {
        syncState = "syncing";
        syncMessage = "";
        try {
            const status = await syncStatus(auth);
            if (status.required !== SyncStatusResponse_Required.NO_CHANGES) {
                // 需要完整同步（上传或下载）：上游会弹一个二选一对话框
                syncState = "idle";
                syncMessage = tr.syncConflictExplanation();
                syncFullModal.show();
                return;
            }
            const syncMedia: boolean = (await getProfileConfig("syncMedia")) ?? true;
            const result = await syncCollection({ auth, syncMedia });
            if (
                result.required === SyncCollectionResponse_ChangesRequired.FULL_SYNC ||
                result.required === SyncCollectionResponse_ChangesRequired.FULL_UPLOAD ||
                result.required === SyncCollectionResponse_ChangesRequired.FULL_DOWNLOAD
            ) {
                syncState = "idle";
                syncMessage = result.serverMessage || tr.syncConflictExplanation();
                syncFullModal.show();
                return;
            }
            syncState = "idle";
            syncMessage = result.serverMessage || tr.syncMediaComplete();
        } catch (problem) {
            // 同步失败时后端有时给不带消息的错误（例如同步协议层的问题），
            // 这时用上游那句"AnkiWeb 出了问题"兜底
            const text = String(problem).replace(/[\u200b-\u200d\ufeff]/g, "").trim();
            const bare = text.length === 0 || /^Error[:\s]*$/.test(text);
            syncState = "error";
            syncMessage = bare ? tr.syncServerError() : text;
        }
    }

    /** 全量同步（上传 / 下载），对照 `sync.py` 的 `full_sync()`。 */
    async function startFullSync(upload: boolean): Promise<void> {
        const auth = await currentSyncAuth();
        if (!auth) {
            return;
        }
        syncFullModal.hide();
        syncState = "syncing";
        syncMessage = "";
        try {
            await fullUploadOrDownload({ auth, upload });
            syncState = "idle";
            syncMessage = upload ? tr.syncUploadingToAnkiweb() : tr.syncDownloadingFromAnkiweb();
        } catch (problem) {
            syncState = "error";
            syncMessage = String(problem);
        }
    }

    // 工具 → 学习牌组 ##########################################################

    /**
     * 上游 `StudyDeck._matches`：筛选词按空格拆开，**每个词**都要出现在名字里（都转小写）。
     */
    function matchesStudyFilter(name: string, filter: string): boolean {
        const haystack = name.toLowerCase();
        return filter
            .toLowerCase()
            .split(" ")
            .filter((word) => word.length > 0)
            .every((word) => haystack.includes(word));
    }

    /** 工具 → 学习牌组：列出现有牌组（含筛选牌组），选中后设为当前牌组并进概览。 */
    async function askStudyDeck(): Promise<void> {
        studyFilter = "";
        // 上游 `all_names_and_ids(include_filtered=True, skip_empty_default=True)`：
        // 树节点的 `name` 只是**叶子名**，所以单独取一次完整名字（筛选牌组也在内）
        studyEntries = (
            await getDeckNames(new GetDeckNamesRequest({ skipEmptyDefault: true }))
        ).entries.map((entry) => ({ id: entry.id, name: entry.name }));
        studyChoice = studyEntries.find((entry) => entry.id === currentDeckId)?.name ?? "";
        studyModal.show();
    }

    /** 上游 `callback`：`decks.id(name)` → `set_current_deck` → `moveToState("overview")`。 */
    async function studyChosenDeck(): Promise<void> {
        const entry = studyEntries.find((it) => it.name === studyChoice);
        if (!entry) {
            return;
        }
        studyModal.hide();
        await studyDeckById(entry.id);
    }

    /** 设为当前牌组并进概览（整页导航，与 `openDeck` 同一招）。 */
    async function studyDeckById(deckId: bigint): Promise<void> {
        await setCurrentDeck(new DeckId({ did: deckId }));
        window.location.href = `/decks/?deck=${deckId}`;
    }

    /** 「学习牌组」里的「添加」（`Ctrl+N`）：新建的牌组直接进概览。 */
    function askCreateThenStudy(): void {
        studyModal.hide();
        createThenStudy = true;
        askCreate();
    }

    // 工具 → 创建筛选牌组 ######################################################

    /**
     * 工具 → 创建筛选牌组（`aqt/filtered_deck.py` 的 `FilteredDeckConfigDialog`）。
     *
     * 草稿来自 `getOrCreateFilteredDeck(DeckId(0))`（上游同样传 0 = 新建），确定后走
     * `addOrUpdateFilteredDeck`，成功即进这个筛选牌组的概览。
     *
     * 与上游的差异：没有第二个筛选条件、没有预览间隔（again/hard/good 秒数）、
     * 没有「搜索」按钮（那是浏览器的搜索编辑器）；FSRS 专用排序（可提取性升/降序）也不列。
     */
    async function askFilteredDeck(): Promise<void> {
        busy = true;
        try {
            const draft = await getOrCreateFilteredDeck(new DeckId({ did: 0n }));
            const term = draft.config?.searchTerms[0];
            filteredId = draft.id;
            filteredConfig = draft.config ?? null;
            filteredName = draft.name;
            filteredSearch = term?.search ?? "";
            filteredLimit = term?.limit || 100;
            filteredOrder = term?.order ?? Deck_Filtered_SearchTerm_Order.OLDEST_REVIEWED_FIRST;
            filteredResched = draft.config?.reschedule ?? false;
            filteredAllowEmpty = false;
            // 顺序下拉：后端每个 order 一个本地化标签（`filteredDeckOrderLabels`），
            // FSRS 专用的两个（`RETRIEVABILITY_*`）不上架
            filteredOrderChoices = (await filteredDeckOrderLabels(new Empty({}))).vals
                .map((label, order) => ({ order: order as Deck_Filtered_SearchTerm_Order, label }))
                .filter(
                    ({ order }) =>
                        order !== Deck_Filtered_SearchTerm_Order.RETRIEVABILITY_ASCENDING &&
                        order !== Deck_Filtered_SearchTerm_Order.RETRIEVABILITY_DESCENDING,
                );
            filteredModal.show();
        } catch (problem) {
            console.error("获取筛选牌组草稿失败", problem);
        } finally {
            busy = false;
        }
    }

    /** 确定：`addOrUpdateFilteredDeck`（上游对话框的 OK/Build），成功即进它的概览。 */
    async function buildFilteredDeck(): Promise<void> {
        busy = true;
        try {
            // 草稿里的其它字段（预览间隔 / v1 的 delays）原样带上
            const config = new Deck_Filtered(filteredConfig ?? {});
            config.reschedule = filteredResched;
            config.searchTerms = [
                new Deck_Filtered_SearchTerm({
                    search: filteredSearch,
                    limit: Math.max(0, Math.floor(filteredLimit)),
                    order: filteredOrder,
                }),
            ];
            const changes = await addOrUpdateFilteredDeck({
                id: filteredId,
                name: filteredName.trim() || "Filtered Deck",
                config,
                // 上游的「即使为空也创建/更新」勾选（默认关：搜不到卡片后端会报错并回滚）
                allowEmpty: filteredAllowEmpty,
            });
            filteredModal.hide();
            await studyDeckById(changes.id);
        } catch (problem) {
            // 搜不到卡片时后端会报 `SearchReturnedNoCards`（除非勾了「即使为空」）
            console.error("创建筛选牌组失败", problem);
            exportStatus = String(problem);
        } finally {
            busy = false;
        }
    }

    /**
     * 帮助 → 关于（对照 `onAbout` → `aqt/about.py`）。
     *
     * 版本信息由宿主给（`about` 桥接命令返回 JSON）：Anki 版本来自 `anki::version`，
     * 外壳版本是 `CARGO_PKG_VERSION`。上游对话框里还有 Python / Qt / Chromium 版本 —— 外壳
     * 没有这三样。
     */
    function showAbout(): void {
        bridgeCommand<{ anki?: string; buildhash?: string; shell?: string; platform?: string }>(
            "about",
            (info) => {
                const data = info ?? {};
                // `about.ftl` 在 ftl/qt 下：**没有**生成的 `tr.aboutXxx()`（只有 core 模块才有），
                // 所以走 `translateRaw("about-...")`
                showReport(
                    translateRaw("about-about-anki"),
                    [
                        `<p>${translateRaw("about-anki-is-a-friendly-intelligent-spaced")}</p>`,
                        `<p>${translateRaw("about-version", {
                            val: `${data.anki ?? "?"} (${data.buildhash || "dev"})`,
                        })}<br>Anki Tauri 外壳 ${data.shell ?? "?"} / ${data.platform ?? ""}</p>`,
                        `<p>${translateRaw("about-anki-is-licensed-under-the-agpl3")}</p>`,
                    ].join(""),
                );
            },
        );
    }

    /** 菜单栏内容（对照 `main.ui` 的 menuCol / menuEdit / menuTools / menuHelp）。 */
    function menuBar(): {
        label: string;
        items: { label: string; action: string; shortcut?: string; run: () => void }[];
    }[] {
        return [
            {
                label: qtAccel("qt-accel-file"),
                items: [
                    {
                        label: qtAccel("qt-accel-import"),
                        action: "import",
                        run: () => bridgeCommand("import"),
                    },
                    {
                        label: qtAccel("qt-accel-export"),
                        action: "export",
                        shortcut: "Ctrl+E",
                        run: exportCurrentDeck,
                    },
                    {
                        label: qtAccel("qt-accel-create-backup"),
                        action: "backup",
                        run: () => {
                            exportStatus = qtAccel("qt-accel-create-backup");
                            bridgeCommand("backup");
                        },
                    },
                    {
                        // 还原备份（`onOpenBackup`）：选 .colpkg → 确认 → 宿主关集合/导入/重开
                        label: translateRaw("qt-accel-load-backup"),
                        action: "restoreBackup",
                        run: () => {
                            exportStatus = translateRaw("qt-accel-load-backup");
                            bridgeCommand("restoreBackup");
                        },
                    },
                    {
                        label: qtAccel("qt-accel-exit"),
                        action: "quit",
                        run: () => bridgeCommand("quit"),
                    },
                ],
            },
            {
                label: qtAccel("qt-accel-edit"),
                items: [
                    {
                        label: tr.undoUndo(),
                        action: "undo",
                        shortcut: "Ctrl+Z",
                        run: () => void undoLast("undo"),
                    },
                    {
                        label: tr.undoRedo(),
                        action: "redo",
                        shortcut: "Ctrl+Shift+Z",
                        run: () => void undoLast("redo"),
                    },
                ],
            },
            {
                label: qtAccel("qt-accel-view"),
                items: [
                    {
                        label: qtAccel("qt-accel-full-screen"),
                        action: "fullscreen",
                        shortcut: "F11",
                        run: () => bridgeCommand("fullscreen"),
                    },
                    {
                        label: qtAccel("qt-accel-zoom-in"),
                        action: "zoomIn",
                        shortcut: "Ctrl++",
                        run: () => zoomView("in"),
                    },
                    {
                        label: qtAccel("qt-accel-zoom-out"),
                        action: "zoomOut",
                        shortcut: "Ctrl+-",
                        run: () => zoomView("out"),
                    },
                    {
                        label: qtAccel("qt-accel-reset-zoom"),
                        action: "zoomReset",
                        shortcut: "Ctrl+0",
                        run: () => zoomView("reset"),
                    },
                ],
            },
            {
                label: qtAccel("qt-accel-tools"),
                items: [
                    {
                        label: tr.decksStudyDeck(),
                        action: "studyDeck",
                        run: askStudyDeck,
                    },
                    {
                        label: translateRaw("qt-misc-create-filtered-deck"),
                        action: "createFiltered",
                        shortcut: "F",
                        run: () => void askFilteredDeck(),
                    },
                    {
                        label: qtAccel("qt-accel-check-database"),
                        action: "checkDatabase",
                        run: () => void runDatabaseCheck(),
                    },
                    {
                        label: qtAccel("qt-accel-check-media"),
                        action: "checkMedia",
                        run: () => void runMediaCheck(),
                    },
                    {
                        label: tr.actionsEmptyCards(),
                        action: "emptyCards",
                        run: () => void runEmptyCardsCheck(),
                    },
                    {
                        label: translateRaw("qt-misc-manage-note-types"),
                        action: "notetypes",
                        run: () => bridgeCommand("notetypes"),
                    },
                    {
                        label: qtAccel("qt-accel-preferences"),
                        action: "preferences",
                        shortcut: "Ctrl+P",
                        run: () => bridgeCommand("preferences"),
                    },
                ],
            },
            {
                label: qtAccel("qt-accel-help"),
                items: [
                    {
                        label: qtAccel("qt-accel-guide"),
                        action: "guide",
                        run: () => void openGuide(),
                    },
                    {
                        label: qtAccel("qt-accel-support-anki"),
                        action: "donate",
                        run: () => void openDonate(),
                    },
                    {
                        label: qtAccel("qt-accel-about"),
                        action: "about",
                        run: showAbout,
                    },
                ],
            },
        ];
    }

    /** 概览里的「学习」：开始复习当前牌组（宿主切到复习页）。 */
    async function study(): Promise<void> {
        bridgeCommand("study");
    }

    // 概览的补充信息与动作 ####################################################

    /**
     * 取概览面板的补充数据（`overview.py:182-226` 的 `_renderPage` /
     * `_desc`，以及 `:283-297` 的底部按钮）：
     *
     * - 描述：普通牌组取 `Deck.Normal.description`（`markdownDescription` 时走后端
     *   `renderMarkdown`，对照 `overview.py:217-219`）；筛选牌组给一段固定说明；
     * - 有没有埋藏的卡片：`congratsInfo` 的 `haveSchedBuried` / `haveUserBuried`
     *   （决定要不要显示「取消埋藏」）。
     */
    async function loadOverviewExtra(): Promise<void> {
        const node = overview;
        if (!node) {
            overviewDeck = null;
            overviewDescription = "";
            haveBuried = false;
            haveSchedBuried = false;
            haveUserBuried = false;
            return;
        }
        const deck = await getDeck({ did: node.deckId });
        overviewDeck = deck;
        const normal = deck.kind.case === "normal" ? deck.kind.value : undefined;
        if (deck.kind.case === "filtered") {
            // 对照 `overview.py:212-215`
            overviewDescription = [
                tr.studyingThisIsASpecialDeckFor(),
                tr.studyingCardsWillBeAutomaticallyReturnedTo(),
                tr.studyingDeletingThisDeckFromTheDeck(),
            ].join(" ");
        } else if (normal?.description) {
            overviewDescription = normal.markdownDescription
                ? (await renderMarkdown({ markdown: normal.description, sanitize: true })).val
                : normal.description;
        } else {
            overviewDescription = "";
        }
        const info = await congratsInfo(new Empty({}));
        haveSchedBuried = info.haveSchedBuried;
        haveUserBuried = info.haveUserBuried;
        haveBuried = haveSchedBuried || haveUserBuried;
    }

    /** 概览「选项」（`overview.py:284-285` 的 `O`）。 */
    function openOverviewOptions(): void {
        if (overview) {
            bridgeCommand(`opts:${overview.deckId}`);
        }
    }

    /** 筛选牌组：重建（`overview.py:138-141`，快捷键 `r`）。 */
    async function rebuildCurrentFilteredDeck(): Promise<void> {
        const node = overview;
        if (!node || !overviewFiltered) {
            return;
        }
        busy = true;
        try {
            await rebuildFilteredDeck({ did: node.deckId });
            await reload();
        } finally {
            busy = false;
        }
    }

    /** 筛选牌组：清空（`overview.py:143-146`，快捷键 `e`）。 */
    async function emptyCurrentFilteredDeck(): Promise<void> {
        const node = overview;
        if (!node || !overviewFiltered) {
            return;
        }
        busy = true;
        try {
            await emptyFilteredDeck({ did: node.deckId });
            await reload();
        } finally {
            busy = false;
        }
    }

    /**
     * 取消埋藏（`overview.py:152-175` 的 `on_unbury`）：两种埋藏都有时先问一句
     * 「手动埋藏 / 埋藏的兄弟 / 全部」，否则直接 ALL。
     */
    function askUnbury(): void {
        if (!overview) {
            return;
        }
        if (haveSchedBuried && haveUserBuried) {
            unburyModal.show();
            return;
        }
        void unbury(UnburyDeckRequest_Mode.ALL);
    }

    async function unbury(mode: UnburyDeckRequest_Mode): Promise<void> {
        const node = overview;
        if (!node) {
            return;
        }
        busy = true;
        try {
            await unburyDeck({ deckId: node.deckId, mode });
            await reload();
        } finally {
            busy = false;
        }
    }

    /** 「描述」按钮（`overview.py:277-278` 的 `DeckDescriptionDialog`）。 */
    function askDescription(): void {
        const deck = overviewDeck;
        if (!deck || deck.kind.case !== "normal") {
            return;
        }
        descriptionText = deck.kind.value.description;
        descriptionMarkdown = deck.kind.value.markdownDescription;
        descriptionModal.show();
    }

    /** 保存描述：整份 `Deck` 写回（上游 `update_deck_dict`，`deckdescription.py:69-75`）。 */
    async function saveDescription(): Promise<void> {
        const deck = overviewDeck;
        if (!deck || deck.kind.case !== "normal") {
            return;
        }
        deck.kind.value.description = descriptionText;
        deck.kind.value.markdownDescription = descriptionMarkdown;
        busy = true;
        try {
            await updateDeck(deck);
            await reload();
        } finally {
            busy = false;
        }
    }

    // 底部按钮 ################################################################

    function askCreate(): void {
        textMode = "create";
        textValue = "";
        textModal.show();
    }

    async function applyText(): Promise<void> {
        const name = textValue.trim();
        if (!name) {
            return;
        }
        busy = true;
        try {
            if (textMode === "create") {
                // `Deck.kind` 是必填的 oneof，缺了后端会报 "missing kind"
                const changes = await addDeck(new Deck({
                    name,
                    kind: { case: "normal", value: {} },
                }));
                // 「学习牌组 → 添加」：新建完直接进它的概览（上游 `StudyDeck.onAddDeck` 的 success）
                if (createThenStudy) {
                    createThenStudy = false;
                    await studyDeckById(changes.id);
                    return;
                }
            } else if (target) {
                await renameDeck(new RenameDeckRequest({ deckId: target.deckId, newName: name }));
            }
            await reload();
        } catch (problem) {
            console.error("牌组操作失败", problem);
        } finally {
            busy = false;
        }
    }

    function askDelete(node: DeckTreeNode): void {
        target = node;
        confirmModal.show();
    }

    async function applyDelete(): Promise<void> {
        if (!target) {
            return;
        }
        busy = true;
        try {
            await removeDecks(new DeckIds({ dids: [target.deckId] }));
            await reload();
        } catch (problem) {
            console.error("删除牌组失败", problem);
        } finally {
            busy = false;
        }
    }

    // 齿轮菜单 ################################################################

    /**
     * 齿轮 → 导出：导出**这个牌组**（`deckbrowser.py:326-327` 的 `_export` →
     * `main.py:1398-1401` 的 `onExport(did=…)`）。
     *
     * 对话框与导出逻辑跟浏览器「更多」→ 导出共用（`ExportDialog.svelte` +
     * `exporting.ts`），区别只是范围默认「该牌组」、建议文件名用牌组名。
     */
    async function runDeckExport(choice: ExportChoice): Promise<void> {
        const deck = exportDeck;
        if (!deck) {
            return;
        }
        busy = true;
        try {
            const message = await performExport(choice, { deck });
            if (message) {
                exportStatus = message;
            }
        } catch (problem) {
            console.error("导出失败", problem);
            exportStatus = String(problem);
        } finally {
            busy = false;
        }
    }

    function openGear(node: DeckTreeNode, event: MouseEvent): void {
        event.preventDefault();
        event.stopPropagation();
        gear = { x: event.clientX, y: event.clientY, node };
    }

    /** 齿轮菜单的项：重命名 / 选项 / 删除（`deckbrowser.py:309-324`）。 */
    function gearSections(): MenuSection[] {
        if (!gear) {
            return [];
        }
        const node = gear.node;
        return [
            {
                items: [
                    {
                        label: tr.actionsRename(),
                        action: "rename",
                        run: () => {
                            target = node;
                            textMode = "rename";
                            textValue = node.name.split("::").at(-1) ?? node.name;
                            textModal.show();
                        },
                    },
                    {
                        label: tr.actionsOptions(),
                        action: "options",
                        run: () => bridgeCommand(`opts:${node.deckId}`),
                    },
                    {
                        label: tr.actionsExport(),
                        action: "export",
                        run: () => {
                            exportStatus = "";
                            exportDeck = { id: node.deckId, name: node.name };
                            exportDialog.show({ deck: exportDeck });
                        },
                    },
                    {
                        label: tr.actionsDelete(),
                        action: "delete",
                        disabled: node.deckId === 1n,
                        run: () => askDelete(node),
                    },
                ],
            },
        ];
    }

    // 键盘 ####################################################################

    /** Qt 主窗口的全局快捷键（`main.py:1189-1198`）在这里同样可用。 */
    function onKeydown(event: KeyboardEvent): void {
        const target = event.target as HTMLElement | null;
        if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) {
            return;
        }
        // 视图 → 全屏（`main.py:1472-1476`：F11 / 标准全屏键，作用域是整个应用）
        if (event.key === "F11") {
            event.preventDefault();
            bridgeCommand("fullscreen");
            return;
        }
        // 工具 → 创建筛选牌组（`forms/main.ui` 的 `actionCreateFiltered`，快捷键 `F`）
        if (event.key.toLowerCase() === "f" && !event.ctrlKey && !event.metaKey && !event.altKey) {
            event.preventDefault();
            void askFilteredDeck();
            return;
        }
        // 工具栏「同步」的快捷键 `Y`（`toolbar.py:412`）
        if (event.key.toLowerCase() === "y" && !event.ctrlKey && !event.metaKey && !event.altKey) {
            event.preventDefault();
            askSync();
            return;
        }
        if (event.ctrlKey || event.metaKey || event.altKey) {
            // 菜单栏的加速键（对照 `forms/main.ui`）：导出 `Ctrl+E`（1.4 的那条）、
            // 撤销/重做 `Ctrl+Z` / `Ctrl+Shift+Z`、首选项 `Ctrl+P`、缩放 `Ctrl+=`/`-`/`0`
            const key = event.key.toLowerCase();
            if (key === "e") {
                event.preventDefault();
                exportCurrentDeck();
            } else if (key === "z") {
                event.preventDefault();
                void undoLast(event.shiftKey ? "redo" : "undo");
            } else if (key === "p") {
                event.preventDefault();
                bridgeCommand("preferences");
            } else if (key === "=" || key === "+") {
                event.preventDefault();
                zoomView("in");
            } else if (key === "-") {
                event.preventDefault();
                zoomView("out");
            } else if (key === "0") {
                event.preventDefault();
                zoomView("reset");
            }
            return;
        }
        switch (event.key.toLowerCase()) {
            case "a":
                event.preventDefault();
                bridgeCommand("add");
                break;
            case "b":
                event.preventDefault();
                bridgeCommand("browse");
                break;
            case "t":
                event.preventDefault();
                bridgeCommand("stats");
                break;
            case "s":
                // Qt：牌组列表里按 `s` → 进概览；概览里按 `s` → 开始复习
                event.preventDefault();
                if (overview) {
                    void study();
                } else if (currentDeckId) {
                    window.location.href = `/decks/?deck=${currentDeckId}`;
                }
                break;
            case "c":
                // Qt：概览里按 `c` 打开自定义学习（`overview.py:126-133`，筛选牌组时不响应）
                if (overview && !overviewFiltered) {
                    event.preventDefault();
                    bridgeCommand(`customStudy:${overview.deckId}`);
                }
                break;
            case "o":
                // 概览的固定快捷键（`overview.py:126-133`）
                if (overview) {
                    event.preventDefault();
                    openOverviewOptions();
                }
                break;
            case "r":
                if (overview && overviewFiltered) {
                    event.preventDefault();
                    void rebuildCurrentFilteredDeck();
                }
                break;
            case "e":
                if (overview && overviewFiltered) {
                    event.preventDefault();
                    void emptyCurrentFilteredDeck();
                }
                break;
            case "u":
                if (overview && haveBuried) {
                    event.preventDefault();
                    askUnbury();
                }
                break;
        }
    }

    onMount(async () => {
        await setupI18n({
            modules: [
                ModuleName.DECKS,
                ModuleName.ACTIONS,
                ModuleName.STATISTICS,
                ModuleName.STUDYING,
                ModuleName.SCHEDULING,
                // 齿轮里的「导出」用的是 `ExportDialog.svelte`（`exporting-*` 文案）
                ModuleName.EXPORTING,
                // 概览的描述编辑弹框（`deck-config-description-new-handling2`）
                ModuleName.DECK_CONFIG,
                // 菜单栏：撤销/重做（`undo-undo` / `undo-redo`）与检查数据库的结果文案
                ModuleName.UNDO,
                ModuleName.DATABASE_CHECK,
                // 「空卡片」没有空卡片时的提示（`empty-cards-not-found`）
                ModuleName.EMPTY_CARDS,
                // 「关于」弹框（`about-about-anki` / `about-version` / 许可说明，都在 ftl/qt）
                ModuleName.ABOUT,
                // 工具栏的「浏览」「统计」住在 ftl/qt/qt-misc.ftl，没有类型化函数
                ModuleName.QT_MISC,
                // 同步（`sync-*`，核心模块）
                ModuleName.SYNC,
            ],
        });
        await reload();
        await loadSyncState();
        // 概览面板：`?deck=<id>` 或 `?deck=current`（宿主 `s` 用后者）
        const wanted = new URLSearchParams(window.location.search).get("deck");
        if (wanted) {
            const id = wanted === "current" ? Number(currentDeckId) : Number(wanted);
            overview = nodes.get(id) ?? null;
            await loadOverviewExtra();
        }
    });
</script>

<svelte:window on:keydown={onKeydown} on:click={() => (openMenu = null)} />

<div class="decks">
    <!-- 主窗口菜单栏（对照 Qt 主窗口的 `menuCol` / `menuEdit` / `menuTools` / `menuHelp`） -->
    <div class="menubar">
        {#each menuBar() as menu (menu.label)}
            <div class="menu">
                <button
                    class="menu-title"
                    class:open={openMenu === menu.label}
                    data-menu-title={menu.label}
                    on:click|stopPropagation={() =>
                        (openMenu = openMenu === menu.label ? null : menu.label)}
                    >{menu.label}</button
                >
                {#if openMenu === menu.label}
                    <div class="menu-popup">
                        {#each menu.items as item (item.action)}
                            <button
                                class="menu-entry"
                                data-menu={item.action}
                                on:click|stopPropagation={() => {
                                    openMenu = null;
                                    item.run();
                                }}
                            >
                                <span>{item.label}</span>
                                {#if item.shortcut}
                                    <span class="shortcut">{item.shortcut}</span>
                                {/if}
                            </button>
                        {/each}
                    </div>
                {/if}
            </div>
        {/each}
    </div>

    <!-- 顶部工具栏（对照 `toolbar.py:353-389`；同步按钮没做） -->
    <div class="toolbar">
        <button
            class="link current"
            title={tr.actionsShortcutKey({ val: "D" })}
            aria-label={tr.actionsDecks()}>{tr.actionsDecks()}</button
        >
        <button class="link" title={tr.actionsShortcutKey({ val: "A" })} on:click={() => bridgeCommand("add")}
            >{tr.actionsAdd()}</button
        >
        <button
            class="link"
            title={tr.actionsShortcutKey({ val: "B" })}
            on:click={() => bridgeCommand("browse")}>{translateRaw("qt-misc-browse")}</button
        >
        <button
            class="link"
            title={tr.actionsShortcutKey({ val: "T" })}
            on:click={() => bridgeCommand("stats")}>{translateRaw("qt-misc-stats")}</button
        >
        <!-- 同步（`toolbar.py:410-419`）：`Y` 键与点击等价，`data-sync-state` 供界面/检查读 -->
        <button
            class="link sync"
            id="sync"
            data-sync-state={syncState}
            title={tr.actionsShortcutKey({ val: "Y" })}
            aria-label={translateRaw("qt-misc-sync")}
            on:click={askSync}>{translateRaw("qt-misc-sync")}</button
        >
    </div>

    <!-- 同步的状态/结果（上游是工具栏变色 + 弹出的对话框） -->
    {#if syncMessage}
        <div class="sync-status" id="sync-status" data-sync-state={syncState}>{syncMessage}</div>
    {/if}

    {#if overview}
        <!-- 概览（Qt 的 Overview 页面） -->
        <div class="overview">
            <h3>{overview.name}</h3>
            <table class="counts">
                <thead>
                    <tr>
                        <th>{tr.actionsNew()}</th>
                        <th>{tr.schedulingLearning()}</th>
                        <th>{tr.studyingToReview()}</th>
                    </tr>
                </thead>
                <tbody>
                    <tr>
                        <td class={overview.newCount === 0 ? "zero" : ""}>{overview.newCount}</td>
                        <td class={overview.learnCount === 0 ? "zero" : ""}>{overview.learnCount}</td>
                        <td class={overview.reviewCount === 0 ? "zero" : ""}>
                            {overview.reviewCount}
                        </td>
                    </tr>
                </tbody>
            </table>
            <div class="overview-buttons">
                <button
                    class="btn btn-primary"
                    id="deck-study"
                    disabled={busy}
                    on:click={() => void study()}>{tr.studyingStudyNow()}</button
                >
                <button
                    class="btn btn-secondary"
                    id="deck-options"
                    title={tr.actionsShortcutKey({ val: "O" })}
                    on:click={openOverviewOptions}>{tr.actionsOptions()}</button
                >
                {#if overviewFiltered}
                    <!-- 筛选牌组：重建 / 清空（`overview.py:287-290`） -->
                    <button
                        class="btn btn-secondary"
                        id="deck-rebuild"
                        title={tr.actionsShortcutKey({ val: "R" })}
                        on:click={() => void rebuildCurrentFilteredDeck()}
                        >{tr.actionsRebuild()}</button
                    >
                    <button
                        class="btn btn-secondary"
                        id="deck-empty"
                        title={tr.actionsShortcutKey({ val: "E" })}
                        on:click={() => void emptyCurrentFilteredDeck()}
                        >{tr.studyingEmpty()}</button
                    >
                {:else}
                    <!-- 对照 Qt 概览底部的「C 自定义学习」（`overview.py:284-292`） -->
                    <button
                        class="btn btn-secondary"
                        id="deck-custom-study"
                        title={tr.actionsShortcutKey({ val: "C" })}
                        on:click={() => bridgeCommand(`customStudy:${overview?.deckId}`)}
                        >{tr.actionsCustomStudy()}</button
                    >
                {/if}
                {#if haveBuried}
                    <!-- 取消埋藏（`overview.py:294-295`） -->
                    <button
                        class="btn btn-secondary"
                        id="deck-unbury"
                        title={tr.actionsShortcutKey({ val: "U" })}
                        on:click={askUnbury}>{tr.studyingUnbury()}</button
                    >
                {/if}
                {#if !overviewFiltered}
                    <!-- 编辑描述（`overview.py:296-297` 的「描述」链接） -->
                    <button class="btn btn-secondary" id="deck-description" on:click={askDescription}
                        >{tr.schedulingDescription()}</button
                    >
                {/if}
                <button class="btn btn-secondary" on:click={() => (window.location.href = "/decks/")}
                    >{tr.actionsDecks()}</button
                >
            </div>
            {#if overviewDescription}
                <!-- 牌组描述（上游用 `stdHtml` 直接插 HTML；这里的 CSP 是不可信页面的
                     那套：内联脚本/事件处理器都被 script-src 挡住） -->
                <div class="description" id="deck-description-text">
                    {@html overviewDescription}
                </div>
            {/if}
        </div>
    {/if}

    <table class="list">
        <thead>
            <tr>
                <th class="name">{tr.decksDeck()}</th>
                <th>{tr.actionsNew()}</th>
                <th>{tr.decksLearnHeader()}</th>
                <th>{tr.decksReviewHeader()}</th>
                <th class="gear"></th>
            </tr>
        </thead>
        <tbody>
            <!-- 拖到这里 = 移到顶层（`deckbrowser.py:303-304` + `deckbrowser.ts:21-24`） -->
            <tr
                class="top-level-drag-row"
                class:drag-hover={dropTarget === 0n}
                on:dragover={(event) => onDragOver(0n, event)}
                on:dragleave={() => onDragLeave(0n)}
                on:drop={(event) => void onDrop(0n, event)}
            >
                <td colspan="5"></td>
            </tr>
            {#each rows as row (row.node.deckId)}
                <tr
                    class:current={row.node.deckId === currentDeckId}
                    class:drag-hover={dropTarget === row.node.deckId}
                    data-deck={row.node.deckId}
                    draggable="true"
                    on:dragstart={(event) => onDragStart(row.node, event)}
                    on:dragover={(event) => onDragOver(row.node.deckId, event)}
                    on:dragleave={() => onDragLeave(row.node.deckId)}
                    on:drop={(event) => void onDrop(row.node.deckId, event)}
                    on:dragend={onDragEnd}
                    on:click={(event) =>
                        event.shiftKey ? void selectDeck(row.node) : void openDeck(row.node)}
                >
                    <td class="name">
                        <span class="indent" style="width: {row.depth * 18}px"></span>
                        {#if row.node.children.length > 0}
                            <button
                                class="collapse"
                                on:click|stopPropagation={() => void toggleCollapse(row.node)}
                                >{row.node.collapsed ? "▶" : "▼"}</button
                            >
                        {:else}
                            <span class="collapse-placeholder"></span>
                        {/if}
                        <a
                            class="deck-name"
                            href="?deck={row.node.deckId}"
                            on:click|preventDefault|stopPropagation={() => void openDeck(row.node)}
                            >{row.node.name}</a
                        >
                    </td>
                    <td class:zero={row.node.newCount === 0}>{row.node.newCount}</td>
                    <td class:zero={row.node.learnCount === 0}>{row.node.learnCount}</td>
                    <td class:zero={row.node.reviewCount === 0}>{row.node.reviewCount}</td>
                    <td class="gear">
                        <button
                            class="gear-button"
                            data-gear={row.node.deckId}
                            aria-label={tr.actionsOptions()}
                            on:click={(event) => openGear(row.node, event)}>⚙</button
                        >
                    </td>
                </tr>
            {/each}
        </tbody>
    </table>

    <!-- 底部一行：今日学习（对照 `deckbrowser.py:207-210`） -->
    {#if studied}
        <div class="studied" id="studied-today">{@html studied}</div>
    {/if}

    <!-- 导出之后的提示（上游是 tooltip） -->
    {#if exportStatus}
        <div class="export-status" id="export-status">{exportStatus}</div>
    {/if}

    <!-- 拖拽失败（只有"目标必须是叶子节点"这一种） -->
    {#if dragStatus}
        <div class="export-status" id="deck-drag-error">{dragStatus}</div>
    {/if}

    <!-- 底部按钮（对照 `deckbrowser.py:382-394`） -->
    <div class="bottom">
        <button class="btn btn-secondary" id="deck-shared" on:click={() => bridgeCommand("shared")}
            >{tr.decksGetShared()}</button
        >
        <button class="btn btn-secondary" id="deck-create" on:click={askCreate}
            >{tr.decksCreateDeck()}</button
        >
        <button class="btn btn-secondary" id="deck-import" on:click={() => bridgeCommand("import")}
            >{tr.decksImportFile()}</button
        >
    </div>
</div>

{#if gear}
    <ContextMenu
        x={gear.x}
        y={gear.y}
        sections={gearSections()}
        onclose={() => (gear = null)}
    />
{/if}

<!-- 齿轮 → 导出（与浏览器那边共用同一个对话框与 `exporting.ts`） -->
<ExportDialog bind:this={exportDialog} onExport={(choice) => void runDeckExport(choice)} />

<!-- 工具 → 学习牌组（`aqt/forms/studydeck.ui`：筛选框 + 名字列表 + 添加 / 学习） -->
<Modal bind:this={studyModal} modalKey="study-deck" onOkClicked={() => void studyChosenDeck()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.decksStudyDeck()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label class="col-form-label" for="study-deck-filter">{tr.decksFilter()}</label>
        <input id="study-deck-filter" class="form-control" bind:value={studyFilter} />
        <div class="study-deck-list" id="study-deck-list">
            {#each studyMatching as name (name)}
                <button
                    type="button"
                    class="study-deck-entry"
                    class:selected={name === studyChoice}
                    data-deck-name={name}
                    on:click={() => (studyChoice = name)}
                    on:dblclick={() => void studyChosenDeck()}>{name}</button
                >
            {/each}
        </div>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" id="study-deck-add" on:click={askCreateThenStudy}
            >{tr.actionsAdd()}</button
        >
        <button
            type="button"
            class="btn btn-primary"
            id="study-deck-study"
            disabled={!studyChoice}
            on:click={() => void studyChosenDeck()}>{tr.decksStudy()}</button
        >
    </div>
</Modal>

<!-- 同步登录（对照 `aqt/sync.py` 的 `SyncLoginDialog`：邮箱 + 密码 + 自定义服务器） -->
<Modal bind:this={syncModal} modalKey="sync-login" onOkClicked={() => void doSyncLogin()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{translateRaw("qt-misc-sync")}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label class="col-form-label" for="sync-user">{tr.syncAnkiwebIdLabel()}</label>
        <input id="sync-user" class="form-control" bind:value={syncUser} />
        <label class="col-form-label" for="sync-password">{tr.syncPasswordLabel()}</label>
        <input id="sync-password" class="form-control" type="password" bind:value={syncPassword} />
        <label class="col-form-label" for="sync-endpoint"
            >{translateRaw("qt-misc-custom-sync-server")}</label
        >
        <input id="sync-endpoint" class="form-control" bind:value={syncEndpoint} />
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={syncModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            type="button"
            class="btn btn-primary"
            id="sync-login"
            on:click={() => void doSyncLogin()}>{tr.syncLogInButton()}</button
        >
    </div>
</Modal>

<!-- 完整同步的二选一（对照 `sync.py` 的 `full_sync`：上传覆盖 / 下载覆盖） -->
<Modal bind:this={syncFullModal} modalKey="sync-full">
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.syncConflictExplanation()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <p id="sync-full-hint">{tr.syncConflictExplanation()}</p>
    </div>
    <div slot="footer" class="modal-footer">
        <button
            type="button"
            class="btn btn-secondary"
            id="sync-download"
            on:click={() => void startFullSync(false)}>{tr.syncDownloadFromAnkiweb()}</button
        >
        <button
            type="button"
            class="btn btn-primary"
            id="sync-upload"
            on:click={() => void startFullSync(true)}>{tr.syncUploadToAnkiweb()}</button
        >
    </div>
</Modal>

<!-- 工具 → 创建筛选牌组（`aqt/forms/filtered_deck.ui`；第二个筛选、预览间隔、
     浏览器搜索编辑器与 FSRS 专用排序都没做，见 README 的差异表） -->
<Modal bind:this={filteredModal} modalKey="filtered-deck" onOkClicked={() => void buildFilteredDeck()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{translateRaw("qt-misc-create-filtered-deck")}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label class="col-form-label" for="filtered-name">{tr.actionsName()}</label>
        <input id="filtered-name" class="form-control" bind:value={filteredName} />
        <label class="col-form-label" for="filtered-search">{tr.actionsFilter()}</label>
        <input id="filtered-search" class="form-control" bind:value={filteredSearch} />
        <label class="col-form-label" for="filtered-limit">{tr.decksLimitTo()}</label>
        <input
            id="filtered-limit"
            class="form-control"
            type="number"
            min="0"
            bind:value={filteredLimit}
        />
        <label class="col-form-label" for="filtered-order">{tr.decksCardsSelectedBy()}</label>
        <select id="filtered-order" class="form-select" bind:value={filteredOrder}>
            {#each filteredOrderChoices as choice (choice.order)}
                <option value={choice.order}>{choice.label}</option>
            {/each}
        </select>
        <div class="form-check">
            <input
                id="filtered-resched"
                class="form-check-input"
                type="checkbox"
                bind:checked={filteredResched}
            />
            <label class="form-check-label" for="filtered-resched"
                >{tr.decksRescheduleCardsBasedOnMyAnswers()}</label
            >
        </div>
        <div class="form-check">
            <input
                id="filtered-allow-empty"
                class="form-check-input"
                type="checkbox"
                bind:checked={filteredAllowEmpty}
            />
            <label class="form-check-label" for="filtered-allow-empty"
                >{translateRaw("decks_create_even_if_empty")}</label
            >
        </div>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={filteredModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            type="button"
            class="btn btn-primary"
            id="filtered-build"
            on:click={() => void buildFilteredDeck()}>{tr.decksBuild()}</button
        >
    </div>
</Modal>

<!-- 工具菜单的报告弹框（检查数据库 / 检查媒体 / 空卡片） -->
<Modal bind:this={reportModal} modalKey="deck-report" dialogClass="modal-lg">
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{reportTitle}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <div id="deck-report-body">{@html reportBody}</div>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={reportModal.cancelHandler}>
            {tr.actionsClose()}
        </button>
    </div>
</Modal>

<!-- 描述编辑（对照 `qt/aqt/deckdescription.py`：markdown 勾选 + 纯文本框 + 保存） -->
<Modal bind:this={descriptionModal} modalKey="deck-description" onOkClicked={() => void saveDescription()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.schedulingDescription()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <div class="form-check mb-2">
            <input
                class="form-check-input"
                type="checkbox"
                id="description-markdown"
                bind:checked={descriptionMarkdown}
            />
            <label
                class="form-check-label"
                for="description-markdown"
                title={tr.deckConfigDescriptionNewHandlingHint()}
                >{tr.deckConfigDescriptionNewHandling2()}</label
            >
        </div>
        <textarea
            id="description-text"
            class="form-control"
            rows="6"
            bind:value={descriptionText}
        ></textarea>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={descriptionModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            id="description-save"
            type="button"
            class="btn btn-primary"
            on:click={descriptionModal.acceptHandler}>{tr.actionsSave()}</button
        >
    </div>
</Modal>

<!-- 「要取消埋藏哪些卡片？」（对照 `overview.py:155-171` 的 askUserDialog） -->
<Modal bind:this={unburyModal} modalKey="deck-unbury">
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.studyingWhatWouldYouLikeToUnbury()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body unbury-choices">
        <button
            class="btn btn-secondary"
            id="unbury-user"
            on:click={() => {
                unburyModal.hide();
                void unbury(UnburyDeckRequest_Mode.USER_ONLY);
            }}>{tr.studyingManuallyBuriedCards()}</button
        >
        <button
            class="btn btn-secondary"
            id="unbury-sched"
            on:click={() => {
                unburyModal.hide();
                void unbury(UnburyDeckRequest_Mode.SCHED_ONLY);
            }}>{tr.studyingBuriedSiblings()}</button
        >
        <button
            class="btn btn-primary"
            id="unbury-all"
            on:click={() => {
                unburyModal.hide();
                void unbury(UnburyDeckRequest_Mode.ALL);
            }}>{tr.studyingAllBuriedCards()}</button
        >
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={unburyModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
    </div>
</Modal>

<!-- 新建/重命名共用这一个文本弹框（Qt 是两个 `getOnlyText` 对话框） -->
<Modal bind:this={textModal} modalKey="deck-text" onOkClicked={() => void applyText()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{textTitle}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <form on:submit|preventDefault={textModal.acceptHandler}>
            <label for="deck-name" class="col-form-label">{textPrompt}</label>
            <input id="deck-name" class="form-control" bind:value={textValue} />
        </form>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={textModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={textModal.acceptHandler}>
            {textMode === "create" ? tr.decksCreateDeck() : tr.actionsRename()}
        </button>
    </div>
</Modal>

<Modal bind:this={confirmModal} modalKey="deck-delete" onOkClicked={() => void applyDelete()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.actionsDelete()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <p>{target?.name ?? ""}</p>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={confirmModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            type="button"
            class="btn btn-danger"
            id="deck-delete-confirm"
            on:click={confirmModal.acceptHandler}>{tr.actionsDelete()}</button
        >
    </div>
</Modal>

<style>
    .decks {
        padding: 1rem;
    }

    .toolbar {
        display: flex;
        gap: 0.5rem;
        justify-content: center;
        padding-bottom: 0.5rem;
        margin-bottom: 1rem;
        border-bottom: 1px solid var(--border, #ddd);
    }

    .link {
        background: none;
        border: none;
        color: var(--fg-link, #0a58ca);
        cursor: pointer;
        font-size: 1rem;
    }

    .link.current {
        color: inherit;
        font-weight: 600;
        cursor: default;
    }

    table.list {
        width: 100%;
        border-collapse: collapse;
    }

    table.list th,
    table.list td {
        padding: 0.25rem 0.5rem;
        text-align: right;
        white-space: nowrap;
    }

    table.list th.name,
    table.list td.name {
        text-align: left;
        width: 100%;
    }

    .indent {
        display: inline-block;
    }

    /* 拖到这一条 = 移到顶层（`deckbrowser.py:303-304` 的 `top-level-drag-row`） */
    .top-level-drag-row td {
        height: 0.6rem;
        padding: 0;
    }

    .drag-hover td,
    .drag-hover {
        background: var(--fg-faint, #e6e6e6);
    }

    .collapse,
    .collapse-placeholder {
        display: inline-block;
        width: 1.2rem;
    }

    .collapse {
        background: none;
        border: none;
        cursor: pointer;
        color: inherit;
        font-size: 0.7rem;
    }

    .deck-name {
        color: var(--fg-link, #0a58ca);
        text-decoration: none;
    }

    tr.current td.name {
        font-weight: 700;
    }

    td.zero {
        color: var(--fg-faint, #999);
    }

    .gear-button {
        background: none;
        border: none;
        cursor: pointer;
        color: inherit;
        opacity: 0.6;
    }

    .studied {
        margin-top: 0.75rem;
        color: var(--fg-faint, #666);
        font-size: 0.9rem;
    }

    .export-status {
        margin-top: 0.5rem;
        color: var(--fg);
        font-size: 0.9rem;
    }

    /* 菜单栏（对照 Qt 主窗口的 QMenuBar） */
    .menubar {
        display: flex;
        gap: 0.25rem;
        padding: 0.25rem 0.5rem 0;
        font-size: 0.9rem;
    }

    .menu {
        position: relative;
    }

    .menu-title {
        padding: 0.15rem 0.5rem;
        border: none;
        border-radius: 3px;
        background: none;
        color: var(--fg);
        cursor: pointer;
    }

    .menu-title:hover,
    .menu-title.open {
        background: var(--canvas-inset, #e6e6e6);
    }

    .menu-popup {
        position: absolute;
        z-index: 1000;
        top: 100%;
        left: 0;
        min-width: 12rem;
        padding: 0.25rem 0;
        border: 1px solid var(--border, #ccc);
        border-radius: 4px;
        background: var(--canvas, #fff);
        box-shadow: 0 2px 8px rgb(0 0 0 / 20%);
    }

    .menu-entry {
        display: flex;
        justify-content: space-between;
        gap: 1.5rem;
        width: 100%;
        padding: 0.2rem 0.75rem;
        border: none;
        background: none;
        color: var(--fg);
        text-align: left;
        cursor: pointer;
    }

    .menu-entry:hover {
        background: var(--canvas-inset, #e6e6e6);
    }

    .shortcut {
        color: var(--fg-faint, #777);
    }

    /* 工具栏的同步链接（对照 `toolbar.py:410-419` 的变色与转圈） */
    .link.sync[data-sync-state="off"] {
        opacity: 0.6;
    }

    .link.sync[data-sync-state="syncing"] {
        color: var(--fg-link, #06c);
    }

    .link.sync[data-sync-state="error"] {
        color: var(--fg-error, #c00);
    }

    .sync-status {
        margin-top: 0.5rem;
        color: var(--fg);
        font-size: 0.9rem;
        white-space: pre-wrap;
    }

    /* 「学习牌组」的名字列表（对照 `studydeck.ui` 的 `QListWidget`） */
    .study-deck-list {
        max-height: 18rem;
        margin-top: 0.5rem;
        overflow-y: auto;
        border: 1px solid var(--border, #ccc);
        border-radius: 4px;
    }

    .study-deck-entry {
        display: block;
        width: 100%;
        padding: 0.2rem 0.5rem;
        border: none;
        background: none;
        color: var(--fg);
        text-align: left;
        cursor: pointer;
    }

    .study-deck-entry:hover,
    .study-deck-entry.selected {
        background: var(--canvas-inset, #e6e6e6);
    }

    .description {
        margin-top: 1rem;
        padding: 0.5rem 0.75rem;
        border: 1px solid var(--border, #ccc);
        border-radius: 4px;
        color: var(--fg);
        overflow-wrap: anywhere;
    }

    .unbury-choices {
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
    }

    .bottom {
        display: flex;
        gap: 0.5rem;
        margin-top: 1.5rem;
        padding-top: 0.75rem;
        border-top: 1px solid var(--border, #ddd);
    }

    .overview {
        margin-bottom: 1.5rem;
    }

    .overview table.counts {
        border-collapse: collapse;
        margin-bottom: 0.75rem;
    }

    .overview table.counts th,
    .overview table.counts td {
        padding: 0.25rem 1.5rem 0.25rem 0;
        text-align: right;
    }

    .overview table.counts th:first-child,
    .overview table.counts td:first-child {
        text-align: left;
    }

    .overview-buttons {
        display: flex;
        gap: 0.5rem;
    }
</style>
