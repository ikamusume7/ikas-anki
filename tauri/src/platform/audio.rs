// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 音频播放。
//!
//! 对照 mediasrv 的 `play_file`（原实现基于 Qt 的 mpv）。
//!
//! 设计：`rodio` 的 `OutputStream` 不是 `Send`，无法存放在全局静态变量里，
//! 因此这里用一个常驻音频线程持有流与 Sink，通过 channel 接收播放请求。
//! 这样也天然实现了「新播放替换旧播放」的语义（与 Qt 版本一致）。

use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::OnceLock;

use anki_proto::generic::String as GenericString;
use prost::Message as _;

use super::HandlerResult;
use crate::state::AppState;

static COMMANDS: OnceLock<mpsc::Sender<PathBuf>> = OnceLock::new();

/// 对应 mediasrv 的 `play_file`：请求播放集合媒体目录下的音频。
pub fn play_file(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    let folder = state
        .media_folder
        .as_ref()
        .ok_or_else(|| "集合尚未打开".to_string())?;

    let path = folder.join(&request.val);
    if !path.exists() {
        return Err(format!("找不到媒体文件: {}", request.val));
    }

    // 播放异步进行：前端调用后立即返回，不等待播放结束
    if commands().send(path).is_err() {
        return Err("音频线程已退出".to_string());
    }
    Ok(Vec::new())
}

fn commands() -> &'static mpsc::Sender<PathBuf> {
    COMMANDS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        std::thread::Builder::new()
            .name("anki-audio".into())
            .spawn(move || audio_loop(rx))
            .expect("无法启动音频线程");
        tx
    })
}

fn audio_loop(rx: mpsc::Receiver<PathBuf>) {
    // `_stream` 必须在整个循环期间存活，drop 后音频会停止
    let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
        tracing::error!("无法初始化音频输出设备，播放功能不可用");
        return;
    };
    let Ok(sink) = rodio::Sink::try_new(&handle) else {
        tracing::error!("无法创建音频 sink");
        return;
    };

    while let Ok(path) = rx.recv() {
        // 与 Qt 版本一致：新播放替换当前播放
        sink.stop();
        match open_source(&path) {
            Ok(source) => sink.append(source),
            Err(error) => tracing::warn!("播放失败 {}: {error}", path.display()),
        }
    }
}

fn open_source(path: &Path) -> Result<rodio::Decoder<std::io::BufReader<std::fs::File>>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())
}
