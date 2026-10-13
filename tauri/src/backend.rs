// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! rslib 后端桥接。
//!
//! Tauri 的 Rust 侧与 rslib 运行在同一进程内，因此**不需要任何 FFI 层**：
//! 直接依赖 `anki` crate 即可。
//!
//! 但要注意一个限制：`SchedulerService` / `CardRenderingService` 等 Collection
//! 侧的服务，在 `impl Backend` 上只生成了**私有的委托方法**（内部委托给
//! `with_col`），外部无法直接调用。因此除少数 Backend 独有方法（如
//! `open_collection`）外，统一走 `run_service_method` + 方法索引 —— 这正是
//! `pylib/rsbridge` 的做法。

use std::sync::Arc;

use anki::backend::init_backend;
use anki::backend::Backend;
use anki::services::BackendCollectionService;
use anki_proto::backend::BackendError;
use anki_proto::backend::BackendInit;
use anki_proto::collection::OpenCollectionRequest;
use prost::Message as _;

use crate::method_table;
use crate::paths::ProfilePaths;

#[derive(Clone)]
pub struct AppBackend {
    inner: Arc<Backend>,
}

impl AppBackend {
    pub fn new(
        preferred_langs: Vec<String>,
        locale_folder_path: String,
        server: bool,
    ) -> anyhow::Result<Self> {
        let init = BackendInit {
            preferred_langs,
            locale_folder_path,
            server,
        };
        let bytes = init.encode_to_vec();
        let backend = init_backend(&bytes).map_err(anyhow::Error::msg)?;
        Ok(Self {
            inner: Arc::new(backend),
        })
    }

    /// 打开（不存在时创建）集合。
    ///
    /// 桌面版由 Python 侧的 `aqt.main` 触发，这里改由外壳直接调用。
    pub fn open_collection(&self, paths: &ProfilePaths) -> anyhow::Result<()> {
        self.inner
            .open_collection(OpenCollectionRequest {
                collection_path: paths.collection.to_string_lossy().into_owned(),
                media_folder_path: paths.media_folder.to_string_lossy().into_owned(),
                media_db_path: paths.media_db.to_string_lossy().into_owned(),
            })
            .map_err(anyhow::Error::from)
    }

    /// 翻译接口（对照 Qt 的 `tr.xxx()`）。
    pub fn i18n(&self) -> &anki_i18n::I18n {
        self.inner.i18n()
    }

    /// 读集合配置里的一个键（缺键或类型不符返回 `None`）。
    ///
    /// 集合配置本身很小，一次 `getAllConfig` 取回再挑键即可。**不要在异步上下文里
    /// 用 `platform::config` 的那套**：那边是同步的 `block_on`，会 panic。
    pub async fn collection_config(&self, key: &str) -> Option<serde_json::Value> {
        let bytes = self
            .call("getAllConfig", anki_proto::generic::Empty {}.encode_to_vec())
            .await
            .ok()?;
        let raw = anki_proto::generic::Json::decode(bytes.as_slice()).ok()?;
        let value: serde_json::Value = serde_json::from_slice(&raw.json).ok()?;
        value.get(key).cloned()
    }

    /// 按 camelCase 方法名调用 rslib 服务，错误已解码为可读文本。
    pub async fn call(&self, method: &str, input: Vec<u8>) -> Result<Vec<u8>, String> {
        let (service, index) = method_table::lookup(method)
            .ok_or_else(|| format!("未注册的接口: {method}"))?;
        self.run(service, index, input)
            .await
            .map_err(|bytes| decode_error(&bytes))
    }

    /// 调用 rslib 的服务分发入口。
    ///
    /// `run_service_method` 是**阻塞**调用（Python 绑定必须用 `py.detach`
    /// 释放 GIL 才不会卡住解释器），所以这里必须交给阻塞线程池，
    /// 否则会阻塞 tokio 运行时并导致 WebView 请求全部排队。
    ///
    /// 错误保持为已编码的 `BackendError` protobuf，便于原样回传给前端。
    pub async fn run(
        &self,
        service: u32,
        method: u32,
        input: Vec<u8>,
    ) -> Result<Vec<u8>, Vec<u8>> {
        let backend = self.inner.clone();
        match tokio::task::spawn_blocking(move || {
            backend.run_service_method(service, method, &input)
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(b"backend task panicked".to_vec()),
        }
    }
}

/// 把 rslib 返回的错误字节解码为可读文本。
fn decode_error(bytes: &[u8]) -> String {
    BackendError::decode(bytes)
        .map(|error| error.message)
        .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned())
}
