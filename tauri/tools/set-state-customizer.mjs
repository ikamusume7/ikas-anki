// 设置 / 清除集合配置里的 `cardStateCustomizer`（自定义调度脚本）。
//
// 用于验证复习界面的状态同步链路：
//   宿主注入 `anki.mutateNextCardStates(key, …)`
//     → 前端 `getSchedulingStatesWithContext`
//     → 用户的脚本改写状态
//     → `setSchedulingStates`（带 key）
//     → `bridgeCommand('statesMutated')`
//
// 示例脚本会把「良好」的间隔改成 42 天，因此改完重取卡片后，评分按钮上的
// 「良好」应显示 42 天 —— 能直接证明状态改写生效。
//
// 用法：
//   node tauri/tools/set-state-customizer.mjs on
//   node tauri/tools/set-state-customizer.mjs off
//   node tauri/tools/set-state-customizer.mjs set "states.good.scheduledDays = 5;"
//
// 前置：外壳正在运行（固定 ANKI_API_PORT / ANKI_API_KEY）。

import { SetConfigJsonRequest } from "../../out/ts/lib/generated/anki/config_pb.js";

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

// 注意：`states` 是 proto JSON 形态，`SchedulingState` 是嵌套 oneof，
// 所以路径是 `states.good.normal.learning` / `states.good.normal.review`；
// 写错字段名会让前端的 `SchedulingStates.fromJson` 直接报错（未知键）。
const SAMPLE = [
    "const good = states.good.normal;",
    "if (good.learning) good.learning.scheduledSecs = 2520;", // 42 分
    "if (good.review) good.review.scheduledDays = 42;", // 42 天
].join("\n");

const [action, scriptArg] = process.argv.slice(2);
let script;
if (action === "on") {
    script = SAMPLE;
} else if (action === "set") {
    script = scriptArg ?? SAMPLE;
} else if (action === "off") {
    script = "";
} else {
    console.error('用法: node tauri/tools/set-state-customizer.mjs on|off|set "<js>"');
    process.exit(2);
}

await call(
    "setConfigJson",
    new SetConfigJsonRequest({
        key: "cardStateCustomizer",
        valueJson: new TextEncoder().encode(JSON.stringify(script)),
    }).toBinary(),
);

console.log(
    script
        ? `cardStateCustomizer 已设置:\n  ${script}`
        : "cardStateCustomizer 已清除",
);
