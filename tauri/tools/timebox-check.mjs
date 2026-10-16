// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 时间盒（PARITY 2.6）的端到端检查。对照：
//
//   - `pylib/anki/collection.py:1239-1251` 的 `startTimebox()` / `timeboxReached()`：
//     `conf["timeLim"]` 为 0 = 关闭，否则「已用时间 > timeLim」时返回
//     `(timeLim, sched.reps - _startReps)`；
//   - `qt/aqt/reviewer.py:973-992` 的 `check_timebox()`：弹「继续 / 结束」，
//     选「结束」回牌组列表、选「继续」重新计时。
//
// 为了不用真等一分钟，这里把偏好设置的「时间盒」改成 **1 秒**
// （`reviewing.timeLimitSecs` 就是秒），答一张卡之后必然到点；对话框用宿主的测试钩子
// `ANKI_TIMEBOX_CHOICE=continue|finish` 跳过（与 `ANKI_CONFIRM_YES` 同一套路），
// 断言两种选择的**可观察结果**：
//
//   - finish：宿主回牌组列表（`主窗口导航到: /decks/`，不带 `?deck=`），不再渲染下一张；
//   - continue：宿主重新计时并继续复习（出现新的「渲染完成」）。
//
// 卡片与偏好设置都是自己造、自己还原，所以两种模式可以在同一个集合上先后跑。
//
// 前置：外壳已启动（`ANKI_TIMEBOX_CHOICE` 决定模式）+ `ANKI_LOG` 指向外壳日志。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/timebox-check/run.log"; $env:ANKI_TIMEBOX_CHOICE="finish"
//   node tauri/tools/timebox-check.mjs

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

import { Preferences } from "../../out/ts/lib/generated/anki/config_pb.js";
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
import { Card, CardId } from "../../out/ts/lib/generated/anki/cards_pb.js";
import {
    GetQueuedCardsRequest,
    QueuedCards,
} from "../../out/ts/lib/generated/anki/scheduler_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
const mode = (process.env.ANKI_TIMEBOX_CHOICE ?? "finish").toLowerCase();
const base = `http://127.0.0.1:${port}`;
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

function readLog() {
    try {
        return fs.readFileSync(logPath, "utf8");
    } catch {
        return "";
    }
}

const countLog = (pattern) => readLog().split(pattern).length - 1;

/** 等日志里某个片段出现次数超过 `minimum`（先取 baseline 再发命令，避免竞态）。 */
async function waitLogAbove(pattern, minimum, timeoutMs = 20000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        if (countLog(pattern) > minimum) {
            return true;
        }
        await sleep(250);
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

const getPreferences = () =>
    rpc("getPreferences", new Empty({}).toBinary()).then((bytes) =>
        Preferences.fromBinary(bytes),
    );

const setPreferences = (prefs) => rpc("setPreferences", prefs.toBinary());

const topCard = () =>
    rpc("getQueuedCards", new GetQueuedCardsRequest({ fetchLimit: 1 }).toBinary()).then(
        (bytes) => QueuedCards.fromBinary(bytes).cards[0]?.card ?? null,
    );

const cardReps = (cardId) =>
    rpc("getCard", new CardId({ cid: cardId }).toBinary()).then(
        (bytes) => Card.fromBinary(bytes).reps,
    );

const removeNotes = (noteId) =>
    rpc("removeNotes", new RemoveNotesRequest({ noteIds: [noteId], cardIds: [] }).toBinary());

/** 改时间盒上限（秒）。
 *
 * 注意要**就地改**那个嵌套消息：`prefs.reviewing = { ...prefs.reviewing }` 会把嵌套消息
 * 换成普通对象，protobuf-es 序列化时会报 `message.toBinary is not a function`
 * （`Preferences` 是 protobuf-es 生成的不可互换结构）。
 */
async function setTimeLimit(secs) {
    const prefs = await getPreferences();
    if (!prefs.reviewing) {
        throw new Error("getPreferences 没有返回 reviewing 子消息");
    }
    prefs.reviewing.timeLimitSecs = secs;
    await setPreferences(prefs);
}

/** 留一张真实窗口的截图（`SHOT_DIR` 由 `verify-timebox.ps1` 传进来）。 */
function shot(name) {
    if (!process.env.SHOT_DIR) {
        return;
    }
    const result = spawnSync(
        "pwsh",
        [
            "-File",
            "tauri/tools/capture-window.ps1",
            "-ProcessName",
            "anki-tauri",
            "-OutFile",
            path.join(process.env.SHOT_DIR, name),
            "-WaitSeconds",
            "20",
            "-TitleLike",
            "Anki",
        ],
        { stdio: "inherit" },
    );
    check(`截到了真实窗口（${name}）`, result.status === 0, `capture-window exit=${result.status}`);
}

// ---------------------------------------------------------------------------
// 准备：把时间盒设成 1 秒、往当前牌组塞一张卡
// ---------------------------------------------------------------------------
const stamp = Date.now() % 100_000;
let noteId = 0n;
let originalTimeLimit = 0;
let originalIntervals = false;

try {
    if (!logPath) {
        throw new Error("缺少 ANKI_LOG");
    }

    const before = await getPreferences();
    originalTimeLimit = before.reviewing?.timeLimitSecs ?? 0;
    originalIntervals = before.reviewing?.showIntervalsOnButtons ?? false;
    await setTimeLimit(1);
    const after = await getPreferences();
    check(
        "时间盒已设成 1 秒",
        after.reviewing?.timeLimitSecs === 1,
        `原来是 ${originalTimeLimit} 秒`,
    );
    check(
        "改时间盒没有把别的复习偏好写坏",
        after.reviewing?.showIntervalsOnButtons === originalIntervals,
        `showIntervalsOnButtons=${after.reviewing?.showIntervalsOnButtons}`,
    );

    const names = NotetypeNames.fromBinary(await rpc("getNotetypeNames", new Empty({}).toBinary()));
    const pick =
        names.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ?? names.entries[0];
    const notetype = Notetype.fromBinary(
        await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
    );
    const added = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: new Note({
                    notetypeId: notetype.id,
                    fields: notetype.fields.map((field, index) =>
                        index === 0 ? `TauriTimebox ${stamp}` : field.name,
                    ),
                }),
            }).toBinary(),
        ),
    );
    // 注意响应的字段是 `noteId`（不是 `note`；`addNote` 返回 `AddNoteResponse{noteId, changes}`）
    noteId = added.noteId;

    // 开始复习：`study` 会重新开始计时（对照 `main.py:1242`）
    const renders = countLog("渲染完成");
    await bridge("study");
    check("发 study 后进入复习页并渲染", await waitLogAbove("渲染完成", renders), logPath);

    const card = await topCard();
    check("队列里有卡片", !!card, card ? `card=${card.id}` : "（队列为空）");
    if (!card) {
        throw new Error("队列为空，无法测时间盒");
    }
    const repsBefore = await cardReps(card.id);

    // 显示答案 → 评分（1 秒上限，答完必然到点）
    const buttons = countLog("评分按钮");
    await bridge("ans");
    check("显示答案后画出了评分按钮", await waitLogAbove("评分按钮", buttons), logPath);

    // 上限只有 1 秒，但上面这几步（取卡、RPC、等日志）可能不到 1 秒就跑完了 ——
    // 时间盒是「已用时间 > 上限」，所以显式等过去，让触发点确定
    await sleep(1500);

    const rendersBeforeAnswer = countLog("渲染完成");
    const boxLogs = countLog("时间盒到点: ");
    const finishLogs = countLog("时间盒到点：结束（回牌组列表）");
    const continueLogs = countLog("时间盒到点：继续（重新计时）");
    await bridge("ease3");

    check("时间盒到点被触发", await waitLogAbove("时间盒到点: ", boxLogs), logPath);
    const repsAfter = await cardReps(card.id);
    check("评分本身仍然落库", repsAfter === repsBefore + 1, `reps ${repsBefore} → ${repsAfter}`);

    if (mode === "finish") {
        check(
            "选「结束」→ 宿主记了结束",
            await waitLogAbove("时间盒到点：结束（回牌组列表）", finishLogs, 15000),
            logPath,
        );
        check("选「结束」时没有走「继续」分支", countLog("时间盒到点：继续（重新计时）") === continueLogs);
        // 结束后不该再渲染下一张（对照 `check_timebox()` 返回 True 后不再 `nextCard()`）
        await sleep(1500);
        check(
            "结束后没有再渲染下一张",
            countLog("渲染完成") === rendersBeforeAnswer,
            `渲染次数 ${rendersBeforeAnswer} → ${countLog("渲染完成")}`,
        );
        const navigations = readLog().match(/主窗口导航到: (\S+)/g) ?? [];
        const last = navigations.at(-1) ?? "";
        check(
            "最后一条导航是牌组列表（不是概览）",
            /\/decks\/$/.test(last),
            last || "（没有导航日志）",
        );
        shot("timebox-finish.png");
    } else {
        check(
            "选「继续」→ 宿主重新计时",
            await waitLogAbove("时间盒到点：继续（重新计时）", continueLogs, 15000),
            logPath,
        );
        check("选「继续」时没有走「结束」分支", countLog("时间盒到点：结束（回牌组列表）") === finishLogs);
        check(
            "继续之后渲染了下一张",
            await waitLogAbove("渲染完成", rendersBeforeAnswer, 15000),
            logPath,
        );
        shot("timebox-continue.png");
    }
} catch (error) {
    check("时间盒检查", false, String(error));
} finally {
    // 还原：偏好设置 + 临时笔记
    try {
        await setTimeLimit(originalTimeLimit);
        if (noteId) {
            await removeNotes(noteId);
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
