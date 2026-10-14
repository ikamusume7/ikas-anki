<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
自定义学习 / 筛选牌组（主窗口概览里的「自定义学习」，快捷键 `C`）。

对照 `qt/aqt/customstudy.py`：六个单选项，每项配一个数字输入 + 前后缀文案
（`onRadioChange` :90-156 会按选项换文案与范围）；只有「按卡片状态或标签学习」
（cram）额外多一个四选一的列表（`customstudy.ui:110-135`）。确定后构造
`CustomStudyRequest` 交给后端（`customstudy.py:177-212`），后端自己建/复用
「Custom Study Session」筛选牌组并把当前牌组切过去
（`rslib/src/scheduler/filtered/mod.rs:182-184`）—— Qt 随后只关对话框，不开始复习，
所以我们关窗后请宿主把主窗口导航到当前牌组的概览。

与 Qt 的差异（见 `tauri/README.md`）：
- cram 的「选择标签」用两个文本框（包含 / 排除，空格分隔）代替 Qt 的 `TagLimit` 对话框；
- 错误（同名牌组 / 没有匹配卡片）用页面上的提示代替原生对话框。
-->
<script lang="ts">
    import {
        CustomStudyRequest,
        CustomStudyRequest_Cram,
        CustomStudyRequest_Cram_CramKind,
    } from "@generated/anki/scheduler_pb";
    import type { CustomStudyDefaultsResponse } from "@generated/anki/scheduler_pb";
    import * as tr from "@generated/ftl";
    import { customStudy, customStudyDefaults } from "@generated/backend";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import { onMount } from "svelte";

    /** 六个选项（顺序与 `customstudy.ui` 一致）。 */
    type Option = "new" | "review" | "forgot" | "ahead" | "preview" | "cram";

    const maxSize = 1000; // 对照 `customstudy.py` 的 `DYN_MAX_SIZE`

    let deckId = 0n;
    let defaults: CustomStudyDefaultsResponse | undefined = undefined;
    let option: Option = "new";
    let spin = 0;
    let cramKind: CustomStudyRequest_Cram_CramKind = CustomStudyRequest_Cram_CramKind.DUE;
    let tagsToInclude = "";
    let tagsToExclude = "";
    let busy = false;
    let error = "";

    /**
     * 六个选项的文案/范围。
     *
     * 必须写成**响应式变量**再在模板里用：选项文案依赖 `defaults`（可用卡片数、
     * `extendNew` 的初值），而 Svelte **不追踪函数体里读到的响应式变量** ——
     * 直接写 `options()[key]` 的话，`defaults` 回来之后提示文案不会补上
     * （与交接文档 §5.25 是同一个坑）。
     */
    $: opts = options(defaults);
    $: spec = opts[option];

    interface OptionSpec {
        /** 单选项本身的文案（`customstudy.ui` 里每个 radio 的 text） */
        name: () => string;
        /** 单选项旁边的补充说明（只有前两项有：可用卡片数） */
        title: () => string;
        /** 数字框前 / 后的文案 */
        pre: () => string;
        post: (n: number) => string;
        min: number;
        max: number;
        initial: number;
    }

    function options(source: CustomStudyDefaultsResponse | undefined): Record<Option, OptionSpec> {
        const availableNew = source
            ? tr.customStudyAvailableNewCards2({
                  countString: `${source.availableNew}${
                      source.availableNewInChildren
                          ? " " +
                            tr.customStudyAvailableChildCount({
                                count: source.availableNewInChildren,
                            })
                          : ""
                  }`,
              })
            : "";
        return {
            new: {
                name: tr.customStudyIncreaseTodaysNewCardLimit,
                title: () => availableNew,
                pre: tr.customStudyIncreaseTodaysNewCardLimitBy,
                post: (n) => tr.customStudyCards({ count: n }),
                min: -maxSize,
                max: maxSize,
                // `extend_new` 是后端给的「上次用的值」
                initial: source?.extendNew ?? 0,
            },
            review: {
                name: tr.customStudyIncreaseTodaysReviewCardLimit,
                title: () => availableNew,
                pre: tr.customStudyIncreaseTodaysReviewLimitBy,
                post: (n) => tr.customStudyCards({ count: n }),
                min: -maxSize,
                max: maxSize,
                initial: source?.extendReview ?? 0,
            },
            forgot: {
                name: tr.customStudyReviewForgottenCards,
                title: () => "",
                pre: tr.customStudyReviewCardsForgottenInLast,
                post: (n) => tr.customStudyDays({ count: n }),
                min: 0,
                max: 30,
                initial: 1,
            },
            ahead: {
                name: tr.customStudyReviewAhead,
                title: () => "",
                pre: tr.customStudyReviewAheadBy,
                post: (n) => tr.customStudyDays({ count: n }),
                min: 1,
                max: 7,
                initial: 1,
            },
            preview: {
                name: tr.customStudyPreviewNewCards,
                title: () => "",
                pre: tr.customStudyPreviewNewCardsAddedInThe,
                post: (n) => tr.customStudyDays({ count: n }),
                min: 1,
                max: 30,
                initial: 1,
            },
            cram: {
                name: tr.customStudyStudyByCardStateOrTag,
                title: () => "",
                pre: tr.customStudySelect,
                post: (n) => tr.customStudyCardsFromTheDeck({ count: n }),
                min: 1,
                max: 1000,
                initial: 100,
            },
        };
    }

    /** 换选项时把数字框拨到该选项的默认值（对照 `onRadioChange`）。 */
    function pick(next: Option): void {
        option = next;
        spin = options(defaults)[next].initial;
        error = "";
    }

    /** 对照 `customstudy.py:177-212` 构造请求。 */
    function buildRequest(): CustomStudyRequest {
        const request = new CustomStudyRequest({ deckId });
        switch (option) {
            case "new":
                request.value = { case: "newLimitDelta", value: spin };
                break;
            case "review":
                request.value = { case: "reviewLimitDelta", value: spin };
                break;
            case "forgot":
                request.value = { case: "forgotDays", value: spin };
                break;
            case "ahead":
                request.value = { case: "reviewAheadDays", value: spin };
                break;
            case "preview":
                request.value = { case: "previewDays", value: spin };
                break;
            case "cram":
                request.value = {
                    case: "cram",
                    value: new CustomStudyRequest_Cram({
                        kind: cramKind,
                        cardLimit: spin,
                        tagsToInclude: splitTags(tagsToInclude),
                        tagsToExclude: splitTags(tagsToExclude),
                    }),
                };
                break;
        }
        return request;
    }

    const splitTags = (tags: string): string[] =>
        tags.trim().split(/\s+/).filter((tag) => tag.length > 0);

    async function accept(): Promise<void> {
        busy = true;
        error = "";
        try {
            await customStudy(buildRequest());
            // 后端已把当前牌组切成新建/复用的筛选牌组；关窗后请宿主把主窗口
            // 导航到当前牌组的概览（Qt 这边只关窗，用户得自己回到概览）
            bridgeCommand("customStudyDone");
        } catch (problem) {
            // 上游的两种错误：同名牌组（`ExistingDeck`）/ 没有匹配卡片（`NoMatchingCards`）
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    onMount(async () => {
        await setupI18n({
            modules: [
                ModuleName.CUSTOM_STUDY,
                ModuleName.ACTIONS,
                ModuleName.STUDYING,
            ],
        });
        const requested = Number(new URLSearchParams(window.location.search).get("deck") ?? 0);
        deckId = BigInt(requested);
        defaults = await customStudyDefaults({ deckId });
        spin = options(defaults).new.initial;
    });
</script>

<div class="custom-study">
    <!-- 标题交给窗口（宿主设成 `actions-custom-study`），页面里不再重复一遍 -->
    {#each Object.keys(opts) as key (key)}
        <label class="row">
            <input
                type="radio"
                name="custom-study-option"
                data-option={key}
                checked={option === key}
                on:change={() => pick(key as Option)}
            />
            <!-- 单选项文案 + （前两项才有的）可用卡片数提示，对照 `customstudy.ui` -->
            <span class="name">{opts[key as Option].name()}</span>
            {#if opts[key as Option].title()}
                <span class="title" data-hint={key}>{opts[key as Option].title()}</span>
            {/if}
        </label>
    {/each}

    <div class="spinner-row">
        <span>{spec.pre()}</span>
        <input
            id="custom-study-spin"
            type="number"
            bind:value={spin}
            min={spec.min}
            max={spec.max}
        />
        <span>{spec.post(spin)}</span>
    </div>

    {#if option === "cram"}
        <div class="cram">
            <select id="custom-study-cram-kind" bind:value={cramKind}>
                <option value={CustomStudyRequest_Cram_CramKind.NEW}
                    >{tr.customStudyNewCardsOnly()}</option
                >
                <option value={CustomStudyRequest_Cram_CramKind.DUE}
                    >{tr.customStudyDueCardsOnly()}</option
                >
                <option value={CustomStudyRequest_Cram_CramKind.REVIEW}>
                    {tr.customStudyAllReviewCardsInRandomOrder()}
                </option>
                <option value={CustomStudyRequest_Cram_CramKind.ALL}>
                    {tr.customStudyAllCardsInRandomOrderDont()}
                </option>
            </select>
            <!-- 文案取自 `taglimit.ui`（Qt 的「选择标签」对话框） -->
            <label class="tags">
                {tr.customStudyRequireOneOrMoreOfThese()}
                <input id="custom-study-tags-include" bind:value={tagsToInclude} />
            </label>
            <label class="tags">
                {tr.customStudySelectTagsToExclude()}
                <input id="custom-study-tags-exclude" bind:value={tagsToExclude} />
            </label>
        </div>
    {/if}

    {#if error}
        <div class="error" id="custom-study-error">{error}</div>
    {/if}

    <div class="buttons">
        <button class="btn btn-secondary" on:click={() => bridgeCommand("close")}>
            {tr.actionsCancel()}
        </button>
        <button
            class="btn btn-primary"
            id="custom-study-ok"
            disabled={busy}
            on:click={() => void accept()}>{tr.customStudyOk()}</button
        >
    </div>
</div>

<style>
    .custom-study {
        padding: 1rem 1.25rem;
        display: flex;
        flex-direction: column;
        gap: 0.35rem;
    }

    .row {
        display: flex;
        align-items: baseline;
        gap: 0.5rem;
    }

    .title {
        color: var(--fg-subtle, #666);
    }

    .spinner-row {
        display: flex;
        align-items: center;
        gap: 0.5rem;
        margin: 0.5rem 0 0.75rem 1.75rem;
    }

    .spinner-row input {
        width: 5rem;
    }

    .cram {
        display: flex;
        flex-direction: column;
        gap: 0.4rem;
        margin: 0 0 0.75rem 1.75rem;
    }

    .tags {
        display: flex;
        gap: 0.5rem;
        align-items: center;
    }

    .tags input {
        flex: 1;
    }

    .error {
        color: var(--fg-error, #c62828);
        white-space: pre-wrap;
    }

    .buttons {
        display: flex;
        justify-content: flex-end;
        gap: 0.5rem;
        margin-top: 1rem;
    }
</style>
