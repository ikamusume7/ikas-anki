// 界面语言的端到端检查：对一个**正在运行**的外壳调 `i18nResources`，断言后端实际生效的
// 语言与页面真正会拿到的文案。
//
// 为什么盯着这个端点：页面上所有文案都来自它（`ts/lib/sveltelib/i18n` 的 `setupI18n`），
// 所以一次调用同时覆盖「宿主给后端喂了哪种语言」与「翻译真的取到了」两件事；比只看日志
// 或截图都硬。
//
// 用法（环境变量）：
//   ANKI_API_PORT        端口（默认 40041）
//   ANKI_API_KEY         密钥（默认 test-key-123）
//   ANKI_EXPECT_LANG     期望 `langs.actions` 的值（可省，省了只检查文案）
//   ANKI_EXPECT_TEXT     期望 actions 模块里出现的文案（例如「牌组」）
//   ANKI_FORBID_TEXT     期望**不**出现的文案（例如英文的 "Decks"）
//
//   $env:ANKI_EXPECT_LANG="zh-CN"; $env:ANKI_EXPECT_TEXT="牌组"; node tauri/tools/i18n-check.mjs

import { I18nResourcesRequest } from "../../out/ts/lib/generated/anki/i18n_pb.js";
import { Json as GenericJson } from "../../out/ts/lib/generated/anki/generic_pb.js";

const port = process.env.ANKI_API_PORT ?? "40041";
const apiKey = process.env.ANKI_API_KEY ?? "test-key-123";
const expectLang = process.env.ANKI_EXPECT_LANG ?? "";
const expectText = process.env.ANKI_EXPECT_TEXT ?? "";
const forbidText = process.env.ANKI_FORBID_TEXT ?? "";
const failures = [];

function check(name, ok, detail = "") {
    console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail ? ` — ${detail}` : ""}`);
    if (!ok) {
        failures.push(name);
    }
}

// `actions` 模块里一定有 `actions-decks`（牌组 / Decks），用它当探针
const request = new I18nResourcesRequest({ modules: ["actions"] });
const response = await fetch(`http://127.0.0.1:${port}/_anki/i18nResources`, {
    method: "POST",
    headers: {
        "Content-Type": "application/binary",
        Authorization: `Bearer ${apiKey}`,
    },
    body: request.toBinary(),
});
check("i18nResources 返回 200", response.ok, `HTTP ${response.status}`);
if (!response.ok) {
    console.log("\n界面语言检查：失败");
    process.exit(1);
}

const payload = JSON.parse(
    new TextDecoder().decode(
        GenericJson.fromBinary(new Uint8Array(await response.arrayBuffer())).json,
    ),
);
// 响应形状是**并行数组**：`langs[0]` / `resources[0]` 是主语言，`langs[1]` / `resources[1]`
// 是英文回退（前端 `setupI18n` 就是按这个顺序建 FluentBundle 的）。
const langs = Array.isArray(payload.langs) ? payload.langs : [];
const resources = Array.isArray(payload.resources) ? payload.resources : [];
const primaryLang = langs[0] ?? "";
const primaryText = resources[0] ?? "";
console.log(`后端报告的语言: ${langs.join(" / ") || "(缺)"}`);

if (expectLang) {
    check(
        `actions 模块的主语言是 ${expectLang}`,
        primaryLang === expectLang,
        `实际 ${primaryLang}`,
    );
}
check(`文案里没有 missing-key（i18n 模块加载正常）`, !primaryText.includes("missing-key"));
if (expectText) {
    check(`文案包含「${expectText}」`, primaryText.includes(expectText));
}
if (forbidText) {
    check(`文案不含「${forbidText}」`, !primaryText.includes(forbidText));
}

if (failures.length) {
    console.log(`\n界面语言检查：失败 ${failures.length} 项（${failures.join(" / ")}）`);
    process.exit(1);
}
console.log("\n界面语言检查：全部通过");
