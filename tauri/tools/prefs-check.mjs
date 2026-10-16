// 首选项页面的端到端检查（PARITY 3.3 / 8.1）。
//
// 两件事：
//   ① 结构：七个分页都在、能切换、每页有控件、文案不是 `missing-key`；
//   ② 往返：每个分页挑代表性控件改一次 → **用 RPC 从后端复查配置真的变了** → 全部还原。
//
// 页面在 Chromium 里跑（按 `tauri/src/bridge.rs` 补鉴权头 + `bridgeCommand` 垫片），
// 改动**真的打到宿主**；RPC 断言在 Node 里发，读的是同一个后端。
//
// 前置：外壳以 `ANKI_START_PAGE=decks` 启动 + 固定 `ANKI_API_PORT`。
//
// 用法：
//   $env:ANKI_API_PORT="40061"; $env:ANKI_SHOT_DIR="out/prefs-check"; node tauri/tools/prefs-check.mjs

import { chromium } from "playwright";

import { Preferences, SetConfigJsonRequest } from "../../out/ts/lib/generated/anki/config_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    Json as GenericJson,
    String as GenericString,
} from "../../out/ts/lib/generated/anki/generic_pb.js";

const port = process.env.ANKI_API_PORT ?? "40061";
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

const encodeText = (value) => new TextEncoder().encode(value);

// 注意：`getMetaJson` 回的是 `generic.Json`（字段 1 才是 JSON 文本），
// 必须先解一层 protobuf 再 JSON.parse —— 直接 decode 整个响应会拿到 `\nnull`
const decodeJson = (bytes) =>
    JSON.parse(new TextDecoder().decode(GenericJson.fromBinary(bytes).json));

const getMeta = async (key) =>
    decodeJson(await rpc("getMetaJson", new GenericString({ val: key }).toBinary()));
const getProfileConfig = async (key) =>
    decodeJson(await rpc("getProfileConfigJson", new GenericString({ val: key }).toBinary()));
const getPreferences = async () =>
    Preferences.fromBinary(await rpc("getPreferences", new Empty({}).toBinary()));
const setPreferences = (message) => rpc("setPreferences", message.toBinary());

// 写入：meta / profile 两个端点收的都是 `config.SetConfigJsonRequest`
async function putSetting(method, key, value) {
    await rpc(
        method,
        new SetConfigJsonRequest({
            key,
            valueJson: encodeText(JSON.stringify(value)),
        }).toBinary(),
    );
}
const putMeta = (key, value) => putSetting("setMetaJson", key, value);
const putProfileConfig = (key, value) => putSetting("setProfileConfigJson", key, value);

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
    // 与 `tauri/src/bridge.rs` 的注入脚本一致：命令**真的打到宿主**（这样 `theme:` 之类的
    // 副作用才会发生），有回调时把响应交给回调（`languages` 就是靠它拿列表）
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

const TAB_LABELS = {
    appearance: "显示",
    review: "复习",
    editing: "编辑",
    network: "同步",
    backups: "备份",
    thirdparty: "第三方服务",
    labs: "实验功能",
};

// 会改动的配置，先记下来，最后原样还原
const original = {
    theme: await getMeta("theme"),
    defaultLang: await getMeta("defaultLang"),
    uiScale: await getMeta("uiScale"),
    prefs: await getPreferences(),
    customSyncUrl: await getProfileConfig("customSyncUrl"),
    currentSyncUrl: await getProfileConfig("currentSyncUrl"),
    autoSync: await getProfileConfig("autoSync"),
};

const languages = []; // 由宿主的 `languages` 命令提供，这里只用于断言「选项确实来了」

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 900 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "preferences" });
    await page.goto(`${base}/preferences/`, { waitUntil: "load" });
    await page.waitForSelector("[data-tab]", { timeout: 20_000 });
    // 等 `onMount` 里的配置读回来（loading 结束）
    await page.waitForFunction(() => !document.body.innerText.includes("…"), null, {
        timeout: 20_000,
    });

    const bridgeCalls = () => page.evaluate(() => window.__bridgeCalls ?? []);
    const openTab = async (id) => {
        await page.locator(`[data-tab="${id}"]`).click();
        await page.waitForTimeout(150);
    };

    // ① 结构 ################################################################
    const tabIds = await page.evaluate(() =>
        [...document.querySelectorAll("[data-tab]")].map((el) => el.getAttribute("data-tab")),
    );
    check("七个分页都在", tabIds.length === 7, tabIds.join(" / "));
    const labels = await page.evaluate(() =>
        [...document.querySelectorAll("[data-tab]")].map((el) => el.textContent.trim()),
    );
    check(
        "分页文案是中文（i18n 模块加载正常）",
        labels.join("|") === Object.values(TAB_LABELS).join("|"),
        labels.join(" / "),
    );
    check(
        "页面里没有 missing-key",
        !(await page.evaluate(() => document.body.innerText)).includes("missing-key"),
    );

    // ② 外观：主题 / 语言 / 界面大小 #########################################
    await openTab("appearance");
    check("外观页有语言下拉", (await page.locator("#prefs-lang").count()) === 1);
    check(
        "语言下拉的选项来自宿主（languages 命令）",
        (await bridgeCalls()).includes("languages"),
        (await bridgeCalls()).join(" / "),
    );
    check(
        "语言下拉列出来自宿主的语言表（>40 种）",
        (await page.locator("#prefs-lang option").count()) > 40,
        `${await page.locator("#prefs-lang option").count()} 项`,
    );
    check("外观页有主题下拉", (await page.locator("#prefs-theme").count()) === 1);
    check("外观页有界面大小输入框", (await page.locator("#prefs-scale").count()) === 1);

    // 主题：页面只发命令，落库与广播在宿主（`theme.rs`）
    await page.locator("#prefs-theme").selectOption("2");
    await page.waitForTimeout(600);
    check(
        "改主题发出了 theme:2",
        (await bridgeCalls()).includes("theme:2"),
        (await bridgeCalls()).join(" / "),
    );
    check("宿主把 theme 落库了", (await getMeta("theme")) === 2, String(await getMeta("theme")));

    // 语言：写 meta.defaultLang（下划线码）+ 提示要重启
    await page.locator("#prefs-lang").selectOption("en_US");
    await page.waitForTimeout(600);
    check("改语言写了 meta.defaultLang", (await getMeta("defaultLang")) === "en_US");
    check(
        "改语言提示要重启",
        (await page.locator("#prefs-status").textContent()).includes("重启"),
        await page.locator("#prefs-status").textContent(),
    );

    // 界面大小：meta.uiScale 是 1.5（页面显示 150%）
    await page.locator("#prefs-scale").fill("150");
    await page.locator("#prefs-scale").dispatchEvent("change");
    await page.waitForTimeout(600);
    check("改界面大小写了 meta.uiScale=1.5", (await getMeta("uiScale")) === 1.5);

    // ③ 复习：调度项（集合配置）#############################################
    await openTab("review");
    await page.locator("#prefs-rollover").fill("7");
    await page.locator("#prefs-rollover").dispatchEvent("change");
    await page.waitForTimeout(700);
    check("改「新的一天开始于」写进集合配置", (await getPreferences()).scheduling.rollover === 7);

    const interruptBefore = (await getPreferences()).reviewing.interruptAudioWhenAnswering;
    await page.locator("#prefs-interrupt").click();
    await page.waitForTimeout(700);
    check(
        "改「作答时打断音频」写进集合配置",
        (await getPreferences()).reviewing.interruptAudioWhenAnswering === !interruptBefore,
    );

    // ④ 编辑：搜索相关 ######################################################
    await openTab("editing");
    await page.locator("#prefs-paste-png").click();
    await page.waitForTimeout(700);
    check(
        "改「粘贴图片为 PNG」写进集合配置",
        (await getPreferences()).editing.pasteImagesAsPng === true,
    );
    await page.locator("#prefs-default-search").fill("deck:tauri-prefs");
    await page.locator("#prefs-default-search").dispatchEvent("change");
    await page.waitForTimeout(700);
    check(
        "改「默认搜索词」写进集合配置",
        (await getPreferences()).editing.defaultSearchText === "deck:tauri-prefs",
    );

    // ⑤ 同步：profile 行 + customSyncUrl 连带清 currentSyncUrl ################
    await openTab("network");
    await putProfileConfig("currentSyncUrl", "https://old.example.invalid/");
    await page.locator("#prefs-custom-url").fill("https://new.example.invalid/sync/");
    await page.locator("#prefs-custom-url").dispatchEvent("change");
    await page.waitForTimeout(700);
    check(
        "改「自定义同步服务器」写进 profile 配置",
        (await getProfileConfig("customSyncUrl")) === "https://new.example.invalid/sync/",
        String(await getProfileConfig("customSyncUrl")),
    );
    check(
        "改同步地址时连带清空 currentSyncUrl",
        (await getProfileConfig("currentSyncUrl")) === null,
        String(await getProfileConfig("currentSyncUrl")),
    );

    const autoSyncBefore = (await getProfileConfig("autoSync")) ?? true;
    await page.locator("#prefs-auto-sync").click();
    await page.waitForTimeout(700);
    check(
        "改「自动同步」写进 profile 配置",
        ((await getProfileConfig("autoSync")) ?? true) === !autoSyncBefore,
    );

    // ⑥ 备份：集合配置里的四个数字 ##########################################
    await openTab("backups");
    await page.locator("#prefs-backup-daily").fill("3");
    await page.locator("#prefs-backup-daily").dispatchEvent("change");
    await page.waitForTimeout(700);
    check("改「每日备份份数」写进集合配置", (await getPreferences()).backups.daily === 3);

    // ⑦ 实验室：原有内容还在 ###############################################
    await openTab("labs");
    check(
        "实验页仍列出 Svelte 编辑器开关",
        (await page.locator(".lab-grid").count()) === 1,
    );

    // ⑧ 底部：帮助 + 关闭 ###################################################
    await page.locator("#prefs-help").click();
    await page.waitForTimeout(300);
    await page.locator("#prefs-close").click();
    await page.waitForTimeout(300);
    check("点「关闭」发出 close", (await bridgeCalls()).includes("close"));

    const shots = process.env.ANKI_SHOT_DIR;
    if (shots) {
        await openTab("appearance");
        await page.screenshot({ path: `${shots}/preferences-page.png`, fullPage: true });
        console.log(`已截图: ${shots}/preferences-page.png`);
    }
} finally {
    // 还原：集合配置整份写回，meta / profile 逐键写回
    try {
        await setPreferences(original.prefs);
        await putMeta("theme", original.theme);
        await putMeta("defaultLang", original.defaultLang);
        await putMeta("uiScale", original.uiScale);
        await putProfileConfig("customSyncUrl", original.customSyncUrl);
        await putProfileConfig("currentSyncUrl", original.currentSyncUrl);
        await putProfileConfig("autoSync", original.autoSync);
        console.log("已还原检查前的配置");
    } catch (error) {
        console.log(`还原配置失败：${error}`);
        failures.push("还原配置");
    }
    await browser.close();
}

if (failures.length) {
    console.log(`\n首选项检查：失败 ${failures.length} 项（${failures.join(" / ")}）`);
    process.exit(1);
}
console.log("\n首选项检查：全部通过");
