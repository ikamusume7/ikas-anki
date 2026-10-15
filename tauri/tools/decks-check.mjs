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

import { Deck, DeckId, DeckIds } from "../../out/ts/lib/generated/anki/decks_pb.js";
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
    window.bridgeCommand = window.pycmd = (arg) => {
        window.__bridgeCalls.push(String(arg));
        window
            .fetch("/_anki/pycmd", {
                method: "POST",
                headers: { "Content-Type": "text/plain" },
                body: String(arg),
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
