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
- 齿轮 → `opts:<did>` → 重命名 / 选项 / 删除（`deckbrowser.py:293-296,309-324`；
  Qt 还有「导出」，本外壳没做，见 README 的差异表）；
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
    import * as tr from "@generated/ftl";
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import {
        addDeck,
        deckTree,
        getCurrentDeck,
        removeDecks,
        renameDeck,
        setCurrentDeck,
        setDeckCollapsed,
        studiedToday,
    } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { onMount } from "svelte";

    import Modal from "$lib/components/Modal.svelte";

    // 菜单是浏览器页面里那一个（行右键与工具栏「更多」共用）；它是通用组件，
    // 这里直接复用，省得再写一份定位/关闭逻辑
    import ContextMenu from "../browser/ContextMenu.svelte";
    import type { MenuSection } from "../browser/menu";

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
    /** 齿轮菜单的位置与目标牌组 */
    let gear: { x: number; y: number; node: DeckTreeNode } | null = null;

    let textModal: Modal;
    let confirmModal: Modal;
    /** 文本弹框在做什么：新建牌组 / 重命名 */
    let textMode: "create" | "rename" = "create";
    let textValue = "";
    /** 重命名与删除的目标牌组 */
    let target: DeckTreeNode | null = null;

    $: textTitle = textMode === "create" ? tr.decksCreateDeck() : tr.actionsRename();
    $: textPrompt = textMode === "create" ? tr.decksNewDeckName() : tr.actionsRename();

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

    /** 概览里的「学习」：开始复习当前牌组（宿主切到复习页）。 */
    async function study(): Promise<void> {
        bridgeCommand("study");
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
                // Qt：概览里按 `c` 打开自定义学习（`overview.py:126-133`）
                if (overview) {
                    event.preventDefault();
                    bridgeCommand(`customStudy:${overview.deckId}`);
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
                    on:click={() => bridgeCommand(`opts:${overview?.deckId}`)}
                    >{tr.actionsOptions()}</button
                >
                <!-- 对照 Qt 概览底部的「C 自定义学习」（`overview.py:284-292`） -->
                <button
                    class="btn btn-secondary"
                    id="deck-custom-study"
                    on:click={() => bridgeCommand(`customStudy:${overview?.deckId}`)}
                    >{tr.actionsCustomStudy()}</button
                >
                <button class="btn btn-secondary" on:click={() => (window.location.href = "/decks/")}
                    >{tr.actionsDecks()}</button
                >
            </div>
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
            {#each rows as row (row.node.deckId)}
                <tr
                    class:current={row.node.deckId === currentDeckId}
                    data-deck={row.node.deckId}
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
