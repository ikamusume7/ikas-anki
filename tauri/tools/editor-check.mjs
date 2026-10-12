// 编辑器「载入 note → 改字段 → 保存」的端到端检查。
//
// 为什么用 Playwright 而不是截 Tauri 窗口：这一步要**真的敲键盘**。编辑器的保存靠
// 字段失焦事件把内容写回 note（`NoteEditor.svelte:1508-1524` 的 `focusout` → 立刻
// `updateCurrentNote()`），直接改 DOM 不会触发它。Chromium 与 WebView2 同源，
// 加载同一个 URL，并按 `tauri/src/bridge.rs` 补上垫片（鉴权头 + bridgeCommand）。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer + 固定 ANKI_API_PORT），集合里有可复习的卡片。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/editor-check.mjs

import { chromium } from "playwright";

import { Note, NoteId, UpdateNotesRequest } from "../../out/ts/lib/generated/anki/notes_pb.js";
import { GetQueuedCardsRequest, QueuedCards } from "../../out/ts/lib/generated/anki/scheduler_pb.js";

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
/// 窗口身份填 `edit-current`：这就是编辑器窗口在宿主侧的标签，
/// 也让页面发出的 `focus:` / `blur:` 命令归属正确。
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
            .then((text) => text && callback?.(text))
            .catch(() => {});
        return false;
    };
}

// 1. 找出复习界面当前这张卡（与 reviewer.rs 一样调 getQueuedCards）
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
const note = Note.fromBinary(await rpc("getNote", new NoteId({ nid: card.noteId }).toBinary()));
const original = note.fields[0];
const marker = `（已编辑${Date.now() % 100_000}）`;
console.log(`卡片 ${card.id} / note ${note.id} / 正面原值「${original}」`);

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "edit-current" });
    await page.goto(`${base}/editor/?mode=current`, { waitUntil: "load" });

    // 页面就绪后会把 `loadNote` 挂到 globalThis 上（NoteEditor.svelte:1322）
    await page.waitForFunction(() => typeof window.loadNote === "function", null, {
        timeout: 20_000,
    });

    // 复现宿主的注入。`await` 它 —— 万一有 RPC 报错（页面会弹 alert 挡住 JS 事件
    // 循环），这里会超时失败，而不是静默地"看起来还行"。
    await page.evaluate(
        async (args) => window.loadNote(args),
        {
            nid: Number(note.id),
            notetypeId: Number(note.notetypeId),
            reviewerCardId: Number(card.id),
            focusTo: 0,
            initial: true,
        },
    );

    const fieldCount = await page.locator('[contenteditable="true"]').count();
    check("note 已渲染出字段", fieldCount > 0, `${fieldCount} 个可编辑字段`);
    if (fieldCount === 0) {
        throw new Error("没有找到可编辑字段，无法继续");
    }

    // 2. 真的敲键盘（保存由失焦触发）
    const editor = page.locator('[contenteditable="true"]').first();
    await editor.click();
    await page.keyboard.press("End");
    await page.keyboard.type(marker);
    await page.evaluate(() => document.activeElement?.blur());
    await page.waitForTimeout(1500);

    const calls = await page.evaluate(() => window.__bridgeCalls ?? []);
    console.log(`页面发来的命令: ${calls.join("、")}`);

    // 3. 用 RPC 复查集合里的内容 —— 这是"保存真的落库了"的唯一证据
    const saved = Note.fromBinary(await rpc("getNote", new NoteId({ nid: card.noteId }).toBinary()));
    check("字段修改已保存到集合", saved.fields[0].includes(marker), `正面现在是「${saved.fields[0]}」`);

    // 4. 还原，避免反复运行把测试笔记改花。
    // 注意方法名是 `updateNotes`（复数，`notes.proto:21` 的 `rpc UpdateNotes`）
    saved.fields[0] = original;
    await rpc("updateNotes", new UpdateNotesRequest({ notes: [saved] }).toBinary());
    const restored = Note.fromBinary(
        await rpc("getNote", new NoteId({ nid: card.noteId }).toBinary()),
    );
    check("已还原测试笔记", restored.fields[0] === original, `正面回到「${restored.fields[0]}」`);
} catch (error) {
    check("编辑器交互流程", false, String(error));
} finally {
    await browser.close();
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
    process.exitCode = 1;
}
