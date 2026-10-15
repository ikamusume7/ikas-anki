<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<!--
「查找重复」对话框（浏览器「更多」菜单 → 查找重复）。

对照 `qt/aqt/browser/find_duplicates.py` 的 `FindDuplicatesDialog` + `forms/finddupes.ui`：
「搜索位置」字段下拉、「可选筛选器」输入框、「搜索」按钮、结果报告（每组一个链接）、
「Tag Duplicates」按钮。判重逻辑复刻 `pylib/anki/collection.py:744-778` 的 `find_dupes()`：

1. 先搜出「该字段非空」的笔记（可选筛选器与它 AND，走 `buildSearchString` 的
   `field_name` 节点 —— 就是 `FieldName` 过滤器，等价于 `Field:_*`）；
2. 逐条取字段值（按字段名在该笔记类型里的位置），`stripHtml(PRESERVE_MEDIA_FILENAMES)`
   去标签、保留媒体文件名（与上游 `strip_html_media` 同一个后端方法）；
3. 空值不算重复；同一个值出现 ≥2 次就是一组，**组的顺序按"第二次出现"排**
   （上游 `find_dupes` 就是这么 append 的）。

差异（有意为之，见 `tauri/README.md` 的「浏览器」）：
- Qt 是挂在浏览器上的**独立非模态对话框**（报告画在 QWebView 里）；这里是页面内的
  模态框，点某一组直接让浏览器换搜索（等价于上游的 `search_for()` + `onNote()`）；
- 上游的搜索框带历史记录（`restore_combo_history`），这里没有历史；
- 后端没有批量取字段的 RPC（`getNote` 一次一条），所以大集合会慢；上游是直接读 SQL。
-->
<script lang="ts">
    import { StripHtmlRequest_Mode } from "@generated/anki/card_rendering_pb";
    import { Empty } from "@generated/anki/generic_pb";
    import { SearchNode_Group_Joiner } from "@generated/anki/search_pb";
    import {
        addNoteTags,
        buildSearchString,
        getNote,
        getNotetype,
        getNotetypeNames,
        searchNotes,
        stripHtml,
    } from "@generated/backend";
    import * as tr from "@generated/ftl";

    import Modal from "$lib/components/Modal.svelte";

    /** 一组重复：同一个字段值 + 用到这个值的笔记（`find_dupes` 的返回值形状）。 */
    interface DuplicateGroup {
        value: string;
        noteIds: bigint[];
    }

    /** 点某一组时把 `nid:…` 搜索词交回页面（页面负责切笔记模式 + 换搜索）。 */
    export let onJump: (search: string) => void;

    let modal: Modal;
    let fields: string[] = [];
    let field = "";
    let filter = "";
    let busy = false;
    let error = "";
    let searched = false;
    let groups: DuplicateGroup[] = [];

    /**
     * 全部笔记类型里的字段名：大小写完全相同的只留第一次出现的写法，最后按小写排序。
     * 对照 `anki/find.py:60-67` 的 `fieldNames()` + `find_duplicates.py:45-47`。
     */
    async function collectFields(): Promise<string[]> {
        const seen = new Set<string>();
        const all: string[] = [];
        for (const entry of (await getNotetypeNames(new Empty({}))).entries) {
            const notetype = await getNotetype({ ntid: entry.id });
            for (const notetypeField of notetype.fields) {
                if (!seen.has(notetypeField.name)) {
                    seen.add(notetypeField.name);
                    all.push(notetypeField.name);
                }
            }
        }
        return all.sort((left, right) => left.toLowerCase().localeCompare(right.toLowerCase()));
    }

    export async function show(): Promise<void> {
        busy = true;
        error = "";
        try {
            fields = await collectFields();
        } catch (problem) {
            error = String(problem);
        }
        if (!fields.includes(field)) {
            field = fields[0] ?? "";
        }
        filter = "";
        groups = [];
        searched = false;
        busy = false;
        modal.show();
    }

    /** 点某一组之后由页面收起本框（`FindDuplicatesDialog` 在点了链接后也会关掉自己）。 */
    export function hide(): void {
        modal.hide();
    }

    /** 「笔记类型 → 字段名顺序」，用来把字段名映射成 `Note.fields` 的下标。 */
    const fieldNamesCache = new Map<string, string[]>();

    async function fieldNamesOf(notetypeId: bigint): Promise<string[]> {
        const key = notetypeId.toString();
        let names = fieldNamesCache.get(key);
        if (!names) {
            names = (await getNotetype({ ntid: notetypeId })).fields.map((entry) => entry.name);
            fieldNamesCache.set(key, names);
        }
        return names;
    }

    async function search(): Promise<void> {
        if (!field) {
            return;
        }
        busy = true;
        error = "";
        groups = [];
        searched = true;
        try {
            const filterText = filter.trim();
            const fieldNode = { filter: { case: "fieldName" as const, value: field } };
            // 拼搜索词走后端（与上游 `build_search_string()` 同一套转义/括号规则）
            const node = filterText
                ? {
                      filter: {
                          case: "group" as const,
                          value: {
                              nodes: [
                                  {
                                      filter: {
                                          case: "parsableText" as const,
                                          value: filterText,
                                      },
                                  },
                                  fieldNode,
                              ],
                              joiner: SearchNode_Group_Joiner.AND,
                          },
                      },
                  }
                : fieldNode;
            const searchText = (await buildSearchString(node)).val;
            const nids = (await searchNotes({ search: searchText }, { alertOnError: false })).ids;

            const byValue = new Map<string, bigint[]>();
            /** 组的顺序 = 值第二次出现的顺序（照上游的 append 时机） */
            const duplicated: string[] = [];
            for (const nid of nids) {
                const note = await getNote({ nid });
                const names = await fieldNamesOf(note.notetypeId);
                const index = names.findIndex(
                    (name) => name.toLowerCase() === field.toLowerCase(),
                );
                if (index < 0) {
                    continue;
                }
                const value = (
                    await stripHtml({
                        text: note.fields[index] ?? "",
                        mode: StripHtmlRequest_Mode.PRESERVE_MEDIA_FILENAMES,
                    })
                ).val;
                if (!value) {
                    continue; // 空值不算重复
                }
                const list = byValue.get(value);
                if (list) {
                    list.push(nid);
                    if (list.length === 2) {
                        duplicated.push(value);
                    }
                } else {
                    byValue.set(value, [nid]);
                }
            }
            groups = duplicated.map((value) => ({ value, noteIds: byValue.get(value) ?? [] }));
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    /** 点一组：把这一组的 nid 拼成搜索词交给页面（对照 `find_duplicates.py:108-114`）。 */
    async function jump(group: DuplicateGroup): Promise<void> {
        const searchText = (
            await buildSearchString({ filter: { case: "nids", value: { ids: group.noteIds } } })
        ).val;
        onJump(searchText);
    }

    /** 「Tag Duplicates」：给所有组里的笔记都加上 `browsing-duplicate` 标签。 */
    async function tagDuplicates(): Promise<void> {
        if (groups.length === 0 || busy) {
            return;
        }
        busy = true;
        error = "";
        try {
            const noteIds = new Set<bigint>();
            for (const group of groups) {
                for (const nid of group.noteIds) {
                    noteIds.add(nid);
                }
            }
            // `NoteIdsAndTagsRequest.tags` 是**一个**空格分隔的字符串（不是 repeated），
            // 与上游 `add_tags_to_notes(space_separated_tags=…)` 的形状一致
            await addNoteTags({ noteIds: [...noteIds], tags: tr.browsingDuplicate() });
        } catch (problem) {
            error = String(problem);
        } finally {
            busy = false;
        }
    }

    const totalNotes = (): number =>
        groups.reduce((total, group) => total + group.noteIds.length, 0);
</script>

<Modal bind:this={modal} modalKey="find-duplicates" dialogClass="modal-lg">
    <div slot="header" class="modal-header">
        <h5 class="modal-title" id="modalLabel">{tr.browsingFindDuplicates()}</h5>
        <button
            type="button"
            class="btn-close"
            data-bs-dismiss="modal"
            aria-label={tr.actionsClose()}
        ></button>
    </div>
    <div slot="body" class="modal-body">
        <div class="row g-2 mb-2">
            <label class="col-auto col-form-label" for="find-dupes-field">
                {tr.browsingSearchIn()}
            </label>
            <div class="col">
                <select id="find-dupes-field" class="form-select" bind:value={field}>
                    {#each fields as name (name)}
                        <option value={name}>{name}</option>
                    {/each}
                </select>
            </div>
        </div>
        <div class="row g-2 mb-2">
            <label class="col-auto col-form-label" for="find-dupes-filter">
                {tr.browsingOptionalFilter()}
            </label>
            <div class="col">
                <input
                    id="find-dupes-filter"
                    class="form-control"
                    bind:value={filter}
                    on:keydown={(event) => {
                        if (event.key === "Enter") {
                            void search();
                        }
                    }}
                />
            </div>
        </div>

        <div id="find-dupes-report" class="report">
            {#if searched}
                <p id="find-dupes-summary">
                    {tr.browsingFoundAsAcrossBs({
                        part: tr.browsingGroup({ count: groups.length }),
                        whole: tr.browsingNoteCount({ count: totalNotes() }),
                    })}
                </p>
                <ol>
                    {#each groups as group (group.value)}
                        <li>
                            <button
                                type="button"
                                class="group"
                                data-group={group.value}
                                on:click={() => void jump(group)}
                            >
                                {tr.browsingNoteCount({ count: group.noteIds.length })}: {group.value}
                            </button>
                        </li>
                    {/each}
                </ol>
            {/if}
        </div>

        {#if error}
            <div class="error" id="find-dupes-error">{error}</div>
        {/if}
    </div>
    <div slot="footer" class="modal-footer">
        <button
            id="find-dupes-tag"
            type="button"
            class="btn btn-secondary"
            disabled={busy || groups.length === 0}
            on:click={() => void tagDuplicates()}>{tr.browsingTagDuplicates()}</button
        >
        <button
            id="find-dupes-search"
            type="button"
            class="btn btn-primary"
            disabled={busy || !field}
            on:click={() => void search()}>{tr.actionsSearch()}</button
        >
        <button type="button" class="btn btn-secondary" on:click={modal.cancelHandler}>
            {tr.actionsClose()}
        </button>
    </div>
</Modal>

<style lang="scss">
    .report {
        max-height: 45vh;
        overflow-y: auto;
    }

    .report ol {
        margin: 0;
        padding-left: 1.25rem;
    }

    .group {
        border: none;
        background: none;
        color: var(--fg-link, #0b5ed7);
        padding: 0;
        text-align: left;
        text-decoration: underline;
        cursor: pointer;
    }

    .error {
        color: red;
        white-space: pre-wrap;
    }
</style>
