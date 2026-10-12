// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 编辑器媒体相关端点。
//!
//! 对照 mediasrv 的 `convert_pasted_image` / `open_media` / `show_in_media_folder`
//! （原实现基于 Qt 的 QImage 与 QDesktopServices）。

use anki_proto::frontend::ConvertPastedImageRequest;
use anki_proto::frontend::ConvertPastedImageResponse;
use anki_proto::generic::String as GenericString;
use prost::Message as _;

use super::HandlerResult;
use crate::state::AppState;

/// 对照 mediasrv：非 png 格式使用 80 的质量。
///
/// Qt 对 png 传入的 "quality" 实际是压缩级别，这里改用 image crate 的默认压缩，
/// 属于可接受的近似。
const OTHER_QUALITY: u8 = 80;

pub fn convert_pasted_image(body: &[u8]) -> HandlerResult {
    let request = ConvertPastedImageRequest::decode(body).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&request.data).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    if request.ext == "png" {
        image
            .write_to(
                &mut std::io::Cursor::new(&mut out),
                image::ImageFormat::Png,
            )
            .map_err(|e| e.to_string())?;
    } else {
        // Cursor 需要绑定到变量，否则其借用会在语句结束时失效
        let mut cursor = std::io::Cursor::new(&mut out);
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut cursor, OTHER_QUALITY);
        encoder.encode_image(&image).map_err(|e| e.to_string())?;
    }

    Ok(ConvertPastedImageResponse { data: out }.encode_to_vec())
}

/// 在文件管理器中定位媒体文件（对应 mediasrv 的 `open_media`）。
pub fn open_media(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    let folder = media_folder(state)?;
    let path = folder.join(&request.val);

    // 文件不存在时退化为打开媒体目录本身
    if path.exists() {
        opener::reveal(&path).map_err(|e| e.to_string())?;
    } else {
        opener::open(&folder).map_err(|e| e.to_string())?;
    }
    Ok(Vec::new())
}

/// 对应 mediasrv 的 `show_in_media_folder`。
pub fn show_in_media_folder(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    let folder = media_folder(state)?;
    opener::reveal(folder.join(&request.val)).map_err(|e| e.to_string())?;
    Ok(Vec::new())
}

fn media_folder(state: &AppState) -> Result<std::path::PathBuf, String> {
    state
        .media_folder
        .as_ref()
        .map(|folder| folder.as_ref().clone())
        .ok_or_else(|| "集合尚未打开".to_string())
}
