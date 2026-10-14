// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 导入入口（对照 `qt/aqt/import_export/importing.py`）。
//!
//! 上游有两条入口，最后都落到 `import_file()`（`importing.py:137-146`）：**按文件名**
//! 挑一个 importer，再开对应的窗口（或直接干活）。这里把两条都做成宿主侧的动作：
//!
//! - 「导入文件」按钮 / 「文件 → 导入」：`prompt_and_import()` —— 先弹原生文件对话框
//!   （对照 `prompt_for_file_then_import()`，`importing.py:148-163`）；
//! - 把文件**拖到主窗口**：`on_drop()` —— 对照 `main.py:112-137` 的 `dragEnterEvent` /
//!   `dropEvent`（接受条件与 `.colpkg` 过滤在 `drop_path()` 里）。
//!
//! | 文件 | 上游 | 本外壳 |
//! | --- | --- | --- |
//! | `*.apkg` / `*.zip` | `ApkgImporter` → `ImportDialog(AnkiPackageArgs(path))` | 开 `import-anki-package/<路径>` 窗口 |
//! | `*.csv` / `*.tsv` / `*.txt` | `CsvImporter` → `CsvArgs(path)` | 开 `import-csv/<路径>` |
//! | `*.colpkg` / `collection.apkg` / `backup-*.apkg` | `ColpkgImporter`：确认后**整体替换集合**（备份 + unload 集合 + 导入 + reload） | **未实现**：只提示（见 `tauri/PARITY.md` 3.4） |
//! | `*.db`（Mnemosyne 2.0） | `MnemosyneImporter`：先转成 JSON 再开 `import-page` | **未实现**（要 `mnemosyne.serialize` + 临时文件） |
//! | 其他 | `importing.py:145` 的 "Unsupported file type." | `importing-unable-to-import-filename`（这条 ftl 是现成的） |
//!
//! 页面由 `ImportDialog` 打开（模态），真正的导入在页面里发生：`.apkg` 走
//! `ImportAnkiPackagePage.svelte:67` 的 `importAnkiPackage({ packagePath, options })`，
//! 路径就是 URL 里那一段（`import_dialog.py:68` 的 `f"{ts_page}/{quote(path)}"`，
//! 路由是 catch-all `[...path]`）。完成后页面发 `importDone`，宿主把模态解除
//! （`platform/mod.rs`，对照 `mediasrv.import_done`）。

use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use anki_i18n::tr_args;
use anyhow::Result;

use crate::platform::dialogs;
use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// 文件对话框里列出的扩展名（对照 `all_accepted_file_endings()`，`importing.py:166-171`：
/// `IMPORTERS` 里各家的 `accepted_file_endings` 并集）。
const ACCEPTED_ENDINGS: [&str; 7] = [".apkg", ".colpkg", ".zip", ".db", ".csv", ".tsv", ".txt"];

/// 一次导入的落点（`importing.py:129-134` 的 `IMPORTERS` 表）。
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// 开对应页面（`ImportDialog`）：路由首段见 `target_for_path`。
    Page(&'static str),
    /// 整体替换集合（`ColpkgImporter`）—— 本外壳还没实现。
    CollectionPackage,
    /// Mnemosyne `.db`：上游先转 JSON 再开 `import-page` —— 本外壳还没实现。
    Mnemosyne,
    /// 没有 importer 认领（`importing.py:145`）。
    Unsupported,
}

/// 按文件名挑 importer（对照 `importing.py:137-146` 的 `import_file`）。
pub fn target_for_path(path: &Path) -> Target {
    let filename = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // `ColpkgImporter.can_import`（`importing.py:46-52`）—— 这三类文件在 Qt 里会**替换
    // 整个集合**，不是导入进当前集合
    if filename == "collection.apkg"
        || (filename.starts_with("backup-") && filename.ends_with(".apkg"))
        || filename.ends_with(".colpkg")
    {
        return Target::CollectionPackage;
    }
    // `ApkgImporter.accepted_file_endings = [".apkg", ".zip"]`（`importing.py:88`）
    if filename.ends_with(".apkg") || filename.ends_with(".zip") {
        return Target::Page("import-anki-package");
    }
    // `CsvImporter.accepted_file_endings = [".csv", ".tsv", ".txt"]`（`importing.py:114`）
    if filename.ends_with(".csv") || filename.ends_with(".tsv") || filename.ends_with(".txt") {
        return Target::Page("import-csv");
    }
    // `MnemosyneImporter.accepted_file_endings = [".db"]`（`importing.py:96`）
    if filename.ends_with(".db") {
        return Target::Mnemosyne;
    }
    Target::Unsupported
}

/// 拖入的一组路径 → 真正要导入的那一个（对照 `main.py:120-131`）：
/// 只收**存在的文件**（目录不算，`main.py:120-121`），`.colpkg` 被显式过滤掉
/// （`main.py:131`），而且一次只导一个（`main.py:137` 的 `return`）。
pub fn drop_path(paths: &[PathBuf]) -> Option<PathBuf> {
    paths
        .iter()
        .find(|path| path.is_file() && !is_collection_package(path))
        .cloned()
}

/// `main.py:131` 只过滤了 `.colpkg`；`collection.apkg` / `backup-*.apkg` 会穿过这层
/// 过滤器，落到 `ColpkgImporter`（`can_import` 是大小写不敏感的，`importing.py:44-52`）。
fn is_collection_package(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    name.ends_with(".colpkg")
}

/// 路径 → 路由（对照 `import_dialog.py:68` 的 `f"{ts_page}/{quote(path)}"`）。
///
/// `quote()` 默认不动 `/`，其余按百分号编码（Windows 的盘符与反斜杠也因此编成
/// `C%3A%5C...`）。SvelteKit 的 catch-all 路由 `[...path]` 会把每一段解码回来。
fn route_for(target: &Target, path: &Path) -> Option<String> {
    let Target::Page(page) = target else {
        return None;
    };
    Some(format!("{page}/{}", encode_path(&path.to_string_lossy())))
}

/// 只保留 URL 里安全的那批字符，其余百分号编码（`urllib.parse.quote` 的默认行为）。
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.bytes() {
        let ch = byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~' | '/') {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// 「导入文件」按钮 / 「文件 → 导入」：先弹文件对话框，选到文件才继续
/// （对照 `prompt_for_file_then_import()`，`importing.py:148-163`）。
pub fn prompt_and_import(state: &AppState) -> Result<()> {
    let i18n = state.backend.i18n();
    let endings = ACCEPTED_ENDINGS
        .iter()
        .map(|ending| format!("*{ending}"))
        .collect::<Vec<_>>()
        .join(" ");
    let description = i18n
        .translate(
            "importing-all-supported-formats",
            Some(tr_args! { "val" => format!("({endings})") }),
        )
        .to_string();
    let extensions: Vec<String> = ACCEPTED_ENDINGS
        .iter()
        .map(|ending| ending.trim_start_matches('.').to_owned())
        .collect();
    let title = i18n.translate("actions-import", None).to_string();

    let Some(path) = dialogs::pick_file(&title, &extensions, &description) else {
        tracing::debug!("导入：用户在文件对话框里取消了");
        return Ok(());
    };
    open_for_path(state, &path)
}

/// 主窗口拖入文件（对照 `main.py:112-137`）。
pub fn on_drop(state: &AppState, paths: &[PathBuf]) -> Result<()> {
    // Qt：`state != "deckBrowser"` 时直接忽略（`main.py:113` / `127`）。本外壳只有
    // 「主窗口在复习界面吗」这一个标志，所以概览也接受拖入（差异见 `tauri/README.md`）
    if state.reviewer_active.load(Ordering::SeqCst) {
        tracing::info!("主窗口正在复习，忽略拖入的 {} 个文件", paths.len());
        return Ok(());
    }
    let Some(path) = drop_path(paths) else {
        tracing::info!("拖入的文件里没有可导入的（{} 个）", paths.len());
        return Ok(());
    };
    tracing::info!("主窗口拖入文件: {}", path.display());
    open_for_path(state, &path)
}

/// 打开某个路径对应的导入界面（含"这两类还没做"的提示）。
pub fn open_for_path(state: &AppState, path: &Path) -> Result<()> {
    let target = target_for_path(path);
    let i18n = state.backend.i18n();
    match &target {
        Target::CollectionPackage => {
            // Qt 这时会问一句「这会删除你现有的集合」然后整体替换（`importing.py:54-84`）。
            // 我们还没有"关掉集合 → 换文件 → 重新打开"这套流程，所以只提示
            tracing::warn!("暂不支持整体替换集合的 .colpkg / collection.apkg: {}", path.display());
            dialogs::warn(
                &i18n
                    .translate("importing-importing-collection", None)
                    .to_string(),
            );
            return Ok(());
        }
        Target::Mnemosyne => {
            tracing::warn!("暂不支持 Mnemosyne 的 .db 导入: {}", path.display());
            dialogs::warn(
                &i18n
                    .translate(
                        "importing-unable-to-import-filename",
                        Some(tr_args! { "filename" => file_name(path) }),
                    )
                    .to_string(),
            );
            return Ok(());
        }
        Target::Unsupported => {
            tracing::warn!("无法识别的导入文件类型: {}", path.display());
            dialogs::warn(
                &i18n
                    .translate(
                        "importing-unable-to-import-filename",
                        Some(tr_args! { "filename" => file_name(path) }),
                    )
                    .to_string(),
            );
            return Ok(());
        }
        Target::Page(_) => {}
    }

    let Some(route) = route_for(&target, path) else {
        return Ok(());
    };
    tracing::info!("打开导入窗口: {route}");
    windows::open(
        state,
        &WindowSpec::new(windows::IMPORT, route)
            // 标题取 Qt 的 `import_dialog.py:75`（`tr.decks_import_file()`）
            .title(i18n.translate("decks-import-file", None))
            .size(900.0, 640.0)
            .modal(true),
    )
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod test {
    use super::*;

    fn page(path: &str) -> Option<String> {
        match target_for_path(Path::new(path)) {
            Target::Page(page) => Some(page.to_owned()),
            _ => None,
        }
    }

    #[test]
    fn importer_by_filename() {
        // ApkgImporter
        assert_eq!(page("deck.apkg").as_deref(), Some("import-anki-package"));
        assert_eq!(page("DECK.APKG").as_deref(), Some("import-anki-package"));
        assert_eq!(page("deck.zip").as_deref(), Some("import-anki-package"));
        // CsvImporter
        assert_eq!(page("notes.csv").as_deref(), Some("import-csv"));
        assert_eq!(page("notes.tsv").as_deref(), Some("import-csv"));
        assert_eq!(page("notes.txt").as_deref(), Some("import-csv"));
        // ColpkgImporter：整体替换集合，本外壳还没做
        assert_eq!(
            target_for_path(Path::new("collection.apkg")),
            Target::CollectionPackage
        );
        assert_eq!(
            target_for_path(Path::new("backup-2020-01-01.apkg")),
            Target::CollectionPackage
        );
        assert_eq!(
            target_for_path(Path::new("my.colpkg")),
            Target::CollectionPackage
        );
        // MnemosyneImporter / 其他
        assert_eq!(target_for_path(Path::new("deck.db")), Target::Mnemosyne);
        assert_eq!(target_for_path(Path::new("readme.md")), Target::Unsupported);
        assert_eq!(target_for_path(Path::new("noext")), Target::Unsupported);
    }

    #[test]
    fn drop_keeps_files_and_skips_colpkg() {
        let dir = std::env::temp_dir();
        let file = dir.join("tauri-drop-test.tauri");
        std::fs::write(&file, b"x").expect("写测试文件失败");
        let colpkg = dir.join("tauri-drop-test.colpkg");
        std::fs::write(&colpkg, b"x").expect("写测试文件失败");
        let missing = dir.join("tauri-drop-missing.tauri");

        // 目录与不存在的文件都不算（`main.py:120-121`）
        assert_eq!(drop_path(&[dir.clone()]), None);
        assert_eq!(drop_path(&[missing.clone()]), None);
        // `.colpkg` 被过滤（`main.py:131`），后面那个文件顶上
        assert_eq!(drop_path(&[colpkg.clone(), file.clone()]), Some(file.clone()));
        assert_eq!(drop_path(&[colpkg.clone()]), None);

        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&colpkg);
    }

    #[test]
    fn route_encodes_path_like_qt() {
        // `quote()` 不动 `/`，其余按百分号编码（Windows 的 `C:\x y\a.apkg`）
        let path = Path::new("x.apkg");
        let target = target_for_path(path);
        assert_eq!(
            route_for(&target, Path::new("C:\\Users\\me\\my deck.apkg")).as_deref(),
            Some("import-anki-package/C%3A%5CUsers%5Cme%5Cmy%20deck.apkg")
        );
        // `+` 要编码（`quote` 的默认 safe 里没有它）；绝对路径开头的 `/` 会多出一个
        // 斜杠 —— 与 Qt 的 `f"{ts_page}/{quote(path)}"` 完全一样（Windows 上路径以盘符
        // 开头，所以实际不会出现这种情况）
        assert_eq!(
            route_for(&target, Path::new("/tmp/a+b.apkg")).as_deref(),
            Some("import-anki-package//tmp/a%2Bb.apkg")
        );
        // 非页面落点没有路由
        assert_eq!(route_for(&Target::CollectionPackage, path), None);
    }
}
