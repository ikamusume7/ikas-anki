// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use tauri::AppHandle;

use crate::backend::AppBackend;
use crate::paths::AppPaths;
use crate::reviewer::Reviewer;

/// 在 HTTP 处理器与窗口之间共享的应用状态。
#[derive(Clone)]
pub struct AppState {
    /// Tauri 应用句柄，用于桥接命令（例如关闭窗口）。
    pub app: AppHandle,
    pub backend: Arc<AppBackend>,
    pub paths: Arc<AppPaths>,
    /// 当前 profile 目录（存放 profile/meta 配置）。
    pub profile_dir: Arc<PathBuf>,
    /// 集合的媒体目录；集合打开后才有值。
    pub media_folder: Option<Arc<PathBuf>>,
    /// 本机 API 密钥。内部请求需携带 `Authorization: Bearer <key>`，
    /// 对照 mediasrv 的 `_APIKEY` / `_have_api_access`。
    pub api_key: Arc<str>,
    /// 本地服务端口（构造 CSP 时需要）。
    port: Arc<AtomicU16>,
    /// 集合是否已打开，供 `/_anki/readyz` 使用。
    collection_open: Arc<AtomicBool>,
    /// 复习会话状态。
    pub reviewer: Arc<Reviewer>,
    /// 当前窗口是否承载复习界面；决定 `domDone` 是否触发复习。
    pub reviewer_active: Arc<AtomicBool>,
}

impl AppState {
    pub fn new(
        app: AppHandle,
        backend: AppBackend,
        paths: AppPaths,
        profile_dir: PathBuf,
        media_folder: Option<PathBuf>,
    ) -> Self {
        Self {
            app,
            backend: Arc::new(backend),
            paths: Arc::new(paths),
            profile_dir: Arc::new(profile_dir),
            media_folder: media_folder.map(Arc::new),
            api_key: Arc::from(generate_api_key()),
            port: Arc::new(AtomicU16::new(0)),
            collection_open: Arc::new(AtomicBool::new(false)),
            reviewer: Arc::new(Reviewer::default()),
            reviewer_active: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn set_port(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    pub fn mark_collection_open(&self) {
        self.collection_open.store(true, Ordering::SeqCst);
    }

    pub fn is_collection_open(&self) -> bool {
        self.collection_open.load(Ordering::SeqCst)
    }
}

/// 对照 mediasrv 的 `secrets.token_urlsafe(32)`：32 字节熵，十六进制表示。
///
/// `ANKI_API_KEY` 可覆盖该值，仅用于调试与端到端测试。
fn generate_api_key() -> String {
    if let Ok(key) = std::env::var("ANKI_API_KEY") {
        if !key.is_empty() {
            tracing::info!("已启用 ANKI_API_KEY 提供的密钥（长度 {}）", key.len());
            return key;
        }
    }

    tracing::info!("已随机生成本机 API 密钥");
    let bytes: [u8; 32] = rand::random();
    let mut key = String::with_capacity(64);
    for byte in bytes {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}
