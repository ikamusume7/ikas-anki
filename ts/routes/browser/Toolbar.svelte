<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
浏览器的动作栏。

Qt 把这些动作放在菜单栏里（Notes / Cards / View 菜单，`browser.py:301-401`），
本页面没有菜单栏，所以最常用的几个平铺成按钮，其余（标记 / 旗标 / 暂停 / 埋藏 /
重置 / 重排 / 到期日 / 立即评分 / 复制 / 卡片信息 / 跳转…）放在「更多」与
行右键菜单里（同一个动作表，见 `+page.svelte` 的 `menuSections`）。

按钮的禁用条件对照 `browser.py:678-710` 的 `_update_row_actions` /
`_update_selection_actions`：这几个动作都要求有选中项。
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    export let notesMode: boolean;
    export let busy: boolean;
    export let selectionCount: number;
    export let onChangeDeck: () => void;
    export let onAddTags: () => void;
    export let onDelete: () => void;
    export let onPreview: () => void;
    export let onToggleNotesMode: () => void;
    /** 打开完整动作菜单（与行右键菜单是同一份） */
    export let onMore: (event: MouseEvent) => void;

    $: hasSelection = selectionCount > 0;
    // 快捷键名不是可翻译文本，直接写（Qt 那边同样由快捷键表生成）
    const previewShortcut = "Ctrl+Shift+P";
</script>

<div class="toolbar">
    <!-- id 供验证脚本定位（`tauri/tools/browser-check.mjs`），与 Qt 的 action 名对应 -->
    <button
        id="browse-change-deck"
        disabled={!hasSelection || busy}
        title="Ctrl+D"
        on:click={onChangeDeck}>{tr.browsingChangeDeck2()}</button
    >
    <button
        id="browse-add-tags"
        disabled={!hasSelection || busy}
        title="Ctrl+Shift+A"
        on:click={onAddTags}>{tr.browsingAddTags2()}</button
    >
    <button
        id="browse-delete"
        disabled={!hasSelection || busy}
        title="Ctrl+Del"
        on:click={onDelete}>{tr.actionsDelete()}</button
    >
    <button
        id="browse-preview"
        disabled={busy}
        title={tr.browsingPreviewSelectedCard({ val: previewShortcut })}
        on:click={onPreview}>{tr.actionsPreview()}</button
    >
    <button id="browse-more" disabled={busy} on:click={onMore}>{tr.studyingMore()}</button>
    <button
        id="browse-toggle-notes-mode"
        class="mode"
        class:active={notesMode}
        disabled={busy}
        on:click={onToggleNotesMode}>{tr.browsingToggleShowingCardsNotes()}</button
    >
</div>

<style lang="scss">
    .toolbar {
        display: flex;
        gap: 0.5rem;
        align-items: center;
        padding: 0 0.5rem 0.5rem;

        button {
            padding: 0.2rem 0.6rem;
            color: var(--fg);
            background: var(--canvas-inset);
            border: 1px solid var(--border-subtle);
            border-radius: var(--border-radius);

            &:disabled {
                color: var(--fg-subtle);
                opacity: 0.6;
            }

            &:not(:disabled):hover {
                border-color: var(--fg-subtle);
            }

            &.active {
                color: var(--selected-fg);
                background: var(--selected-bg);
            }
        }

        .mode {
            margin-inline-start: auto;
        }
    }
</style>
