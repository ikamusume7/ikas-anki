// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 应用运行期需要访问的资源路径。
//!
//! rslib 不负责决定数据目录，路径总是由上层通过 protobuf 传入。
//! TODO(Android): `user_data` 需要改为 `getFilesDir()`。

use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use tauri::AppHandle;
use tauri::Manager as _;

use crate::platform::prefs21::Prefs21;
use crate::platform::prefs21::Store;

/// 资源根目录与用户数据目录。
#[derive(Clone, Debug)]
pub struct AppPaths {
    /// 构建产物目录：`qt/_aqt/data/web`
    ///
    /// 注意是 `_aqt`（下划线）—— 构建系统把 `ts/` 与 `qt/aqt/data/web` 的
    /// 产物汇总到这里，运行时实际读取的就是它。
    pub web: PathBuf,
    /// 源目录 `qt/aqt/data/web`，仅在未构建时作为回退。
    pub web_source: Option<PathBuf>,
    /// 前端构建产物的原始位置（仅开发期回退使用）
    pub sveltekit: PathBuf,
    /// 用户数据目录（profile 与 collection）
    pub user_data: PathBuf,
}

/// 一个 profile（用户配置）对应的集合与媒体路径。
#[derive(Clone, Debug)]
pub struct ProfilePaths {
    /// profile 目录（存放 profile / meta 配置）
    pub dir: PathBuf,
    pub collection: PathBuf,
    pub media_folder: PathBuf,
    pub media_db: PathBuf,
}

impl AppPaths {
    /// 解析资源路径。
    ///
    /// - **仓库模式**（在源码树内运行）：直接用源目录，便于查看前端产物，
    ///   无需每次先跑打包流程。
    /// - **打包模式**：使用 Tauri 的资源目录。
    ///
    /// `ANKI_RESOURCE_DIR` 可以**强制**打包模式并指定资源根目录 —— 用来验证打出来的
    /// 目录/安装包里资源是否齐全（在源码树里跑 release 二进制时，`repo_root()` 仍然
    /// 命中，所以光靠默认逻辑没法测打包布局）。见 `tauri/tools/verify-package.ps1`。
    pub fn resolve(app: &AppHandle) -> Result<Self> {
        if let Ok(root) = std::env::var("ANKI_RESOURCE_DIR") {
            if !root.is_empty() {
                tracing::info!("以打包模式运行（ANKI_RESOURCE_DIR）: {root}");
                return Ok(Self::for_resource_dir(PathBuf::from(root)));
            }
        }
        if let Some(root) = repo_root() {
            // 用 `proto/anki` 是否存在作为「这是源码树」的判据
            if root.join("proto/anki").is_dir() {
                tracing::info!("以仓库模式运行，资源目录: {}", root.display());
                return Ok(Self::for_repo(root));
            }
        }

        let resource_dir = app.path().resource_dir().context("无法定位资源目录")?;
        tracing::info!("以打包模式运行，资源目录: {}", resource_dir.display());
        Ok(Self::for_resource_dir(resource_dir))
    }

    /// 用户数据目录（`<base>`）。`ANKI_BASE` 可以覆盖它 —— 对照 aqt 的 `--base`，
    /// 用来把整个 base（`prefs21.db` + 各 profile 目录）挪到隔离位置，
    /// 这样**不设 `ANKI_COLLECTION`** 也能测「按 `last_loaded_profile_name` 选 profile」
    /// （`ANKI_COLLECTION` 的优先级更高，会把子进程钉在同一个集合上，见 `profile()`）。
    fn user_data_dir() -> PathBuf {
        match std::env::var("ANKI_BASE") {
            Ok(base) if !base.is_empty() => {
                tracing::info!("ANKI_BASE 生效，用户数据目录: {base}");
                PathBuf::from(base)
            }
            _ => default_user_data_dir(),
        }
    }

    fn for_repo(root: PathBuf) -> Self {
        Self {
            web: root.join("qt/_aqt/data/web"),
            web_source: Some(root.join("qt/aqt/data/web")),
            sveltekit: root.join("out/sveltekit"),
            user_data: Self::user_data_dir(),
        }
    }

    /// 打包后的布局：资源位于 `<resources>/web` 与 `<resources>/sveltekit`。
    fn for_resource_dir(root: PathBuf) -> Self {
        Self {
            web: root.join("web"),
            web_source: None,
            sveltekit: root.join("sveltekit"),
            user_data: Self::user_data_dir(),
        }
    }

    /// 解析要打开的集合路径。
    ///
    /// 优先级（对照 `main.py:296-319` 的 `setupProfile()`）：
    /// 1. `ANKI_COLLECTION` —— 直接指定集合文件（测试常用），profile 目录取它的父目录；
    /// 2. `ANKI_PROFILE` —— 指定 profile 名（测试用，避免动 `prefs21.db`）；
    /// 3. `prefs21.db` 的 `meta["last_loaded_profile_name"]` —— 桌面版/上次用的那个；
    /// 4. `User 1`。
    ///
    /// 若集合文件不存在，rslib 会创建一个空集合。
    pub fn profile(&self) -> ProfilePaths {
        if let Ok(path) = std::env::var("ANKI_COLLECTION") {
            if !path.is_empty() {
                let collection = PathBuf::from(path);
                let dir = collection
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.user_data.clone());
                return ProfilePaths::in_dir(dir, collection);
            }
        }

        let name = std::env::var("ANKI_PROFILE")
            .ok()
            .filter(|name| !name.trim().is_empty())
            .or_else(|| self.stored_profile_name())
            .unwrap_or_else(|| "User 1".to_owned());
        let dir = self.user_data.join(name);
        let collection = dir.join("collection.anki2");
        ProfilePaths::in_dir(dir, collection)
    }

    /// `meta["last_loaded_profile_name"]`（`ProfileManager` 记的「上次用的配置」）。
    fn stored_profile_name(&self) -> Option<String> {
        let prefs = Prefs21::for_meta(&self.user_data);
        match prefs.get(Store::Meta, "last_loaded_profile_name") {
            Ok(Some(serde_json::Value::String(name))) if !name.trim().is_empty() => Some(name),
            _ => None,
        }
    }
}

impl ProfilePaths {
    /// 由 profile 目录推出集合 / 媒体路径。
    fn in_dir(dir: PathBuf, collection: PathBuf) -> Self {
        Self {
            media_folder: dir.join("collection.media"),
            media_db: dir.join("collection.media.db2"),
            collection,
            dir,
        }
    }
}

/// 源码树根目录；从可执行文件位置推断，仅在 dev 布局下成立。
fn repo_root() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Some(manifest_dir.parent()?.to_path_buf())
}

/// 与 aqt 的 profile 目录保持一致（`%APPDATA%/Anki2`）。
fn default_user_data_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Anki2");
        }
        return PathBuf::from("Anki2");
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME")
            .map(|home| PathBuf::from(home).join(".local/share/Anki2"))
            .unwrap_or_else(|_| PathBuf::from("Anki2"))
    }
}
