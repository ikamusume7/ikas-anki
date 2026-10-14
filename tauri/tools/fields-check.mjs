// 「字段…」对话框（`ts/routes/fields/`，对照 `qt/aqt/fields.py`）的检查。
//
//   1. 页面按 URL 里的 `notetype` 载入字段列表（`getNotetype`）；
//   2. 点「Add」→ 弹文本框 → 填名字 → 列表里立刻多一项（改动只在内存里）；
//   3. 点「Save」→ `updateNotetype` 整份落库 → 用 RPC 复查字段真的多了一个；
//   4. 还原：检查开始时留了一份 `getNotetype` 的**原始字节**，最后原样写回去。
//
// 为什么用 Playwright：要真的点按钮、在弹框里输入、再点保存。
// Chromium 与 WebView2 同源，加载同一个 URL，按 `tauri/src/bridge.rs` 补垫片。
//
// 前置：外壳已启动（固定 ANKI_API_PORT），集合里有一个可编辑的笔记类型。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/fields-check.mjs

import { chromium } from "playwright";

import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
} from "../../out/ts/lib/generated/anki/notetypes_pb.js";

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
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
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

// ##########################################################################
// 准备：挑一个笔记类型，并留一份原始字节用于还原
// ##########################################################################

const names = NotetypeNames.fromBinary(
    await rpc("getNotetypeNames", new Empty({}).toBinary()),
);
// 挑普通问答类型：新建集合里 `entries[0]` 可能是「遮盖」/Image Occlusion
const target =
    names.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ?? names.entries[0];
if (!target) {
    console.error("集合里没有笔记类型");
    process.exit(2);
}
const request = new NotetypeId({ ntid: target.id }).toBinary();
const originalBytes = await rpc("getNotetype", request);
const original = Notetype.fromBinary(originalBytes);
console.log(`笔记类型: ${original.name}（${original.id}），现有 ${original.fields.length} 个字段`);

const newFieldName = `TauriField${Date.now() % 100_000}`;
const browser = await chromium.launch();
let restored = false;
try {
    const page = await browser.newPage({ viewport: { width: 700, height: 800 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "fields" });
    await page.goto(`${base}/fields/?notetype=${original.id}`, { waitUntil: "load" });
    await page.waitForSelector("#field-list li", { timeout: 20_000 });

    const rows = () => page.locator("#field-list li").count();
    check(
        "载入了全部字段",
        (await rows()) === original.fields.length,
        `${await rows()} / ${original.fields.length}`,
    );
    check(
        "字段名显示正确",
        (await page.locator("#field-list li").first().innerText()).includes(
            original.fields[0].name,
        ),
        await page.locator("#field-list li").first().innerText(),
    );

    // 添加字段（弹框里输入名字）
    await page.locator("#field-add").click();
    await page.waitForSelector("#fields-text-input", { timeout: 5000 });
    await page.locator("#fields-text-input").fill(newFieldName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(500);
    check(
        "添加后列表里立刻多了一项",
        (await rows()) === original.fields.length + 1,
        `${await rows()} 项`,
    );

    // 保存（整份 notetype 落库）
    await page.locator("#fields-save").click();
    await page.waitForTimeout(1500);
    const saved = Notetype.fromBinary(await rpc("getNotetype", request));
    check(
        "保存后字段真的写进了集合",
        saved.fields.some((field) => field.name === newFieldName),
        saved.fields.map((field) => field.name).join(" / "),
    );

    const shotDir = process.env.SHOT_DIR ?? "out/editor-check";
    await page.screenshot({ path: `${shotDir}/fields.png` });
    console.log(`已截图: ${shotDir}/fields.png`);
} catch (error) {
    check("字段对话框流程", false, String(error));
} finally {
    try {
        // 原样写回（`updateNotetype` 收的就是刚才 `getNotetype` 的字节）
        await rpc("updateNotetype", originalBytes);
        const back = Notetype.fromBinary(await rpc("getNotetype", request));
        check(
            "已还原笔记类型",
            back.fields.length === original.fields.length &&
                !back.fields.some((field) => field.name === newFieldName),
            back.fields.map((field) => field.name).join(" / "),
        );
        restored = true;
    } catch (problem) {
        console.error(`还原失败: ${problem}`);
    }
    await browser.close();
}

console.log(
    failures.length === 0
        ? `字段对话框检查：全部通过（还原${restored ? "完成" : "失败"}）`
        : `字段对话框检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
