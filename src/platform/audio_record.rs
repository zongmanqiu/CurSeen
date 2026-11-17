//! 纯录音（M4A）：WASAPI 采集 + Media Foundation AAC 编码，音频-only MP4 容器。
//!
//! # 为什么复用这两块
//!
//! - 采集：[`AudioCapture`]（麦克风 + 系统声 loopback、48kHz/16-bit/立体声、逐样本
//!   混音 + RNNoise 降噪）与录屏共用同一条路——"录音默认带麦"的直觉与录屏一致，
//!   音源/降噪配置也直接沿用 `record.record_mic / record_speaker / mic_denoise`，
//!   不另开一套配置口径（m81 §3 拍板：不引第三方库，当前的就够）。
//! - 封装：MF 的 AAC 编码器 + MPEG4 Media Sink 与录屏音频轨同一套 OS 组件，
//!   **零新增体积**。容器走 `MFCreateMPEG4MediaSink`（显式 sink）而不是按扩展名猜
//!   （`MFCreateSinkWriterFromURL` 依赖扩展名映射，`.m4a` 是否命中是环境行为；
//!   `mp4_trim` 的显式 sink 路线是本仓验证过的，照抄）。
//!
//! # 与录屏录制器的边界
//!
//! 录屏（`platform::recorder`）围着 DXGI 桌面复制转：单例、楔住检测、换格式。
//! 纯录音没有桌面复制单例约束，所以运行时侧独立持有（`Runtime::audio_rec`），
//! 但**交互与提示跟录屏完全同一套**（2026-11-16 用户口径：提示格式必须统一）——
//! 同一只 3 秒倒计时钟、同一张「即将开始 N」居中横幅（全功能唯一入口），归零才开录，再按即停。
//!
//! # 收尾模型
//!
//! 与录屏的 `finish_detached` 同款：`stop` 只置位，收尾（Finalize 落盘）在录音线程上做，
//! 结果经通道回传——UI 线程绝不等 Finalize（AAC 收尾要 flush 编码器，几十到几百毫秒）。

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;

use super::audio_capture::AudioCapture;
use super::mp4_trim::MfGuard;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u32 = 2;
/// AAC 目标码率（字节/秒）＝ 128 kbps，与录屏音频轨同一档（`recorder.rs`）。
const AAC_BYTES_PER_SEC: u32 = 16_000;

/// 一次录音的全部参数（音源沿用录屏配置，见模块头）。
#[derive(Debug, Clone)]
pub struct AudioRecOptions {
    /// 输出文件（`.m4a`）。
    pub path: PathBuf,
    /// 麦克风来源：`None` = 不采麦；`Some(设备名)`，空串 = 跟随系统默认。
    pub mic: Option<String>,
    /// 系统声来源：`None` = 不采系统声；`Some(设备名)`，空串 = 跟随系统默认。
    pub speaker: Option<String>,
    /// 麦克风降噪档（`off` / `weak` / `medium` / `strong`），只作用于麦路。
    pub mic_denoise: String,
}

/// 进行中的一段录音。`stop()` 置位后由 [`Self::finish_detached`] 收结果。
pub struct AudioRecorder {
    stop: Arc<std::sync::atomic::AtomicBool>,
    done_rx: Mutex<Option<Receiver<Result<(), String>>>>,
    /// 通知端存这里：`finish_detached` 时 drop，让异常路径也能立刻断线报错。
    done_tx: Mutex<Option<Sender<Result<(), String>>>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AudioRecorder {
    /// 启动录音（线程内建采集与编码器，失败立即返回 Err，UI 当场给反馈）。
    pub fn start(opts: AudioRecOptions) -> Result<Self, String> {
        if opts.mic.is_none() && opts.speaker.is_none() {
            return Err("麦克风与系统声音都未启用（设置页「视频录制 > 声音」里至少开一路）".into());
        }
        if let Some(parent) = opts.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let stop_t = stop.clone();
        let done_tx_t = done_tx.clone();
        let handle = thread::Builder::new()
            .name("curseen-audio-rec".into())
            .spawn(move || {
                let res = record_loop(opts, stop_t);
                let _ = done_tx_t.send(res);
            })
            .map_err(|e| format!("录音线程启动失败: {e}"))?;
        Ok(Self {
            stop,
            done_rx: Mutex::new(Some(done_rx)),
            done_tx: Mutex::new(Some(done_tx)),
            handle: Some(handle),
        })
    }

    /// 请求停止（不等待收尾）。
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// 收尾：等录音线程把文件落盘（上限 `timeout`），结果交给 `done`。
    ///
    /// ★ 与录屏 `finish_detached` 同一条纪律：**UI 线程最多等 `timeout`**，等不到就放弃
    /// （线程自己会收完盘，产物不丢），绝不让退出/切换卡死。调用后本句柄作废。
    pub fn finish_detached(
        mut self,
        timeout: Duration,
        done: impl FnOnce(Result<(), String>) + Send + 'static,
    ) {
        self.stop();
        let rx = self.done_rx.lock().ok().and_then(|mut g| g.take());
        let handle = self.handle.take();
        thread::Builder::new()
            .name("curseen-audio-rec-finish".into())
            .spawn(move || {
                let res = match rx {
                    // 正常路径：录音线程自己发来结果。
                    Some(rx) => match rx.recv_timeout(timeout) {
                        Ok(r) => r,
                        Err(_) => Err("录音收尾超时（线程未在限时内落盘）".into()),
                    },
                    None => Err("录音结果通道已被取走".into()),
                };
                if let Some(h) = handle {
                    let _ = h.join();
                }
                done(res);
            })
            .map_err(|e| log::warn!("录音收尾线程启动失败: {e}"))
            .ok();
        // done_tx 在此 drop：录音线程若因异常没发结果，recv_timeout 立刻断线报错。
        if let Ok(mut g) = self.done_tx.lock() {
            *g = None;
        }
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        // 兜底：忘了 finish 也要停（结果无人接，线程收完盘自行退出）。
        self.stop();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// 录音线程主体：采集 → MF AAC 编码 → 音频-only MP4（`.m4a`）。
///
/// ★ 封装格式只有 m4a 一档（2026-11-16 用户拍板：WAV 选项撤销，"特地多一个选项
///   根本没必要"——当天的 WAV 实验代码已整体回退，本注释留档防复添）。
fn record_loop(
    opts: AudioRecOptions,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), String> {
    let _mf = MfGuard::start();
    let capture = start_capture(&opts)?;

    let sink = open_sink(&opts.path)?;
    unsafe {
        let stream = add_audio_stream(&sink)?;
        set_pcm_input(&sink, stream)?;
        sink.writer
            .BeginWriting()
            .map_err(|e| format!("BeginWriting 失败: {e}"))?;

        let mut frames: u64 = 0; // 已写入的帧数（一帧 = 一对立体声样本）
        let mut err: Option<String> = None;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let samples = capture.take_samples();
            if samples.is_empty() {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            if let Err(e) = write_pcm(&sink, stream, &samples, frames) {
                err = Some(format!("写样本失败: {e}"));
                break;
            }
            frames += (samples.len() / CHANNELS as usize) as u64;
        }
        // 收尾顺序：先停采集线程（不再产样本）再 Finalize（编码器 flush）。
        capture.stop();
        drop(capture);
        if let Some(e) = err {
            let _ = sink.writer.Finalize(); // 尽力落盘已录部分
            return Err(e);
        }
        sink.writer
            .Finalize()
            .map_err(|e| format!("Finalize 失败: {e}"))?;
    }
    log::info!("录音已保存：{}", opts.path.display());
    Ok(())
}

/// 音源参数 → 采集器（降噪链与录屏同一条 RNNoise）。
fn start_capture(opts: &AudioRecOptions) -> Result<AudioCapture, String> {
    AudioCapture::start(
        opts.mic.is_some(),
        opts.speaker.is_some(),
        &opts.mic_denoise,
        opts.mic.as_deref().unwrap_or(""),
        opts.speaker.as_deref().unwrap_or(""),
    )
    .ok_or_else(|| "音频采集启动失败（设备被占用或不存在）".to_string())
}

// ─────────────────── SinkWriter 装配（音频-only MP4/M4A） ───────────────────

/// 一次性装好的 writer：显式 MPEG4 sink + 一条 AAC 输出流。
struct AudioSink {
    writer: IMFSinkWriter,
}

/// AAC 输出流（与 `recorder.rs` 音频轨同参：128 kbps / 48kHz / 立体声 / 16-bit 源）。
unsafe fn add_audio_stream(sink: &AudioSink) -> Result<u32, String> {
    let out: IMFMediaType = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .map_err(|e| format!("音频 MAJOR_TYPE: {e}"))?;
    out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
        .map_err(|e| format!("音频 SUBTYPE: {e}"))?;
    out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SEC)
        .map_err(|e| format!("音频码率: {e}"))?;
    out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)
        .map_err(|e| format!("音频采样率: {e}"))?;
    out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)
        .map_err(|e| format!("音频声道数: {e}"))?;
    out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
        .map_err(|e| format!("音频位深: {e}"))?;
    sink.writer
        .AddStream(&out)
        .map_err(|e| format!("AddStream: {e}"))
}

/// 声明编码器的**输入**是 PCM（48kHz/立体声/16-bit）——`recorder.rs` 音频轨同参。
/// ★ 缺了这步 `WriteSample` 会因输入类型未定而全部失败（AAC 编码器要知道喂的是什么）。
unsafe fn set_pcm_input(sink: &AudioSink, stream: u32) -> Result<(), String> {
    let inp: IMFMediaType = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .map_err(|e| format!("输入 MAJOR_TYPE: {e}"))?;
    inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
        .map_err(|e| format!("输入 SUBTYPE: {e}"))?;
    inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)
        .map_err(|e| format!("输入采样率: {e}"))?;
    inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)
        .map_err(|e| format!("输入声道数: {e}"))?;
    inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
        .map_err(|e| format!("输入位深: {e}"))?;
    inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)
        .map_err(|e| format!("输入对齐: {e}"))?;
    inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, SAMPLE_RATE * 4)
        .map_err(|e| format!("输入字节率: {e}"))?;
    sink.writer
        .SetInputMediaType(stream, &inp, None)
        .map_err(|e| format!("SetInputMediaType: {e}"))
}

/// 建音频-only MP4 sink：字节流 → MPEG4MediaSink → SinkWriter（`mp4_trim` 同路线）。
fn open_sink(path: &Path) -> Result<AudioSink, String> {
    unsafe {
        let url = HSTRING::from(path.as_os_str());
        let byte_stream: IMFByteStream = MFCreateFile(
            MF_ACCESSMODE_WRITE,
            MF_OPENMODE_DELETE_IF_EXIST,
            MF_FILEFLAGS_NONE,
            &url,
        )
        .map_err(|e| format!("MFCreateFile 失败: {e}"))?;
        // 第 2/3 参传 None：不预建轨道，交给下面的 AddStream（见 `mp4_trim` 模块头的
        // "4 条轨"事故——预建 + AddStream 会产出零样本空轨）。
        let media_sink: IMFMediaSink = MFCreateMPEG4MediaSink(&byte_stream, None, None)
            .map_err(|e| format!("MFCreateMPEG4MediaSink 失败: {e}"))?;
        let writer: IMFSinkWriter = MFCreateSinkWriterFromMediaSink(&media_sink, None)
            .map_err(|e| format!("MFCreateSinkWriterFromMediaSink 失败: {e}"))?;
        Ok(AudioSink { writer })
    }
}

/// 把一段交错立体声 PCM 写进 AAC 流（时间戳按帧数推，48kHz → 100ns 单位）。
unsafe fn write_pcm(
    sink: &AudioSink,
    stream: u32,
    samples: &[i16],
    start_frame: u64,
) -> Result<(), String> {
    let byte_len = samples.len() * 2;
    let buf: IMFMediaBuffer =
        MFCreateMemoryBuffer(byte_len as u32).map_err(|e| format!("MFCreateMemoryBuffer: {e}"))?;
    {
        let mut p = std::ptr::null_mut::<u8>();
        buf.Lock(&mut p, None, None)
            .map_err(|e| format!("buffer Lock: {e}"))?;
        // SAFETY: p 由 Lock 给出且容量恰为 byte_len；samples 的 i16 内存按字节重解释
        // 与拷贝（i16 无填充，等价于 platform::clipboard.rs 的逐行 memcpy 口径）。
        std::ptr::copy_nonoverlapping(samples.as_ptr().cast::<u8>(), p, byte_len);
        buf.Unlock().map_err(|e| format!("buffer Unlock: {e}"))?;
    }
    buf.SetCurrentLength(byte_len as u32)
        .map_err(|e| format!("SetCurrentLength: {e}"))?;
    let sample: IMFSample = MFCreateSample().map_err(|e| format!("MFCreateSample: {e}"))?;
    sample
        .AddBuffer(&buf)
        .map_err(|e| format!("AddBuffer: {e}"))?;
    let hns_per_frame = (10_000_000i64 / SAMPLE_RATE as i64).max(1);
    let t0 = (start_frame as i64).saturating_mul(hns_per_frame);
    let dur = ((samples.len() / CHANNELS as usize) as i64).saturating_mul(hns_per_frame);
    sample
        .SetSampleTime(t0)
        .map_err(|e| format!("SetSampleTime: {e}"))?;
    sample
        .SetSampleDuration(dur)
        .map_err(|e| format!("SetSampleDuration: {e}"))?;
    sink.writer
        .WriteSample(stream, Some(&sample))
        .map_err(|e| format!("WriteSample: {e}"))
}

#[cfg(test)]
mod tests {
    // （封装格式只有 m4a 一档；WAV 实验代码连同其单测已于 2026-11-16 整体回退。）
}
