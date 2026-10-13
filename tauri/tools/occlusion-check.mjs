// 图像遮挡的端到端检查。
//
// **这个 Anki 版本没有独立的遮挡窗口**：遮挡编辑器就是编辑器页面里的
// `ImageOcclusionPage`，由工具栏的 `image-occlusion-button` 切换（对照
// `qt/aqt/editor.py:500-535` 的 `setup_mask_editor` 与
// `ts/routes/editor/NoteEditor.svelte:795-815` 的 `pickIOImage`）。所以这里查的不是
// "窗口"，而是「添加遮挡卡片」那条链路：
//
//   1. 图像遮挡笔记类型下，工具栏的遮挡按钮**出现**（`NoteEditor.svelte:1191-1195`
//      按 `originalStockKind == IMAGE_OCCLUSION` 调 `setShown`）；
//   2. 点它 → 进图片选择器（`ioMaskEditorVisible` 且还没载入图片）；
//   3. 点「选图片」→ 页面向宿主要路径（`/openFilePicker`，测试用 `ANKI_OPEN_PATH`
//      跳过原生对话框）→ `addMediaFromPath` 把图片拷进媒体夹（在磁盘上复查）；
//   4. 掩码编辑器把图片渲染出来：`#image` 的 src 是后端返回的 data URL
//      （`mask-editor.ts:41` 的 `getImageForOcclusion`，add 模式按**路径**取图）；
//   5. 收尾：删掉拷进去的媒体文件（后端没暴露 `deleteMediaFiles`）。
//
// 为什么用 add 模式：编辑已有笔记时页面走 `getImageOcclusionNote(noteId)`
// （`mask-editor.ts:58-76`），它读的是**已保存**的笔记 —— 页面刚把新图片写进字段
// 还没落库，就会拿到空图（上游的时序问题，与外壳无关）。add 模式按文件路径取图，
// 不受这个时序影响。
//
// 前置：外壳已启动（`ANKI_START_PAGE=reviewer` + 固定 `ANKI_API_PORT`），集合里有
// `Image Occlusion` 笔记类型（新建集合自带），`ANKI_OPEN_PATH` 指向一张真图片。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_OPEN_PATH="D:\...\out\editor-check\occlusion-fixture.png"
//   node tauri/tools/occlusion-check.mjs

import fs from "node:fs";
import path from "node:path";

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
const fixture = process.env.ANKI_OPEN_PATH ?? "";
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

/** 与 `tauri/src/bridge.rs` 等价：补鉴权头 + 窗口身份 + `bridgeCommand`。 */
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

if (!fixture) {
    console.error("没有设 ANKI_OPEN_PATH（指向一张真实图片），无法测试选图链路");
    process.exit(2);
}
const fixtureName = path.basename(fixture);
// 媒体夹：`<集合目录>/collection.media/`
const mediaDir = path.join(path.dirname(process.env.ANKI_COLLECTION ?? ""), "collection.media");
const mediaPath = path.join(mediaDir, fixtureName);

// ##########################################################################
// 准备：找到「Image Occlusion」笔记类型
// ##########################################################################

const names = NotetypeNames.fromBinary(
    await rpc("getNotetypeNames", new Empty({}).toBinary()),
);
let ioNotetype;
for (const entry of names.entries) {
    const notetype = Notetype.fromBinary(
        await rpc("getNotetype", new NotetypeId({ ntid: entry.id }).toBinary()),
    );
    // StockNotetype.OriginalStockKind.IMAGE_OCCLUSION == 6
    if (notetype.config?.originalStockKind === 6) {
        ioNotetype = notetype;
        break;
    }
}
if (!ioNotetype) {
    console.error("集合里没有图像遮挡笔记类型（新建集合应该自带）");
    process.exit(2);
}
const fieldNames = ioNotetype.fields.map((field) => field.name);
console.log(`图像遮挡笔记类型: ${ioNotetype.name}（${ioNotetype.id}）字段: ${fieldNames.join(" / ")}`);
check(
    "图像遮挡笔记类型有 Occlusion 与 Image 字段",
    fieldNames.includes("Occlusion") && fieldNames.includes("Image"),
    fieldNames.join(" / "),
);
if (fs.existsSync(mediaPath)) {
    fs.unlinkSync(mediaPath);
}

const browser = await chromium.launch();
let cleaned = false;
try {
    // 供"选图片"用的真图片：Chromium 截一张 100x80 的纯色图（verify 脚本先放了个
    // 1x1 的占位 PNG 让文件存在，这里覆盖成能解码出真实像素的图）
    const canvasPage = await browser.newPage({ viewport: { width: 100, height: 80 } });
    await canvasPage.setContent(
        '<html><body style="margin:0"><div style="width:100px;height:80px;background:#4a90d9"></div></body></html>',
    );
    await canvasPage.screenshot({ path: fixture });

    const page = await browser.newPage({ viewport: { width: 1000, height: 800 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "add-cards" });

    // 记下选图片那次请求：页面 → 宿主的 `/openFilePicker`
    let pickerCalls = 0;
    await page.route("**/_anki/openFilePicker", async (route) => {
        pickerCalls += 1;
        await route.continue();
    });

    await page.goto(`${base}/editor/?mode=add`, { waitUntil: "load" });
    await page.waitForFunction(() => typeof window.loadNote === "function", null, {
        timeout: 20_000,
    });
    // 复现宿主的注入（`editor.rs::inject_new_note`：add 模式不带 nid/deckId）。
    // 这里额外指定遮挡笔记类型，省得页面按"上次用过"的类型猜。
    await page.evaluate(async (args) => window.loadNote(args), {
        nid: null,
        notetypeId: Number(ioNotetype.id),
        deckId: null,
        originalNoteId: null,
        reviewerCardId: null,
        focusTo: 0,
        initial: true,
    });

    // 1. 遮挡按钮出现（只有遮挡笔记类型会显示）
    await page.waitForSelector("#image-occlusion-button", { timeout: 20_000 });
    check(
        "图像遮挡笔记类型下工具栏有遮挡按钮",
        await page.locator("#io-mask-btn").isVisible(),
    );

    // 2. 进图片选择器。`ioMaskEditorVisible` 的初值不一定是 false（遮挡笔记类型下
    //    页面可能一进来就是掩码编辑器状态），所以先看状态，缺了才点——点两下会把它
    //    又切回去。
    if ((await page.locator(".image-occlusion-picker").count()) === 0) {
        await page.locator("#io-mask-btn").click();
    }
    await page.waitForSelector(".image-occlusion-picker", { timeout: 15_000 });
    check("已进入图片选择器", await page.locator("#io-pick-image-file").isVisible());

    // 3. 选图片：页面走宿主的 `/openFilePicker`（`ANKI_OPEN_PATH` 提供路径）
    await page.locator("#io-pick-image-file .io-image-picker-button").click();
    await page.waitForTimeout(2500);
    check("选图片向宿主发了 /openFilePicker", pickerCalls === 1, `${pickerCalls} 次`);
    check(
        "图片已拷进媒体夹",
        fs.existsSync(mediaPath),
        fs.existsSync(mediaPath) ? `${fixtureName}（${fs.statSync(mediaPath).size} 字节）` : mediaPath,
    );

    // 4. 掩码编辑器把图片渲染出来（`#image` 的 src 是后端返回的 data URL）。
    // 阈值放宽：100x80 的纯色 PNG 压完 base64 只有 ~360 字符，但"空图"只有
    // `data:image/png;base64,`（~22 字符）—— 两者的差别足够明显。
    const hasRealImage = (src) => src.startsWith("data:image/") && src.length > 150;
    await page.waitForFunction(
        (minLength) => {
            const src = document.querySelector("#image")?.getAttribute("src") ?? "";
            return src.startsWith("data:image/") && src.length > minLength;
        },
        150,
        { timeout: 20_000 },
    );
    const imageSrc = (await page.locator("#image").getAttribute("src")) ?? "";
    check("掩码编辑器把图片渲染出来了", hasRealImage(imageSrc), `${imageSrc.length} 字符的 data URL`);
    check(
        "遮罩编辑器与图片字段都挂在编辑器页面里（没有独立窗口）",
        (await page.locator(".image-occlusion").count()) > 0,
    );

    const shotDir = process.env.SHOT_DIR ?? "out/occlusion-check";
    fs.mkdirSync(shotDir, { recursive: true });
    await page.screenshot({ path: `${shotDir}/occlusion-editor.png` });
    console.log(`已截图: ${shotDir}/occlusion-editor.png`);
} catch (error) {
    check("图像遮挡链路", false, String(error));
} finally {
    try {
        if (fs.existsSync(mediaPath)) {
            fs.unlinkSync(mediaPath);
        }
        cleaned = true;
    } catch (problem) {
        console.error(`清理失败: ${problem}`);
    }
    await browser.close();
}

console.log(
    failures.length === 0
        ? `图像遮挡检查：全部通过（清理${cleaned ? "完成" : "失败"}）`
        : `图像遮挡检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
