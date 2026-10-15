// 「管理笔记类型」窗口（`ts/routes/notetypes/`，对照 `qt/aqt/models.py` 的 `Models`）的检查。
//
//   1. 列表按 `getNotetypeNamesAndCounts` 渲染成「名字 [笔记数]」（`models.py:157-158`）；
//   2. 「添加」两步走：选原厂类型 / 克隆现有类型 → 问名字 → `addNotetypeLegacy`，
//      之后用 RPC 复查真的建出来了（并抽查字段数 = 原厂 Basic 的 2）；
//   3. 「重命名」→ `updateNotetype`；「删除」→ 确认框文案按「有没有笔记在用」二选一
//      （`models.py:195-197`）→ `removeNotetype`；
//   4. 「字段…」「卡片…」两个按钮打的是宿主新端点
//      （`openFieldsDialogForNotetype` / `openCardsDialogForNotetype`，请求体是 NotetypeId）；
//   5. 收尾：把这次建出来的类型删干净（按名字找，失败时也不会留下垃圾）。
//
// 为什么用 Playwright：要真的点按钮、在下拉里选、在弹出的输入框里打字、还要读确认框文案。
// Chromium 与 WebView2 同源，加载同一个 URL，按 `tauri/src/bridge.rs` 补垫片。
//
// 前置：外壳已启动（固定 ANKI_API_PORT）。
//
// 用法：
//   $env:ANKI_API_PORT="40021"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/notetypes-check.mjs

import { chromium } from "playwright";

import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
    NotetypeUseCounts,
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

const entries = async () =>
    NotetypeUseCounts.fromBinary(
        await rpc("getNotetypeNamesAndCounts", new Empty({}).toBinary()),
    ).entries;

const names = async () => (await entries()).map((entry) => entry.name);
const idOf = async (name) => (await entries()).find((entry) => entry.name === name)?.id;

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
    window.bridgeCommand = window.pycmd = () => false;
}

const stamp = Date.now() % 100_000;
const addedName = `TauriNT${stamp}`;
const cloneName = `TauriNT${stamp}Clone`;
const renamedName = `TauriNT${stamp}Renamed`;
const created = [addedName, cloneName, renamedName];
const baseline = await names();
console.log(`起始笔记类型 ${baseline.length} 个：${baseline.join(" / ")}`);

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 700, height: 640 } });
    const posted = [];
    const dialogs = [];
    page.on("request", (request) => posted.push(request.url().split("/").pop()));
    page.on("pageerror", (error) => dialogs.push(`[pageerror] ${error}`));
    // 确认框与提示框：记下文案（删除的两种问法就是靠它区分），一律点「确定」
    page.on("dialog", (dialog) => {
        dialogs.push(dialog.message());
        void dialog.accept();
    });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "notetypes" });
    await page.goto(`${base}/notetypes/`, { waitUntil: "load" });
    await page.waitForSelector("#nt-list li button", { timeout: 20_000 });

    const rowTexts = () => page.locator("#nt-list li button").allInnerTexts();
    /** 去掉 Fluent 的数字隔离符（`browsing-note-count` 渲染成「[⁨0⁩ 条笔记]」） */
    const strip = (text) => text.replace(/[\u2068\u2069]/g, "");
    check(
        "列表渲染出全部笔记类型",
        (await rowTexts()).length === baseline.length,
        `${(await rowTexts()).length} / ${baseline.length}`,
    );
    check(
        "每项是「名字 [笔记数]」",
        (await rowTexts()).every((text) => /\[\d+/.test(strip(text))),
        (await rowTexts()).slice(0, 3).map(strip).join(" / "),
    );

    // 1. 添加：原厂类型（Basic）##############################################
    // 弹框是页面里的一棵子树（隐藏时也在 DOM 里），要等「选项填进去了」而不是等元素出现
    const waitForChoices = () =>
        page.waitForFunction(
            () => document.querySelectorAll("#nt-add-select option").length > 0,
            null,
            { timeout: 10_000 },
        );
    await page.locator("#nt-add").click();
    await waitForChoices();
    const choices = await page.locator("#nt-add-select option").allInnerTexts();
    check("添加弹框列了原厂类型", choices.length >= 6, choices.slice(0, 3).join(" / "));
    check(
        "原厂项文案是「Add: <名字>」",
        /^(Add:|添加：)/.test(choices[0]),
        choices[0],
    );
    await page.locator("#nt-add-select").selectOption({ index: 0 });
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForSelector("#nt-text-input", { timeout: 5000 });
    const prefilled = await page.locator("#nt-text-input").inputValue();
    check("第二步预填了原厂类型的名字", prefilled.length > 0, prefilled);
    await page.locator("#nt-text-input").fill(addedName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    const addedId = await idOf(addedName);
    check("原厂类型已加入（RPC 复查）", addedId !== undefined, (await names()).join(" / "));
    if (addedId !== undefined) {
        const notetype = Notetype.fromBinary(
            await rpc("getNotetype", new NotetypeId({ ntid: addedId }).toBinary()),
        );
        check(
            "新类型就是原厂 Basic（2 字段 / 1 模板）",
            notetype.fields.length === 2 && notetype.templates.length === 1,
            `${notetype.fields.length} 字段 / ${notetype.templates.length} 模板`,
        );
    }

    // 2. 添加：克隆现有类型 ##################################################
    await page.locator("#nt-add").click();
    await waitForChoices();
    const cloneIndex = (await page.locator("#nt-add-select option").allInnerTexts()).findIndex((label) =>
        label.includes(addedName),
    );
    check("添加弹框里能克隆现有类型", cloneIndex >= 0, `第 ${cloneIndex + 1} 项`);
    await page.locator("#nt-add-select").selectOption({ index: cloneIndex });
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForSelector("#nt-text-input", { timeout: 5000 });
    await page.locator("#nt-text-input").fill(cloneName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    check("克隆出的类型已加入（RPC 复查）", (await idOf(cloneName)) !== undefined);

    // 3. 重命名（选中行 → F2 之外走按钮）#####################################
    await page.locator(`[data-notetype="${await idOf(addedName)}"]`).click();
    await page.locator("#nt-rename").click();
    await page.waitForSelector("#nt-text-input", { timeout: 5000 });
    check(
        "重命名弹框预填当前名字",
        (await page.locator("#nt-text-input").inputValue()) === addedName,
    );
    await page.locator("#nt-text-input").fill(renamedName);
    await page.locator(".modal.show .modal-footer .btn-primary").click();
    await page.waitForTimeout(1500);
    check(
        "重命名生效（RPC 复查）",
        (await idOf(renamedName)) !== undefined && (await idOf(addedName)) === undefined,
        (await names()).join(" / "),
    );

    // 4. 字段… / 卡片…（宿主端点）############################################
    await page.locator(`[data-notetype="${await idOf(renamedName)}"]`).click();
    await page.locator("#nt-fields").click();
    await page.locator("#nt-cards").click();
    await page.waitForTimeout(1000);
    check(
        "「字段…」打了 openFieldsDialogForNotetype",
        posted.includes("openFieldsDialogForNotetype"),
        posted.filter((url) => url.includes("Notetype")).join(" / "),
    );
    check(
        "「卡片…」打了 openCardsDialogForNotetype",
        posted.includes("openCardsDialogForNotetype"),
    );

    // 5. 删除：没人用的类型 → 确认框是「删掉这个没人用的类型」#################
    dialogs.length = 0;
    await page.locator(`[data-notetype="${await idOf(cloneName)}"]`).click();
    await page.locator("#nt-delete").click();
    await page.waitForTimeout(1200);
    check(
        "删除确认框问的是「没人用的类型」",
        dialogs.some((message) => /unused|没人用|未使用/.test(message)),
        dialogs.join(" / "),
    );
    check("删除后类型消失（RPC 复查）", (await idOf(cloneName)) === undefined);

    // 名字避开 `verify-browser.ps1` 截的**真实窗口**（`notetypes.png`）
    const shotDir = process.env.SHOT_DIR ?? "out/browser-check";
    await page.screenshot({ path: `${shotDir}/notetypes-page.png` });
    console.log(`已截图: ${shotDir}/notetypes-page.png`);
    check("页面没有报错", !dialogs.some((message) => message.startsWith("[pageerror]")), dialogs.join(" / "));
} catch (error) {
    check("管理笔记类型流程", false, String(error));
} finally {
    // 收尾：这次建出来的都删掉（不区分成功与否，按名字找；没建出来的自然也找不到）
    try {
        for (const name of created) {
            const id = await idOf(name);
            if (id !== undefined) {
                await rpc("removeNotetype", new NotetypeId({ ntid: id }).toBinary());
            }
        }
        const left = await names();
        check(
            "已清理临时笔记类型",
            created.every((name) => !left.includes(name)),
            left.join(" / "),
        );
    } catch (problem) {
        console.error(`清理失败: ${problem}`);
    }
    await browser.close();
}

console.log(
    failures.length === 0
        ? "管理笔记类型检查：全部通过"
        : `管理笔记类型检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
