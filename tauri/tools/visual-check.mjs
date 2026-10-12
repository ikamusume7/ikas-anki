// 对复习界面做可重复的布局/视觉检查。
//
// 为什么不用截 Tauri 窗口：窗口截图能看到"结果"，但拿不到 DOM 度量，定位问题时
// 只能靠猜。这里让 Chromium（与 WebView2 同源）加载同一个 legacy 页面，补上
// 与 tauri/src/bridge.rs 等价的垫片，然后**复现宿主注入的内容**
// （底部栏 + 卡片），从而得到精确的坐标、计算样式与截图。
//
// 前置：先启动外壳（ANKI_START_PAGE=reviewer，固定 ANKI_API_PORT）。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/visual-check.mjs

import fs from "node:fs";

import { chromium } from "playwright";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const outDir = process.env.SHOT_DIR ?? "out";
const base = `http://127.0.0.1:${port}`;

// 与 tauri/src/bridge.rs 等价：补鉴权头 + 提供 bridgeCommand
function bridgeShim(key) {
    const originalFetch = window.fetch.bind(window);
    window.fetch = (input, init) => {
        const url = typeof input === "string" ? input : (input && input.url) || "";
        if (url.startsWith("/_anki/")) {
            init = init ?? {};
            init.headers = { ...(init.headers ?? {}), Authorization: `Bearer ${key}` };
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

// 与 tauri/src/reviewer.rs 的 bottom_bar_html() 输出一致
const bottomBar = `<center id="outer">
<table id="innertable" width="100%" cellspacing="0" cellpadding="0">
<tr>
<td align="start" valign="top" class="stat"><button title="快捷键：E" onclick="pycmd('edit');">编辑</button></td>
<td align="center" valign="top" id="middle"></td>
<td align="end" valign="top" class="stat"><button title="快捷键：M" onclick="pycmd('more');">更多 <span id="time" class="stattxt"></span></button></td>
</tr>
</table>
</center>`;

const answerButton = `<table cellpadding="0"><tr><td class="stat2" align="center"><button title="快捷键：空格" id="ansbut" onclick='pycmd("ans");'>显示答案</button></td></tr></table>`;

const question =
    `<style>body { font-family: sans-serif; }</style><b>前端渲染验证：问题面</b><br>这张卡片由 seed-card.mjs 写入。`;

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 820 } });
    await page.addInitScript(bridgeShim, apiKey);
    await page.goto(`${base}/_anki/legacyPageData?id=1`, { waitUntil: "load" });
    await page.waitForFunction(
        () => typeof window._showQuestion === "function" && typeof window.showQuestion === "function",
        null,
        { timeout: 20_000 },
    );

    // 复现宿主在取卡后做的两次 eval
    await page.evaluate((html) => {
        document.getElementById("bottom-container").innerHTML = html;
    }, bottomBar);
    await page.evaluate((q) => window._showQuestion(q, "", ""), question);
    await page.evaluate((btn) => window.showQuestion(btn, 0), answerButton);
    await page.waitForTimeout(500);

    const metrics = await page.evaluate(() => {
        const describe = (selector) => {
            const el = document.querySelector(selector);
            if (!el) {
                return null;
            }
            const rect = el.getBoundingClientRect();
            const style = getComputedStyle(el);
            return {
                rect: {
                    x: Math.round(rect.x),
                    y: Math.round(rect.y),
                    w: Math.round(rect.width),
                    h: Math.round(rect.height),
                },
                position: style.position,
                margin: style.margin,
                padding: style.padding,
            };
        };
        return {
            viewport: { w: innerWidth, h: innerHeight },
            body: describe("body"),
            qa: describe("#qa"),
            bottomContainer: describe("#bottom-container"),
            outer: describe("#outer"),
            buttons: [...document.querySelectorAll("button")].map((button) => {
                const rect = button.getBoundingClientRect();
                return {
                    text: button.textContent.trim(),
                    x: Math.round(rect.x),
                    y: Math.round(rect.y),
                    w: Math.round(rect.width),
                    h: Math.round(rect.height),
                };
            }),
            styleSheets: [...document.styleSheets].map((sheet) => sheet.href).filter(Boolean),
            bridgeCalls: window.__bridgeCalls,
        };
    });

    console.log(JSON.stringify(metrics, null, 2));

    fs.mkdirSync(outDir, { recursive: true });
    const shot = `${outDir}/visual-check.png`;
    await page.screenshot({ path: shot });
    console.log(`截图: ${shot}`);

    // 回归断言。这几条不变量此前被打破过（底部栏悬在卡片下方、卡片紧贴窗口边缘），
    // 固化成断言后，后续改动只要碰坏布局就会立刻失败。
    const failures = [];
    if (metrics.body.margin !== "20px") {
        failures.push(`body 外边距应为 20px（reviewer.scss 的值），实际 ${metrics.body.margin}`);
    }
    if (metrics.bottomContainer.position !== "fixed") {
        failures.push(`底部栏应为固定定位，实际 ${metrics.bottomContainer.position}`);
    }
    const barBottom = metrics.bottomContainer.rect.y + metrics.bottomContainer.rect.h;
    if (Math.abs(barBottom - metrics.viewport.h) > 1) {
        failures.push(`底部栏未贴住窗口底部（${barBottom} vs ${metrics.viewport.h}）`);
    }
    for (const label of ["编辑", "更多"]) {
        if (!metrics.buttons.some((button) => button.text.startsWith(label))) {
            failures.push(`底部栏缺少按钮：${label}`);
        }
    }
    for (const button of metrics.buttons) {
        if (button.x < 0 || button.x + button.w > metrics.viewport.w) {
            failures.push(`按钮超出窗口：${button.text}（x=${button.x} w=${button.w}）`);
        }
    }

    if (failures.length > 0) {
        console.error(`布局检查未通过：\n${failures.map((item) => `  - ${item}`).join("\n")}`);
        process.exitCode = 1;
    } else {
        console.log("布局检查通过：底部栏贴住窗口底部、卡片内边距正常、底部栏按钮齐全且在窗口内");
    }
} finally {
    await browser.close();
}
