// 清掉检查脚本留下的临时牌组与临时笔记（正常结束时会自己删，这个处理**中途失败**
// 留下的，以及少数"故意留到宿主断言之后"的，例如 `browser-check.mjs` 给预览翻页
// 造的那张双卡片笔记）。
//
// 用法（外壳需已启动，端口/密钥同其它脚本）：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/cleanup-test-decks.mjs

import {
    DeckIds,
    DeckNames,
    GetDeckNamesRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";
import { RemoveNotesRequest } from "../../out/ts/lib/generated/anki/notes_pb.js";
import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;

// 各检查脚本用的临时牌组名前缀：牌组列表（decks-check）、浏览器（browser-check）等
const prefixes = [
    "TauriTemp",
    "TauriDecks",
    "TauriCreated",
    "TauriRenamed",
    "TauriImport",
    "TauriFiltered",
    // 答完队列的收尾检查（`congrats-check.mjs`）造的「今天学完」牌组
    "TauriCongrats",
];

/**
 * 临时笔记的**内容**标记（搜索词，按 `searchNotes` 的语法）：
 *
 * 各脚本都往测试集合里写临时笔记，正常路径会自己删；失败时（或像预览翻页那样要留到
 * 宿主断言之后）就留下来了。这里按内容一次收掉。
 * 注意不要用泛化的 `Tauri` —— 验证种子卡片里也有这个词（`TauriReversed 正面`）。
 */
const noteMarkers = [
    "TauriPrev",
    "TauriDel",
    "TauriFr",
    "TauriNt",
    "TauriExp",
    "TauriProbe",
    "TauriBrowser",
    "TauriDecks 补的卡片",
    "TauriFiltered",
    // `congrats-check.mjs` 造的那张（卡片正面文本）
    "TauriCongrats",
    // `timebox-check.mjs` 造的那张
    "TauriTimebox",
];

const noteIds = async () => {
    const ids = [];
    for (const marker of noteMarkers) {
        const found = SearchResponse.fromBinary(
            await rpc("searchNotes", new SearchRequest({ search: marker }).toBinary()),
        ).ids;
        ids.push(...found);
    }
    return [...new Set(ids.map(String))].map(BigInt);
};

const stray = await noteIds();
if (stray.length > 0) {
    await rpc("removeNotes", new RemoveNotesRequest({ noteIds: stray }).toBinary());
    console.log(`清理了 ${stray.length} 张临时笔记`);
}

async function rpc(method, body) {
    const response = await fetch(`${base}/_anki/${method}`, {
        method: "POST",
        headers: { "Content-Type": "application/binary", Authorization: `Bearer ${apiKey}` },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

const names = DeckNames.fromBinary(
    await rpc(
        "getDeckNames",
        new GetDeckNamesRequest({ skipEmptyDefault: false }).toBinary(),
    ),
);
const entries = names.entries.map((entry) => ({ id: entry.id, name: entry.name }));

const targets = entries.filter((entry) => prefixes.some((p) => entry.name.includes(p)));
if (targets.length === 0) {
    console.log("没有需要清理的临时牌组");
    process.exit(0);
}

for (const entry of targets) {
    console.log(`删除 ${entry.name}（${entry.id}）`);
}
// 一次删掉（`removeDecks` 会连带删子牌组）
await rpc("removeDecks", new DeckIds({ dids: targets.map((it) => it.id) }).toBinary());
console.log(`已清理 ${targets.length} 个临时牌组`);
