// 自定义学习窗口（`ts/routes/custom-study/`，对照 `qt/aqt/customstudy.py`）的检查。
//
//   1. 六个单选项 + 数字框都渲染出来，默认选第一项（增加今天的新卡片上限）；
//   2. 切到「按卡片状态或标签学习」会多出四选一的列表与标签输入；
//   3. 点确定 → 页面调 `customStudy` RPC → 后端建/复用「Custom Study Session」
//      筛选牌组并把当前牌组切过去 → 页面发 `customStudyDone`（宿主据此关窗 + 导航）；
//   4. 用 RPC 复查"确实多了一个筛选牌组、且它就是当前牌组"，然后还原。
//
// 为什么用 Playwright：要真的点单选框、填数字、点确定，并观察页面发往宿主的命令。
// Chromium 与 WebView2 同源，加载同一个 URL，按 `tauri/src/bridge.rs` 补垫片。
//
// 前置：外壳已启动（`ANKI_START_PAGE=decks` + 固定 `ANKI_API_PORT`），集合里有卡片。
//
// 用法：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/custom-study-check.mjs

import fs from "node:fs";

import { chromium } from "playwright";

import {
    Deck,
    DeckId,
    DeckIds,
    DeckTreeNode,
    DeckTreeRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";

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
            // 不留 keep-alive 句柄（见 m2-check.mjs 的说明）
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/** 发一条桥接命令（模拟页面里的 `pycmd`；`X-Anki-Window` 默认主窗口）。 */
async function sendBridge(command, window = "main") {
    const response = await fetch(`${base}/_anki/pycmd`, {
        method: "POST",
        headers: {
            "Content-Type": "text/plain",
            Authorization: `Bearer ${apiKey}`,
            "X-Anki-Window": window,
            Connection: "close",
        },
        body: command,
    });
    return response.status;
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

/** 展平整棵牌组树（`level`/`children` 都是后端给的）。 */
async function deckNodes() {
    const tree = DeckTreeNode.fromBinary(
        await rpc(
            "deckTree",
            new DeckTreeRequest({ now: BigInt(Math.floor(Date.now() / 1000)) }).toBinary(),
        ),
    );
    const out = [];
    const walk = (node) => {
        out.push(node);
        for (const child of node.children) {
            walk(child);
        }
    };
    for (const child of tree.children) {
        walk(child);
    }
    return out;
}

const currentDeck = () =>
    rpc("getCurrentDeck", new Empty({}).toBinary()).then((bytes) => Deck.fromBinary(bytes));

// ##########################################################################
// 准备：找一个普通（非筛选）牌组当目标
// ##########################################################################

const before = await deckNodes();
// 优先挑**有卡片的**普通牌组：集合里可能留着别的空牌组（上一次跑崩留下的），
// 挑到空牌组的话 cram（「全部卡片」）会因为一张卡片都没有而失败
const normalDecks = before.filter((node) => !node.filtered);
const target = normalDecks.find((node) => node.totalInDeck > 0) ?? normalDecks[0];
if (!target) {
    console.error("集合里没有普通牌组，先跑 seed-card.mjs 造一张卡片");
    process.exit(2);
}
const deckId = target.deckId;
const originalDeckId = (await currentDeck()).id;
console.log(`目标牌组: ${target.name}（${deckId}）/ 原当前牌组 ${originalDeckId}`);

const browser = await chromium.launch();
let cleaned = false;
try {
    const page = await browser.newPage({ viewport: { width: 700, height: 700 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "custom-study" });
    await page.goto(`${base}/custom-study/?deck=${deckId}`, { waitUntil: "load" });
    await page.waitForSelector("#custom-study-ok", { timeout: 20_000 });

    // 1. 六个选项 + 数字框（对照 `customstudy.py:90-156`）
    const radios = page.locator('input[name="custom-study-option"]');
    const radioCount = await radios.count();
    check("渲染出六个选项", radioCount === 6, `${radioCount} 项`);
    check("默认选中第一项（增加今天的新卡片上限）", await radios.nth(0).isChecked());
    // 可用卡片数是 `customStudyDefaults` 回来之后才有的 —— 这一条同时守住
    // "Svelte 不追踪函数体里的响应式变量"那个坑（见交接文档 §5.25）
    const hint = await page.locator('[data-hint="new"]').count();
    check(
        "第一项旁边显示了可用新卡片数",
        hint === 1 && ((await page.locator('[data-hint="new"]').innerText()) ?? "").length > 0,
        hint ? await page.locator('[data-hint="new"]').innerText() : "没有提示",
    );
    const spinnerText = await page.locator(".spinner-row").innerText();
    check(
        "数字框与前后缀文案都在",
        (await page.locator("#custom-study-spin").count()) === 1 && spinnerText.length > 0,
        spinnerText.replace(/\s+/g, " ").trim(),
    );

    // 2. 切到 cram → 多出四选一列表与标签输入
    await page.locator('input[data-option="cram"]').check();
    check(
        "cram 选项会展开四选一列表",
        (await page.locator("#custom-study-cram-kind").count()) === 1,
    );
    const kindCount = await page.locator("#custom-study-cram-kind option").count();
    check("cram 有四个卡片范围", kindCount === 4, `${kindCount} 个`);
    check(
        "cram 有标签包含/排除输入",
        (await page.locator("#custom-study-tags-include").count()) === 1 &&
            (await page.locator("#custom-study-tags-exclude").count()) === 1,
    );

    // 3. 「全部卡片（不按顺序）」+ 100 张 → 确定
    await page.locator("#custom-study-cram-kind").selectOption({ index: 3 });
    await page.locator("#custom-study-spin").fill("100");
    await page.locator("#custom-study-ok").click();
    await page.waitForTimeout(2500);
    const calls = await page.evaluate(() => window.__bridgeCalls ?? []);
    check("确定后发了 customStudyDone", calls.includes("customStudyDone"), calls.join("、"));
    const errorCount = await page.locator("#custom-study-error").count();
    check(
        "没有出现错误提示",
        errorCount === 0,
        errorCount ? await page.locator("#custom-study-error").innerText() : "",
    );

    const shotDir = process.env.SHOT_DIR ?? "out/decks-check";
    fs.mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: `${shotDir}/custom-study.png` });
    console.log(`已截图: ${shotDir}/custom-study.png`);

    // 4. 复查：多了一个筛选牌组，而且它就是当前牌组
    const after = await deckNodes();
    const filtered = after.find((node) => node.filtered);
    check("后端建出了筛选牌组", !!filtered, filtered?.name ?? "没找到");
    if (filtered) {
        const current = await currentDeck();
        check(
            "当前牌组已切到筛选牌组",
            current.id === filtered.deckId,
            `${current.name}（${current.id}）`,
        );

        // 4b. 筛选牌组的概览：有「重建 / 清空」、没有「自定义学习」##############
        // （对照 `overview.py:283-297` 的底部按钮与 `:126-133` 的快捷键 r/e）
        const cardsInDeck = async () =>
            SearchResponse.fromBinary(
                await rpc(
                    "searchCards",
                    new SearchRequest({ search: `did:${filtered.deckId}` }).toBinary(),
                ),
            ).ids.length;
        await page.goto(`${base}/decks/?deck=${filtered.deckId}`, { waitUntil: "load" });
        await page.waitForSelector(".overview", { timeout: 20_000 });
        check(
            "筛选牌组的概览有「重建 / 清空」，没有「自定义学习」",
            (await page.locator("#deck-rebuild").count()) === 1 &&
                (await page.locator("#deck-empty").count()) === 1 &&
                (await page.locator("#deck-custom-study").count()) === 0,
        );
        const beforeEmpty = await cardsInDeck();
        await page.locator("#deck-empty").click();
        await page.waitForTimeout(1500);
        const afterEmpty = await cardsInDeck();
        check(
            "点「清空」后筛选牌组里没有卡片",
            beforeEmpty > 0 && afterEmpty === 0,
            `${beforeEmpty} → ${afterEmpty}`,
        );
        await page.keyboard.press("r");
        await page.waitForTimeout(1500);
        check("按 `r` 重建筛选牌组", (await cardsInDeck()) > 0, `${await cardsInDeck()} 张`);

        // 4c. 筛选牌组的「选项」（PARITY 3.1）：对照 `deckoptions.py:125-134`，dyn 牌组的
        //     「选项」开的是 `FilteredDeckConfigDialog`，**不是**牌组选项页（后者对筛选牌组
        //     会直接报错）。宿主应开 `filtered-deck` 窗口，页面用 `?filtered=<id>` 预填编辑器。
        if (process.env.ANKI_LOG) {
            const logOf = () => {
                try {
                    return fs.readFileSync(process.env.ANKI_LOG, "utf8");
                } catch {
                    return "";
                }
            };
            const countOf = (pattern) => logOf().split(pattern).length - 1;
            const deckOptionsBefore = countOf("打开窗口: deck-options ->");
            const filteredBefore = countOf("打开窗口: filtered-deck ->");
            const status = await sendBridge(`opts:${filtered.deckId}`);
            check("opts:<筛选牌组> 被接受", status === 200, `HTTP ${status}`);
            const deadline = Date.now() + 20_000;
            while (
                Date.now() < deadline &&
                countOf("打开窗口: filtered-deck ->") <= filteredBefore
            ) {
                await new Promise((resolve) => setTimeout(resolve, 300));
            }
            check(
                "筛选牌组的「选项」开的是 filtered-deck 窗口",
                countOf("打开窗口: filtered-deck ->") > filteredBefore,
            );
            check(
                "没有误开牌组选项页",
                countOf("打开窗口: deck-options ->") === deckOptionsBefore,
            );
        } else {
            console.log("没有 ANKI_LOG，跳过筛选牌组「选项」的宿主断言");
        }
        // 页面侧：`?filtered=<id>` 打开时要预填这个名字与搜索词
        await page.goto(`${base}/decks/?filtered=${filtered.deckId}`, { waitUntil: "load" });
        await page.waitForSelector(".modal.show #filtered-name", { timeout: 20_000 });
        const prefilledName = await page.locator(".modal.show #filtered-name").inputValue();
        const prefilledSearch = await page.locator(".modal.show #filtered-search").inputValue();
        check(
            "筛选牌组编辑器预填了名字",
            prefilledName.length > 0,
            `名字「${prefilledName}」`,
        );
        check(
            "筛选牌组编辑器预填了搜索词",
            prefilledSearch.length > 0,
            `搜索「${prefilledSearch}」`,
        );

        await rpc("removeDecks", new DeckIds({ dids: [filtered.deckId] }).toBinary());
        await rpc("setCurrentDeck", new DeckId({ did: originalDeckId }).toBinary());
        check(
            "临时筛选牌组已删除",
            !(await deckNodes()).some((node) => node.filtered),
        );
        cleaned = true;
    }
} catch (error) {
    check("自定义学习流程", false, String(error));
} finally {
    await browser.close();
}

console.log(
    failures.length === 0
        ? `自定义学习检查：全部通过（清理${cleaned ? "完成" : "失败"}）`
        : `自定义学习检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
