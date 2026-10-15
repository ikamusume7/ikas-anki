<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
导出对话框（浏览器的「更多」菜单 → 导出…）。

对照 `qt/aqt/import_export/exporting.py` 的 `ExportDialog` + `aqt/forms/exporting.ui`：
格式下拉（apkg / colpkg / 笔记纯文本 / 卡片纯文本）、范围下拉（选中的笔记 / 全部集合）、
按格式显隐的勾选项（`exporter_changed()`，`exporting.py:98-111`）。

格式 ↔ 选项（各 `Exporter` 的 `show_*` 开关，`exporting.py:232-365`）：

| 格式 | 范围 | 排程 | 牌组预设 | 媒体 | HTML | 标签 | 牌组名 | 笔记类型 | GUID | 旧版 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| apkg | ✓ | ✓ | ✓ | ✓ | | | | | | ✓ |
| colpkg | | | | ✓ | | | | | | ✓ |
| notes-txt | ✓ | | | | ✓ | ✓ | ✓ | ✓ | ✓ | |
| cards-txt | ✓ | | | | ✓ | | | | | |

勾选项的初值照 `forms/exporting.ui`（`includeHTML` 由 `setup()` 额外置 true，
`exporting.py:88`）：排程 / 媒体 / HTML / 标签 = true，牌组预设 / 牌组名 / 笔记类型 /
GUID / 旧版 = false。

差异：
- 范围下拉：从浏览器打开是「选中的笔记 / 全部集合」，从**牌组列表的齿轮**打开是
  「<牌组名> / 全部集合」（上游是一个列着所有牌组的下拉，可以在这里换成别的牌组）；
- 「笔记纯文本 / 卡片纯文本」的列由**后端按勾选项决定**（`rslib/src/import_export/
  text/csv/export.rs`），没有另做一套选列对话框；
- 确认按钮点了以后由页面去问宿主要保存路径（`saveFileDialog`），再调导出 RPC；
  上游是原生 QFileDialog。
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    import type { ExportChoice } from "./menu";

    export let onExport: (choice: ExportChoice) => void;

    /** 一个格式显示哪些选项（对照各 `Exporter` 的 `show_*` 开关）。 */
    interface FormatOptions {
        scope: boolean;
        sched: boolean;
        deckConfigs: boolean;
        media: boolean;
        html: boolean;
        tags: boolean;
        deck: boolean;
        notetype: boolean;
        guid: boolean;
        legacy: boolean;
    }

    const formatOptions: Record<ExportChoice["format"], FormatOptions> = {
        apkg: {
            scope: true,
            sched: true,
            deckConfigs: true,
            media: true,
            html: false,
            tags: false,
            deck: false,
            notetype: false,
            guid: false,
            legacy: true,
        },
        colpkg: {
            scope: false,
            sched: false,
            deckConfigs: false,
            media: true,
            html: false,
            tags: false,
            deck: false,
            notetype: false,
            guid: false,
            legacy: true,
        },
        "notes-txt": {
            scope: true,
            sched: false,
            deckConfigs: false,
            media: false,
            html: true,
            tags: true,
            deck: true,
            notetype: true,
            guid: true,
            legacy: false,
        },
        "cards-txt": {
            scope: true,
            sched: false,
            deckConfigs: false,
            media: false,
            html: true,
            tags: false,
            deck: false,
            notetype: false,
            guid: false,
            legacy: false,
        },
    };

    let modal: Modal;
    let format: ExportChoice["format"] = "apkg";
    let scope: ExportChoice["scope"] = "selection";
    let includeScheduling = true;
    let includeDeckConfigs = false;
    let includeMedia = true;
    let includeHtml = true;
    let includeTags = true;
    let includeDeck = false;
    let includeNotetype = false;
    let includeGuid = false;
    let legacy = false;
    /** 选中的笔记数（用来给「选中的笔记」加个数量，对照 Qt 的 deck 下拉） */
    let selectedCount = 0;
    /**
     * 范围下拉的选项：从浏览器打开是「选中的笔记 / 全部集合」，从牌组列表的齿轮打开是
     * 「<牌组名> / 全部集合」（对照 `exporting.py:78-84` 的 deck 下拉）。
     *
     * 在 `show()` 里算 —— `tr.*` 要等 `setupI18n` 之后才有内容（§5.22 / §5.26）。
     */
    let scopeChoices: { value: ExportChoice["scope"]; label: string }[] = [];

    $: options = formatOptions[format];

    export function show(
        choice: { selectedCount?: number; deck?: { id: bigint; name: string } } = {},
    ): void {
        selectedCount = choice.selectedCount ?? 0;
        // 初值照 `exporting.ui` + `exporting.py:88`（每次打开都重置）
        format = "apkg";
        scope = choice.deck ? "deck" : "selection";
        scopeChoices = choice.deck
            ? [
                  { value: "deck", label: choice.deck.name },
                  { value: "all", label: tr.exportingAllDecks() },
              ]
            : [
                  {
                      value: "selection",
                      label: `${tr.exportingSelectedNotes()}（${selectedCount}）`,
                  },
                  { value: "all", label: tr.exportingAllDecks() },
              ];
        includeScheduling = true;
        includeDeckConfigs = false;
        includeMedia = true;
        includeHtml = true;
        includeTags = true;
        includeDeck = false;
        includeNotetype = false;
        includeGuid = false;
        legacy = false;
        modal.show();
    }

    /** 切格式时把范围拉回来（colpkg 没有牌组下拉，只能是整个集合）。 */
    function onFormatChanged(): void {
        if (!options.scope) {
            scope = "all";
        }
    }

    function onOkClicked(): void {
        onExport({
            format,
            scope: options.scope ? scope : "all",
            includeScheduling,
            includeDeckConfigs,
            includeMedia,
            includeHtml,
            includeTags,
            includeDeck,
            includeNotetype,
            includeGuid,
            legacy,
        });
    }

    /** `exporting-export-format` 之类的文案自带 `<b>` 标签（Qt 里是富文本）。 */
    const plain = (label: string): string => label.replace(/<[^>]*>/g, "");
</script>

<Modal bind:this={modal} modalKey="browse-export" {onOkClicked}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title" id="modalLabel">{tr.actionsExport()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <div class="mb-2">
            <label class="col-form-label" for="export-format">{plain(tr.exportingExportFormat())}</label>
            <select id="export-format" class="form-select" bind:value={format} on:change={onFormatChanged}>
                <option value="apkg">{tr.exportingAnkiDeckPackage()} (.apkg)</option>
                <option value="colpkg">{tr.exportingAnkiCollectionPackage()} (.colpkg)</option>
                <option value="notes-txt">{tr.exportingNotesInPlainText()} (.txt)</option>
                <option value="cards-txt">{tr.exportingCardsInPlainText()} (.txt)</option>
            </select>
        </div>

        {#if options.scope}
            <div class="mb-2">
                <label class="col-form-label" for="export-scope">{plain(tr.exportingInclude())}</label>
                <select id="export-scope" class="form-select" bind:value={scope}>
                    {#each scopeChoices as choice (choice.value)}
                        <option value={choice.value}>{choice.label}</option>
                    {/each}
                </select>
            </div>
        {/if}

        {#if options.sched}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-sched"
                    bind:checked={includeScheduling}
                />
                <label class="form-check-label" for="export-sched">
                    {tr.exportingIncludeSchedulingInformation()}
                </label>
            </div>
        {/if}
        {#if options.deckConfigs}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-deck-configs"
                    bind:checked={includeDeckConfigs}
                />
                <label class="form-check-label" for="export-deck-configs">
                    {tr.exportingIncludeDeckConfigs()}
                </label>
            </div>
        {/if}
        {#if options.media}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-media"
                    bind:checked={includeMedia}
                />
                <label class="form-check-label" for="export-media">
                    {tr.exportingIncludeMedia()}
                </label>
            </div>
        {/if}
        {#if options.html}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-html"
                    bind:checked={includeHtml}
                />
                <label class="form-check-label" for="export-html">
                    {tr.exportingIncludeHtmlAndMediaReferences()}
                </label>
            </div>
        {/if}
        {#if options.tags}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-tags"
                    bind:checked={includeTags}
                />
                <label class="form-check-label" for="export-tags">
                    {tr.exportingIncludeTags()}
                </label>
            </div>
        {/if}
        {#if options.deck}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-deck"
                    bind:checked={includeDeck}
                />
                <label class="form-check-label" for="export-deck">
                    {tr.exportingIncludeDeck()}
                </label>
            </div>
        {/if}
        {#if options.notetype}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-notetype"
                    bind:checked={includeNotetype}
                />
                <label class="form-check-label" for="export-notetype">
                    {tr.exportingIncludeNotetype()}
                </label>
            </div>
        {/if}
        {#if options.guid}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-guid"
                    bind:checked={includeGuid}
                />
                <label class="form-check-label" for="export-guid">
                    {tr.exportingIncludeGuid()}
                </label>
            </div>
        {/if}
        {#if options.legacy}
            <div class="form-check">
                <input
                    class="form-check-input"
                    type="checkbox"
                    id="export-legacy"
                    bind:checked={legacy}
                />
                <label class="form-check-label" for="export-legacy">
                    {tr.exportingSupportOlderAnkiVersions()}
                </label>
            </div>
        {/if}
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={modal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            id="export-confirm"
            type="button"
            class="btn btn-primary"
            on:click={modal.acceptHandler}>{tr.exportingExport()}</button
        >
    </div>
</Modal>
