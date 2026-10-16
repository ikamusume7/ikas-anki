<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import * as tr from "@generated/ftl";

    import { onMount } from "svelte";
    import DeckOptionsPage from "../DeckOptionsPage.svelte";
    import { commitEditing } from "../lib";
    import type { PageData } from "./$types";
    import { askUser, deckOptionsRequireClose, deckOptionsReady } from "@generated/backend";

    export let data: PageData;
    let page: DeckOptionsPage;

    globalThis.anki ||= {};
    globalThis.anki.deckOptionsPendingChanges = async (): Promise<void> => {
        await commitEditing();
        if (!(await data.state.isModified())) {
            // 没有改动，直接关
            deckOptionsRequireClose({});
            return;
        }
        // 有改动：问一句。**走宿主的 `askUser`** 而不是页面里的 `confirm()`——
        // WebView2 也能弹原生 confirm，但那样自动化没法回答它（`ANKI_CONFIRM_YES`
        // 是宿主弹框的钩子），而且确认框统一由宿主弹更符合本外壳的其它流程。
        // 文案与上游一致（`deckoptions.py` 里对应的 ftl key 是
        // `card-templates-discard-changes`）。
        const discard = await askUser({
            text: tr.cardTemplatesDiscardChanges(),
            title: tr.actionsClose(),
        });
        if (discard.val) {
            deckOptionsRequireClose({});
        }
    };

    onMount(() => {
        globalThis.$deckOptions = new Promise((resolve, _reject) => {
            resolve(page);
        });
        data.state.resolveOriginalConfigs();
        deckOptionsReady({});
    });
</script>

<DeckOptionsPage state={data.state} bind:this={page} />
