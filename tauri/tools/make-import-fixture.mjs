// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 造一个真的 `.apkg` 夹具（临时牌组 + 一张卡 → `exportAnkiPackage` → 删掉临时牌组），
// 供"命令行带文件启动"的检查用（`verify-cli-open.ps1` 第一阶段）。
//
// 与 `import-check.mjs` 里那段夹具逻辑同一套（字段数/note type 的选择都有原因，见那边的注释），
// 这里只留造夹具需要的部分。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_IMPORT_FIXTURE="<abs>/out/cli-open/import-fixture.apkg"
//   node tauri/tools/make-import-fixture.mjs
//
// 输出最后一行是 `MARKER=<牌组名>`，检查脚本据此搜索。

import fs from "node:fs";
import path from "node:path";

import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { ExportAnkiPackageRequest } from "../../out/ts/lib/generated/anki/import_export_pb.js";
import { Deck, DeckIds } from "../../out/ts/lib/generated/anki/decks_pb.js";
import { OpChangesWithId } from "../../out/ts/lib/generated/anki/collection_pb.js";
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

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const fixture = path.resolve(process.env.ANKI_IMPORT_FIXTURE ?? "out/cli-open/import-fixture.apkg");
const base = `http://127.0.0.1:${port}`;

async function rpc(method, body) {
    const response = await fetch(`${base}/_anki/${method}`, {
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

const deckName = `TauriCliOpen${Date.now() % 100_000}`;
const notetypeNames = NotetypeNames.fromBinary(
    await rpc("getNotetypeNames", new Empty({}).toBinary()),
);
const pick =
    notetypeNames.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ??
    notetypeNames.entries[0];
const notetype = Notetype.fromBinary(
    await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
);
const noteFields = notetype.fields.map((_, index) => `${deckName} 字段${index + 1}`);

const deckId = BigInt(
    OpChangesWithId.fromBinary(
        await rpc(
            "addDeck",
            new Deck({ name: deckName, kind: { case: "normal", value: {} } }).toBinary(),
        ),
    ).id,
);
await rpc(
    "addNote",
    new AddNoteRequest({
        note: new Note({ notetypeId: notetype.id, fields: noteFields }),
        deckId,
    }).toBinary(),
);

fs.mkdirSync(path.dirname(fixture), { recursive: true });
fs.rmSync(fixture, { force: true });
await rpc(
    "exportAnkiPackage",
    new ExportAnkiPackageRequest({
        outPath: fixture,
        options: {
            withScheduling: true,
            // 测试集合里 `addDeck` 建出来的牌组 `config_id` 是 0，带上会报
            // "No such deck config: '0'"（真实 profile 没这问题）
            withDeckConfigs: false,
            withMedia: false,
        },
    }).toBinary(),
);
// 夹具要"导进来才算真的"：临时牌组删掉（note 随牌组一起没）
await rpc("removeDecks", new DeckIds({ dids: [deckId] }).toBinary());

console.log(`已生成夹具: ${fixture}（${fs.statSync(fixture).size} 字节）`);
console.log(`MARKER=${deckName}`);
