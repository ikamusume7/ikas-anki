// 牌组列表（主窗口默认页面，`/decks/`）的端到端检查。
//
//   表头与列 → 当前牌组高亮 → 折叠/展开 → 点牌组进概览（`?deck=`）→ 概览里点「学习」
//   → 齿轮菜单（选项 / 重命名 / 删除）→ 新建牌组 → 底部按钮（导入）
//
// 为什么用 Playwright：要真的点齿轮、真的在弹框里输入、真的要断言「折叠后子牌组消失」。
// 与 browser-check.mjs 同一套办法：Chromium 加载同一个 URL，按 `tauri/src/bridge.rs`
// 补垫片（鉴权头 + 窗口身份 + bridgeCommand），桥接命令**真的打到宿主**。
//
// 对照上游 `qt/aqt/deckbrowser.py`（`open:<did>` / `collapse:<did>` / `select:<did>` /
// `opts:<did>` 与底部三个按钮）与 `qt/aqt/overview.py`（概览那块面板）。
//
// 前置：外壳以 `ANKI_START_PAGE=decks` 启动 + 固定 `ANKI_API_PORT`。
//
// 用法：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/decks-check.mjs

import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";

import {
    Deck,
    DeckId,
    DeckIds,
    DeckNames,
    GetDeckNamesRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";
import { CardIds } from "../../out/ts/lib/generated/anki/cards_pb.js";
import { OpChangesWithId } from "../../out/ts/lib/generated/anki/collection_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
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
    CongratsInfoResponse,
} from "../../out/ts/lib/generated/anki/scheduler_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";

const port = process.env.ANKI_API_PORT ?? "40031";
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

const currentDeckId = () =>
    rpc("getCurrentDeck", new Empty({}).toBinary()).then(
        (bytes) => Deck.fromBinary(bytes).id,
    );

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
    // 与 `tauri/src/bridge.rs` 的注入脚本一致：命令打到宿主，**有回调时**把响应
    // （JSON，解析不了就给原文）交给回调 —— 「帮助 → 关于」靠它拿版本信息
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
                try {
                    callback(JSON.parse(text));
                } catch {
                    callback(text);
                }
            })
            .catch(() => {});
        return false;
    };
}

// ##########################################################################
// 准备：记住当前牌组（结束时还原）、建一个「父::子」临时牌组（用来验证折叠）
// ##########################################################################

const stamp = Date.now() % 100_000;
const parentName = `TauriDecks${stamp}`;
const childName = `${parentName}::Child`;
const createdName = `TauriCreated${stamp}`;
const originalDeck = await currentDeckId();

const addDeck = async (name) =>
    BigInt(
        OpChangesWithId.fromBinary(
            await rpc(
                "addDeck",
                new Deck({ name, kind: { case: "normal", value: {} } }).toBinary(),
            ),
        ).id,
    );

// 空的**默认牌组**会被后端从树里藏掉（`hide_default_deck`，`rslib/src/decks/tree.rs:182`：
// 只要还有别的牌组，默认牌组就不显示），于是「当前牌组有高亮」会因为默认牌组压根没渲染出来
// 而失败 —— 真实用户的默认牌组通常有卡片。所以先确保当前牌组里有卡片
// （与 `verify-editor.ps1` 给复习界面补卡片同一思路）
const searchNotes = async (query) =>
    SearchResponse.fromBinary(
        await rpc("searchNotes", new SearchRequest({ search: query }).toBinary()),
    ).ids;
if ((await searchNotes(`did:${originalDeck}`)).length === 0) {
    const names = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    );
    // 与 `seed-card.mjs` 一样优先挑普通问答类型：新建集合里 `entries[0]` 可能是
    // 「遮盖」/Image Occlusion，那样补出来的是一张遮挡笔记 —— 编辑器检查（`editor-check.mjs`）
    // 接着要编辑复习队列里的这张卡，会因为没有可编辑字段而失败
    const pick =
        names.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ?? names.entries[0];
    const notetype = Notetype.fromBinary(
        await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
    );
    await rpc(
        "addNote",
        new AddNoteRequest({
            note: new Note({
                notetypeId: notetype.id,
                // 字段数必须与笔记类型一致（新建集合里第一个可能是 5 个字段的 Image Occlusion）
                fields: notetype.fields.map((field, index) =>
                    index === 0 ? `TauriDecks 补的卡片${stamp}` : field.name,
                ),
            }),
            deckId: originalDeck,
        }).toBinary(),
    );
    console.log(`当前牌组（${originalDeck}）是空的，补了一张卡片（否则它不会出现在列表里）`);
}

const parentId = await addDeck(parentName);
// `addDeck` 走上游的 `col.decks.id(name)`：`::` 里的祖先牌组会被顺带建出来
const childId = await addDeck(childName);

console.log(
    `临时牌组 ${parentName}（${parentId}）/ 子牌组（${childId}）/ 原当前牌组 ${originalDeck}`,
);

const browser = await chromium.launch();
let cleanedUp = false;

try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "main" });

    const bridgeCalls = () => page.evaluate(() => window.__bridgeCalls ?? []);
    const deckRows = () =>
        page.evaluate(() =>
            [...document.querySelectorAll("tbody tr")].map((row) => ({
                id: row.dataset.deck,
                text: row.querySelector("td.name")?.textContent?.trim() ?? "",
                current: row.classList.contains("current"),
            })),
        );
    /** 点某个牌组行的齿轮，再点菜单里的一项。 */
    const gearAction = async (deckId, action) => {
        await page.locator(`[data-gear="${deckId}"]`).click();
        await page.waitForSelector(".context-menu", { timeout: 5000 });
        await page.locator(`.context-menu [data-action="${action}"]`).click();
        await page.waitForTimeout(600);
    };

    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 20_000 });

    // 1. 表头与列（对照 `deckbrowser.py:212-231`）############################
    const headers = await page.evaluate(() =>
        [...document.querySelectorAll("table.list thead th")].map((th) => th.className),
    );
    check("表头是「牌组 + 三个计数 + 齿轮」", headers.length === 5, headers.join(" | "));
    check(
        "第一个表头是牌组名列",
        headers[0].includes("name") && headers.at(-1).includes("gear"),
        headers.join(" | "),
    );

    const rows = await deckRows();
    check(
        "列出了临时牌组",
        rows.some((row) => row.text.includes(parentName)),
        rows.map((row) => row.text).join(" / "),
    );
    check(
        "子牌组默认可见（父牌组没被折叠）",
        rows.some((row) => row.text.includes("Child")),
    );
    check(
        "当前牌组有高亮",
        rows.some((row) => row.current),
        rows.filter((row) => row.current).map((row) => row.text).join(" / "),
    );

    // 2. 折叠 / 展开（`collapse:<did>` 落在集合的 `study_collapsed` 上）######
    const parentRow = page.locator(`tbody tr[data-deck="${parentId}"]`);
    await parentRow.locator(".collapse").click();
    await page.waitForTimeout(1200);
    check(
        "折叠父牌组后子牌组消失",
        !(await deckRows()).some((row) => row.text.includes("Child")),
    );
    await parentRow.locator(".collapse").click();
    await page.waitForTimeout(1200);
    check(
        "再点一次展开",
        (await deckRows()).some((row) => row.text.includes("Child")),
    );

    // 3. 点牌组名 → 概览（`open:<did>`：设为当前牌组 + 切到概览）###########
    await page.locator(`tbody tr[data-deck="${childId}"] td.name a`).click();
    await page.waitForURL(/\/decks\/\?deck=/, { timeout: 15_000 });
    await page.waitForSelector(".overview", { timeout: 15_000 });
    // SvelteKit 会把 `/decks/?deck=` 规范化成 `/decks?deck=`，两种都接受
    check("点牌组进概览（URL 带 ?deck=）", /\/decks\/?\?deck=\d+/.test(page.url()), page.url());
    check(
        "概览里的牌组名正确",
        (await page.locator(".overview h3").textContent())?.includes("Child") ?? false,
    );
    check(
        "概览把该牌组设为当前牌组",
        (await currentDeckId()) === childId,
        `${await currentDeckId()}`,
    );
    check("概览有「学习」按钮", (await page.locator("#deck-study").count()) === 1);
    // 概览里的「自定义学习」（对照 Qt 概览底部的「C」，`overview.py:284-292`）
    check(
        "概览有「自定义学习」按钮",
        (await page.locator("#deck-custom-study").count()) === 1,
    );
    await page.locator("#deck-custom-study").click();
    await page.waitForTimeout(800);
    check(
        "点「自定义学习」发出 customStudy:<did>",
        (await bridgeCalls()).includes(`customStudy:${childId}`),
        `customStudy:${childId}`,
    );

    // 4. 概览里点「学习」→ 宿主切到复习页 ####################################
    await page.locator("#deck-study").click();
    await page.waitForTimeout(1200);
    check("概览「学习」发出 study", (await bridgeCalls()).includes("study"));

    // 5. 齿轮菜单：选项 / 重命名 / 删除 ####################################
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector(`tbody tr[data-deck="${parentId}"]`, { timeout: 15_000 });
    await gearAction(parentId, "options");
    check(
        "齿轮 → 选项发出 opts:<did>",
        (await bridgeCalls()).includes(`opts:${parentId}`),
        `opts:${parentId}`,
    );

    const renamed = `TauriRenamed${stamp}`;
    await gearAction(parentId, "rename");
    await page.waitForSelector(".modal.show #deck-name", { timeout: 5000 });
    await page.locator(".modal.show #deck-name").fill(renamed);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1200);
    check(
        "重命名后列表里是新名字",
        (await deckRows()).some((row) => row.text.includes(renamed)),
        (await deckRows()).map((row) => row.text).join(" / "),
    );
    await gearAction(parentId, "rename");
    await page.waitForSelector(".modal.show #deck-name", { timeout: 5000 });
    await page.locator(".modal.show #deck-name").fill(parentName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1200);

    // 导出与概览两节共用的夹具工具：随便挑一个普通问答类型，按它的字段数造一条笔记
    const basicNotetype = Notetype.fromBinary(
        await rpc(
            "getNotetype",
            new NotetypeId({
                ntid: NotetypeNames.fromBinary(
                    await rpc("getNotetypeNames", new Empty({}).toBinary()),
                ).entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)).id,
            }).toBinary(),
        ),
    );
    const addNoteTo = async (deckId, firstField) =>
        AddNoteResponse.fromBinary(
            await rpc(
                "addNote",
                new AddNoteRequest({
                    note: new Note({
                        notetypeId: basicNotetype.id,
                        fields: basicNotetype.fields.map((field, index) =>
                            index === 0 ? firstField : field.name,
                        ),
                    }),
                    deckId,
                }).toBinary(),
            ),
        ).noteId;

    // 5b. 齿轮 → 导出（`deckbrowser.py:317-319` → `main.py:1398-1401`）#########
    // 夹具：父牌组里一张笔记（marker），默认牌组里另一张（另一个 marker）。
    // 导成「笔记纯文本」后直接读文件断言 —— 这样能真的验证「只导了这个牌组」
    // （`ExportLimit` 是双层 oneof，少嵌一层会静默变成整个集合，见 §5.33）。
    const exportDir = process.env.ANKI_SAVE_PATH ?? "";
    if (!exportDir) {
        console.log("没有设置 ANKI_SAVE_PATH，跳过「齿轮 → 导出」检查");
    } else {
        const deckMarker = `TauriDeckExport${stamp}`;
        const otherMarker = `TauriOtherExport${stamp}`;
        const deckNote = await addNoteTo(parentId, deckMarker);
        const otherNote = await addNoteTo(originalDeck, otherMarker);

        const textFiles = () => fs.readdirSync(exportDir).filter((name) => name.endsWith(".txt"));
        const before = new Set(textFiles());
        await gearAction(parentId, "export");
        await page.waitForSelector(".modal.show #export-format", { timeout: 5000 });
        const scopeOptions = await page.locator("#export-scope option").allInnerTexts();
        check(
            "齿轮导出的范围默认是「该牌组」",
            (await page.locator("#export-scope").inputValue()) === "deck",
            scopeOptions.join(" / "),
        );
        await page.locator("#export-format").selectOption("notes-txt");
        await page.waitForTimeout(200);
        await page.locator("#export-confirm").click();
        await page.waitForTimeout(3000);
        const produced = textFiles().find((name) => !before.has(name));
        const exportedText = produced
            ? fs.readFileSync(path.join(exportDir, produced), "utf8")
            : "";
        // 建议文件名用牌组名（上游 `exporting.py:173-180`；非法字符换成 `_`）
        const expectedName = `${parentName.replace(/[\\/?<>:*|"^]/g, "_")}.txt`;
        check(
            "齿轮导出的文件名用了牌组名",
            produced === expectedName,
            `${produced ?? "（没有新文件）"}（期望 ${expectedName}）`,
        );
        check(
            "导出内容只有这个牌组的笔记",
            exportedText.includes(deckMarker) && !exportedText.includes(otherMarker),
            `含本牌组标记=${exportedText.includes(deckMarker)} 含别的牌组标记=${exportedText.includes(otherMarker)}`,
        );
        if (produced) {
            fs.rmSync(path.join(exportDir, produced), { force: true });
        }
        await rpc(
            "removeNotes",
            new RemoveNotesRequest({ noteIds: [deckNote, otherNote], cardIds: [] }).toBinary(),
        );
        check(
            "已清理导出检查的笔记",
            (await searchNotes(deckMarker)).length === 0 &&
                (await searchNotes(otherMarker)).length === 0,
        );
    }

    // 5c. 概览：描述 + 取消埋藏（`overview.py`）##############################
    // 描述：先写一段 HTML → 概览里要渲染出来；点「描述」改一段 Markdown → 概览渲染成 HTML
    const descMarker = `TauriDesc${stamp}`;
    const descEdited = `TauriDescEdited${stamp}`;
    const parentDeck = () =>
        rpc("getDeck", new DeckId({ did: parentId }).toBinary()).then((bytes) =>
            Deck.fromBinary(bytes),
        );
    {
        const deck = await parentDeck();
        deck.kind.value.description = `<b>${descMarker}</b>`;
        deck.kind.value.markdownDescription = false;
        await rpc("updateDeck", deck.toBinary());

        await page.locator(`tbody tr[data-deck="${parentId}"] td.name a`).click();
        await page.waitForSelector(".overview", { timeout: 15_000 });
        await page.waitForSelector("#deck-description-text", { timeout: 15_000 });
        const shown = (await page.locator("#deck-description-text").innerText()).trim();
        check("概览渲染了牌组描述", shown.includes(descMarker), shown);

        await page.locator("#deck-description").click();
        await page.waitForSelector(".modal.show #description-text", { timeout: 5000 });
        check(
            "描述弹框预填了当前描述",
            (await page.locator("#description-text").inputValue()).includes(descMarker),
        );
        await page.locator("#description-text").fill(`**${descEdited}**`);
        await page.locator("#description-markdown").check();
        await page.locator("#description-save").click();
        await page.waitForTimeout(1500);

        const saved = await parentDeck();
        check(
            "描述已保存（含 markdown 开关）",
            saved.kind.value.description === `**${descEdited}**` &&
                saved.kind.value.markdownDescription === true,
            JSON.stringify({
                desc: saved.kind.value.description,
                md: saved.kind.value.markdownDescription,
            }),
        );
        const rendered = (await page.locator("#deck-description-text").innerText()).trim();
        check(
            "markdown 描述渲染成了 HTML",
            rendered.includes(descEdited) && !rendered.includes("**"),
            rendered,
        );
    }

    // 取消埋藏：埋一张父牌组的卡片 → 概览里出现「取消埋藏」→ 点掉它 → 标记消失
    {
        const buriedNote = await addNoteTo(parentId, `TauriBuried${stamp}`);
        const cardIds = CardIds.fromBinary(
            await rpc("cardsOfNote", new NoteId({ nid: buriedNote }).toBinary()),
        ).cids;
        await rpc(
            "buryOrSuspendCards",
            new BuryOrSuspendCardsRequest({
                cardIds,
                mode: BuryOrSuspendCardsRequest_Mode.BURY_USER,
            }).toBinary(),
        );
        check(
            "埋藏后 congratsInfo 报告用户埋藏",
            CongratsInfoResponse.fromBinary(
                await rpc("congratsInfo", new Empty({}).toBinary()),
            ).haveUserBuried === true,
        );

        await page.reload({ waitUntil: "load" });
        await page.waitForSelector(".overview", { timeout: 15_000 });
        await page.waitForSelector("#deck-unbury", { timeout: 15_000 });
        check("埋藏之后概览出现「取消埋藏」", (await page.locator("#deck-unbury").count()) === 1);
        await page.locator("#deck-unbury").click();
        await page.waitForTimeout(1500);
        check(
            "点「取消埋藏」后按钮消失",
            (await page.locator("#deck-unbury").count()) === 0,
        );
        check(
            "RPC 复查：没有用户埋藏的卡片了",
            CongratsInfoResponse.fromBinary(
                await rpc("congratsInfo", new Empty({}).toBinary()),
            ).haveUserBuried === false,
        );
        await rpc(
            "removeNotes",
            new RemoveNotesRequest({ noteIds: [buriedNote], cardIds: [] }).toBinary(),
        );
        check("已清理埋藏检查的笔记", (await searchNotes(`TauriBuried${stamp}`)).length === 0);
    }

    // 5d. 拖拽移动牌组（`deckbrowser.ts` 的拖放 → `reparentDecks`）#############
    // 上游是 jQuery UI 的拖放 + `pycmd("drag:source,target")`；这里用页面内的
    // `DragEvent` 合成（`locator.dispatchEvent` 造的事件在页面里读不到 dataTransfer，
    // 见 TAURI_HANDOVER §5.32）
    const deckNames = async () =>
        DeckNames.fromBinary(
            await rpc(
                "getDeckNames",
                new GetDeckNamesRequest({ skipEmptyDefault: false }).toBinary(),
            ),
        ).entries.map((entry) => entry.name);
    const dragDeckOnto = async (sourceId, targetSelector) =>
        page.evaluate(
            ({ source, selector }) => {
                const sourceRow = document.querySelector(`tbody tr[data-deck="${source}"]`);
                const targetRow = document.querySelector(selector);
                const dataTransfer = new DataTransfer();
                sourceRow.dispatchEvent(
                    new DragEvent("dragstart", { dataTransfer, bubbles: true }),
                );
                for (const kind of ["dragenter", "dragover"]) {
                    targetRow.dispatchEvent(
                        new DragEvent(kind, { dataTransfer, bubbles: true, cancelable: true }),
                    );
                }
                targetRow.dispatchEvent(
                    new DragEvent("drop", { dataTransfer, bubbles: true, cancelable: true }),
                );
                sourceRow.dispatchEvent(new DragEvent("dragend", { dataTransfer, bubbles: true }));
            },
            { source: String(sourceId), selector: targetSelector },
        );

    const dragName = `TauriDrag${stamp}`;
    const dragId = await addDeck(dragName);
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector(`tbody tr[data-deck="${dragId}"]`, { timeout: 15_000 });
    check("拖拽前是顶层牌组", (await deckNames()).includes(dragName));
    await dragDeckOnto(dragId, `tbody tr[data-deck="${parentId}"]`);
    await page.waitForTimeout(1500);
    check(
        "拖到另一个牌组上 → 变成它的子牌组",
        (await deckNames()).includes(`${parentName}::${dragName}`),
        (await deckNames()).filter((name) => name.includes(dragName)).join(" / "),
    );
    check(
        "子牌组出现在列表里（父牌组下面）",
        (await deckRows()).some((row) => row.text.includes(dragName)),
    );
    await dragDeckOnto(dragId, "tbody tr.top-level-drag-row");
    await page.waitForTimeout(1500);
    check(
        "拖到顶部那条 → 回到顶层",
        (await deckNames()).includes(dragName) &&
            !(await deckNames()).includes(`${parentName}::${dragName}`),
        (await deckNames()).filter((name) => name.includes(dragName)).join(" / "),
    );
    await rpc("removeDecks", new DeckIds({ dids: [dragId] }).toBinary());
    check("已清理拖拽检查的牌组", !(await deckNames()).includes(dragName));

    // 5e. 菜单栏（`main.ui` 的 menuCol / menuEdit / menuTools / menuHelp）#######
    // 断言菜单渲染，并让几个能验证的动作真的走通（拦 RPC 请求 / 看桥接命令 / 等产物）。
    // 「退出」与两个帮助链接只断言存在 —— 点了会真的退出 / 打开系统浏览器。
    const posted = [];
    page.on("request", (request) => posted.push(request.url().split("/").pop()));
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });
    const menuTitles = await page.evaluate(() =>
        [...document.querySelectorAll(".menubar [data-menu-title]")].map((el) =>
            el.textContent.trim(),
        ),
    );
    check("菜单栏有五个菜单", menuTitles.length === 5, menuTitles.join(" / "));
    // 按下标点：文案随语言变（文件 / 编辑 / 视图 / 工具 / 帮助）
    const clickMenu = async (index, action) => {
        await page.locator(".menubar [data-menu-title]").nth(index).click();
        await page.waitForSelector(`.menu-popup [data-menu="${action}"]`, { timeout: 5000 });
        await page.locator(`.menu-popup [data-menu="${action}"]`).click();
        await page.waitForTimeout(1000);
    };

    // 文件 → 创建备份（走宿主：`backup` 桥接命令 → `crate::backup::create_now`）
    const collectionPath = process.env.ANKI_COLLECTION ?? "";
    const backupDir = collectionPath ? path.join(path.dirname(collectionPath), "backups") : "";
    const backupsBefore = new Set(
        backupDir && fs.existsSync(backupDir) ? fs.readdirSync(backupDir) : [],
    );
    await clickMenu(0, "backup");
    let newBackup = null;
    for (let attempt = 0; attempt < 20 && !newBackup; attempt++) {
        await page.waitForTimeout(500);
        const files = backupDir && fs.existsSync(backupDir) ? fs.readdirSync(backupDir) : [];
        newBackup = files.find((name) => !backupsBefore.has(name)) ?? null;
    }
    check("文件 → 创建备份产出了新的 .colpkg", !!newBackup, newBackup ?? "（没有新文件）");

    // 文件 → 导出…（范围默认当前牌组）与 `Ctrl+E` 等价
    await clickMenu(0, "export");
    await page.waitForSelector(".modal.show #export-format", { timeout: 5000 });
    check(
        "文件 → 导出… 打开导出对话框（范围默认当前牌组）",
        (await page.locator("#export-scope").inputValue()) === "deck",
    );
    await page.locator(".modal.show .btn-close").click();
    await page.waitForTimeout(500);
    await page.keyboard.press("Control+e");
    await page.waitForSelector(".modal.show #export-format", { timeout: 5000 });
    check("Ctrl+E 也能打开导出对话框", (await page.locator(".modal.show #export-format").count()) === 1);
    await page.locator(".modal.show .btn-close").click();
    await page.waitForTimeout(500);

    // 视图 → 放大 / 缩小 / 重置（宿主侧 `set_zoom`）与全屏（F11 → `set_fullscreen`）；
    // 宿主真的改了窗口状态，由 verify-decks.ps1 断言日志（`缩放: ` / `全屏: `）
    for (const [action, bridge] of [
        ["zoomIn", "zoom:in"],
        ["zoomOut", "zoom:out"],
        ["zoomReset", "zoom:reset"],
    ]) {
        await clickMenu(2, action);
        check(`视图 → ${action} 发出 ${bridge}`, (await bridgeCalls()).includes(bridge));
    }
    await page.keyboard.press("F11");
    await page.waitForTimeout(500);
    check("F11 发出 fullscreen", (await bridgeCalls()).includes("fullscreen"));
    await page.keyboard.press("F11"); // 切回窗口模式，别影响后面的检查
    await page.waitForTimeout(500);

    // 工具 → 检查数据库 / 检查媒体 / 空卡片（空卡片的报告在没有空卡片时本来就是空的，
    // 所以那一项只要求请求 + 弹框）
    for (const [action, method, label, needsText] of [
        ["checkDatabase", "checkDatabase", "检查数据库", true],
        ["checkMedia", "checkMedia", "检查媒体", true],
        ["emptyCards", "getEmptyCards", "空卡片", false],
    ]) {
        posted.length = 0;
        await clickMenu(3, action);
        await page.waitForSelector(".modal.show #deck-report-body", { timeout: 20_000 });
        const body = (await page.locator("#deck-report-body").innerText()).trim();
        check(
            `工具 → ${label} 调了 ${method} 并弹出报告`,
            posted.includes(method) && (!needsText || body.length > 0),
            `${method}${body ? ` / ${body.slice(0, 40)}` : ""}`,
        );
        await page.locator(".modal.show .btn-close").click();
        await page.waitForTimeout(500);
    }

    // 编辑 → 撤销（没有可撤销的东西时后端会报错，页面只记 console.error，请求照样发出）
    posted.length = 0;
    await clickMenu(1, "undo");
    check("编辑 → 撤销调了 undo", posted.includes("undo"), posted.filter((m) => m === "undo").length + " 次");

    // 文件 → 导入…（桥接命令，宿主弹原生文件对话框）
    await clickMenu(0, "import");
    check("文件 → 导入… 发出 import", (await bridgeCalls()).includes("import"));

    // 帮助 / 文件菜单里那些"不能点"的项只断言存在
    await page.locator(".menubar [data-menu-title]").nth(4).click();
    await page.waitForTimeout(300);
    const helpActions = await page.evaluate(() =>
        [...document.querySelectorAll(".menu-popup [data-menu]")].map((el) => el.dataset.menu),
    );
    check(
        "帮助菜单有用户手册与支持 Anki",
        helpActions.includes("guide") && helpActions.includes("donate"),
        helpActions.join(" / "),
    );
    // 帮助 → 关于：版本信息由宿主给（`about` 桥接命令 → JSON）
    await page.locator('.menu-popup [data-menu="about"]').click();
    await page.waitForSelector(".modal.show #deck-report-body", { timeout: 5000 });
    const aboutText = (await page.locator("#deck-report-body").innerText()).trim();
    check(
        "帮助 → 关于 显示版本信息",
        /Anki/.test(aboutText) && /\d+\.\d+/.test(aboutText),
        (aboutText.split("\n").find((line) => /\d+\.\d+/.test(line)) ?? aboutText).slice(0, 50),
    );
    await page.locator(".modal.show .btn-close").click();
    await page.waitForTimeout(300);
    await page.locator(".menubar [data-menu-title]").nth(2).click();
    await page.waitForTimeout(300);
    const viewActions = await page.evaluate(() =>
        [...document.querySelectorAll(".menu-popup [data-menu]")].map((el) => el.dataset.menu),
    );
    check(
        "视图菜单有全屏与三档缩放",
        ["fullscreen", "zoomIn", "zoomOut", "zoomReset"].every((action) =>
            viewActions.includes(action)
        ),
        viewActions.join(" / "),
    );
    await page.locator(".menubar [data-menu-title]").nth(0).click();
    await page.waitForTimeout(300);
    const fileActions = await page.evaluate(() =>
        [...document.querySelectorAll(".menu-popup [data-menu]")].map((el) => el.dataset.menu),
    );
    check("文件菜单有退出", fileActions.includes("quit"), fileActions.join(" / "));

    // 5f. 工具 → 学习牌组（`aqt/studydeck.py`：筛选框 + 名字列表 + 添加 / 学习）####
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });
    await clickMenu(3, "studyDeck");
    await page.waitForSelector(".modal.show #study-deck-list", { timeout: 5000 });
    // 等列表渲染出条目（`{#each}` 与响应式重算是异步的）
    await page.waitForSelector("#study-deck-list [data-deck-name]", { timeout: 5000 });
    const listedNames = await page.evaluate(() =>
        [...document.querySelectorAll("#study-deck-list [data-deck-name]")].map((el) =>
            el.dataset.deckName
        ),
    );
    check("学习牌组列出所有牌组", listedNames.includes(parentName), `${listedNames.length} 个`);

    // 筛选框：空格分开的词都要命中（上游 `StudyDeck._matches`）
    await page.locator(".modal.show #study-deck-filter").fill(`tauri ${stamp}`);
    await page.waitForTimeout(300);
    const afterFilter = await page.evaluate(() =>
        [...document.querySelectorAll("#study-deck-list [data-deck-name]")].map((el) =>
            el.dataset.deckName
        ),
    );
    check(
        "筛选框按空格分词匹配",
        afterFilter.includes(parentName),
        afterFilter.join(" / ") || "（空）",
    );

    // 选中子牌组 → 学习：设为当前牌组并进概览（上游 `set_current_deck` → 概览）
    await page.locator(".modal.show #study-deck-filter").fill("child");
    await page.waitForTimeout(300);
    await page.locator(`.modal.show #study-deck-list [data-deck-name="${childName}"]`).click();
    await page.locator(".modal.show #study-deck-study").click();
    await page.waitForTimeout(1500);
    check("学习牌组 → 学习：进了该牌组的概览", page.url().includes(`deck=${childId}`), page.url());
    check(
        "概览显示的是选中的牌组",
        ((await page.locator(".overview h3").textContent()) ?? "").includes("Child"),
        (await page.locator(".overview h3").textContent()) ?? "",
    );

    // 「添加」：新建后直接进那个牌组的概览（上游 `onAddDeck` 的 success 回调）
    const studyCreatedName = `TauriStudy${stamp}`;
    await clickMenu(3, "studyDeck");
    await page.locator(".modal.show #study-deck-add").click();
    await page.waitForSelector(".modal.show #deck-name", { timeout: 5000 });
    await page.locator(".modal.show #deck-name").fill(studyCreatedName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(2500);
    const studyCreatedTitle = (await page.locator(".overview h3").textContent()) ?? "";
    check(
        "学习牌组 → 添加：新建后直接进概览",
        page.url().includes("?deck=") && studyCreatedTitle.includes(studyCreatedName),
        studyCreatedTitle.trim() || page.url(),
    );

    // 收尾：删掉新建的牌组，并把页面带回牌组列表（第 6 节从这里继续）
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });
    const studyRow = (await deckRows()).find((row) => row.text.includes(studyCreatedName));
    if (studyRow?.id) {
        await rpc("removeDecks", new DeckIds({ dids: [BigInt(studyRow.id)] }).toBinary());
    }
    check("已清理「学习牌组 → 添加」建的牌组", !(await deckNames()).includes(studyCreatedName));

    // 5g. 工具 → 创建筛选牌组（`qt/aqt/filtered_deck.py`）#######################
    // 先给父牌组塞一张卡片，好让 `deck:<父牌组>` 这个搜索词能匹配到卡片
    const filteredNote = await addNoteTo(parentId, `TauriFiltered${stamp}`);
    const filteredName = `TauriFiltered${stamp}`;
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });
    // 工具菜单里两项都在（`main.ui`：actionStudyDeck / actionCreateFiltered）
    await page.locator(".menubar [data-menu-title]").nth(3).click();
    await page.waitForTimeout(300);
    const toolsActions = await page.evaluate(() =>
        [...document.querySelectorAll(".menu-popup [data-menu]")].map((el) => el.dataset.menu),
    );
    check(
        "工具菜单有学习牌组与创建筛选牌组",
        toolsActions.includes("studyDeck") && toolsActions.includes("createFiltered"),
        toolsActions.join(" / "),
    );
    await page.keyboard.press("Escape");
    // 快捷键 `F` 与菜单项（`main.ui` 的 `actionCreateFiltered`）都开同一个弹框
    await page.keyboard.press("f");
    await page.waitForSelector(".modal.show #filtered-name", { timeout: 10_000 });
    check(
        "快捷键 F 打开「创建筛选牌组」",
        (await page.locator(".modal.show #filtered-name").inputValue()).length > 0,
        await page.locator(".modal.show #filtered-name").inputValue(),
    );
    const orderLabels = await page.evaluate(() =>
        [...document.querySelectorAll(".modal.show #filtered-order option")].map(
            (option) => option.textContent,
        ),
    );
    check(
        "顺序下拉来自 filteredDeckOrderLabels（去掉两个 FSRS 专用项）",
        orderLabels.length === 9 && orderLabels.every((label) => (label ?? "").length > 0),
        `${orderLabels.length} 项`,
    );
    await page.locator(".modal.show #filtered-name").fill(filteredName);
    await page.locator(".modal.show #filtered-search").fill(`deck:${parentName}`);
    await page.locator(".modal.show #filtered-limit").fill("5");
    await page.locator(".modal.show #filtered-order").selectOption("6"); // DUE
    await page.locator(".modal.show #filtered-build").click();
    await page.waitForTimeout(2500);
    check(
        "构建后进了筛选牌组的概览",
        page.url().includes("?deck=") &&
            ((await page.locator(".overview h3").textContent()) ?? "").includes(filteredName),
        page.url(),
    );
    const filteredId = BigInt(
        (await page.locator(".overview #deck-rebuild").count()) > 0 ? page.url().split("deck=")[1] : "0",
    );
    const filteredDeck = filteredId
        ? await rpc("getDeck", new DeckId({ did: filteredId }).toBinary()).then((bytes) =>
            Deck.fromBinary(bytes)
        )
        : null;
    check(
        "后端建出的是筛选牌组",
        filteredDeck?.kind.case === "filtered",
        filteredDeck?.kind.case ?? "（没建出来）",
    );
    const filteredCards = await searchNotes(`did:${filteredId}`);
    check("筛选牌组里有匹配到的卡片", filteredCards.length > 0, `${filteredCards.length} 张`);
    check(
        "筛选牌组的搜索条件落库了",
        filteredDeck?.kind.case === "filtered" &&
            filteredDeck.kind.value.searchTerms[0]?.search === `deck:${parentName}`,
        filteredDeck?.kind.case === "filtered"
            ? (filteredDeck.kind.value.searchTerms[0]?.search ?? "（空）")
            : "（没建出来）",
    );

    // 收尾：删掉筛选牌组与那张夹具卡片，并把页面带回牌组列表
    await rpc("removeDecks", new DeckIds({ dids: [filteredId] }).toBinary());
    await rpc(
        "removeNotes",
        new RemoveNotesRequest({ noteIds: [filteredNote], cardIds: [] }).toBinary(),
    );
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });
    check("已清理筛选牌组与夹具卡片", !(await deckNames()).includes(filteredName));

    // 5h. 文件 → 还原备份（`main.py:441-477`）####################################
    // verify-decks 用 `ANKI_RESTORE_PATH=newest`：还原「备份目录里最新的 .colpkg」——
    // 那一份是 5e 里点「创建备份」生成的（在 parent/child 建好之后、这一节之前），
    // 所以还原后 parent/child 还在，而这一节新建的标记牌组应该消失（集合被整份替换）
    const restoreMarker = `TauriRestore${stamp}`;
    await addDeck(restoreMarker);
    check("还原前新建了标记牌组", (await deckNames()).includes(restoreMarker));
    await clickMenu(0, "restoreBackup");
    // 宿主：关集合 → 导入备份 → 重开 → 回牌组列表（页面会被重新导航一次）
    await page.waitForTimeout(6000);
    const namesAfterRestore = await deckNames();
    check(
        "还原后标记牌组消失（集合被备份替换）",
        !namesAfterRestore.includes(restoreMarker),
        namesAfterRestore.filter((name) => name.includes("Tauri")).join(" / ") || "（只剩测试牌组）",
    );
    check("还原后备份里的牌组还在", namesAfterRestore.includes(parentName));
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });

    // 6. 新建牌组 → 删除 ####################################################
    await page.locator("#deck-create").click();
    await page.waitForSelector(".modal.show #deck-name", { timeout: 5000 });
    await page.locator(".modal.show #deck-name").fill(createdName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    const created = (await deckRows()).find((row) => row.text.includes(createdName));
    check("新建的牌组出现在列表里", !!created, created?.id ?? "没找到");

    if (created) {
        await gearAction(created.id, "delete");
        await page.waitForSelector("#deck-delete-confirm", { timeout: 5000 });
        await page.locator("#deck-delete-confirm").click();
        await page.waitForTimeout(1500);
        check(
            "删除后牌组从列表消失",
            !(await deckRows()).some((row) => row.text.includes(createdName)),
        );
    }

    // 7. 底部与工具栏 #######################################################
    check("底部有「获取共享牌组」（不点：会开系统浏览器）", (await page.locator("#deck-shared").count()) === 1);
    await page.locator("#deck-import").click();
    await page.waitForTimeout(1000);
    check("点「导入文件」发出 import", (await bridgeCalls()).includes("import"));

    const toolbar = await page.evaluate(() =>
        [...document.querySelectorAll(".toolbar .link")].map((el) => el.textContent.trim()),
    );
    check("工具栏有四个链接（同步未实现）", toolbar.length === 4, toolbar.join(" / "));
    await page.locator(".toolbar .link").nth(2).click();
    await page.waitForTimeout(800);
    check("工具栏「浏览」发出 browse", (await bridgeCalls()).includes("browse"));

    // 8. 删除临时牌组（父+子一起删）########################################
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector(`tbody tr[data-deck="${parentId}"]`, { timeout: 15_000 });
    await gearAction(parentId, "delete");
    await page.waitForSelector("#deck-delete-confirm", { timeout: 5000 });
    await page.locator("#deck-delete-confirm").click();
    await page.waitForTimeout(1500);
    check(
        "临时牌组已删除（含子牌组）",
        !(await deckRows()).some((row) => row.text.includes(parentName)),
        (await deckRows()).map((row) => row.text).join(" / "),
    );
} finally {
    // 还原当前牌组（检查过程中点过牌组，会把它改掉）
    try {
        await rpc("setCurrentDeck", new DeckId({ did: originalDeck }).toBinary());
        await rpc(
            "removeDecks",
            new DeckIds({ dids: [parentId, childId] }).toBinary(),
        );
        cleanedUp = true;
    } catch (problem) {
        console.error(`清理失败: ${problem}`);
    }
    await browser.close();
}

console.log(
    failures.length === 0
        ? `牌组列表检查：全部通过（清理${cleanedUp ? "完成" : "失败"}）`
        : `牌组列表检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
