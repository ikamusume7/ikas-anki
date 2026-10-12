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

/// PNG 的 zlib 压缩级别，对齐 Qt 的**实际**行为。
///
/// Qt 侧传给 `image.save()` 的是 `quality`（png 50、其它 80），但对 PNG 而言
/// 那个 50 *不会生效*：`QImageWriter` 只设置了 `Quality`，而 `QPngHandlerPrivate`
/// 的 `compression` 默认值就是 50（≥0），`write_png_image()` 会优先走 compression
/// 分支，把它按 `(50 * 9) / 91 = 4` 映射成 **zlib level 4**（Qt6
/// `qpnghandler.cpp`）。所以这里显式指定 level 4。
///
/// 注意 `image` 的 PNG 默认档位是 `Fast`（不等于 level 4），不能依赖默认值。
const PNG_ZLIB_LEVEL: u8 = 4;

/// 非 png 格式的质量，对照 mediasrv 的 80（JPEG 的 quality 语义是直接对应的）。
const OTHER_QUALITY: u8 = 80;

pub fn convert_pasted_image(body: &[u8]) -> HandlerResult {
    let request = ConvertPastedImageRequest::decode(body).map_err(|e| e.to_string())?;
    let image = image::load_from_memory(&request.data).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    if request.ext == "png" {
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            std::io::Cursor::new(&mut out),
            image::codecs::png::CompressionType::Level(PNG_ZLIB_LEVEL),
            // Qt 走的是 libpng 默认的自适应过滤
            image::codecs::png::FilterType::Adaptive,
        );
        image.write_with_encoder(encoder).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_png(
        image: image::DynamicImage,
        compression: image::codecs::png::CompressionType,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            std::io::Cursor::new(&mut out),
            compression,
            image::codecs::png::FilterType::Adaptive,
        );
        image.write_with_encoder(encoder).expect("编码失败");
        out
    }

    /// 渐变测试图：纯色在任意档位下都极小，测不出压缩差异
    fn sample_png() -> Vec<u8> {
        let (width, height) = (64u32, 64u32);
        let mut raw = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                raw.push((x * 4) as u8);
                raw.push((y * 4) as u8);
                raw.push(((x + y) * 2) as u8);
            }
        }
        let image = image::RgbImage::from_raw(width, height, raw).expect("构造测试图失败");
        encode_png(
            image::DynamicImage::ImageRgb8(image),
            image::codecs::png::CompressionType::Fast,
        )
    }

    /// 走真实端点：请求 → 编码 → 响应
    fn convert(data: &[u8], ext: &str) -> Vec<u8> {
        let request = ConvertPastedImageRequest {
            data: data.to_vec(),
            ext: ext.to_string(),
        };
        let reply = convert_pasted_image(&request.encode_to_vec()).expect("转换失败");
        ConvertPastedImageResponse::decode(reply.as_slice())
            .expect("解析响应失败")
            .data
    }

    #[test]
    fn png_is_lossless_and_uses_level_4() {
        let input = sample_png();
        let converted = convert(&input, "png");
        assert_eq!(&converted[..8], b"\x89PNG\r\n\x1a\n", "输出应当是 PNG");

        // 无损：像素必须完全一致
        let before = image::load_from_memory(&input)
            .expect("解码输入失败")
            .to_rgb8();
        let after = image::load_from_memory(&converted)
            .expect("解码输出失败")
            .to_rgb8();
        assert_eq!(before.dimensions(), after.dimensions());
        assert_eq!(before.into_raw(), after.into_raw());

        // 对齐 Qt 的 zlib level 4：应当比 image 的默认档位（Fast）更小
        let fast = encode_png(
            image::load_from_memory(&input).expect("解码输入失败"),
            image::codecs::png::CompressionType::Fast,
        );
        assert!(
            converted.len() < fast.len(),
            "level 4 的输出（{} 字节）应当小于 Fast（{} 字节）",
            converted.len(),
            fast.len()
        );
    }

    #[test]
    fn other_formats_fall_back_to_jpeg() {
        let converted = convert(&sample_png(), "jpg");
        assert_eq!(&converted[..2], &[0xff, 0xd8], "输出应当是 JPEG");
    }
}
