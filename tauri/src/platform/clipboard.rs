// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 剪贴板端点。
//!
//! 对照 mediasrv 的 `read_clipboard` / `write_clipboard`：前端按 MIME 类型
//! 请求原始字节（见 `ts/routes/editor/rich-text-input/data-transfer.ts`），
//! 编辑器粘贴图片时实际只用到两种形态：
//!
//! - `text/uri-list`：资源管理器里「复制文件」时写入的格式。编辑器据此拿到
//!   文件路径，再交给 `addMediaFromUrl`（rslib 支持 `file://`，见
//!   `rslib/src/editor.rs::retrieve_url`）；
//! - `image/*`：真正的图片数据（截图工具、画图等直接放图片时）。
//!
//! `arboard` 只暴露文本与图片，不提供文件列表，因此 `text/uri-list` 在 Windows
//! 上直接读 `CF_HDROP`。Qt 侧靠 `QMimeData.data(type)` 取任意格式，这是本方案
//! 唯一需要平台代码的地方。

use std::collections::HashMap;

use anki_proto::frontend::ReadClipboardRequest;
use anki_proto::frontend::ReadClipboardResponse;
use anki_proto::frontend::WriteClipboardRequest;
use prost::Message as _;

use super::HandlerResult;

/// 文件列表格式。编辑器用它把「复制的文件」变成媒体文件。
const URI_LIST: &str = "text/uri-list";

pub fn read(body: &[u8]) -> HandlerResult {
    let request = ReadClipboardRequest::decode(body).map_err(|e| e.to_string())?;

    // `text/uri-list` 走平台原生，不经 arboard
    let uri_list = request
        .types
        .iter()
        .any(|kind| kind == URI_LIST)
        .then(clipboard_uri_list)
        .flatten();
    // 只在真的需要文本/图片时才去开剪贴板（开剪贴板会短暂占用它）
    let needs_arboard = request.types.iter().any(|kind| kind != URI_LIST);
    let mut clipboard = needs_arboard.then(|| arboard::Clipboard::new().ok()).flatten();

    let mut data: HashMap<String, Vec<u8>> = HashMap::new();
    for requested in &request.types {
        if requested == URI_LIST {
            if let Some(list) = &uri_list {
                data.insert(requested.clone(), list.clone().into_bytes());
            }
            continue;
        }

        let Some(clipboard) = clipboard.as_mut() else {
            continue;
        };
        if requested.starts_with("image/") {
            // 统一给 PNG 字节：前端拿到后会走 `convertPastedImage` 重新编码成
            // 用户偏好的格式（png / jpg），并不依赖请求时的 MIME 是否精确。
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

    // 图片优先：同时写入文本与图片时，QVariant 语义下图片更具体
    if let Some(bytes) = request
        .data
        .iter()
        .find(|(mime, _)| mime.starts_with("image/"))
        .map(|(_, bytes)| bytes)
    {
        let decoded = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let rgba = decoded.to_rgba8();
        let image = arboard::ImageData {
            width: rgba.width() as usize,
            height: rgba.height() as usize,
            bytes: rgba.into_raw().into(),
        };
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_image(image))
            .map_err(|e| e.to_string())?;
        return Ok(Vec::new());
    }

    // 其余按文本写入：前端的 `writeBlobToClipboard` 只会传 blob 自己的类型，
    // `text/uri-list` 这类非文本格式目前没有调用方。
    if let Some(bytes) = request
        .data
        .iter()
        .find(|(mime, _)| mime.starts_with("text/"))
        .map(|(_, bytes)| bytes)
    {
        let text = String::from_utf8_lossy(bytes).into_owned();
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.set_text(text))
            .map_err(|e| e.to_string())?;
        return Ok(Vec::new());
    }

    tracing::debug!(
        "写剪贴板：没有可用的格式 {:?}",
        request.data.keys().collect::<Vec<_>>()
    );
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

/// 读剪贴板里的文件列表并拼成 `text/uri-list`。
#[cfg(windows)]
fn clipboard_uri_list() -> Option<String> {
    use windows::Win32::System::DataExchange::CloseClipboard;
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    use windows::Win32::System::DataExchange::OpenClipboard;

    // 对照 Qt：`QMimeData` 直接提供该格式，这里等价于读 `CF_HDROP`
    if unsafe { IsClipboardFormatAvailable(CF_HDROP) }.is_err() {
        return None;
    }
    if let Err(error) = unsafe { OpenClipboard(None) } {
        tracing::debug!("无法打开剪贴板: {error}");
        return None;
    }

    // 无论成败都要释放剪贴板，否则其他程序读不到
    let paths = unsafe { read_drop_paths() };
    let _ = unsafe { CloseClipboard() };

    let paths = paths?;
    if paths.is_empty() {
        return None;
    }
    tracing::debug!("剪贴板里有 {} 个文件", paths.len());
    Some(to_uri_list(&paths))
}

/// 非 Windows 平台暂不支持文件列表：`arboard` 没有该能力，Linux / macOS 需要
/// 各自的剪贴板 API。Android 上文件选择走 SAF，与本路径无关。
#[cfg(not(windows))]
fn clipboard_uri_list() -> Option<String> {
    None
}

/// `CF_HDROP`。常量定义在 `Win32::System::Ole` 里（值 15），
/// 为这一个常量多启用一个 feature 不划算。
#[cfg(windows)]
const CF_HDROP: u32 = 15;

/// 在已打开的剪贴板上读出 `CF_HDROP` 里的文件路径。
///
/// 调用方负责 `OpenClipboard` / `CloseClipboard`。
#[cfg(windows)]
unsafe fn read_drop_paths() -> Option<Vec<String>> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::UI::Shell::DragQueryFileW;
    use windows::Win32::UI::Shell::HDROP;

    let handle = GetClipboardData(CF_HDROP).ok()?;
    let drop = HDROP(handle.0);

    // ifile 传 u32::MAX 时返回文件个数
    let count = DragQueryFileW(drop, u32::MAX, None);
    if count == 0 {
        return None;
    }

    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // 先问长度（不含结尾的 NUL），再取内容
        let len = DragQueryFileW(drop, index, None);
        if len == 0 {
            continue;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let written = DragQueryFileW(drop, index, Some(&mut buffer));
        if written == 0 {
            continue;
        }
        buffer.truncate(written as usize);
        paths.push(String::from_utf16_lossy(&buffer));
    }
    Some(paths)
}

/// 拼 `text/uri-list`。前端 `getUrls` 只做 `split("\n")` + `trim()`，
/// 对换行符不敏感；这里按规范用 CRLF。
fn to_uri_list(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| file_uri(path))
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 本地路径 → `file://` URL。
///
/// 需要转义空格等字符：`urlToLink` 是按后缀匹配的，而 `file://` 会被 rslib 的
/// `Url::parse` + 本地读取处理，未转义的空格会破坏解析。
fn file_uri(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let encoded = percent_encoding::utf8_percent_encode(&normalized, URI_LIST_ESCAPE);
    if normalized.starts_with('/') {
        // 已经是绝对路径（类 Unix）
        format!("file://{encoded}")
    } else {
        // Windows 盘符：file:///C:/...
        format!("file:///{encoded}")
    }
}

/// 需要转义的字符集合。保留 `/` 与 `:`，其余按 RFC 3986 的路径规则收紧。
const URI_LIST_ESCAPE: &percent_encoding::AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');
