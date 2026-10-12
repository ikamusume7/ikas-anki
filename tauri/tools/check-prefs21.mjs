// 验证 profile / meta 配置确实读写了桌面的 prefs21.db。
//
// 前置：
//   1. py tauri/tools/make-prefs21-fixture.py init out/prefs21.db tauri-test
//      （base 目录是 prefs21.db 所在处；集合在 out/tauri-test/ 时 base 就是 out）
//   2. 启动外壳（ANKI_COLLECTION=out/tauri-test/collection.anki2，固定端口与密钥）
// 收尾用 `... make-prefs21-fixture.py show out/prefs21.db tauri-test` 确认写回内容
// 与 bytes 项未被破坏。
//
// 用法：
//   $env:ANKI_API_PORT="40001"; $env:ANKI_API_KEY="test-key-123"
//   node tauri/tools/check-prefs21.mjs

import { SetSettingJsonRequest } from "../../out/ts/lib/generated/anki/frontend_pb.js";
import { Json, String as GenericString } from "../../out/ts/lib/generated/anki/generic_pb.js";

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
        throw new Error(`HTTP ${response.status}: ${new TextDecoder().decode(body)}`);
    }
    return body;
}

async function getSetting(method, name) {
    const out = await call(method, new GenericString({ val: name }).toBinary());
    return JSON.parse(new TextDecoder().decode(Json.fromBinary(out).json));
}

async function setSetting(method, name, value) {
    await call(
        method,
        new SetSettingJsonRequest({
            key: name,
            valueJson: new TextEncoder().encode(JSON.stringify(value)),
        }).toBinary(),
    );
}

const failures = [];
function check(label, ok, detail) {
    console.log(`${ok ? "ok  " : "FAIL"} ${label}${detail === undefined ? "" : `: ${JSON.stringify(detail)}`}`);
    if (!ok) {
        failures.push(label);
    }
}

// 1) 读取 Python 侧写入的值（证明读的是同一份文件，而不是本地 JSON 回退）
check("profile.numBackups = 50", (await getSetting("getProfileConfigJson", "numBackups")) === 50);
check(
    "profile.searchHistory 为列表",
    JSON.stringify(await getSetting("getProfileConfigJson", "searchHistory")) ===
        JSON.stringify(["deck:current", "tag:marked"]),
);
check("profile.syncMedia = true", (await getSetting("getProfileConfigJson", "syncMedia")) === true);
check("meta.defaultLang = zh_CN", (await getSetting("getMetaJson", "defaultLang")) === "zh_CN");

// 2) 不存在的项应返回 null（对照 pm.profile.get 的语义）
check("缺失项返回 null", (await getSetting("getProfileConfigJson", "doesNotExist")) === null);

// 3) 二进制项应明确报错（Qt 版用 orjson 序列化时同样会失败）
let binaryError = "";
try {
    await getSetting("getProfileConfigJson", "mainWindowGeom");
} catch (error) {
    binaryError = error.message;
}
check("bytes 项明确报错", binaryError.includes("500"), binaryError);

// 4) 写入后应能被桌面版读到（由调用方再用 Python 校验）
await setSetting("setProfileConfigJson", "importMode", 3);
await setSetting("setMetaJson", "lastMsg", 7);
console.log("已写入 profile.importMode=3、meta.lastMsg=7");

console.log(
    failures.length === 0
        ? "\n读取检查全部通过（请再用 py tauri/tools/make-prefs21-fixture.py show 确认写回内容与 bytes 未被破坏）"
        : `\n失败项：${failures.join(", ")}`,
);
process.exitCode = failures.length === 0 ? 0 : 1;
