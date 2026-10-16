<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import "../deck-options/deck-options-base.scss";
    import { askUser } from "@generated/backend";
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { onMount } from "svelte";

    let profiles: string[] = [];
    let current = "";
    let status = "";
    let loading = true;

    // 新建 / 重命名共用一个输入弹框（Qt 用 `askUserDialog` 的单行输入版）
    let promptOpen = false;
    let promptTitle = "";
    let promptValue = "";
    let promptAction: "create" | "rename" = "create";
    let promptFrom = "";

    const text = (key: string): string => translateRaw(key);

    /** 宿主返回的错误代号 → 文案（对照 `profiles.py` / `qt-misc` 的几张表）。 */
    function errorText(code: string): string {
        switch (code) {
            case "name-exists":
                return text("qt-misc-name-exists");
            case "invalid-char":
                return text("qt-misc-the-following-character-can-not-be");
            case "folder-exists":
                return text("profiles-folder-already-exists");
            case "last-profile":
                return text("qt-misc-there-must-be-at-least-one");
            case "missing":
                return text("profiles-anki-could-not-read-your-profile");
            default:
                return code;
        }
    }

    /** 发一条 `profile:<json>` 命令，返回宿主给的 JSON。 */
    function sendProfile(request: Record<string, unknown>): Promise<{
        ok: boolean;
        error?: string;
        current?: string;
        profiles?: string[];
    }> {
        return new Promise((resolve) => {
            bridgeCommand(`profile:${JSON.stringify(request)}`, (result: unknown) => {
                if (result && typeof result === "object") {
                    resolve(result as { ok: boolean; error?: string });
                } else {
                    resolve({ ok: false, error: String(result) });
                }
            });
        });
    }

    async function reload(): Promise<void> {
        const result = await sendProfile({ action: "list" });
        if (result.ok && result.profiles) {
            profiles = result.profiles;
            current = result.current ?? "";
        } else {
            status = result.error ?? "";
        }
        loading = false;
    }

    onMount(async () => {
        await setupI18n({
            modules: [
                ModuleName.PROFILES,
                ModuleName.ACTIONS,
                ModuleName.QT_MISC,
                ModuleName.QT_ACCEL,
                ModuleName.DECKS,
            ],
        });
        await reload();
    });

    async function open(name: string): Promise<void> {
        status = "";
        const result = await sendProfile({ action: "switch", name });
        // 成功时宿主要重启进程，这个窗口会被关掉；失败才需要提示
        if (!result.ok) {
            status = errorText(result.error ?? "");
        }
    }

    function startCreate(): void {
        promptAction = "create";
        promptTitle = text("actions-add");
        promptValue = "";
        promptOpen = true;
    }

    function startRename(name: string): void {
        promptAction = "rename";
        promptTitle = text("actions-rename");
        promptValue = name;
        promptFrom = name;
        promptOpen = true;
    }

    async function confirmPrompt(): Promise<void> {
        const value = promptValue.trim();
        promptOpen = false;
        if (!value) {
            return;
        }
        const request =
            promptAction === "create"
                ? { action: "create", name: value }
                : { action: "rename", name: promptFrom, to: value };
        const result = await sendProfile(request);
        if (result.ok) {
            status = "";
            await reload();
        } else {
            status = errorText(result.error ?? "");
        }
    }

    async function remove(name: string): Promise<void> {
        // 对照 `profiles.py:409-422` 的确认框
        const confirmed = (await askUser({ text: text("qt-misc-all-cards-notes-and-media-for") }))
            .val;
        if (!confirmed) {
            return;
        }
        const result = await sendProfile({ action: "delete", name });
        if (result.ok) {
            status = "";
            await reload();
        } else {
            status = errorText(result.error ?? "");
        }
    }

    async function openBackups(): Promise<void> {
        const result = await sendProfile({ action: "open_backups" });
        if (!result.ok) {
            status = errorText(result.error ?? "");
        }
    }

    function quit(): void {
        bridgeCommand("quit");
    }
</script>

<div class="profiles">
    <h2>{text("profiles-profiles")}</h2>

    {#if loading}
        <p>…</p>
    {:else}
        <ul>
            {#each profiles as name (name)}
                <li
                    class:current={name === current}
                    data-profile={name}
                    data-current={name === current ? "1" : "0"}
                >
                    <button class="name" on:click={() => open(name)}>{name}</button>
                    {#if name === current}
                        <span class="badge" title={text("profiles-profiles")}>✓</span>
                    {/if}
                    <button
                        class="icon"
                        data-action={`rename:${name}`}
                        title={text("actions-rename")}
                        on:click={() => startRename(name)}>✎</button
                    >
                    <button
                        class="icon"
                        data-action={`delete:${name}`}
                        title={text("actions-delete")}
                        on:click={() => remove(name)}>🗑</button
                    >
                </li>
            {/each}
        </ul>

        <div class="buttons">
            <button id="profiles-open" on:click={() => current && open(current)}>
                {text("profiles-open")}
            </button>
            <button id="profiles-add" on:click={startCreate}>{text("actions-add")}</button>
            <button id="profiles-backup" on:click={openBackups}>
                {text("profiles-open-backup")}
            </button>
            <button id="profiles-quit" on:click={quit}>{text("profiles-quit")}</button>
        </div>
    {/if}

    <p class="status" id="profiles-status">{status}</p>
    <p class="note">⚠️ 打开另一个配置会**重启外壳**（Qt 是关集合 + 重开，效果相同）；
        删除会把整个 profile 目录移进回收站。</p>
</div>

{#if promptOpen}
    <!-- 自己画一个最小弹层（不用 bootstrap Modal，避免 SvelteKit 路由里多一层依赖） -->
    <div class="overlay">
        <div class="prompt">
            <h3>{promptTitle}</h3>
            <input id="profile-name" type="text" bind:value={promptValue} />
            <div class="prompt-buttons">
                <button on:click={() => (promptOpen = false)}>{text("actions-cancel")}</button>
                <button class="primary" id="profile-name-ok" on:click={confirmPrompt}
                    >{text("actions-save")}</button
                >
            </div>
        </div>
    </div>
{/if}

<style lang="scss">
    .profiles {
        padding: 1em;
        height: 100vh;
        box-sizing: border-box;
        color: var(--fg);
        background: var(--canvas);
    }

    ul {
        list-style: none;
        margin: 0;
        padding: 0;
        max-width: 32em;
        border: 1px solid var(--border);
        border-radius: var(--border-radius);
    }

    li {
        display: flex;
        align-items: center;
        gap: 0.4em;
        padding: 0.4em 0.6em;
        border-bottom: 1px solid var(--border);

        &:last-child {
            border-bottom: none;
        }

        &.current {
            background: var(--canvas-elevated, rgba(128, 128, 128, 0.15));
            font-weight: bold;
        }
    }

    .name {
        flex: 1;
        text-align: start;
        background: transparent;
        border: none;
        color: inherit;
        font: inherit;
        cursor: pointer;
    }

    .badge {
        font-size: 0.75em;
        opacity: 0.7;
        font-weight: normal;
    }

    .icon {
        background: transparent;
        border: none;
        color: inherit;
        cursor: pointer;
    }

    .buttons {
        display: flex;
        flex-wrap: wrap;
        gap: 0.5em;
        margin-top: 1em;
    }

    button {
        border: 1px solid var(--border);
        border-radius: var(--border-radius);
        background: var(--canvas);
        color: var(--fg);
        padding: 0.35em 0.9em;
        cursor: pointer;

        &.primary {
            font-weight: bold;
        }
    }

    .status {
        color: var(--danger, #b00);
        min-height: 1.4em;
    }

    .note {
        opacity: 0.7;
        font-size: 0.85em;
        max-width: 32em;
    }

    .overlay {
        position: fixed;
        inset: 0;
        display: flex;
        align-items: center;
        justify-content: center;
        background: rgba(0, 0, 0, 0.35);
        z-index: 10;
    }

    .prompt {
        display: flex;
        flex-direction: column;
        gap: 0.6em;
        padding: 1em;
        min-width: 20em;
        color: var(--fg);
        background: var(--canvas);

        input {
            padding: 0.3em 0.5em;
            color: var(--fg);
            background: var(--canvas);
            border: 1px solid var(--border);
            border-radius: var(--border-radius);
        }
    }

    .prompt-buttons {
        display: flex;
        justify-content: flex-end;
        gap: 0.5em;
    }
</style>
