<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
「字段…」对话框（`ts/routes/fields/`，对照 `qt/aqt/fields.py` 的 `FieldDialog`）。

- 字段列表 + 添加 / 重命名 / 删除 / 重新定位（`actions-add` / `-rename` / `-delete` /
  `-reposition`，`fields.ui:43-64`）；
- 选中字段的设置：**编辑字体**（`fields-editing-font`，写进
  `Field.Config.fontName/fontSize`，默认 Arial/20，见 `rslib/src/notetype/fields.rs:45-46`）、
  描述（`fields-description`）、以及四个勾选（RTL / HTML 默认 / 折叠 / 排除搜索）；
- 保存（`Save`）时整份 notetype 一起落库：`updateNotetype`（对照 `fields.py:294-303` 的
  `accept` → `update_notetype_legacy`）；取消时有改动会先问一句
  （`card-templates-discard-changes`，对照 `fields.py:283-292`）。

与 Qt 的差异（见 `tauri/README.md`）：
- 「编辑字体」用文本框 + 数字框（Qt 是 `QFontComboBox` 列出系统字体）；
- 「重新定位」是弹一个数字框（Qt 还能拖动列表项）；
- 字段删除少一个「有 N 张笔记用到它」的确认框（后端在保存时才校验）；
- 本页不做模板/样式（那是「卡片…」对话框，见 `ts/routes/cards/`）。
-->
<script lang="ts">
    // 字段的 `ord` 是 `anki.generic.UInt32`（不是普通 number），所以要从 generic_pb 取
    import { UInt32 } from "@generated/anki/generic_pb";
    import {
        Notetype,
        Notetype_Config,
        Notetype_Field,
        Notetype_Field_Config,
    } from "@generated/anki/notetypes_pb";
    import * as tr from "@generated/ftl";
    import { getNotetype, updateNotetype } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { onMount } from "svelte";

    import Modal from "$lib/components/Modal.svelte";

    let notetype: Notetype | undefined = undefined;
    let selected = 0;
    let busy = false;
    let dirty = false;
    let error = "";

    let textModal: Modal;
    let numberModal: Modal;
    /** 文本框弹框在做什么：添加 / 重命名；数字框弹框用于重新定位 */
    let textMode: "add" | "rename" = "add";
    let textValue = "";
    let positionValue = 1;

    $: fields = notetype?.fields ?? [];
    $: selectedField = fields[selected];

    $: textTitle = textMode === "add" ? tr.fieldsAddField() : tr.actionsRename();

    /** 保存：整份 notetype 一起写回（对照 `fields.py:294-303`）。 */
    async function save(): Promise<void> {
        if (!notetype) {
            return;
        }
        busy = true;
        error = "";
        try {
            await updateNotetype(notetype);
            bridgeCommand("close");
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    /** 取消：有改动先问一句（`card-templates-discard-changes`）。 */
    function cancel(): void {
        if (dirty && !confirm(tr.cardTemplatesDiscardChanges())) {
            return;
        }
        bridgeCommand("close");
    }

    // 字段操作（都在内存里改，保存时一起落库）##############################

    function nameTaken(name: string, skip: number): boolean {
        return fields.some((field, index) => index !== skip && field.name === name);
    }

    function askAdd(): void {
        textMode = "add";
        textValue = "";
        textModal.show();
    }

    function askRename(): void {
        if (!selectedField) {
            return;
        }
        textMode = "rename";
        textValue = selectedField.name;
        textModal.show();
    }

    /** 添加/重命名共用的确认（对照 `fields.py:150-181` 的 `onAdd` / `onRename`）。 */
    function applyText(): void {
        const name = textValue.trim();
        if (!name || !notetype) {
            return;
        }
        if (nameTaken(name, textMode === "rename" ? selected : -1)) {
            alert(tr.fieldsThatFieldNameIsAlreadyUsed());
            return;
        }
        if (textMode === "add") {
            notetype.fields = [
                ...notetype.fields,
                new Notetype_Field({
                    name,
                    ord: new UInt32({ val: notetype.fields.length }),
                }),
            ];
            selected = notetype.fields.length - 1;
        } else {
            notetype.fields[selected].name = name;
        }
        dirty = true;
    }

    /** 删除：至少留一个字段，且不能删「禁止删除」的（`fields.py:183-201`）。 */
    function removeSelected(): void {
        if (!notetype || !selectedField) {
            return;
        }
        if (notetype.fields.length <= 1) {
            alert(tr.fieldsNotesRequireAtLeastOneField());
            return;
        }
        if (selectedField.config?.preventDeletion) {
            alert(tr.fieldsFieldIsRequired());
            return;
        }
        notetype.fields = notetype.fields.filter((_, index) => index !== selected);
        renumber();
        selected = Math.max(0, selected - 1);
        dirty = true;
    }

    function askReposition(): void {
        if (!selectedField) {
            return;
        }
        positionValue = selected + 1;
        numberModal.show();
    }

    /** 重新定位（对照 `fields.py:225-232` 的 `reposition_field`）。 */
    function applyReposition(): void {
        if (!notetype) {
            return;
        }
        const target = Math.min(Math.max(Math.round(positionValue), 1), notetype.fields.length);
        const next = [...notetype.fields];
        const [field] = next.splice(selected, 1);
        next.splice(target - 1, 0, field);
        notetype.fields = next;
        renumber();
        selected = target - 1;
        dirty = true;
    }

    /** 重排之后把 `ord` 重新编号 0..n-1（对照 pylib 的 `reposition_field`）。 */
    function renumber(): void {
        if (!notetype) {
            return;
        }
        notetype.fields = notetype.fields.map(
            (field, index) =>
                new Notetype_Field({
                    name: field.name,
                    ord: new UInt32({ val: index }),
                    config: field.config,
                }),
        );
    }

    function setSortField(): void {
        if (!notetype) {
            return;
        }
        notetype.config!.sortFieldIdx = selected;
        dirty = true;
    }

    onMount(async () => {
        await setupI18n({
            modules: [
                ModuleName.FIELDS,
                ModuleName.ACTIONS,
                ModuleName.EDITING,
                ModuleName.CARD_TEMPLATES,
            ],
        });
        const requested = Number(new URLSearchParams(window.location.search).get("notetype") ?? 0);
        const loaded = await getNotetype({ ntid: BigInt(requested) });
        // 字段配置可能缺省（后端只在需要时才发），补出来省得后面到处 `!`
        loaded.config = loaded.config ?? new Notetype_Config();
        for (const field of loaded.fields) {
            field.config = field.config ?? new Notetype_Field_Config();
        }
        notetype = loaded;
    });
</script>

<div class="fields">
    <div class="list-header">{tr.editingFields()}</div>

    <ul class="list" id="field-list">
        {#each fields as field, index (index)}
            <li
                class:selected={index === selected}
                data-field={index}
                on:click={() => (selected = index)}
                on:keydown={() => {}}
                role="option"
                aria-selected={index === selected}
            >
                <span class="name">{field.name}</span>
                {#if notetype?.config?.sortFieldIdx === index}
                    <span class="sort-mark">{tr.fieldsSortByThisFieldInThe()}</span>
                {/if}
            </li>
        {/each}
    </ul>

    <div class="list-buttons">
        <button class="btn btn-secondary" id="field-add" on:click={askAdd}
            >{tr.actionsAdd()}</button
        >
        <button class="btn btn-secondary" id="field-rename" on:click={askRename}
            >{tr.actionsRename()}</button
        >
        <button class="btn btn-secondary" id="field-delete" on:click={removeSelected}
            >{tr.actionsDelete()}</button
        >
        <button class="btn btn-secondary" id="field-reposition" on:click={askReposition}
            >{tr.actionsReposition()}</button
        >
    </div>

    {#if selectedField}
        <div class="options">
            <div class="font-row">
                <span>{tr.fieldsEditingFont()}</span>
                <input
                    id="field-font-name"
                    bind:value={selectedField.config!.fontName}
                    on:input={() => (dirty = true)}
                />
                <input
                    id="field-font-size"
                    type="number"
                    min="5"
                    max="300"
                    bind:value={selectedField.config!.fontSize}
                    on:input={() => (dirty = true)}
                />
            </div>

            <label class="row">
                <input
                    type="checkbox"
                    id="field-sort"
                    checked={notetype?.config?.sortFieldIdx === selected}
                    on:change={setSortField}
                />
                {tr.fieldsSortByThisFieldInThe()}
            </label>
            <label class="row">
                <input
                    type="checkbox"
                    id="field-rtl"
                    bind:checked={selectedField.config!.rtl}
                    on:change={() => (dirty = true)}
                />
                {tr.fieldsReverseTextDirectionRtl()}
            </label>
            <label class="row">
                <input
                    type="checkbox"
                    id="field-plain"
                    bind:checked={selectedField.config!.plainText}
                    on:change={() => (dirty = true)}
                />
                {tr.fieldsHtmlByDefault()}
            </label>
            <label class="row">
                <input
                    type="checkbox"
                    id="field-collapsed"
                    bind:checked={selectedField.config!.collapsed}
                    on:change={() => (dirty = true)}
                />
                {tr.fieldsCollapseByDefault()}
            </label>
            <label class="row">
                <input
                    type="checkbox"
                    id="field-exclude"
                    bind:checked={selectedField.config!.excludeFromSearch}
                    on:change={() => (dirty = true)}
                />
                {tr.fieldsExcludeFromSearch()}
            </label>

            <label class="description">
                {tr.fieldsDescription()}
                <input
                    id="field-description"
                    placeholder={tr.fieldsDescriptionPlaceholder()}
                    bind:value={selectedField.config!.description}
                    on:input={() => (dirty = true)}
                />
            </label>
        </div>
    {/if}

    {#if error}
        <div class="error" id="fields-error">{error}</div>
    {/if}

    <div class="buttons">
        <button class="btn btn-secondary" on:click={cancel}>{tr.actionsCancel()}</button>
        <button
            class="btn btn-primary"
            id="fields-save"
            disabled={busy || !notetype}
            on:click={() => void save()}>{tr.actionsSave()}</button
        >
    </div>
</div>

<Modal
    bind:this={textModal}
    modalKey="fields-text"
    onOkClicked={applyText}
    onShown={() => document.getElementById("fields-text-input")?.focus()}
>
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
            <label for="fields-text-input" class="col-form-label">{tr.actionsNewName()}</label>
            <input id="fields-text-input" class="form-control" bind:value={textValue} />
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

<Modal
    bind:this={numberModal}
    modalKey="fields-position"
    onOkClicked={applyReposition}
    onShown={() => document.getElementById("fields-position-input")?.focus()}
>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.actionsReposition()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <form on:submit|preventDefault={numberModal.acceptHandler}>
            <label for="fields-position-input" class="col-form-label"
                >{tr.fieldsNewPosition1({ val: fields.length })}</label
            >
            <input
                id="fields-position-input"
                class="form-control"
                type="number"
                min="1"
                max={fields.length}
                bind:value={positionValue}
            />
        </form>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={numberModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={numberModal.acceptHandler}>
            {tr.actionsReposition()}
        </button>
    </div>
</Modal>

<style>
    .fields {
        padding: 1rem;
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
        height: 100%;
    }

    .list {
        list-style: none;
        margin: 0;
        padding: 0;
        border: 1px solid var(--border, #ccc);
        max-height: 14rem;
        overflow-y: auto;
    }

    .list li {
        display: flex;
        justify-content: space-between;
        gap: 0.5rem;
        padding: 0.3rem 0.5rem;
        cursor: pointer;
    }

    .list li.selected {
        background: var(--selected-bg, #cfe3ff);
    }

    .sort-mark {
        color: var(--fg-subtle, #666);
        font-size: 0.85rem;
    }

    .list-buttons,
    .buttons {
        display: flex;
        gap: 0.5rem;
    }

    .list-buttons {
        justify-content: flex-start;
    }

    .buttons {
        justify-content: flex-end;
        margin-top: auto;
        padding-top: 0.75rem;
    }

    .options {
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
        border-top: 1px solid var(--border, #ccc);
        padding-top: 0.75rem;
    }

    .font-row {
        display: flex;
        align-items: center;
        gap: 0.5rem;
    }

    .font-row input[type="number"] {
        width: 5rem;
    }

    .row {
        display: flex;
        align-items: center;
        gap: 0.5rem;
    }

    .description {
        display: flex;
        flex-direction: column;
        gap: 0.25rem;
    }

    .error {
        color: var(--fg-error, #c62828);
        white-space: pre-wrap;
    }
</style>
