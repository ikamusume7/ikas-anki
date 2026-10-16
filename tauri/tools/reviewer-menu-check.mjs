// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 复习界面「更多」/右键菜单的检查。分两半：
//
//   1. **菜单 UI**：把宿主注入的那份脚本（`tauri/assets/reviewer-menu.js`，宿主用
//      `include_str!` 引用，见 `src/reviewer_menu.rs`）加载到一个空页面里，喂一份
//      菜单数据，断言渲染、勾选态、子菜单、表单对话框与回传的桥接命令。
//      宿主注入到 WebView 里的脚本没法从 Playwright 直接看（那在 WebView2 里），
//      所以这里测的是**同一个文件**，避免"测的不是跑的那份"。
//   2. **宿主动作**：对着真外壳发 `rev:<动作>` 桥接命令，用 RPC 复查集合状态
//      （旗标、标记、到期日）。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer）、集合里至少有一张卡。
//
// 用法：
//   $env:ANKI_API_PORT="40028"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/reviewer-menu-check.mjs

import fs from "node:fs";
import http from "node:http";
import zlib from "node:zlib";

import { chromium } from "playwright";

import { SetSettingJsonRequest } from "../../out/ts/lib/generated/anki/frontend_pb.js";
import { Card, CardId, SetFlagRequest } from "../../out/ts/lib/generated/anki/cards_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";
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
            // 不让 socket 留着（keep-alive 的句柄会在 node 退出时触发 libuv 断言）
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/** 发一条桥接命令（模拟页面里的 `pycmd`）。 */
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

/** 改 profile 配置（同步凭据就住在那儿，与桌面版同一批键）。 */
async function setProfile(key, value) {
    await rpc(
        "setProfileConfigJson",
        new SetSettingJsonRequest({
            key,
            valueJson: new TextEncoder().encode(JSON.stringify(value)),
        }).toBinary(),
    );
}

/** 直接改旗标（测试前把状态归零用）。 */
async function setCardFlag(cardId, flag) {
    await rpc(
        "setFlag",
        new SetFlagRequest({ cardIds: [cardId], flag }).toBinary(),
    );
}

const userFlag = (card) => card.flags & 7;

// ---------------------------------------------------------------------------
// 1. 菜单脚本本身（同一个文件）
// ---------------------------------------------------------------------------
const script = fs.readFileSync("tauri/assets/reviewer-menu.js", "utf8");
const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
    await page.setContent('<html><body><p id="card">卡片</p></body></html>');
    // 桩要在 `setContent` 之后装：`addInitScript` 只在导航时跑，`setContent` 不算导航
    await page.evaluate(() => {
        window.__commands = [];
        window.pycmd = (command) => window.__commands.push(command);
    });
    await page.addScriptTag({ content: script });

    // 与宿主拼出来的形状一致（见 `reviewer_menu::show`）
    const spec = {
        items: [
            {
                label: "标记卡片",
                submenu: [
                    { label: "红", shortcut: "Ctrl+1", checked: false, action: "flag:1" },
                    { label: "绿", shortcut: "Ctrl+3", checked: true, action: "flag:3" },
                ],
            },
            { label: "埋藏卡片", shortcut: "-", action: "buryCard" },
            { label: "重置卡片…", shortcut: "Ctrl+Alt+N", action: "forget" },
            null,
            { label: "标记笔记", shortcut: "*", checked: false, action: "mark" },
        ],
    };
    await page.evaluate((menu) => window.showReviewerMenu(menu), spec);
    check("菜单已渲染", (await page.locator(".rev-menu").count()) === 1);
    // 只数顶层项（子菜单里的也算 `.rev-menu-item`）
    check(
        "菜单项与分隔线齐全",
        (await page.locator(".rev-menu > .rev-menu-item").count()) === 4 &&
            (await page.locator(".rev-menu > .rev-menu-separator").count()) === 1,
        `${await page.locator(".rev-menu > .rev-menu-item").count()} 项`,
    );
    check(
        "子菜单里的勾选态来自数据",
        (await page.locator('[data-action="flag:3"] .rev-menu-check').innerText()).trim() === "✓" &&
            (await page.locator('[data-action="flag:1"] .rev-menu-check').innerText()).trim() === "",
    );
    // 子菜单默认隐藏，要先悬停父项（`display: none` → `block`）
    await page.locator(".rev-menu-parent").hover();
    check("子菜单悬停展开", await page.locator(".rev-submenu").first().isVisible());
    await page.locator('[data-action="flag:1"]').click();
    check(
        "点菜单项回传 rev:<动作>",
        (await page.evaluate(() => window.__commands)).join("|") === "rev:flag:1",
        (await page.evaluate(() => window.__commands)).join("|"),
    );
    check("点完菜单关闭", (await page.locator(".rev-menu").count()) === 0);

    // 卡片右键 → 打开菜单（对照 Qt 给 review webview 设的 context menu policy）
    await page.evaluate(() => (window.__commands.length = 0));
    await page.locator("#card").click({ button: "right" });
    check(
        "右键发 more",
        (await page.evaluate(() => window.__commands)).join("|") === "more",
        (await page.evaluate(() => window.__commands)).join("|"),
    );

    // 表单对话框（「重置卡片…」用）
    await page.evaluate(() =>
        window.showReviewerDialog({
            title: "重置卡片",
            action: "forget",
            labels: { cancel: "取消", confirm: "保存" },
            fields: [
                { kind: "check", label: "恢复位置", value: true },
                { kind: "check", label: "重置计数", value: false },
            ],
        }),
    );
    check("表单对话框已渲染", (await page.locator(".rev-dialog").count()) === 1);
    await page.locator(".rev-dialog .rev-primary").click();
    check(
        "表单确认回传 rev:forget:1,0",
        (await page.evaluate(() => window.__commands)).at(-1) === "rev:forget:1,0",
        (await page.evaluate(() => window.__commands)).at(-1),
    );

    // 文本字段（「设置到期日…」用）：值要 URL 编码
    await page.evaluate(() =>
        window.showReviewerDialog({
            title: "设置到期日",
            action: "due",
            labels: { cancel: "取消", confirm: "保存" },
            fields: [{ kind: "text", label: "到期日", value: "2026-11-01" }],
        }),
    );
    await page.locator(".rev-dialog input[type=text]").fill("2026-11-01");
    await page.locator(".rev-dialog .rev-primary").click();
    check(
        "表单文本值会 URL 编码",
        (await page.evaluate(() => window.__commands)).at(-1) === "rev:due:2026-11-01",
        (await page.evaluate(() => window.__commands)).at(-1),
    );

    // -----------------------------------------------------------------------
    // 顶部工具栏（同一套做法：读宿主注入的那份脚本，见 `assets/reviewer-toolbar.js`）
    // -----------------------------------------------------------------------
    const toolbarScript = fs.readFileSync("tauri/assets/reviewer-toolbar.js", "utf8");
    const toolbarPage = await browser.newPage({ viewport: { width: 1000, height: 700 } });
    await toolbarPage.setContent(
        '<html><body><div id="top-toolbar"></div><div id="qa">卡片</div></body></html>',
    );
    await toolbarPage.evaluate(() => {
        window.__commands = [];
        window.pycmd = (command) => window.__commands.push(command);
    });
    await toolbarPage.addScriptTag({ content: toolbarScript });
    // 与宿主拼出来的形状一致（见 `reviewer::toolbar_payload`）
    const toolbarSpec = {
        hostId: "top-toolbar",
        links: [
            { id: "decks", cmd: "decks", label: "牌组", tip: "快捷键：D" },
            { id: "add", cmd: "add", label: "添加", tip: "快捷键：A" },
            { id: "browse", cmd: "browse", label: "浏览", tip: "快捷键：B" },
            { id: "stats", cmd: "stats", label: "统计", tip: "快捷键：T" },
            { id: "sync", cmd: "sync", label: "同步", tip: "快捷键：Y" },
        ],
    };
    check(
        "工具栏已渲染",
        await toolbarPage.evaluate((data) => window.installToolbar(data), toolbarSpec),
    );
    const items = await toolbarPage.locator("#top-toolbar .hitem").allInnerTexts();
    check(
        "工具栏五个链接（牌组/添加/浏览/统计/同步）",
        items.join("|") === "牌组|添加|浏览|统计|同步",
        items.join("|"),
    );
    check(
        "同步链接带 Y 快捷键提示",
        (await toolbarPage.locator("#top-toolbar #sync").getAttribute("title")) === "快捷键：Y",
    );
    check(
        "每个链接带快捷键提示",
        (await toolbarPage.locator('#top-toolbar #stats').getAttribute("title")) ===
            "快捷键：T",
    );
    // 只断言 CSS 属性不够：间距必须真的落在**链接**之间（第一版把 flex/gap 放在
    // 外层 `.header` 上，而链接在里层 `.toolbar` 里，于是四个链接挤成一串）
    const boxes = await toolbarPage.evaluate(() =>
        [...document.querySelectorAll("#top-toolbar .hitem")].map((el) => el.getBoundingClientRect()),
    );
    check(
        "链接之间有实际间距",
        boxes.every((box, index) => index === 0 || box.left >= boxes[index - 1].right + 10),
        boxes.map((box) => `${Math.round(box.left)}~${Math.round(box.right)}`).join(" / "),
    );
    check(
        "工具栏铺在整行并居中",
        await toolbarPage.evaluate(() => {
            const bar = document.getElementById("top-toolbar");
            const header = document.querySelector("#top-toolbar .header");
            return (
                getComputedStyle(bar).position === "fixed" &&
                getComputedStyle(header).justifyContent === "center"
            );
        }),
    );
    await toolbarPage.locator("#top-toolbar #browse").click();
    check(
        "点「浏览」回传 browse",
        (await toolbarPage.evaluate(() => window.__commands)).join("|") === "browse",
        (await toolbarPage.evaluate(() => window.__commands)).join("|"),
    );
    check(
        "工具栏高度写进了 CSS 变量",
        /^\d+(\.\d+)?px$/.test(
            await toolbarPage.evaluate(() =>
                document.documentElement.style.getPropertyValue("--top-bar-height"),
            ),
        ),
    );
    await toolbarPage.evaluate((data) => window.installToolbar(data), toolbarSpec);
    check(
        "重复注入不会叠加",
        (await toolbarPage.locator("#top-toolbar .header").count()) === 1,
    );
    // 同步状态：宿主跑完会用 `window.__ankiSyncStatus()` 往工具栏写一行
    await toolbarPage.evaluate(() => window.__ankiSyncStatus("正在同步…"));
    check(
        "同步状态能写进工具栏",
        (await toolbarPage.locator("#top-toolbar #sync-status").innerText()) === "正在同步…",
        await toolbarPage.locator("#top-toolbar #sync-status").innerText(),
    );
    await toolbarPage.close();
} finally {
    await browser.close();
}

// ---------------------------------------------------------------------------
// 2. 宿主动作（真外壳）
// ---------------------------------------------------------------------------
try {
    const queued = QueuedCards.fromBinary(
        await rpc("getQueuedCards", new GetQueuedCardsRequest({ fetchLimit: 1 }).toBinary()),
    );
    const card = queued.cards[0]?.card;
    if (!card) {
        check("复习队列里有卡片", false, "队列为空，无法测试菜单动作");
        throw new Error("队列为空");
    }
    const readCard = async () =>
        Card.fromBinary(await rpc("getCard", new CardId({ cid: card.id }).toBinary()));

    await bridge("more");
    check("more 命令被接受", true);

    // 旗标：设置 → 同旗标再设 = 清除（对照 `reviewer.py:1099-1108`）。
    // 先把旗标清干净：上一次失败的运行可能留下非 0 的值，而"同值即清除"的语义
    // 会让断言随初始状态而变。
    await setCardFlag(card.id, 0);
    check("测试前旗标已归零", userFlag(await readCard()) === 0);
    await bridge("rev:flag:3");
    check("rev:flag:3 落到卡片上", userFlag(await readCard()) === 3);
    await bridge("rev:flag:3");
    check("同一旗标再点一次 = 清除", userFlag(await readCard()) === 0);

    // 标记：加 → 去（`marked` 标签）
    const markedCount = async () =>
        SearchResponse.fromBinary(
            await rpc("searchNotes", new SearchRequest({ search: "tag:marked" }).toBinary()),
        ).ids.length;
    // 自愈：上一次失败的运行可能把 marked 标签留在这张卡的笔记上（`rev:mark` 是 toggle，
    // 留着的话「加一次」的断言会变成「减一次」）。注意看的是**这张卡的笔记**，
    // 不能看全集合的 marked 数量（别的笔记也会让它 > 0）
    const noteMarked = async () =>
        SearchResponse.fromBinary(
            await rpc(
                "searchNotes",
                new SearchRequest({ search: `nid:${card.noteId} tag:marked` }).toBinary(),
            ),
        ).ids.length > 0;
    if (await noteMarked()) {
        await bridge("rev:mark");
    }
    const before = await markedCount();
    await bridge("rev:mark");
    check("rev:mark 加了 marked 标签", (await markedCount()) === before + 1);
    await bridge("rev:mark");
    check("再点一次取消标记", (await markedCount()) === before);

    // 设置到期日（跳过对话框直接给值）
    const dueBefore = (await readCard()).due;
    await bridge("rev:due:5");
    check(
        "rev:due:5 改了到期日",
        (await readCard()).due !== dueBefore,
        `${dueBefore} → ${(await readCard()).due}`,
    );
    await bridge("undo");
    check("撤销后到期日还原", (await readCard()).due === dueBefore);

    // 「上一张卡片信息」（PARITY 2.2 / `Ctrl+Alt+I`）：答一张卡 → 宿主把它记成"上一张" →
    // `rev:prevCardInfo` 打开的就是它（日志里的 card id 必须对得上）。之后 `undo` 还原。
    if (process.env.ANKI_LOG) {
        const logOf = () => {
            try {
                return fs.readFileSync(process.env.ANKI_LOG, "utf8");
            } catch {
                return "";
            }
        };
        const infos = () =>
            [...logOf().matchAll(/打开(?:上一张)?卡片信息: card=(\d+)/g)].map((match) => match[1]);
        const before = infos().length;
        await bridge("ans");
        await new Promise((resolve) => setTimeout(resolve, 900));
        await bridge("ease3");
        await new Promise((resolve) => setTimeout(resolve, 1500));
        await bridge("rev:prevCardInfo");
        const deadline = Date.now() + 15_000;
        while (Date.now() < deadline && infos().length <= before) {
            await new Promise((resolve) => setTimeout(resolve, 300));
        }
        const seen = infos();
        check(
            "rev:prevCardInfo 打开的是刚答掉那张卡",
            seen.length > before && seen.at(-1) === String(card.id),
            `日志里的卡片：${seen.slice(before).join(" / ") || "（没有）"}，刚答掉的是 ${card.id}`,
        );
        // 宿主侧拼出来的真实菜单项数（13 项 + 1 条分隔线），新增项就在里面
        await bridge("more");
        check(
            "真实菜单是 20 项（含「上一张卡片信息」「重播音频」「录音」「重播录音」「暂停」「±5 秒」「自动前进」）",
            /打开复习菜单: .*共 20 项/.test(logOf()),
            (logOf().match(/打开复习菜单: [^\n]*/) ?? ["（没有菜单日志）"]).at(-1),
        );
        await bridge("undo");
        await new Promise((resolve) => setTimeout(resolve, 1200));
    } else {
        console.log("没有 ANKI_LOG，跳过「上一张卡片信息」的宿主断言");
    }

    // 复习界面工具栏的「同步」：宿主的 `sync` 命令 → 真的连同步服务器
    // （假服务器只实现 `/sync/meta`，与 decks-check 5i 同一套路）
    const syncPaths = [];
    const fakeSync = http.createServer((request, response) => {
        syncPaths.push(request.url ?? "");
        if (request.url === "/sync/meta") {
            const raw = Buffer.from(
                JSON.stringify({
                    mod: 1,
                    scm: 1,
                    usn: 1,
                    // `ts` 必须是当前时间，否则客户端报 ClockIncorrect（消息还是空的）；
                    // `media_usn` 没有 `#[serde(default)]`，缺了直接解析失败
                    ts: Math.floor(Date.now() / 1000),
                    msg: "",
                    cont: true,
                    hostNum: 1,
                    media_usn: 1,
                }),
            );
            response.writeHead(200, {
                "Content-Type": "application/octet-stream",
                "anki-original-size": String(raw.length),
            });
            response.end(zlib.zstdCompressSync(raw));
            return;
        }
        response.writeHead(500, { "Content-Type": "text/plain" });
        response.end("fake sync server");
    });
    await new Promise((resolve) => fakeSync.listen(0, "127.0.0.1", resolve));
    const syncEndpoint = `http://127.0.0.1:${fakeSync.address().port}/`;
    await setProfile("syncKey", "fake-host-key");
    await setProfile("customSyncUrl", syncEndpoint);
    syncPaths.length = 0;
    await bridge("sync");
    await new Promise((resolve) => setTimeout(resolve, 4000));
    check(
        "工具栏「同步」→ 宿主连上了同步服务器",
        syncPaths.includes("/sync/meta"),
        syncPaths.join(" / ") || "（没有请求）",
    );
    await setProfile("syncKey", null);
    await setProfile("customSyncUrl", null);
    await new Promise((resolve) => fakeSync.close(resolve));
} catch (error) {
    check("宿主动作检查", false, String(error));
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
