// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

// 导出目录拦截（PARITY 5.9）的检查。对照 `qt/aqt/import_export/exporting.py:132-138`：
// 保存路径落在 Anki 数据目录里就提示"换个位置"，不写文件。本外壳判的是**当前 profile
// 目录**（`collection.anki2` 所在处）。
//
// 自动化里没法重弹原生保存框，所以宿主在测试钩子 `ANKI_SAVE_PATH` 指向 profile 目录内时
// 直接拒绝（返回空串）并记日志 —— 这个脚本就是断言那条路径：宿主返回空、日志有拒绝、
// 文件确实没被写出来。
//
// 前置：外壳以 `ANKI_SAVE_PATH=<profile 目录>` 启动 + `ANKI_LOG` / `ANKI_PROFILE_DIR`。
//
// 用法：
//   $env:ANKI_API_PORT="40031"; $env:ANKI_API_KEY="test-key-123"
//   $env:ANKI_LOG="out/decks-check/run-guard.log"; $env:ANKI_PROFILE_DIR="out/tauri-test"
//   node tauri/tools/export-guard-check.mjs

import fs from "node:fs";
import path from "node:path";

import { String as GenericString } from "../../out/ts/lib/generated/anki/generic_pb.js";
import { openFilePickerRequest } from "../../out/ts/lib/generated/anki/frontend_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const logPath = process.env.ANKI_LOG ?? "";
const profileDir = process.env.ANKI_PROFILE_DIR ?? "";
const base = `http://127.0.0.1:${port}`;
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
            Connection: "close",
        },
        body,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

try {
    if (!profileDir) {
        throw new Error("缺少 ANKI_PROFILE_DIR");
    }
    const target = path.resolve(profileDir, "export-guard.colpkg");
    if (fs.existsSync(target)) {
        fs.rmSync(target, { force: true });
    }

    const request = new openFilePickerRequest({
        title: "Export",
        key: "export-guard.colpkg",
        filterDescription: "Anki Collection Package",
        extensions: ["colpkg"],
    });
    const picked = GenericString.fromBinary(
        await rpc("saveFileDialog", request.toBinary()),
    ).val;
    check("profile 目录内的导出路径被拒绝（返回空）", picked === "", `返回「${picked}」`);

    const log = (() => {
        try {
            return fs.readFileSync(logPath, "utf8");
        } catch {
            return "";
        }
    })();
    check(
        "宿主日志里记了这次拒绝",
        log.includes("导出路径在 profile 目录内，拒绝并要求重选"),
        logPath,
    );
    check("被拒绝的路径没有被写出来", !fs.existsSync(target), target);

    // 再问一次：拒绝是稳定的（"目录外照常放行"由 decks-check 的四种导出覆盖）
    const again = GenericString.fromBinary(
        await rpc("saveFileDialog", request.toBinary()),
    ).val;
    check("再问一次仍然拒绝（不会偶尔放行）", again === "", `返回「${again}」`);
} catch (error) {
    check("导出目录拦截检查", false, String(error));
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
}
process.exit(failures.length > 0 ? 1 : 0);
