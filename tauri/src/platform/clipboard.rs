// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 剪贴板端点。
//!
//! 对照 mediasrv 的 `read_clipboard` / `write_clipboard`：前端按 MIME 类型
//! 请求原始字节（见 `ts/routes/editor/rich-text-input/data-transfer.ts`）。
//!
//! 限制：`arboard` 只暴露文本与图片两种形态，因此无法像 Qt 那样按任意
//! MIME 类型读取。这里把图片类格式统一映射为 PNG，其余按文本处理。
//! `text/uri-list`（拖入的文件路径列表）暂未支持，见 README。

use std::collections::HashMap;

use anki_proto::frontend::ReadClipboardRequest;
use anki_proto::frontend::ReadClipboardResponse;
use anki_proto::frontend::WriteClipboardRequest;
use prost::Message as _;

use super::HandlerResult;

const IMAGE_PNG: &str = "image/png";
const TEXT_PLAIN: &str = "text/plain";

pub fn read(body: &[u8]) -> HandlerResult {
    let request = ReadClipboardRequest::decode(body).map_err(|e| e.to_string())?;
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;

    let mut data: HashMap<String, Vec<u8>> = HashMap::new();
    for requested in &request.types {
        if requested.starts_with("image/") {
            if let Ok(image) = clipboard.get_image() {
                if let Ok(png) = encode_png(&image) {
                    data.insert(requested.clone(), png);
                }
            }
        } else if let Ok(text) = clipboard.get_text() {
            data.insert(requested.clone(), text.into_bytes());
        }
    }

    Ok(ReadClipboardResponse { data }.encode_to_vec())
}

pub fn write(body: &[u8]) -> HandlerResult {
    let request = WriteClipboardRequest::decode(body).map_err(|e| e.to_string())?;
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;

    // 图片优先：同时写入文本与图片时，QVariant 语义下图片更具体
    if let Some(png) = request.data.get(IMAGE_PNG) {
        let decoded = image::load_from_memory(png).map_err(|e| e.to_string())?;
        let rgba = decoded.to_rgba8();
        let image = arboard::ImageData {
            width: rgba.width() as usize,
            height: rgba.height() as usize,
            bytes: rgba.into_raw().into(),
        };
        clipboard.set_image(image).map_err(|e| e.to_string())?;
        return Ok(Vec::new());
    }

    if let Some(text) = request.data.get(TEXT_PLAIN) {
        clipboard
            .set_text(String::from_utf8_lossy(text).into_owned())
            .map_err(|e| e.to_string())?;
    }

    Ok(Vec::new())
}

fn encode_png(image: &arboard::ImageData<'_>) -> Result<Vec<u8>, String> {
    let rgba = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.to_vec(),
    )
    .ok_or_else(|| "剪贴板图片数据与尺寸不匹配".to_string())?;

    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(
            &mut std::io::Cursor::new(&mut out),
            image::ImageFormat::Png,
        )
        .map_err(|e| e.to_string())?;
    Ok(out)
}
