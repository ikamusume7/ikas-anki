<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
导出对话框（浏览器的「更多」菜单 → 导出…）。

对照 `qt/aqt/import_export/exporting.py` 的 `ExportDialog` + `aqt/forms/exporting.ui`：
格式下拉、范围下拉（选中的笔记 / 全部牌组）、四个勾选项（含排程 / 含牌组预设 /
含媒体 / 支持旧版 Anki），选项按格式显隐（`exporter_changed()`，`exporting.py:98-111`）。

差异：
- 只做了 **apkg**（Anki 牌组包）与 **colpkg**（整个集合）两种格式；上游客还有
  「笔记纯文本 / 卡片 CSV」，它们要先选列（`getCsvMetadata`），是另一套对话框。
- 范围只有「选中的笔记 / 全部集合」两档（上游客的牌组下拉可以逐个牌组导出）。
- 确认按钮点了以后由页面去问宿主要保存路径（`saveFileDialog`），再调导出 RPC；
  上游是原生 QFileDialog。
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    import type { ExportChoice } from "./menu";

    export let onExport: (choice: ExportChoice) => void;

    let modal: Modal;
    let format: ExportChoice["format"] = "apkg";
    let scope: ExportChoice["scope"] = "selection";
    let includeScheduling = true;
    let includeDeckConfigs = true;
    let includeMedia = true;
    let legacy = false;
    /** 选中的笔记数（用来给「选中的笔记」加个数量，对照 Qt 的 deck 下拉） */
    let selectedCount = 0;

    /** colpkg 是整个集合，没有范围可选（`ColpkgExporter.show_deck_list = False`）。 */
    $: hasScope = format === "apkg";

    export function show(count: number): void {
        selectedCount = count;
        scope = "selection";
        format = "apkg";
        includeScheduling = true;
        includeDeckConfigs = true;
        includeMedia = true;
        legacy = false;
        modal.show();
    }

    /** 切格式时把范围拉回来（colpkg 只能是整个集合）。 */
    function onFormatChanged(): void {
        if (!hasScope) {
            scope = "all";
        }
    }

    function onOkClicked(): void {
        onExport({
            format,
            scope: hasScope ? scope : "all",
            includeScheduling: hasScope && includeScheduling,
            includeDeckConfigs: hasScope && includeDeckConfigs,
            includeMedia,
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
            </select>
        </div>

        {#if hasScope}
            <div class="mb-2">
                <label class="col-form-label" for="export-scope">{plain(tr.exportingInclude())}</label>
                <select id="export-scope" class="form-select" bind:value={scope}>
                    <option value="selection">
                        {tr.exportingSelectedNotes()}（{selectedCount}）
                    </option>
                    <option value="all">{tr.exportingAllDecks()}</option>
                </select>
            </div>
        {/if}

        {#if hasScope}
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
