<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import type { DeckTreeNode } from "@generated/anki/decks_pb";
    import * as tr from "@generated/ftl";

    export let decks: DeckTreeNode[] = [];
    export let notetypes: { id: bigint; name: string }[] = [];
    export let onDeck: (deckName: string) => void;
    export let onNotetype: (notetypeName: string) => void;
    export let onWholeCollection: () => void;
    export let onCurrentDeck: () => void;

    interface DeckEntry {
        /** 完整名（`父::子`）：搜索用这个，对照 `sidebar/tree.py:817` 的 `deck=head + node.name` */
        fullName: string;
        /** 显示名（只有自己那一段） */
        basename: string;
        depth: number;
        counts: string;
    }

    function flatten(
        nodes: DeckTreeNode[],
        parent: string,
        depth: number,
        out: DeckEntry[],
    ): DeckEntry[] {
        for (const node of nodes) {
            const fullName = parent ? `${parent}::${node.name}` : node.name;
            out.push({
                fullName,
                basename: node.name,
                depth,
                counts: `${node.newCount} + ${node.learnCount} + ${node.reviewCount}`,
            });
            flatten(node.children, fullName, depth + 1, out);
        }
        return out;
    }

    $: entries = flatten(decks, "", 0, []);
</script>

<div class="sidebar">
    <p class="section">{tr.browsingSidebarDecks()}</p>
    <button class="item" on:click={onCurrentDeck}>{tr.browsingCurrentDeck()}</button>
    <button class="item" on:click={onWholeCollection}>{tr.browsingWholeCollection()}</button>
    {#each entries as entry}
        <button
            class="item deck"
            style="padding-inline-start: {0.6 + entry.depth * 0.9}rem"
            title={entry.fullName}
            on:click={() => onDeck(entry.fullName)}
        >
            <span class="name">{entry.basename}</span>
            <span class="counts">{entry.counts}</span>
        </button>
    {/each}

    <p class="section">{tr.browsingSidebarNotetypes()}</p>
    {#each notetypes as notetype}
        <button
            class="item"
            title={notetype.name}
            on:click={() => onNotetype(notetype.name)}
        >
            <span class="name">{notetype.name}</span>
        </button>
    {/each}
</div>

<style lang="scss">
    .sidebar {
        width: 15rem;
        overflow: auto;
        padding: 0.25rem;
        border-inline-end: 1px solid var(--border-subtle);
    }

    .section {
        margin: 0.5rem 0 0.25rem;
        padding: 0 0.4rem;
        color: var(--fg-subtle);
        font-size: 0.85em;
    }

    .item {
        display: flex;
        gap: 0.5rem;
        align-items: baseline;
        width: 100%;
        padding: 0.25rem 0.4rem;
        color: var(--fg);
        text-align: start;
        background: transparent;
        border: none;
        border-radius: var(--border-radius);

        &:hover {
            background: var(--canvas-inset);
        }

        .name {
            overflow: hidden;
            text-overflow: ellipsis;
            white-space: nowrap;
        }

        .counts {
            margin-inline-start: auto;
            color: var(--fg-subtle);
            font-size: 0.85em;
        }
    }
</style>
