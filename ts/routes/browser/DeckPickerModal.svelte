<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
「更改所属牌组」对话框（Ctrl+D）。

对照 `browser.py:918-949` 的 `set_deck_of_selected_cards`：Qt 弹的是 `StudyDeck`
（一个带搜索框的牌组列表），标题 `browsing-change-deck`、确认按钮
`browsing-move-cards`。这里用同一个 Modal 组件 + 自己拼的列表实现（`$lib` 里的
`DeckChooser` 是「按钮 + 弹框」的组合件，没法只借它的弹框）。
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    export let decks: { id: bigint; name: string }[] = [];
    export let onPick: (deckId: bigint) => void;

    let modal: Modal;
    let filter = "";
    let picked: bigint | null = null;

    $: matches = filter.trim()
        ? decks.filter((deck) => deck.name.toLowerCase().includes(filter.trim().toLowerCase()))
        : decks;

    export function show(): void {
        filter = "";
        picked = null;
        modal.show();
    }

    function onOkClicked(): void {
        if (picked !== null) {
            onPick(picked);
        }
    }

    function onShown(): void {
        // 默认选中第一个匹配项，方便直接回车确认
        picked = matches[0]?.id ?? null;
    }
</script>

<Modal bind:this={modal} modalKey="browse-change-deck" {onOkClicked} {onShown}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title" id="modalLabel">{tr.browsingChangeDeck()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label for="deck-filter" class="col-form-label">{tr.browsingMoveCardsToDeck()}</label>
        <input id="deck-filter" class="form-control" bind:value={filter} />
        <ul class="deck-list">
            {#each matches as deck (deck.id)}
                <li>
                    <button
                        class:selected={picked === deck.id}
                        on:click={() => (picked = deck.id)}>{deck.name}</button
                    >
                </li>
            {/each}
        </ul>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={modal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button
            type="button"
            class="btn btn-primary"
            disabled={picked === null}
            on:click={modal.acceptHandler}>{tr.browsingMoveCards()}</button
        >
    </div>
</Modal>

<style lang="scss">
    .deck-list {
        max-height: 40vh;
        margin-top: 0.5rem;
        padding: 0;
        overflow: auto;
        list-style: none;

        button {
            width: 100%;
            padding: 0.25rem 0.5rem;
            color: var(--fg);
            text-align: start;
            background: transparent;
            border: none;
            border-radius: var(--border-radius);

            &:hover {
                background: var(--canvas-inset);
            }

            &.selected {
                background: var(--selected-bg);
                color: var(--selected-fg);
            }
        }
    }
</style>
