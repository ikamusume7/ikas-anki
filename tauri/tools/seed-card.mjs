// 通过外壳的 HTTP API 往测试集合里写入一张 Basic 卡片。
//
// 用途：让复习界面的视觉验证有真实内容可渲染（新建集合默认没有卡片）。
// 只应指向隔离的测试集合（out/tauri-test/…），不要指向真实 profile。
//
// note type 与牌组的 id 由接口查询得到，不硬编码 —— 新建集合里的内置 id
// 并不一定是 1（实测 addNote 会报 "missing note type"）。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/seed-card.mjs

import { GetDeckNamesRequest, DeckNames } from "../../out/ts/lib/generated/anki/decks_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { AddNoteRequest, AddNoteResponse } from "../../out/ts/lib/generated/anki/notes_pb.js";
import { NotetypeNames } from "../../out/ts/lib/generated/anki/notetypes_pb.js";

// 可用命令行参数覆盖字段内容与 note type，便于验证特定场景：
//   node tauri/tools/seed-card.mjs "<front>" "<back>"
//   node tauri/tools/seed-card.mjs "<front>" "<back>" "问答题（输入答案）"
const [frontArg, backArg, notetypeArg] = process.argv.slice(2);

const port = process.env.ANKI_API_PORT;
const key = process.env.ANKI_API_KEY;
if (!port || !key) {
    console.error("需要设置 ANKI_API_PORT 与 ANKI_API_KEY");
    process.exit(2);
}

async function call(method, bytes) {
    const response = await fetch(`http://127.0.0.1:${port}/_anki/${method}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/binary",
            Authorization: `Bearer ${key}`,
        },
        body: bytes,
    });
    const body = new Uint8Array(await response.arrayBuffer());
    if (!response.ok) {
        throw new Error(`${method} -> HTTP ${response.status}: ${new TextDecoder().decode(body)}`);
    }
    return body;
}

function requireFirst(entries, what) {
    if (entries.length === 0) {
        throw new Error(`集合里没有可用的${what}`);
    }
    return entries[0];
}

const notetypes = NotetypeNames.fromBinary(await call("getNotetypeNames", new Empty().toBinary()));
const decks = DeckNames.fromBinary(
    await call("getDeckNames", new GetDeckNamesRequest({ includeFiltered: false }).toBinary()),
);

// 内置 note type 的名字会在集合创建时按界面语言写进库，因此不能只匹配 "Basic"：
// zh-CN 下 Basic = 「问答题」、Cloze = 「填空题」、Image Occlusion = 「图片遮盖」。
// 必须精确匹配，否则会命中「问答题（附翻转卡片）」这类前缀相同的名字。
const basic = notetypes.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name));
// 第三个参数可以指定 note type 的（本地化）名字，例如「问答题（输入答案）」
const chosen = notetypeArg
    ? notetypes.entries.find((entry) => entry.name === notetypeArg)
    : undefined;
if (notetypeArg && !chosen) {
    throw new Error(`找不到 note type: ${notetypeArg}`);
}
const notetype = chosen ?? basic ?? requireFirst(notetypes.entries, "note type");
const deck = decks.entries.find((entry) => /^(Default|系统默认)$/i.test(entry.name)) ??
    requireFirst(decks.entries, "牌组");
console.log(
    `集合中的 note type: ${notetypes.entries.map((e) => `${e.name}(${e.id})`).join(", ")}`,
);
console.log(`使用 note type: ${notetype.name}(${notetype.id}) / 牌组: ${deck.name}(${deck.id})`);

const fields = [
    frontArg ??
        "<b>前端渲染验证：问题面</b><br>这张卡片由 seed-card.mjs 写入。",
    backArg ?? "这是答案面。<i>用于确认卡片渲染与样式注入正常。</i>",
];

const request = new AddNoteRequest({
    note: {
        notetypeId: notetype.id,
        fields,
    },
    deckId: deck.id,
});

const result = AddNoteResponse.fromBinary(await call("addNote", request.toBinary()));
console.log(`已添加笔记 noteId=${result.noteId}`);
