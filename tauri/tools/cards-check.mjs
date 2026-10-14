// 「卡片…」对话框（`ts/routes/cards/`，对照 `qt/aqt/clayout.py` 的 `CardLayout`）的检查。
//
//   1. 页面按 URL 上的 `notetype` / `ord` 载入卡片类型与三个编辑区（正面 / 背面 / 样式）；
//   2. 改正面模板 → **未保存**的改动立刻反映到预览里（走 `RenderUncommittedCard`）；
//   3. 「选项 → 添加卡片类型」能加一个（复制当前模板），下拉里立刻多一项；
//   4. 点「保存」→ `updateNotetype` 整份落库 → 用 RPC 复查模板真的改了；
//   5. 还原：检查开始时留了一份 `getNotetype` 的**原始字节**，最后原样写回去。
//
// 为什么用 Playwright：要真的在下拉里选、在编辑区打字、等预览重画、再点保存。
// Chromium 与 WebView2 同源，加载同一个 URL，按 `tauri/src/bridge.rs` 补垫片。
//
// 前置：外壳已启动（固定 ANKI_API_PORT），集合里有一个可编辑的笔记类型。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/cards-check.mjs

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

/** 轮询等一个条件（预览是去抖 + 异步渲染的，不能只等一下）。 */
async function waitFor(probe, timeoutMs = 8000) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        const value = await probe();
        if (value) {
            return value;
        }
        if (Date.now() > deadline) {
            return value;
        }
        await new Promise((resolve) => setTimeout(resolve, 200));
    }
}

// ##########################################################################
// 准备：挑一个笔记类型，并留一份原始字节用于还原
// ##########################################################################

const names = NotetypeNames.fromBinary(
    await rpc("getNotetypeNames", new Empty({}).toBinary()),
);
const target = names.entries[0];
if (!target) {
    console.error("集合里没有笔记类型");
    process.exit(2);
}
const request = new NotetypeId({ ntid: target.id }).toBinary();
const originalBytes = await rpc("getNotetype", request);
const original = Notetype.fromBinary(originalBytes);
console.log(
    `笔记类型: ${original.name}（${original.id}），${original.fields.length} 个字段 / ` +
        `${original.templates.length} 个卡片类型`,
);

const marker = `TauriCards${Date.now() % 100_000}`;
const newCardName = `TauriCardType${Date.now() % 100_000}`;
const browser = await chromium.launch();
let restored = false;
try {
    const page = await browser.newPage({ viewport: { width: 1100, height: 800 } });
    // 添加卡片类型会弹确认框（集合里有用量）
    page.on("dialog", (dialog) => void dialog.accept());
    await page.addInitScript(bridgeShim, { key: apiKey, window: "cards" });
    await page.goto(`${base}/cards/?notetype=${original.id}&ord=0`, { waitUntil: "load" });
    await page.waitForSelector("#card-type", { timeout: 20_000 });

    // 1. 载入与三个编辑区 ###############################################
    const options = await page.locator("#card-type option").allInnerTexts();
    check(
        "卡片类型下拉载入了全部模板",
        options.length === original.templates.length,
        `${options.join(" / ")}`,
    );

    const frontEditor = page.locator("#editor-front");
    check(
        "正面模板编辑区显示的是当前模板",
        (await frontEditor.inputValue()) === original.templates[0].config?.qFormat,
        await frontEditor.inputValue(),
    );

    await page.locator("#tab-styling").click();
    const styling = page.locator("#editor-styling");
    check(
        "「样式」切换到了笔记类型的 css",
        (await styling.inputValue()) === (original.config?.css ?? ""),
        (await styling.inputValue()).slice(0, 40),
    );
    await page.locator("#tab-front").click();

    // 2. 未保存的改动要立刻出现在预览里（RenderUncommittedCard） ##########
    await page.locator("#preview-fill-empty").click();
    await frontEditor.fill(`${marker}\n{{${original.fields[0].name}}}`);
    const previewText = await waitFor(async () => {
        const text = await page.frameLocator("#preview").locator("body").innerText();
        return text.includes(marker) ? text : "";
    });
    check("预览用的是**还没保存**的模板", previewText.includes(marker), previewText.trim().slice(0, 60));
    const backendTemplate = Notetype.fromBinary(await rpc("getNotetype", request));
    check(
        "此时集合里的模板还没变",
        backendTemplate.templates[0].config?.qFormat !== `${marker}\n{{${original.fields[0].name}}}`,
        "只在内存里改了",
    );

    // 背面预览也要能切
    await page.locator("#preview-back").click();
    await waitFor(async () => !(await page.frameLocator("#preview").locator("body").innerText()).includes(marker));
    check("切到背面预览后不再是正面内容", true);
    await page.locator("#preview-front").click();

    // 3. 添加卡片类型（选项菜单） ########################################
    await page.locator("#cards-options").click();
    await page.waitForSelector("#cards-options-menu", { timeout: 5000 });
    await page.locator("#menu-add-type").click();
    await page.waitForSelector("#cards-text-input", { timeout: 5000 });
    await page.locator("#cards-text-input").fill(newCardName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(500);
    const afterAdd = await page.locator("#card-type option").allInnerTexts();
    check(
        "添加卡片类型后下拉里多了一项",
        afterAdd.length === original.templates.length + 1 && afterAdd.includes(newCardName),
        afterAdd.join(" / "),
    );
    check(
        "下拉切到了新卡片类型，编辑区是复制来的内容",
        (await page.locator("#card-type").inputValue()) === String(original.templates.length) &&
            (await frontEditor.inputValue()) === `${marker}\n{{${original.fields[0].name}}}`,
        await frontEditor.inputValue(),
    );

    // 新卡片类型复制了当前模板，所以正面必须改一下：后端会拒绝「正面完全相同」的
    // 模板（`xCard template N ... The front side is identical to card template M`），
    // Qt 那边同样保存不了，用户在 Qt 里也得先改
    await frontEditor.fill(`${marker}Back\n{{${original.fields[0].name}}}`);

    // 4. 保存 → 落库 ####################################################
    await page.locator("#cards-save").click();
    await page.waitForTimeout(1500);
    const saved = Notetype.fromBinary(await rpc("getNotetype", request));
    check(
        "保存后模板真的写进了集合",
        saved.templates[0].config?.qFormat === `${marker}\n{{${original.fields[0].name}}}`,
        saved.templates[0].config?.qFormat,
    );
    check(
        "保存后新的卡片类型也在",
        saved.templates.some((entry) => entry.name === newCardName),
        saved.templates.map((entry) => entry.name).join(" / "),
    );
    check("页面上没有报错", (await page.locator("#cards-error").count()) === 0);

    const shotDir = process.env.SHOT_DIR ?? "out/editor-check";
    await page.screenshot({ path: `${shotDir}/cards.png` });
    console.log(`已截图: ${shotDir}/cards.png`);
} catch (error) {
    check("卡片对话框流程", false, String(error));
} finally {
    try {
        // 原样写回（`updateNotetype` 收的就是刚才 `getNotetype` 的字节）
        await rpc("updateNotetype", originalBytes);
        const back = Notetype.fromBinary(await rpc("getNotetype", request));
        check(
            "已还原笔记类型",
            back.templates.length === original.templates.length &&
                back.templates[0].config?.qFormat === original.templates[0].config?.qFormat,
            back.templates.map((entry) => entry.name).join(" / "),
        );
        restored = true;
    } catch (problem) {
        console.error(`还原失败: ${problem}`);
    }
    await browser.close();
}

console.log(
    failures.length === 0
        ? `卡片对话框检查：全部通过（还原${restored ? "完成" : "失败"}）`
        : `卡片对话框检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
