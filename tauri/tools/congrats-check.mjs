// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 答完队列的收尾（Congratulations）检查。对照 `qt/aqt/reviewer.py:257-259`
// （没卡片就 `moveToState("overview")`）与 `qt/aqt/overview.py:190-209`
// （概览没有可学卡片时换成 `/congrats` 那块内容）。
//
// 为了不依赖前面几步留下的状态，这个检查**自己造一个「今天学完」的牌组**：
// 新建临时牌组 → 放一张卡片并埋藏它（这样既有埋藏提示、队列又一定是空的）→
// 把当前牌组切过去 → 发 `study`（真实入口）→ 断言宿主把主窗口导到概览 →
// 用 Playwright 打开概览，断言画出的是恭喜内容、并真的点一下「取消埋藏」看它复原。
//
// 前置：外壳已启动（任意起始页）+ `ANKI_LOG` 指向外壳日志。
//
// 用法：
//   $env:ANKI_API_PORT="40028"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/reviewer-check/run.log"
//   node tauri/tools/congrats-check.mjs

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { chromium } from "playwright";

import { OpChangesWithId } from "../../out/ts/lib/generated/anki/collection_pb.js";
import { Deck, DeckId, DeckIds } from "../../out/ts/lib/generated/anki/decks_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { AddNoteRequest, Note } from "../../out/ts/lib/generated/anki/notes_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
} from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import {
    BuryOrSuspendCardsRequest,
    BuryOrSuspendCardsRequest_Mode,
    CongratsInfoResponse,
    GetQueuedCardsRequest,
    QueuedCards,
} from "../../out/ts/lib/generated/anki/scheduler_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
const base = `http://127.0.0.1:${port}`;
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function readLog() {
    try {
        return fs.readFileSync(logPath, "utf8");
    } catch {
        return "";
    }
}

function countLog(pattern) {
    return readLog().split(pattern).length - 1;
}

/** 等日志里某个片段的出现次数超过 `minimum`（先取 baseline 再发命令，避免竞态）。 */
async function waitLogAbove(pattern, minimum, timeoutMs = 20000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        if (countLog(pattern) > minimum) {
            return true;
        }
        await sleep(250);
    }
    return false;
}

async function rpc(method, body) {
    const response = await fetch(`${base}/_anki/${method}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/binary",
            Authorization: `Bearer ${apiKey}`,
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

async function bridge(command, window = "main") {
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

const getCurrentDeck = () =>
    rpc("getCurrentDeck", new Empty({}).toBinary()).then((bytes) => Deck.fromBinary(bytes).id);

const getQueue = () =>
    rpc("getQueuedCards", new GetQueuedCardsRequest({ fetchLimit: 1 }).toBinary()).then(
        (bytes) => QueuedCards.fromBinary(bytes),
    );

const getCongrats = () =>
    rpc("congratsInfo", new Empty({}).toBinary()).then((bytes) =>
        CongratsInfoResponse.fromBinary(bytes),
    );

/** 与 `tauri/src/bridge.rs` 的注入脚本一致：补鉴权头 + 窗口身份 + `bridgeCommand`。 */
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

// ---------------------------------------------------------------------------
// 准备：临时牌组 + 一张被埋藏的卡片（队列一定是空的，但牌组不"空"）
// ---------------------------------------------------------------------------
const stamp = Date.now() % 100_000;
const deckName = `TauriCongrats${stamp}`;
const originalDeck = await getCurrentDeck();
let tempDeck = 0n;

const addDeck = async (name) =>
    BigInt(
        OpChangesWithId.fromBinary(
            await rpc(
                "addDeck",
                new Deck({ name, kind: { case: "normal", value: {} } }).toBinary(),
            ),
        ).id,
    );

try {
    tempDeck = await addDeck(deckName);
    const names = NotetypeNames.fromBinary(await rpc("getNotetypeNames", new Empty({}).toBinary()));
    // 与 `seed-card.mjs` 一样优先挑普通问答类型（新建集合里第一个可能是遮盖类型）
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
                fields: notetype.fields.map((field, index) =>
                    index === 0 ? `TauriCongrats 卡片${stamp}` : field.name,
                ),
            }),
            deckId: tempDeck,
        }).toBinary(),
    );
    await rpc("setCurrentDeck", new DeckId({ did: tempDeck }).toBinary());
    // 埋藏卡片：队列立刻空了，但 `congratsInfo` 会报 `haveUserBuried`
    const cardIds = (await getQueue()).cards.map((card) => card.card.id);
    check("临时牌组里取到了卡片", cardIds.length === 1, `共 ${cardIds.length} 张`);
    await rpc(
        "buryOrSuspendCards",
        new BuryOrSuspendCardsRequest({
            cardIds,
            noteIds: [],
            mode: BuryOrSuspendCardsRequest_Mode.BURY_USER,
        }).toBinary(),
    );
    const queue = await getQueue();
    const info = await getCongrats();
    check("埋藏后队列为空（= 今天学完了）", queue.cards.length === 0, `${queue.cards.length} 张`);
    check(
        "congratsInfo 报告了用户埋藏",
        info.haveUserBuried && !info.haveSchedBuried,
        `user=${info.haveUserBuried} sched=${info.haveSchedBuried}`,
    );
    check(
        "congratsInfo 说明没有可学的卡",
        info.learnRemaining === 0 && !info.reviewRemaining && !info.newRemaining,
        `learn=${info.learnRemaining} review=${info.reviewRemaining} new=${info.newRemaining}`,
    );

    // 真实入口：底部工具栏/概览的「学习」按钮发的是 `study`
    if (!logPath) {
        check("ANKI_LOG 已设置", false, "需要读外壳日志");
    } else {
        const emptyLogs = countLog("队列已空：清理复习会话并回到概览");
        const navLogs = countLog("主窗口导航到:");
        await bridge("study");
        check(
            "发 study 后宿主发现队列为空（清理会话 + 回概览）",
            await waitLogAbove("队列已空：清理复习会话并回到概览", emptyLogs, 20000),
            logPath,
        );
        // 两次导航：→ 复习页（legacyPageData）→ 概览（decks/?deck=current）
        check(
            "宿主随后把主窗口导到概览",
            await waitLogAbove("主窗口导航到:", navLogs + 1, 20000),
            logPath,
        );
        const navigations = readLog().match(/主窗口导航到: (\S+)/g) ?? [];
        check(
            "最后一条导航指向概览（?deck=current）",
            (navigations.at(-1) ?? "").includes("/decks/?deck=current"),
            navigations.at(-1) ?? "（没有导航日志）",
        );
    }

    // -----------------------------------------------------------------------
    // 页面上真的画出了恭喜内容，并且「取消埋藏」能把它变回计数表
    // -----------------------------------------------------------------------
    // 先给真实窗口留一张截图：这时主窗口正停在「学完」的概览上（`SHOT_DIR` 由
    // `verify-reviewer.ps1` 传进来，与 `visual-check.mjs` 用的是同一个变量）。
    // 必须用 `-TitleLike`：到这一步进程里还开着统计/设置/牌组选项等窗口，
    // 不给标题会截到 z 序最上面的那个（实测截到了「设置」）。
    if (process.env.SHOT_DIR) {
        const shot = spawnSync(
            "pwsh",
            [
                "-File",
                "tauri/tools/capture-window.ps1",
                "-ProcessName",
                "anki-tauri",
                "-OutFile",
                path.join(process.env.SHOT_DIR, "congrats.png"),
                "-WaitSeconds",
                "20",
                // 主窗口标题固定在 "Anki"（`main.rs:166`；只有浏览器窗口会被改标题）
                "-TitleLike",
                "Anki",
            ],
            { stdio: "inherit" },
        );
        check("截到了概览窗口的真实渲染", shot.status === 0, `capture-window exit=${shot.status}`);
    }

    const browser = await chromium.launch();
    try {
        const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
        await page.addInitScript(bridgeShim, { key: apiKey, window: "main" });
        await page.goto(`${base}/decks/?deck=current`, { waitUntil: "load" });
        await page.waitForSelector(".overview", { timeout: 20_000 });
        await page.waitForSelector("#congrats", { timeout: 20_000 });

        const title = (await page.locator("#congrats h1").innerText()).trim();
        check("概览里画出了 Congratulations 标题", title.length > 0, title);
        check(
            "学完时不再有「学习」按钮",
            (await page.locator("#deck-study").count()) === 0,
        );
        check(
            "学完时保留概览底部的选项按钮（Qt 的底栏学完时也还在）",
            (await page.locator("#deck-options").count()) === 1,
        );
        check(
            "没有学习卡片时不显示「下次学习时间」",
            (await page.locator("#congrats-next-learn").count()) === 0,
        );
        check(
            "有埋藏卡片时给出「取消埋藏」链接",
            (await page.locator('#congrats-buried a[data-overview-action="unbury"]').count()) === 1,
            await page.locator("#congrats-buried").innerText().catch(() => "（没有这一句）"),
        );
        check(
            "普通牌组给出「自定义学习」提示",
            (await page.locator('#congrats-custom-study a[data-overview-action="customStudy"]')
                .count()) === 1,
        );

        // 真的点「取消埋藏」：页面 → `unburyDeck` RPC → 重新取概览
        await page.locator('#congrats-buried a[data-overview-action="unbury"]').click();
        await page.waitForSelector("#deck-study", { timeout: 20_000 });
        check("点「取消埋藏」后回到普通概览（出现「学习」按钮）", true);
        check("取消埋藏后 Congratulations 消失", (await page.locator("#congrats").count()) === 0);
        const after = await getQueue();
        check(
            "取消埋藏后卡片回到队列",
            after.cards.length === 1,
            `队列 ${after.cards.length} 张`,
        );
        await page.close();
    } finally {
        await browser.close();
    }
} catch (error) {
    check("Congratulations 检查", false, String(error));
} finally {
    // 收拾：切回原来的牌组、删掉临时牌组（含里面的卡片）。
    // 字段名是 `dids` —— 写成 `ids` 会被 protobuf 静默忽略，请求变成空的，
    // 于是"删掉了"但集合里还留着（这个检查最初就踩了这一下）
    try {
        if (tempDeck) {
            await rpc("setCurrentDeck", new DeckId({ did: originalDeck }).toBinary());
            await rpc("removeDecks", new DeckIds({ dids: [tempDeck] }).toBinary());
        }
    } catch (error) {
        console.log(`清理临时牌组失败：${String(error)}`);
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
