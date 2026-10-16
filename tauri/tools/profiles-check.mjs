// 多 profile 管理的端到端检查（PARITY 1.5 / 8.2）。
//
// 页面上真的点：列表渲染 / 当前项标记 / 新建（输入框）/ 非法名提示 / 重命名 / 删除确认，
// 每一步都用**宿主的 `profile:` 命令**复查后端（`profiles` 行 + 目录），最后清理干净。
//
// 注意：切换配置会让宿主**重启进程**，所以这里只点「当前 profile」的打开按钮（无操作），
// 真正的重启链路由 `verify-profiles.ps1` 单独跑（它要等新进程的日志）。
//
// 前置：外壳以 `ANKI_START_PAGE=decks` + `ANKI_CONFIRM_YES=1`（确认框自动点「是」）启动。
//
// 用法：
//   $env:ANKI_API_PORT="40071"; node tauri/tools/profiles-check.mjs

import { chromium } from "playwright";

const port = process.env.ANKI_API_PORT ?? "40071";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;
const failures = [];
const tempNames = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

/** 直接打宿主的 `profile:<json>` 桥接命令（后端复查用）。 */
async function profileCommand(request) {
    const response = await fetch(`${base}/_anki/pycmd`, {
        method: "POST",
        headers: {
            "Content-Type": "text/plain",
            Authorization: `Bearer ${apiKey}`,
            "X-Anki-Window": "main",
        },
        body: `profile:${JSON.stringify(request)}`,
    });
    return JSON.parse(await response.text());
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
    window.bridgeCommand = window.pycmd = (arg, callback) => {
        window.__bridgeCalls.push(String(arg));
        originalFetch("/_anki/pycmd", {
            method: "POST",
            headers: {
                "Content-Type": "text/plain",
                Authorization: `Bearer ${config.key}`,
                "X-Anki-Window": config.window,
            },
            body: String(arg),
        })
            .then((response) => response.text())
            .then((text) => {
                if (!callback || !text) {
                    return;
                }
                try {
                    callback(JSON.parse(text));
                } catch {
                    callback(text);
                }
            })
            .catch(() => {});
        return false;
    };
}

const stamp = Date.now() % 100_000;
const created = `TauriProfile${stamp}`;
const renamed = `${created}Renamed`;

const before = await profileCommand({ action: "list" });
const originalCurrent = before.current;

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "profiles" });
    await page.goto(`${base}/profiles/`, { waitUntil: "load" });
    await page.waitForSelector("[data-profile]", { timeout: 20_000 });

    const listed = () =>
        page.evaluate(() =>
            [...document.querySelectorAll("[data-profile]")].map((el) => ({
                name: el.getAttribute("data-profile"),
                current: el.getAttribute("data-current") === "1",
            })),
        );
    const promptName = async (value) => {
        await page.locator("#profile-name").fill(value);
        await page.locator("#profile-name-ok").click();
        await page.waitForTimeout(700);
    };

    // ① 列表与当前项 ########################################################
    const rows = await listed();
    check(
        "列出全部 profile（与宿主一致）",
        rows.length === before.profiles.length,
        rows.map((row) => row.name).join(" / "),
    );
    check(
        "当前 profile 被标记",
        rows.filter((row) => row.current).map((row) => row.name).join() === originalCurrent,
        rows.filter((row) => row.current).map((row) => row.name).join(" / "),
    );
    check(
        "文案是中文（i18n 加载正常）",
        (await page.locator("h2").textContent()).trim() === "账户",
        await page.locator("h2").textContent(),
    );
    check("有「打开备份文件夹」按钮", (await page.locator("#profiles-backup").count()) === 1);

    // ② 新建 ################################################################
    await page.locator("#profiles-add").click();
    await page.waitForSelector("#profile-name", { timeout: 5000 });
    await promptName(created);
    tempNames.push(created);
    check("界面上出现了新 profile", (await listed()).some((row) => row.name === created));
    check(
        "宿主里也插入了这一行",
        (await profileCommand({ action: "list" })).profiles.includes(created),
    );

    // ③ 非法名 ##############################################################
    await page.locator("#profiles-add").click();
    await page.waitForSelector("#profile-name", { timeout: 5000 });
    await promptName("a/b");
    const status = (await page.locator("#profiles-status").textContent()).trim();
    check("非法名给提示且没建出来", status.length > 0, status);
    check(
        "非法名没有写进宿主",
        !(await profileCommand({ action: "list" })).profiles.includes("a/b"),
    );

    // ④ 重命名 ##############################################################
    await page.locator(`[data-action="rename:${created}"]`).click();
    await page.waitForSelector("#profile-name", { timeout: 5000 });
    await promptName(renamed);
    tempNames.push(renamed);
    const afterRename = await profileCommand({ action: "list" });
    check("重命名后列表里是新名字", (await listed()).some((row) => row.name === renamed));
    check(
        "宿主里旧名字没了、新名字在",
        afterRename.profiles.includes(renamed) && !afterRename.profiles.includes(created),
        afterRename.profiles.join(" / "),
    );

    // ⑤ 删除（确认框由 `ANKI_CONFIRM_YES=1` 自动点「是」）#####################
    await page.locator(`[data-action="delete:${renamed}"]`).click();
    await page.waitForTimeout(1200);
    const afterDelete = await profileCommand({ action: "list" });
    check("删除后宿主里没有它了", !afterDelete.profiles.includes(renamed));
    check("删除后界面里也没有它了", !(await listed()).some((row) => row.name === renamed));

    // ⑥ 「打开」当前 profile 是无操作（真正的切换由 verify-profiles.ps1 验证）####
    await page.locator("#profiles-open").click();
    await page.waitForTimeout(600);
    check(
        "点「打开」当前 profile 不发重启（状态为空）",
        (await page.locator("#profiles-status").textContent()).trim() === "",
        await page.locator("#profiles-status").textContent(),
    );
} finally {
    // 清理：把检查期间建的临时 profile 都删掉（走宿主命令，避免留下垃圾）
    for (const name of tempNames) {
        await profileCommand({ action: "delete", name }).catch(() => {});
    }
    const left = (await profileCommand({ action: "list" })).profiles.filter((name) =>
        tempNames.includes(name),
    );
    if (left.length) {
        console.log(`清理不干净：${left.join(" / ")}`);
        failures.push("清理临时 profile");
    } else {
        console.log("已清理临时 profile");
    }
    await browser.close();
}

if (failures.length) {
    console.log(`\n多 profile 检查：失败 ${failures.length} 项（${failures.join(" / ")}）`);
    process.exit(1);
}
console.log("\n多 profile 检查：全部通过");
