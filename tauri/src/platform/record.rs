// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 录音端点（`recordAudio`）。
//!
//! 对照 `qt/aqt/sound.py`：Qt 用 `QAudioSource` 采集、用 `wave` 模块写 WAV，
//! 并弹一个 `RecordDialog`（录音图标 + 计时 + 保存/取消）。HTTP 请求会一直挂起，
//! 直到对话框结束才带着文件路径返回，因此**这是个长阻塞端点**（调用方在
//! `spawn_blocking` 里跑，见 `server/api.rs`）。
//!
//! 与 Qt 的差异：
//!
//! - 采集用 `cpal`，写 WAV 用 `hound`（Qt 在 Windows 上同样存 WAV）；
//! - 对话框是宿主自建的 legacy 页面（`legacy::recorder_page`），按钮通过
//!   `bridgeCommand` 回传决定；
//! - Qt 保存后会用外部 `lame` 把 WAV 转成 MP3（没装 `lame` 就报错），这里**直接
//!   返回 WAV**：不引入外部可执行文件依赖，代价是文件更大（一分钟约 5 MB）。
//!   Android 上的 AnkiDroid 用的也是录音器的原生格式，并非 MP3。
//! - 用户在 Qt 里点「取消」时，`RecordDialog.reject` 不会把结果回给请求方，
//!   那个 HTTP 请求会永远挂着（实现缺陷）。这里改为返回空路径，让调用方至少能
//!   结束这次调用。

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anki_proto::generic::String as GenericString;
use cpal::traits::DeviceTrait as _;
use cpal::traits::HostTrait as _;
use cpal::traits::StreamTrait as _;
use prost::Message as _;
use tauri::Manager as _;

use super::HandlerResult;
use crate::legacy;
use crate::state::AppState;
use crate::windows;
use crate::windows::WindowSpec;

/// 对照 Qt：停止后延后一小段时间再收尾，避免截断结尾
/// （`Recorder.stop` 里的 `QTimer.singleShot(500, ...)`）。
const STOP_DELAY: Duration = Duration::from_millis(500);

/// 等待用户决定的轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 翻译串里代表秒数的占位符，由页面脚本按帧替换。
const SECS_PLACEHOLDER: &str = "__ANKI_SECS__";

/// 用户对录音的处置。对照 `RecordDialog.accept` / `reject`。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// 保存，并把文件路径返回给调用方（`accept`）
    Save,
    /// 丢弃，并删掉录音文件（`reject`）
    Discard,
}

/// 录音会话状态。
///
/// 对照 Qt：`RecordDialog` 是模态的，同时只可能存在一个录音会话。
#[derive(Default)]
pub struct Recorder {
    session: Mutex<Option<Arc<Active>>>,
}

/// 一次进行中的录音；`outcome` 由桥接命令写入。
#[derive(Default)]
struct Active {
    outcome: Mutex<Option<Outcome>>,
}

impl Recorder {
    /// 记录用户的处置。返回 `false` 表示没有正在进行的录音。
    pub fn finish(&self, outcome: Outcome) -> bool {
        let active = self.session.lock().expect("录音状态已中毒").clone();
        let Some(active) = active else {
            tracing::debug!("没有正在进行的录音，忽略处置: {outcome:?}");
            return false;
        };
        *active.outcome.lock().expect("录音状态已中毒") = Some(outcome);
        true
    }

    fn begin(&self) -> Option<Arc<Active>> {
        let mut guard = self.session.lock().expect("录音状态已中毒");
        if guard.is_some() {
            return None;
        }
        let active = Arc::new(Active::default());
        *guard = Some(active.clone());
        Some(active)
    }

    fn clear(&self) {
        *self.session.lock().expect("录音状态已中毒") = None;
    }
}

/// 对应 mediasrv 的 `record_audio`：**阻塞**直到用户确定或取消。
pub fn record(state: &AppState) -> HandlerResult {
    let Some(active) = state.recorder.begin() else {
        return Err("已经有正在进行的录音".to_string());
    };

    let path = output_path()?;
    // 与 Qt 同样的顺序：先开始采集，再弹对话框
    let capture = match Capture::start(&path) {
        Ok(capture) => capture,
        Err(error) => {
            state.recorder.clear();
            return Err(error);
        }
    };

    let page = legacy::recorder_page(
        state.port(),
        state.backend.i18n(),
        SECS_PLACEHOLDER,
        &crate::theme::style(state),
    );
    let spec = WindowSpec::new(windows::RECORDER, format!("_anki/legacyPageData?id={page}"))
        .title("Anki")
        .size(360.0, 200.0)
        // Qt 的对话框是主窗口的子窗口；置顶能保证它不会被编辑窗口盖住
        .always_on_top(true);
    if let Err(error) = windows::open(state, &spec) {
        if let Err(error) = capture.finish() {
            tracing::warn!("收尾录音失败: {error}");
        }
        let _ = std::fs::remove_file(&path);
        state.recorder.clear();
        return Err(format!("打开录音窗口失败: {error:#}"));
    }

    let outcome = wait_for_outcome(state, &active);
    std::thread::sleep(STOP_DELAY);
    if let Err(error) = capture.finish() {
        tracing::warn!("收尾录音失败: {error}");
    }
    windows::close(state, windows::RECORDER);
    state.recorder.clear();

    match outcome {
        Outcome::Save => {
            tracing::info!("录音完成: {}", path.display());
            let response = GenericString {
                val: path.to_string_lossy().into_owned(),
            };
            Ok(response.encode_to_vec())
        }
        Outcome::Discard => {
            let _ = std::fs::remove_file(&path);
            tracing::debug!("录音已取消");
            Ok(GenericString::default().encode_to_vec())
        }
    }
}

/// 等待用户点「保存/取消」，或直接关掉录音窗口（等同于取消）。
fn wait_for_outcome(state: &AppState, active: &Active) -> Outcome {
    loop {
        if let Some(outcome) = *active.outcome.lock().expect("录音状态已中毒") {
            return outcome;
        }
        // 窗口没了说明用户关掉了它；这条路径必须有，
        // 否则录音会一直占着麦克风（Qt 里对话框被销毁时同样会走到收尾）
        if state.app.get_webview_window(windows::RECORDER).is_none() {
            tracing::debug!("录音窗口已被关闭，按取消处理");
            return Outcome::Discard;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 录音输出路径。对照 Qt 的 `namedtmp("rec.wav")`：临时目录 + 唯一文件名。
fn output_path() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("anki");
    std::fs::create_dir_all(&dir).map_err(|error| format!("创建临时目录失败: {error}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default();
    Ok(dir.join(format!("rec-{stamp}.wav")))
}

/// 进行中的采集。
struct Capture {
    stream: cpal::Stream,
    writer: Writer,
}

/// WAV 写入器的句柄。
///
/// `hound::WavWriter` 不是 `Sync`，而采集回调跑在音频线程上；用
/// `Mutex<Option<_>>` 包一层后，收尾时可以直接「取走」再 `finalize`，
/// 不必要求独占所有权（回调线程可能才刚刚释放锁）。
type Writer = Arc<Mutex<Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>>>;

impl Capture {
    fn start(path: &Path) -> Result<Self, String> {
        let device = cpal::default_host()
            .default_input_device()
            .ok_or_else(|| "找不到可用的录音设备".to_string())?;
        let supported = device
            .default_input_config()
            .map_err(|error| format!("读取录音设备配置失败: {error}"))?;
        let sample_format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        // 统一写 16 位整数。对照 Qt：把设备的 float 采样转成 int16 再写文件。
        let spec = hound::WavSpec {
            channels: config.channels,
            sample_rate: config.sample_rate.0,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)
            .map_err(|error| format!("创建录音文件失败: {error}"))?;
        let writer = Arc::new(Mutex::new(Some(writer)));

        let stream = build_stream(&device, &config, sample_format, writer.clone())?;
        stream
            .play()
            .map_err(|error| format!("启动录音失败: {error}"))?;
        tracing::info!(
            "开始录音: {} 通道 / {} Hz / {sample_format:?}",
            config.channels,
            config.sample_rate.0
        );
        Ok(Self { stream, writer })
    }

    /// 停止采集并写完 WAV 尾（`finalize` 会回填数据长度）。
    fn finish(self) -> Result<(), String> {
        // 先停流，否则回调可能还在写
        drop(self.stream);
        let writer = self.writer.lock().expect("录音写入器已中毒").take();
        match writer {
            Some(writer) => writer
                .finalize()
                .map_err(|error| format!("写入录音文件失败: {error}")),
            None => Ok(()),
        }
    }
}

fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    writer: Writer,
) -> Result<cpal::Stream, String> {
    let stream = match format {
        cpal::SampleFormat::I16 => {
            let writer = writer.clone();
            device.build_input_stream(
                config,
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    write_samples(&writer, data.iter().copied());
                },
                on_stream_error,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let writer = writer.clone();
            device.build_input_stream(
                config,
                move |data: &[u16], _: &cpal::InputCallbackInfo| {
                    // 无符号 16 位以 32768 为静音中点
                    write_samples(
                        &writer,
                        data.iter()
                            .map(|sample| (*sample as i32 - 32768) as i16),
                    );
                },
                on_stream_error,
                None,
            )
        }
        cpal::SampleFormat::F32 => {
            let writer = writer.clone();
            device.build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    write_samples(
                        &writer,
                        data.iter().map(|sample| {
                            (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
                        }),
                    );
                },
                on_stream_error,
                None,
            )
        }
        other => return Err(format!("暂不支持的录音采样格式: {other:?}")),
    };
    stream.map_err(|error| format!("无法打开录音设备: {error}"))
}

fn on_stream_error(error: cpal::StreamError) {
    tracing::warn!("录音流出错: {error}");
}

fn write_samples(writer: &Writer, samples: impl Iterator<Item = i16>) {
    let Ok(mut guard) = writer.lock() else {
        return;
    };
    let Some(writer) = guard.as_mut() else {
        return;
    };
    for sample in samples {
        if let Err(error) = writer.write_sample(sample) {
            tracing::warn!("写入录音数据失败: {error}");
            return;
        }
    }
}
