// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 还原备份（`main.py:441-477` 的 `onOpenBackup`，菜单「文件 → 还原备份」）。
//!
//! 上游的流程：
//!
//! 1. `getFile(filter="*.colpkg", dir=self.pm.backupFolder())` 选一个备份；
//! 2. `askUser(...replace-your-collection-with-an-earlier2, defaultno=True)` 确认；
//! 3. `unloadProfile()`（**关集合**）→ `import_collection_package` → `loadProfile()`。
//!
//! 本外壳的集合由 Rust 侧持有，所以三件事都在宿主里做：`closeCollection` →
//! `importCollectionPackage` → `openCollection`，然后关掉其它窗口、主窗口回牌组列表。
//! `import_collection_package` 要求集合**已经关闭**（`rslib/src/backend/import_export.rs:28`
//! 的 `lock_closed_collection`），所以顺序不能反。
//!
//! 测试钩子：`ANKI_RESTORE_PATH` 设了就跳过「选文件 + 确认」两步（原生对话框没法自动点，
//! 与 `ANKI_SAVE_PATH` / `ANKI_OPEN_PATH` 同一套办法）。值可以是路径，也可以是
//! `newest`（= 备份目录里最新的 `backup-*.colpkg`）。

use std::path::Path;
use std::path::PathBuf;

use anki_i18n::tr_args;
use anki_proto::collection::CloseCollectionRequest;
use anki_proto::import_export::ImportCollectionPackageRequest;
use prost::Message as _;

use crate::platform::dialogs;
use crate::state::AppState;
use crate::windows;

/// 备份目录（与桌面版一致：`<profile>/backups`）。
pub fn backup_folder(state: &AppState) -> PathBuf {
    crate::backup::backup_folder(&state.profile_dir)
}

/// 备份目录里最新的 `backup-*.colpkg`。
fn newest_backup(state: &AppState) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(backup_folder(state))
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "colpkg"))
        .collect();
    files.sort_by_key(|path| path.metadata().and_then(|meta| meta.modified()).ok());
    files.pop()
}

/// 测试钩子给出的备份路径（没设钩子返回 `None`）。
fn hooked_path(state: &AppState) -> Option<PathBuf> {
    let hook = std::env::var("ANKI_RESTORE_PATH").ok()?;
    if hook.is_empty() {
        return None;
    }
    if hook == "newest" {
        let newest = newest_backup(state);
        tracing::info!("ANKI_RESTORE_PATH=newest 命中: {newest:?}");
        return newest;
    }
    tracing::info!("ANKI_RESTORE_PATH 生效，跳过文件对话框与确认框: {hook}");
    Some(PathBuf::from(hook))
}

/// 「文件 → 还原备份」：选文件（默认落在备份目录）→ 确认 → 还原。
pub async fn prompt_and_restore(state: &AppState) -> anyhow::Result<()> {
    let hooked = hooked_path(state);
    let title = state
        .backend
        .i18n()
        .translate("qt-accel-load-backup", None)
        .replace('&', "");
    let picked = match &hooked {
        Some(path) => Some(path.clone()),
        None => dialogs::pick_file_in(
            &title,
            &["colpkg".to_string()],
            "Anki Collection Package",
            Some(&backup_folder(state)),
        ),
    };
    let Some(backup) = picked else {
        tracing::info!("还原备份：用户取消了文件对话框");
        return Ok(());
    };
    if hooked.is_none() {
        let name = backup
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let message = state.backend.i18n().translate(
            "qt-misc-replace-your-collection-with-an-earlier2",
            Some(tr_args!["val" => name]),
        );
        if !dialogs::confirm(&message) {
            tracing::info!("还原备份：用户在确认框里选了否");
            return Ok(());
        }
    }
    restore_from(state, &backup).await
}

/// 关集合 → `importCollectionPackage` → 重开 → 回牌组列表。
async fn restore_from(state: &AppState, backup: &Path) -> anyhow::Result<()> {
    let profile = state.paths.profile();
    tracing::info!(
        "还原备份: {} → {}",
        backup.display(),
        profile.collection.display()
    );

    // 上游 `unloadProfile()`：先关集合
    state
        .backend
        .call(
            "closeCollection",
            CloseCollectionRequest::default().encode_to_vec(),
        )
        .await
        .map_err(|error| anyhow::anyhow!("关闭集合失败: {error}"))?;

    let request = ImportCollectionPackageRequest {
        backup_path: backup.to_string_lossy().into_owned(),
        col_path: profile.collection.to_string_lossy().into_owned(),
        media_folder: profile.media_folder.to_string_lossy().into_owned(),
        media_db: profile.media_db.to_string_lossy().into_owned(),
    };
    let imported = state
        .backend
        .call("importCollectionPackage", request.encode_to_vec())
        .await;

    // 成败都要把集合开回来（上游 `loadProfile()`），否则整个外壳就废了
    let reopened = state.backend.open_collection(&profile);
    match (imported, reopened) {
        (Ok(_), Ok(())) => {
            tracing::info!("备份已还原，集合已重新打开");
            after_restore(state);
            Ok(())
        }
        (Err(error), Ok(())) => Err(anyhow::anyhow!("导入备份失败: {error}")),
        (Ok(_), Err(error)) => Err(anyhow::anyhow!("还原后重新打开集合失败: {error:#}")),
        (Err(error), Err(reopen_error)) => Err(anyhow::anyhow!(
            "导入备份失败（{error}），且重新打开集合也失败（{reopen_error:#}）"
        )),
    }
}

/// 还原之后收尾：关掉除主窗口以外的窗口（上游 `unloadProfile` 会把它们一起收掉），
/// 主窗口回牌组列表。
fn after_restore(state: &AppState) {
    for label in windows::open_labels(state) {
        if label != windows::MAIN {
            windows::close(state, &label);
        }
    }
    if let Err(error) = crate::main_window::show_decks(state) {
        tracing::warn!("还原后回到牌组列表失败: {error:#}");
    }
}
