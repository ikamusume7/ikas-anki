// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 同步的**真实成功路径**端到端检查（PARITY 6.1 缺口①）。
//
// 现有检查（`decks-check.mjs` 5i）用的是只实现 `/sync/hostKey` + `/sync/meta` 的假服务器，
// 之后一路 500 —— 这个检查改用仓库自带的真同步服务器（`anki-sync-server`，
// 由 `tauri/tools/sync-server.ps1` 起），所以能断言：
//
//   1. 登录真的打到 `/sync/hostKey`，host key 落 profile（`syncKey`）；
//   2. 服务器上没有集合时，整份同步弹「上传/下载」→ 选上传 → **服务器侧真的出现集合**
//      （`SYNC_BASE/<user>/collection.anki2`，以及服务器日志里的 `/sync/upload`）；
//   3. 客户端侧的 `syncStatus`/`syncCollection` 走到成功（状态文字 + 日志）。
//
// 前置：外壳已启动（`ANKI_API_PORT`/`ANKI_API_KEY`/`ANKI_LOG`）、同步服务器已就绪
// （`SYNC_PORT`、`SYNC_BASE`、`SYNC_USER`）。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"; $env:ANKI_LOG="out/..."
//   $env:SYNC_PORT="40011"; $env:SYNC_BASE="out/tauri-test/sync-server"; $env:SYNC_USER="tauri"
//   $env:SYNC_PASSWORD="tauri-pass"
//   node tauri/tools/sync-check.mjs

import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";

import { SetSettingJsonRequest } from "../../out/ts/lib/generated/anki/frontend_pb.js";
import { Json, String as GenericString } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    AddNoteRequest,
    AddNoteResponse,
    Note,
} from "../../out/ts/lib/generated/anki/notes_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
} from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import { Deck } from "../../out/ts/lib/generated/anki/decks_pb.js";
import { AddMediaFileRequest } from "../../out/ts/lib/generated/anki/media_pb.js";

const port = process.env.FIRST_API_PORT ?? process.env.ANKI_API_PORT ?? "40001";
const port2 = process.env.SECOND_API_PORT ?? process.env.ANKI_API_PORT2 ?? "";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
const syncPort = process.env.SYNC_PORT ?? "40011";
const syncBase = process.env.SYNC_BASE ?? "";
const syncUser = process.env.SYNC_USER ?? "tauri";
const syncPassword = process.env.SYNC_PASSWORD ?? "tauri-pass";
const base = `http://127.0.0.1:${port}`;
const base2 = port2 ? `http://127.0.0.1:${port2}` : "";
const endpoint = `http://127.0.0.1:${syncPort}/`;
const serverLog = process.env.SYNC_LOG ?? "";
const collection2 = process.env.SECOND_COLLECTION ?? "";
const collection1 = process.env.FIRST_COLLECTION ?? "";
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function readLog(file) {
    try {
        return fs.readFileSync(file, "utf8");
    } catch {
        return "";
    }
}

const hostLog = () => readLog(logPath);
const syncLog = () => readLog(serverLog);

/** 媒体夹：集合所在目录下的 `collection.media`（与桌面版同一套命名）。 */
const mediaDirOf = (collectionPath) => path.join(path.dirname(collectionPath), "collection.media");

async function waitFor(name, predicate, timeoutMs = 60_000, intervalMs = 500) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        try {
            if (await predicate()) {
                return true;
            }
        } catch {
            // 服务器/文件还没就绪，继续等
        }
        await sleep(intervalMs);
    }
    console.log(`  （等待超时：${name}）`);
    return false;
}

async function rpcOn(target, method, body) {
    const response = await fetch(`${target}/_anki/${method}`, {
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

const setProfile = (key, value) =>
    rpc(
        "setProfileConfigJson",
        new SetSettingJsonRequest({
            key,
            valueJson: new TextEncoder().encode(JSON.stringify(value)),
        }).toBinary(),
    );

const rpc = (method, body) => rpcOn(base, method, body);

/** 服务器侧那份集合（上传成功后才有）。 */
const serverCollection = path.join(syncBase, syncUser, "collection.anki2");

/// Playwright 里的页面需要自己带上 API 凭据与窗口身份（与 `decks-check.mjs` 同一套 shim）。
/// 真窗口由宿主的注入脚本给这些头，Playwright 加载的页面没有，缺了就会一路 401
/// （`has_api_access` 只认 `Authorization: Bearer <api_key>`）。
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
        window
            .fetch("/_anki/pycmd", {
                method: "POST",
                headers: { "Content-Type": "text/plain" },
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
let browser = null;
let page2 = null;

try {
    if (!logPath) {
        throw new Error("缺少 ANKI_LOG");
    }
    if (!syncBase) {
        throw new Error("缺少 SYNC_BASE");
    }
    // 服务器侧那份集合由 `verify-sync.ps1` 在**起服务器之前**清掉：服务器开着时它占着文件，
    // 这边删会 EPERM。清干净之后整份同步才会走「上传/下载」分支（下一段会断言那个弹框）。

    // 造一张卡：本地集合非空，服务器为空 —— 这样整份同步会弹「上传/下载」
    const deckId = Deck.fromBinary(
        await rpc("getCurrentDeck", new Empty({}).toBinary()),
    ).id;
    const names = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    );
    const pick =
        names.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ?? names.entries[0];
    const notetype = Notetype.fromBinary(
        await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
    );
    const marker = `TauriSync ${Date.now() % 1_000_000}`;
    await rpc(
        "addNote",
        new AddNoteRequest({
            note: new Note({ notetypeId: notetype.id, fields: [marker, "同步检查"] }),
            deckId,
        }).toBinary(),
    );

    // 端点在 profile 里（`customSyncUrl`，与桌面版同一把键）；host key 先清掉
    await setProfile("customSyncUrl", endpoint);
    await setProfile("syncKey", null);

    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1200, height: 800 } });
    await page.addInitScript(bridgeShim, { key: apiKey, window: "main" });
    await page.goto(`${base}/decks/`, { waitUntil: "load" });
    await page.waitForSelector("table.list tbody tr", { timeout: 15_000 });

    // 1. 登录：真打 `/sync/hostKey`
    const hostKeyBefore = syncLog().split("/sync/hostKey").length - 1;
    await page.locator("#sync").click();
    try {
        await page.waitForSelector(".modal.show #sync-user", { timeout: 20_000 });
    } catch (error) {
        // 诊断：是"弹框没出来"还是"选择器不对"
        const state = await page.locator("#sync").getAttribute("data-sync-state");
        const status = await page.locator("#sync-status").innerText().catch(() => "（没有状态文字）");
        const modals = await page.locator(".modal").count();
        const shown = await page.locator(".modal.show").count();
        throw new Error(
            `${error.message}；data-sync-state=${state} 状态文字=${JSON.stringify(status)}` +
                ` .modal=${modals} .modal.show=${shown}`,
        );
    }
    await page.locator(".modal.show #sync-user").fill(syncUser);
    await page.locator(".modal.show #sync-password").fill(syncPassword);
    await page.locator(".modal.show #sync-endpoint").fill(endpoint);
    await page.locator(".modal.show #sync-login").click();
    check(
        "登录打到了真服务器的 /sync/hostKey",
        await waitFor("hostKey", () =>
            syncLog().split("/sync/hostKey").length - 1 > hostKeyBefore,
        ),
        serverLog,
    );
    // 登录成功之后页面不该再停在"未登录"状态（`#sync` 的 data-sync-state 由页面维护）
    check(
        "登录后页面离开「未登录」状态",
        await waitFor(
            "sync state after login",
            async () => (await page.locator("#sync").getAttribute("data-sync-state")) !== "off",
            30_000,
        ),
        await page.locator("#sync").getAttribute("data-sync-state"),
    );

    // 2. 整份同步：服务器没有集合 → 弹「上传/下载」→ 选上传
    const uploadedBefore = syncLog().split("/sync/upload").length - 1;
    await page.waitForSelector("#sync-upload", { state: "visible", timeout: 60_000 });
    check("服务器没有集合时弹出了「上传/下载」选择", true);
    await page.locator("#sync-upload").click();
    check(
        "上传打到了真服务器的 /sync/upload",
        await waitFor(
            "upload",
            () => syncLog().split("/sync/upload").length - 1 > uploadedBefore,
            90_000,
        ),
        serverLog,
    );
    check(
        "服务器侧真的出现了集合（collection.anki2）",
        await waitFor("server collection", () => fs.existsSync(serverCollection), 60_000),
        serverCollection,
    );
    const size = fs.existsSync(serverCollection) ? fs.statSync(serverCollection).size : 0;
    check("服务器侧集合不是空文件", size > 1024, `${size} 字节`);

    // 3. 客户端状态回到正常
    check(
        "同步状态回到非 error",
        await waitFor(
            "sync state",
            async () => (await page.locator("#sync").getAttribute("data-sync-state")) !== "error",
            30_000,
        ),
        await page.locator("#sync").getAttribute("data-sync-state"),
    );
    // 这条同步是**页面**驱动的（不是 `autosync.rs` 的自动同步），所以成功痕迹要看服务器
    // 日志的整条链路：hostKey → meta（登录后）→ upload，全 200
    check(
        "服务器日志里整条链路都是 200（hostKey → meta → upload）",
        /"\/sync\/hostKey"[^\n]*httpstatus=200/.test(syncLog()) &&
            /"\/sync\/meta"[^\n]*httpstatus=200/.test(syncLog()) &&
            /"\/sync\/upload"[^\n]*httpstatus=200/.test(syncLog()),
        (syncLog().match(/"\/sync\/[a-zA-Z]+"[^\n]*httpstatus=\d+/g) ?? []).join(" / "),
    );
    // 4. 整份**下载**方向：第二个客户端（空集合、同一个 host key）从服务器拉回同一份集合
    if (port2) {
        // 同一台机器上"已经登录过"的场景：把第一个客户端的 host key 搬进第二个 profile
        const keyJson = new TextDecoder().decode(
            Json.fromBinary(
                await rpc(
                    "getProfileConfigJson",
                    new GenericString({ val: "syncKey" }).toBinary(),
                ),
            ).json,
        ).trim();
        const hostKey = JSON.parse(keyJson);
        check(
            "拿到第一个客户端的 host key",
            typeof hostKey === "string" && hostKey.length > 0,
            JSON.stringify(hostKey),
        );
        for (const [key, value] of [
            ["customSyncUrl", endpoint],
            ["syncKey", hostKey],
        ]) {
            await rpcOn(
                base2,
                "setProfileConfigJson",
                new SetSettingJsonRequest({
                    key,
                    valueJson: new TextEncoder().encode(JSON.stringify(value)),
                }).toBinary(),
            );
        }

        page2 = await browser.newPage({ viewport: { width: 1200, height: 800 } });
        await page2.addInitScript(bridgeShim, { key: apiKey, window: "main" });
        await page2.goto(`${base2}/decks/`, { waitUntil: "load" });
        await page2.waitForSelector("table.list tbody tr", { timeout: 15_000 });
        const downloadsBefore = syncLog().split("/sync/download").length - 1;
        await page2.locator("#sync").click();
        // 第二个客户端的集合是新建的、服务器有数据：上游会先 `syncStatus`，需要整份同步时
        // 弹「上传/下载」。两种结果都接受 —— 弹框就点「下载」，不弹就说明后端直接判定了下载。
        const downloadDialog = await page2
            .waitForSelector("#sync-download", { state: "visible", timeout: 30_000 })
            .catch(() => null);
        if (downloadDialog) {
            check("第二个客户端弹出了「上传/下载」选择", true);
            await page2.locator("#sync-download").click();
        } else {
            check(
                "第二个客户端没有弹框（后端直接判定为下载）",
                true,
                await page2.locator("#sync-status").innerText().catch(() => "（没有状态文字）"),
            );
        }
        // 下载方向**不能**靠服务器日志断言：`/sync/download` 是流式响应，服务器的日志层不会
        // 为它写 "finished" 行（实测只有 meta/hostKey/upload 有）。改用产物：第二个客户端的
        // 集合大小与服务器上那份**完全一致**（新建集合只有 4KB 左右），且里面有那张卡。
        check(
            "第二个客户端的集合变成了服务器上那份（大小一致）",
            await waitFor(
                "downloaded collection",
                () =>
                    fs.existsSync(collection2) &&
                    fs.existsSync(serverCollection) &&
                    fs.statSync(collection2).size === fs.statSync(serverCollection).size,
                60_000,
            ),
            `${collection2} vs ${serverCollection}`,
        );
        check(
            "第二个客户端真的拿到了第一个客户端的那张卡",
            await waitFor(
                "marker note",
                async () =>
                    SearchResponse.fromBinary(
                        await rpcOn(
                            base2,
                            "searchNotes",
                            new SearchRequest({ search: marker }).toBinary(),
                        ),
                    ).ids.length > 0,
                60_000,
            ),
            marker,
        );
    }
    // 5. **媒体同步**的成功路径：client1 加一个媒体文件 → 同步 → 服务器媒体夹里出现它，
    //    client2 再同步 → 它的媒体夹里也出现（`/msync/*` 一整条）
    const mediaName = "tauri-sync-check.txt";
    const storedName = GenericString.fromBinary(
        await rpc(
            "addMediaFile",
            new AddMediaFileRequest({
                desiredName: mediaName,
                data: new TextEncoder().encode(`tauri sync media ${Date.now()}`),
            }).toBinary(),
        ),
    ).val;
    check(
        "第一个客户端加了媒体文件（返回真实存下的名字）",
        typeof storedName === "string" && storedName.length > 0,
        storedName,
    );
    // 再点一次同步：这次集合同步没有变化，媒体同步会把新文件推上去
    await page.locator("#sync").click();
    const client1Media = collection1 ? path.join(mediaDirOf(collection1), storedName) : "";
    check(
        "第一个客户端的媒体夹里有它",
        client1Media
            ? await waitFor("client1 media", () => fs.existsSync(client1Media), 30_000)
            : false,
        client1Media || "（没有 FIRST_COLLECTION）",
    );
    const serverMedia = path.join(syncBase, syncUser, "media", storedName);
    check(
        "服务器媒体夹里出现了这个文件",
        await waitFor("server media", () => fs.existsSync(serverMedia), 90_000),
        serverMedia,
    );
    check(
        "服务器日志里有 /msync/ 请求",
        /"\/msync\/[a-zA-Z]+"/.test(syncLog()),
        (syncLog().match(/"\/msync\/[a-zA-Z]+"/g) ?? []).slice(0, 4).join(" / "),
    );

    if (port2) {
        const client2Media = path.join(mediaDirOf(collection2), storedName);
        await page2.locator("#sync").click();
        check(
            "第二个客户端也拿到了这个媒体文件",
            await waitFor("client2 media", () => fs.existsSync(client2Media), 90_000),
            client2Media,
        );
    }
} catch (error) {
    check("同步检查", false, String(error));
} finally {
    if (browser) {
        await browser.close();
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
