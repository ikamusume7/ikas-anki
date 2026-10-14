// 「导入」的端到端检查。
//
// 上半段（造夹具 + 页面链路）：
//   1. 建一个临时牌组 + 一张卡片 → 用 `exportAnkiPackage` 导出成 `.apkg`（写到
//      `ANKI_IMPORT_FIXTURE` 指定的路径）→ **把临时牌组删掉**（这样后面导进来才算真的）；
//   2. 用 Playwright 打开宿主会打开的那个地址
//      `<base>/import-anki-package/<百分号编码的路径>`（`import_dialog.py:68` 的 URL 形状，
//      路由是 catch-all `[...path]`），断言页面把路径解码出来了（顶栏显示文件名）
//      → 点「导入」→ 断言集合里真的长出那个牌组与卡片 → 页面发过 `importDone`。
//   3. 收尾：删掉临时牌组；**保留** .apkg 夹具 —— `verify-decks.ps1` 接着要用它断言
//      宿主那一半（「导入文件」按钮 → `ANKI_OPEN_PATH` 顶掉原生对话框 → 开这个窗口）。
//
// 宿主那一半（原生文件对话框 + 打开窗口 + URL 编码）由 `verify-decks.ps1` 的日志断言覆盖，
// 纯逻辑（按扩展名挑页面、拖入时的过滤）由 `tauri/src/importing.rs` 的 `#[cfg(test)]` 覆盖。
//
// 前置：外壳已启动；启动前设了 `ANKI_OPEN_PATH=<夹具路径>`（原生对话框没法自动点）
//       与 `ANKI_IMPORT_FIXTURE=<夹具路径>`。
//
// 用法：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_IMPORT_FIXTURE="<abs>/out/decks-check/import-fixture.apkg"
//   node tauri/tools/import-check.mjs

import fs from "node:fs";
import path from "node:path";

import { chromium } from "playwright";

import { OpChangesWithId } from "../../out/ts/lib/generated/anki/collection_pb.js";
import {
    Deck,
    DeckId,
    DeckIds,
    DeckNames,
    GetDeckNamesRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";
import { SetConfigJsonRequest } from "../../out/ts/lib/generated/anki/config_pb.js";
import {
    Empty,
    Json as GenericJson,
    String as GenericString,
} from "../../out/ts/lib/generated/anki/generic_pb.js";
import { ExportAnkiPackageRequest } from "../../out/ts/lib/generated/anki/import_export_pb.js";
import {
    AddNoteRequest,
    AddNoteResponse,
    Note,
    RemoveNotesRequest,
} from "../../out/ts/lib/generated/anki/notes_pb.js";
import { Notetype, NotetypeId, NotetypeNames } from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";

const port = process.env.ANKI_API_PORT ?? "40031";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;
const fixture = path.resolve(
    process.env.ANKI_IMPORT_FIXTURE ?? "out/decks-check/import-fixture.apkg",
);
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

/// 与 `tauri/src/importing.rs::encode_path` 一致：只保留 URL 安全字符（`quote()` 的默认
/// safe 是 `/`，`+` `:` `\` 都要编码）。
function encodePath(value) {
    return [...Buffer.from(value, "utf8")]
        .map((byte) => {
            const char = String.fromCharCode(byte);
            return /[A-Za-z0-9\-_.~\/]/.test(char)
                ? char
                : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
        })
        .join("");
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
    window.bridgeCommand = window.pycmd = () => false;
}

/// 读一个原始配置项（`getConfigJson` 收键名、回 JSON 文本）。
async function configValue(key) {
    const json = GenericJson.fromBinary(
        await rpc("getConfigJson", new GenericString({ val: key }).toBinary()),
    ).json;
    return JSON.parse(new TextDecoder().decode(json));
}

async function setConfig(key, value) {
    await rpc(
        "setConfigJson",
        new SetConfigJsonRequest({
            key,
            valueJson: new TextEncoder().encode(JSON.stringify(value ?? null)),
            undoable: false,
        }).toBinary(),
    );
}

async function waitFor(probe, timeoutMs = 20_000) {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        if (await probe()) {
            return true;
        }
        if (Date.now() > deadline) {
            return false;
        }
        await new Promise((resolve) => setTimeout(resolve, 250));
    }
}

const deckNames = async () =>
    DeckNames.fromBinary(
        await rpc(
            "getDeckNames",
            new GetDeckNamesRequest({ skipEmptyDefault: false, includeFiltered: false }).toBinary(),
        ),
    ).entries.map((entry) => entry.name);

const searchNotes = async (search) =>
    SearchResponse.fromBinary(
        await rpc("searchNotes", new SearchRequest({ search }).toBinary()),
    ).ids;

// ##########################################################################
// 1. 造夹具：临时牌组 + 一张卡片 → 导出 .apkg → 删掉临时牌组
// ##########################################################################

const deckName = `TauriImport${Date.now() % 100_000}`;
const notetypeNames = NotetypeNames.fromBinary(
    await rpc("getNotetypeNames", new Empty({}).toBinary()),
);
if (!notetypeNames.entries[0]) {
    console.error("集合里没有笔记类型");
    process.exit(2);
}
// 挑普通问答类型 + 按它的字段数造字段：
//  - `entries[0]` 在新建集合里是「遮盖」/Image Occlusion，5 个字段 —— 少给字段会
//    「note has 2 fields, expected 5」；
//  - 而且遮挡笔记的模板要 `{{c1::}}`，空字段会让卡片变成「空卡片」，导出时连带
//    note 一起不翼而飞（夹具里没笔记，导回来自然搜不到）
const pick =
    notetypeNames.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ??
    notetypeNames.entries[0];
const notetype = Notetype.fromBinary(
    await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
);
const noteFields = notetype.fields.map((_, index) => `${deckName} 字段${index + 1}`);


// 导入会把**当前牌组**改成导进来的那个（实测），收尾时必须还原 —— 后面的检查
// （decks-check 的「当前牌组有高亮」、自定义学习的 `deck:current`）都看它
const originalDeck = Deck.fromBinary(
    await rpc("getCurrentDeck", new Empty({}).toBinary()),
).id;

const removeDeck = async (deckId) =>
    rpc("removeDecks", new DeckIds({ dids: [deckId] }).toBinary());

const deckId = BigInt(
    OpChangesWithId.fromBinary(
        await rpc(
            "addDeck",
            new Deck({ name: deckName, kind: { case: "normal", value: {} } }).toBinary(),
        ),
    ).id,
);
const noteId = BigInt(
    AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: new Note({ notetypeId: notetype.id, fields: noteFields }),
                deckId,
            }).toBinary(),
        ),
    ).noteId,
);

fs.mkdirSync(path.dirname(fixture), { recursive: true });
fs.rmSync(fixture, { force: true });
await rpc(
    "exportAnkiPackage",
    new ExportAnkiPackageRequest({
        outPath: fixture,
        // 与浏览器「导出 apkg」的选项同一套（`browser/+page.svelte` 的导出对话框）。
        // `withDeckConfigs` 关掉：测试集合里 `addDeck` 建出来的牌组 `config_id` 是 0，
        // 带牌组配置导出会报 "No such deck config: '0'"（真实 profile 里没这问题）
        options: {
            withScheduling: true,
            withDeckConfigs: false,
            withMedia: true,
            legacy: false,
        },
        // `limit` 是 `ExportLimit`，它自己**又有一层**同名 oneof，所以嵌两层
        // （`browser/+page.svelte:1153` 也是这么写的）。少一层不会报错，但会**静默导出
        // 整个集合** —— 于是导回来的 apkg 里连 Default 的卡片都在，把这个集合搅乱
        limit: { limit: { case: "deckId", value: deckId } },
    }).toBinary(),
);
check(
    "导出前那张卡片在集合里",
    (await searchNotes(`"${deckName} 字段1"`)).length === 1,
    `搜到 ${(await searchNotes(`"${deckName} 字段1"`)).length} 张`,
);
check(
    "导出了 .apkg 夹具",
    fs.existsSync(fixture) && fs.statSync(fixture).size > 0,
    `${fixture}（${fs.existsSync(fixture) ? fs.statSync(fixture).size : 0} 字节）`,
);

// 牌组与那张卡片都删掉：只删牌组的话 note 会留下（GUID 还在），导回来会被当成
// 「已有同样内容的笔记」跳过，牌组是空的 —— 第一次跑就是这么被判失败的
await rpc("removeNotes", new RemoveNotesRequest({ noteIds: [noteId] }).toBinary());
await removeDeck(deckId);
check("导入前临时牌组已删掉", !(await deckNames()).includes(deckName), (await deckNames()).join(" / "));
check(
    "导入前那张卡片也删掉了",
    (await searchNotes(`"${deckName} 字段1"`)).length === 0,
    `${(await searchNotes(deckName)).length} 张`,
);

// ##########################################################################
// 2. 页面链路：宿主会打开的那个地址 → 点「导入」
// ##########################################################################

const browser = await chromium.launch();
let mergeBefore;
try {
    // 夹具是从**同一份集合**导出的：里面的笔记类型与集合里的是同一个（同 id）。
    // 新集合的默认配置 `mergeNotetypes=false` 时，这种 apkg 导入**不会把笔记带回来**
    // （上游行为，`apkg/import/notes.rs:144` 那条分支；页面还传了 `alertOnError: false`，
    // 所以看起来"没报错却什么都没导进来"）。像用户在导入对话框里勾上「合并笔记类型」
    // 那样开一下，导入完再还原。
    mergeBefore = await configValue("mergeNotetypes");
    await setConfig("mergeNotetypes", true);
    console.log(`mergeNotetypes: ${mergeBefore ?? "(未设置)"} → true（导入完还原）`);

    const page = await browser.newPage({ viewport: { width: 900, height: 700 } });
    const pageErrors = [];
    const requests = [];
    page.on("pageerror", (error) => pageErrors.push(String(error)));
    page.on("request", (request) => requests.push(request.url()));
    await page.addInitScript(bridgeShim, { key: apiKey, window: "import" });
    await page.goto(`${base}/import-anki-package/${encodePath(fixture)}`, { waitUntil: "load" });

    await page.waitForSelector(".filename", { timeout: 20_000 });
    check(
        "路径经 URL 段传进页面后解码正确（顶栏显示文件名）",
        (await page.locator(".filename").innerText()).includes(path.basename(fixture)),
        await page.locator(".filename").innerText(),
    );

    await page.locator("button:has(.import)").click();
    // 等**卡片**出现，不要只等牌组：牌组先建出来、note 晚一步入库，先看到牌组就断言
    // 会读到「0 张」（第一次就是被这个竞态判失败的）
    const imported = await waitFor(
        async () => (await searchNotes(`"${deckName} 字段1"`)).length === 1,
        30_000,
    );
    check(
        "点「导入」后牌组与卡片都导进了集合",
        imported && (await deckNames()).includes(deckName),
        (await deckNames()).join(" / "),
    );

    // 只认「那张 note 回来了」，**不**认它在哪个牌组：这份测试集合里有个英文名的
    // `Default`（外加历次检查留下的牌组），导回来的卡片会被映射到 Default；换个干净的
    // 集合导入同一个 apkg，卡片就落在 `TauriImport…` 牌组里（实测过）—— 属于测试集合的
    // 历史包袱，与本外壳无关
    // 同样要轮询：导入落库是分步的，中间会有「搜不到」的瞬间（只查一次就会误判）
    const cameBack = await waitFor(
        async () => (await searchNotes(`"${deckName} 字段1"`)).length === 1,
        20_000,
    );
    check(
        "导入的卡片也回来了",
        cameBack,
        `搜到 ${(await searchNotes(`"${deckName} 字段1"`)).length} 张`,
    );

    check(
        "页面发过 importDone（宿主据此解除模态）",
        await waitFor(
            async () => requests.some((url) => url.endsWith("/_anki/importDone")),
            10_000,
        ),
        requests.filter((url) => url.includes("import")).join(" / "),
    );
    check("页面没有报错", pageErrors.length === 0, pageErrors.join(" / "));

    await page.screenshot({ path: path.join(path.dirname(fixture), "import.png") });
} catch (error) {
    check("导入页面流程", false, String(error));
} finally {
    await browser.close();
}

// ##########################################################################
// 3. 收尾：删掉临时牌组（.apkg 夹具留着，宿主那一半还要用）
// ##########################################################################

try {
    // 先删导回来的那张 note（它可能落在 Default 里，删牌组删不掉它），再按**名字**删牌组
    // —— 导进来的是新牌组（新 id），按最初那个 id 删会留下空壳
    const stray = await searchNotes(`"${deckName} 字段1"`);
    if (stray.length > 0) {
        await rpc("removeNotes", new RemoveNotesRequest({ noteIds: stray }).toBinary());
    }
    const left = DeckNames.fromBinary(
        await rpc(
            "getDeckNames",
            new GetDeckNamesRequest({ skipEmptyDefault: false, includeFiltered: true }).toBinary(),
        ),
    ).entries.filter((entry) => entry.name === deckName);
    for (const entry of left) {
        await removeDeck(entry.id);
    }
    // 当前牌组也还原（导入会把它改成导进来的那个）
    await rpc("setCurrentDeck", new DeckId({ did: originalDeck }).toBinary());
    // 配置也还原（导入前临时打开的「合并笔记类型」）
    if (typeof mergeBefore !== "undefined") {
        await setConfig("mergeNotetypes", mergeBefore ?? false);
    }
    const current = Deck.fromBinary(await rpc("getCurrentDeck", new Empty({}).toBinary())).id;
    check(
        "已清理临时牌组与卡片，当前牌组与配置已还原",
        !(await deckNames()).includes(deckName) &&
            (await searchNotes(`"${deckName} 字段1"`)).length === 0 &&
            current === originalDeck &&
            (await configValue("mergeNotetypes")) === (mergeBefore ?? false),
        `${(await deckNames()).join(" / ")}（当前 ${current}，原 ${originalDeck}）`,
    );
} catch (problem) {
    check("清理临时牌组", false, String(problem));
}

console.log(
    failures.length === 0
        ? "导入检查：全部通过"
        : `导入检查：${failures.length} 项失败\n- ${failures.join("\n- ")}`,
);
process.exit(failures.length === 0 ? 0 : 1);
