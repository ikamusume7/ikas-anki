// 往 Basic note type 的正面模板里插入 / 移除 `{{tts en_US:Front}}`。
//
// 用途：验证复习界面的 TTS 链路。测试集合里没有带 `{{tts}}` 的卡片，而这条链路
// （渲染出 TTS 标签 → rslib 合成 WAV → 入队播放）必须真的跑一次才算验证过。
// 改完模板后让复习界面重新渲染，日志里应出现「TTS 已合成」与「播放音频」。
//
// 只应指向隔离的测试集合（out/tauri-test/…），不要指向真实 profile。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/set-tts-template.mjs add     # 插入模板
//   node tauri/tools/set-tts-template.mjs remove  # 还原

import { Notetype, NotetypeId, NotetypeNames } from "../../out/ts/lib/generated/anki/notetypes_pb.js";
import { Empty } from "../../out/ts/lib/generated/anki/generic_pb.js";

const action = process.argv[2] ?? "add";
if (!["add", "remove"].includes(action)) {
    console.error("用法：node set-tts-template.mjs [add|remove]");
    process.exit(2);
}

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

const notetypes = NotetypeNames.fromBinary(await call("getNotetypeNames", new Empty().toBinary()));
// zh-CN 下 Basic 叫「问答题」；必须精确匹配，否则会命中「问答题（附翻转卡片）」
const basic = notetypes.entries.find((entry) => /^(Basic|问答题)$/i.test(entry.name));
if (!basic) {
    throw new Error(`找不到 Basic note type：${notetypes.entries.map((e) => e.name).join("、")}`);
}

const notetype = Notetype.fromBinary(
    await call("getNotetype", new NotetypeId({ ntid: basic.id }).toBinary()),
);
const template = notetype.templates[0];
if (!template) {
    throw new Error(`${basic.name} 没有卡片模板`);
}
// 字段名是**本地化**的：zh-CN 下 Basic 的字段是「正面」「背面」而不是 Front/Back，
// 写成 Front 会被 rslib 的模板校验拒绝（"未找到「Front」字段"）。
const field = notetype.fields[0];
if (!field) {
    throw new Error(`${basic.name} 没有字段`);
}
const TTS = `{{tts en_US:${field.name}}}`;

// proto 里的字段名是 `q_format`（对应 Qt 的 `qfmt`）
const before = template.config.qFormat;
const stripped = before
    .split("\n")
    .filter((line) => line.trim() !== TTS)
    .join("\n");
const after = action === "add" ? `${TTS}\n${stripped}` : stripped;

if (after === before) {
    console.log(`模板无需改动（当前${action === "add" ? "已包含" : "不含"} ${TTS}）`);
    process.exit(0);
}
template.config.qFormat = after;

await call("updateNotetype", Notetype.fromBinary(notetype.toBinary()).toBinary());
console.log(`已${action === "add" ? "插入" : "移除"} ${TTS}`);
console.log(`模板正面：\n${after.split("\n").slice(0, 3).join("\n")}\n…`);
