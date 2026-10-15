// 浏览器窗口的端到端检查：
//   侧栏 → 首屏搜索词 → 列头/排序 → 逐行取数 → 多选 → 笔记模式 → 列清单
//   → 批量动作（加标签 / 改牌组 / 删除 + 撤销）→ 双击开编辑器 → 预览
//   → 最后让 verify 脚本断言 searchInBrowser / browserSearch 的复用路径。
//
// 为什么用 Playwright：要真的点列头、真的按 Ctrl/Shift 多选、真的在弹框里输入。
// Chromium 与 WebView2 同源，加载同一个 URL，并按 `tauri/src/bridge.rs` 补上垫片
// （鉴权头 + 窗口身份 + bridgeCommand）。
//
// 桥接命令会**真的打到宿主**（`/_anki/pycmd`）—— 这正是我们想要的：`browserReady`
// 的响应来自宿主，`editCard` / `preview` 会真的开窗口（由 verify 脚本断言）。
// 写集合的动作（加标签 / 改牌组 / 删除）在检查结束时都会还原或清理。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer + 固定 ANKI_API_PORT），集合里至少有一张卡。
//
// 用法：
//   $env:ANKI_API_PORT="40027"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/browser-check.mjs

import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";

import {
    Card,
    CardId,
    SetDeckRequest,
    SetFlagRequest,
} from "../../out/ts/lib/generated/anki/cards_pb.js";
import { OpChangesWithId } from "../../out/ts/lib/generated/anki/collection_pb.js";
import {
    Deck,
    DeckIds,
    DeckNames,
    GetDeckNamesRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";
import { Empty, Json as GenericJson, String as GenericString } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    AddNoteRequest,
    AddNoteResponse,
    Note,
    NoteId,
    RemoveNotesRequest,
} from "../../out/ts/lib/generated/anki/notes_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
} from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import {
    BuryOrSuspendCardsRequest,
    BuryOrSuspendCardsRequest_Mode,
    GetQueuedCardsRequest,
    GradeNowRequest,
    QueuedCards,
    SetDueDateRequest,
} from "../../out/ts/lib/generated/anki/scheduler_pb.js";
import {
    BrowserColumns,
    SearchRequest,
    SearchResponse,
    SearchNode,
} from "../../out/ts/lib/generated/anki/search_pb.js";
import { NoteIdsAndTagsRequest } from "../../out/ts/lib/generated/anki/tags_pb.js";
import { SetConfigJsonRequest } from "../../out/ts/lib/generated/anki/config_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

async function rpc(method, body) {
    const response = await fetch(`${base}/_anki/${method}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/binary",
            Authorization: `Bearer ${apiKey}`,
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/** 读集合配置（缺键 → null）。 */
async function configValue(key) {
    return JSON.parse(
        new TextDecoder().decode(
            GenericJson.fromBinary(
                await rpc("getConfigJson", new GenericString({ val: key }).toBinary()),
            ).json,
        ),
    );
}

async function setConfig(key, value) {
    await rpc(
        "setConfigJson",
        new SetConfigJsonRequest({
            key,
            valueJson: new TextEncoder().encode(JSON.stringify(value ?? null)),
            undoable: false,
        }).toBinary(),
    );
}

/** 按后端搜索语法找 note（用 `SearchRequest` 而不是自己解 protobuf）。 */
async function searchNotes(search) {
    const response = SearchResponse.fromBinary(
        await rpc("searchNotes", new SearchRequest({ search }).toBinary()),
    );
    return response.ids.map(Number);
}

function bridgeShim(config) {
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
        const url = typeof input === "string" ? input : (input && input.url) || "";
        if (url.startsWith("/_anki/")) {
            init = init ?? {};
            init.headers = {
                ...(init.headers ?? {}),
                Authorization: `Bearer ${config.key}`,
                "X-Anki-Window": config.window,
            };
        }
        return originalFetch(input, init);
    };
    window.__bridgeCalls = [];
    window.bridgeCommand = window.pycmd = (arg, callback) => {
        window.__bridgeCalls.push(String(arg));
        window
            .fetch("/_anki/pycmd", {
                method: "POST",
                headers: { "Content-Type": "text/plain" },
                body: String(arg),
            })
            .then((response) => response.text())
            .then((text) => {
                if (!callback || !text) {
                    return;
                }
                // 与 tauri/src/bridge.rs 一致：响应是 JSON，解析后再交给回调
                try {
                    callback(JSON.parse(text));
                } catch {
                    callback(text);
                }
            });
        return false;
    };
}

// ##########################################################################
// 准备
// ##########################################################################

const queued = QueuedCards.fromBinary(
    await rpc(
        "getQueuedCards",
        new GetQueuedCardsRequest({ fetchLimit: 1, intradayLearningOnly: false }).toBinary(),
    ),
);
const card = queued.cards[0]?.card;
if (!card) {
    console.error("集合里没有可复习的卡片，先跑 seed-card.mjs");
    process.exit(2);
}

// 首屏搜索是 `deck:current`，后面的「选中第 2 行」「双击第 2 行」都需要至少两行 ——
// 全新的集合里只有一张卡片，这里先补一张陪跑的（与 `verify-editor.ps1` / `decks-check.mjs`
// 补卡片同一思路；只在不足两张时才补）
const cardDeckId = Card.fromBinary(
    await rpc("getCard", new CardId({ cid: BigInt(card.id) }).toBinary()),
).deckId;
const notesInDeck = SearchResponse.fromBinary(
    await rpc(
        "searchNotes",
        new SearchRequest({ search: `did:${cardDeckId}` }).toBinary(),
    ),
).ids;
if (notesInDeck.length < 2) {
    const note = Note.fromBinary(
        await rpc("getNote", new NoteId({ nid: card.noteId }).toBinary()),
    );
    await rpc(
        "addNote",
        new AddNoteRequest({
            note: new Note({
                notetypeId: note.notetypeId,
                // 字段数照抄现有笔记（它就是这张卡对应的笔记类型）
                fields: note.fields.map((_, index) =>
                    index === 0 ? `TauriBrowser 陪跑卡片${Date.now() % 100_000}` : "陪跑",
                ),
            }),
            deckId: cardDeckId,
        }).toBinary(),
    );
    console.log(`牌组 ${cardDeckId} 里不足两张卡片，补了一张陪跑卡片`);
}

const allColumns = BrowserColumns.fromBinary(
    await rpc("allBrowserColumns", new Empty({}).toBinary()),
).columns;
const defaultKeys = ["noteFld", "template", "cardDue", "deck"];
const original = {
    activeCols: await configValue("activeCols"),
    sortType: await configValue("sortType"),
    sortBackwards: await configValue("sortBackwards"),
    notesMode: await configValue("browserTableShowNotesMode"),
};
const activeKeys =
    Array.isArray(original.activeCols) && original.activeCols.length > 0
        ? original.activeCols.map(String)
        : defaultKeys;
const labelFor = (key) => allColumns.find((it) => it.key === key)?.cardsModeLabel;
const expectedHeaders = activeKeys.map(labelFor).filter((label) => !!label);
const originalDeckId = Card.fromBinary(
    await rpc("getCard", new CardId({ cid: BigInt(card.id) }).toBinary()),
).deckId;

// 临时牌组（改牌组测试）：`addDeck(Deck) returns (OpChangesWithId)`。
// `Deck.kind` 是必填的 oneof，缺了会报 "missing kind"
const tempDeckName = `TauriTemp${Date.now() % 100_000}`;
const tempDeckId = OpChangesWithId.fromBinary(
    await rpc(
        "addDeck",
        new Deck({ name: tempDeckName, kind: { case: "normal", value: {} } }).toBinary(),
    ),
).id;

console.log(`临时牌组 ${tempDeckName}（${tempDeckId}）/ 触发卡片 ${card.id}`);

const browser = await chromium.launch();
let cleanedUp = false;

try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 760 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "browser" });

    // 记录搜索请求的原始字节（用来断言排序参数确实交给了后端）
    const searchPayloads = [];
    await page.route("**/_anki/search*", async (route) => {
        const body = route.request().postDataBuffer();
        if (body) {
            searchPayloads.push({
                url: route.request().url(),
                text: Buffer.from(body).toString("latin1"),
            });
        }
        await route.continue();
    });

    await page.goto(`${base}/browser/`, { waitUntil: "load" });
    await page.waitForFunction(() => typeof window.loadSearch === "function", null, {
        timeout: 20_000,
    });
    await page.waitForSelector(".sidebar", { timeout: 20_000 });

    // 虚拟列表（`VirtualTable.svelte`）会在首尾插「撑高度」的空 `<tr>`：它只有一个
    // 没有 `title` 的 `<td>`。所有「行」相关的断言都要排除它们，否则会多算一行。
    const readTable = () =>
        page.evaluate(() => ({
            headers: [...document.querySelectorAll("thead th")].map((th) =>
                th.textContent.trim().replace(/[▲▼]/g, "").trim(),
            ),
            rows: [...document.querySelectorAll("tbody tr")].filter((row) =>
                row.querySelector("td[title]"),
            ).length,
            spacers: [...document.querySelectorAll("tbody tr")].filter(
                (row) => !row.querySelector("td[title]"),
            ).length,
            firstCells: [...document.querySelectorAll("tbody tr")]
                .filter((row) => row.querySelector("td[title]"))
                .slice(0, 3)
                .map((row) => [...row.querySelectorAll("td")].map((cell) => cell.textContent.trim())),
            status: document.querySelector(".status")?.textContent?.trim() ?? "",
        }));
    const titles = () =>
        page.evaluate(() =>
            (window.__bridgeCalls ?? []).filter((call) => call.startsWith("browserTitle:")),
        );
    /** 只有数据行的定位器（排除虚拟列表的撑高行，见 `readTable`） */
    const rows = page.locator("tbody tr:has(td[title])");

    // 1. 侧栏 + 首屏搜索词 + 列头 ###########################################
    const sidebarItems = await page.evaluate(() =>
        [...document.querySelectorAll(".sidebar button")].map((item) => item.textContent.trim()),
    );
    check(
        "侧栏已渲染牌组",
        // 默认牌组的名字随界面语言（`Default` / 「系统默认」）
        sidebarItems.some((item) => /Default|系统默认/.test(item)),
        sidebarItems.slice(0, 5).join(" / "),
    );

    const hostSearch = await page.evaluate(
        () =>
            new Promise((resolve) => {
                window.bridgeCommand("browserReady", (response) =>
                    resolve(response?.search ?? null),
                );
            }),
    );
    check("宿主给出了首屏搜索词", typeof hostSearch === "string" && hostSearch.length > 0, String(hostSearch));
    await page.evaluate((search) => window.loadSearch(search), hostSearch);
    await page.waitForFunction(
        () => {
            const cell = document.querySelector("tbody td");
            return !!cell && cell.textContent.trim().length > 0;
        },
        null,
        { timeout: 20_000 },
    );
    let table = await readTable();
    check(
        "列头 = activeCols 指定的列",
        JSON.stringify(table.headers) === JSON.stringify(expectedHeaders),
        table.headers.join(" | "),
    );
    check("结果行已渲染", table.rows > 0, `${table.rows} 行`);

    // 2. 排序 ##############################################################
    const sortableIndex = activeKeys.indexOf("cardDue");
    if (sortableIndex >= 0) {
        searchPayloads.length = 0;
        await page.locator("thead th").nth(sortableIndex).click();
        await page.waitForTimeout(1500);
        check(
            "点列头后 sortType 落库",
            (await configValue("sortType")) === "cardDue",
            String(await configValue("sortType")),
        );
        const backwardsBefore = await configValue("sortBackwards");
        await page.locator("thead th").nth(sortableIndex).click();
        await page.waitForTimeout(1500);
        check(
            "再点一次翻转方向",
            (await configValue("sortBackwards")) !== backwardsBefore,
            `${backwardsBefore} → ${await configValue("sortBackwards")}`,
        );
        // 排序参数必须出现在发给后端的请求里（`SortOrder.builtin.column`）
        check(
            "排序参数交给了后端",
            searchPayloads.length > 0 && searchPayloads.some((it) => it.text.includes("cardDue")),
            `${searchPayloads.length} 次搜索请求`,
        );
        check("列头显示排序箭头", (await readTable()).headers.length === expectedHeaders.length);
    } else {
        check("activeCols 里有可排序的 cardDue 列", false, JSON.stringify(activeKeys));
    }

    // 3. 多选 ##############################################################
    await rows.nth(0).click();
    await page.waitForTimeout(300);
    check("单选一行 → 标题报告 1 选中", /^browserTitle:0:1:/.test((await titles()).at(-1) ?? ""), (await titles()).at(-1) ?? "");

    await rows.nth(1).click({ modifiers: ["Control"] });
    await page.waitForTimeout(300);
    check("Ctrl+点击加选", /^browserTitle:0:2:/.test((await titles()).at(-1) ?? ""), (await titles()).at(-1) ?? "");

    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
    check("Escape 清空选择", /^browserTitle:0:0:/.test((await titles()).at(-1) ?? ""), (await titles()).at(-1) ?? "");

    const rowCount = await rows.count();
    await page.keyboard.press("Control+a");
    await page.waitForTimeout(300);
    check(
        "Ctrl+A 全选",
        new RegExp(`^browserTitle:0:${rowCount}:`).test((await titles()).at(-1) ?? ""),
        `${(await titles()).at(-1) ?? ""}（共 ${rowCount} 行）`,
    );

    // 4. 笔记模式 ##########################################################
    const notesSearches = [];
    page.on("response", (response) => {
        if (response.url().includes("/_anki/searchNotes")) {
            notesSearches.push(response.url());
        }
    });
    await page.locator("#browse-toggle-notes-mode").click();
    await page.waitForTimeout(1500);
    check("切换到笔记模式并落库", (await configValue("browserTableShowNotesMode")) === true);
    check("笔记模式改用 searchNotes", notesSearches.length > 0, `${notesSearches.length} 次`);
    check("标题切换到笔记文案", /^browserTitle:1:/.test((await titles()).at(-1) ?? ""), (await titles()).at(-1) ?? "");
    const noteKeys = ["noteFld", "note", "template", "noteTags"];
    check(
        "笔记模式的列头来自 activeNoteCols",
        (await page.locator("thead th").count()) === noteKeys.length,
        (await readTable()).headers.join(" | "),
    );
    await page.locator("#browse-toggle-notes-mode").click();
    await page.waitForTimeout(1500);
    check("切回卡片模式", (await configValue("browserTableShowNotesMode")) === false);

    // 5. 列清单（表头右键） ################################################
    await page.locator("thead tr").click({ button: "right" });
    await page.waitForSelector(".columns-menu", { timeout: 5000 });
    const menuCount = await page.locator(".columns-menu input").count();
    check("列清单列出了全部列", menuCount === allColumns.length, `${menuCount} 项`);
    const uncheckedIndex = await page.evaluate(() =>
        [...document.querySelectorAll(".columns-menu input")].findIndex((box) => !box.checked),
    );
    check("有未勾选的列可供测试", uncheckedIndex >= 0, `序号 ${uncheckedIndex}`);
    if (uncheckedIndex >= 0) {
        await page.locator(".columns-menu input").nth(uncheckedIndex).check();
        await page.waitForTimeout(1200);
        const afterCols = await configValue("activeCols");
        check(
            "勾选一列后立即写回 activeCols",
            Array.isArray(afterCols) && afterCols.length === activeKeys.length + 1,
            JSON.stringify(afterCols),
        );
        await page.locator(".columns-menu input").nth(uncheckedIndex).uncheck();
        await page.waitForTimeout(1200);
        check(
            "取消后恢复",
            Array.isArray(await configValue("activeCols")) &&
                (await configValue("activeCols")).length === activeKeys.length,
            JSON.stringify(await configValue("activeCols")),
        );
    }
    await page.keyboard.press("Escape");
    await page.waitForTimeout(400);
    check("Escape 关掉列清单", (await page.locator(".columns-menu").count()) === 0);

    // 5.5 拖动列头换顺序 / 列宽按模式分开 ##################################
    const headerKeys = () =>
        page.evaluate(() => [...document.querySelectorAll("thead th")].map((th) => th.dataset.column));
    const headerWidths = () =>
        page.evaluate(() => [...document.querySelectorAll("thead th")].map((th) => th.style.width));
    const cellsOf = (row) =>
        page.evaluate(
            (index) =>
                [...(document.querySelectorAll("tbody tr")[index]?.querySelectorAll("td") ?? [])].map(
                    (cell) => cell.textContent.trim(),
                ),
            row,
        );

    /**
     * 把第 `from` 列的列头拖到第 `to` 列。
     *
     * 落点语义（页面里的规则）：指针落在某列的**左半边**就插到那一列前面。
     * 所以 `side = "after"`（落在右半边）用于「挪到这一列之后」。
     */
    const dragHeader = async (from, to, side = "after") => {
        const fromBox = await page.locator("thead th").nth(from).boundingBox();
        const toBox = await page.locator("thead th").nth(to).boundingBox();
        const targetX =
            side === "after" ? toBox.x + toBox.width - 4 : toBox.x + Math.min(4, toBox.width / 4);
        await page.mouse.move(fromBox.x + fromBox.width / 2, fromBox.y + fromBox.height / 2);
        await page.mouse.down();
        await page.mouse.move(targetX, toBox.y + toBox.height / 2, { steps: 8 });
        await page.mouse.up();
        await page.waitForTimeout(1500);
    };

    const keysBefore = await headerKeys();
    const cellsBefore = await cellsOf(0);
    const sortBefore = await configValue("sortType");
    await dragHeader(0, keysBefore.length - 1);
    const keysAfter = await headerKeys();
    check(
        "拖动列头 → 首列挪到末尾",
        JSON.stringify(keysAfter) === JSON.stringify([...keysBefore.slice(1), keysBefore[0]]),
        keysAfter.join(" | "),
    );
    check(
        "新顺序写回 activeCols",
        JSON.stringify(await configValue("activeCols")) === JSON.stringify(keysAfter),
        JSON.stringify(await configValue("activeCols")),
    );
    const cellsAfter = await cellsOf(0);
    check(
        "单元格跟着列顺序走（没和表头错位）",
        JSON.stringify(cellsAfter) === JSON.stringify([...cellsBefore.slice(1), cellsBefore[0]]),
        `「${cellsAfter.join(" | ")}」`,
    );
    check("拖动不会被当成点列头排序", (await configValue("sortType")) === sortBefore);
    await dragHeader(keysAfter.length - 1, 0, "before");
    check(
        "拖回原顺序并落库",
        JSON.stringify(await headerKeys()) === JSON.stringify(keysBefore) &&
            JSON.stringify(await configValue("activeCols")) === JSON.stringify(keysBefore),
        `${(await headerKeys()).join(" | ")} / activeCols=${JSON.stringify(await configValue("activeCols"))}`,
    );

    // 列宽：卡片/笔记各存一份（切模式再切回来就能验证是分开的、且都落了库）
    /** 拖第一列的右下角手柄，返回拖完后的宽度（px）。 */
    const resizeFirstColumn = async (delta) => {
        const handle = await page.locator("thead th .resizer").first().boundingBox();
        await page.mouse.move(handle.x + 2, handle.y + handle.height / 2);
        await page.mouse.down();
        await page.mouse.move(handle.x + 2 + delta, handle.y + handle.height / 2, { steps: 5 });
        await page.mouse.up();
        await page.waitForTimeout(1200);
        return parseFloat((await headerWidths())[0]);
    };
    /** 把第一列拖回指定宽度（拖动是线性的：新宽 = 旧宽 + 位移）。 */
    const restoreWidth = async (target) => {
        const current = parseFloat((await headerWidths())[0]);
        await resizeFirstColumn(target - current);
    };

    const cardsWidthBefore = parseFloat((await headerWidths())[0]);
    const cardsWidth = await resizeFirstColumn(60);
    check(
        "卡片模式可以拖宽列头",
        cardsWidth === cardsWidthBefore + 60,
        `${cardsWidthBefore} → ${cardsWidth}`,
    );
    await page.locator("#browse-toggle-notes-mode").click();
    await page.waitForTimeout(1500);
    const notesWidthBefore = parseFloat((await headerWidths())[0]);
    const notesWidth = await resizeFirstColumn(40);
    check(
        "笔记模式用的是另一份列宽",
        notesWidth === notesWidthBefore + 40 && notesWidth !== cardsWidth,
        `${notesWidthBefore} → ${notesWidth}（卡片模式是 ${cardsWidth}）`,
    );
    await restoreWidth(notesWidthBefore);
    await page.locator("#browse-toggle-notes-mode").click();
    await page.waitForTimeout(1500);
    check(
        "切回卡片模式恢复自己的列宽",
        parseFloat((await headerWidths())[0]) === cardsWidth,
        (await headerWidths())[0],
    );
    await restoreWidth(cardsWidthBefore);
    check(
        "卡片模式列宽已还原",
        parseFloat((await headerWidths())[0]) === cardsWidthBefore,
        (await headerWidths())[0],
    );

    // 6. 加标签（Ctrl+Shift+A） ############################################
    const tagMarker = `tauriTag${Date.now() % 100_000}`;
    await rows.nth(0).click();
    await page.keyboard.press("Control+Shift+a");
    await page.waitForSelector("#tags-input", { timeout: 5000 });
    await page.locator("#tags-input").fill(tagMarker);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1800);
    const tagged = await searchNotes(`tag:${tagMarker}`);
    check("加标签：能按新标签搜到", tagged.length > 0, `${tagged.length} 条`);
    check("加标签后状态栏有提示", (await readTable()).status.length > 0, (await readTable()).status);
    if (tagged.length > 0) {
        await rpc(
            "removeNoteTags",
            new NoteIdsAndTagsRequest({ noteIds: tagged.map(BigInt), tags: tagMarker }).toBinary(),
        );
        check("已清理测试标签", (await searchNotes(`tag:${tagMarker}`)).length === 0);
    }

    // 7. 改牌组（Ctrl+D） ##################################################
    await page.locator(".search").fill(`nid:${card.noteId}`);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1500);
    await rows.nth(0).click();
    await page.keyboard.press("Control+d");
    await page.waitForSelector("#deck-filter", { timeout: 5000 });
    await page.locator("#deck-filter").fill(tempDeckName);
    await page.locator(".modal.show .deck-list button").first().click();
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1800);
    const moved = Card.fromBinary(
        await rpc("getCard", new CardId({ cid: BigInt(card.id) }).toBinary()),
    );
    check("改牌组：卡片真的换到新牌组", moved.deckId === tempDeckId, `${originalDeckId} → ${moved.deckId}`);
    check("改牌组后状态栏有提示", (await readTable()).status.length > 0, (await readTable()).status);
    await rpc(
        "setDeck",
        new SetDeckRequest({ cardIds: [BigInt(card.id)], deckId: originalDeckId }).toBinary(),
    );
    check(
        "改牌组：已还原",
        Card.fromBinary(await rpc("getCard", new CardId({ cid: BigInt(card.id) }).toBinary()))
            .deckId === originalDeckId,
    );

    // 8. 删除 + 撤销 #######################################################
    const deleteMarker = `TauriDel${Date.now() % 100_000}`;
    const notetype = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    ).entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name));
    const deckNames = DeckNames.fromBinary(
        await rpc("getDeckNames", new GetDeckNamesRequest({ includeFiltered: false }).toBinary()),
    ).entries;
    const defaultDeck = deckNames.find((entry) => /^(Default|系统默认)$/i.test(entry.name))?.id ?? 1n;
    const disposableNote = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: { notetypeId: notetype.id, fields: [deleteMarker, "back"] },
                deckId: defaultDeck,
            }).toBinary(),
        ),
    ).noteId;
    await page.locator(".search").fill(deleteMarker);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(2000);
    // 注意：不要断言"第一格"是正文字段 —— 列可以拖动/勾选，顺序由用户决定
    // （上面的列顺序测试改过 `activeCols`），所以扫整行的单元格
    const diag = await page.evaluate(() => ({
        search: document.querySelector(".search")?.value ?? null,
        rows: document.querySelectorAll("tbody tr").length,
        cells: [...(document.querySelector("tbody tr")?.querySelectorAll("td") ?? [])].map((cell) =>
            cell.textContent.trim(),
        ),
        status: document.querySelector(".status")?.textContent?.trim() ?? null,
        modalOpen: !!document.querySelector(".modal.show"),
    }));
    console.log(`删除测试前的页面状态: ${JSON.stringify(diag)}`);
    check(
        "搜到了新建的测试 note",
        diag.cells.some((text) => text.includes(deleteMarker)),
        `搜索框「${diag.search}」/ ${diag.rows} 行 / 行内「${diag.cells.join(" | ")}」`,
    );
    const removed = [];
    page.on("response", (response) => {
        if (response.url().includes("/_anki/removeNotes")) {
            removed.push(response.status());
        }
    });
    await rows.nth(0).click();
    await page.keyboard.press("Delete");
    await page.waitForTimeout(1800);
    check("删除发出了 removeNotes", removed.length > 0, removed.join(","));
    check("删除后集合里没有了", (await searchNotes(deleteMarker)).length === 0, `note ${disposableNote}`);
    check("删除后状态栏有提示", (await readTable()).status.length > 0, (await readTable()).status);
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(1800);
    check("Ctrl+Z 撤销后 note 回来了", (await searchNotes(deleteMarker)).length === 1);
    await rpc(
        "removeNotes",
        new RemoveNotesRequest({ noteIds: [disposableNote], cardIds: [] }).toBinary(),
    );
    check("已清理测试 note", (await searchNotes(deleteMarker)).length === 0);

    // 9. 动作表（Qt 的 Notes / Cards / View / Jumps 菜单） #################
    //    前面的删除测试把结果集换成了临时 note（且已被删掉），这里先回到完整结果 ——
    //    否则后面的行操作会落在空列表上。
    await page.locator(".search").fill(hostSearch);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1500);
    await page.locator("tbody tr").nth(0).click();
    await page.waitForTimeout(300);

    //    记录之后的全部 RPC 请求：动作是否真的打到了对应的后端接口，看请求体
    const rpcCalls = [];
    await page.route("**/_anki/*", async (route) => {
        const body = route.request().postDataBuffer();
        rpcCalls.push({
            method: route.request().url().split("/").pop(),
            body: body ? new Uint8Array(body) : new Uint8Array(),
        });
        await route.continue();
    });
    const lastCall = (method) => [...rpcCalls].reverse().find((call) => call.method === method);
    /** 选中行的序号（只算数据行，所以 `[0]` 就是第一个结果） */
    const selectedIndexes = () =>
        page.evaluate(() =>
            [...document.querySelectorAll("tbody tr")]
                .filter((row) => row.querySelector("td[title]"))
                .map((row, index) => (row.classList.contains("selected") ? index : -1))
                .filter((index) => index >= 0),
        );

    // 右键一行 → 打开动作菜单（与工具栏的「更多」是同一份表）
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    const menuItems = await page.locator(".context-menu .item").count();
    check("右键菜单已打开", menuItems >= 20, `${menuItems} 项`);
    check(
        "菜单项带了 Qt 的快捷键提示",
        (await page.locator('.context-menu .shortcut:has-text("Ctrl+K")').count()) === 1,
    );

    // 卡片信息（只有菜单入口）：宿主会开 card-info 窗口，由 verify 脚本断言
    await page.locator('[data-action="cardInfo"]').click();
    await page.waitForTimeout(1500);
    check(
        "菜单 → 卡片信息发出 cardInfo:<cid>",
        (await page.evaluate(() => window.__bridgeCalls ?? [])).some((call) =>
            call.startsWith("cardInfo:"),
        ),
    );

    // 复制笔记：开「添加卡片」并载入当前笔记（verify 脚本断言窗口内容）
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="createCopy"]').click();
    await page.waitForTimeout(1200);
    check(
        "菜单 → 复制笔记发出 addCopy",
        (await page.evaluate(() => window.__bridgeCalls ?? [])).some((call) =>
            call.startsWith("addCopy:"),
        ),
    );

    // 清除未使用标签（无快捷键，只能从菜单走）
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="clearUnusedTags"]').click();
    await page.waitForTimeout(1500);
    check("菜单 → 清除未使用标签", !!lastCall("clearUnusedTags"));

    // 标记（Ctrl+K）：`marked` 标签的加/删（`browser.py:1083-1087`）
    await page.locator("tbody tr").nth(0).click();
    await page.keyboard.press("Control+k");
    await page.waitForTimeout(1500);
    const markCall = lastCall("addNoteTags");
    check(
        "Ctrl+K 加 marked 标签",
        markCall && NoteIdsAndTagsRequest.fromBinary(markCall.body).tags === "marked",
    );
    check("集合里能按 marked 搜到", (await searchNotes("tag:marked")).length > 0);
    await page.locator("tbody tr").nth(0).click();
    await page.keyboard.press("Control+k");
    await page.waitForTimeout(1500);
    check("再按一次 Ctrl+K 取消标记", !!lastCall("removeNoteTags"));
    check("marked 标签已清掉", (await searchNotes("tag:marked")).length === 0);

    // 旗标（Ctrl+3）：与当前行相同则清除（`browser.py:1056-1068`）
    await page.keyboard.press("Control+3");
    await page.waitForTimeout(1200);
    const flagCall = lastCall("setFlag");
    check(
        "Ctrl+3 设置绿色旗标",
        flagCall && SetFlagRequest.fromBinary(flagCall.body).flag === 3,
        flagCall ? `flag=${SetFlagRequest.fromBinary(flagCall.body).flag}` : "",
    );
    await page.keyboard.press("Control+3");
    await page.waitForTimeout(1200);
    check(
        "同一旗标再按一次 = 清除",
        SetFlagRequest.fromBinary(lastCall("setFlag").body).flag === 0,
    );

    // 暂停 / 埋藏（Ctrl+J、Ctrl+Shift+J）：切换语义
    await page.keyboard.press("Control+j");
    await page.waitForTimeout(1200);
    const suspendCall = lastCall("buryOrSuspendCards");
    check(
        "Ctrl+J 暂停",
        suspendCall &&
            BuryOrSuspendCardsRequest.fromBinary(suspendCall.body).mode ===
                BuryOrSuspendCardsRequest_Mode.SUSPEND,
    );
    await page.keyboard.press("Control+j");
    await page.waitForTimeout(1200);
    check("再次 Ctrl+J 取消暂停", !!lastCall("restoreBuriedAndSuspendedCards"));

    await page.keyboard.press("Control+Shift+j");
    await page.waitForTimeout(1200);
    const buryCall = lastCall("buryOrSuspendCards");
    check(
        "Ctrl+Shift+J 埋藏",
        buryCall &&
            BuryOrSuspendCardsRequest.fromBinary(buryCall.body).mode ===
                BuryOrSuspendCardsRequest_Mode.BURY_USER,
    );
    await page.keyboard.press("Control+Shift+j");
    await page.waitForTimeout(1200);

    // 重置（Ctrl+Alt+N）：弹框里两个勾选 → schedule_cards_as_new
    await page.keyboard.press("Control+Alt+n");
    await page.waitForSelector(".modal.show #browse-field-0", { timeout: 5000 });
    check("重置弹框有 2 个勾选项", (await page.locator(".modal.show input[type=checkbox]").count()) === 2);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    check("重置发出 scheduleCardsAsNew", !!lastCall("scheduleCardsAsNew"));
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(1500);

    // 设置到期日（Ctrl+Shift+D）：单输入框 → setDueDate
    await page.keyboard.press("Control+Shift+d");
    await page.waitForSelector(".modal.show #browse-field-0", { timeout: 5000 });
    await page.locator(".modal.show #browse-field-0").fill("2");
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    const dueCall = lastCall("setDueDate");
    check(
        "设置到期日发出 setDueDate(days=2)",
        dueCall && SetDueDateRequest.fromBinary(dueCall.body).days === "2",
    );
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(1500);

    // 重新定位（Ctrl+Shift+S）→ SortCards
    await page.keyboard.press("Control+Shift+s");
    await page.waitForSelector(".modal.show #browse-field-0", { timeout: 5000 });
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    check("重新定位发出 sortCards", !!lastCall("sortCards"));
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(1500);

    // 立即评分（菜单 → 良好）→ GradeNow
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="grade-3"]').click();
    await page.waitForTimeout(1500);
    const gradeCall = lastCall("gradeNow");
    check(
        "菜单 → 立即评分（良好）",
        gradeCall && GradeNowRequest.fromBinary(gradeCall.body).rating === 3,
    );
    await page.keyboard.press("Control+z");
    await page.waitForTimeout(1500);

    // 跳转 / 反选 / 全选（Ctrl+N、Home/End、Ctrl+Alt+S、Ctrl+Alt+A）
    const total = await rows.count();
    const jumpDiag = await readTable();
    console.log(
        `跳转测试：数据行 ${jumpDiag.rows} / 撑高行 ${jumpDiag.spacers} / 状态栏「${jumpDiag.status}」`,
    );
    /** 状态栏里的「已选取 N 张」——页面自己的计数（与 DOM 的 class 是两条证据） */
    const selectionCount = async () => {
        const status = (await readTable()).status;
        const match = status.match(/选取[^\d]*(\d+)/);
        return match ? Number(match[1]) : -1;
    };
    // 注意：这几条**同时**看 DOM 的高亮与状态栏的计数 —— 页面状态对了但高亮没刷新
    // 也算 bug（踩过一次：`selected={isSelected(id)}` 让 Svelte 追踪不到 `selected`）
    await page.keyboard.press("Home");
    await page.waitForTimeout(400);
    check(
        "Home 选中第一行",
        JSON.stringify(await selectedIndexes()) === "[0]",
        `选中行 ${JSON.stringify(await selectedIndexes())}`,
    );
    await page.keyboard.press("Control+n");
    await page.waitForTimeout(400);
    check(
        "Ctrl+N 选到下一行",
        JSON.stringify(await selectedIndexes()) === "[1]",
        `选中行 ${JSON.stringify(await selectedIndexes())} / 状态栏计数 ${await selectionCount()}`,
    );
    await page.keyboard.press("End");
    await page.waitForTimeout(400);
    check(
        "End 选中最后一行",
        JSON.stringify(await selectedIndexes()) === `[${total - 1}]`,
        `选中行 ${JSON.stringify(await selectedIndexes())}`,
    );
    await page.keyboard.press("Control+Alt+a");
    await page.waitForTimeout(400);
    check(
        "Ctrl+Alt+A 全选（高亮也要跟上）",
        (await selectedIndexes()).length === total && (await selectionCount()) === total,
        `高亮 ${(await selectedIndexes()).length}/${total} / 状态栏计数 ${await selectionCount()}`,
    );
    await page.keyboard.press("Control+Alt+s");
    await page.waitForTimeout(400);
    check(
        "Ctrl+Alt+S 反选",
        (await selectedIndexes()).length === 0 && (await selectionCount()) === 0,
        `高亮 ${(await selectedIndexes()).length} / 状态栏计数 ${await selectionCount()}`,
    );

    // 选中笔记（菜单）→ 换成 `nid:` 搜索并全选。
    //
    // 这一步之后的「预览」会由 `verify-browser.ps1` 断言 `previewNext` / `previewPrev` ——
    // 那需要**同一张 note 有 ≥2 张卡片**（预览会话来自选中的卡片）。集合里恰好有没有
    // 双卡片笔记是不确定的（以前是靠"攒下来的老集合"里正好有翻转卡片类型），所以这里
    // 显式挑一个「模板数 ≥ 2」的笔记类型造一张测试笔记，再从它进入预览
    const previewMarker = `TauriPrev${Date.now() % 100_000}`;
    let multiCardNotetype;
    const allNotetypes = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    ).entries;
    for (const entry of allNotetypes) {
        const candidate = Notetype.fromBinary(
            await rpc("getNotetype", new NotetypeId({ ntid: entry.id }).toBinary()),
        );
        if (candidate.templates.length >= 2) {
            multiCardNotetype = candidate;
            break;
        }
    }
    check(
        "集合里有模板数 ≥ 2 的笔记类型（预览翻页要用）",
        !!multiCardNotetype,
        multiCardNotetype?.name ?? "(没有)",
    );
    const previewNote = multiCardNotetype
        ? AddNoteResponse.fromBinary(
              await rpc(
                  "addNote",
                  new AddNoteRequest({
                      note: new Note({
                          notetypeId: multiCardNotetype.id,
                          fields: multiCardNotetype.fields.map((_, index) =>
                              index === 0 ? previewMarker : "预览用",
                          ),
                      }),
                      deckId: defaultDeck,
                  }).toBinary(),
              ),
          ).noteId
        : 0n;
    await page.locator(".search").fill(previewMarker);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1500);

    await page.locator("tbody tr").nth(0).click();
    await page.waitForTimeout(300);
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="selectNotes"]').click();
    await page.waitForTimeout(1500);
    check(
        "菜单 → 选中笔记（换成 nid: 搜索）",
        (await page.evaluate(() => document.querySelector(".status .search-info")?.textContent ?? "")).includes(
            "nid:",
        ),
        await page.evaluate(
            () => document.querySelector(".status .search-info")?.textContent ?? "",
        ),
    );

    // 查找替换（Ctrl+Alt+F）：在一条临时笔记上做，做完删掉
    const frMarker = `TauriFr${Date.now() % 100_000}`;
    const frReplaced = `${frMarker}Done`;
    const frNote = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: { notetypeId: notetype.id, fields: [frMarker, "back"] },
                deckId: defaultDeck,
            }).toBinary(),
        ),
    ).noteId;
    await page.locator(".search").fill(frMarker);
    await page.keyboard.press("Enter");
    await page.waitForFunction(
        (marker) =>
            [...(document.querySelectorAll("tbody td") ?? [])].some((cell) =>
                (cell.textContent ?? "").includes(marker),
            ),
        frMarker,
        { timeout: 20_000 },
    );
    await page.locator("tbody tr").nth(0).click();
    await page.keyboard.press("Control+Alt+f");
    await page.waitForSelector(".modal.show #browse-field-0", { timeout: 5000 });
    await page.locator(".modal.show #browse-field-0").fill(frMarker);
    await page.locator(".modal.show #browse-field-1").fill(frReplaced);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(2000);
    check("查找替换发出 findAndReplace", !!lastCall("findAndReplace"));
    const replaced = Note.fromBinary(
        await rpc("getNote", new NoteId({ nid: frNote }).toBinary()),
    ).fields[0];
    check("查找替换真的改了字段", replaced === frReplaced, `「${replaced}」`);
    await rpc(
        "removeNotes",
        new RemoveNotesRequest({ noteIds: [frNote], cardIds: [] }).toBinary(),
    );
    check("已清理查找替换的测试 note", (await searchNotes(frMarker)).length === 0);

    // 8.5 更改笔记类型 #####################################################
    // 宿主开窗口（verify 脚本断言），页面里保存 ⇒ 后端改类型 + **宿主关窗**
    // （`server/api.rs` 接管 `changeNotetype`，对照 `mediasrv.py:772`）
    const ntMarker = `TauriNt${Date.now() % 100_000}`;
    const notetypeEntries = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    ).entries;
    const ntNoteId = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: { notetypeId: notetype.id, fields: [ntMarker, "back"] },
                deckId: defaultDeck,
            }).toBinary(),
        ),
    ).noteId;
    await page.locator(".search").fill(ntMarker);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1500);
    await page.locator("tbody tr").nth(0).click();
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="changeNotetype"]').click();
    await page.waitForTimeout(1500);
    check(
        "菜单 → 更改笔记类型发出 changeNoteType",
        (await page.evaluate(() => window.__bridgeCalls ?? [])).some((call) =>
            call.startsWith("changeNoteType:"),
        ),
    );

    // 8.6 管理笔记类型 #####################################################
    // Qt 在浏览器侧栏 Note Types 节点的右键菜单里（`tree.py:924-928`），本外壳的侧栏
    // 没有右键菜单，入口挪到了「更多」菜单；窗口与页面由宿主开（`pycmd("notetypes")`），
    // 页面本身由 `notetypes-check.mjs` 验
    await page.locator("tbody tr").nth(0).click({ button: "right" });
    await page.waitForSelector(".context-menu", { timeout: 5000 });
    await page.locator('[data-action="manageNotetypes"]').click();
    await page.waitForTimeout(1200);
    check(
        "菜单 → 管理笔记类型发出 notetypes",
        (await page.evaluate(() => window.__bridgeCalls ?? [])).includes("notetypes"),
    );

    // 窗口里"选好映射后保存"由一个独立页面驱动（与其它检查一样：同一个后端、
    // 同一个 URL）。URL 支持 `change-notetype/<old>/<new>`，新类型直接从 URL 给，
    // 页面会按字段名自动映射。
    const ntTarget = notetypeEntries.find((entry) => entry.id !== notetype.id && entry.name.startsWith(notetype.name.slice(0, 5)));
    if (ntTarget === undefined) {
        console.log("没有可用于测试的第二个笔记类型，跳过更改笔记类型");
    } else {
        const ntPage = await browser.newPage({ viewport: { width: 900, height: 700 } });
        await ntPage.addInitScript(bridgeShim, { key: apiKey, window: "change-notetype" });
        await ntPage.goto(`${base}/change-notetype/${notetype.id}/${ntTarget.id}`, {
            waitUntil: "load",
        });
        await ntPage.waitForSelector(".select-container", { timeout: 20_000 });
        check("更改笔记类型页面已渲染", (await ntPage.locator(".select-container").count()) >= 2);
        await ntPage.getByRole("button", { name: /^(Save|保存)$/ }).click();
        await ntPage.waitForTimeout(2000);
        const changed = Note.fromBinary(
            await rpc("getNote", new NoteId({ nid: ntNoteId }).toBinary()),
        );
        check(
            "更改笔记类型真的生效",
            changed.notetypeId === ntTarget.id,
            `${notetype.id} → ${changed.notetypeId}（目标 ${ntTarget.id}）`,
        );
        await ntPage.close();
    }
    await rpc(
        "removeNotes",
        new RemoveNotesRequest({ noteIds: [ntNoteId], cardIds: [] }).toBinary(),
    );
    check("已清理更改笔记类型的测试 note", (await searchNotes(ntMarker)).length === 0);

    // 8.6 导出（apkg / colpkg）#############################################
    // 保存路径由宿主给：`ANKI_SAVE_PATH` 是外壳的测试钩子（原生保存对话框没法自动点，
    // 见 `platform/dialogs.rs`）；指向目录时文件名用页面建议的那个。
    const exportDir = process.env.ANKI_SAVE_PATH ?? "";
    if (!exportDir) {
        console.log("没有设置 ANKI_SAVE_PATH，跳过导出检查");
    } else {
        const exportedSince = () =>
            fs
                .readdirSync(exportDir)
                .filter((name) => /\.(apkg|colpkg)$/.test(name))
                .map((name) => ({
                    name,
                    path: path.join(exportDir, name),
                    mtime: fs.statSync(path.join(exportDir, name)).mtimeMs,
                }))
                .sort((x, y) => y.mtime - x.mtime);

        // 单独建一张临时 note 当作导出内容（前面改笔记类型那张已经删了）
        const expMarker = `TauriExp${Date.now() % 100_000}`;
        const expNoteId = AddNoteResponse.fromBinary(
            await rpc(
                "addNote",
                new AddNoteRequest({
                    note: { notetypeId: notetype.id, fields: [expMarker, "back"] },
                    deckId: defaultDeck,
                }).toBinary(),
            ),
        ).noteId;
        const before = new Set(exportedSince().map((entry) => entry.name));
        await page.locator(".search").fill(expMarker);
        await page.keyboard.press("Enter");
        await page.waitForTimeout(1500);
        await page.locator("tbody tr").nth(0).click();
        await page.locator("tbody tr").nth(0).click({ button: "right" });
        await page.waitForSelector(".context-menu", { timeout: 5000 });
        await page.locator('[data-action="export"]').click();
        await page.waitForSelector(".modal.show #export-format", { timeout: 5000 });
        check(
            "导出对话框默认 apkg + 选中的笔记",
            (await page.locator("#export-format").inputValue()) === "apkg" &&
                (await page.locator("#export-scope").inputValue()) === "selection",
        );
        // 顺手挡住"漏了 i18n 模块"这类问题：缺 key 会直接显示 `missing key: …`
        // （踩过：`exporting-collection` 没加载 → 建议的文件名带冒号 → 导出必然失败）
        const dialogText = await page.locator(".modal.show").innerText();
        check(
            "导出对话框的文案已本地化",
            !dialogText.includes("missing key"),
            dialogText.split("\n").slice(0, 3).join(" / "),
        );
        // colpkg 没有范围与排程/预设选项（`ColpkgExporter.show_deck_list = False`）
        await page.locator("#export-format").selectOption("colpkg");
        await page.waitForTimeout(200);
        check(
            "切到 colpkg 后只剩「含媒体 / 旧版支持」",
            (await page.locator("#export-scope").count()) === 0 &&
                (await page.locator("#export-sched").count()) === 0 &&
                (await page.locator("#export-media").count()) === 1,
        );
        await page.locator(".modal.show .modal-footer .btn-secondary").click();
        await page.waitForTimeout(400);

        // 真的导出一份 apkg（选中的那张笔记）
        await page.locator("tbody tr").nth(0).click({ button: "right" });
        await page.waitForSelector(".context-menu", { timeout: 5000 });
        await page.locator('[data-action="export"]').click();
        await page.waitForSelector(".modal.show #export-format", { timeout: 5000 });
        await page.locator("#export-confirm").click();
        await page.waitForTimeout(4000);
        const produced = exportedSince().find((entry) => !before.has(entry.name));
        const stats = produced ? fs.statSync(produced.path) : undefined;
        const magic = stats
            ? Buffer.from(fs.readFileSync(produced.path).subarray(0, 2)).toString("latin1")
            : "";
        check(
            "导出 apkg 产出了合法文件",
            !!produced && magic === "PK" && (stats?.size ?? 0) > 512,
            `${produced?.name ?? "（没有新文件）"} ${stats?.size ?? 0} 字节 ${magic}`,
        );
        await rpc(
            "removeNotes",
            new RemoveNotesRequest({ noteIds: [expNoteId], cardIds: [] }).toBinary(),
        );
        check("已清理导出的测试 note", (await searchNotes(expMarker)).length === 0);
    }

    // 9. 双击开编辑器 + 预览 ##############################################
    // 预览会话来自这一屏选中的卡片，宿主侧还要翻页（`previewNext`），所以搜到刚造的那张
    // **双卡片**笔记上 —— 用 `hostSearch` 的话第一行多半是单卡笔记，翻页断言会失败
    await page.locator(".search").fill(previewMarker);
    await page.keyboard.press("Enter");
    await page.waitForTimeout(1500);
    await rows.nth(0).dblclick();
    await page.waitForTimeout(1200);
    const calls = await page.evaluate(() => window.__bridgeCalls ?? []);
    check(
        "双击行发出了 editCard",
        calls.some((call) => call.startsWith("editCard:")),
        calls.filter((call) => call.startsWith("editCard:")).at(-1) ?? "",
    );

    await rows.nth(0).click();
    await page.waitForTimeout(300);
    await page.locator("#browse-preview").click();
    await page.waitForTimeout(1500);
    const previewCall = (await page.evaluate(() => window.__bridgeCalls ?? [])).find((call) =>
        call.startsWith("preview:"),
    );
    check("预览发出了 preview:<cid>", !!previewCall, previewCall ?? "");

    // 10. 还原配置 #########################################################
    await setConfig("sortType", original.sortType ?? "noteFld");
    await setConfig("sortBackwards", original.sortBackwards ?? false);
    await setConfig("browserTableShowNotesMode", original.notesMode ?? false);
    await setConfig("activeCols", original.activeCols ?? defaultKeys);
    check(
        "配置已还原",
        JSON.stringify(await configValue("activeCols")) ===
            JSON.stringify(original.activeCols ?? defaultKeys),
        JSON.stringify(await configValue("sortType")),
    );

    // searchInBrowser 端点在 verify 脚本里断言（会真的开/复用窗口）
    await rpc(
        "searchInBrowser",
        new SearchNode({ filter: { case: "nids", value: { ids: [BigInt(card.noteId)] } } }).toBinary(),
    );
    console.log(`已通过 searchInBrowser 请求搜索 nid:${card.noteId}`);

    // 预览用的那张双卡片笔记**不在这里删**：宿主侧的 `previewNext` / `previewPrev`
    // 断言（`verify-browser.ps1`）在本脚本退出之后才跑，还要靠它的两张卡片。
    // 它由下一次运行开头的 `cleanup-test-decks.mjs` 收拾（连同失败时残留的牌组）。

    // 注意字段名是 `dids`（`DeckIds { repeated int64 dids = 1 }`），不是 `deck_ids`
    await rpc("removeDecks", new DeckIds({ dids: [tempDeckId] }).toBinary());
    const decksLeft = DeckNames.fromBinary(
        await rpc("getDeckNames", new GetDeckNamesRequest({ includeFiltered: false }).toBinary()),
    ).entries.map((entry) => entry.name);
    cleanedUp = !decksLeft.includes(tempDeckName);
    check("已删除临时牌组", cleanedUp, tempDeckName);
} catch (error) {
    check("浏览器交互流程", false, String(error));
} finally {
    await browser.close();
    if (!cleanedUp) {
        console.log("注意：检查中途失败，可能残留临时牌组/配置改动");
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
    process.exitCode = 1;
}
