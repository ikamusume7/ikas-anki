// 编辑器「拖入文件」（HTML5 drop，PARITY 4.4）的检查。
//
// 覆盖的是**页面这一侧**的链路：拖一张图片进字段 → `data-transfer.ts` 的
// `handleDrop`（`processImages`）→ `convertPastedImage` + `addMediaFile` 落进
// 媒体夹 → 字段里出现 `<img src="paste-….jpg">` → 失焦保存后集合里的 note 也有它。
//
// 不覆盖「操作系统把文件交给 WebView2」这一步 —— 那一半没法在自动化里可靠地造出来
// （要真的按住鼠标拖），只能靠宿主日志断言「已关闭 Tauri 的拖放拦截」
// （见 `verify-editor.ps1`）。Playwright 只能合成 `DataTransfer`。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer + 固定 ANKI_API_PORT + 设了
//       ANKI_COLLECTION），集合里有可复习的卡片。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_COLLECTION="<集合路径>"
//   node tauri/tools/drop-check.mjs

import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";

import { Note, NoteId, UpdateNotesRequest } from "../../out/ts/lib/generated/anki/notes_pb.js";
import { GetQueuedCardsRequest, QueuedCards } from "../../out/ts/lib/generated/anki/scheduler_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;
const failures = [];

// 1x1 的透明 PNG（70 字节，IHDR/IDAT/IEND 的 CRC 都验过）。
// 注意：后端会**真的解码**这张图（`convertPastedImage` → image crate），字节不对会
// 直接 500（`Format error decoding Png: CRC error…`）—— 随手抄的 base64 多半是坏的。
const PNG_1X1 =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

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

// 与 tauri/src/bridge.rs 等价：补鉴权头与窗口身份 + 提供 bridgeCommand
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
        return false;
    };
}

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
const noteId = new NoteId({ nid: card.noteId });
const note = Note.fromBinary(await rpc("getNote", noteId.toBinary()));
const original = note.fields[0];
const mediaDir = path.join(path.dirname(process.env.ANKI_COLLECTION ?? ""), "collection.media");
console.log(`卡片 ${card.id} / note ${note.id} / 媒体夹 ${mediaDir}`);

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(String(error)));
    await page.addInitScript(bridgeShim, { key: apiKey, window: "edit-current" });
    await page.goto(`${base}/editor/?mode=current`, { waitUntil: "load" });
    await page.waitForFunction(() => typeof window.loadNote === "function", null, {
        timeout: 20_000,
    });
    await page.evaluate(async (args) => window.loadNote(args), {
        nid: Number(note.id),
        notetypeId: Number(note.notetypeId),
        reviewerCardId: Number(card.id),
        focusTo: 0,
        initial: true,
    });

    const field = page.locator('[contenteditable="true"]').first();
    check("note 已渲染出字段", (await page.locator('[contenteditable="true"]').count()) > 0);
    // 插入发生在光标处，所以先把光标放进字段（与手动"点到字段里再拖"一致）
    await field.click();

    // 合成一次 drop：一个真 PNG 文件。
    //
    // 两个坑：
    // 1. 事件要在**页面里**自己 `new DragEvent`（元素句柄由 Playwright 给，能穿透
    //    shadow DOM）—— `locator.dispatchEvent("drop", { dataTransfer })` 造出来的事件
    //    读不到 `dataTransfer`，编辑器拿到的就是 `undefined`，什么都不会发生；
    // 2. 刻意**不放** text/html —— 页面见到 html 会当成「内部粘贴」原样插入，就不走
    //    媒体链路了（`processDataTransferEvent:346-349`）。
    const dispatched = await (
        await field.elementHandle()
    ).evaluate(async (element, base64) => {
        element.focus();
        const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
        const transfer = new DataTransfer();
        transfer.items.add(new File([bytes], "tauri-drop.png", { type: "image/png" }));
        for (const type of ["dragover", "drop"]) {
            element.dispatchEvent(
                new DragEvent(type, { dataTransfer: transfer, bubbles: true, cancelable: true }),
            );
        }
        return transfer.files.length;
    }, PNG_1X1);
    check("合成了一次带文件的 drop", dispatched === 1, `files=${dispatched}`);

    const inserted = await (async () => {
        const deadline = Date.now() + 15_000;
        for (;;) {
            const html = await field.innerHTML();
            if (html.includes("<img")) {
                return html;
            }
            if (Date.now() > deadline) {
                return html;
            }
            await page.waitForTimeout(200);
        }
    })();
    check("拖入的图片插进了字段", inserted.includes("<img"), inserted.slice(0, 80));
    const filename = /<img src="([^"]+)"/.exec(inserted)?.[1] ?? "";
    check(
        "字段里是媒体文件名（paste-<哈希>.<扩展名>）",
        /^paste-[0-9a-f]+\.(jpg|jpeg|png|gif)$/.test(filename),
        filename,
    );

    // 媒体真的落在媒体夹里了
    check(
        "图片已拷进媒体夹",
        Boolean(filename) && fs.existsSync(path.join(mediaDir, decodeURI(filename))),
        filename,
    );

    // 失焦触发保存（与 editor-check.mjs 一样，保存由 focusout 触发）
    await page.evaluate(() => document.activeElement?.blur());
    await page.waitForTimeout(1500);
    const saved = Note.fromBinary(await rpc("getNote", noteId.toBinary()));
    check(
        "拖入的结果已保存到集合",
        saved.fields[0].includes(`<img src="${filename}"`),
        saved.fields[0].slice(0, 80),
    );
    check("页面没有报错", pageErrors.length === 0, pageErrors.join(" / "));

    // 还原：字段恢复原值 + 删掉刚生成的媒体文件（后端没有暴露删除媒体的接口）
    saved.fields[0] = original;
    await rpc("updateNotes", new UpdateNotesRequest({ notes: [saved] }).toBinary());
    const restored = Note.fromBinary(await rpc("getNote", noteId.toBinary()));
    check("已还原测试笔记", restored.fields[0] === original, restored.fields[0]);
    if (filename) {
        const file = path.join(mediaDir, decodeURI(filename));
        if (fs.existsSync(file)) {
            fs.unlinkSync(file);
        }
    }
} catch (error) {
    check("拖入文件流程", false, String(error));
} finally {
    await browser.close();
}

console.log(
    failures.length === 0
        ? `拖入文件检查：全部通过`
        : `拖入文件检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
