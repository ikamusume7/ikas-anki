// 夜间模式的页面级检查（PARITY 8.3）：用 Playwright 加载真实的 SvelteKit 页面，断言
// 两条注入通道都能让页面变深色，并读**计算后**的 CSS 变量（不是看截图猜）。
//
// 两条通道（对照 `qt/aqt/webview.py`）：
//   A. URL 尾部的 `#night` → 页面自己 `checkNightMode()`（`ts/lib/tslib/nightmode.ts:6-12`）
//   B. 宿主在 document-start 注入（`crate::bridge` 的 `window_init_script`）——
//      这里用等价的 init script 复现，并断言「没有 hash 时类也不会被抹掉」
//
// 前置：外壳以 `ANKI_START_PAGE=decks` 启动 + 固定 `ANKI_API_PORT`。
//
// 用法：
//   $env:ANKI_API_PORT="40041"; node tauri/tools/theme-check.mjs

import { chromium } from "playwright";

const port = process.env.ANKI_API_PORT ?? "40051";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

const CANVAS_LIGHT = "#f5f5f5";
const CANVAS_DARK = "#2c2c2c";

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
    // 复现 `crate::theme::window_init_script`：document-start 打类
    if (config.night) {
        const apply = () => {
            const doc = document.documentElement;
            if (doc) {
                doc.dataset.bsTheme = "dark";
                doc.classList.add("night-mode");
            }
            if (document.body) {
                document.body.classList.add("isWin", "nightMode", "night_mode");
            }
            return !!(doc && document.body);
        };
        if (!apply()) {
            document.addEventListener("DOMContentLoaded", apply);
        }
    }
}

const readTheme = (page) =>
    page.evaluate(() => ({
        htmlClass: document.documentElement.className,
        bsTheme: document.documentElement.dataset.bsTheme ?? "",
        canvas: getComputedStyle(document.documentElement).getPropertyValue("--canvas").trim(),
        fg: getComputedStyle(document.documentElement).getPropertyValue("--fg").trim(),
        bodyBg: getComputedStyle(document.body).backgroundColor,
        bodyClass: document.body.className,
    }));

const browser = await chromium.launch();
try {
    // A. `#night`（页面自己 checkNightMode）
    const night = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    await night.addInitScript(bridgeShim, { key: apiKey, window: "main", night: false });
    await night.goto(`${base}/decks/#night`, { waitUntil: "load" });
    await night.waitForSelector("table.list tbody tr", { timeout: 20_000 });
    const dark = await readTheme(night);
    check("A/#night：html 上有 night-mode", dark.htmlClass.includes("night-mode"), dark.htmlClass);
    check("A/#night：data-bs-theme=dark", dark.bsTheme === "dark", dark.bsTheme);
    check(`A/#night：--canvas 是深色 ${CANVAS_DARK}`, dark.canvas === CANVAS_DARK, dark.canvas);

    // B. 不带 hash，但宿主在 document-start 注入（类不能被页面抹掉）
    const injected = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    await injected.addInitScript(bridgeShim, { key: apiKey, window: "main", night: true });
    await injected.goto(`${base}/decks/`, { waitUntil: "load" });
    await injected.waitForSelector("table.list tbody tr", { timeout: 20_000 });
    const injectedTheme = await readTheme(injected);
    check(
        "B/宿主注入：没有 hash 时 night-mode 仍在",
        injectedTheme.htmlClass.includes("night-mode"),
        injectedTheme.htmlClass,
    );
    check(
        `B/宿主注入：--canvas 是深色 ${CANVAS_DARK}`,
        injectedTheme.canvas === CANVAS_DARK,
        injectedTheme.canvas,
    );
    check(
        "B/宿主注入：body 上有 nightMode/night_mode",
        injectedTheme.bodyClass.includes("nightMode") && injectedTheme.bodyClass.includes("night_mode"),
        injectedTheme.bodyClass,
    );

    // C. 浅色基线：不带 hash、不注入
    const light = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    await light.addInitScript(bridgeShim, { key: apiKey, window: "main", night: false });
    await light.goto(`${base}/decks/`, { waitUntil: "load" });
    await light.waitForSelector("table.list tbody tr", { timeout: 20_000 });
    const lightTheme = await readTheme(light);
    check("C/基线：html 上没有 night-mode", !lightTheme.htmlClass.includes("night-mode"), lightTheme.htmlClass);
    check(`C/基线：--canvas 是浅色 ${CANVAS_LIGHT}`, lightTheme.canvas === CANVAS_LIGHT, lightTheme.canvas);
    check(
        "C/基线：深浅两套变量确实不同",
        lightTheme.canvas !== dark.canvas && lightTheme.fg !== dark.fg,
        `${lightTheme.fg} vs ${dark.fg}`,
    );

    const shots = process.env.ANKI_SHOT_DIR;
    if (shots) {
        await night.screenshot({ path: `${shots}/theme-night.png` });
        await light.screenshot({ path: `${shots}/theme-light.png` });
        console.log(`已截图: ${shots}/theme-night.png、theme-light.png`);
    }
} finally {
    await browser.close();
}

if (failures.length) {
    console.log(`\n夜间模式检查：失败 ${failures.length} 项（${failures.join(" / ")}）`);
    process.exit(1);
}
console.log("\n夜间模式检查：全部通过");
