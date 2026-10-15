<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->

<!--
「管理笔记类型」窗口（对照 `qt/aqt/models.py` 的 `Models` 对话框 + `AddModel`）。

上游有两个入口：主窗口菜单「管理笔记类型」（`main.py:1357` → `Models(mw, mw, fromMain=True)`）
与浏览器侧栏 Note Types 节点的右键菜单（`browser/sidebar/tree.py:924-928`）。本外壳还没有
菜单栏（PARITY 1.2），所以入口挂在浏览器窗口的「更多」菜单里（`bridgeCommand("notetypes")`）。

对话内容（`models.py:91-160`）：

- 列表每项就是「名字 [笔记数]」（`models.py:157-158`，文案 `browsing-note-count`），
  数据来自 `getNotetypeNamesAndCounts`（`NotetypeNameIdUseCount`：id / name / use_count）；
- 按钮：添加（`actions-add`）/ 重命名（`actions-rename`）/ 删除（`actions-delete`）/
  字段…（`notetypes-fields`）/ 卡片…（`notetypes-cards`）—— Qt 只在 `fromMain=True` 时
  给后两个按钮，本外壳两个窗口都有，就都给；
- 「添加」分两步（`models.py:166-187` + `AddModel`）：先选**原厂类型**（6 种，
  文案 `notetypes-add`）或**克隆现有类型**（`notetypes-clone`），再问名字（`actions-name`），
  最后走 `addNotetypeLegacy`（与 Qt 的 `add_notetype_legacy` 同一条路）；
- 「删除」的守卫与确认文案照抄 `models.py:189-198`：少于两个类型时只提示
  `notetypes-please-add-another-note-type-first`；有笔记在用 → 「连同卡片一起删」，
  否则「删掉这个没人用的类型」；
- 「重命名」弹 `actions-new-name`（默认填当前名字）→ `getNotetype` + `updateNotetype`；
- 双击列表项 = 重命名（`models.py:116`）。

与 Qt 的差异：Qt 的「选项」按钮开的是老的 `ModelAdvanced` 对话框（里面就是字段/卡片两块），
本外壳的「字段…」「卡片…」已经覆盖，所以没做「选项」。
-->

<script lang="ts">
    import { onMount } from "svelte";

    import {
        addOrUpdateNotetype,
        getNotetype,
        getNotetypeLegacy,
        getNotetypeNamesAndCounts,
        getStockNotetypeLegacy,
        removeNotetype,
        updateNotetype,
    } from "@generated/backend";
    import * as tr from "@generated/ftl";
    import { Empty } from "@generated/anki/generic_pb";
    import {
        NotetypeId,
        StockNotetype,
        StockNotetype_Kind as Kind,
        type NotetypeNameIdUseCount,
    } from "@generated/anki/notetypes_pb";
    import { postProto } from "@generated/post";
    import { ModuleName, setupI18n } from "@tslib/i18n";

    import Modal from "$lib/components/Modal.svelte";

    /** 原厂类型的顺序与种类照 `pylib/anki/stdmodels.py:35-42`（6 种） */
    const stockKinds = [
        Kind.BASIC,
        Kind.BASIC_AND_REVERSED,
        Kind.BASIC_OPTIONAL_REVERSED,
        Kind.BASIC_TYPING,
        Kind.CLOZE,
        Kind.IMAGE_OCCLUSION,
    ];

    let rows: NotetypeNameIdUseCount[] = [];
    let selected: bigint | null = null;
    let busy = false;
    let error = "";

    /** 「添加」第一步的下拉项：原厂类型（`kind`）或克隆现有类型（`clone`） */
    let addChoices: { kind: number; clone?: bigint; label: string }[] = [];
    let addChoice = 0;

    let addModal: Modal;
    let textModal: Modal;
    /** 文本框弹框在干什么：添加时问名字 / 重命名 */
    let textMode: "add" | "rename" = "add";
    let textTitle = "";
    let textValue = "";

    $: selectedRow = rows.find((row) => row.id === selected) ?? null;

    async function reload(): Promise<void> {
        rows = (await getNotetypeNamesAndCounts(new Empty({}))).entries;
        if (selected != null && !rows.some((row) => row.id === selected)) {
            selected = null;
        }
    }

    onMount(async () => {
        // 模块要按本页面用到的文案来选，漏了哪个按钮就会显示 `missing-key: xxx`
        await setupI18n({
            modules: [ModuleName.NOTETYPES, ModuleName.ACTIONS, ModuleName.BROWSING],
        });
        await reload();
    });

    /** 读一份 legacy JSON（原厂类型 / 现有类型都用它，与 Qt 走同一条路） */
    async function legacyJson(kind: number, clone?: bigint): Promise<Record<string, unknown>> {
        const json =
            clone != null
                ? await getNotetypeLegacy(new NotetypeId({ ntid: clone }))
                : await getStockNotetypeLegacy(new StockNotetype({ kind }));
        return JSON.parse(new TextDecoder().decode(json.json));
    }

    async function openAdd(): Promise<void> {
        error = "";
        addChoices = [];
        // 注意：Svelte 只认**赋值**（`push` 不会触发更新，见 TAURI_HANDOVER §5.25），
        // 所以先在本地数组里拼好，最后一次性赋值
        const choices: typeof addChoices = [];
        for (const kind of stockKinds) {
            const parsed = await legacyJson(kind);
            choices.push({ kind, label: tr.notetypesAdd({ val: String(parsed.name) }) });
        }
        for (const row of rows) {
            choices.push({ clone: row.id, kind: -1, label: tr.notetypesClone({ val: row.name }) });
        }
        addChoices = choices;
        addChoice = 0;
        addModal.show();
    }

    /** 「添加」第二步：问名字（默认填选中那项的名字，与 Qt 的 `getText(actions-name)` 一致） */
    async function askAddName(): Promise<void> {
        const choice = addChoices[addChoice];
        const parsed = await legacyJson(choice.kind, choice.clone);
        addModal.hide();
        textMode = "add";
        textTitle = tr.actionsName();
        textValue = String(parsed.name);
        textModal.show();
    }

    function askRename(): void {
        if (!selectedRow) {
            return;
        }
        textMode = "rename";
        textTitle = tr.actionsNewName();
        textValue = selectedRow.name;
        textModal.show();
    }

    async function applyText(): Promise<void> {
        const name = textValue.trim();
        if (!name) {
            textModal.hide();
            return;
        }
        busy = true;
        error = "";
        try {
            if (textMode === "rename" && selected != null) {
                const notetype = await getNotetype(new NotetypeId({ ntid: selected }));
                notetype.name = name;
                await updateNotetype(notetype);
            } else {
                const choice = addChoices[addChoice];
                const parsed = await legacyJson(choice.kind, choice.clone);
                parsed.name = name;
                // 新建：把 id 置 0，后端就按「新增」处理
                // （`rslib/src/notetype/service.rs:64-77` 的 `add_or_update_notetype`：
                // `if nt.id.0 == 0 { add_notetype }`）。注意 **不能**删字段、也不能给
                // `null` —— `NotetypeSchema11.id` 是必填的数字（原厂 JSON 里干脆没有
                // 这个字段，所以两种错法分别报 `missing field id` 与 `StringOrInt`）
                parsed.id = 0;
                await addOrUpdateNotetype({
                    json: new TextEncoder().encode(JSON.stringify(parsed)),
                    preserveUsnAndMtime: false,
                    skipChecks: false,
                });
            }
            textModal.hide();
            await reload();
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    async function remove(): Promise<void> {
        if (!selectedRow) {
            return;
        }
        // 对照 `models.py:190-192`：只剩一个类型时不让删
        if (rows.length < 2) {
            alert(tr.notetypesPleaseAddAnotherNoteTypeFirst());
            return;
        }
        const message =
            selectedRow.useCount > 0
                ? tr.notetypesDeleteThisNoteTypeAndAll()
                : tr.notetypesDeleteThisUnusedNoteType();
        if (!confirm(message)) {
            return;
        }
        busy = true;
        error = "";
        try {
            await removeNotetype(new NotetypeId({ ntid: selectedRow.id }));
            selected = null;
            await reload();
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    /** 「字段…」「卡片…」：宿主没有窗口能力，照 `openFieldsDialog` 的样式各开一条端点 */
    async function openFieldsDialog(): Promise<void> {
        if (selectedRow) {
            await postProto(
                "openFieldsDialogForNotetype",
                new NotetypeId({ ntid: selectedRow.id }),
                Empty,
            );
        }
    }

    async function openCardsDialog(): Promise<void> {
        if (selectedRow) {
            await postProto(
                "openCardsDialogForNotetype",
                new NotetypeId({ ntid: selectedRow.id }),
                Empty,
            );
        }
    }

    function onKeydown(event: KeyboardEvent): void {
        // F2 重命名、Delete 删除（Qt 的列表也吃键盘，这里给两个常用键）
        if (event.key === "F2") {
            event.preventDefault();
            askRename();
        } else if (event.key === "Delete" && !(event.target instanceof HTMLInputElement)) {
            event.preventDefault();
            void remove();
        }
    }
</script>

<svelte:window on:keydown={onKeydown} />

<div class="notetypes">
    <ul class="list" id="nt-list" role="listbox" aria-label={tr.notetypesNoteTypes()}>
        {#each rows as row (row.id)}
            <li role="option" aria-selected={row.id === selected}>
                <button
                    type="button"
                    class="row"
                    class:selected={row.id === selected}
                    data-notetype={row.id}
                    on:click={() => (selected = row.id)}
                    on:dblclick={() => askRename()}
                >
                    {row.name} [{tr.browsingNoteCount({ count: row.useCount })}]
                </button>
            </li>
        {/each}
    </ul>

    {#if error}
        <div class="error" id="nt-error">{error}</div>
    {/if}

    <div class="buttons">
        <button id="nt-add" class="btn btn-secondary" disabled={busy} on:click={() => void openAdd()}
            >{tr.actionsAdd()}</button
        >
        <button
            id="nt-rename"
            class="btn btn-secondary"
            disabled={busy || !selectedRow}
            on:click={askRename}>{tr.actionsRename()}</button
        >
        <button
            id="nt-delete"
            class="btn btn-secondary"
            disabled={busy || !selectedRow}
            on:click={() => void remove()}>{tr.actionsDelete()}</button
        >
        <button
            id="nt-fields"
            class="btn btn-secondary"
            disabled={busy || !selectedRow}
            on:click={() => void openFieldsDialog()}>{tr.notetypesFields()}</button
        >
        <button
            id="nt-cards"
            class="btn btn-secondary"
            disabled={busy || !selectedRow}
            on:click={() => void openCardsDialog()}>{tr.notetypesCards()}</button
        >
    </div>
</div>

<!-- 「添加」第一步：原厂类型 / 克隆现有类型（`models.py:257` 的 `AddModel`） -->
<Modal bind:this={addModal} modalKey="nt-add" onOkClicked={() => void askAddName()}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.notetypesAddNoteType()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <select id="nt-add-select" class="form-control" bind:value={addChoice}>
            {#each addChoices as choice, index (choice.label)}
                <option value={index}>{choice.label}</option>
            {/each}
        </select>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={addModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={addModal.acceptHandler}>
            {tr.actionsAdd()}
        </button>
    </div>
</Modal>

<!-- 第二步（添加时问名字）/ 重命名 -->
<Modal bind:this={textModal} modalKey="nt-text" onOkClicked={() => void applyText()}>
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
            <input id="nt-text-input" class="form-control" bind:value={textValue} />
        </form>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={textModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={textModal.acceptHandler}>
            {textMode === "add" ? tr.actionsAdd() : tr.actionsRename()}
        </button>
    </div>
</Modal>

<style lang="scss">
    .notetypes {
        padding: 0.75rem;
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
        /* 窗口高度撑满（`100%` 在这里解析不到窗口高度，见「卡片…」对话框） */
        height: 100vh;
        box-sizing: border-box;
    }

    .list {
        flex: 1;
        overflow-y: auto;
        margin: 0;
        padding: 0;
        list-style: none;
        border: 1px solid var(--border, #ccc);
        border-radius: 4px;
    }

    .list .row {
        display: block;
        width: 100%;
        padding: 0.25rem 0.5rem;
        border: none;
        background: none;
        color: inherit;
        text-align: left;
        cursor: pointer;
        user-select: none;
    }

    .list .row.selected {
        background: var(--fg, #007bff);
        color: var(--bg, #fff);
    }

    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 0.25rem;
    }

    .error {
        color: red;
        white-space: pre-wrap;
    }
</style>
