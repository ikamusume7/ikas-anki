// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 音频显示与语义（PARITY 2.3）的端到端检查。对照：
//
//   - `pylib/anki/sound.py:69` 的 `AV_REF_RE = re.compile(r"\[anki:(play:(.):(\d+))\]")`
//     与 `:900-921` 的 `av_refs_to_play_icons()`：渲染结果里的音频标记要换成播放按钮，
//     点了发 `play:q:N` / `play:a:N`；
//   - `qt/aqt/sound.py:924-937` 的 `play_clicked_audio`：按面与序号播放**那一个**标签；
//   - `reviewer.py:1043-1044` 的 `replayAudio`：重播当前这一面；
//   - 答案面不该把问题面的音频**再播一遍**（Qt 的 `answer_av_tags()` 是按答案面算的）。
//
// 做法：往 `collection.media` 写一个能播放的真 WAV，再写一张字段里带
// `[sound:...]` 的卡片 → `study` → 断言问题面注入了播放按钮并自动播放 →
// `ans` → 断言答案面**跳过**了问题面那份 → `play:a:0` / `rev:replayAudio` → 断言各自的播放日志。
//
// 前置：外壳已启动 + `ANKI_LOG` 指向外壳日志 + 知道集合目录（`ANKI_COLLECTION`）。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/reviewer-check/run.log"; $env:ANKI_COLLECTION="out/tauri-test/collection.anki2"
//   node tauri/tools/audio-check.mjs

import fs from "node:fs";
import path from "node:path";

import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";
import {
    AddNoteRequest,
    AddNoteResponse,
    Note,
    RemoveNotesRequest,
} from "../../out/ts/lib/generated/anki/notes_pb.js";
import {
    Notetype,
    NotetypeId,
    NotetypeNames,
} from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import { Deck } from "../../out/ts/lib/generated/anki/decks_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
const collection = process.env.ANKI_COLLECTION ?? "";
const base = `http://127.0.0.1:${port}`;
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function readLog() {
    try {
        return fs.readFileSync(logPath, "utf8");
    } catch {
        return "";
    }
}

const countLog = (pattern) => readLog().split(pattern).length - 1;

async function waitLogAbove(pattern, minimum, timeoutMs = 15_000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        if (countLog(pattern) > minimum) {
            return true;
        }
        await sleep(200);
    }
    return false;
}

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

async function bridge(command, window = "main") {
    const response = await fetch(`${base}/_anki/pycmd`, {
        method: "POST",
        headers: {
            "Content-Type": "text/plain",
            Authorization: `Bearer ${apiKey}`,
            "X-Anki-Window": window,
            Connection: "close",
        },
        body: command,
    });
    return response.status;
}

/** 写一个 0.2 秒的 8kHz 单声道 8bit WAV（rodio 能解码，文件也很小）。 */
function writeTestWav(file) {
    const sampleRate = 8000;
    const samples = Math.floor(sampleRate * 0.2);
    const data = Buffer.alloc(samples, 128);
    const header = Buffer.alloc(44);
    header.write("RIFF", 0);
    header.writeUInt32LE(36 + data.length, 4);
    header.write("WAVE", 8);
    header.write("fmt ", 12);
    header.writeUInt32LE(16, 16);
    header.writeUInt16LE(1, 20); // PCM
    header.writeUInt16LE(1, 22); // 单声道
    header.writeUInt32LE(sampleRate, 24);
    header.writeUInt32LE(sampleRate, 28);
    header.writeUInt16LE(1, 32);
    header.writeUInt16LE(8, 34);
    header.write("data", 36);
    header.writeUInt32LE(data.length, 40);
    fs.writeFileSync(file, Buffer.concat([header, data]));
}

const mediaName = "tauri-audio-check.wav";
let noteId = 0n;
let mediaFile = "";

try {
    if (!logPath) {
        throw new Error("缺少 ANKI_LOG");
    }
    if (!collection) {
        throw new Error("缺少 ANKI_COLLECTION（要往集合的 media 目录写夹具）");
    }
    mediaFile = path.join(path.dirname(collection), "collection.media", mediaName);
    fs.mkdirSync(path.dirname(mediaFile), { recursive: true });
    writeTestWav(mediaFile);
    check("测试 WAV 已写入媒体夹", fs.existsSync(mediaFile), mediaFile);

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
    // 正面只放音频（自动播放 + 播放按钮），背面放别的音频（答案面自己的那份）
    noteId = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: new Note({
                    notetypeId: notetype.id,
                    fields: [
                        `[sound:${mediaName}]`,
                        `背面 [sound:${mediaName}]`,
                    ],
                }),
                deckId,
            }).toBinary(),
        ),
    ).noteId;

    const renders = countLog("渲染完成");
    await bridge("study");
    await sleep(800);
    await bridge("domDone");
    check("复习页渲染了这张卡", await waitLogAbove("渲染完成", renders), logPath);

    // 1. 显示层：问题面注入了播放按钮（标记 → 图标由宿主做，`av_refs_to_play_icons`）
    check(
        "问题面注入了播放按钮",
        readLog().includes("问题面注入了 1 个播放按钮"),
        (readLog().match(/问题面注入了[^\n]*/) ?? ["（没有日志）"]).at(-1),
    );
    // 2. 自动播放：问题面的 WAV 真的进了播放队列
    check("问题面自动播放了那个 WAV", readLog().includes(mediaName));

    // 3. 答案面：**不再**把问题面的音频再播一遍（前缀跳过）
    const answerSkips = countLog("答案面跳过");
    await bridge("ans");
    check("显示答案后有「答案面跳过」的日志", await waitLogAbove("答案面跳过", answerSkips), logPath);
    // 答案页本来就带 `{{FrontSide}}`，所以那 1 个播放按钮照样显示（Qt 也一样）——
    // 「不重复」指的是**自动播放**不再播问题面那份（上一行的「答案面跳过」）
    check(
        "答案面的播放按钮是问题面 + 背面两份（与 Qt 的答案页一致）",
        readLog().includes("答案面注入了 2 个播放按钮"),
        (readLog().match(/答案面注入了[^\n]*/) ?? ["（没有日志）"]).at(-1),
    );

    // 4. 点播放按钮：`play:a:0` 播答案面的第 0 个标签
    const clicked = countLog("播放点击的音频");
    await bridge("play:a:0");
    check("play:a:0 播了答案面的音频", await waitLogAbove("播放点击的音频", clicked), logPath);
    check(
        "播放的是第 0 个",
        readLog().includes("播放点击的音频: 答案面第 0 个"),
        (readLog().match(/播放点击的音频[^\n]*/) ?? ["（没有日志）"]).at(-1),
    );

    // 4b. 暂停 / ±5 秒（`5`/`6`/`7` 与菜单里的三项）：宿主把命令交到播放器
    const pauses = countLog("音频: 暂停/继续");
    await bridge("rev:pauseAudio");
    check("rev:pauseAudio 发了暂停/继续", await waitLogAbove("音频: 暂停/继续", pauses), logPath);
    const seeks = countLog("音频: 跳到");
    await bridge("rev:seekForward");
    await bridge("rev:seekBack");
    check("rev:seekForward/seekBack 发了 ±5 秒", await waitLogAbove("音频: 跳到", seeks + 1), logPath);
    check(
        "日志里带正负偏移",
        readLog().includes("音频: 跳到 +5 秒") && readLog().includes("音频: 跳到 -5 秒"),
        (readLog().match(/音频: 跳到[^\n]*/g) ?? ["（没有日志）"]).slice(-2).join(" / "),
    );

    // 5. `r`/F5/菜单的「重播音频」：重播当前这一面
    const replays = countLog("重播音频（");
    await bridge("rev:replayAudio");
    check("rev:replayAudio 重播了音频", await waitLogAbove("重播音频（", replays), logPath);
    check(
        "重播的是答案面（当前显示的是答案）",
        readLog().includes("重播音频（答案面"),
        (readLog().match(/重播音频（[^\n]*/) ?? ["（没有日志）"]).at(-1),
    );
} catch (error) {
    check("音频检查", false, String(error));
} finally {
    try {
        if (noteId) {
            await rpc(
                "removeNotes",
                new RemoveNotesRequest({ noteIds: [noteId], cardIds: [] }).toBinary(),
            );
        }
        if (mediaFile && fs.existsSync(mediaFile)) {
            fs.rmSync(mediaFile, { force: true });
        }
    } catch (error) {
        console.log(`清理失败：${String(error)}`);
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
