// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 自动前进（PARITY 2.4）的端到端检查。对照 `qt/aqt/reviewer.py:408-443,489-527,1219-1231`：
//
//   - 开关是**运行时**的（`auto_advance_enabled`，每次构造 reviewer 都是 false），
//     只能靠菜单/`Shift+A` 打开；牌组配置里那五个字段只是参数
//     （`qt/aqt` 里根本没有读 `autoAdvance` 的地方）；
//   - 问题面：`secondsToShowQuestion` 秒后按 `questionAction` 走（显示答案 / 提示）；
//   - 答案面：`secondsToShowAnswer` 秒后按 `answerAction` 走（重来/困难/良好 / 提示 / 埋藏）；
//   - `waitForAudio` 时有音频在播就不动；**窗口没有焦点就自己关掉自动前进**。
//
// 为了不真等一分钟，这里把牌组配置改成 1 秒（新建/修改牌组配置走
// `getDeckConfigsForUpdate` + `updateDeckConfigs`，收尾原样写回）。
//
// 前置：外壳已启动 + `ANKI_LOG` 指向外壳日志。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/reviewer-check/run.log"
//   node tauri/tools/auto-advance-check.mjs

import fs from "node:fs";
import { spawnSync } from "node:child_process";

import {
    DeckConfigsForUpdate,
    UpdateDeckConfigsMode,
    UpdateDeckConfigsRequest,
    DeckConfig_Config_AnswerAction,
    DeckConfig_Config_QuestionAction,
} from "../../out/ts/lib/generated/anki/deck_config_pb.js";
import { Deck, DeckId } from "../../out/ts/lib/generated/anki/decks_pb.js";
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

import { SearchRequest, SearchResponse } from "../../out/ts/lib/generated/anki/search_pb.js";
import { Card, CardId } from "../../out/ts/lib/generated/anki/cards_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
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

async function waitLogAbove(pattern, minimum, timeoutMs = 20_000) {
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

const cardReps = (cardId) =>
    rpc("getCard", new CardId({ cid: cardId }).toBinary()).then(
        (bytes) => Card.fromBinary(bytes).reps,
    );

/** 把牌组配置里的自动前进参数写进去（`config` 直接改原对象）。 */
async function writeConfigs(deckId, configs) {
    await rpc(
        "updateDeckConfigs",
        new UpdateDeckConfigsRequest({
            targetDeckId: deckId,
            configs,
            removedConfigIds: [],
            mode: UpdateDeckConfigsMode.NORMAL,
        }).toBinary(),
    );
}

let noteId = 0n;
let deckId = 0n;
let originalConfig = null;
let forUpdate = null;
let originalValues = null;

try {
    if (!logPath) {
        throw new Error("缺少 ANKI_LOG");
    }

    // 自动前进在"窗口没有焦点"时会自己关掉，所以先把主窗口切到前台
    // （真实用户当然是有焦点的；自动化里终端才是前台）
    const focused = spawnSync(
        "pwsh",
        [
            "-File",
            "tauri/tools/focus-window.ps1",
            "-ProcessName",
            "anki-tauri",
            "-TitleLike",
            "Anki",
        ],
        { stdio: "inherit" },
    );
    check("主窗口已切到前台", focused.status === 0, `focus-window exit=${focused.status}`);

    deckId = Deck.fromBinary(
        await rpc("getCurrentDeck", new Empty({}).toBinary()),
    ).id;
    forUpdate = DeckConfigsForUpdate.fromBinary(
        await rpc("getDeckConfigsForUpdate", new DeckId({ did: deckId }).toBinary()),
    );
    const currentId = forUpdate.currentDeck?.configId ?? 0n;
    const configs = forUpdate.allConfig
        .map((entry) => entry.config)
        .filter((config) => config !== undefined);
    originalConfig = configs.find((config) => config.id === currentId) ?? null;
    check("取到了当前牌组的配置", originalConfig !== null, `deck=${deckId} config=${currentId}`);
    if (!originalConfig) {
        throw new Error("找不到当前牌组的配置");
    }

    // 造一张新卡（新卡今天到期），保证队列里有东西
    const names = NotetypeNames.fromBinary(
        await rpc("getNotetypeNames", new Empty({}).toBinary()),
    );
    const pick =
        names.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name)) ?? names.entries[0];
    const notetype = Notetype.fromBinary(
        await rpc("getNotetype", new NotetypeId({ ntid: pick.id }).toBinary()),
    );
    noteId = AddNoteResponse.fromBinary(
        await rpc(
            "addNote",
            new AddNoteRequest({
                note: new Note({
                    notetypeId: notetype.id,
                    fields: notetype.fields.map((field, index) =>
                        index === 0 ? `TauriAutoAdvance ${Date.now() % 100_000}` : field.name,
                    ),
                }),
                deckId,
            }).toBinary(),
        ),
    ).noteId;

    // 参数：问题面 1 秒后显示答案、答案面 1 秒后评「良好」
    originalValues = {
        secondsToShowQuestion: originalConfig.config.secondsToShowQuestion,
        secondsToShowAnswer: originalConfig.config.secondsToShowAnswer,
        questionAction: originalConfig.config.questionAction,
        answerAction: originalConfig.config.answerAction,
        waitForAudio: originalConfig.config.waitForAudio,
    };
    originalConfig.config.secondsToShowQuestion = 1;
    originalConfig.config.questionAction = DeckConfig_Config_QuestionAction.SHOW_ANSWER;
    originalConfig.config.secondsToShowAnswer = 1;
    originalConfig.config.answerAction = DeckConfig_Config_AnswerAction.ANSWER_GOOD;
    originalConfig.config.waitForAudio = false;
    await writeConfigs(deckId, configs);
    const readBack = DeckConfigsForUpdate.fromBinary(
        await rpc("getDeckConfigsForUpdate", new DeckId({ did: deckId }).toBinary()),
    );
    const saved = readBack.allConfig
        .map((entry) => entry.config)
        .find((config) => config?.id === currentId);
    check(
        "读回的配置诊断",
        true,
        `ids=[${readBack.allConfig.map((entry) => String(entry.config?.id)).join(",")}] 总数=${readBack.allConfig.length} want=${currentId} 当前牌组=${readBack.currentDeck?.name ?? "?"}`,
    );
    check(
        "牌组配置已改成 1 秒 / 显示答案 / 评「良好」",
        saved?.config?.secondsToShowQuestion === 1 &&
            saved?.config?.secondsToShowAnswer === 1 &&
            saved?.config?.answerAction === DeckConfig_Config_AnswerAction.ANSWER_GOOD,
        `q=${saved?.config?.secondsToShowQuestion}s a=${saved?.config?.secondsToShowAnswer}s action=${saved?.config?.answerAction}`,
    );

    // 开始复习（问题面）。开关默认是关的，所以什么都不该发生
    const renders = countLog("渲染完成");
    await bridge("study");
    // 主窗口可能已经停在复习页上（前面的检查留下的），那样 `study` 只是把同一个 URL
    // 再赋一次、不会重新加载，`domDone` 也就不会再来 —— 补一条 `domDone` 保证真的取队列
    await sleep(800);
    await bridge("domDone");
    check("发 study 后渲染了问题面", await waitLogAbove("渲染完成", renders), logPath);

    const buttonsBefore = countLog("评分按钮");
    await sleep(2500);
    check(
        "没开自动前进时，问题面不会自己翻到答案",
        countLog("评分按钮") === buttonsBefore,
        `评分按钮 ${buttonsBefore} → ${countLog("评分按钮")}`,
    );

    // 打开自动前进：1 秒后应该出现评分按钮（没有人按过「显示答案」）
    await bridge("rev:autoAdvance");
    check(
        "打开后问题面自动翻到答案（questionAction=SHOW_ANSWER）",
        await waitLogAbove("评分按钮", buttonsBefore, 8000),
        logPath,
    );
    check("宿主日志说是「到点显示答案」", readLog().includes("自动前进: 到点显示答案"));

    // 再 1 秒后应该自动评「良好」，然后渲染下一张
    // （卡片 id 要从 note 查：`addNote` 只回 noteId）
    const cardId = SearchResponse.fromBinary(
        await rpc(
            "searchCards",
            new SearchRequest({ search: `nid:${noteId}` }).toBinary(),
        ),
    ).ids[0];
    check("查到了临时卡片", cardId !== undefined, `card=${cardId}`);
    const repsBefore = await cardReps(cardId);
    let repsAfter = repsBefore;
    const repsDeadline = Date.now() + 12_000;
    while (repsAfter === repsBefore && Date.now() < repsDeadline) {
        await sleep(300);
        repsAfter = await cardReps(cardId);
    }
    check(
        "答案面自动评了分（answerAction=ANSWER_GOOD）",
        repsAfter === repsBefore + 1,
        `reps ${repsBefore} → ${repsAfter}`,
    );
    check("宿主日志说是「到点评「良好」」", readLog().includes("自动前进: 到点评「良好」"));

    // 关掉：正在跑的那一代计时器要作废（`auto_advance_generation`），不该再有新渲染
    await bridge("rev:autoAdvance");
    await sleep(300);
    const rendersAfterOff = countLog("渲染完成");
    const repsAfterOff = await cardReps(cardId);
    await sleep(3000);
    check(
        "关掉之后不再自动前进",
        countLog("渲染完成") === rendersAfterOff && (await cardReps(cardId)) === repsAfterOff,
        `渲染 ${rendersAfterOff} → ${countLog("渲染完成")}，reps ${repsAfterOff} → ${await cardReps(cardId)}`,
    );
    check("宿主日志记了关闭", readLog().includes("自动前进: false"));
} catch (error) {
    check("自动前进检查", false, String(error));
} finally {
    // 还原：牌组配置（把改过的五个字段写回原值）+ 临时笔记
    try {
        if (originalConfig && forUpdate) {
            originalConfig.config.secondsToShowQuestion = originalValues.secondsToShowQuestion;
            originalConfig.config.secondsToShowAnswer = originalValues.secondsToShowAnswer;
            originalConfig.config.questionAction = originalValues.questionAction;
            originalConfig.config.answerAction = originalValues.answerAction;
            originalConfig.config.waitForAudio = originalValues.waitForAudio;
            const configs = forUpdate.allConfig
                .map((entry) => entry.config)
                .filter((config) => config !== undefined);
            await writeConfigs(deckId, configs);
        }
        if (noteId) {
            await rpc(
                "removeNotes",
                new RemoveNotesRequest({ noteIds: [noteId], cardIds: [] }).toBinary(),
            );
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
