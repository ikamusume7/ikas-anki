<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
通用表单弹框：`set due date` / `reposition` / `reset` / `find & replace` 共用。

它们的字段都是「几个输入 + 几个勾选」，为每个动作单独写一个组件不划算；字段与按钮
文案由调用方给（都来自 ftl），对照的上游对话框是：

| 动作 | 上游对话框 |
| --- | --- |
| 设置到期日 | `aqt/operations/scheduling.py:30-62`（`getText`） |
| 重新定位 | `aqt/forms/reposition.ui` |
| 重置（Forget） | `aqt/forms/forget.ui` |
| 查找替换 | `qt/aqt/browser/find_and_replace.py` |
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    import type { FormField, FormValues } from "./menu";

    export let confirmLabel = "";
    export let onOk: (values: FormValues) => void;

    let modal: Modal;
    let title = "";
    let fields: FormField[] = [];

    /** 打开弹框；`next` 决定字段与标题（每次都重建，避免残留上一次的输入）。 */
    export function show(nextTitle: string, next: FormField[]): void {
        title = nextTitle;
        fields = next;
        modal.show();
    }

    /** 一项输入 → 交回给调用方的载荷（三个键都填齐，调用方按字段顺序取用）。 */
    function payloadFor(field: FormField): FormValues[number] {
        if (field.kind === "check") {
            return { check: field.value, number: 0, text: "" };
        }
        if (field.kind === "number") {
            return { number: field.value, check: false, text: "" };
        }
        return { text: field.value, check: false, number: 0 };
    }

    function onOkClicked(): void {
        onOk(fields.map(payloadFor));
    }
</script>

<Modal bind:this={modal} modalKey="browse-form" {onOkClicked}>
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
        {#each fields as field, index (index)}
            {#if field.kind === "check"}
                <div class="form-check">
                    <input
                        class="form-check-input"
                        type="checkbox"
                        id="browse-field-{index}"
                        bind:checked={field.value}
                    />
                    <label class="form-check-label" for="browse-field-{index}">
                        {field.label}
                    </label>
                </div>
            {:else}
                <div class="mb-2">
                    <label class="col-form-label" for="browse-field-{index}">
                        {field.label}
                    </label>
                    <input
                        id="browse-field-{index}"
                        class="form-control"
                        type={field.kind === "number" ? "number" : "text"}
                        min={field.kind === "number" ? (field.min ?? 0) : undefined}
                        bind:value={field.value}
                    />
                </div>
            {/if}
        {/each}
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
