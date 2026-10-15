// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 自动备份（对照 `qt/aqt/main.py` 的备份逻辑，PARITY 1.5 / 6.2）。
//!
//! Qt 的两个触发点：
//! - **关闭时**：`main.py:726-733` 调 `col.create_backup(force=False)`，退出前还会
//!   `await_backup_completion()`（`main.py:608`）；
//! - **每 5 分钟**：`main.py:1543-1549` 的定时器，同样 `force=False`。
//!
//! 是否真的落盘由后端决定（`BackupLimits` 默认 30 分钟间隔 + 集合自上次备份后有改动，
//! 见 `rslib/src/collection/backup.rs:28-48`）。本外壳的两个触发点是：
//!
//! - **启动时先尝试一次**：外壳没有"优雅关闭"的保证（验证脚本是 `Stop-Process -Force`，
//!   真实使用也可能被强杀），所以把 Qt 的"关闭时备份"换成"启动时先补一次"—— 上一次
//!   会话的改动因此能被保住，语义仍然是 `force=False`；
//! - **每 5 分钟一次**，与 Qt 的定时器一致。
//!
//! 备份目录与桌面版相同：`<profile>/backups`（`qt/aqt/profiles.py` 的 `backupFolder()`），
//! 文件名由后端按 `backup-%Y-%m-%d-%H.%M.%S.colpkg` 生成。

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use anki_proto::collection::CreateBackupRequest;
use anki_proto::generic::Bool;
use prost::Message as _;

use crate::state::AppState;

/// 备份目录（与桌面版一致：`<profile>/backups`）。
pub fn backup_folder(profile_dir: &Path) -> PathBuf {
    profile_dir.join("backups")
}

/// Qt 的定时器间隔（`main.py:1543-1549` 的 5 分钟）。
const INTERVAL: Duration = Duration::from_secs(5 * 60);

/// 起一个后台线程：启动时尝试一次，之后每 5 分钟一次。
///
/// 与 `server::spawn` 同一套路：`bootstrap` 跑在 Tauri 的 `setup` 里，**没有**现成的
/// Tokio 运行时，所以要自己建一个（直接 `tokio::spawn` 会 panic：no reactor running）。
pub fn spawn(state: AppState) {
    std::thread::Builder::new()
        .name("anki-backup".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("构建备份运行时失败");
            runtime.block_on(async move {
                // 测试钩子：强制先备份一次（`force=True` + 等它写完），好让验证脚本断言
                // 产物 —— 否则默认间隔是 30 分钟，脚本等不起（与 `ANKI_SAVE_PATH` 同类）
                if matches!(
                    std::env::var("ANKI_BACKUP_NOW").as_deref(),
                    Ok("1") | Ok("true")
                ) {
                    try_backup(&state, true, true).await;
                }
                loop {
                    try_backup(&state, false, false).await;
                    tokio::time::sleep(INTERVAL).await;
                }
            });
        })
        .expect("启动备份线程失败");
}

/// 调 `createBackup`：`force` 跳过间隔判断，`wait` 等它写完（默认留给后端线程）。
async fn try_backup(state: &AppState, force: bool, wait: bool) {
    let folder = backup_folder(&state.profile_dir);
    if let Err(error) = std::fs::create_dir_all(&folder) {
        tracing::warn!("创建备份目录失败（{}）: {error:#}", folder.display());
        return;
    }
    let request = CreateBackupRequest {
        backup_folder: folder.to_string_lossy().into_owned(),
        force,
        wait_for_completion: wait,
    };
    match state
        .backend
        .call("createBackup", request.encode_to_vec())
        .await
    {
        Ok(bytes) => match Bool::decode(bytes.as_slice()) {
            Ok(created) if created.val => tracing::info!("已创建备份: {}", folder.display()),
            Ok(_) => tracing::debug!("备份检查: 现在还不需要备份"),
            Err(error) => tracing::warn!("备份响应解码失败: {error}"),
        },
        Err(error) => tracing::warn!("备份失败: {error}"),
    }
}
