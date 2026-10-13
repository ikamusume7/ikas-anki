<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
浏览器行右键菜单。

Qt 的浏览器有四个菜单栏（Notes / Cards / View / Jumps）；Web 侧没有菜单栏，
所以同一份动作表放在**行右键菜单**里（工具栏的「更多」按钮打开同一个菜单）。
动作、快捷键与勾选态都对照 `qt/aqt/forms/browser.ui` 的 action 表，见 README 的
「浏览器」与 `qt/aqt/browser/browser.py:301-401`。
-->
<script lang="ts">
    import type { MenuSection } from "./menu";

    export let x: number;
    export let y: number;
    export let sections: MenuSection[] = [];
    export let onclose: () => void;

    function pick(item: { run: () => void; disabled?: boolean }): void {
        if (item.disabled) {
            return;
        }
        onclose();
        item.run();
    }

    /** 每项的 `data-action`（没有显式 action 的项用文案当标识）。 */
    function actionId(item: { action?: string; label: string }): string {
        return item.action ?? item.label;
    }
</script>

<div class="menu-backdrop" role="presentation" on:click={onclose} on:contextmenu={onclose}></div>
<div class="context-menu" style="left: {x}px; top: {y}px">
    {#each sections as section, index (index)}
        {#if index > 0}
            <hr class="separator" />
        {/if}
        {#each section.items as item (actionId(item))}
            <button
                class="item"
                class:disabled={item.disabled}
                data-action={actionId(item)}
                on:click={() => pick(item)}
            >
                <span class="check">{item.checked ? "✓" : ""}</span>
                <span class="label">{item.label}</span>
                {#if item.shortcut}
                    <span class="shortcut">{item.shortcut}</span>
                {/if}
            </button>
        {/each}
    {/each}
</div>

<style lang="scss">
    .menu-backdrop {
        position: fixed;
        inset: 0;
        z-index: 1040;
    }

    .context-menu {
        position: fixed;
        z-index: 1050;
        min-width: 15rem;
        max-height: 80vh;
        padding: 0.25rem;
        overflow: auto;
        background: var(--canvas);
        border: 1px solid var(--border);
        border-radius: var(--border-radius);
        box-shadow: 0 4px 16px rgb(0 0 0 / 25%);
    }

    .item {
        display: flex;
        gap: 0.5rem;
        align-items: center;
        width: 100%;
        padding: 0.2rem 0.4rem;
        color: var(--fg);
        text-align: start;
        background: transparent;
        border: none;
        border-radius: var(--border-radius);

        &:hover:not(.disabled) {
            background: var(--selected-bg);
            color: var(--selected-fg);
        }

        &.disabled {
            color: var(--fg-subtle);
            cursor: default;
        }
    }

    .check {
        width: 1rem;
        text-align: center;
    }

    .label {
        flex: 1;
        white-space: nowrap;
    }

    .shortcut {
        color: var(--fg-subtle);
        font-size: 0.85em;
    }

    .separator {
        margin: 0.25rem 0;
        border-color: var(--border-subtle);
        opacity: 1;
    }
</style>
