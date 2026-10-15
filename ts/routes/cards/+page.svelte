<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
「卡片…」对话框（`ts/routes/cards/`，对照 `qt/aqt/clayout.py` 的 `CardLayout`）。

界面骨架照抄 Qt（`clayout.py:84-89`）：顶部「卡片类型: [下拉] [选项 ▾]」，
中间左边编辑区（正面模板 / 背面模板 / 样式，用单选切换 —— Qt 用的是 QRadioButton，
不是页签）、右边预览；底部按钮行（添加字段 / 翻转 / 保存 / 取消）。

- 编辑区是**纯文本**（Qt 也是 `setAcceptRichText(False)`，`clayout.py:273`）；
- 「添加字段」按 `clayout.py:914-923` 往当前编辑位置追加
  `<div style='font-family: "Arial"; font-size: 20px;'>{{字段}}</div>`（Qt 会先弹一个
  选字段+字体的对话框，这里同样弹一个）；
- 预览走 `RenderUncommittedCard`（`clayout.py:533-541` 的
  `note.ephemeral_card(ord, custom_note_type=..., custom_template=...)`）——
  用的是**还没保存**的模板与样式，所以改一个字就能看到效果；
- 保存（`updateNotetype`，对照 `clayout.py:938` 的 `update_notetype_legacy`）后关窗；
  取消时有改动先问一句（`card-templates-discard-changes`，`clayout.py:942-964`）。

数据全部由本页面调 RPC 取：`getNotetype`（笔记类型）、`getNote`（预览用的那张笔记，
宿主通过 `?note=` 传来；没有就造一张空笔记，与 Qt 新建笔记时 `Note(col, mid)` 一致）、
`getNotetypeNamesAndCounts`（「有 N 张笔记用到它」的提示）、`getDeckNames`（牌组覆盖）、
`searchCards`（删除卡片类型时数一下有多少张卡）。

两个上游行为，用的时候会遇到：

- 「添加卡片类型」复制的是**当前**模板（`clayout.py:707-711`），所以保存前要把它改得
  不一样 —— 后端会拒绝两张正面完全相同的模板（"The front side is identical to card
  template N"），Qt 那边同样保存不了；
- 「删除卡片类型」的确认框里带的是卡片张数（Qt 用 `template_use_count`，本外壳用一次
  `card:<序号> note:"<笔记类型>"` 搜索代替）。

与 Qt 的差异（见 `tauri/README.md`）：
- 「重排卡片类型」是弹一个数字框（Qt 是同样的 `getOnlyText`，也能拖动）；
- 「添加字段」与 Qt 一样带字体/字号，但字体是文本框（Qt 是系统字体下拉）；
- 卡片类型下拉在 cloze 类型上禁用（Qt 也一样），但没有 Qt 的 Alt+1..9 填空快捷键；
- 复制信息到剪贴板后没有 Qt 的 "copied to clipboard" 提示 —— 那个 ftl key
  （`about-copied-to-clipboard`）已经不在本仓库的 ftl 里了；
- 帮助按钮没做（要接 HelpPage）。
-->
<script lang="ts">
    import type { RenderCardResponse, RenderedTemplateNode } from "@generated/anki/card_rendering_pb";
    import { UInt32 } from "@generated/anki/generic_pb";
    import { Note } from "@generated/anki/notes_pb";
    import type { Notetype } from "@generated/anki/notetypes_pb";
    import {
        Notetype_Config,
        Notetype_Config_Kind,
        Notetype_Template,
        Notetype_Template_Config,
        RestoreNotetypeToStockRequest,
        StockNotetype_Kind,
    } from "@generated/anki/notetypes_pb";
    import * as tr from "@generated/ftl";
    import {
        getDeckNames,
        getNote,
        getNotetype,
        getNotetypeNamesAndCounts,
        renderUncommittedCard,
        restoreNotetypeToStock,
        searchCards,
        updateNotetype,
        writeClipboard,
    } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { onMount } from "svelte";

    import Modal from "$lib/components/Modal.svelte";

    let notetype: Notetype | undefined = undefined;
    /** 预览用的笔记：编辑器里那张（`?note=`），没有就造一张空的 */
    let previewNote: Note | undefined = undefined;
    /** 当前编辑/预览的卡片模板 */
    let ord = 0;
    /** 0 正面模板 / 1 背面模板 / 2 样式（Qt 的三个单选按钮，`clayout.py:317`） */
    let editorIndex = 0;
    let previewFront = true;
    let fillEmpty = false;
    let nightMode = false;

    let busy = false;
    let error = "";
    let optionsOpen = false;
    let ready = false;
    /** 打开时的样子，用来算「有没有未保存的改动」（Qt 的 `ChangeTracker`） */
    let initialSnapshot = "";
    let useCount = 0;
    let decks: { id: bigint; name: string }[] = [];

    let previewHtml = "";
    let previewEmpty = false;
    let previewTimer: ReturnType<typeof setTimeout> | undefined = undefined;

    // 弹框
    let textModal: Modal;
    let fieldModal: Modal;
    let positionModal: Modal;
    let stockModal: Modal;
    let browserModal: Modal;
    let deckModal: Modal;
    let textMode: "add" | "rename" = "add";
    let textValue = "";
    let addFieldName = "";
    let addFieldFont = "Arial";
    let addFieldSize = 20;
    let positionValue = 1;
    let stockKind: StockNotetype_Kind = StockNotetype_Kind.BASIC;
    let browserFront = "";
    let browserBack = "";
    let deckOverride: string = "";

    $: config = notetype?.config;
    $: templates = notetype?.templates ?? [];
    $: template = templates[ord];
    $: isCloze = config?.kind === Notetype_Config_Kind.CLOZE;
    $: dirty = initialSnapshot !== "" && snapshot() !== initialSnapshot;
    $: textTitle = textMode === "add" ? tr.cardTemplatesAddCardType() : tr.actionsRename();
    // 任何会影响预览的东西变了都重画（Qt 也是靠 timer + redraw_everything）
    $: signature = [
        ord,
        previewFront,
        fillEmpty,
        nightMode,
        template?.config?.qFormat,
        template?.config?.aFormat,
        config?.css,
        previewNote?.id,
        ready,
    ].join("|");
    $: if (signature) {
        schedulePreview();
    }

    /** 未保存改动的指纹（只包含保存时会写回后端的字段）。 */
    function snapshot(): string {
        if (!notetype) {
            return "";
        }
        return JSON.stringify({
            css: notetype.config?.css ?? "",
            templates: notetype.templates.map((entry) => ({
                name: entry.name,
                ord: entry.ord?.val ?? 0,
                q: entry.config?.qFormat ?? "",
                a: entry.config?.aFormat ?? "",
                bq: entry.config?.qFormatBrowser ?? "",
                ba: entry.config?.aFormatBrowser ?? "",
                bf: entry.config?.browserFontName ?? "",
                bs: entry.config?.browserFontSize ?? 0,
                did: String(entry.config?.targetDeckId ?? 0n),
            })),
        });
    }

    // 保存 / 取消 ##########################################################

    /** 保存：整份笔记类型一起写回（对照 `clayout.py:931-940`，Qt 保存后也关窗）。 */
    async function save(): Promise<void> {
        if (!notetype || busy) {
            return;
        }
        busy = true;
        error = "";
        try {
            await updateNotetype(notetype);
            bridgeCommand("close");
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    /** 取消：有改动先问一句（Qt 会问「保存 / 放弃 / 取消」三选，这里只问放弃）。 */
    function cancel(): void {
        if (dirty && !confirm(tr.cardTemplatesDiscardChanges())) {
            return;
        }
        bridgeCommand("close");
    }

    // 编辑 ################################################################

    /** 往当前编辑区末尾追加一个字段引用（`clayout.py:914-923` 的 `_addField`）。 */
    function applyAddField(): void {
        const target = currentText();
        if (target === undefined || !addFieldName) {
            return;
        }
        const snippet = `\n<div style='font-family: "${addFieldFont}"; font-size: ${addFieldSize}px;'>{{${addFieldName}}}</div>\n`;
        writeCurrentText(target + snippet);
    }

    /** 翻转正反面（`clayout.py:743-755` 的 `onFlip` / `_flipQA`）。 */
    function flip(): void {
        const entry = template;
        if (!entry?.config) {
            return;
        }
        const match = /([\s\S]+)<hr id=answer>([\s\S]+)/.exec(entry.config.aFormat);
        if (!match) {
            alert(tr.cardTemplatesAnkiCouldntFindTheLineBetween());
            return;
        }
        const question = entry.config.qFormat;
        entry.config.aFormat = `{{FrontSide}}\n\n<hr id=answer>\n\n${question}`;
        entry.config.qFormat = match[2].trim();
    }

    /** 当前编辑区里的文本（样式是笔记类型级的）。 */
    function currentText(): string | undefined {
        if (editorIndex === 2) {
            return config?.css;
        }
        const entry = template?.config;
        if (!entry) {
            return undefined;
        }
        return editorIndex === 0 ? entry.qFormat : entry.aFormat;
    }

    function writeCurrentText(text: string): void {
        if (editorIndex === 2) {
            if (config) {
                config.css = text;
            }
            return;
        }
        const entry = template?.config;
        if (!entry) {
            return;
        }
        if (editorIndex === 0) {
            entry.qFormat = text;
        } else {
            entry.aFormat = text;
        }
    }

    // 卡片类型 ############################################################

    function renameCardType(): void {
        textMode = "rename";
        textValue = template?.name ?? "";
        textModal.show();
    }

    function addCardType(): void {
        textMode = "add";
        textValue = nextCardName();
        textModal.show();
    }

    /** 新卡片类型的默认名字：`card-templates-card`（"Card {n}"，`clayout.py:690-697`）。 */
    function nextCardName(): string {
        let n = templates.length + 1;
        for (;;) {
            const name = stripIsolation(tr.cardTemplatesCard({ val: n }));
            if (!templates.some((entry) => entry.name === name)) {
                return name;
            }
            n += 1;
        }
    }

    /** ftl 里的 `<n>` 会带 Unicode 隔离符（U+2068/U+2069），Qt 那边也会去掉。 */
    function stripIsolation(text: string): string {
        return text.replace(/[\u2068\u2069]/g, "");
    }

    async function applyText(): Promise<void> {
        const name = stripIsolation(textValue.trim()).replace(/"/g, "");
        if (!notetype || !name) {
            return;
        }
        if (textMode === "rename") {
            if (template) {
                template.name = name;
            }
            return;
        }
        // 添加：先确认（有用量时，对照 `clayout.py:699-703`），新模板复制当前模板
        if (useCount > 0 && !confirm(tr.cardTemplatesThisWillCreateCardProceed({ count: useCount }))) {
            return;
        }
        const current = template?.config;
        const created = new Notetype_Template({
            name,
            ord: new UInt32({ val: templates.length }),
            config: new Notetype_Template_Config({
                qFormat: current?.qFormat ?? "",
                aFormat: current?.aFormat ?? "",
            }),
        });
        notetype.templates = [...notetype.templates, created];
        ord = notetype.templates.length - 1;
    }

    /** 删除卡片类型：至少留一个，并数一下有多少张卡会被删（`clayout.py:615-652`）。 */
    async function removeCardType(): Promise<void> {
        if (!notetype || !template) {
            return;
        }
        if (notetype.templates.length < 2) {
            alert(tr.cardTemplatesAtLeastOneCardTypeIs());
            return;
        }
        // Qt 在确认框里放的是「N cards」那句话（`clayout.py:628-633`），但这条 ftl 的
        // `$cards` 在类型化导出里是数字，直接给数量
        const cards = (await countCardsInTemplate(template)) ?? 0;
        if (!confirm(tr.cardTemplatesDeleteTheAsCardTypeAnd({ template: template.name, cards }))) {
            return;
        }
        notetype.templates = notetype.templates.filter((entry) => entry !== template);
        renumberTemplates();
        ord = Math.min(ord, notetype.templates.length - 1);
    }

    /** 这个卡片模板下有多少张卡。Qt 查的是 `template_use_count`，RPC 没有这个接口，
     *  所以用一次搜索（`card:<序号> note:"<笔记类型>"`）代替。 */
    async function countCardsInTemplate(entry: Notetype_Template): Promise<number | undefined> {
        const index = (entry.ord?.val ?? 0) + 1;
        try {
            const response = await searchCards({
                search: `note:"${notetype?.name ?? ""}" card:${index}`,
            });
            return response.ids.length;
        } catch (problem) {
            // 搜索语法不被接受时就不报数量，确认框退化成不带数字的版本
            console.debug("数卡片模板用量失败", problem);
            return undefined;
        }
    }

    function renumberTemplates(): void {
        if (!notetype) {
            return;
        }
        notetype.templates = notetype.templates.map(
            (entry, index) =>
                new Notetype_Template({
                    name: entry.name,
                    ord: new UInt32({ val: index }),
                    config: entry.config,
                }),
        );
    }

    function askReposition(): void {
        positionValue = ord + 1;
        positionModal.show();
    }

    /** 重排卡片类型（`clayout.py:665-688`）。 */
    function applyReposition(): void {
        if (!notetype) {
            return;
        }
        const target = Math.round(positionValue);
        if (target < 1 || target > notetype.templates.length || target - 1 === ord) {
            return;
        }
        const next = [...notetype.templates];
        const [entry] = next.splice(ord, 1);
        next.splice(target - 1, 0, entry);
        notetype.templates = next;
        renumberTemplates();
        ord = target - 1;
    }

    // 选项 ################################################################

    /** 恢复默认：先把整份笔记类型还原成原厂状态（`clayout.py:715-741`）。 */
    async function restoreToDefault(forceKind?: StockNotetype_Kind): Promise<void> {
        if (!notetype || !config) {
            return;
        }
        // 老笔记类型没记 `originalStockKind`，Qt 这时让用户挑一个（`SelectStockNotetype`）
        if (forceKind === undefined && !config.originalStockKind) {
            stockModal.show();
            return;
        }
        if (!confirm(tr.cardTemplatesRestoreToDefaultConfirmation())) {
            return;
        }
        busy = true;
        try {
            await restoreNotetypeToStock(
                new RestoreNotetypeToStockRequest({
                    notetypeId: { ntid: notetype.id },
                    forceKind,
                }),
            );
            bridgeCommand("close");
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    function openBrowserAppearance(): void {
        if (!template?.config) {
            return;
        }
        browserFront = template.config.qFormatBrowser;
        browserBack = template.config.aFormatBrowser;
        browserModal.show();
    }

    /** 浏览器外观（`clayout.py:847-858`）：只影响浏览器里的列显示。 */
    function applyBrowserAppearance(): void {
        if (!template?.config) {
            return;
        }
        template.config.qFormatBrowser = browserFront.trim();
        template.config.aFormatBrowser = browserBack.trim();
    }

    function openDeckOverride(): void {
        deckOverride = String(template?.config?.targetDeckId ?? 0n);
        deckModal.show();
    }

    /** 牌组覆盖（`clayout.py:860-892`）：空值即取消覆盖。 */
    function applyDeckOverride(): void {
        if (!template?.config) {
            return;
        }
        template.config.targetDeckId = BigInt(deckOverride || "0");
    }

    /** 复制模板信息到剪贴板（`clayout.py:757-780`）。 */
    async function copyInfo(): Promise<void> {
        const entry = template?.config;
        if (!entry) {
            return;
        }
        const markdown = [
            "## Front Template",
            "```html",
            entry.qFormat.replace(/```/g, "\\`\\`\\`"),
            "```",
            "## Back Template",
            "```html",
            entry.aFormat.replace(/```/g, "\\`\\`\\`"),
            "```",
            "## Styling",
            "```css",
            (config?.css ?? "").replace(/```/g, "\\`\\`\\`"),
            "```",
            "",
        ].join("\n");
        try {
            await writeClipboard({ data: { "text/plain": new TextEncoder().encode(markdown) } });
        } catch (problem) {
            error = String(problem);
        }
    }

    // 预览 ################################################################

    function schedulePreview(): void {
        if (!ready) {
            return;
        }
        clearTimeout(previewTimer);
        // 与 Qt 的预览 timer 一样稍微等一下，别每敲一个字母就渲染一次
        previewTimer = setTimeout(() => void renderPreview(), 250);
    }

    async function renderPreview(): Promise<void> {
        if (!notetype || !template || !previewNote) {
            return;
        }
        try {
            const response = await renderUncommittedCard({
                note: previewNote,
                cardOrd: ord,
                template,
                fillEmpty,
                partialRender: false,
            });
            previewEmpty = response.isEmpty;
            previewHtml = pageHtml(response);
        } catch (problem) {
            error = String(problem);
        }
    }

    /** 把渲染结果拼成一份能直接塞进 iframe 的 HTML。 */
    function pageHtml(response: RenderCardResponse): string {
        const body = nodesToHtml(previewFront ? response.questionNodes : response.answerNodes);
        // 媒体文件是相对路径（与复习页一样靠 <base> 指到本地服务）
        const night = nightMode ? " nightMode night_mode" : "";
        // 注意：这里不能出现字面量的 style 开标签 —— Svelte 解析组件时会把它当成
        // 组件自己的样式块（`\x3c` 就是 `<`）
        return [
            '<!doctype html><html><head><meta charset="utf-8">',
            `<base href="${window.location.origin}/">`,
            `\x3cstyle>${response.css}\x3c/style>`,
            "\x3cstyle>body { margin: 8px; }\x3c/style>",
            `\x3c/head>\x3cbody class="card${night}">${body}\x3c/body>\x3c/html>`,
        ].join("\n");
    }

    /** `RenderedTemplateNode[]` → HTML（替换节点用 `currentText`，对照
     *  `rslib/src/template.rs` 的 `rendered_nodes_to_string`）。 */
    function nodesToHtml(nodes: RenderedTemplateNode[]): string {
        return nodes
            .map((node) =>
                node.value.case === "text" ? node.value.value : (node.value.value?.currentText ?? ""),
            )
            .join("");
    }

    // 生命周期 ############################################################

    function onKeydown(event: KeyboardEvent): void {
        if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
            // 对照 `clayout.py:466-470` 的 Save 快捷键
            event.preventDefault();
            void save();
        } else if (event.ctrlKey && ["1", "2", "3"].includes(event.key)) {
            editorIndex = Number(event.key) - 1;
        } else if (event.key === "F3" || event.key === "F4") {
            // Qt：F3 上一张 / F4 下一张（`clayout.py:202-219`）
            event.preventDefault();
            const step = event.key === "F3" ? -1 : 1;
            ord = Math.min(Math.max(ord + step, 0), templates.length - 1);
        }
    }

    onMount(async () => {
        await setupI18n({
            modules: [ModuleName.CARD_TEMPLATES, ModuleName.ACTIONS, ModuleName.FIELDS],
        });
        const params = new URLSearchParams(window.location.search);
        try {
            const requested = Number(params.get("notetype") ?? 0);
            const loaded = await getNotetype({ ntid: BigInt(requested) });
            // 后端只发有值的部分，这里补齐省得后面到处 `!`
            loaded.config = loaded.config ?? new Notetype_Config();
            for (const entry of loaded.templates) {
                entry.config = entry.config ?? new Notetype_Template_Config();
            }
            loaded.fields = loaded.fields ?? [];
            notetype = loaded;
            ord = Number(params.get("ord") ?? 0);
            if (!(ord >= 0 && ord < loaded.templates.length)) {
                ord = 0;
            }

            // 预览要一张真笔记：宿主给的是编辑器窗口正在编辑的那张；新建笔记时没有，
            // 就造一张空的（Qt 那边是 `Note(col, mid)`，字段全空，`editor.py:320-325`）
            const noteId = Number(params.get("note") ?? 0);
            previewNote = noteId
                ? await getNote({ nid: BigInt(noteId) })
                : new Note({ notetypeId: loaded.id });
            previewNote.fields = previewNote.fields ?? [];
            // 后台会按 1..n 补空字段名，缺字段的笔记渲染出来是空的，这里补够
            while (previewNote.fields.length < loaded.fields.length) {
                previewNote.fields.push("");
            }

            // 「添加卡片类型」要不要先确认、牌组覆盖的候选列表：两者失败都不致命
            try {
                const counts = await getNotetypeNamesAndCounts({});
                useCount =
                    counts.entries.find((entry) => entry.id === loaded.id)?.useCount ?? 0;
            } catch (problem) {
                console.debug("取笔记类型用量失败", problem);
            }
            try {
                decks = (
                    await getDeckNames({ skipEmptyDefault: false, includeFiltered: false })
                ).entries.map((entry) => ({
                    id: entry.id,
                    name: entry.name,
                }));
            } catch (problem) {
                console.debug("取牌组列表失败", problem);
            }

            initialSnapshot = snapshot();
            ready = true;
            await renderPreview();
        } catch (problem) {
            error = String(problem);
        }
    });
</script>

<svelte:window on:keydown={onKeydown} />

{#if notetype && config}
    <div class="cards">
        <div class="top">
            <span class="label">{tr.cardTemplatesCardType()}</span>
            <select id="card-type" bind:value={ord} disabled={isCloze}>
                {#each templates as entry, index (index)}
                    <option value={index}>{entry.name}</option>
                {/each}
            </select>
            <button
                class="btn btn-secondary"
                id="cards-options"
                on:click={() => (optionsOpen = !optionsOpen)}
            >
                {tr.actionsOptions()} ▾
            </button>
        </div>

        {#if optionsOpen}
            <div class="menu" id="cards-options-menu">
                <button class="menu-item" id="menu-restore" on:click={() => void restoreToDefault()}>
                    {tr.cardTemplatesRestoreToDefault()}...
                </button>
                {#if !isCloze}
                    <button class="menu-item" id="menu-add-type" on:click={addCardType}>
                        {tr.cardTemplatesAddCardType()}
                    </button>
                    <button class="menu-item" id="menu-rename-type" on:click={renameCardType}>
                        {tr.cardTemplatesRenameCardType()}
                    </button>
                    <button class="menu-item" id="menu-reposition-type" on:click={askReposition}>
                        {tr.cardTemplatesRepositionCardType()}
                    </button>
                    <button
                        class="menu-item"
                        id="menu-remove-type"
                        on:click={() => void removeCardType()}
                    >
                        {tr.cardTemplatesRemoveCardType()}
                    </button>
                    <button class="menu-item" id="menu-deck-override" on:click={openDeckOverride}>
                        {tr.cardTemplatesDeckOverride()}{template?.config?.targetDeckId
                            ? tr.cardTemplatesOn()
                            : tr.cardTemplatesOff()}
                    </button>
                {/if}
                <button class="menu-item" id="menu-copy-info" on:click={() => void copyInfo()}>
                    {tr.cardTemplatesCopyInfo()}
                </button>
                <button class="menu-item" id="menu-browser" on:click={openBrowserAppearance}>
                    {tr.cardTemplatesBrowserAppearance()}
                </button>
            </div>
        {/if}

        <div class="main">
            <section class="pane">
                <div class="group-title">{tr.cardTemplatesTemplateBox()}</div>
                <div class="row">
                    <label
                        ><input id="tab-front" type="radio" bind:group={editorIndex} value={0} />
                        {tr.cardTemplatesFrontTemplate()}</label
                    >
                    <label
                        ><input id="tab-back" type="radio" bind:group={editorIndex} value={1} />
                        {tr.cardTemplatesBackTemplate()}</label
                    >
                    <label
                        ><input id="tab-styling" type="radio" bind:group={editorIndex} value={2} />
                        {tr.cardTemplatesTemplateStyling()}</label
                    >
                </div>

                {#if editorIndex === 2}
                    <textarea id="editor-styling" bind:value={config.css} spellcheck="false"
                    ></textarea>
                {:else if template?.config}
                    {#if editorIndex === 0}
                        <textarea
                            id="editor-front"
                            bind:value={template.config!.qFormat}
                            spellcheck="false"
                        ></textarea>
                    {:else}
                        <textarea
                            id="editor-back"
                            bind:value={template.config!.aFormat}
                            spellcheck="false"
                        ></textarea>
                    {/if}
                {/if}
            </section>

            <section class="pane">
                <div class="group-title">{tr.cardTemplatesPreviewBox()}</div>
                <div class="row">
                    <label
                        ><input id="preview-front" type="radio" bind:group={previewFront} value={true} />
                        {tr.cardTemplatesFrontPreview()}</label
                    >
                    <label
                        ><input id="preview-back" type="radio" bind:group={previewFront} value={false} />
                        {tr.cardTemplatesBackPreview()}</label
                    >
                    <label class="check"
                        ><input type="checkbox" id="preview-fill-empty" bind:checked={fillEmpty} />
                        {tr.cardTemplatesFillEmpty()}</label
                    >
                    <label class="check"
                        ><input type="checkbox" id="preview-night" bind:checked={nightMode} />
                        {tr.cardTemplatesNightMode()}</label
                    >
                </div>
                {#if previewEmpty}
                    <div class="warn">{tr.cardTemplatesTypeBoxesWarning()}</div>
                {/if}
                <iframe id="preview" title={tr.cardTemplatesPreviewBox()} srcdoc={previewHtml}
                ></iframe>
            </section>
        </div>

        {#if error}
            <div class="error" id="cards-error">{error}</div>
        {/if}

        <div class="buttons">
            <button class="btn btn-secondary" id="cards-add-field" on:click={() => fieldModal.show()}>
                {tr.fieldsAddField()}
            </button>
            {#if !isCloze}
                <button class="btn btn-secondary" id="cards-flip" on:click={flip}>
                    {tr.cardTemplatesFlip()}
                </button>
            {/if}
            <span class="spacer"></span>
            <button class="btn btn-secondary" id="cards-cancel" on:click={cancel}>
                {tr.actionsCancel()}
            </button>
            <button
                class="btn btn-primary"
                id="cards-save"
                disabled={busy}
                on:click={() => void save()}>{tr.actionsSave()}</button
            >
        </div>
    </div>
{:else if error}
    <div class="error" id="cards-error">{error}</div>
{:else}
    <div class="loading">…</div>
{/if}

<Modal bind:this={textModal} modalKey="cards-text" onOkClicked={applyText}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{textTitle}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <form on:submit|preventDefault={textModal.acceptHandler}>
            <label for="cards-text-input" class="col-form-label">
                {textMode === "add" ? tr.cardTemplatesAddCardType() : tr.actionsNewName()}
            </label>
            <input id="cards-text-input" class="form-control" bind:value={textValue} />
        </form>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={textModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={textModal.acceptHandler}>
            {textMode === "add" ? tr.actionsAdd() : tr.actionsRename()}
        </button>
    </div>
</Modal>

<Modal bind:this={fieldModal} modalKey="cards-field" onOkClicked={applyAddField}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.fieldsAddField()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label for="cards-field-select" class="col-form-label">{tr.cardTemplatesCardType()}</label>
        <select id="cards-field-select" class="form-control" bind:value={addFieldName}>
            {#each notetype?.fields ?? [] as field (field.name)}
                <option value={field.name}>{field.name}</option>
            {/each}
        </select>
        <label for="cards-field-font" class="col-form-label">{tr.fieldsEditingFont()}</label>
        <div class="font-row">
            <input id="cards-field-font" class="form-control" bind:value={addFieldFont} />
            <input
                id="cards-field-size"
                class="form-control"
                type="number"
                min="5"
                max="300"
                bind:value={addFieldSize}
            />
        </div>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={fieldModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={fieldModal.acceptHandler}>
            {tr.actionsAdd()}
        </button>
    </div>
</Modal>

<Modal bind:this={positionModal} modalKey="cards-position" onOkClicked={applyReposition}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.cardTemplatesRepositionCardType()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label for="cards-position-input" class="col-form-label"
            >{tr.cardTemplatesEnterNewCardPosition1({ val: templates.length })}</label
        >
        <input
            id="cards-position-input"
            class="form-control"
            type="number"
            min="1"
            max={templates.length}
            bind:value={positionValue}
        />
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={positionModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={positionModal.acceptHandler}>
            {tr.actionsReposition()}
        </button>
    </div>
</Modal>

<Modal bind:this={stockModal} modalKey="cards-stock" onOkClicked={() => void restoreToDefault(stockKind)}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.cardTemplatesRestoreToDefault()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <!-- 对照 `clayout.py:983-1015` 的 `SelectStockNotetype`：老笔记类型没记原始
             类型时，让用户挑一个（名字与顺序同 `aqt.stdmodels` 的标准牌组） -->
        <select id="cards-stock-select" class="form-control" bind:value={stockKind}>
            <option value={StockNotetype_Kind.BASIC}>Basic</option>
            <option value={StockNotetype_Kind.BASIC_AND_REVERSED}>Basic (and reversed card)</option>
            <option value={StockNotetype_Kind.BASIC_OPTIONAL_REVERSED}
                >Basic (optional reversed card)</option
            >
            <option value={StockNotetype_Kind.BASIC_TYPING}>Basic (type in the answer)</option>
            <option value={StockNotetype_Kind.CLOZE}>Cloze</option>
            <option value={StockNotetype_Kind.IMAGE_OCCLUSION}>Image Occlusion</option>
        </select>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={stockModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={stockModal.acceptHandler}>
            {tr.actionsClose()}
        </button>
    </div>
</Modal>

<Modal bind:this={browserModal} modalKey="cards-browser" onOkClicked={applyBrowserAppearance}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.cardTemplatesBrowserAppearance()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <label for="cards-browser-front" class="col-form-label"
            >{tr.cardTemplatesFrontTemplate()}</label
        >
        <input id="cards-browser-front" class="form-control" bind:value={browserFront} />
        <label for="cards-browser-back" class="col-form-label"
            >{tr.cardTemplatesBackTemplate()}</label
        >
        <input id="cards-browser-back" class="form-control" bind:value={browserBack} />
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={browserModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={browserModal.acceptHandler}>
            {tr.actionsClose()}
        </button>
    </div>
</Modal>

<Modal bind:this={deckModal} modalKey="cards-deck" onOkClicked={applyDeckOverride}>
    <div slot="header" class="modal-header">
        <h5 class="modal-title">{tr.cardTemplatesDeckOverride()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <select id="cards-deck-select" class="form-control" bind:value={deckOverride}>
            <option value="0">—</option>
            {#each decks as deck (String(deck.id))}
                <option value={String(deck.id)}>{deck.name}</option>
            {/each}
        </select>
    </div>
    <div slot="footer" class="modal-footer">
        <button type="button" class="btn btn-secondary" on:click={deckModal.cancelHandler}>
            {tr.actionsCancel()}
        </button>
        <button type="button" class="btn btn-primary" on:click={deckModal.acceptHandler}>
            {tr.actionsClose()}
        </button>
    </div>
</Modal>

<style>
    .cards {
        padding: 0.75rem;
        display: flex;
        flex-direction: column;
        gap: 0.5rem;
        /* 窗口高度撑满：编辑区与预览各占一半（`100%` 在这里解析不到窗口高度） */
        height: 100vh;
        box-sizing: border-box;
    }

    .top {
        display: flex;
        align-items: center;
        gap: 0.5rem;
    }

    .top select {
        flex: 1;
        max-width: 24rem;
    }

    .menu {
        display: flex;
        flex-direction: column;
        align-items: stretch;
        border: 1px solid var(--border, #ccc);
        background: var(--canvas, #fff);
        position: absolute;
        right: 0.75rem;
        margin-top: 2rem;
        z-index: 10;
        min-width: 14rem;
    }

    .menu-item {
        text-align: left;
        padding: 0.3rem 0.6rem;
        background: none;
        border: none;
    }

    .menu-item:hover {
        background: var(--selected-bg, #cfe3ff);
    }

    .main {
        display: flex;
        flex: 1;
        gap: 0.5rem;
        min-height: 0;
    }

    .pane {
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
        flex: 1 1 50%;
        min-width: 0;
    }

    .group-title {
        font-weight: 600;
    }

    .row {
        display: flex;
        flex-wrap: wrap;
        align-items: center;
        gap: 0.6rem;
    }

    .row label {
        display: flex;
        align-items: center;
        gap: 0.25rem;
    }

    textarea {
        flex: 1;
        min-height: 12rem;
        font-family: var(--monospace, monospace);
        font-size: 0.9rem;
        resize: none;
    }

    #preview {
        flex: 1;
        min-height: 12rem;
        border: 1px solid var(--border, #ccc);
        background: #fff;
    }

    .warn {
        color: var(--fg-subtle, #666);
    }

    .buttons {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        padding-top: 0.25rem;
    }

    .spacer {
        flex: 1;
    }

    .font-row {
        display: flex;
        gap: 0.5rem;
    }

    .font-row input[type="number"] {
        width: 6rem;
    }

    .error {
        color: var(--fg-error, #c62828);
        white-space: pre-wrap;
    }

    .loading {
        padding: 1rem;
    }
</style>
