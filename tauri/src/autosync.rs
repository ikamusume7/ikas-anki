// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 自动同步（PARITY 6.1）。
//!
//! 对照上游的两个触发点（`qt/aqt/main.py`）：
//!
//! - **启动/关闭时同步整个集合**：`maybe_auto_sync_on_open_close`（`main.py:1120-1129`，
//!   开关是 profile 的 `autoSync`，默认 true）；外壳没有"优雅关闭"的钩子，所以只做**启动**那一次；
//! - **媒体同步定时器**：`main.py:1534` 每 5 分钟跑一次 `on_periodic_sync_timer`，
//!   距上次媒体同步超过 `autoSyncMediaMinutes`（默认 15）才真的同步 —— 只同步媒体。
//!
//! 凭据与开关都在 **profile 配置**里（`prefs21.db`，与桌面版同一批键）：
//! `syncKey` / `customSyncUrl` / `autoSync` / `autoSyncMediaMinutes` / `syncMedia`。
//!
//! 测试钩子：`ANKI_SYNC_TICK_SECONDS` 改定时器间隔（默认 300 秒），配合脚本把
//! `autoSyncMediaMinutes` 设成 0 就能立刻看到一次自动媒体同步。

use std::time::Duration;
use std::time::Instant;

use anki_proto::sync::SyncAuth;
use prost::Message as _;

use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;
use crate::state::AppState;

/// 上游的定时器间隔（`main.py:1534` 的 5 分钟）。
const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// `autoSyncMediaMinutes` 的默认值（`profiles.py:722`）。
const DEFAULT_MEDIA_MINUTES: u64 = 15;

/// 起一个后台线程：启动时尝试一次，之后按 `ANKI_SYNC_TICK_SECONDS`（默认 5 分钟）轮询。
pub fn spawn(state: AppState) {
    let tick = std::env::var("ANKI_SYNC_TICK_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(INTERVAL);
    tracing::info!("自动同步定时器: 每 {} 秒检查一次", tick.as_secs());

    std::thread::Builder::new()
        .name("anki-autosync".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("构建自动同步运行时失败");
            runtime.block_on(async move {
                let mut last_media_sync = Instant::now();
                let mut first = true;
                loop {
                    match sync_auth(&state) {
                        Some(auth) => {
                            if !auto_sync_enabled(&state) {
                                tracing::debug!("自动同步: 用户关了 autoSync");
                            } else if first {
                                // 启动时：整份同步一次（上游的 on-open 那条）
                                sync_collection_and_media(&state, &auth).await;
                                last_media_sync = Instant::now();
                            } else {
                                let minutes = media_minutes(&state);
                                if minutes == 0
                                    || last_media_sync.elapsed() >= Duration::from_secs(minutes * 60)
                                {
                                    sync_media(&state, &auth).await;
                                    last_media_sync = Instant::now();
                                }
                            }
                        }
                        None => tracing::debug!("自动同步: 还没登录"),
                    }
                    first = false;
                    tokio::time::sleep(tick).await;
                }
            });
        })
        .expect("启动自动同步线程失败");
}

/// profile 里的同步凭据（没登录返回 `None`）。
fn sync_auth(state: &AppState) -> Option<SyncAuth> {
    let prefs = prefs21(state)?;
    let hkey = prefs.get(Store::Profile, "syncKey").ok().flatten()?.as_str()?.to_string();
    let endpoint = prefs
        .get(Store::Profile, "customSyncUrl")
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(str::to_string));
    Some(SyncAuth {
        hkey,
        endpoint,
        io_timeout_secs: None,
    })
}

/// `autoSync`（`profiles.py:677-679`，默认 true）。
fn auto_sync_enabled(state: &AppState) -> bool {
    prefs21(state)
        .and_then(|prefs| prefs.get(Store::Profile, "autoSync").ok().flatten())
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}

/// `autoSyncMediaMinutes`（`profiles.py:721-722`，默认 15）。
fn media_minutes(state: &AppState) -> u64 {
    prefs21(state)
        .and_then(|prefs| prefs.get(Store::Profile, "autoSyncMediaMinutes").ok().flatten())
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_MEDIA_MINUTES)
}

fn prefs21(state: &AppState) -> Option<Prefs21> {
    Prefs21::locate(state.profile_dir.as_path())
}

/// 启动时那一次：`syncStatus` → `syncCollection`（+ 媒体）。
async fn sync_collection_and_media(state: &AppState, auth: &SyncAuth) {
    match state
        .backend
        .call("syncStatus", auth.encode_to_vec())
        .await
    {
        Ok(_) => tracing::info!("自动同步: 同步状态已取回"),
        Err(error) => {
            tracing::warn!("自动同步: 取同步状态失败: {error}");
            return;
        }
    }
    let request = anki_proto::sync::SyncCollectionRequest {
        auth: Some(auth.clone()),
        sync_media: sync_media_enabled(state),
    };
    match state
        .backend
        .call("syncCollection", request.encode_to_vec())
        .await
    {
        Ok(_) => tracing::info!("自动同步: 集合已同步"),
        Err(error) => tracing::warn!("自动同步: 集合同步失败: {error}"),
    }
}

/// 定时器那一次：只同步媒体（上游 `media_syncer.start(True)`）。
async fn sync_media(state: &AppState, auth: &SyncAuth) {
    if !sync_media_enabled(state) {
        tracing::debug!("自动同步: 用户在设置里关了媒体同步");
        return;
    }
    tracing::info!("自动同步: 开始媒体同步");
    match state
        .backend
        .call("syncMedia", auth.encode_to_vec())
        .await
    {
        Ok(_) => tracing::info!("自动同步: 媒体同步已启动"),
        Err(error) => tracing::warn!("自动同步: 媒体同步失败: {error}"),
    }
}

/// `syncMedia`（`profiles.py:675`，默认 true）。
fn sync_media_enabled(state: &AppState) -> bool {
    prefs21(state)
        .and_then(|prefs| prefs.get(Store::Profile, "syncMedia").ok().flatten())
        .and_then(|value| value.as_bool())
        .unwrap_or(true)
}
