// 清掉检查脚本留下的临时牌组（正常结束时会自己删，这个只处理**中途失败**留下的）。
//
// 用法（外壳需已启动，端口/密钥同其它脚本）：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/cleanup-test-decks.mjs

import {
    DeckIds,
    DeckNames,
    GetDeckNamesRequest,
} from "../../out/ts/lib/generated/anki/decks_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const base = `http://127.0.0.1:${port}`;

// 各检查脚本用的临时牌组名前缀：牌组列表（decks-check）、浏览器（browser-check）
const prefixes = ["TauriTemp", "TauriDecks", "TauriCreated", "TauriRenamed"];

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
