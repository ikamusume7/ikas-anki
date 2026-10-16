// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 「命令行带文件启动」（双击 `.apkg` / `anki file.apkg`）的检查（PARITY 6.7 缺口②）。
//
// 对照 `qt/aqt/main.py` 的命令行处理：命令行里的文件走**与拖入同一套**接受规则
// （`crate::importing::on_drop` → `drop_path`：只收存在的文件、跳过 `.colpkg`、一次一个），
// 最后打开同一个导入页 `/import-anki-package/<百分号编码的路径>`。
//
// 这里断言的是"命令行这条路真的通到了导入页"：
//   1. 宿主日志里有 `命令行传入文件: <路径>`；
//   2. 主窗口导航到了那份文件的导入页（路径按 `quote()` 编码）。
// 导入页本身（`.filename`、点「导入」、卡片真的入库）由 `import-check.mjs` 覆盖，
// 不在这里重复。
//
// 前置：外壳是**带着夹具路径当参数**启动的（`verify-cli-open.ps1` 第二阶段），
// 并且 `ANKI_LOG` 指向它的日志。
//
// 用法：
//   $env:ANKI_LOG="out/cli-open/run.log"; $env:CLI_FIXTURE="<abs>/import-fixture.apkg"
//   node tauri/tools/cli-open-check.mjs

import fs from "node:fs";
import path from "node:path";

const logPath = process.env.ANKI_LOG ?? "";
const fixture = process.env.CLI_FIXTURE ?? "";
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

async function waitFor(predicate, timeoutMs = 20_000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
        if (predicate()) {
            return true;
        }
        await sleep(200);
    }
    return false;
}

// 与 `tauri/src/importing.rs` 的 `quote()` 同一套：不动 `/`，其余按百分号编码
/// （Windows 的盘符与反斜杠因此编成 `C%3A%5C...`）。
function encodePath(file) {
    return file
        .split("/")
        .map((segment) => encodeURIComponent(segment))
        .join("/");
}

try {
    if (!logPath) {
        throw new Error("缺少 ANKI_LOG");
    }
    if (!fixture || !fs.existsSync(fixture)) {
        throw new Error(`夹具不存在: ${fixture}`);
    }

    check(
        "宿主日志里有「命令行传入文件」",
        await waitFor(() => readLog().includes(`命令行传入文件: ${fixture}`)),
        (readLog().match(/命令行传入文件[^\n]*/) ?? ["（没有日志）"]).join(" / "),
    );

    // `.apkg` 走的是 Qt 那种**独立导入窗口**（`importing.rs:222` 的 `打开导入窗口` →
    // `windows::open(IMPORT, route)`），不是主窗口导航
    // 注意路由**没有前导 `/`**（`import_dialog.py:68` 是 `f"{ts_page}/{quote(path)}"`，
    // 窗口 URL 由 `windows::open` 补成 `http://127.0.0.1:<port>/import-anki-package/...`）
    // 注意：`quote()` 会把 Windows 的反斜杠也编成 `%5C`（整条路径就是**一段**），
    // 所以这里不能先把 `\` 换成 `/`，否则编码结果对不上
    const route = `import-anki-package/${encodePath(fixture)}`;
    check(
        "打开了这份文件的导入窗口（同一个路由）",
        await waitFor(() => readLog().includes(`打开导入窗口: ${route}`)),
        (readLog().match(/打开导入窗口[^\n]*/) ?? ["（没有导入窗口日志）"]).join(" / "),
    );
    check(
        "导入窗口真的建出来了",
        await waitFor(() => readLog().includes("打开窗口: import")),
        (readLog().match(/打开窗口: [^\n]*/) ?? ["（没有建窗日志）"]).slice(-2).join(" / "),
    );

    check(
        "命令行这条路没走拖入（两者要能区分）",
        !readLog().includes("主窗口拖入"),
        (readLog().match(/主窗口拖入[^\n]*/) ?? ["（没有拖入日志）"]).join(" / "),
    );
} catch (error) {
    check("命令行打开检查", false, String(error));
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
