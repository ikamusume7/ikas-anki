<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
标签对话框：加标签（Ctrl+Shift+A）与移除标签（Ctrl+Alt+Shift+A）共用。

对照 `browser.py:961-995`：Qt 两处都弹 `TagEdit`（`utils.getTag`），只在
「问句」上不同 —— 加是 `browsing-enter-tags-to-add`、删是
`browsing-enter-tags-to-delete`。标签用空格分隔，多余的空白会被规范化。
补全与最近标签未实现（见 README 的差异表）。

按钮文案：Qt 的确认按钮是 Qt 自带的标准键（"OK"），仓库里没有对应的核心 ftl key，
所以这里用动作名本身（`actions-add` / `browsing-remove-tags`）。
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    export let onOk: (tags: string, mode: "add" | "remove") => void;

    let modal: Modal;
    let inputRef: HTMLInputElement;
    let tags = "";
    let mode: "add" | "remove" = "add";

    $: title = mode === "add" ? tr.browsingAddTags2() : tr.browsingRemoveTags();
    $: prompt =
        mode === "add" ? tr.browsingEnterTagsToAdd() : tr.browsingEnterTagsToDelete();
    $: confirmLabel = mode === "add" ? tr.actionsAdd() : tr.browsingRemoveTags();

    export function show(next: "add" | "remove" = "add"): void {
        mode = next;
        tags = "";
        modal.show();
    }

    function onOkClicked(): void {
        // 对照 `TagEdit`：多个标签用空格分隔，多余的空白会被规范化
        onOk(
            tags.trim().split(/\s+/).filter((tag) => tag.length > 0).join(" "),
            mode,
        );
    }

    function onShown(): void {
        inputRef?.focus();
    }
</script>

<Modal bind:this={modal} modalKey="browse-tags-{mode}" {onOkClicked} {onShown}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title" id="modalLabel">{title}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <form on:submit|preventDefault={modal.acceptHandler}>
            <label for="tags-input" class="col-form-label">{prompt}</label>
            <input id="tags-input" bind:this={inputRef} class="form-control" bind:value={tags} />
        </form>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={modal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={modal.acceptHandler}>
            {confirmLabel}
        </button>
    </div>
</Modal>
