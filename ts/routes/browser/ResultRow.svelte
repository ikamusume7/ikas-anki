<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
结果表格的一行。

对照 Qt 的 `QAbstractTableModel.data()`（`browser/table/model.py:103-125`）：
后端**只有单行取数接口**（`browserRowForId`），所以这里挂载时才去取，虚拟列表滚到哪就取哪。

行内的单元格是**纯文本**（`rslib/src/browser_table.rs` 内部已经调 `html_to_text_line`），
不需要二次转换；颜色/字体来自 `BrowserRow.color` 与 `font_name`/`font_size`。
-->
<script lang="ts">
    import type { BrowserRow } from "@generated/anki/search_pb";
    import { BrowserRow_Color } from "@generated/anki/search_pb";
    import { browserRowForId } from "@generated/backend";

    /** 卡片模式是 cid、笔记模式是 nid（后端按 `browserTableShowNotesMode` 判定） */
    export let id: number;
    export let columnCount: number;
    /**
     * 列集合/顺序的版本号。
     *
     * `cells` 的顺序由后端按「送进 `setActiveBrowserColumns` 的列顺序」决定，所以列一变
     * （勾选、拖动列头）就得重新取数 —— 只按 `id` 判断的话，同一次搜索里的行不会刷新，
     * 表头与内容就会错位。
     */
    export let version = 0;
    /** 列 key（顺序与列头一致），用于取列宽 */
    export let columnKeys: string[] = [];
    export let widths: Record<string, number> = {};
    export let defaultWidths: Record<string, number> = {};
    /** 该列是否使用卡片的字体（`BrowserColumns.Column.uses_cell_font`） */
    export let cellFonts: boolean[] = [];
    /**
     * 选中的 id 列表。
     *
     * **必须是数组，不能传 `selected={isSelected(id)}`** —— Svelte 只追踪模板表达式里
     * 直接引用的响应式变量，函数体里用到的 `selected` 它看不见，于是「只改 `selected`
     * 不改其它状态」时（Ctrl+A 全选、反选）行的高亮不会刷新（实测踩到过）。
     */
    export let selectedIds: number[] = [];
    /** 「当前项」（Qt 的 currentIndex）：被编辑器/预览跟随的那一行 */
    export let isCurrent = false;
    export let onSelect: (id: number, event: MouseEvent) => void;
    export let onEdit: (id: number) => void;
    /** 右键：打开动作菜单（Qt 那边是 Notes/Cards 菜单栏） */
    export let onMenu: (id: number, event: MouseEvent) => void = () => {};

    let row: BrowserRow | undefined = undefined;
    /** 已经取过数的那一行（虚拟列表复用组件实例，所以要知道 `id`/列版本换没换） */
    let loadedId: number | undefined = undefined;
    let loadedVersion = -1;

    function cellStyle(index: number): string {
        const key = columnKeys[index];
        const width = key ? (widths[key] ?? defaultWidths[key] ?? 140) : 140;
        const parts = [`width: ${width}px`];
        if (!cellFonts[index] || !row?.fontName) {
            return parts.join("; ");
        }
        parts.push(`font-family: ${row.fontName}`, `font-size: ${row.fontSize}px`);
        return parts.join("; ");
    }

    function colorClass(color: BrowserRow_Color | undefined): string {
        // 注意枚举成员名：protoc-gen-es 会把 proto 里的 `COLOR_` 前缀去掉
        // （`COLOR_MARKED` → `MARKED`），与 proto 原文不同
        switch (color) {
            case BrowserRow_Color.MARKED:
                return "marked";
            case BrowserRow_Color.SUSPENDED:
                return "suspended";
            case BrowserRow_Color.BURIED:
                return "buried";
            case BrowserRow_Color.FLAG_RED:
                return "flag1";
            case BrowserRow_Color.FLAG_ORANGE:
                return "flag2";
            case BrowserRow_Color.FLAG_GREEN:
                return "flag3";
            case BrowserRow_Color.FLAG_BLUE:
                return "flag4";
            case BrowserRow_Color.FLAG_PINK:
                return "flag5";
            case BrowserRow_Color.FLAG_TURQUOISE:
                return "flag6";
            case BrowserRow_Color.FLAG_PURPLE:
                return "flag7";
            default:
                return "";
        }
    }

    // 逐行取数。`alertOnError: false`：行取不到（例如卡片刚好被删）只该让这一行留空，
    // 而 `alert()` 会卡住整个页面的 JS 事件循环（见 TAURI_HANDOVER.md §5.18）。
    async function load(cardId: number): Promise<void> {
        try {
            const fetched = await browserRowForId(
                { val: BigInt(cardId) },
                { alertOnError: false },
            );
            // 期间又换了卡片的话，这次结果已经过期
            if (loadedId === cardId) {
                row = fetched;
            }
        } catch (error) {
            console.error(`browserRowForId(${cardId}) 失败`, error);
        }
    }

    // **必须跟着 `id` 变化重新取数**：`VirtualTable` 的 `{#each slice as index (index)}`
    // 以行号为 key，搜索结果换了之后同一个行号会对应另一张卡片，而组件实例是被复用的 ——
    // 只在 `onMount` 取一次的话，表格会一直显示上一次搜索的内容（实测踩到过）。
    $: if (loadedId !== id || loadedVersion !== version) {
        loadedId = id;
        loadedVersion = version;
        row = undefined;
        void load(id);
    }
</script>

<tr
    class:selected={selectedIds.includes(id)}
    class:current={isCurrent}
    class={colorClass(row?.color)}
    on:click={(event) => onSelect(id, event)}
    on:dblclick={() => onEdit(id)}
    on:contextmenu={(event) => onMenu(id, event)}
>
    {#each Array(columnCount) as _, index}
        <td style={cellStyle(index)} title={row?.cells[index]?.text ?? ""}>
            {row?.cells[index]?.text ?? ""}
        </td>
    {/each}
</tr>

<style lang="scss">
    /* 变量名取自 `ts/lib/sass/_vars.scss`（`--selected-*`、`--state-*`、`--flag-N`） */
    tr {
        cursor: default;

        &.selected,
        &.selected:hover {
            background: var(--selected-bg);
            color: var(--selected-fg);
        }

        /* 「当前项」：Qt 里是 currentIndex 的虚线框 */
        &.current td {
            outline: 1px solid var(--fg-subtle);
            outline-offset: -1px;
        }

        &.marked td:first-child {
            color: var(--state-marked);
        }

        &.suspended td:first-child {
            color: var(--state-suspended);
        }

        &.buried td:first-child {
            color: var(--state-buried);
        }

        &.flag1 td:first-child {
            color: var(--flag-1);
        }

        &.flag2 td:first-child {
            color: var(--flag-2);
        }

        &.flag3 td:first-child {
            color: var(--flag-3);
        }

        &.flag4 td:first-child {
            color: var(--flag-4);
        }

        &.flag5 td:first-child {
            color: var(--flag-5);
        }

        &.flag6 td:first-child {
            color: var(--flag-6);
        }

        &.flag7 td:first-child {
            color: var(--flag-7);
        }
    }
</style>
