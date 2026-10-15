<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import Shortcut from "$lib/components/Shortcut.svelte";
    import { saveCustomColours } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { tick } from "svelte";

    export let keyCombination: string | null = null;
    export let value: string;

    let inputRef: HTMLInputElement;

    /**
     * 宿主有没有原生取色器（Windows 上由 `tauri/src/bridge.rs` 注入）。
     *
     * WebView2 的 `<input type="color">` 是 Chromium 自带的取色器，宿主拿不到它的自定义
     * 色槽 —— 上游的调色板（集合配置 `customColorPickerPalette`）就没法更新。所以在
     * 有原生取色器的宿主上改走 `pickColour:` 桥接命令，与 Qt/mediasrv 的取色器等价。
     */
    const hostColourPicker =
        (window as { __ankiHostColourPicker?: boolean }).__ankiHostColourPicker === true;

    function onClick(event: MouseEvent): void {
        // 上游的 `<input type="color">` 在 click 时就保存一次对话框色槽
        saveCustomColours({});
        if (!hostColourPicker) {
            return;
        }
        event.preventDefault();
        bridgeCommand<string>(`pickColour:${value}`, async (chosen) => {
            if (!chosen || chosen === value) {
                return;
            }
            value = chosen;
            // 父组件的 `setColor` 读的是 `event.currentTarget.value`，所以要先把 DOM 的
            // value 落下去（Svelte 的 prop 更新是异步的），再补发事件
            inputRef.value = chosen;
            await tick();
            // 程序改 value 不会触发 input/change，而父组件靠这两个事件上色
            inputRef.dispatchEvent(new Event("input", { bubbles: true }));
            inputRef.dispatchEvent(new Event("change", { bubbles: true }));
        });
    }
</script>

<input
    bind:this={inputRef}
    tabindex="-1"
    type="color"
    bind:value
    on:input
    on:change
    on:click={onClick}
/>

{#if keyCombination}
    <Shortcut {keyCombination} on:action={() => inputRef.click()} />
{/if}

<style lang="scss">
    input {
        display: inline-block;
        width: 100%;
        height: 100%;
        cursor: pointer;
        opacity: 0;
    }
</style>
