// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! 音频播放。
//!
//! 对照 `qt/aqt/sound.py` 的 `AVPlayer`（原实现基于 mpv 进程）。Qt 的模型是**队列**：
//! `play_tags()` 先清空队列并打断当前播放，然后逐个播完再播下一个；复习界面在显示
//! 问题/答案时会调用它自动播放卡片里的音频。因此这里同样维护队列，而不是"播放一个
//! 文件"。
//!
//! 设计：`rodio` 的 `OutputStream` 不是 `Send`，无法存放在全局静态变量里，
//! 因此用一个常驻音频线程持有流与 Sink，通过 channel 接收播放请求。
//! 播放线程用 `recv_timeout` 轮询而不是阻塞 `recv` —— 否则在播放期间收不到
//! 「打断」指令。

use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::Hash as _;
use std::hash::Hasher as _;
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use anki_proto::card_rendering::all_tts_voices_response::TtsVoice;
use anki_proto::card_rendering::AllTtsVoicesRequest;
use anki_proto::card_rendering::AllTtsVoicesResponse;
use anki_proto::card_rendering::TtsTag;
use anki_proto::card_rendering::WriteTtsStreamRequest;
use anki_proto::generic::Empty;
use anki_proto::generic::String as GenericString;
use prost::Message as _;

use super::HandlerResult;
use crate::state::AppState;

/// 播放线程的轮询间隔：也决定「打断」的最坏延迟
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// 发给音频线程的指令。
///
/// 目前只有「播放 / 暂停 / 前后跳」：Qt 的 `stop_and_clear_queue` 只用在 shutdown 与
/// 切换调用者时，本外壳还没有这两种场景，所以不保留单独的 Stop 指令。
enum Command {
    /// 清空队列、打断当前播放，然后按顺序播放这些文件（对照 `play_tags`）
    Play(Vec<PathBuf>),
    /// 暂停 / 继续当前播放（对照 `av_player.toggle_pause()`）
    TogglePause,
    /// 相对当前位置前后跳（秒，可负）；对照 `av_player.seek_relative()`
    SeekRelative(f32),
}

static COMMANDS: OnceLock<mpsc::Sender<Command>> = OnceLock::new();


/// 当前是否有音频在播（自动前进的 `waitForAudio` 用它，对照 `av_player.current_player`）。
static PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// 是否正在播放音频（复制标签/`playFile` 都会走同一个队列）。
pub fn is_playing() -> bool {
    PLAYING.load(std::sync::atomic::Ordering::SeqCst)
}


/// 暂停 / 继续（对照 `reviewer.py:644-646` 的 `on_pause_audio` → `av_player.toggle_pause()`）。
pub fn toggle_pause() -> Result<(), String> {
    commands()
        .send(Command::TogglePause)
        .map_err(|_| "音频线程已退出".to_string())
}

/// 相对当前位置前后跳（`reviewer.py:650-656` 的 `on_seek_backward` / `on_seek_forward`）。
pub fn seek_relative(seconds: f32) -> Result<(), String> {
    commands()
        .send(Command::SeekRelative(seconds))
        .map_err(|_| "音频线程已退出".to_string())
}

/// 对应 mediasrv 的 `play_file`。
pub fn play_file(state: &AppState, body: &[u8]) -> HandlerResult {
    let request = GenericString::decode(body).map_err(|e| e.to_string())?;
    let path = media_path(state, &request.val)?;
    play_files(vec![path])
}

/// 按顺序播放一组媒体文件；新调用会清空队列并打断当前播放。
pub fn play_files(paths: Vec<PathBuf>) -> HandlerResult {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    if commands().send(Command::Play(paths)).is_err() {
        return Err("音频线程已退出".to_string());
    }
    Ok(Vec::new())
}

/// 解析集合媒体目录下的文件。
///
/// `AVPlayer.play_file` 的注释特别强调：文件名可能来自集合，**只应使用 basename**。
/// 否则 `[sound:../../some/secret.mp3]` 这样的卡片就能播放媒体目录之外的任意文件。
pub fn media_path(state: &AppState, name: &str) -> Result<PathBuf, String> {
    let folder = state
        .media_folder
        .as_ref()
        .ok_or_else(|| "集合尚未打开".to_string())?;
    let base = Path::new(name)
        .file_name()
        .ok_or_else(|| format!("非法的媒体文件名: {name}"))?;
    let path = folder.join(base);
    if !path.exists() {
        return Err(format!("找不到媒体文件: {name}"));
    }
    Ok(path)
}

fn commands() -> &'static mpsc::Sender<Command> {
    COMMANDS.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Command>();
        std::thread::Builder::new()
            .name("anki-audio".into())
            .spawn(move || audio_loop(rx))
            .expect("无法启动音频线程");
        tx
    })
}

fn audio_loop(rx: mpsc::Receiver<Command>) {
    // `_stream` 必须在整个循环期间存活，drop 后音频会停止
    let Ok((_stream, handle)) = rodio::OutputStream::try_default() else {
        tracing::error!("无法初始化音频输出设备，播放功能不可用");
        // 仍然消费指令，避免发送端拿到「线程已退出」以外看不出原因的失败
        while rx.recv().is_ok() {}
        return;
    };
    let Ok(sink) = rodio::Sink::try_new(&handle) else {
        tracing::error!("无法创建音频 sink");
        return;
    };

    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    let mut playing = false;

    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(Command::Play(paths)) => {
                // 对照 `clear_queue_and_maybe_interrupt()`
                sink.stop();
                sink.play();
                queue = paths.into();
                playing = false;
            }
            // 对照 `av_player.toggle_pause()`
            Ok(Command::TogglePause) => {
                if sink.is_paused() {
                    sink.play();
                    tracing::debug!("音频继续");
                } else {
                    sink.pause();
                    tracing::debug!("音频暂停");
                }
            }
            // 对照 `av_player.seek_relative()`：rodio 的 `try_seek` 是绝对位置，
            // 所以取当前位置再加偏移（下钳到 0）
            Ok(Command::SeekRelative(seconds)) => {
                let current = sink.get_pos();
                let target = if seconds >= 0.0 {
                    current + std::time::Duration::from_secs_f32(seconds)
                } else {
                    current
                        .saturating_sub(std::time::Duration::from_secs_f32(-seconds))
                };
                match sink.try_seek(target) {
                    Ok(()) => tracing::debug!("音频跳到 {:?}（{seconds:+.0} 秒）", target),
                    Err(error) => tracing::warn!("音频跳转失败: {error}"),
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        // 当前文件播完后（source 已从 sink 移除）开始下一个
        if playing && sink.len() == 0 {
            playing = false;
        }

        if !playing {
            if let Some(path) = queue.pop_front() {
                match open_source(&path) {
                    Ok(source) => {
                        tracing::debug!("播放音频: {}", path.display());
                        sink.append(source);
                        playing = true;
                    }
                    Err(error) => tracing::warn!("播放失败 {}: {error}", path.display()),
                }
            }
        }

        // 供自动前进的 `waitForAudio` 判断（对照 Qt 的 `av_player.current_player`）
        PLAYING.store(playing, std::sync::atomic::Ordering::SeqCst);
    }
}

fn open_source(path: &Path) -> Result<rodio::Decoder<std::io::BufReader<std::fs::File>>, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())
}

/// 把 TTS 标签合成为 WAV，返回可直接入队播放的文件路径。
///
/// 对照 `qt/aqt/tts.py` 的 `WindowsRTTTSFilePlayer`：Windows 上 Qt 实际走的就是
/// 这条路径（`backend.all_tts_voices` + `backend.write_tts_stream`），而不是用 SAPI
/// 的 `WindowsTTSPlayer` —— 后者 `default_rank = -1`，会被 rank 更高的实现盖过
/// （`av_player` 选 rank 最高的播放器）。因此这里也不需要引入新的依赖：
/// 语音合成由 rslib 的 `card_rendering::tts` 完成。
///
/// 合成结果按「语音包 + 文本 + 语速」缓存，与 Qt 的 `temp_file_for_tag_and_voice`
/// 一致：同一张卡反复显示时不会重复合成。
pub async fn tts_file(state: &AppState, tag: &TtsTag) -> anyhow::Result<PathBuf> {
    let voices = all_voices(state).await?;
    let voice = pick_voice(&voices, tag)
        .ok_or_else(|| anyhow::anyhow!("找不到语言 {} 的语音包", tag.lang))?;

    let path = tts_cache_path(&voice.id, tag);
    if path.exists() {
        return Ok(path);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let request = WriteTtsStreamRequest {
        path: path.to_string_lossy().into_owned(),
        voice_id: voice.id.clone(),
        speed: tag.speed,
        text: tag.field_text.clone(),
    };
    let bytes = state
        .backend
        .call("writeTtsStream", request.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    // 返回的是 generic.Empty，解码一次以确认响应合法
    Empty::decode(bytes.as_slice())?;
    tracing::debug!("TTS 已合成: {}", path.display());
    Ok(path)
}

async fn all_voices(state: &AppState) -> anyhow::Result<Vec<TtsVoice>> {
    // `validate: false` 与 Qt 一致：只列出语音包，不逐个试读（后者很慢）
    let request = AllTtsVoicesRequest { validate: false };
    let bytes = state
        .backend
        .call("allTtsVoices", request.encode_to_vec())
        .await
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(AllTtsVoicesResponse::decode(bytes.as_slice())?.voices)
}

/// 对照 `TTSPlayer.voice_for_tag`：先按用户写的 `voices=` 顺序找「同名 + 同语言」
/// 的语音包，都匹配不上时退回该语言下第一个可用的。
fn pick_voice<'a>(voices: &'a [TtsVoice], tag: &TtsTag) -> Option<&'a TtsVoice> {
    for requested in &tag.voices {
        let matched = voices.iter().find(|voice| {
            tidy_voice_name(&voice.name) == *requested && voice_lang(voice) == tag.lang
        });
        if let Some(voice) = matched {
            return Some(voice);
        }
    }
    voices.iter().find(|voice| voice_lang(voice) == tag.lang)
}

/// 对照 `aqt/tts.py` 的 `_tidy_name`：语音包名字里的空格换成下划线，
/// 这样模板里可以写 `voices=Microsoft_Huihui`。
fn tidy_voice_name(name: &str) -> String {
    name.replace(' ', "_")
}

/// 对照 `WindowsRTTTSFilePlayer.from_backend_voice`：把 `en-US` 归一成模板里
/// 使用的 `en_US`。
fn voice_lang(voice: &TtsVoice) -> String {
    voice.language.replace('-', "_")
}

/// 合成结果的缓存路径。对照 Qt 的 `temp_file_for_tag_and_voice`。
fn tts_cache_path(voice_id: &str, tag: &TtsTag) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    voice_id.hash(&mut hasher);
    tag.field_text.hash(&mut hasher);
    tag.speed.to_bits().hash(&mut hasher);
    std::env::temp_dir()
        .join("anki")
        .join(format!("tts-{:016x}.wav", hasher.finish()))
}