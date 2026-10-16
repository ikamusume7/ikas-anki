// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 复习键盘的检查。分两半：
//
//   1. **按键 → 桥接命令**：把宿主注入的那份脚本（`tauri/assets/reviewer-keys.js`，
//      宿主用 `include_str!` 引用，见 `src/reviewer.rs`）加载到 Playwright 页面里，
//      按真实按键，断言 `pycmd` 收到的命令。页面里的 `#middle`（答案面判定）与
//      `showAnswer()` / `selectedAnswerButton()` 用上游那份 `reviewer-bottom.js` 摆，
//      保证状态判定和页面里是同一套代码。
//   2. **宿主守卫**（真外壳）：`ease` 只在答案面生效（对照 `_answerCard()` 的
//      `self.state != "answer"`）。这条守卫在宿主侧，所以直接发桥接命令验证：
//      问题面发 `ease3` 不能把卡片答掉。
//
// 前置：外壳已启动（ANKI_START_PAGE=reviewer）、队列里有卡片。
//       `ANKI_LOG` 指向外壳的日志（第 2 半要等宿主日志）。
//
// 用法：
//   $env:ANKI_API_PORT="40028"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/reviewer-check/run.log"
//   node tauri/tools/reviewer-keys-check.mjs

import fs from "node:fs";

import { chromium } from "playwright";

import { GetQueuedCardsRequest, QueuedCards } from "../../out/ts/lib/generated/anki/scheduler_pb.js";
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

function countLog(pattern) {
    return readLog().split(pattern).length - 1;
}

/** 等日志里某个片段的出现次数超过 `minimum`（外壳的日志按行追加）。 */
async function waitLogAbove(pattern, minimum, timeoutMs = 15000) {
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
            // 不让 socket 留着（keep-alive 的句柄会在 node 退出时触发 libuv 断言）
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/** 发一条桥接命令（模拟页面里的 `pycmd`）。 */
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

/** 队列顶端卡片（没有卡片时返回 null）。 */
async function topCard() {
    const queued = QueuedCards.fromBinary(
        await rpc("getQueuedCards", new GetQueuedCardsRequest({ fetchLimit: 1 }).toBinary()),
    );
    return queued.cards[0]?.card ?? null;
}

/**
 * 卡片的复习次数（`Card.reps`）。
 *
 * 判断「有没有真的评过分」不能用"卡片离没离开队列顶端"：学习卡片评完之后
 * 还在 learn-ahead 窗口里，会被 `getQueuedCards` 再取出来（同一张卡、`reps` 已 +1）。
 */
async function cardReps(cardId) {
    const card = Card.fromBinary(await rpc("getCard", new CardId({ cid: cardId }).toBinary()));
    return card.reps;
}

// ---------------------------------------------------------------------------
// 1. 按键 → 桥接命令（同一个脚本文件）
// ---------------------------------------------------------------------------
const script = fs.readFileSync("tauri/assets/reviewer-keys.js", "utf8");
// 上游的底部栏脚本：`showAnswer()` 写 `#middle`、`selectedAnswerButton()` 取焦点按钮，
// 页面里加载的是同一份（`src/legacy.rs` 的 reviewer_page js 列表）
const bottomScript = fs.readFileSync("qt/_aqt/data/web/js/reviewer-bottom.js", "utf8");

const browser = await chromium.launch();
try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 700 } });
    await page.setContent(
        '<html><body><div id="middle"></div><div id="time"></div><input id="typeans"></body></html>',
    );
    // 桩要在 `setContent` 之后装：`addInitScript` 只在导航时跑，`setContent` 不算导航
    await page.evaluate(() => {
        window.__commands = [];
        window.__prevented = [];
        window.pycmd = (command) => window.__commands.push(command);
        // 宿主每次会话开始时写的那份配置（见 `reviewer::install_reviewer_shortcuts`）
        window.__ankiReviewerConfig = { spacebarRatesCard: true };
    });
    await page.addScriptTag({ content: bottomScript });
    await page.addScriptTag({ content: script });
    // 记录每个按键是否被 `preventDefault()`（本监听器在脚本之后注册，看到的是最终值）
    await page.evaluate(() => {
        document.addEventListener("keydown", (event) => {
            window.__prevented.push(event.defaultPrevented);
        });
    });

    /** 摆出问题面 / 答案面（走上游的 `showAnswer()`，与页面里一致）。 */
    async function setAnswerShown(shown) {
        await page.evaluate((answerShown) => {
            window.__commands = [];
            if (answerShown) {
                Object.assign(window.__ankiReviewerConfig, { spacebarRatesCard: true });
                window.showAnswer(
                    '<button data-ease="1">重来</button><button data-ease="2">困难</button>' +
                        '<button data-ease="3">良好</button><button data-ease="4">简单</button>',
                );
            } else {
                window.showAnswer('<button id="ansbut">显示答案</button>');
            }
            // 必须显式 blur：`body.focus()` 不会把焦点从输入框上拿开
            // （`body` 默认不可聚焦），那样后面的按键都会被当成输入
            if (document.activeElement instanceof HTMLElement) {
                document.activeElement.blur();
            }
            document.body.focus();
        }, shown);
    }

    /** 按一串键，返回 `pycmd` 收到的命令。 */
    async function press(...keys) {
        await page.evaluate(() => {
            window.__commands = [];
            window.__prevented = [];
        });
        for (const key of keys) {
            await page.keyboard.press(key);
        }
        return page.evaluate(() => ({
            commands: window.__commands.slice(),
            prevented: window.__prevented.slice(),
        }));
    }

    async function expect(name, keys, expected) {
        const result = await press(...keys);
        const actual = result.commands.join(" ");
        check(name, actual === expected.join(" "), actual || "（没有命令）");
        return result;
    }

    // —— 问题面 ——
    await setAnswerShown(false);
    await expect("问题面空格 → 显示答案", ["Space"], ["ans"]);
    await expect("问题面回车 → 显示答案", ["Enter"], ["ans"]);
    await expect("问题面按 1 不评分（对照 `state != \"answer\"`）", ["Digit1"], []);

    // —— 答案面 ——
    await setAnswerShown(true);
    await expect("答案面 1–4 → ease1–ease4", ["Digit1", "Digit2", "Digit3", "Digit4"], [
        "ease1",
        "ease2",
        "ease3",
        "ease4",
    ]);
    await expect("答案面空格 → 默认档 3（没有焦点按钮）", ["Space"], ["ease3"]);
    await expect("答案面回车 → 默认档 3", ["Enter"], ["ease3"]);
    await page.locator('[data-ease="2"]').focus();
    await expect("答案面空格 → 焦点按钮那一档", ["Space"], ["ease2"]);
    await expect("答案面 1 → ease1（与焦点无关）", ["Digit1"], ["ease1"]);

    // 空格评分关闭（`meta.spacebar_rates_card = false`）
    await setAnswerShown(true);
    await page.evaluate(() => {
        window.__ankiReviewerConfig.spacebarRatesCard = false;
    });
    await expect("空格评分关闭后答案面空格 → 不评分", ["Space"], []);
    await expect("空格评分关闭不影响 1–4", ["Digit2"], ["ease2"]);
    await page.evaluate(() => {
        window.__ankiReviewerConfig.spacebarRatesCard = true;
    });

    // —— 类型作答的输入框里不抢按键 ——
    await setAnswerShown(false);
    await page.locator("#typeans").focus();
    const typing = await expect(
        "输入框里 u / a / 1 / 空格 不触发快捷键",
        ["KeyU", "KeyA", "Digit1", "Space"],
        [],
    );
    check(
        "输入框里的空格没有被 preventDefault（是普通字符）",
        typing.prevented.every((value) => value === false),
        typing.prevented.join(","),
    );

    // —— 组合键 ——
    await setAnswerShown(false);
    await expect("Ctrl+1..7 → 旗标", [
        "Control+Digit1",
        "Control+Digit2",
        "Control+Digit3",
        "Control+Digit4",
        "Control+Digit5",
        "Control+Digit6",
        "Control+Digit7",
    ], [
        "rev:flag:1",
        "rev:flag:2",
        "rev:flag:3",
        "rev:flag:4",
        "rev:flag:5",
        "rev:flag:6",
        "rev:flag:7",
    ]);
    await expect("Ctrl+Alt+N → 重置卡片", ["Control+Alt+KeyN"], ["rev:forget"]);
    await expect("Ctrl+Alt+E → 创建副本", ["Control+Alt+KeyE"], ["rev:copy"]);
    await expect("Ctrl+Alt+I → 上一张卡片信息", ["Control+Alt+KeyI"], ["rev:prevCardInfo"]);
    await expect("Ctrl+Shift+D → 设置到期日", ["Control+Shift+KeyD"], ["rev:due"]);
    await expect("Ctrl+Delete → 删除笔记", ["Control+Delete"], ["rev:deleteNote"]);
    await expect("Ctrl+Z → 撤销", ["Control+KeyZ"], ["undo"]);
    await expect("Ctrl+P → 设置", ["Control+KeyP"], ["preferences"]);

    // —— 字母与符号 ——
    await expect("e → 编辑当前卡片", ["KeyE"], ["edit"]);
    await expect("m → 更多菜单", ["KeyM"], ["more"]);
    await expect("u → 撤销", ["KeyU"], ["undo"]);
    await expect("o / i → 牌组选项 / 卡片信息", ["KeyO", "KeyI"], ["rev:options", "rev:cardInfo"]);
    await expect("a / b / t / d / s → 主窗口全局入口", ["KeyA", "KeyB", "KeyT", "KeyD", "KeyS"], [
        "add",
        "browse",
        "stats",
        "decks",
        "overview",
    ]);
    await expect("Shift+t → 统计（`main.py:1186`）", ["Shift+KeyT"], ["stats"]);
    await expect("y → 同步（`main.py:1187`）", ["KeyY"], ["sync"]);
    await expect("= / - → 暂缓笔记 / 暂缓卡片", ["Equal", "Minus"], [
        "rev:buryNote",
        "rev:buryCard",
    ]);
    await expect("* → 标记笔记", ["Shift+Digit8"], ["rev:mark"]);
    await expect("! / @ → 搁置笔记 / 搁置卡片", ["Shift+Digit1", "Shift+Digit2"], [
        "rev:suspendNote",
        "rev:suspendCard",
    ]);

    // —— 尚未实现的按键不该乱发命令 ——
    // 音频（r / F5 / 5 / 6 / 7 / v / Shift+v）与自动前进（Shift+A）没有后端能力
    await expect("Shift+A → 自动前进开关（不是裸 a）", ["Shift+KeyA"], ["rev:autoAdvance"]);
    await expect("音频与录音键未绑定", ["KeyR", "Digit5", "Digit6", "Digit7", "KeyV", "Shift+KeyV"], []);
    const f5 = await expect("F5 不发命令", ["F5"], []);
    check("F5 被拦住（否则 WebView 会重载页面）", f5.prevented[0] === true, String(f5.prevented[0]));

    await page.close();
} finally {
    await browser.close();
}

// ---------------------------------------------------------------------------
// 2. 宿主守卫（真外壳）
// ---------------------------------------------------------------------------
try {
    if (!logPath) {
        check("ANKI_LOG 已设置", false, "第 2 半需要读外壳日志");
        throw new Error("缺少 ANKI_LOG");
    }
    // 先让宿主回到复习页并渲染问题面（保证当前是「还没显示答案」的状态）。
    // 用 `study` 而不是 `domDone`：前面的检查可能已经把队列清空过一次、主窗口
    // 停在概览上（那种情况下 `domDone` 会被忽略，`reviewer_active` 是 false）
    const renders = countLog("渲染完成");
    await bridge("study");
    check("发 study 后宿主渲染了问题面", await waitLogAbove("渲染完成", renders), logPath);

    const before = await topCard();
    if (!before) {
        check("队列里有卡片", false, "队列为空，无法测试评分守卫");
        throw new Error("队列为空");
    }
    const repsBefore = await cardReps(before.id);

    // 问题面发 ease3：宿主必须挡住（对照 `_answerCard()` 的 `self.state != "answer"`）
    await bridge("ease3");
    await sleep(1200);
    check(
        "问题面发 ease3 没有把卡片答掉",
        (await cardReps(before.id)) === repsBefore,
        `reps ${repsBefore} → ${await cardReps(before.id)}`,
    );
    check("宿主日志记录了这次忽略", readLog().includes("尚未显示答案"));

    // 答案面：`ans` 之后 `ease3` 应当评分（`reps` +1）并重新取队列
    const buttonLogs = countLog("评分按钮");
    await bridge("ans");
    check("显示答案后画出了评分按钮", await waitLogAbove("评分按钮", buttonLogs), logPath);
    const queueLogs = countLog("队列: 共");
    await bridge("ease3");
    let repsAfter = await cardReps(before.id);
    const repsDeadline = Date.now() + 10000;
    while (repsAfter === repsBefore && Date.now() < repsDeadline) {
        await sleep(300);
        repsAfter = await cardReps(before.id);
    }
    check("评分后卡片的复习次数 +1", repsAfter === repsBefore + 1, `reps ${repsBefore} → ${repsAfter}`);
    check(
        "评分后宿主重新取了队列（进入下一张）",
        await waitLogAbove("队列: 共", queueLogs, 10000),
        logPath,
    );
} catch (error) {
    check("宿主守卫检查", false, String(error));
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
