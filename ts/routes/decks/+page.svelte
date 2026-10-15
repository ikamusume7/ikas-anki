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
        RenameDeckRequest,
    } from "@generated/anki/decks_pb";
    import { Empty } from "@generated/anki/generic_pb";
    import { UnburyDeckRequest_Mode } from "@generated/anki/scheduler_pb";
    import * as tr from "@generated/ftl";
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import {
        addDeck,
        congratsInfo,
        deckTree,
        emptyFilteredDeck,
        getCurrentDeck,
        getDeck,
        rebuildFilteredDeck,
        removeDecks,
        renameDeck,
        renderMarkdown,
        reparentDecks,
        setCurrentDeck,
        setDeckCollapsed,
        studiedToday,
        unburyDeck,
        updateDeck,
    } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
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
                await addDeck(new Deck({ name, kind: { case: "normal", value: {} } }));
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
        if (event.ctrlKey || event.metaKey || event.altKey) {
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
                // 工具栏的「浏览」「统计」住在 ftl/qt/qt-misc.ftl，没有类型化函数
                ModuleName.QT_MISC,
            ],
        });
        await reload();
        // 概览面板：`?deck=<id>` 或 `?deck=current`（宿主 `s` 用后者）
        const wanted = new URLSearchParams(window.location.search).get("deck");
        if (wanted) {
            const id = wanted === "current" ? Number(currentDeckId) : Number(wanted);
            overview = nodes.get(id) ?? null;
            await loadOverviewExtra();
        }
    });
</script>

<svelte:window on:keydown={onKeydown} />

<div class="decks">
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
    </div>

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
