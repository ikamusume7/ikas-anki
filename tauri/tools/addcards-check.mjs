// 添加卡片窗口的端到端检查：
//   开窗 → 页面自算默认牌组/笔记类型 → 填字段 → 添加 → 落库 → 清理
//   以及关闭契约：页面把「有没有未保存输入」的判定交给宿主（`closeAddCards` RPC）。
//
// 为什么用 Playwright（而不是截 Tauri 窗口）：这里要**真的敲键盘、真的点按钮**。
// 「添加」按钮走 RPC `addNote`（`NoteEditor.svelte:591-610`），关闭走 RPC
// `closeAddCards`，直接改 DOM 都不会触发它们。Chromium 与 WebView2 同源，加载同一个
// URL，并按 `tauri/src/bridge.rs` 补上垫片（鉴权头 + 窗口身份 + bridgeCommand）。
//
// 注意 `closeAddCards` 被**拦住不放行**：真的发到宿主就会弹出「放弃输入？」确认框，
// 把自动化卡住。这里只验「页面交上来的布尔值对不对」，宿主侧的弹框与关窗由
// `verify-addcards.ps1` 直接用 RPC 验证。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer + 固定 ANKI_API_PORT），集合里至少有一张
// 可复习的卡片（add 模式要用它算 home deck）。
//
// 用法：
//   $env:ANKI_API_PORT="40026"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/addcards-check.mjs

import { chromium } from "playwright";

import { Bool } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    AddNoteRequest,
    Note,
    NoteId,
    RemoveNotesRequest,
} from "../../out/ts/lib/generated/anki/notes_pb.js";
import { GetQueuedCardsRequest, QueuedCards } from "../../out/ts/lib/generated/anki/scheduler_pb.js";
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
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/// 与 tauri/src/bridge.rs 等价：补鉴权头与窗口身份 + 提供 bridgeCommand。
///
/// 窗口身份必须填 `add-cards`：`pycmd.rs` 靠它把 `editorReady` 分流到「新建 note」
/// 的注入分支（`editor.rs::on_ready`）。
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

/// 点击底部动作栏里以 `text` 开头的按钮（区分「添加」与「最近添加」）。
/// 返回按钮序号；找不到返回 -1。
async function actionButtonIndex(page, text) {
    return page.evaluate((prefix) => {
        const buttons = [...document.querySelectorAll(".action-buttons button")];
        return buttons.findIndex((button) =>
            new RegExp(`^(${prefix})`).test(button.textContent.trim()),
        );
    }, text);
}

// 1. 复习中的这张卡就是 add 模式算「home deck」的输入（`reviewer.rs::current_card`）
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
console.log(`复习中的卡片 ${card.id} / note ${card.noteId}`);

// 纯 ASCII、无空格：既要能敲进字段，又要能直接当浏览器搜索词用（`searchNotes`）
const marker = `TauriProbe${Date.now() % 100_000}`;
let createdNoteId = null;

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "add-cards" });
    await page.goto(`${base}/editor/?mode=add`, { waitUntil: "load" });

    // 页面就绪后会把 `loadNote` 挂到 globalThis 上（NoteEditor.svelte:1322）
    await page.waitForFunction(() => typeof window.loadNote === "function", null, {
        timeout: 20_000,
    });

    // 2. 复现宿主的注入：add 模式**不带 nid/deckId/notetypeId**，只给
    //    reviewerCardId（算 home deck）+ `initial: true`（`editor.rs::inject_new_note`）。
    //    `await` 它 —— 万一有 RPC 报错（页面会弹 alert 挡住 JS 事件循环），这里会超时
    //    失败，而不是静默地"看起来还行"。
    await page.evaluate(async (args) => window.loadNote(args), {
        nid: null,
        notetypeId: null,
        deckId: null,
        originalNoteId: null,
        reviewerCardId: Number(card.id),
        focusTo: 0,
        initial: true,
    });

    // 3. 选择器只在 add 模式渲染（`NoteEditor.svelte:1441-1447`）。它有内容就说明
    //    `initial: true` 那条分支真的跑到了 `defaultsForAdding()`（否则页面会退到
    //    `notetypeChooser.getSelected()`，牌组/笔记类型是空的）。
    const choosers = await page.evaluate(() => {
        const read = (selector) => document.querySelector(selector)?.textContent?.trim() ?? "";
        return { deck: read(".deck-chooser"), notetype: read(".notetype-chooser") };
    });
    check("牌组选择器已渲染", choosers.deck.length > 0, `「${choosers.deck}」`);
    check("笔记类型选择器已渲染", choosers.notetype.length > 0, `「${choosers.notetype}」`);

    const fieldCount = await page.locator('[contenteditable="true"]').count();
    check("新建 note 已渲染出字段", fieldCount > 0, `${fieldCount} 个可编辑字段`);
    if (fieldCount === 0) {
        throw new Error("没有找到可编辑字段，无法继续");
    }

    // 4. 关闭契约：拦住 `closeAddCards`，验页面交上来的布尔值。
    //    `Bool.val == true` ⇒ 页面认为「有未保存输入」，宿主该弹确认框
    //    （对照 `qt/aqt/addcards.py:107-127`）。
    const closeRequests = [];
    await page.route("**/_anki/closeAddCards", async (route) => {
        // `Bool{val:false}` 的 protobuf 是**零字节**，此时 Playwright 给的是 null
        const body = route.request().postDataBuffer() ?? new Uint8Array();
        closeRequests.push(Bool.fromBinary(body).val);
        await route.fulfill({ status: 200, contentType: "application/binary", body: "" });
    });

    // 4a. 填第一个字段（「添加」按钮会先 saveNow()，所以内容要真的进 DOM）
    const field = page.locator('[contenteditable="true"]').first();
    await field.click();
    await page.keyboard.press("End");
    await page.keyboard.type(marker);
    await page.evaluate(() => document.activeElement?.blur());
    await page.waitForTimeout(1200);

    // 4b. 点「关闭」→ 有输入，页面应报 true。
    //     按钮文案是 `tr.actions_close()`；锚定开头以免撞上别的按钮。
    const closeIndex = await actionButtonIndex(page, "关闭|Close");
    check("「关闭」按钮存在", closeIndex >= 0, `按钮序号 ${closeIndex}`);
    if (closeIndex >= 0) {
        await page.locator(".action-buttons button").nth(closeIndex).click();
        await page.waitForTimeout(500);
        check(
            "有输入时页面报告需要确认",
            closeRequests.at(-1) === true,
            `closeAddCards(val=${closeRequests.at(-1)})`,
        );
    }

    // 5. 点「添加」按钮（文案是 `tr.actions_add()`）。ActionButtons 里还有
    //    「最近添加」按钮，所以必须锚定在开头匹配。
    //    请求**放行**（`route.continue()`），只在旁边读一份请求体：读了它才知道页面
    //    是不是真的把字段交上去了。
    const addRequests = [];
    await page.route("**/_anki/addNote", async (route) => {
        const body = route.request().postDataBuffer();
        addRequests.push(body ? AddNoteRequest.fromBinary(body) : null);
        await route.continue();
    });

    const addButtonIndex = await actionButtonIndex(page, "添加|Add");
    check("「添加」按钮存在", addButtonIndex >= 0, `按钮序号 ${addButtonIndex}`);
    if (addButtonIndex < 0) {
        throw new Error("找不到「添加」按钮，无法继续");
    }

    await page.locator(".action-buttons button").nth(addButtonIndex).click();
    await page.waitForTimeout(2500);
    check(
        "点击「添加」发出了 addNote",
        addRequests.length === 1,
        `${addRequests.length} 次请求`,
    );
    check(
        "addNote 带着字段内容",
        addRequests[0]?.note?.fields?.[0]?.includes(marker) === true,
        `「${addRequests[0]?.note?.fields?.[0] ?? ""}」`,
    );

    const calls = await page.evaluate(() => window.__bridgeCalls ?? []);
    console.log(`页面发来的命令: ${calls.join("、")}`);

    // 6. 回到集合里查这张 note —— 这才是"添加真的落库了"的证据
    //    （不读 Playwright 抓到的响应体：那条路径实测拿不到 body，见 README 的「添加卡片」）
    const searchResponse = SearchResponse.fromBinary(
        await rpc("searchNotes", new SearchRequest({ search: marker }).toBinary()),
    );
    const found = searchResponse.ids.map(Number);
    check("新增的 note 已落库", found.length === 1, `搜到 ${found.length} 条`);
    if (found.length === 1) {
        createdNoteId = found[0];
        const added = Note.fromBinary(
            await rpc("getNote", new NoteId({ nid: createdNoteId }).toBinary()),
        );
        check(
            "新增的 note 内容正确",
            added.fields[0].includes(marker),
            `正面「${added.fields[0]}」`,
        );
    }

    // 7. 添加后表单被重置（`addCurrentNoteInner` 末尾的 `loadNote`），此时再关一次，
    //    页面应报「没有输入」→ false。
    await page.waitForTimeout(1000);
    if (closeIndex >= 0) {
        await page.locator(".action-buttons button").nth(closeIndex).click();
        await page.waitForTimeout(500);
        check(
            "无输入时页面报告可直接关闭",
            closeRequests.at(-1) === false,
            `closeAddCards(val=${closeRequests.at(-1)})`,
        );
    }
} catch (error) {
    check("添加卡片交互流程", false, String(error));
} finally {
    await browser.close();
    // 8. 清掉测试产生的 note，避免反复运行把隔离集合堆满
    if (createdNoteId && createdNoteId > 0) {
        await rpc(
            "removeNotes",
            new RemoveNotesRequest({ noteIds: [createdNoteId] }).toBinary(),
        );
        console.log(`已清理测试 note ${createdNoteId}`);
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
    process.exitCode = 1;
}
