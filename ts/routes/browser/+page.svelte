<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
浏览器（Browse）窗口。

Qt 的浏览器是**原生 Qt 界面**（`qt/aqt/browser/`：搜索框、笔记表格、侧边栏、菜单栏
全是 Qt 控件），仓库里没有对应的 Web 页面，因此这个页面是新写的。宿主侧的契约
（`browserReady` 的响应、`eval` 注入 `loadSearch`、窗口标题、双击开编辑器、预览窗口）
见 `tauri/src/browser.rs`。

本页面对照的上游实现，按功能分：

| 功能 | 上游 |
| --- | --- |
| 搜索词（后端语法字符串） | `browser.py:502-512` `onSearchActivated` |
| 排序（列 key + 方向交给后端） | `browser/table/table.py:492-516`、`collection.py:692-719` |
| 多选与选中计数 | `browser/table/table.py:351-407`、`browser.py:632-664` |
| 改牌组 / 加标签 / 删除 | `browser.py:895-974`、`aqt/operations/{card,note,tag}.py` |
| 笔记模式 | `browser/table/table.py:40,201`、`pylib/anki/browser.py:7-11` |
| 列清单（`activeCols`） | `browser/table/state.py:137-148` |
| 列宽（PM profile） | `browser/table/table.py:326-335`、`aqt/utils.py:849-859` |
-->
<script lang="ts">
    import { onMount } from "svelte";

    import { ConfigKey_String, OptionalStringConfigKey } from "@generated/anki/config_pb";
    import type { DeckTreeNode } from "@generated/anki/decks_pb";
    import { openFilePickerRequest } from "@generated/anki/frontend_pb";
    import { Empty, String as GenericString } from "@generated/anki/generic_pb";
    import { ExportLimit } from "@generated/anki/import_export_pb";
    import { NoteIds } from "@generated/anki/notes_pb";
    import { postProto } from "@generated/post";
    import type { BrowserRow, BrowserColumns_Column } from "@generated/anki/search_pb";
    import {
        BrowserColumns_Sorting,
        BrowserRow_Color,
        SortOrder,
    } from "@generated/anki/search_pb";
    import {
        BuryOrSuspendCardsRequest_Mode,
        CardAnswer_Rating,
        ScheduleCardsAsNewRequest_Context,
    } from "@generated/anki/scheduler_pb";
    import * as tr from "@generated/ftl";
    // `qt-accel-*` 的文案没有类型化函数（生成器只覆盖核心仓库，见
    // `rslib/i18n/write_strings.rs` 的 `is_core()`），所以按 key 直接取
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import {
        addNoteTags,
        allBrowserColumns,
        browserRowForId,
        buildSearchString,
        buryOrSuspendCards,
        cardsOfNote,
        clearUnusedTags,
        deckTree,
        exportAnkiPackage,
        exportCardCsv,
        exportCollectionPackage,
        exportNoteCsv,
        findAndReplace,
        getCard,
        getNote,
        getNotetypeNames,
        gradeNow,
        redo,
        removeNotes,
        removeNoteTags,
        repositionDefaults,
        restoreBuriedAndSuspendedCards,
        scheduleCardsAsNew,
        scheduleCardsAsNewDefaults,
        searchCards,
        searchNotes,
        setActiveBrowserColumns,
        setDeck,
        setDueDate,
        setFlag,
        sortCards,
        undo,
    } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { getColConfig, getProfileConfig, setColConfig, setProfileConfig } from "@tslib/profile";

    import VirtualTable from "$lib/components/VirtualTable.svelte";

    import ContextMenu from "./ContextMenu.svelte";
    import DeckPickerModal from "./DeckPickerModal.svelte";
    import ExportDialog from "./ExportDialog.svelte";
    import FormModal from "./FormModal.svelte";
    import type { ExportChoice, FormValues, MenuSection } from "./menu";
    import ResultRow from "./ResultRow.svelte";
    import Sidebar from "./Sidebar.svelte";
    import TagsModal from "./TagsModal.svelte";
    import Toolbar from "./Toolbar.svelte";

    /** 行高：VirtualTable 用固定行高做虚拟滚动 */
    const itemHeight = 30;
    /** 底部状态栏高度，供 VirtualTable 算可视区（`bottomOffset`） */
    const statusHeight = 26;

    /** 默认列，对照 `pylib/anki/browser.py:33-34` 的 `BrowserDefaults` */
    const cardColumnDefaults = ["noteFld", "template", "cardDue", "deck"];
    const noteColumnDefaults = ["noteFld", "note", "template", "noteTags"];

    /** 默认列宽（Qt 把「显示哪些列」存集合配置，宽度与列顺序存 profile） */
    const defaultWidths: Record<string, number> = {
        noteFld: 260,
        note: 260,
        template: 150,
        cardDue: 90,
        deck: 150,
        noteTags: 170,
    };
    /**
     * 列宽的 profile 配置键。
     *
     * Qt 用 `utils.saveHeader`（`browser/table/table.py:327`）把整个表头状态（列宽 +
     * 列顺序）存成一份 blob，卡片/笔记各一个键（`table/state.py:131` 的
     * `GEOMETRY_KEY_PREFIX`）；我们只存宽度，形状是 `{ cards: {...}, notes: {...} }`
     * —— **按模式分开**（与 Qt 两个键等价）。旧版存的是扁平的 `{ 列key: 宽度 }`，
     * 读到会自动迁移过来。
     */
    const widthsConfigKey = "tauriBrowserColumnWidths";

    let ready = false;
    let allColumns: BrowserColumns_Column[] = [];
    /** 列 key → 列定义（`columns` 由它和 `columnKeys` 推出来） */
    let columnIndex: Map<string, BrowserColumns_Column> = new Map();
    $: columnIndex = new Map(allColumns.map((column) => [column.key, column]));
    /**
     * 当前显示的列 key，**顺序就是显示顺序**（= 集合配置 `activeCols`/`activeNoteCols`）。
     *
     * 后端 `browserRowForId` 返回的 `cells` **按送进 `setActiveBrowserColumns` 的顺序
     * 排**，所以「表头顺序」与「单元格顺序」必须同源 —— 拖动列头改的就是这个数组
     * （Qt 让模型顺序不变、把视图顺序记进表头状态，观察到的结果一样）。
     */
    let columnKeys: string[] = [];
    /** 显示顺序 = `columnKeys` 的顺序 */
    $: columns = columnKeys
        .map((key) => columnIndex.get(key))
        .filter((column): column is BrowserColumns_Column => !!column);
    /** 列集合/顺序变了就 +1：让每行重新取数（`ResultRow` 按 id 缓存，见它的 `version`） */
    let columnsVersion = 0;
    /** 刚拖动过列头：丢掉紧随其后的那次 click（否则拖完还会顺带按新列排序） */
    let suppressSortClick = false;
    let widths: Record<string, number> = {};
    let decks: DeckTreeNode[] = [];
    let deckEntries: { id: bigint; name: string }[] = [];
    let notetypes: { id: bigint; name: string }[] = [];

    /** 当前搜索结果的 id（卡片模式下是 cid，笔记模式下是 nid） */
    let cards: number[] = [];
    /** 选中的 id（顺序无关） */
    let selected: number[] = [];
    /** 「当前项」：被编辑/预览的那一行（Qt 的 `current`） */
    let current: number | null = null;
    /** Shift 多选的锚点（行号） */
    let anchorIndex: number | null = null;

    let search = "";
    let query = "";
    /** 状态栏的临时提示（对照 Qt 的 `tooltip()`） */
    let status = "";
    let error: string | undefined = undefined;
    let busy = false;

    /** 笔记模式（`browserTableShowNotesMode`） */
    let notesMode = false;
    /** 排序：列 key + 是否倒序（`sortType`/`sortBackwards`，笔记模式另一套） */
    let sortColumn = "";
    let sortBackwards = false;

    let deckModal: DeckPickerModal;
    let tagsModal: TagsModal;
    let formModal: FormModal;
    let exportDialog: ExportDialog;
    /** 导出对话框打开时选中的笔记（点了「导出」之后要用） */
    let exportNoteIds: number[] = [];
    /** 通用表单弹框是哪个动作打开的（决定 `onFormOk` 怎么分发） */
    let formAction: "due" | "reposition" | "forget" | "findReplace" | null = null;
    /**
     * 表单弹框的确认按钮文案。
     *
     * Qt 这几个对话框的确认键是 Qt 自带的标准按钮（"OK"，仓库里没有对应的核心 ftl
     * key），所以取最贴合的可用文案：到期日/重排/重置用动作名，
     * 查找替换用对话框标题。
     */
    let confirmLabel = "";
    /** 行右键菜单 / 工具栏「更多」的位置（`null` 表示没打开） */
    let actionMenu: { x: number; y: number } | null = null;
    /** 当前行的状态，供菜单显示勾选态（对照 `browser.py:1009-1093` 的几个 `_update_*`） */
    let menuState = { flag: 0, suspended: false, buried: false, marked: false };
    /** 列清单弹层的位置（在表头右键打开，对照 Qt「视图」菜单里的列勾选项） */
    let columnsMenu: { x: number; y: number } | null = null;

    /** 页面就绪前收到的搜索词（宿主在窗口已存在时会直接注入，可能比挂载更早） */
    let pending: string | undefined;
    let statusTimer: ReturnType<typeof setTimeout> | undefined;

    function flash(message: string): void {
        status = message;
        clearTimeout(statusTimer);
        statusTimer = setTimeout(() => (status = ""), 4000);
    }

    /**
     * 取要显示的列，并把它们告诉后端。
     *
     * 对照 `pylib/anki/collection.py:890-912` 的 `load_browser_card_columns()` /
     * `load_browser_note_columns()`：列清单来自集合配置 `activeCols`（笔记模式
     * `activeNoteCols`，缺省是 `BrowserDefaults`），而且**必须先
     * `setActiveBrowserColumns`** —— 少了它 `browserRowForId` 会直接报
     * `Active browser columns not set.`
     */
    async function loadColumns(): Promise<void> {
        const configured = await getColConfig(notesMode ? "activeNoteCols" : "activeCols");
        const defaults = notesMode ? noteColumnDefaults : cardColumnDefaults;
        columnKeys =
            Array.isArray(configured) && configured.length > 0
                ? configured.map(String)
                : defaults;

        await setActiveBrowserColumns({ vals: columnKeys });
        // `columns` 是由 `columnKeys` 推导出来的（见上面的响应式语句）
    }

    /** 列宽是否为旧版格式（扁平的一份，不区分卡片/笔记模式）。 */
    function isFlatWidths(value: unknown): boolean {
        return (
            !!value &&
            typeof value === "object" &&
            Object.values(value as Record<string, unknown>).every(
                (width) => typeof width === "number",
            )
        );
    }

    /** 读当前模式的列宽（旧版扁平格式会原样沿用，下次保存时迁移成按模式分开）。 */
    async function loadWidths(): Promise<void> {
        const stored = (await getProfileConfig(widthsConfigKey)) as Record<string, unknown> | null;
        if (isFlatWidths(stored)) {
            widths = stored as Record<string, number>;
            return;
        }
        widths = ((stored?.[notesMode ? "notes" : "cards"] ?? {}) as Record<string, number>) ?? {};
    }

    /** 写回当前模式的列宽（另一模式的保留不动）。 */
    async function saveWidths(): Promise<void> {
        const stored = (await getProfileConfig(widthsConfigKey)) as Record<string, unknown> | null;
        const perMode = isFlatWidths(stored) ? {} : { ...(stored ?? {}) };
        perMode[notesMode ? "notes" : "cards"] = widths;
        await setProfileConfig(widthsConfigKey, perMode);
    }

    /** 读排序配置：键随模式不同（`sortType`/`noteSortType`）。 */
    async function loadSort(): Promise<void> {
        const column = await getColConfig(notesMode ? "noteSortType" : "sortType");
        // 笔记模式的 `noteSortType` 在 schema 里没有默认值，Qt 会退回 `noteCrt`
        // （`browser/table/model.py:285-288`）
        const defaultColumn = notesMode ? "noteCrt" : "noteFld";
        sortColumn = typeof column === "string" && column ? column : defaultColumn;
        const backwards = await getColConfig(
            notesMode ? "browserNoteSortBackwards" : "sortBackwards",
        );
        sortBackwards = backwards === true;
    }

    /** 排序配置落库（对照 `browser/table/state.py:74-87` 的 setter）。 */
    async function saveSort(): Promise<void> {
        await setColConfig(notesMode ? "noteSortType" : "sortType", sortColumn);
        await setColConfig(notesMode ? "browserNoteSortBackwards" : "sortBackwards", sortBackwards);
    }

    /** 当前的排序请求。**排序交给后端**（对照 `collection.py:692-719`）。 */
    function sortOrder(): SortOrder {
        if (!sortColumn) {
            return new SortOrder({ value: { case: "none", value: new Empty({}) } });
        }
        return new SortOrder({
            value: {
                case: "builtin",
                value: { column: sortColumn, reverse: sortBackwards },
            },
        });
    }

    /** 跑一次搜索（`next` 是后端搜索语法的字符串）。 */
    async function loadSearch(next: string): Promise<void> {
        if (!ready) {
            pending = next;
            return;
        }
        pending = undefined;
        query = next;
        search = next;
        error = undefined;
        const request = { search: next, order: sortOrder() };
        try {
            // `alertOnError: false`：搜索语法写错时把错误显示在结果区里，
            // 而不是弹 `alert()` 卡住页面（对照 Qt 用 `showWarning` 弹原生提示框，
            // 并且保留上一次的结果）
            const options = { alertOnError: false };
            const response = notesMode
                ? await searchNotes(request, options)
                : await searchCards(request, options);
            cards = response.ids.map(Number);
            // 对照 `browser/table/table.py:520-543` 的 `_restore_selection`：
            // 刷新后只保留仍然存在的选中项
            selected = selected.filter((id) => cards.includes(id));
            if (current !== null && !cards.includes(current)) {
                current = null;
            }
        } catch (problem) {
            console.error("搜索失败", next, problem);
            error = String(problem);
            cards = [];
            selected = [];
            current = null;
        }
        updateTitle();
    }

    /** 供宿主注入（`tauri/src/browser.rs` 的 `loadSearch`）。 */
    Object.assign(globalThis, { loadSearch });

    /** 供宿主注入：编辑器里的预览按钮转过来时，用**当前那一行**去预览。 */
    function previewCurrent(): void {
        void preview();
    }
    Object.assign(globalThis, { previewCurrent });

    /**
     * 供宿主注入：预览窗口点了上一张/下一张时，由持有结果列表的这边算出邻居。
     *
     * 对照 Qt：清单在浏览器窗口里，预览器问它要下一张（`previewer.py:412-430`）。
     * 一处兜底：预览可能是**别的窗口**请求的（例如编辑器里的预览按钮、或另一个
     * 客户端），那张卡片未必在当前结果里 —— 这时退化成从「当前行」（再不然是第一行）
     * 开始算，而不是让翻页静默失效。
     */
    function previewNeighbor(offset: number, cardId: number): void {
        let index = cards.indexOf(cardId);
        if (index < 0) {
            index = current !== null ? cards.indexOf(current) : 0;
        }
        if (index < 0) {
            index = 0;
        }
        const neighbor = cards[index + offset];
        if (neighbor !== undefined) {
            selectRow(neighbor);
            bridgeCommand(`preview:${neighbor}`);
        }
    }
    Object.assign(globalThis, { previewNeighbor });

    /** 窗口标题沿用 Qt 的 `browsing-window-title(-notes)`（含已选/总数），由宿主本地化。 */
    function updateTitle(): void {
        bridgeCommand(`browserTitle:${notesMode ? 1 : 0}:${selected.length}:${cards.length}`);
    }

    function isSelected(id: number): boolean {
        return selected.includes(id);
    }

    /** 单击一行。`Ctrl` 加选、`Shift` 选范围，其余情况单选（对照 QTableView）。 */
    function selectRow(id: number, event?: MouseEvent): void {
        const index = cards.indexOf(id);
        if (event?.shiftKey && anchorIndex !== null) {
            const from = Math.min(anchorIndex, index);
            const to = Math.max(anchorIndex, index);
            selected = cards.slice(from, to + 1);
            current = id;
        } else if (event?.ctrlKey || event?.metaKey) {
            selected = isSelected(id) ? selected.filter((it) => it !== id) : [...selected, id];
            current = id;
            anchorIndex = index;
        } else {
            selected = [id];
            current = id;
            anchorIndex = index;
        }
        updateTitle();
    }

    function selectAll(): void {
        selected = [...cards];
        updateTitle();
    }

    function clearSelection(): void {
        selected = [];
        current = null;
        anchorIndex = null;
        updateTitle();
    }

    /** 双击一行 → 请求宿主用编辑器打开（对照 Qt 的编辑器联动）。 */
    function editRow(id: number): void {
        bridgeCommand(notesMode ? `editNote:${id}` : `editCard:${id}`);
    }

    /** 侧栏项 → 搜索词。交给后端拼（引号/转义规则都在 rslib 里）。 */
    async function searchNode(filter: { case: "deck" | "note"; value: string }): Promise<void> {
        const built = await buildSearchString({ filter });
        await loadSearch(built.val);
    }

    /** 点列头：换列或翻转方向，然后**重新向后端要数据**（前端不排序）。 */
    async function sortByColumn(column: BrowserColumns_Column): Promise<void> {
        const sorting = notesMode ? column.sortingNotes : column.sortingCards;
        if (sorting === BrowserColumns_Sorting.NONE) {
            // 对照 `browser/table/table.py:495-498`
            flash(tr.browsingSortingOnThisColumnIsNot());
            return;
        }
        if (sortColumn === column.key) {
            sortBackwards = !sortBackwards;
        } else {
            sortColumn = column.key;
            // 该列的默认方向（`table.py:494`）
            sortBackwards = sorting === BrowserColumns_Sorting.DESCENDING;
        }
        await saveSort();
        await loadSearch(search);
    }

    /** 列宽拖动：Qt 把表头状态存进 PM profile，我们存进 profile 配置（见 README）。 */
    function startResize(key: string, event: MouseEvent): void {
        event.preventDefault();
        event.stopPropagation();
        const startX = event.clientX;
        const startWidth = widths[key] ?? defaultWidths[key] ?? 140;
        const onMove = (move: MouseEvent) => {
            widths = { ...widths, [key]: Math.max(60, startWidth + move.clientX - startX) };
        };
        const onUp = async () => {
            window.removeEventListener("mousemove", onMove);
            window.removeEventListener("mouseup", onUp);
            await saveWidths();
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
    }

    /**
     * 拖动列头换位（Qt 的 `QHeaderView` 开了 `setSectionsMovable(True)`，
     * 见 `browser/table/table.py:391` 的 `sectionMoved`）。
     *
     * 规则：指针落在哪一列的**左半边**，就把拖动的列插到那一列的位置（落在所有列的
     * 右边就是末尾）—— 与拖动的手感一致，也便于自动断言（拖到第一列左半边 ⇒ 变第一列）。
     *
     * 拖动过程中只改表头顺序（`columnKeys`），松手才写回 `activeCols` 并重新取数：
     * `cells` 的顺序由后端按送进去的列顺序决定，落库 + `columnsVersion` 之后每行会
     * 重新取数，表头与内容才对得上（拖动中短暂错位，松手即恢复）。
     */
    function startReorder(key: string, event: MouseEvent): void {
        if (event.button !== 0) {
            return;
        }
        const startX = event.clientX;
        const boxes = [...document.querySelectorAll<HTMLElement>("thead th")].map((cell) =>
            cell.getBoundingClientRect(),
        );
        const original = [...columnKeys];
        let moved = false;

        const onMove = (move: MouseEvent) => {
            // 4px 之内不算拖动，避免影响「点列头排序」
            if (!moved && Math.abs(move.clientX - startX) < 4) {
                return;
            }
            moved = true;
            const slot = boxes.findIndex((box) => move.clientX < box.left + box.width / 2);
            const to = slot < 0 ? boxes.length - 1 : slot;
            const next = columnKeys.filter((it) => it !== key);
            next.splice(to, 0, key);
            if (next.join() !== columnKeys.join()) {
                columnKeys = next;
            }
        };
        const onUp = async () => {
            window.removeEventListener("mousemove", onMove);
            window.removeEventListener("mouseup", onUp);
            if (!moved) {
                return; // 没拖动 → 交给 click 排序
            }
            // 鼠标松开后浏览器还会补一个 click：拖完不能被当成"点列头排序"
            suppressSortClick = true;
            if (columnKeys.join() !== original.join()) {
                await applyColumns(columnKeys);
            }
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
    }

    // 批量动作 ################################################################

    /**
     * 选中项映射成 note id。
     *
     * 卡片模式下表格里是卡片，而 Qt 的删除/加标签都是**对笔记**生效
     * （`browser.py:905-906` 传的是 `note_ids`），所以要先反查 note id。
     * 后端没有"卡片列表 → 笔记列表"的批量接口（`cards.proto` 只有 `GetCard`），
     * 只能逐张取（只取唯一值）。
     */
    async function noteIdsOfSelection(): Promise<number[]> {
        if (notesMode) {
            return [...selected];
        }
        const notes = await Promise.all(
            selected.map(async (cardId) => {
                const card = await getCard({ cid: BigInt(cardId) });
                return Number(card.noteId);
            }),
        );
        return [...new Set(notes)];
    }

    /** 选中项映射成 card id（笔记模式下要展开成该笔记的所有卡片）。 */
    async function cardIdsOfSelection(): Promise<number[]> {
        if (!notesMode) {
            return [...selected];
        }
        const perNote = await Promise.all(
            selected.map(async (noteId) => {
                const ids = await cardsOfNote({ nid: BigInt(noteId) });
                return ids.cids.map(Number);
            }),
        );
        return perNote.flat();
    }

    /** 一次批量动作之后：重跑搜索并报告结果（对照 Qt 的 operation 回调 + 刷新）。 */
    async function afterAction(message: string): Promise<void> {
        await loadSearch(search);
        flash(message);
    }

    async function addTags(tags: string): Promise<void> {
        if (!tags.trim() || selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const noteIds = await noteIdsOfSelection();
            const changes = await addNoteTags({ noteIds: noteIds.map(BigInt), tags });
            // 对照 `aqt/operations/tag.py`：`browsing_notes_updated(count=out.count)`
            await afterAction(tr.browsingNotesUpdated({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    async function moveToDeck(deckId: bigint): Promise<void> {
        busy = true;
        try {
            const cardIds = await cardIdsOfSelection();
            const changes = await setDeck({ cardIds: cardIds.map(BigInt), deckId });
            // 对照 `aqt/operations/card.py`：`browsing_cards_updated(count=out.count)`
            await afterAction(tr.browsingCardsUpdated({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /**
     * 删除选中的行（对照 `browser.py:895-906`）。
     *
     * 上游**没有确认框**：直接删，靠通用 undo 兜底。这里保持一致，但把结果写进状态栏
     * 并在页面上支持 `Ctrl+Z` 撤销（Qt 是在浏览器菜单里）。
     */
    async function deleteSelected(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const noteIds = await noteIdsOfSelection();
            // 上游传的是 note_ids（删笔记连带其卡片，`browser.py:905-906`）
            const changes = await removeNotes({ noteIds: noteIds.map(BigInt), cardIds: [] });
            // 对照 `aqt/operations/note.py:40-42`：`browsing_cards_deleted(count=out.count)`
            await afterAction(tr.browsingCardsDeleted({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 撤销/重做（对照 Qt 浏览器菜单里的 Undo/Redo，Ctrl+Z / Ctrl+Shift+Z）。 */
    async function undoLast(): Promise<void> {
        await undo(new Empty({}));
        await loadSearch(search);
    }

    async function redoLast(): Promise<void> {
        await redo(new Empty({}));
        await loadSearch(search);
    }

    /** 预览当前项（对照 `browser.py:853-865` 的 `onTogglePreview`）。 */
    async function preview(): Promise<void> {
        // 对照 Qt 的 `skip_if_selection_is_empty`：没有当前项就什么也不做
        if (current === null) {
            return;
        }
        const cardId = notesMode
            ? Number((await cardsOfNote({ nid: BigInt(current) })).cids[0] ?? 0n)
            : current;
        if (!cardId) {
            return;
        }
        bridgeCommand(`preview:${cardId}`);
    }

    /** 卡片/笔记模式切换（对照 `browser/table/table.py:196-206`）。 */
    async function toggleNotesMode(): Promise<void> {
        notesMode = !notesMode;
        await setColConfig("browserTableShowNotesMode", notesMode);
        // 列宽按模式分开存（Qt 是两个 `saveHeader` 键），所以切模式要换一份
        await loadWidths();
        await loadColumns();
        await loadSort();
        await loadSearch(search);
    }

    /**
     * 写回列集合/顺序并让行重新取数。
     *
     * `activeCols` 的顺序就是显示顺序（见 `columnKeys` 的注释），所以勾选与拖动列头
     * 都走这里。`columnsVersion` 是必须的：`ResultRow` 按 `id` 缓存取回来的 `cells`，
     * 列集合一变它的内容就对不上了。
     */
    async function applyColumns(keys: string[]): Promise<void> {
        await setColConfig(notesMode ? "activeNoteCols" : "activeCols", keys);
        await loadColumns();
        columnsVersion += 1;
    }

    /**
     * 勾选/取消一列。
     *
     * 上游把「显示哪些列」放在「视图」菜单里逐个勾选，并且**立即写回**集合配置
     * （`browser/table/state.py:137-148` 的 `toggle_active_column`）。这里在表头右键
     * 打开同样的清单，落库时机保持一致。
     */
    async function toggleColumn(key: string): Promise<void> {
        const keys = columnKeys.includes(key)
            ? columnKeys.filter((it) => it !== key)
            : [...columnKeys, key];
        if (keys.length === 0) {
            // 对照 `state.py:143-148`：Qt 会弹 `browsing-you-must-have-at-least-one`
            flash(tr.browsingYouMustHaveAtLeastOne());
            return;
        }
        await applyColumns(keys);
    }

    function openColumnsMenu(event: MouseEvent): void {
        event.preventDefault();
        columnsMenu = { x: event.clientX, y: event.clientY };
    }

    // 行/卡片状态动作（对照 `browser.py:1006-1167`） ##########################

    /** 标记用的标签名，对照 `pylib/anki/tags.py:31` 的 `MARKED_TAG`。 */
    const markedTag = "marked";

    /**
     * 取 Qt 的加速键文案（`qt-accel-*`）。
     *
     * 这些 key 在 `ftl/qt/qt-accel.ftl`（`browser.ui` 里 `actionUndo` / `action_Info` /
     * `action_set_due_date` 等用的就是它们），**不在核心仓库**里，所以没有类型化函数，
     * 只能按 key 取（`setupI18n` 要带上 `ModuleName.QT_ACCEL`）。文案里的 `&` 是 Qt 的
     * 助记符，网页上要去掉。
     */
    function qtAccel(key: string): string {
        return translateRaw(key).replace(/&/g, "");
    }

    /** `BrowserRow.Color` → `Card.user_flag()`（0 表示没有旗标）。 */
    function flagOfColor(color: BrowserRow_Color | undefined): number {
        if (color === undefined) {
            return 0;
        }
        const flag = color - BrowserRow_Color.FLAG_RED + 1;
        return flag >= 1 && flag <= 7 ? flag : 0;
    }

    /** 当前行（`current`）的浏览行数据；没有当前项或取不到时返回 `undefined`。 */
    async function currentRow(): Promise<BrowserRow | undefined> {
        if (current === null) {
            return undefined;
        }
        try {
            return await browserRowForId({ val: BigInt(current) }, { alertOnError: false });
        } catch (problem) {
            console.error("取当前行失败", problem);
            return undefined;
        }
    }

    /**
     * 当前行对应的 note id。
     *
     * 注意 `BrowserRow` **只有 cells / color / 字体**，没有 id —— 行 id 一直都在
     * `cards`（`current`）里：卡片模式是 cid、笔记模式是 nid（`+page.svelte` 顶部）。
     * 所以卡片模式下要先 `getCard` 反查。
     */
    async function currentNoteId(): Promise<number | undefined> {
        if (current === null) {
            return undefined;
        }
        if (notesMode) {
            return current;
        }
        try {
            return Number((await getCard({ cid: BigInt(current) })).noteId);
        } catch (problem) {
            console.error("取卡片失败", current, problem);
            return undefined;
        }
    }

    /** 当前行对应的 card id（笔记模式下取该笔记的第一张卡片，与预览/卡片信息一致）。 */
    async function currentCardId(): Promise<bigint | undefined> {
        if (current === null) {
            return undefined;
        }
        if (!notesMode) {
            return BigInt(current);
        }
        const noteId = await currentNoteId();
        if (noteId === undefined) {
            return undefined;
        }
        return (await cardsOfNote({ nid: BigInt(noteId) })).cids[0];
    }

    async function noteTagsOf(noteId: number): Promise<string[]> {
        try {
            return (await getNote({ nid: BigInt(noteId) })).tags;
        } catch (problem) {
            console.error("取笔记失败", noteId, problem);
            return [];
        }
    }

    /**
     * 刷新菜单的勾选态。
     *
     * 对照 `browser.py:1009-1093` 的几个 `_update_*`：它们读的都是「当前卡片/当前笔记」。
     * 旗标 / 暂停 / 埋藏可以从行颜色推出（`BrowserRow.Color`），标记必须查笔记的标签
     * （行颜色在「既标记又有旗标」时只会显示其中一个）。
     */
    async function refreshMenuState(): Promise<void> {
        menuState = { flag: 0, suspended: false, buried: false, marked: false };
        const row = await currentRow();
        const noteId = await currentNoteId();
        if (!row || noteId === undefined) {
            return;
        }
        menuState = {
            flag: flagOfColor(row.color),
            suspended: row.color === BrowserRow_Color.SUSPENDED,
            buried: row.color === BrowserRow_Color.BURIED,
            marked: (await noteTagsOf(noteId)).includes(markedTag),
        };
    }

    /** 标记（`marked` 标签的加/删，对照 `browser.py:1083-1093`）。 */
    async function toggleMark(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const noteIds = await noteIdsOfSelection();
            const tags = await Promise.all(noteIds.map((noteId) => noteTagsOf(noteId)));
            const allMarked = tags.every((it) => it.includes(markedTag));
            const request = { noteIds: noteIds.map(BigInt), tags: markedTag };
            const changes = allMarked
                ? await removeNoteTags(request)
                : await addNoteTags(request);
            await afterAction(tr.browsingNotesUpdated({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 旗标：与当前卡片相同的旗标再点一次 = 清除（`browser.py:1056-1068`）。 */
    async function setSelectionFlag(flag: number): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const cardIds = await cardIdsOfSelection();
            const currentFlag = flagOfColor((await currentRow())?.color);
            const changes = await setFlag({
                cardIds: cardIds.map(BigInt),
                flag: flag === currentFlag ? 0 : flag,
            });
            await afterAction(tr.browsingCardsUpdated({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /**
     * 暂停 / 取消暂停（`browser.py:1015-1022`）。
     *
     * 后端的 RPC 是分开的：暂停走 `buryOrSuspendCards(mode=SUSPEND)`，恢复只有
     * 「恢复被暂停/被埋藏的卡片」（`scheduler.proto:11` 的
     * `RestoreBuriedAndSuspendedCards`），所以「取消暂停」也会顺带恢复埋藏 ——
     * 与 Qt 的两个独立入口相比，这是本外壳的简化。
     */
    async function toggleSuspend(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            if ((await currentRow())?.color === BrowserRow_Color.SUSPENDED) {
                await restoreBuriedAndSuspendedCards({ cids: cardIds });
            } else {
                await buryOrSuspendCards({
                    cardIds,
                    noteIds: [],
                    mode: BuryOrSuspendCardsRequest_Mode.SUSPEND,
                });
            }
            await afterAction(tr.browsingCardsUpdated({ count: cardIds.length }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 埋藏 / 取消埋藏（`browser.py:1035-1042`），恢复语义同上。 */
    async function toggleBury(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            if ((await currentRow())?.color === BrowserRow_Color.BURIED) {
                await restoreBuriedAndSuspendedCards({ cids: cardIds });
            } else {
                // `BURY_USER` = 用户手动埋藏（`BURY_SCHED` 是"埋藏同一天的其他卡片"）
                await buryOrSuspendCards({
                    cardIds,
                    noteIds: [],
                    mode: BuryOrSuspendCardsRequest_Mode.BURY_USER,
                });
            }
            await afterAction(tr.browsingCardsUpdated({ count: cardIds.length }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /**
     * 立即评分（`browser.py:1129-1167` 的对话框）。
     *
     * Qt 弹的是一个「重来 / 困难 / 良好 / 简单 + 取消」的对话框；这里放进菜单，
     * 四个评分各一项（少一次点击，语义相同）。
     */
    async function gradeSelected(rating: CardAnswer_Rating): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            await gradeNow({ cardIds, rating });
            await afterAction(tr.schedulingGradedCardsDone({ cards: cardIds.length }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 设置到期日（`aqt/operations/scheduling.py:30-62`）：输入语法由后端解析。 */
    async function askSetDueDate(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        // Qt 把上次输入存在 `setDueBrowser` 集合配置里（`Config.String.SET_DUE_BROWSER`）
        const remembered = await getColConfig("setDueBrowser");
        formAction = "due";
        confirmLabel = tr.actionsSave();
        formModal.show(qtAccel("qt-accel-set-due-date"), [
            {
                kind: "text",
                label: tr.schedulingSetDueDatePrompt({ cards: selected.length }),
                value: `${remembered ?? ""}`,
            },
        ]);
    }

    async function applySetDueDate(days: string): Promise<void> {
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            // `config_key` 让后端自己记住这次输入（`ConfigKey.String.SET_DUE_BROWSER`，
            // 对照 `operations/scheduling.py:47-57`），下面那句只是让本窗口下次预填
            await setDueDate({
                cardIds,
                days,
                configKey: new OptionalStringConfigKey({
                    key: ConfigKey_String.SET_DUE_BROWSER,
                }),
            });
            await setColConfig("setDueBrowser", days);
            await afterAction(tr.schedulingSetDueDateDone({ cards: cardIds.length }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 重新定位新卡（`aqt/forms/reposition.ui` + `scheduling.py:134-206`）。 */
    async function askReposition(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        const defaults = await repositionDefaults(new Empty({}));
        formAction = "reposition";
        confirmLabel = tr.browsingRepositionNewCards();
        formModal.show(tr.browsingRepositionNewCards(), [
            { kind: "number", label: tr.browsingStartPosition(), value: 1, min: 1 },
            { kind: "number", label: tr.browsingStep(), value: 1, min: 1 },
            { kind: "check", label: tr.browsingRandomizeOrder(), value: defaults.random },
            { kind: "check", label: tr.browsingShiftPositionOfExistingCards(), value: defaults.shift },
        ]);
    }

    async function applyReposition(
        startingFrom: number,
        stepSize: number,
        randomize: boolean,
        shiftExisting: boolean,
    ): Promise<void> {
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            const changes = await sortCards({
                cardIds,
                startingFrom,
                stepSize,
                randomize,
                shiftExisting,
            });
            await afterAction(tr.browsingChangedNewPosition({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 重置（Forget，`aqt/forms/forget.ui` + `scheduling.py:95-131`）。 */
    async function askForget(): Promise<void> {
        if (selected.length === 0) {
            return;
        }
        const defaults = await scheduleCardsAsNewDefaults({
            context: ScheduleCardsAsNewRequest_Context.BROWSER,
        });
        formAction = "forget";
        confirmLabel = tr.actionsForgetCard();
        formModal.show(tr.actionsForgetCard(), [
            {
                kind: "check",
                label: tr.schedulingRestorePosition(),
                value: defaults.restorePosition,
            },
            { kind: "check", label: tr.schedulingResetCounts(), value: defaults.resetCounts },
        ]);
    }

    async function applyForget(restorePosition: boolean, resetCounts: boolean): Promise<void> {
        busy = true;
        try {
            const cardIds = (await cardIdsOfSelection()).map(BigInt);
            await scheduleCardsAsNew({
                cardIds,
                log: false,
                restorePosition,
                resetCounts,
                context: ScheduleCardsAsNewRequest_Context.BROWSER,
            });
            await afterAction(tr.schedulingForgotCards({ cards: cardIds.length }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 移除标签（与加标签共用弹框，`browser.py:976-988`）。 */
    async function removeTags(tags: string): Promise<void> {
        if (!tags.trim() || selected.length === 0) {
            return;
        }
        busy = true;
        try {
            const noteIds = await noteIdsOfSelection();
            const changes = await removeNoteTags({ noteIds: noteIds.map(BigInt), tags });
            await afterAction(tr.browsingNotesUpdated({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /**
     * 清除未使用标签（`browser.py:997-1000`）。
     *
     * Qt 那边是 `col.tags.clear_unused_tags()` + 提示「清掉了 N 个」，
     * 没有确认框，这里保持一致。
     */
    async function clearUnused(): Promise<void> {
        busy = true;
        try {
            const changes = await clearUnusedTags(new Empty({}));
            await afterAction(tr.browsingRemovedUnusedTagsCount({ count: changes.count }));
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 把选中项换成它们对应的全部卡片（`browser.py:1175-1183` 的 `selectNotes`）。 */
    async function selectNotes(): Promise<void> {
        const noteIds = await noteIdsOfSelection();
        if (noteIds.length === 0) {
            return;
        }
        const built = await buildSearchString({
            filter: { case: "nids", value: { ids: noteIds.map(BigInt) } },
        });
        await loadSearch(built.val);
        selectAll();
    }

    function invertSelection(): void {
        const inverted = cards.filter((id) => !isSelected(id));
        selected = inverted;
        current = inverted.at(-1) ?? null;
        anchorIndex = null;
        updateTitle();
    }

    /** Qt 的时间戳格式（`import_export/exporting.py:179` 的 `%Y-%m-%d@%H-%M-%S`）。 */
    function timestampSuffix(): string {
        const now = new Date();
        const pad = (value: number) => `${value}`.padStart(2, "0");
        return [
            `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`,
            `${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`,
        ].join("@");
    }

    /**
     * 问宿主要一个保存路径。
     *
     * `saveFileDialog` 是**外壳新增的平台端点**（上游没有：Qt 的导出对话框直接用原生
     * `getSaveFile()`，`exporting.py:119-140`）。请求复用 `OpenFilePickerRequest`，
     * 其中 `key` 按外壳约定传建议的文件名（见 `platform/dialogs.rs` 的注释）。
     * 用户取消时返回空串。
     */
    async function askSavePath(extension: string, filterLabel: string): Promise<string> {
        const request = new openFilePickerRequest({
            title: tr.actionsExport(),
            key: `${tr.exportingCollection()}-${timestampSuffix()}.${extension}`,
            filterDescription: filterLabel,
            extensions: [extension],
        });
        const response = await postProto("saveFileDialog", request, GenericString);
        return response.val;
    }

    /** 导出（`browser.py:377` 的 `_on_export_notes` + `import_export/exporting.py`）。 */
    async function askExport(): Promise<void> {
        const noteIds = await noteIdsOfSelection();
        if (noteIds.length === 0) {
            return;
        }
        exportNoteIds = noteIds;
        exportDialog.show(noteIds.length);
    }

    /**
     * 格式 → 扩展名 / 保存框上的筛选名（对照各 `Exporter` 的 `extension` + `name()`，
     * `exporting.py:232-365`）。
     *
     * 必须是函数：`tr.*` 要等 `onMount` 里 `setupI18n` 之后才有内容，写成顶层常量会在
     * 组件初始化时就把 `missing key: …` 定下来（§5.22 / §5.26）。
     */
    function exportFormatOf(format: ExportChoice["format"]): {
        extension: string;
        label: string;
    } {
        switch (format) {
            case "apkg":
                return { extension: "apkg", label: tr.exportingAnkiDeckPackage() };
            case "colpkg":
                return { extension: "colpkg", label: tr.exportingAnkiCollectionPackage() };
            case "notes-txt":
                return { extension: "txt", label: tr.exportingNotesInPlainText() };
            case "cards-txt":
                return { extension: "txt", label: tr.exportingCardsInPlainText() };
        }
    }

    /**
     * 范围 → `ExportLimit`。
     *
     * 注意 `ExportLimit` 里还有一个同名的 `limit` oneof，构造时要嵌一层 —— 少嵌一层
     * 不会报错，而是**静默导出整个集合**（见 TAURI_HANDOVER §5.33）。这里用生成的类
     * 显式构造，让类型检查帮忙挡住这种写法。
     */
    function limitFor(scope: ExportChoice["scope"], noteIds: number[]): ExportLimit {
        return new ExportLimit({
            limit:
                scope === "selection"
                    ? { case: "noteIds", value: new NoteIds({ noteIds: noteIds.map(BigInt) }) }
                    : { case: "wholeCollection", value: new Empty({}) },
        });
    }

    /**
     * 按对话框的选择真的导出。
     *
     * 对照 `exporting.py:142-160` 的 `options()` 与四个 `Exporter.export()`：给 `nids`
     * 时 `limit` 是这些笔记，否则是整份集合；勾选项一一对应请求里的字段。
     */
    async function runExport(choice: ExportChoice): Promise<void> {
        const noteIds = exportNoteIds;
        if (noteIds.length === 0) {
            return;
        }
        const { extension, label } = exportFormatOf(choice.format);
        const path = await askSavePath(extension, label);
        if (!path) {
            return; // 用户取消
        }
        busy = true;
        try {
            switch (choice.format) {
                case "colpkg":
                    // colpkg 没有 `limit`（整个集合），见 `ColpkgExporter.export()`
                    await exportCollectionPackage({
                        outPath: path,
                        includeMedia: choice.includeMedia,
                        legacy: choice.legacy,
                    });
                    // 对照 `exporting.py:248`：`exporting_collection_exported()`
                    flash(tr.exportingCollectionExported());
                    break;
                case "apkg": {
                    const exported = await exportAnkiPackage({
                        outPath: path,
                        options: {
                            withScheduling: choice.includeScheduling,
                            withDeckConfigs: choice.includeDeckConfigs,
                            withMedia: choice.includeMedia,
                            legacy: choice.legacy,
                        },
                        limit: limitFor(choice.scope, noteIds),
                    });
                    // 对照 `exporting.py:286`：`exporting_note_exported(count=…)`
                    flash(tr.exportingNoteExported({ count: exported.val }));
                    break;
                }
                case "notes-txt": {
                    // `NoteCsvExporter.export()`（`exporting.py:317-337`）
                    const exported = await exportNoteCsv({
                        outPath: path,
                        withHtml: choice.includeHtml,
                        withTags: choice.includeTags,
                        withDeck: choice.includeDeck,
                        withNotetype: choice.includeNotetype,
                        withGuid: choice.includeGuid,
                        limit: limitFor(choice.scope, noteIds),
                    });
                    flash(tr.exportingNoteExported({ count: exported.val }));
                    break;
                }
                case "cards-txt": {
                    // `CardCsvExporter.export()`（`exporting.py:349-365`）
                    const exported = await exportCardCsv({
                        outPath: path,
                        withHtml: choice.includeHtml,
                        limit: limitFor(choice.scope, noteIds),
                    });
                    // 对照 `exporting.py:355`：`exporting_card_exported(count=…)`
                    flash(tr.exportingCardExported({ count: exported.val }));
                    break;
                }
            }
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /**
     * 更改笔记类型（`browser.py:1241-1252` 的 `onChangeNoteType`）。
     *
     * 窗口由宿主开（`change-notetype/<oldId>`，既有路由），用户在页面里把字段/模板
     * 映射到新类型后点保存：那条 RPC 由后端执行，**保存成功后宿主关窗**（见
     * `server/api.rs` 对 `changeNotetype` 的接管，对照 `mediasrv.py:772`）。
     * 选中的笔记必须是同一种笔记类型，否则宿主弹警告且不开窗。
     */
    async function openChangeNotetype(): Promise<void> {
        const noteIds = await noteIdsOfSelection();
        if (noteIds.length === 0) {
            return;
        }
        bridgeCommand(`changeNoteType:${noteIds.join(",")}`);
    }

    /**
     * 复制笔记（`browser.py:828-834`）。
     *
     * Qt 不是"复制出几条新笔记"，而是**打开「添加卡片」窗口并填上当前笔记的内容**
     * （`AddCards.set_note(note, deck_id)`），牌组取当前卡片所在的牌组。
     */
    async function createCopy(): Promise<void> {
        const noteId = await currentNoteId();
        const cardId = await currentCardId();
        if (noteId === undefined || cardId === undefined) {
            return;
        }
        const deckId = (await getCard({ cid: cardId })).deckId;
        bridgeCommand(`addCopy:${noteId}:${deckId}`);
    }

    /**
     * 「管理笔记类型」（Qt 在浏览器侧栏 Note Types 节点的右键菜单里：
     * `browser/sidebar/tree.py:924-928` 的 `browsing-manage-note-types` → `Models(...)`）。
     *
     * 本外壳的侧栏节点没有右键菜单，入口放在这里的「更多」菜单（主窗口菜单栏还没做，
     * 见 PARITY 1.2）；窗口与页面由宿主开（`pycmd("notetypes")`）。
     */
    function manageNotetypes(): void {
        bridgeCommand("notetypes");
    }

    /** 卡片信息窗口（`browser.py:799-803` → `aqt/card_info.py:62`）。 */
    async function showCardInfo(): Promise<void> {
        const cardId = await currentCardId();
        if (cardId === undefined || cardId === 0n) {
            return;
        }
        bridgeCommand(`cardInfo:${cardId}`);
    }

    /**
     * 查找替换（`qt/aqt/browser/find_and_replace.py` + `browser.ui` 里的对话框）。
     *
     * 差异：Qt 的下拉框可以选择「所有字段 / 标签 / 某个字段」，这里固定为所有字段；
     * 「只看选中的笔记」勾选项也没做（本动作本来就只作用于选中项）。
     */
    function askFindReplace(): void {
        if (selected.length === 0) {
            return;
        }
        formAction = "findReplace";
        confirmLabel = tr.browsingFindAndReplace();
        formModal.show(tr.browsingFindAndReplace(), [
            { kind: "text", label: tr.browsingFind(), value: "" },
            { kind: "text", label: tr.browsingReplaceWith(), value: "" },
            { kind: "check", label: tr.browsingTreatInputAsRegularExpression(), value: false },
            { kind: "check", label: tr.browsingIgnoreCase(), value: false },
        ]);
    }

    async function applyFindReplace(
        searchText: string,
        replacement: string,
        regex: boolean,
        matchCase: boolean,
    ): Promise<void> {
        if (!searchText) {
            return;
        }
        busy = true;
        try {
            const noteIds = await noteIdsOfSelection();
            const changes = await findAndReplace({
                nids: noteIds.map(BigInt),
                search: searchText,
                replacement,
                regex,
                matchCase,
                // 空 = 所有字段（Qt 的下拉框可以选择单个字段，这里没做）
                fieldName: "",
            });
            // 对照 `find_and_replace.py:173-177`：`findreplace_notes_updated(...)`
            await afterAction(
                tr.findreplaceNotesUpdated({
                    changed: changes.count,
                    total: noteIds.length,
                }),
            );
        } catch (problem) {
            flash(String(problem));
        } finally {
            busy = false;
        }
    }

    /** 跳转类动作（Qt 的 Jumps 菜单，`browser.py:379-397`）。 */
    function jump(offset: number | "first" | "last"): void {
        if (cards.length === 0) {
            return;
        }
        const index = current === null ? -1 : cards.indexOf(current);
        let target: number;
        if (offset === "first") {
            target = 0;
        } else if (offset === "last") {
            target = cards.length - 1;
        } else {
            target = Math.min(Math.max(index + offset, 0), cards.length - 1);
        }
        selectRow(cards[target]);
        // 选中行可能在可视区外 —— 虚拟列表用 `.outer` 做滚动容器
        const scroller = document.querySelector<HTMLElement>(".results .outer");
        if (scroller) {
            scroller.scrollTop = Math.max(0, target * itemHeight - scroller.clientHeight / 2);
        }
    }

    /** Ctrl+F 的「查找」：把焦点交给搜索框（`browser.py` 的 `onFind`）。 */
    function focusSearch(): void {
        document.querySelector<HTMLInputElement>(".search")?.focus();
    }

    // 动作菜单 ################################################################

    /**
     * 完整的动作表。
     *
     * Qt 是四个菜单栏（Notes / Cards / View / Jumps，`browser.py:301-401`）；
     * 网页没有菜单栏，所以同一份表放在行右键菜单 + 工具栏「更多」里。
     * 每条都标了 Qt 的快捷键（`qt/aqt/forms/browser.ui`），快捷键本身由
     * `onKeydown` 实现。
     */
    function menuSections(): MenuSection[] {
        const hasSelection = selected.length > 0;
        const hasCurrent = current !== null;
        const flagItems = [
            [tr.actionsFlagRed(), 1],
            [tr.actionsFlagOrange(), 2],
            [tr.actionsFlagGreen(), 3],
            [tr.actionsFlagBlue(), 4],
            [tr.actionsFlagPink(), 5],
            [tr.actionsFlagTurquoise(), 6],
            [tr.actionsFlagPurple(), 7],
        ] as const;
        return [
            {
                items: [
                    { label: qtAccel("qt-accel-undo"), shortcut: "Ctrl+Z", run: () => void undoLast() },
                    {
                        label: qtAccel("qt-accel-redo"),
                        shortcut: "Ctrl+Shift+Z",
                        run: () => void redoLast(),
                    },
                    {
                        label: qtAccel("qt-accel-invert-selection"),
                        shortcut: "Ctrl+Alt+S",
                        run: invertSelection,
                    },
                    {
                        label: qtAccel("qt-accel-select-notes"),
                        action: "selectNotes",
                        disabled: !hasSelection,
                        run: () => void selectNotes(),
                    },
                ],
            },
            {
                items: [
                    {
                        label: tr.browsingAddNotes(),
                        shortcut: "Ctrl+E",
                        run: () => bridgeCommand("add"),
                    },
                    {
                        label: tr.actionsCreateCopy(),
                        action: "createCopy",
                        shortcut: "Ctrl+Alt+E",
                        disabled: !hasCurrent,
                        run: () => void createCopy(),
                    },
                    {
                        // Qt 的 Notes 菜单里 Export 在「复制笔记」之后（`browser.ui`），
                        // 文案是 `qt-accel-export-notes`，快捷键 Ctrl+Shift+E
                        label: qtAccel("qt-accel-export-notes"),
                        action: "export",
                        shortcut: "Ctrl+Shift+E",
                        disabled: !hasSelection,
                        run: () => void askExport(),
                    },
                    {
                        label: tr.browsingAddTags2(),
                        shortcut: "Ctrl+Shift+A",
                        disabled: !hasSelection,
                        run: () => tagsModal.show("add"),
                    },
                    {
                        label: tr.browsingRemoveTags(),
                        shortcut: "Ctrl+Alt+Shift+A",
                        disabled: !hasSelection,
                        run: () => tagsModal.show("remove"),
                    },
                    {
                        label: tr.browsingClearUnusedTags(),
                        action: "clearUnusedTags",
                        run: () => void clearUnused(),
                    },
                    {
                        label: tr.browsingChangeNoteType2(),
                        action: "changeNotetype",
                        disabled: !hasSelection,
                        run: () => void openChangeNotetype(),
                    },
                    {
                        label: tr.browsingManageNoteTypes(),
                        action: "manageNotetypes",
                        run: manageNotetypes,
                    },
                    {
                        label: tr.browsingToggleMark(),
                        shortcut: "Ctrl+K",
                        checked: menuState.marked,
                        disabled: !hasSelection,
                        run: () => void toggleMark(),
                    },
                    {
                        label: tr.actionsDelete(),
                        shortcut: "Ctrl+Del",
                        disabled: !hasSelection,
                        run: () => void deleteSelected(),
                    },
                ],
            },
            {
                items: [
                    {
                        label: tr.browsingChangeDeck2(),
                        shortcut: "Ctrl+D",
                        disabled: !hasSelection,
                        run: () => deckModal.show(),
                    },
                    {
                        label: qtAccel("qt-accel-info"),
                        action: "cardInfo",
                        shortcut: "Ctrl+Shift+I",
                        disabled: !hasCurrent,
                        run: () => void showCardInfo(),
                    },
                    {
                        label: tr.browsingReposition(),
                        shortcut: "Ctrl+Shift+S",
                        disabled: !hasSelection,
                        run: () => void askReposition(),
                    },
                    {
                        label: qtAccel("qt-accel-set-due-date"),
                        shortcut: "Ctrl+Shift+D",
                        disabled: !hasSelection,
                        run: () => void askSetDueDate(),
                    },
                    {
                        // Qt 的菜单项文案是 `qt-accel-forget`（"Reset"），
                        // 但它的对话框标题用的是 `actions-forget-card`；这里用后者，
                        // 因为它才是核心仓库里的（`qt-accel-*` 没有类型化函数）
                        label: tr.actionsForgetCard(),
                        shortcut: "Ctrl+Alt+N",
                        disabled: !hasSelection,
                        run: () => void askForget(),
                    },
                    {
                        label: tr.browsingToggleSuspend(),
                        shortcut: "Ctrl+J",
                        checked: menuState.suspended,
                        disabled: !hasSelection,
                        run: () => void toggleSuspend(),
                    },
                    {
                        label: tr.browsingToggleBury(),
                        shortcut: "Ctrl+Shift+J",
                        checked: menuState.buried,
                        disabled: !hasSelection,
                        run: () => void toggleBury(),
                    },
                ],
            },
            {
                items: [
                    // Qt 把「立即评分」做成对话框（四个大按钮）；这里拆成菜单项
                    ...[
                        [tr.studyingAgain(), CardAnswer_Rating.AGAIN],
                        [tr.studyingHard(), CardAnswer_Rating.HARD],
                        [tr.studyingGood(), CardAnswer_Rating.GOOD],
                        [tr.studyingEasy(), CardAnswer_Rating.EASY],
                    ].map(([label, rating]) => ({
                        label: `${tr.actionsGradeNow()}: ${label}`,
                        action: `grade-${rating}`,
                        disabled: !hasSelection,
                        run: () => void gradeSelected(rating as CardAnswer_Rating),
                    })),
                    ...flagItems.map(([label, flag]) => ({
                        label,
                        shortcut: `Ctrl+${flag}`,
                        checked: menuState.flag === flag,
                        disabled: !hasSelection,
                        run: () => void setSelectionFlag(flag as number),
                    })),
                ],
            },
            {
                items: [
                    {
                        label: tr.actionsPreview(),
                        shortcut: "Ctrl+Shift+P",
                        run: () => void preview(),
                    },
                    {
                        label: tr.browsingToggleShowingCardsNotes(),
                        shortcut: "Ctrl+Alt+T",
                        run: () => void toggleNotesMode(),
                    },
                    {
                        label: tr.browsingFindAndReplace(),
                        action: "findReplace",
                        shortcut: "Ctrl+Alt+F",
                        disabled: !hasSelection,
                        run: askFindReplace,
                    },
                    { label: tr.browsingFind(), shortcut: "Ctrl+F", run: focusSearch },
                ],
            },
            {
                items: [
                    { label: tr.browsingFirstCard(), shortcut: "Home", run: () => jump("first") },
                    {
                        label: qtAccel("qt-accel-previous-card"),
                        shortcut: "Ctrl+P",
                        run: () => jump(-1),
                    },
                    { label: qtAccel("qt-accel-next-card"), shortcut: "Ctrl+N", run: () => jump(1) },
                    { label: tr.browsingLastCard(), shortcut: "End", run: () => jump("last") },
                ],
            },
            {
                items: [{ label: tr.actionsClose(), shortcut: "Ctrl+W", run: () => bridgeCommand("close") }],
            },
        ];
    }

    /** 打开动作菜单（行右键 / 工具栏「更多」）。 */
    async function openActionMenu(event: MouseEvent, select?: number): Promise<void> {
        event.preventDefault();
        if (select !== undefined && !isSelected(select)) {
            selectRow(select);
        }
        await refreshMenuState();
        actionMenu = { x: event.clientX, y: event.clientY };
    }

    /** 通用表单弹框的回调：按打开它的动作分发（见 `formAction`）。 */
    function onFormOk(values: FormValues): void {
        const text = (index: number) => values[index]?.text ?? "";
        const number = (index: number) => Number(values[index]?.number ?? 0);
        const check = (index: number) => values[index]?.check === true;
        switch (formAction) {
            case "due":
                void applySetDueDate(text(0));
                break;
            case "reposition":
                void applyReposition(number(0), number(1), check(2), check(3));
                break;
            case "forget":
                void applyForget(check(0), check(1));
                break;
            case "findReplace":
                void applyFindReplace(text(0), text(1), check(2), check(3));
                break;
        }
    }

    // 键盘 ####################################################################

    function inTextInput(target: EventTarget | null): boolean {
        const element = target as HTMLElement | null;
        if (!element) {
            return false;
        }
        const name = element.tagName;
        return name === "INPUT" || name === "TEXTAREA" || element.isContentEditable === true;
    }

    /**
     * 快捷键表。
     *
     * 全部对照 `qt/aqt/forms/browser.ui` 里的 `<property name="shortcut">`（Qt 那边是
     * QAction 自带的，网页要在 `keydown` 里自己实现）。`Ctrl+P`/`Ctrl+N` 在 Qt 里是
     * 「上一张/下一张卡片」，而浏览器里打印是 Ctrl+P 的常见用途 —— 上游同样占用了它。
     */
    function onKeydown(event: KeyboardEvent): void {
        if (event.key === "Escape") {
            // 弹层与选择都要能退；弹框交给 Bootstrap 自己处理
            if (actionMenu) {
                actionMenu = null;
            } else if (columnsMenu) {
                columnsMenu = null;
            } else {
                clearSelection();
            }
            return;
        }
        // 列清单/菜单里是复选框与按钮（也算 input），但这里已经处理完 Escape
        if (inTextInput(event.target)) {
            return;
        }
        const modifier = event.ctrlKey || event.metaKey;
        const key = event.key.toLowerCase();
        const shift = event.shiftKey;
        const alt = event.altKey;

        if (event.key === "Delete" || (modifier && event.key === "Del")) {
            // 对照 Qt 的删除动作（Ctrl+Del）
            event.preventDefault();
            void deleteSelected();
        } else if (!modifier && event.key === "Home") {
            event.preventDefault();
            jump("first");
        } else if (!modifier && event.key === "End") {
            event.preventDefault();
            jump("last");
        } else if (!modifier) {
            return;
        } else if (key === "z" && !alt) {
            event.preventDefault();
            void (shift ? redoLast() : undoLast());
        } else if (alt && shift && key === "a") {
            // 对照 `actionRemove_Tags`（Ctrl+Alt+Shift+A）
            event.preventDefault();
            tagsModal.show("remove");
        } else if (alt && key === "a") {
            // 对照 `actionSelectAll`（Ctrl+Alt+A）
            event.preventDefault();
            selectAll();
        } else if (alt && key === "s") {
            // 对照 `actionInvertSelection`（Ctrl+Alt+S）
            event.preventDefault();
            invertSelection();
        } else if (alt && key === "e") {
            // 对照 `actionCopy`（Ctrl+Alt+E）
            event.preventDefault();
            void createCopy();
        } else if (alt && key === "n") {
            // 对照 `action_forget`（Ctrl+Alt+N）
            event.preventDefault();
            void askForget();
        } else if (alt && key === "f") {
            // 对照 `actionFindReplace`（Ctrl+Alt+F）
            event.preventDefault();
            askFindReplace();
        } else if (alt && key === "t") {
            // 对照 `action_toggle_mode`（Ctrl+Alt+T）
            event.preventDefault();
            void toggleNotesMode();
        } else if (shift && key === "a") {
            // 对照 `browsing-add-tags`（Ctrl+Shift+A）
            event.preventDefault();
            tagsModal.show("add");
        } else if (shift && key === "e") {
            // 对照 `actionExport`（Ctrl+Shift+E，`browser.ui`）
            event.preventDefault();
            void askExport();
        } else if (shift && key === "p") {
            // 对照 Qt 的预览动作（Ctrl+Shift+P，与编辑器里的预览按钮同一个快捷键）
            event.preventDefault();
            void preview();
        } else if (shift && key === "i") {
            // 对照 `action_Info`（Ctrl+Shift+I）
            event.preventDefault();
            void showCardInfo();
        } else if (shift && key === "d") {
            // 对照 `action_set_due_date`（Ctrl+Shift+D）
            event.preventDefault();
            void askSetDueDate();
        } else if (shift && key === "s") {
            // 对照 `actionReposition`（Ctrl+Shift+S）
            event.preventDefault();
            void askReposition();
        } else if (shift && key === "j") {
            // 对照 `action_toggle_bury`（Ctrl+Shift+J）
            event.preventDefault();
            void toggleBury();
        } else if (shift && key === "g") {
            // 对照 `action_grade_now`（Ctrl+Shift+G）：Qt 弹对话框选评分，
            // 这里默认「良好」那一档的语义不方便猜，所以交给菜单
            event.preventDefault();
            void openActionMenuFromKeyboard();
        } else if (key === "k") {
            // 对照 `actionToggle_Mark`（Ctrl+K）
            event.preventDefault();
            void toggleMark();
        } else if (key === "j") {
            // 对照 `actionToggle_Suspend`（Ctrl+J）
            event.preventDefault();
            void toggleSuspend();
        } else if (key === "d") {
            // 对照 `browsing-change-deck`（Ctrl+D）
            event.preventDefault();
            deckModal.show();
        } else if (key === "e") {
            // 对照 `actionAdd`（Ctrl+E）
            event.preventDefault();
            bridgeCommand("add");
        } else if (key === "n") {
            // 对照 `actionNextCard`（Ctrl+N）
            event.preventDefault();
            jump(1);
        } else if (key === "p") {
            // 对照 `actionPreviousCard`（Ctrl+P）
            event.preventDefault();
            jump(-1);
        } else if (key === "f") {
            // 对照 `actionFind`（Ctrl+F）
            event.preventDefault();
            focusSearch();
        } else if (key === "w") {
            // 对照 `actionClose`（Ctrl+W）
            event.preventDefault();
            bridgeCommand("close");
        } else if (key === "a" && !shift && !alt) {
            // Qt 用 Ctrl+Alt+A 全选；Ctrl+A 是表格里的习惯操作，两个都留
            event.preventDefault();
            selectAll();
        } else if (/^[1-7]$/.test(key)) {
            // 对照旗标动作（Ctrl+1..Ctrl+7）
            event.preventDefault();
            void setSelectionFlag(Number(key));
        }
    }

    /** 键盘触发「立即评分」时把菜单开在工具栏按钮下方（Qt 是对话框）。 */
    async function openActionMenuFromKeyboard(): Promise<void> {
        const button = document.querySelector<HTMLElement>("#browse-more");
        const rect = button?.getBoundingClientRect();
        await refreshMenuState();
        actionMenu = { x: rect?.left ?? 40, y: (rect?.bottom ?? 40) + 4 };
    }

    onMount(async () => {
        // 模块要按页面用到的文案来选：工具栏/弹框的按钮来自 ACTIONS，标签输入框的
        // 标签来自 EDITING，其余在 BROWSING / DECKS / NOTETYPES（对照
        // `ts/routes/editor/base.ts` 的 `editorModules`）。漏了哪个模块，那个按钮就
        // 会显示 `missing-key: xxx`。
        await setupI18n({
            modules: [
                ModuleName.BROWSING,
                ModuleName.DECKS,
                ModuleName.NOTETYPES,
                ModuleName.ACTIONS,
                ModuleName.EDITING,
                ModuleName.SCHEDULING,
                ModuleName.STUDYING,
                ModuleName.FINDREPLACE,
                ModuleName.EXPORTING,
                ModuleName.QT_ACCEL,
            ],
        });
        await loadWidths();
        const stored = await getColConfig("browserTableShowNotesMode");
        notesMode = stored === true;
        allColumns = (await allBrowserColumns(new Empty({}))).columns;
        await loadColumns();
        await loadSort();
        const tree = (await deckTree({ now: BigInt(Math.floor(Date.now() / 1000)) })).children;
        decks = tree;
        deckEntries = flattenDecks(tree, "", []).map((entry) => ({
            id: entry.id,
            name: entry.fullName,
        }));
        notetypes = (await getNotetypeNames(new Empty({}))).entries;
        ready = true;
        if (pending !== undefined) {
            void loadSearch(pending);
        }
        bridgeCommand("browserReady", (response: { search?: string }) => {
            if (typeof response?.search === "string") {
                void loadSearch(response.search);
            }
        });
    });

    function flattenDecks(
        nodes: DeckTreeNode[],
        parent: string,
        out: { id: bigint; fullName: string }[],
    ): { id: bigint; fullName: string }[] {
        for (const node of nodes) {
            const fullName = parent ? `${parent}::${node.name}` : node.name;
            out.push({ id: node.deckId, fullName });
            flattenDecks(node.children, fullName, out);
        }
        return out;
    }
</script>

<svelte:window on:keydown={onKeydown} />

{#if ready}
    <div class="browser">
        <div class="top-bar">
            <input
                class="search"
                bind:value={query}
                placeholder={tr.browsingSearchBarHint()}
                on:keydown={(event) => {
                    if (event.key === "Enter") {
                        event.preventDefault();
                        void loadSearch(query);
                    }
                }}
            />
            <button
                class="clear"
                title={tr.browsingEmptyClearSearch()}
                aria-label={tr.browsingEmptyClearSearch()}
                on:click={() => {
                    query = "";
                    void loadSearch("");
                }}
            >
                ×
            </button>
        </div>

        <Toolbar
            {notesMode}
            {busy}
            selectionCount={selected.length}
            onChangeDeck={() => deckModal.show()}
            onAddTags={() => tagsModal.show()}
            onDelete={() => void deleteSelected()}
            onPreview={() => void preview()}
            onToggleNotesMode={() => void toggleNotesMode()}
            onMore={(event) => void openActionMenu(event)}
        />

        <div class="main">
            <Sidebar
                {decks}
                {notetypes}
                onDeck={(name) => void searchNode({ case: "deck", value: name })}
                onNotetype={(name) => void searchNode({ case: "note", value: name })}
                onWholeCollection={() => void loadSearch("")}
                onCurrentDeck={() => void searchNode({ case: "deck", value: "current" })}
            />
            <div class="results">
                {#if error}
                    <p class="message error">{error}</p>
                {:else if cards.length === 0}
                    <p class="message">{tr.browsingEmptyNoMatchTitle({ notesMode: "no" })}</p>
                {:else}
                    <VirtualTable
                        class="browser-table"
                        itemsCount={cards.length}
                        {itemHeight}
                        bottomOffset={statusHeight}
                    >
                        <svelte:fragment slot="headers">
                            <!-- 表头右键 = Qt「视图」菜单里的列勾选项 -->
                            <tr on:contextmenu={openColumnsMenu}>
                                {#each columns as column (column.key)}
                                    <th
                                        class:sortable={(notesMode
                                            ? column.sortingNotes
                                            : column.sortingCards) !== BrowserColumns_Sorting.NONE}
                                        class:sorted={sortColumn === column.key}
                                        style="width: {widths[column.key] ??
                                            defaultWidths[column.key] ??
                                            140}px"
                                        title={notesMode
                                            ? column.notesModeTooltip
                                            : column.cardsModeTooltip}
                                        data-column={column.key}
                                        on:click={() => {
                                            if (suppressSortClick) {
                                                suppressSortClick = false;
                                                return;
                                            }
                                            void sortByColumn(column);
                                        }}
                                        on:mousedown={(event) => startReorder(column.key, event)}
                                    >
                                        {notesMode ? column.notesModeLabel : column.cardsModeLabel}
                                        {#if sortColumn === column.key}
                                            <span class="sort-arrow"
                                                >{sortBackwards ? "▼" : "▲"}</span
                                            >
                                        {/if}
                                        <!-- 拖拽调列宽（Qt 用 QHeaderView 的 resize handle） -->
                                        <span
                                            class="resizer"
                                            role="presentation"
                                            on:mousedown={(event) => startResize(column.key, event)}
                                        ></span>
                                    </th>
                                {/each}
                            </tr>
                        </svelte:fragment>
                        <svelte:fragment slot="row" let:index>
                            <ResultRow
                                id={cards[index]}
                                columnCount={columns.length}
                                version={columnsVersion}
                                {columnKeys}
                                {widths}
                                {defaultWidths}
                                cellFonts={columns.map((column) => column.usesCellFont)}
                                selectedIds={selected}
                                isCurrent={current === cards[index]}
                                onSelect={selectRow}
                                onEdit={editRow}
                                onMenu={(id, event) => void openActionMenu(event, id)}
                            />
                        </svelte:fragment>
                    </VirtualTable>
                {/if}
            </div>
        </div>

        <div class="status">
            {notesMode
                ? tr.browsingWindowTitleNotes({ selected: selected.length, total: cards.length })
                : tr.browsingWindowTitle({ selected: selected.length, total: cards.length })}
            <span class="info">{status}</span>
            <span class="search-info">{search}</span>
        </div>
    </div>
{/if}

<DeckPickerModal bind:this={deckModal} decks={deckEntries} onPick={(id) => void moveToDeck(id)} />
<TagsModal
    bind:this={tagsModal}
    onOk={(tags, mode) => void (mode === "remove" ? removeTags(tags) : addTags(tags))}
/>
<FormModal bind:this={formModal} {confirmLabel} onOk={onFormOk} />
<ExportDialog bind:this={exportDialog} onExport={(choice) => void runExport(choice)} />

{#if actionMenu}
    <ContextMenu
        x={actionMenu.x}
        y={actionMenu.y}
        sections={menuSections()}
        onclose={() => (actionMenu = null)}
    />
{/if}

{#if columnsMenu}
    <div
        class="menu-backdrop"
        role="presentation"
        on:click={() => (columnsMenu = null)}
        on:contextmenu={() => (columnsMenu = null)}
    ></div>
    <div class="columns-menu" style="left: {columnsMenu.x}px; top: {columnsMenu.y}px">
        {#each allColumns as column (column.key)}
            <label>
                <input
                    type="checkbox"
                    checked={columnKeys.includes(column.key)}
                    on:change={() => void toggleColumn(column.key)}
                />
                {notesMode ? column.notesModeLabel : column.cardsModeLabel}
            </label>
        {/each}
    </div>
{/if}

<style lang="scss">
    .browser {
        display: flex;
        flex-direction: column;
        height: 100vh;
    }

    .top-bar {
        display: flex;
        gap: 0.5rem;
        padding: 0.5rem;
        border-bottom: 1px solid var(--border-subtle);
    }

    .search {
        flex: 1;
    }

    .clear {
        padding: 0 0.6rem;
        color: var(--fg-subtle);
        background: transparent;
        border: none;
        border-radius: var(--border-radius);

        &:hover {
            background: var(--canvas-inset);
        }
    }

    .main {
        display: flex;
        flex: 1;
        min-height: 0;
    }

    .results {
        flex: 1;
        min-width: 0;
    }

    .message {
        padding: 1rem;
        color: var(--fg-subtle);

        &.error {
            color: var(--accent-danger);
        }
    }

    .status {
        display: flex;
        gap: 1rem;
        padding: 0.25rem 0.5rem;
        color: var(--fg-subtle);
        font-size: 0.85em;
        border-top: 1px solid var(--border-subtle);

        .info {
            color: var(--fg);
        }

        .search-info {
            margin-inline-start: auto;
            font-family: monospace;
        }
    }

    /* 表头右键的列清单（图层与 Modal 一致：1065/1066） */
    .menu-backdrop {
        position: fixed;
        inset: 0;
        z-index: 1065;
    }

    .columns-menu {
        position: fixed;
        z-index: 1066;
        display: flex;
        flex-direction: column;
        gap: 0.25rem;
        max-height: 60vh;
        padding: 0.5rem;
        overflow: auto;
        color: var(--fg);
        background: var(--canvas);
        border: 1px solid var(--border-subtle);
        border-radius: var(--border-radius);

        label {
            display: flex;
            gap: 0.4rem;
            align-items: center;
            white-space: nowrap;
        }
    }

    /* 列宽来自配置，因此必须固定布局（否则浏览器会按内容重新分配宽度） */
    :global(.browser-table) {
        table-layout: fixed;
        width: 100%;
    }

    :global(.browser-table th),
    :global(.browser-table td) {
        max-width: none;
    }

    :global(.browser-table th) {
        position: relative;
        cursor: default;
        user-select: none;

        &.sortable {
            cursor: pointer;
        }

        .sort-arrow {
            margin-inline-start: 0.25rem;
        }

        .resizer {
            position: absolute;
            inset-inline-end: 0;
            inset-block: 0;
            width: 6px;
            cursor: col-resize;
        }
    }
</style>
