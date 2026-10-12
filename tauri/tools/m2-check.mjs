// M2 平台能力的端到端验证（外壳需已启动，见 verify-m2.ps1）。
//
// 覆盖 M2 期间新实现的三件事：
//   1. readClipboard 的 text/uri-list（Windows 上读 CF_HDROP，此前完全不支持）
//   2. TTS：allTtsVoices → writeTtsStream 能产出可播放的 WAV
//   3. recordAudio：开始录音 → 桥接命令「保存」→ 拿到一个合法的 WAV 路径
//
// 用法：
//   $env:ANKI_API_PORT="40016"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/m2-check.mjs

import fs from "node:fs";
import os from "node:os";
import path from "node:path";

import { ExtractAvTagsRequest, ExtractAvTagsResponse, AllTtsVoicesRequest, AllTtsVoicesResponse, WriteTtsStreamRequest } from "../../out/ts/lib/generated/anki/card_rendering_pb.js";
import { ReadClipboardRequest, ReadClipboardResponse } from "../../out/ts/lib/generated/anki/frontend_pb.js";
import { Empty, String as GenericString } from "../../out/ts/lib/generated/anki/generic_pb.js";

const port = process.env.ANKI_API_PORT ?? "40001";
const key = process.env.ANKI_API_KEY ?? "";
const base = `http://127.0.0.1:${port}`;

const failures = [];
const skipped = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

function skip(name, reason) {
    console.log(`SKIP  ${name} — ${reason}`);
    skipped.push(name);
}

async function rpc(method, body, signal) {
    const response = await fetch(`${base}/_anki/${method}`, {
        method: "POST",
        headers: {
            "Content-Type": "application/binary",
            Authorization: `Bearer ${key}`,
        },
        body,
        signal,
    });
    if (!response.ok) {
        throw new Error(`${method} → HTTP ${response.status} ${await response.text()}`);
    }
    return new Uint8Array(await response.arrayBuffer());
}

/// 模拟页面里的 `bridgeCommand`。`window` 必须填 —— 注入脚本会给每个请求带上
/// `X-Anki-Window`，宿主据此判断命令来自哪个窗口（见 `src/bridge.rs`）。
async function bridge(window, command) {
    const response = await fetch(`${base}/_anki/pycmd`, {
        method: "POST",
        headers: {
            "Content-Type": "text/plain",
            Authorization: `Bearer ${key}`,
            "X-Anki-Window": window,
        },
        body: command,
    });
    return response.status;
}

function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
}

/// 给「会一直挂着的请求」兜底，避免脚本卡死。
function withTimeout(promise, ms, label) {
    return Promise.race([
        promise,
        new Promise((_, reject) => {
            const timer = setTimeout(() => reject(new Error(`${label} 超时（${ms} ms）`)), ms);
            timer.unref?.();
        }),
    ]);
}

/// WAV 文件头是 `RIFF....WAVE`，用它判断合成/录音产物是不是合法音频。
function isWav(file) {
    if (!fs.existsSync(file)) {
        return false;
    }
    const header = Buffer.alloc(12);
    const fd = fs.openSync(file, "r");
    fs.readSync(fd, header, 0, 12, 0);
    fs.closeSync(fd);
    return header.toString("latin1", 0, 4) === "RIFF" && header.toString("latin1", 8, 12) === "WAVE";
}

// 1. 剪贴板的文件列表（调用方需先把文件放进剪贴板，见 verify-m2.ps1）
{
    const expected = (process.env.M2_CLIPBOARD_FILES ?? "")
        .split(";")
        .filter(Boolean)
        .map((file) => file.replace(/\\/g, "/"));
    const response = ReadClipboardResponse.fromBinary(
        await rpc("readClipboard", new ReadClipboardRequest({ types: ["text/uri-list"] }).toBinary()),
    );
    const raw = response.data["text/uri-list"];
    const list = raw ? new TextDecoder().decode(raw) : "";
    const entries = list.split("\n").map((line) => line.trim()).filter(Boolean);

    const decoded = entries.map((uri) => decodeURIComponent(uri.replace(/^file:\/\/\//, "").replace(/^file:\/\//, "")));
    check(
        "剪贴板 text/uri-list",
        expected.length > 0 &&
            decoded.length === expected.length &&
            expected.every((file, index) => decoded[index]?.toLowerCase() === file.toLowerCase()),
        decoded.join(" | ") || "（剪贴板里没有文件列表）",
    );
    // 路径里有空格时必须要转义，否则 rslib 的 `Url::parse` 会解析失败
    check("含空格的路径被转义", entries.some((uri) => uri.includes("%20")), entries.join(" | "));
}

// 2. TTS 合成
let voices = [];
try {
    voices = AllTtsVoicesResponse.fromBinary(
        await rpc("allTtsVoices", new AllTtsVoicesRequest({ validate: false }).toBinary()),
    ).voices;
    check("allTtsVoices", voices.length > 0, `${voices.length} 个语音包`);
} catch (error) {
    check("allTtsVoices", false, String(error));
}

if (voices.length > 0) {
    const target = path.join(os.tmpdir(), "anki", `m2-check-${Date.now()}.wav`);
    // rslib 的 `write_tts_stream` 只负责写文件、不建目录
    fs.mkdirSync(path.dirname(target), { recursive: true });
    try {
        await rpc(
            "writeTtsStream",
            new WriteTtsStreamRequest({
                path: target,
                voiceId: voices[0].id,
                speed: 1.0,
                text: "hello from the tauri shell",
            }).toBinary(),
        );
        const size = fs.existsSync(target) ? fs.statSync(target).size : 0;
        check("writeTtsStream 产出 WAV", isWav(target), `${size} 字节 @ ${target}`);
    } catch (error) {
        check("writeTtsStream 产出 WAV", false, String(error));
    }
} else {
    skip("writeTtsStream 产出 WAV", "系统没有可用的语音包");
}

// 3. 复习界面取 TTS 标签的链路（reviewer 靠它决定是否调用上面的合成）
{
    const response = ExtractAvTagsResponse.fromBinary(
        await rpc(
            "extractAvTags",
            new ExtractAvTagsRequest({
                text: "[anki:tts lang=en_US]hello[/anki:tts]",
                questionSide: true,
            }).toBinary(),
        ),
    );
    const ttsTag = response.avTags.find((tag) => tag.value.case === "tts");
    check(
        "extractAvTags 解析出 TTS 标签",
        Boolean(ttsTag),
        ttsTag
            ? `lang=${ttsTag.value.value.lang} text=${ttsTag.value.value.fieldText}`
            : "（未解析出 TTS）",
    );
}

// 4. 录音闭环：请求一直挂起，直到桥接命令告诉它保存
{
    const controller = new AbortController();
    let recorded;
    try {
        // 不 await：这个请求会一直挂到用户在录音窗口里点保存
        const pending = rpc("recordAudio", new Empty().toBinary(), controller.signal);
        await sleep(5000);
        // 录音窗口的页面就是这么发的（`bridgeCommand("saveRecording")`）
        const status = await bridge("recorder", "saveRecording");
        check("桥接命令 saveRecording 被接受", status === 200, `HTTP ${status}`);
        recorded = GenericString.fromBinary(await withTimeout(pending, 30_000, "recordAudio")).val;
    } catch (error) {
        check("recordAudio 闭环", false, String(error.cause ?? error));
    } finally {
        // 超时路径下要主动断开，否则挂着的请求会让 node 不退出
        controller.abort();
    }
    if (recorded !== undefined) {
        const size = recorded && fs.existsSync(recorded) ? fs.statSync(recorded).size : 0;
        check("recordAudio 返回合法 WAV", isWav(recorded), `${recorded || "（空路径）"} ${size} 字节`);
    }
}

console.log(`\n结果：${failures.length === 0 ? "全部通过" : `${failures.length} 项失败`}${skipped.length ? `，跳过 ${skipped.length} 项` : ""}`);
if (failures.length > 0) {
    console.error(`失败项：${failures.join("、")}`);
    process.exitCode = 1;
}
