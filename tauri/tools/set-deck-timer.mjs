// 开关测试牌组（Default）的计时器，用于验证「计时与时间盒」。
//
// 计时器默认关闭（`showTimer = false`），关闭时外壳传 `maxTime = 0`、前端不显示
// 计时。开启后 `maxTime` 取牌组的 `maxTaken`（秒），前端会从 0:00 开始计时，
// 到达上限后变红。
//
// 用法：
//   node tauri/tools/set-deck-timer.mjs show          # 打印当前配置
//   node tauri/tools/set-deck-timer.mjs on 10         # 开启，上限 10 秒
//   node tauri/tools/set-deck-timer.mjs off
//
// 前置：外壳正在运行（固定 ANKI_API_PORT / ANKI_API_KEY）。

import { DeckConfig, DeckConfigId } from "../../out/ts/lib/generated/anki/deck_config_pb.js";
import { DeckId, Deck } from "../../out/ts/lib/generated/anki/decks_pb.js";
import { Json } from "../../out/ts/lib/generated/anki/generic_pb.js";

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

const decodeJson = (bytes) => JSON.parse(new TextDecoder().decode(Json.fromBinary(bytes).json));
const encodeJson = (value) =>
    new Json({ json: new TextEncoder().encode(JSON.stringify(value)) }).toBinary();

async function defaultDeckConfigId() {
    const deck = Deck.fromBinary(await call("getDeck", new DeckId({ did: 1 }).toBinary()));
    if (deck.kind.case !== "normal") {
        throw new Error(`牌组 1 不是普通牌组（${deck.kind.case}）`);
    }
    return deck.kind.value.configId;
}

/// 同时读 legacy JSON 与 proto 两种表示，便于对比映射
async function readConfig() {
    const configId = await defaultDeckConfigId();
    const id = new DeckConfigId({ dcid: configId });
    const legacy = decodeJson(await call("getDeckConfigLegacy", id.toBinary()));
    const proto = DeckConfig.fromBinary(await call("getDeckConfig", id.toBinary()));
    return { configId, legacy, proto };
}

const [action, secondsArg] = process.argv.slice(2);
const seconds = Number(secondsArg ?? 10);

if (action === "show") {
    const { configId, legacy, proto } = await readConfig();
    console.log(`configId=${configId}`);
    console.log(`legacy: timer=${legacy.timer}  maxTaken=${legacy.maxTaken}`);
    console.log(
        `proto:  showTimer=${proto.config.showTimer}  capAnswerTimeToSecs=${proto.config.capAnswerTimeToSecs}`,
    );
} else if (action === "on" || action === "off") {
    const configId = await defaultDeckConfigId();
    const id = new DeckConfigId({ dcid: configId });
    const config = decodeJson(await call("getDeckConfigLegacy", id.toBinary()));
    config.timer = action === "on" ? 1 : 0;
    config.maxTaken = seconds;
    await call("addOrUpdateDeckConfigLegacy", encodeJson(config));

    const after = await readConfig();
    console.log(
        `已更新 configId=${configId}: legacy timer=${after.legacy.timer} maxTaken=${after.legacy.maxTaken}`,
    );
    console.log(
        `proto:  showTimer=${after.proto.config.showTimer}  capAnswerTimeToSecs=${after.proto.config.capAnswerTimeToSecs}`,
    );
} else {
    console.error("用法: node tauri/tools/set-deck-timer.mjs show|on|off [秒]");
    process.exit(2);
}
