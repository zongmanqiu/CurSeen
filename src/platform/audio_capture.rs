//! WASAPI 音频采集：麦克风 + 系统声音（loopback），逐样本混音，可选降噪。
//!
//! # 设计要点
//!
//! 1. **两路独立采集**：麦克风（`eCapture`）和系统声音（`eRender` + `AUDCLNT_STREAMFLAGS_LOOPBACK`）
//!    各走一条 WASAPI 采集线程，互不干扰。
//! 2. **格式统一**：两路都初始化为 48kHz / 16-bit / 立体声（PCM），这是 Windows 默认混合格式，
//!    也是 Media Foundation AAC 编码器的输入要求（32-bit float 的 mix 也在此转成 16-bit）。
//! 3. **设备可选**：`mic_device` / `speaker_device` 是设备友好名（空串=系统默认）。
//!    `list_mic_devices` / `list_speaker_devices` 给出候选，UI 下拉用。
//! 4. **混音**：两路各自进独立 buffer，[`AudioCapture::take_samples`] 把两路**逐样本求和**
//!    （限幅）成一路立体声，再交给录制线程写 AAC。早期实现是「拼接」两路 buffer，
//!    会得到时序错乱的音频——求和是正确做法。
//! 5. **降噪**：仅作用于麦克风，单一下拉（`off` / `weak` / `medium` / `strong`）。
//!    底层是 **`nnnoiseless`**（Xiph RNNoise 神经网络降噪算法的纯 Rust 移植，BSD-3-Clause，
//!    见 `third_party/nnnoiseless`）——**不再自写谱减**。自写谱减在「强」档会冒出电子音
//!    （musical noise，谱减法的经典缺陷），神经网络降噪没有这个问题。三档靠「干湿混合比」
//!    模拟（见 [`DenoiseStrength::wet_ratio`]）：弱=50% 湿、中=80%、强=100%（全 RNNoise）。
//!    实测对比见 `tests/denoise_probe.rs`。
//!
//!    - `Off`：原样透传。
//!    - `Rnnoise`：`nnnoiseless` 神经网络降噪，每 480 样本（10ms @ 48kHz）一帧、无需重叠相加；
//!      声道按「前两路各自一路状态、其余声道透传」处理。无外部 C 依赖，二进制只多一个纯
//!      Rust crate，契合「小而美」。配置字段 `mic_denoise` 仍认 `"rnnoise"` / `"spectral"` /
//!      `"gate"` 作为 RNNoise 的别名（遗留兼容）。

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IAudioRenderClient,
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Threading::{CreateEventA, WaitForSingleObject};

/// 麦克风降噪引擎档位。
///
/// 现在只有「关」与「RNNoise」两态——噪声门（`Gate`）这一档已在 2026-09-19 删除：
/// 它只在整帧都安静时才清零、**说话时的底噪原样保留**，而 RNNoise 说话时也在压，
/// 且对电流声 / 小声音同样有效，单独保留噪声门没有意义。强度（弱 / 中 / 强）
/// 由 [`DenoiseStrength`] 携带（表现为干湿混合比），见 [`parse_mic_denoise`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseMode {
    /// 不处理（原样透传）。
    Off,
    /// RNNoise 神经网络降噪（纯 Rust 的 `nnnoiseless` 移植）。
    Rnnoise,
}

/// 把配置里的降噪取值（单一下拉：`off` / `weak` / `medium` / `strong`，
/// 以及遗留的 `gate` / `spectral` / `rnnoise`）解析成 `(引擎档, 强度)`。
///
/// - `off` → 不处理；
/// - `weak` / `medium` / `strong` → RNNoise + 对应干湿比；
/// - 遗留 `gate` / `spectral` / `rnnoise` → RNNoise + 中等强度（噪声门已删，并入 RNNoise）；
/// - 其它 → 不处理（安全兜底）。
///
/// 录制与试听都吃同一个字符串，保证"试听听到的"就是"录进去的"。
pub fn parse_mic_denoise(s: &str) -> (DenoiseMode, DenoiseStrength) {
    match s {
        "off" => (DenoiseMode::Off, DenoiseStrength::Medium),
        "weak" => (DenoiseMode::Rnnoise, DenoiseStrength::Weak),
        "medium" => (DenoiseMode::Rnnoise, DenoiseStrength::Medium),
        "strong" => (DenoiseMode::Rnnoise, DenoiseStrength::Strong),
        // 遗留兼容：噪声门 / 旧谱减取值统一按中等强度并入 RNNoise。
        "gate" | "spectral" | "rnnoise" => (DenoiseMode::Rnnoise, DenoiseStrength::Medium),
        _ => (DenoiseMode::Off, DenoiseStrength::Medium),
    }
}

/// 降噪强度（界面上的「弱 / 中 / 强」）。
///
/// RNNoise 本身没有强度档，这里用「干湿混合比」模拟——[`DenoiseStrength::wet_ratio`]
/// 是降噪结果（湿）在最终输出里占的比例，其余为原始（干）。**不做成一个连续的滑块**
/// 是有意的：三档对应台阶式的听感（弱=几乎只压底噪、强=明显削掉气声和齿音），连续量程
/// 会让用户调到一个"既削人声又没压住噪声"的中间地带。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DenoiseStrength {
    /// 弱：只压稳定的底噪（风扇、电流声），人声几乎不动（50% 湿）。
    Weak,
    /// 中：出厂档。底噪明显下来，人声保持自然（80% 湿）。
    Medium,
    /// 强：键盘声、呼吸声、电流声一起压，代价是气声/齿音会被削（100% 湿）。
    Strong,
}

impl DenoiseStrength {
    /// 从配置下标解析（0/1/2），越界回落 `Medium`。
    pub fn from_level(level: u32) -> Self {
        match level {
            0 => DenoiseStrength::Weak,
            2 => DenoiseStrength::Strong,
            _ => DenoiseStrength::Medium,
        }
    }

    /// 干湿混合比：降噪结果（湿）在最终输出里占的比例，其余为原始（干）。
    ///
    /// RNNoise 的降噪已经很彻底（接近 WebRTC/Speex 级别），所以：
    /// - `Weak`(0.5) 只压稳定的稳态底噪、人声几乎原样；
    /// - `Medium`(0.8) 出厂档，底噪明显下来、人声自然；
    /// - `Strong`(1.0) 全量 RNNoise，键盘声/呼吸声/电流声一起压，气声齿音会被削。
    ///
    /// 之所以用混合比而不是重训模型，是为了"几个档位 + 极小二进制"——RNNoise 模型
    /// 是固定的，档位只是调输出里保留多少原始信号。
    pub fn wet_ratio(self) -> f32 {
        match self {
            DenoiseStrength::Weak => 0.5,
            DenoiseStrength::Medium => 0.8,
            DenoiseStrength::Strong => 1.0,
        }
    }
}

/// 音频采集器：管理麦克风和系统声音两路采集，并提供混音后的样本。
pub struct AudioCapture {
    /// 麦克风采集到的 PCM 样本（16-bit 立体声，48kHz）。
    mic_buffer: Arc<Mutex<Vec<i16>>>,
    /// 系统声音采集到的 PCM 样本（16-bit 立体声，48kHz）。
    spk_buffer: Arc<Mutex<Vec<i16>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    /// 两路**都**启用（且两条线程都真起来了）→ 混音必须走时间对齐分支。
    ///
    /// ★ 单路时 `take_samples` 保持原来的"有多少取多少"语义（一路不存在对齐问题），
    ///   只有两路同开才需要下面 `aligned` 那套对齐，见该函数的说明。
    both: bool,
    /// 两路的起点是否已经对齐过（**只需做一次**，见 `take_samples`）。
    aligned: std::sync::atomic::AtomicBool,
    /// 两路中只有一路有数据、另一路为空的**起始时刻**（`None` = 两路刚到齐过）。
    ///
    /// 用来区分"另一路这一轮还没到"（正常抖动，等下一轮即可）和
    /// "另一路真的没声音"（loopback 在没有播放会话时一个包都不产）。
    /// ★ 判据必须是**等待了多久**，不能是"堆了多少样本"——见 [`LANE_IDLE_WAIT`]。
    idle_since: Mutex<Option<Instant>>,
    /// 采集线程句柄：Drop 时 join，保证"停止请求 → 线程确认退出"闭环
    /// （不 join 也能自终，但尾部线程与缓冲会多活一个采集周期，且与
    /// Recorder / MicMonitor 的口径不一致。泄露审计 2026-09-30）。
    threads: Vec<thread::JoinHandle<()>>,
}

impl AudioCapture {
    /// 启动音频采集。
    ///
    /// `capture_mic` = 是否采集麦克风；`capture_speaker` = 是否采集系统声音。
    /// 至少一路为 true 才有意义；两路都 false 返回 None（无音频）。
    /// `mic_device` / `speaker_device` 为空串时取系统默认设备。
    pub fn start(
        capture_mic: bool,
        capture_speaker: bool,
        mic_denoise: &str,
        mic_device: &str,
        speaker_device: &str,
    ) -> Option<Self> {
        if !capture_mic && !capture_speaker {
            return None;
        }

        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mic_buffer = Arc::new(Mutex::new(Vec::new()));
        let spk_buffer = Arc::new(Mutex::new(Vec::new()));
        let mut threads: Vec<thread::JoinHandle<()>> = Vec::new();

        if capture_mic {
            let stop = stop.clone();
            let buffer = mic_buffer.clone();
            let dev = mic_device.to_string();
            let setting = mic_denoise.to_string();
            if let Ok(h) = thread::Builder::new()
                .name("curseen-audio-mic".into())
                .spawn(move || {
                    run_capture_loop(eCapture, false, &dev, buffer, stop, true, &setting)
                })
            {
                threads.push(h);
            }
        }

        if capture_speaker {
            let stop = stop.clone();
            let buffer = spk_buffer.clone();
            let dev = speaker_device.to_string();
            let setting = mic_denoise.to_string();
            if let Ok(h) = thread::Builder::new()
                .name("curseen-audio-spk".into())
                .spawn(move || run_capture_loop(eRender, true, &dev, buffer, stop, false, &setting))
            {
                threads.push(h);
            }
        }

        // 两路**都开且两条线程都真起来了**才需要时间对齐：任一线程没起来，那一路的
        // buffer 会恒空，只能走单路语义（否则会永远等一个不存在的包）。
        let both = capture_mic && capture_speaker && threads.len() == 2;
        Some(Self {
            mic_buffer,
            spk_buffer,
            stop,
            both,
            aligned: std::sync::atomic::AtomicBool::new(false),
            idle_since: Mutex::new(None),
            threads,
        })
    }

    /// 取出混音后的 PCM 样本（消费式）：麦克风 + 系统声音逐样本求和、限幅。
    ///
    /// # ★★ 两路同开时必须按时间对齐（2026-10-09 查"系统声音有回音"时实测出来的）
    ///
    /// 旧实现是 `n = mic.len().max(spk.len())`，然后 `mic[i] + spk[i]`。问题在于
    /// **两路是两条独立线程**：各自 100ms 缓冲、各自被 WASAPI 事件唤醒，谁先到谁
    /// 写在 buffer 前面。于是"第 i 个麦克风样本"与"第 i 个系统声音样本"
    /// **根本不是同一时刻的声音**。后果两条，都能被听见：
    ///
    /// 1. **音频被拉长**（`tests/probes/audio_echo.rs::rate_check` 实测）：
    ///    两路交替到达时 `Σ max(mic_i, spk_i)` 逼近**两倍**单路 —— 本该 1 秒的音频
    ///    被写成近 2 秒。本机实测：单路 0.970 倍、**两路同开 1.593 倍**。
    ///    写进 MP4 后音轨比视频长得多，音画不同步、听起来"慢半拍"。
    /// 2. **回音**：系统声音走 spk 路直达，又被麦克风拾到走 mic 路，两路错开
    ///    Δ 毫秒相加。实测每个脉冲后面跟着一个 **+20~30ms** 的重复脉冲。
    ///
    /// 修法两条：
    /// - **起点对齐（只做一次）**：首次两路都到齐时，两路 buffer 的长度差正好是
    ///   "启动时间差 × 采样率"，丢掉先启动那路多出的前置样本，之后两路同起点；
    /// - **逐轮取 min**：只输出两路**都到齐**的那部分，慢的那路的余量留到下一轮
    ///   （不丢）。于是输出量恒等于较慢那路的产出量 ≈ 实时，不再被串接放大。
    ///
    /// ★ 剩下的那 20~30ms（麦克风拾到扬声器外放）是**声学现象**，对齐消不掉：
    ///   要消它得靠戴耳机或 AEC（回声消除），不是本函数该管的事。
    pub fn take_samples(&self) -> Vec<i16> {
        let mut mic = match self.mic_buffer.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut spk = match self.spk_buffer.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };

        // 单路（或另一路线程没起来）：不存在对齐问题，保持原语义"有多少取多少"。
        if !self.both {
            let n = mic.len().max(spk.len());
            return if n == 0 {
                Vec::new()
            } else {
                mix_and_drain(&mut mic, &mut spk, n)
            };
        }

        // ── 两路同开：先对齐起点（只做一次），再逐轮取 min ──
        if !self.aligned.load(std::sync::atomic::Ordering::Relaxed) {
            let (m, s) = (mic.len(), spk.len());
            if m > 0 && s > 0 {
                // 两路都到齐：长度差 = 启动时间差 × 采样率，丢掉先启动那路的前置。
                if m > s {
                    mic.drain(..(m - s));
                } else {
                    spk.drain(..(s - m));
                }
                self.aligned.store(true, std::sync::atomic::Ordering::Relaxed);
            } else if m.max(s) > 0 {
                // ★ 另一路还没到齐：**直接按有的那一路输出，绝不等待**。
                //   两路设备的初始化有先后（实测差到 170ms）。若这里等待，时间线
                //   就会停滞那么久，而且**追不回来**（停滞量小于录制层补静音的
                //   200ms 阈值，永远触发不了补齐）⇒ 音轨永久落后 = 声音比画面慢。
                //   另一路起来（或系统从静默恢复）时，它的第一个样本正好对应
                //   "那一刻"，而另一路的消费位置也走到了那一刻 —— 静默期间我们
                //   没消费过它，所以两边天然对齐，不会积压。
                return mix_and_drain(&mut mic, &mut spk, m.max(s));
            } else {
                return Vec::new();
            }
        }
        let n = mic.len().min(spk.len());
        if n > 0 {
            if let Ok(mut g) = self.idle_since.lock() {
                *g = None;
            }
            return mix_and_drain(&mut mic, &mut spk, n);
        }
        let longest = mic.len().max(spk.len());
        if longest == 0 {
            // 两路都空：系统彻底没声音 + 麦克风也没声。返回空，让录制层按视频
            // 时间线补静音（那是**唯一**允许推进时间线的地方，不能两边都推进）。
            if let Ok(mut g) = self.idle_since.lock() {
                *g = None;
            }
            return Vec::new();
        }
        // 只有一路有数据：另一路要么"这一轮还没到"（正常抖动），要么**真的没声音**。
        // 判据是**等了多久**，不是堆了多少 —— 见 [`LANE_IDLE_WAIT`] 的说明。
        let waited = match self.idle_since.lock() {
            Ok(mut g) => match *g {
                None => {
                    *g = Some(Instant::now());
                    None
                }
                Some(t) => Some(t.elapsed()),
            },
            // 锁坏了就按"已经等够"处理：宁可退回单路，也不能卡住时间线。
            Err(_) => Some(LANE_IDLE_WAIT),
        };
        match waited {
            Some(t) if t >= LANE_IDLE_WAIT => mix_and_drain(&mut mic, &mut spk, longest),
            _ => Vec::new(),
        }
    }

    /// 请求停止并等待线程退出。
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
        // join：采集循环每轮观察 stop 位后自终，join 最多等一个采集周期。
        for h in self.threads.drain(..) {
            let _ = h.join();
        }
    }
}

/// 判定"另一路真的没声音"的**等待时长**。
///
/// ★★ 为什么判据必须是时间、不能是堆积量（2026-10-09 修"声音比画面慢"时定稿）：
/// 旧实现用堆积量（0.5 秒的样本数）。系统静默时 loopback **一个包都不产**
/// （没有播放会话 —— 实测如此），于是麦克风路一路堆到阈值才吐一次；而这期间
/// `take_samples` 一直返回空 ⇒ 录制层按视频时间线**补了静音**、时间线照常推进，
/// 等那 0.5 秒积压一次性吐出，时间线**又推进一次**。两边都推进 = 音轨被拉长，
/// 播放出来就是"声音比画面慢"（点击模式实测 −86 ~ −513ms，随录制时长累积）。
/// 只开系统声音时不受影响（单路没有对齐分支），实测偏移 ±70ms 内。
///
/// 取 60ms：约 2 帧（30fps）／1 帧（15fps），远大于一个 WASAPI 包周期（10ms），
/// 不会把"这一轮还没到"误判成静默；又远小于录制层补静音的 200ms 阈值
/// （`recorder::SILENCE_FILL_MIN_HNS`），双重计量的窗口根本打不开。
const LANE_IDLE_WAIT: std::time::Duration = std::time::Duration::from_millis(60);

/// 取两路的前 `n` 个样本逐样本求和、限幅，并把它们从各自 buffer 里消费掉。
///
/// ★ 用 `get(i).unwrap_or(0)`：某路不足 `n` 个时补 0（那一刻它没声音）——
///   这是"另一路停了"时的兜底，不是常态。
fn mix_and_drain(mic: &mut Vec<i16>, spk: &mut Vec<i16>, n: usize) -> Vec<i16> {
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = mic.get(i).copied().unwrap_or(0);
        let b = spk.get(i).copied().unwrap_or(0);
        out.push(clamp_i16(a as i32 + b as i32));
    }
    let m = mic.len().min(n);
    mic.drain(..m);
    let s = spk.len().min(n);
    spk.drain(..s);
    out
}

/// 枚举麦克风设备友好名（供 UI 下拉）。
pub fn list_mic_devices() -> Vec<String> {
    list_audio_devices(eCapture)
}

/// 枚举系统声音（loopback 来源）设备友好名（供 UI 下拉）。
pub fn list_speaker_devices() -> Vec<String> {
    list_audio_devices(eRender)
}

/// 当前系统默认设备的两个名字。
///
/// 存在的理由：面板上「默认 / 指定设备」是两种**不同语义**（跟随系统 vs 钉死某一台），
/// 但「默认」这个词本身不告诉用户"现在到底跟随的是哪台"。拿这两个名字把「默认」
/// 写成「默认 (Realtek High Definition Audio)」，歧义就没了。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultDevice {
    /// `PKEY_Device_FriendlyName`，如 `扬声器 (Realtek High Definition Audio)`。
    /// 与 [`list_mic_devices`] / [`resolve_device`] 同字段，用来判断
    /// "配置里钉的那台是否就是当前默认"。
    pub full: String,
    /// 硬件名，如 `Realtek High Definition Audio`。只用于文案（匹配走 [`Self::full`]）。
    /// 取不到时依次回落"完整名括号里的内容"→"完整名本身"，见 `device_short_name`。
    pub short: String,
}

/// 当前默认麦克风设备（`None` = 没有默认端点 / 枚举失败）。
pub fn default_mic_device() -> Option<DefaultDevice> {
    default_device(eCapture)
}

/// 当前默认系统声音设备。
pub fn default_speaker_device() -> Option<DefaultDevice> {
    default_device(eRender)
}

fn default_device(flow: EDataFlow) -> Option<DefaultDevice> {
    unsafe {
        let _com = ComGuard::new();
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
        let dev = enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok()?;
        let full = device_display_name(&dev)?;
        let short = device_short_name(&dev, &full);
        Some(DefaultDevice { full, short })
    }
}

/// 枚举指定数据流的活跃设备名。
fn list_audio_devices(flow: EDataFlow) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let _com = ComGuard::new();
        let enumerator: IMMDeviceEnumerator =
            match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(_) => return out,
            };
        let Ok(coll) = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE) else {
            return out;
        };
        let Ok(count) = coll.GetCount() else {
            return out;
        };
        for i in 0..count {
            if let Ok(dev) = coll.Item(i) {
                if let Some(name) = device_display_name(&dev) {
                    out.push(name);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 单路采集循环（在独立线程中运行）。
#[allow(clippy::too_many_arguments)]
fn run_capture_loop(
    flow: EDataFlow,
    is_loopback: bool,
    device_name: &str,
    buffer: Arc<Mutex<Vec<i16>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    is_mic: bool,
    mic_denoise: &str,
) {
    // 麦克风线程才需要降噪处理器；系统声音原样进 buffer。
    let mut processor = if is_mic {
        let (denoise, strength) = parse_mic_denoise(mic_denoise);
        Some(MicProcessor::new(denoise, strength))
    } else {
        None
    };

    unsafe {
        let _com = ComGuard::new();
        let result = init_and_capture(
            flow,
            is_loopback,
            device_name,
            &buffer,
            &stop,
            processor.as_mut(),
        );
        if let Err(e) = result {
            log::error!(
                "音频采集失败 ({}): {e}",
                if is_loopback {
                    "系统声音"
                } else {
                    "麦克风"
                }
            );
        }
    }
}

/// 初始化 WASAPI 并执行采集循环。
#[allow(clippy::too_many_arguments)]
unsafe fn init_and_capture(
    flow: EDataFlow,
    is_loopback: bool,
    device_name: &str,
    buffer: &Mutex<Vec<i16>>,
    stop: &std::sync::atomic::AtomicBool,
    mut processor: Option<&mut MicProcessor>,
) -> std::result::Result<(), String> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|e| format!("创建 MMDeviceEnumerator 失败: {e}"))?;

    // 按名字选设备；名字为空或匹配不到 → 系统默认。
    let device_flow = if is_loopback { eRender } else { flow };
    let device: IMMDevice = match resolve_device(&enumerator, device_flow, device_name) {
        Some(d) => d,
        None => enumerator
            .GetDefaultAudioEndpoint(device_flow, eConsole)
            .map_err(|e| format!("获取默认音频端点失败: {e}"))?,
    };

    let client: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活 IAudioClient 失败: {e}"))?;

    // ★★ 采集格式强制 48k/2ch/16bit + AUTOCONVERTPCM（2026-10-09，对齐开源做法）。
    //    旧实现按 `GetMixFormat` 原样采集，但下游全部硬编码假设 48k×2ch：
    //    混音求和（两路逐样本相加）、`write_audio_samples` 的时长折算（48000×2）、
    //    RNNoise 的 FRAME_SIZE(=480@48k)。设备混音格式一旦不是 48k 立体声
    //    （44.1k / 单声道很常见），产物就会变速/变调/声道错乱——"接近原声但怪怪的"。
    //    现在让 WASAPI 引擎自己转换（Rivulet `AUTOCONVERTPCM|SRC_DEFAULT_QUALITY`、
    //    wcap 强制目标格式同款做法），采集端恒定 48k/2ch/16bit，假设永远成立。
    let target = WAVEFORMATEX {
        wFormatTag: 1, // WAVE_FORMAT_PCM（无扩展头的裸 PCM）
        nChannels: 2,
        nSamplesPerSec: TARGET_SAMPLE_RATE,
        wBitsPerSample: 16,
        nBlockAlign: 4,
        nAvgBytesPerSec: TARGET_SAMPLE_RATE * 4,
        cbSize: 0,
    };

    let mut stream_flags: u32 = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
        | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if is_loopback {
        stream_flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }

    // 缓冲区时长：100ms。★ 参数单位是 hns，换算见 [`hns_from_ms`]——
    //    这里原来写的 `1_000_000 * 100` 实际是 10 秒（差 100 倍）。
    let buffer_duration: i64 = hns_from_ms(CAPTURE_BUFFER_MS);

    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            stream_flags,
            buffer_duration,
            0,
            &target,
            None,
        )
        .map_err(|e| format!("初始化音频客户端失败: {e}"))?;

    let event = CreateEventA(None, false, false, None).map_err(|e| format!("创建事件失败: {e}"))?;
    client
        .SetEventHandle(event)
        .map_err(|e| format!("设置事件句柄失败: {e}"))?;

    let capture_client: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("获取 IAudioCaptureClient 失败: {e}"))?;

    client
        .Start()
        .map_err(|e| format!("启动音频采集失败: {e}"))?;

    let bytes_per_frame = target.nBlockAlign as usize;
    let channels = target.nChannels as usize;
    loop {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }

        let wait_result = WaitForSingleObject(event, 100);
        if wait_result.0 != 0 {
            continue;
        }

        let packet_size = match client.GetCurrentPadding() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if packet_size == 0 {
            continue;
        }

        let mut data_ptr: *mut u8 = std::ptr::null_mut();
        let mut num_frames = 0u32;
        let mut flags = 0u32;

        if capture_client
            .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
            .is_err()
        {
            continue;
        }
        if num_frames == 0 || data_ptr.is_null() {
            let _ = capture_client.ReleaseBuffer(num_frames);
            continue;
        }

        let byte_count = num_frames as usize * bytes_per_frame;
        let samples: Vec<i16> = if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 {
            let src = std::slice::from_raw_parts(data_ptr, byte_count);
            match target.wBitsPerSample {
                16 => src
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]))
                    .collect(),
                32 => src
                    .chunks_exact(4)
                    .map(|c| {
                        let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                        (f * 32767.0).clamp(-32768.0, 32767.0) as i16
                    })
                    .collect(),
                _ => {
                    let _ = capture_client.ReleaseBuffer(num_frames);
                    continue;
                }
            }
        } else {
            vec![0i16; num_frames as usize * channels]
        };

        let _ = capture_client.ReleaseBuffer(num_frames);

        // 系统声音原样；麦克风过降噪处理器（RNNoise）。
        // `channels` 从采集格式读，别写死 2（麦克风常是单声道）。
        let processed = match &mut processor {
            Some(p) => p.process(&samples, channels),
            None => samples,
        };

        if let Ok(mut buf) = buffer.lock() {
            buf.extend_from_slice(&processed);
        }
    }

    client.Stop().ok();
    Ok(())
}

/// 按友好名解析设备；空名或匹配不到回落默认。
unsafe fn resolve_device(
    enumerator: &IMMDeviceEnumerator,
    flow: EDataFlow,
    name: &str,
) -> Option<IMMDevice> {
    if name.is_empty() {
        return enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok();
    }
    let coll = enumerator
        .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
        .ok()?;
    let count = coll.GetCount().ok()?;
    for i in 0..count {
        if let Ok(dev) = coll.Item(i) {
            if let Some(n) = device_display_name(&dev) {
                if n.eq_ignore_ascii_case(name) {
                    return Some(dev);
                }
            }
        }
    }
    enumerator.GetDefaultAudioEndpoint(flow, eConsole).ok()
}

/// 取设备属性表里的一个字符串属性（用完释放其 PropVariant 字符串内存）。
///
/// `key` 传 [`PKEY_Device_FriendlyName`]（完整名，如 `扬声器 (Realtek High Definition Audio)`）
/// 或 [`PKEY_Device_DeviceDesc`]（简短描述，如 `Realtek High Definition Audio`）。
unsafe fn device_prop_string(
    device: &IMMDevice,
    key: &windows::Win32::Foundation::PROPERTYKEY,
) -> Option<String> {
    use windows::Win32::System::Com::STGM_READ;
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut pv = store.GetValue(key).ok()?;
    if pv.Anonymous.Anonymous.vt != VT_LPWSTR {
        let _ = PropVariantClear(&mut pv);
        return None;
    }
    // 先拷出字符串，再交给 PropVariantClear 统一释放（手动 CoTaskMemFree 在
    // windows 0.62 下实测堆损坏 0xc0000374，见 tests::lists_audio_devices…）。
    let pw = pv.Anonymous.Anonymous.Anonymous.pwszVal;
    let s = String::from_utf16_lossy(pw.as_wide());
    let _ = PropVariantClear(&mut pv);
    if s.trim().is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 取设备友好名（`PKEY_Device_FriendlyName`）。
///
/// ★ **设备匹配与下拉列表都用这个字段**，所以它必须与 [`resolve_device`] 的比较口径
/// 完全一致 —— 换成简短描述会让"配置里钉的设备"再也匹配不上，静默回落默认设备。
unsafe fn device_display_name(device: &IMMDevice) -> Option<String> {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    device_prop_string(device, &PKEY_Device_FriendlyName)
}

/// 取设备的**硬件名**（`Realtek High Definition Audio`），用于「默认 (…)」文案。
///
/// **只用于文案，不参与任何匹配** —— 匹配走 [`device_display_name`]。完整名带
/// `扬声器 (...)` / `麦克风 (...)` 外壳，直接拼进「默认 (…)」又长又套括号。
///
/// 三级兜底（顺序就是优先级）：
/// 1. `PKEY_DeviceInterface_FriendlyName` —— 硬件名，正是要的；
/// 2. 完整名括号里的内容（`扬声器 (Realtek High Definition Audio)` → `Realtek…`）；
/// 3. 完整名本身（文案长一点，总比「默认 ()」空括号强）。
///
/// ★ **不能用 `PKEY_Device_DeviceDesc`**：音频端点上它存的是**通用类型名**
///   （中文系统实测就是光秃秃的 `扬声器` / `麦克风`），拿它当"具体是哪台"等于没给信息。
unsafe fn device_short_name(device: &IMMDevice, full: &str) -> String {
    use windows::Win32::Devices::FunctionDiscovery::PKEY_DeviceInterface_FriendlyName;
    if let Some(s) = device_prop_string(device, &PKEY_DeviceInterface_FriendlyName) {
        return s;
    }
    // 按**字节偏移**切（`find`/`rfind` 返回的就是字节下标且必落在字符边界上），
    // 不能用 `chars()` 下标 —— 设备名里有非 ASCII。
    if let (Some(a), Some(b)) = (full.find('('), full.rfind(')')) {
        let inner = full[a + 1..b].trim();
        if !inner.is_empty() {
            return inner.to_string();
        }
    }
    full.to_string()
}

/// COM 初始化守卫：作用域内保证 CoInitialize/Couninitialize 配对。
struct ComGuard;
impl ComGuard {
    fn new() -> Option<Self> {
        unsafe {
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() {
                Some(Self)
            } else {
                None
            }
        }
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

/// `hnsBufferDuration` 的换算：**单位是 100 纳秒**（hns = hundred-nanoseconds）。
///
/// ★★★ 2026-09-19 实测踩坑：`IAudioClient::Initialize` 的缓冲时长参数是 hns
///   （`1 ms = 10_000`、`1 s = 10_000_000`）。旧代码写 `1_000_000 * 60` 并在注释里
///   当成 "60ms"，实际是 **60,000,000 hns = 6 秒**；`1_000_000 * 100` 当成 "100ms"，
///   实际是 **10 秒**。差 100 倍，而且**不报任何错** —— 它只是安静地把缓冲放大 100 倍。
///
///   在**渲染**流上后果是致命的：起播前"铺满静音"会一举占掉整段缓冲，于是
///   麦克风试听的前 6 秒全是静音、之后就永远晚 6 秒（`tests/monitor_probe.rs`
///   的时间线实测：直到 `[6.0s)` 那一箱才出现 440Hz）。
fn hns_from_ms(ms: u64) -> i64 {
    (ms * 10_000) as i64
}

/// 采集流的缓冲时长。采集由事件唤醒、每 5ms 抽干一次，缓冲大小**不构成延迟**，
/// 100ms 只是给"抽干"留余量。
const CAPTURE_BUFFER_MS: u64 = 100;

/// 采集端强制目标采样率（Hz）。下游（混音 / RNNoise / MP4 写入）全部按此假设。
const TARGET_SAMPLE_RATE: u32 = 48_000;

/// 试听**渲染**流的缓冲时长。★ 这个值**直接等于试听延迟**（缓冲满则新写的数据排在
/// 后面播），所以必须小 —— 60ms 足够抗抖动，又小到"说话即闻"。
const MONITOR_RENDER_BUFFER_MS: u64 = 60;

/// [`IAudioClient::GetMixFormat`] 返回缓冲区的守卫。
///
/// ★★ 它指向的**不是** 18 字节的 `WAVEFORMATEX`，而是一整块 `WAVEFORMATEXTENSIBLE`
///    （`wFormatTag = 0xFFFE`、`cbSize = 22`，共 40 字节）—— Windows 共享模式下恒为
///    此格式。传给 `Initialize` 的必须是**这一整块**：只把 `*ptr` 拷成 `WAVEFORMATEX`
///    会丢掉后 22 字节，而 `cbSize` 仍声明 22，WASAPI 照着 `cbSize` 读越界，直接返回
///    `E_INVALIDARG`（0x80070057）—— 表现为"勾了麦克风/系统声音却录不到任何声音"。
///
///    现场取证（`probe_mix_format_truncation`，本机实测）：扬声器与麦克风两条路径，
///    截断成 18 字节传参 → `Err(0x80070057)`；原指针传整块 → `Ok(())`。
///
/// 顺带用 RAII 管住 `CoTaskMemFree`：下面 `Initialize` 的每个 `?` 早退都会走 Drop。
struct MixFormat(*mut WAVEFORMATEX);

impl MixFormat {
    /// `Initialize` 要的格式指针 —— **整块原样传**，不要自己解引用拷贝。
    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.0
    }
    fn channels(&self) -> usize {
        unsafe { (*self.0).nChannels as usize }
    }
    fn rate(&self) -> u32 {
        unsafe { (*self.0).nSamplesPerSec }
    }
    fn block_align(&self) -> usize {
        unsafe { (*self.0).nBlockAlign as usize }
    }
    fn bits(&self) -> u16 {
        unsafe { (*self.0).wBitsPerSample }
    }
}

impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
    }
}

/// 麦克风处理器：把一段交错 i16 PCM 转成降噪（或原样透传）后的交错 i16。
struct MicProcessor {
    denoise: DenoiseMode,
    strength: DenoiseStrength,
    /// 每条要处理的声道一个 RNNoise 状态（**最多 2 条**）。
    ///
    /// ★ 按采集格式的声道数在第一次 `process` 时定下来，之后复用 —— RNNoise 是带状态的
    ///   （RNN 隐藏层），每帧重建等于永远在"重新初始化"，降噪会退化。
    states: Vec<Box<nnnoiseless::DenoiseState<'static>>>,
    /// 上一轮没凑满一帧（FRAME_SIZE 样本/路）而留下的交织样本（所有声道），下轮续上。
    inbuf: Vec<i16>,
}

impl MicProcessor {
    fn new(denoise: DenoiseMode, strength: DenoiseStrength) -> Self {
        Self {
            denoise,
            strength,
            states: Vec::new(),
            inbuf: Vec::new(),
        }
    }

    /// `channels` = 采集格式的声道数（`nChannels`）。
    ///
    /// ★★ 这里必须是**实际声道数**，不能写死 2：麦克风的混音格式大多是**单声道**，
    ///   早期版本把样本按 `chunks_exact(2)` 当立体声拆，于是每两个相邻的单声道采样被
    ///   当成"左右声道"分别去噪，降噪效果整体失准（且单声道下输出长度只有输入的一半）。
    fn process(&mut self, samples: &[i16], channels: usize) -> Vec<i16> {
        let ch = channels.max(1);
        match self.denoise {
            DenoiseMode::Off => samples.to_vec(),
            DenoiseMode::Rnnoise => {
                let lanes = ch.min(2);
                if self.states.len() != lanes {
                    self.states = (0..lanes)
                        .map(|_| nnnoiseless::DenoiseState::new())
                        .collect();
                    self.inbuf.clear();
                }
                self.inbuf.extend_from_slice(samples);
                // 每帧 RNNoise 吃 FRAME_SIZE(=480) 个样本/路；交织下每帧占 FRAME_SIZE*ch 个样本。
                let frame_len = nnnoiseless::FRAME_SIZE * ch;
                let n_frames = self.inbuf.len() / frame_len;
                let mut out = Vec::with_capacity(n_frames * frame_len);
                let wet = self.strength.wet_ratio();
                let dry = 1.0 - wet;
                let mut obuf = [0f32; nnnoiseless::FRAME_SIZE];
                for f in 0..n_frames {
                    let base = f * frame_len;
                    // 取出每路（前 lanes 路）本帧的 FRAME_SIZE 个样本（f32，范围 [-32768,32767]）。
                    let mut lane_in: Vec<[f32; nnnoiseless::FRAME_SIZE]> =
                        Vec::with_capacity(lanes);
                    for c in 0..lanes {
                        let mut arr = [0f32; nnnoiseless::FRAME_SIZE];
                        // 步长 ch 的解交织取样本：i 既是 arr 下标也是 inbuf 的步长序号。
                        for (i, slot) in arr.iter_mut().enumerate() {
                            *slot = self.inbuf[base + i * ch + c] as f32;
                        }
                        lane_in.push(arr);
                    }
                    // 处理每路：RNNoise 出湿信号，与原信号按干湿比混合。
                    let mut lane_out: Vec<[f32; nnnoiseless::FRAME_SIZE]> =
                        Vec::with_capacity(lanes);
                    for (c, lane) in lane_in.iter().enumerate() {
                        self.states[c].process_frame(&mut obuf, lane);
                        let mut arr = [0f32; nnnoiseless::FRAME_SIZE];
                        // RNNoise 湿信号与原信号按干湿比逐样本混合。
                        for ((slot, &wet_v), &orig) in
                            arr.iter_mut().zip(obuf.iter()).zip(lane.iter())
                        {
                            *slot = wet_v * wet + orig * dry;
                        }
                        lane_out.push(arr);
                    }
                    // 交织输出：每样本位置先写前 lanes 路降噪结果，再写 passthrough 路。
                    // 外层用 enumerate 驱动：i 同时是 wet 帧下标与 inbuf 的步长序号。
                    for (i, _) in obuf.iter().enumerate() {
                        for lane in &lane_out {
                            out.push(clamp_i16(lane[i].round() as i32));
                        }
                        for c in lanes..ch {
                            out.push(self.inbuf[base + i * ch + c]);
                        }
                    }
                }
                // 丢掉已处理的部分，余数留到下一轮（不会有样本丢失）。
                let consumed = n_frames * frame_len;
                self.inbuf.drain(..consumed);
                out
            }
        }
    }
}

/// 离屏降噪试算：把一段交错 i16 PCM 过一遍与录制**完全相同**的处理链
/// （[`MicProcessor`]，模式 + 强度全走真实现），返回处理后的样本。
///
/// 存在的理由：面板上的「试听」是**实时**链 —— 要有麦克风、要有人说话、还要一台
/// 播放设备，自动化测试和排障脚本都用不了它。这条是同一套算法的**离线**入口，
/// `tests/denoise_probe.rs` 用它量"降噪前后噪声底差多少 dB"。
///
/// `setting` 是配置里的降噪取值（`off`/`weak`/`medium`/`strong`，遗留 `gate`/`spectral`/
/// `rnnoise` 也认，统一并入 RNNoise 中等强度）；`channels` 必须是采集格式的**真实**声道数
/// （单声道麦传 1，立体声传 2）—— 声道数传错会让降噪器把半个波形当一路、效果失准。
pub fn denoise_offline(samples: &[i16], channels: usize, setting: &str) -> Vec<i16> {
    let (mode, strength) = parse_mic_denoise(setting);
    MicProcessor::new(mode, strength).process(samples, channels)
}

/// 限幅到 i16 范围。
fn clamp_i16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// WASAPI 裸字节 → i16 交错样本。只认共享模式下实际会出现的两种样本宽度：
/// 16 位整数与 32 位 IEEE float。其它宽度返回 `None`（调用方原样透传）。
///
/// ★ 用 `chunks_exact`：尾部不足一个样本的字节会被**丢弃**而不是 panic，且丢弃量
///   小于一个样本、不会破坏"长度是帧长整数倍"这条前提（`nBlockAlign` 是样本宽度
///   的整数倍）。
fn bytes_to_i16(src: &[u8], bits: u16) -> Option<Vec<i16>> {
    match bits {
        16 => Some(
            src.chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect(),
        ),
        32 => Some(
            src.chunks_exact(4)
                .map(|c| {
                    let f = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                    // 与采集路径同一套换算：[-1,1] 的 float 映射到 i16 满量程。
                    // 下限取 -32768（i16 最小值）而非 -32767：-1.0 该落在最小整数上，
                    // 少一格会让满幅负样本系统性偏小、听感上轻微不对称。
                    (f * 32767.0).clamp(-32768.0, 32767.0) as i16
                })
                .collect(),
        ),
        _ => None,
    }
}

/// [`bytes_to_i16`] 的反向：i16 交错样本 → 与采集端同格式的裸字节。
fn i16_to_bytes(samples: &[i16], bits: u16) -> Vec<u8> {
    match bits {
        32 => {
            let mut out = Vec::with_capacity(samples.len() * 4);
            for &s in samples {
                // 用 `/ 32768.0` 而不是 `/ 32767.0`：与浮点→整数方向的端点对齐，
                // 满幅 -32768 正好回到 -1.0（否则 -32768 → -1.0003 会被播放端削顶）。
                let f = s as f32 / 32768.0;
                out.extend_from_slice(&f.to_le_bytes());
            }
            out
        }
        _ => {
            let mut out = Vec::with_capacity(samples.len() * 2);
            for &s in samples {
                out.extend_from_slice(&s.to_le_bytes());
            }
            out
        }
    }
}

/// 麦克风试听（面板上的「试听」按钮）。
///
/// 把所选麦克风的采集内容**原样**送到系统默认播放设备，让用户当场听到"这个麦 +
/// 这档降噪"的实际效果 —— 否则只能录一段再回放，调参数变成盲调。
///
/// # 关键实现选择
///
/// - **不做任何格式转换**：播放端用 `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM |
///   AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY` 初始化，并把**麦克风的混音格式**原样
///   报给它，让 WASAPI 自己重采样、自己改声道数。于是"麦克风是 44.1k 单声道、
///   扬声器是 48k 立体声"这种常见组合不需要我们写一行重采样代码。
/// - **走与录制同一条降噪链**：试听的用途是"先把参数调好再录"，所以听到的必须就是
///   录出来会有的效果 —— 换档（不降噪 / RNNoise 弱·中·强）或换强度，试听里要能当场听出
///   差别。早期版本为了"听原始声音"跳过降噪，结果是调完参数还得录一段回放才知道对不对，
///   把试听的意义削掉了一半。要听干声就把降噪档切到「不降噪」。
/// - **与录制并行安全**：WASAPI 共享模式允许多个客户端，录制时试听不会互相踢掉。
/// - **不阻塞 UI**：整条链路在自己的线程里，失败只写日志并把错误串返回给调用方。
pub struct MicMonitor {
    stop: Arc<std::sync::atomic::AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
}

impl MicMonitor {
    /// 起试听线程。返回 `Ok` 表示链路已建好（设备能开、格式能协商）。
    ///
    /// `mic_denoise` 与录制配置同源（配置里的降噪取值字符串）：试听要复现录制链路的处理。
    pub fn start(mic_device: &str, mic_denoise: &str) -> std::result::Result<Self, String> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let t_stop = stop.clone();
        let dev = mic_device.to_string();
        let setting = mic_denoise.to_string();
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<(), String>>();
        let join = thread::Builder::new()
            .name("curseen-mic-monitor".into())
            .spawn(move || {
                // 先把"链路建好了没有"回给调用方，再进循环 —— 于是 `start` 能同步报错。
                let r = monitor_loop(&dev, &setting, t_stop, &tx);
                if let Err(e) = r {
                    log::warn!("麦克风试听结束: {e}");
                }
            })
            .map_err(|e| format!("起试听线程失败: {e}"))?;
        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                join: Some(join),
            }),
            Ok(Err(e)) => {
                let _ = join.join();
                Err(e)
            }
            Err(_) => {
                // 超时：多半是设备打开卡住。停掉线程，报一个可读的原因。
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = join.join();
                Err("打开麦克风超时（设备被独占或在切换中）".to_string())
            }
        }
    }

    /// 停止试听（等线程收尾）。
    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for MicMonitor {
    fn drop(&mut self) {
        self.stop();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// 试听主循环：采集端读干 → 播放端写满，中间一个字节队列。
///
/// 队列上限 200ms：麦克风比播放端快的时候（时钟漂移）不设上限会越积越多，
/// 表现是"试听声音越来越延迟"；宁可丢掉最旧的样本，听感上只是偶尔的极小跳变。
#[allow(clippy::too_many_arguments)]
fn monitor_loop(
    mic_device: &str,
    mic_denoise: &str,
    stop: Arc<std::sync::atomic::AtomicBool>,
    ready: &std::sync::mpsc::Sender<std::result::Result<(), String>>,
) -> std::result::Result<(), String> {
    // 与麦克风录制线程同款处理器：同一份实现，参数同源，所以"试听听到的"就是
    // "录进去的"。RNNoise 的状态跨块延续，必须建一次、用整场。
    let (denoise, strength) = parse_mic_denoise(mic_denoise);
    let mut processor = MicProcessor::new(denoise, strength);
    unsafe {
        let _com = ComGuard::new();
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("创建设备枚举器失败: {e}"))?;

        // ── 采集端：按名字选麦，选不到回落默认 ──
        let cap_dev: IMMDevice = match resolve_device(&enumerator, eCapture, mic_device) {
            Some(d) => d,
            None => enumerator
                .GetDefaultAudioEndpoint(eCapture, eConsole)
                .map_err(|e| format!("找不到麦克风设备: {e}"))?,
        };
        let cap_client: IAudioClient = cap_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("激活麦克风失败: {e}"))?;
        let fmt = MixFormat(
            cap_client
                .GetMixFormat()
                .map_err(|e| format!("获取麦克风格式失败: {e}"))?,
        );
        if fmt.block_align() == 0 {
            return Err("麦克风格式异常（nBlockAlign = 0）".to_string());
        }

        let ev =
            CreateEventA(None, false, false, None).map_err(|e| format!("创建试听事件失败: {e}"))?;
        cap_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                hns_from_ms(CAPTURE_BUFFER_MS),
                0,
                fmt.as_ptr(),
                None,
            )
            .map_err(|e| format!("初始化麦克风失败: {e}"))?;
        cap_client
            .SetEventHandle(ev)
            .map_err(|e| format!("设置麦克风事件失败: {e}"))?;
        let cap: IAudioCaptureClient = cap_client
            .GetService()
            .map_err(|e| format!("获取 IAudioCaptureClient 失败: {e}"))?;

        // ── 播放端：用**麦克风的格式**初始化，靠 AUTOCONVERTPCM 让 WASAPI 自己转 ──
        let ren_dev: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("找不到播放设备: {e}"))?;
        let ren_client: IAudioClient = ren_dev
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("激活播放设备失败: {e}"))?;
        ren_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                // ★★ 这条必须小：它的值就是"听到自己声音"的延迟。
                //    原来写成 `1_000_000 * 60`（注释当 60ms，实际 6 秒）时，
                //    `GetBufferSize()` 实测 288000 帧 @48kHz，起播先铺满 6 秒静音。
                hns_from_ms(MONITOR_RENDER_BUFFER_MS),
                0,
                fmt.as_ptr(),
                None,
            )
            .map_err(|e| format!("初始化播放流失败: {e}"))?;
        let ren: IAudioRenderClient = ren_client
            .GetService()
            .map_err(|e| format!("获取 IAudioRenderClient 失败: {e}"))?;
        let buf_frames = ren_client
            .GetBufferSize()
            .map_err(|e| format!("取播放缓冲大小失败: {e}"))?;

        // 起播先塞满静音：否则第一次取缓冲时欠载，会听到"啪"的一声。
        if let Ok(p) = ren.GetBuffer(buf_frames) {
            let _ = ren.ReleaseBuffer(buf_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32);
            let _ = p;
        }
        cap_client
            .Start()
            .map_err(|e| format!("启动麦克风失败: {e}"))?;
        if let Err(e) = ren_client.Start() {
            let _ = cap_client.Stop();
            return Err(format!("启动播放失败: {e}"));
        }

        // 链路已通：放行 `start` 的等待。
        let _ = ready.send(Ok(()));

        let frame_bytes = fmt.block_align();
        let bits = fmt.bits();
        let ch = fmt.channels();
        let max_bytes = (fmt.rate() as usize * frame_bytes / 5).max(frame_bytes);
        let mut pending: Vec<u8> = Vec::with_capacity(max_bytes);

        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            // 1) 把采集端读干（事件驱动；超时 20ms 只是为了能及时看到 stop）。
            loop {
                let avail = match cap.GetNextPacketSize() {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if avail == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if cap
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                let n = frames as usize * frame_bytes;
                if n > 0 && !data.is_null() {
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        pending.resize(pending.len() + n, 0);
                    } else {
                        let src = std::slice::from_raw_parts(data, n);
                        // 降噪要改样本，必须按格式解成 i16 再编回去（解不出格式就原样透传，
                        // 宁可听到未处理的干声，也不要因为强行解读而把声音变成噪声）。
                        match bytes_to_i16(src, bits) {
                            Some(s) => {
                                let out = processor.process(&s, ch);
                                pending.extend_from_slice(&i16_to_bytes(&out, bits));
                            }
                            None => pending.extend_from_slice(src),
                        }
                    }
                }
                let _ = cap.ReleaseBuffer(frames);
                if frames == 0 {
                    break;
                }
            }
            if pending.len() > max_bytes {
                let drop = pending.len() - max_bytes;
                pending.drain(..drop);
            }

            // 2) 往播放端写（按"还剩多少空位"写，写不完的留到下次）。
            if !pending.is_empty() {
                if let Ok(padding) = ren_client.GetCurrentPadding() {
                    let free = buf_frames.saturating_sub(padding) as usize;
                    let n = (free * frame_bytes).min(pending.len()) / frame_bytes * frame_bytes;
                    if n > 0 {
                        if let Ok(p) = ren.GetBuffer((n / frame_bytes) as u32) {
                            if !p.is_null() {
                                std::ptr::copy_nonoverlapping(pending.as_ptr(), p, n);
                                let _ = ren.ReleaseBuffer((n / frame_bytes) as u32, 0);
                                pending.drain(..n);
                            }
                        }
                    }
                }
            }
            // 无事件时睡 5ms（WASAPI 的缓冲是 60ms，5ms 粒度足够，且不空转 CPU）。
            let _ = WaitForSingleObject(ev, 5);
        }

        let _ = ren_client.Stop();
        let _ = cap_client.Stop();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_roundtrip_is_lossless_for_i16_and_close_for_f32() {
        let src: Vec<i16> = vec![i16::MIN, -1234, 0, 1234, i16::MAX];
        let b16 = i16_to_bytes(&src, 16);
        assert_eq!(b16.len(), src.len() * 2);
        assert_eq!(
            bytes_to_i16(&b16, 16).unwrap(),
            src,
            "16 位是同一套整数表示，往返必须逐样本无损"
        );
        let b32 = i16_to_bytes(&src, 32);
        assert_eq!(b32.len(), src.len() * 4);
        let back = bytes_to_i16(&b32, 32).unwrap();
        assert_eq!(back.len(), src.len());
        for (a, b) in src.iter().zip(back.iter()) {
            // f32 路走的是 ±32767 归一化，往返误差 ≤1 个量化步；放宽到 2 只为挡住"量纲写错"
            // 这类会放大 100 倍以上的错误。
            assert!(
                (*a as i32 - *b as i32).abs() <= 2,
                "{a} → {b} 的往返误差过大"
            );
        }
        // 24 位不在支持路径上：必须**返回 None 让调用方原样透传**，而不是猜一个布局。
        assert!(bytes_to_i16(&[0u8; 6], 24).is_none());
    }

    #[test]
    fn nn_denoise_keeps_a_steady_flow_on_mono() {
        // 单声道喂长流：输出是连续的（不短于 输入 - 一帧余量），且不被误当立体声
        // 长度对折/翻倍——后者会让录进 mp4 的音频持续变调。
        let mut p = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Medium);
        let input: Vec<i16> = (0..8192).map(|i| (((i % 100) - 50) * 200) as i16).collect();
        let out = p.process(&input, 1);
        assert!(!out.is_empty());
        let frame = nnnoiseless::FRAME_SIZE; // 单声道一帧余量
        assert!(
            out.len() >= input.len() - frame,
            "单声道输出不该短于输入一帧以上（实得 {}）",
            out.len()
        );
    }

    #[test]
    fn nn_denoise_interleaves_stereo_and_passes_extra_channels_through() {
        // 立体声：输出长度必须是**偶数**（成对回来，不能把 L/R 交错破坏掉）。
        let mut p = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Weak);
        let input: Vec<i16> = (0..4096).map(|i| ((i % 73) as i16) * 7).collect();
        let out = p.process(&input, 2);
        assert!(
            out.len().is_multiple_of(2),
            "立体声输出必须成对: {}",
            out.len()
        );
        // 4 声道：第 3/4 路原样透传 —— 只要长度是 4 的整数倍、且没 panic 就算对
        // （前两路被降噪，无法逐样本比对；这里守的是"别把它们按交错立体声读"）。
        let mut p4 = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Weak);
        let input4: Vec<i16> = (0..4096).map(|i| ((i % 61) as i16) * 5).collect();
        let out4 = p4.process(&input4, 4);
        assert!(
            out4.len().is_multiple_of(4),
            "4 声道输出必须按 4 对齐: {}",
            out4.len()
        );
    }

    #[test]
    fn nn_processor_sizes_its_states_from_the_format_not_hardcoded() {
        // ★ 这是"单声道被当立体声"那条 bug 的直接回归锁：RNNoise 状态的条数必须跟着
        //   **采集格式的声道数**走，而不是永远建两条。老版本写死 `den_l` + `den_r`，
        //   单声道麦克风的样本就被左右两路各分走一半，等于把波形拆成了两块。
        let mut p = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Medium);
        let _ = p.process(&vec![0i16; 2048], 1);
        assert_eq!(p.states.len(), 1, "单声道只该建一条 RNNoise 状态");
        let _ = p.process(&vec![0i16; 2048], 2);
        assert_eq!(p.states.len(), 2, "声道数变成 2 后应扩到两条");
        // 声道数再变回去也不该重建（沿用同一条，RNN 隐藏层不丢）。
        let _ = p.process(&vec![0i16; 2048], 1);
        assert_eq!(p.states.len(), 1, "回到单声道应缩回一条");
        let _ = p.process(&vec![0i16; 2048], 6);
        assert_eq!(p.states.len(), 2, ">2 声道只处理前两路，不该无限建");
    }

    /// 造一个不走真实 WASAPI 的采集器（只为了直接验 `take_samples` 的混音语义）。
    fn stub_capture(mic: Vec<i16>, spk: Vec<i16>, both: bool, aligned: bool) -> AudioCapture {
        AudioCapture {
            mic_buffer: Arc::new(Mutex::new(mic)),
            spk_buffer: Arc::new(Mutex::new(spk)),
            stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            both,
            aligned: std::sync::atomic::AtomicBool::new(aligned),
            idle_since: Mutex::new(None),
            threads: Vec::new(),
        }
    }

    #[test]
    fn mix_sums_two_streams_and_clamps() {
        // 单路语义（`both = false`）：有多少取多少，短的那路补 0。
        let mut ac = stub_capture(vec![100, -100], vec![50, 50], false, false);
        let mixed = ac.take_samples();
        assert_eq!(mixed, vec![150, -50]);
        // 不同长度：短的那路按 0 补齐。
        ac.mic_buffer = Arc::new(Mutex::new(vec![1, 1, 1, 1]));
        ac.spk_buffer = Arc::new(Mutex::new(vec![2, 2]));
        assert_eq!(ac.take_samples(), vec![3, 3, 1, 1]);
        // 限幅：两路都满幅 → 不溢出。
        ac.mic_buffer = Arc::new(Mutex::new(vec![i16::MAX]));
        ac.spk_buffer = Arc::new(Mutex::new(vec![i16::MAX]));
        assert_eq!(ac.take_samples(), vec![i16::MAX]);
    }

    /// ★ 两路同开时**只输出两路都到齐的那部分**（2026-10-09 修"回音"的核心）。
    ///
    /// 旧实现取 `max`，两路交替到达时会把两路**串接**而不是叠加 —— 实测把 3 秒的
    /// 真实音频写成 4.78 秒（1.593 倍）。这条锁住"取 min + 余量留到下一轮"。
    #[test]
    fn mix_takes_only_the_overlap_when_both_lanes_are_on() {
        let ac = stub_capture(vec![1, 1, 1, 1], vec![2, 2], true, true);
        // 只输出重叠的 2 个：1+2。
        assert_eq!(ac.take_samples(), vec![3, 3]);
        // spk 没补货 → 不再输出（而不是把 mic 余下的样本当成 spk 的对应时刻）。
        assert_eq!(
            ac.take_samples(),
            Vec::<i16>::new(),
            "一路没到齐时不能靠另一路凑数"
        );
        // 补给 spk 后，拿到的是刚才留着的 mic 余量，不是新混的错位数据。
        *ac.spk_buffer.lock().expect("锁") = vec![10, 10];
        assert_eq!(ac.take_samples(), vec![11, 11]);
    }

    /// 两路的**起点**只对齐一次：长度差 = 启动时间差 × 采样率，先启动那路的
    /// 前置样本要丢掉，否则两路永远错着一个固定的偏移量（那正是"回音"）。
    #[test]
    fn mix_aligns_lane_starts_exactly_once() {
        // mic 先启动（多出 3 个前置样本）。
        let ac = stub_capture(vec![1, 1, 1, 1, 1], vec![2, 2], true, false);
        assert_eq!(ac.take_samples(), vec![3, 3], "丢掉 mic 的 3 个前置后取重叠");
        // 第二次起不再对齐（即便两路又不等长，也不该再丢）。
        *ac.mic_buffer.lock().expect("锁") = vec![5, 5, 5];
        *ac.spk_buffer.lock().expect("锁") = vec![1];
        assert_eq!(ac.take_samples(), vec![6], "起点只对齐一次");
    }

    /// 另一路**真的没声音**时（设备被拔 / loopback 在没有播放会话时一个包都不产），
    /// 不能一直等 —— 但判据是**等了多久**，不是"堆了多少"（`LANE_IDLE_WAIT`）。
    ///
    /// ★ 这条同时锁住了"持续静默期间每轮都直接输出"：旧实现按堆积量判定，
    ///   静默期一直返回空，麦克风路堆到 0.5 秒才吐一次，与录制层的静音填充
    ///   **双重推进时间线** ⇒ 音轨被拉长 ⇒ 播放出来"声音比画面慢"。
    #[test]
    fn mix_falls_back_to_single_lane_after_the_other_goes_idle() {
        // 麦克风有声、系统声音静默。
        let ac = stub_capture(vec![7; 32], Vec::new(), true, true);
        // 刚出现空缺：先当"这一轮还没到"，不拿一路凑数（否则两路错位 = 回音）。
        assert!(
            ac.take_samples().is_empty(),
            "刚出现空缺 → 先等一轮，不能拿一路凑数"
        );
        assert!(
            ac.idle_since.lock().expect("锁").is_some(),
            "应记下空缺起始时刻"
        );
        // 等过判定时长：认定另一路真的没声音 → 按长的那路输出、短的补 0。
        *ac.idle_since.lock().expect("锁") = Some(Instant::now() - LANE_IDLE_WAIT);
        let out = ac.take_samples();
        assert_eq!(out.len(), 32, "另一路静默 → 按长的那路输出");
        assert_eq!(out[0], 7);
        // ★ 持续静默：之后每轮都直接输出，不能再"等一轮"——否则快的那路会堆积，
        //   与录制层的静音填充双重推进时间线（音轨被拉长 = 声音比画面慢）。
        *ac.mic_buffer.lock().expect("锁") = vec![9; 8];
        assert_eq!(ac.take_samples(), vec![9; 8], "持续静默 → 每轮直接输出");
        // 另一路恢复：两路到齐，空缺状态复位。
        *ac.mic_buffer.lock().expect("锁") = vec![1, 1, 1];
        *ac.spk_buffer.lock().expect("锁") = vec![2, 2, 2];
        assert_eq!(ac.take_samples(), vec![3, 3, 3]);
        assert!(
            ac.idle_since.lock().expect("锁").is_none(),
            "两路到齐 → 空缺状态复位"
        );
    }

    // 注：自带 FFT 的自测已移除 —— FFT 现在由 `nnnoiseless` 内部提供，本文件不再有
    // 自有 FFT 实现，故没有可测的 FFT 单元。

    #[test]
    fn nn_denoise_keeps_silence_near_silent() {
        // RNNoise 在纯静音上应当几乎不动（输出接近 0，不放大、不 panic）。
        let mut p = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Strong);
        let input: Vec<i16> = vec![0i16; nnnoiseless::FRAME_SIZE * 4];
        let out = p.process(&input, 1);
        let rms = (out.iter().map(|&x| (x as f32) * (x as f32)).sum::<f32>()
            / out.len().max(1) as f32)
            .sqrt();
        assert!(rms < 5.0, "静音输出 RMS 应接近 0，实得 {rms}");
    }

    /// 生成一段**确定**的宽带稳态噪声（LCG 白噪声），用来检验 RNNoise 对稳态噪声的压制。
    /// `seed` 固定 → 每次跑出来的噪声完全一致，测试可复现。
    fn make_broadband_noise(n: usize, seed: u64) -> Vec<i16> {
        let mut s = seed;
        (0..n)
            .map(|_| {
                s = s
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                // 取高 31 位映射到 [-1, 1]。
                let r = ((s >> 33) as f32) / (1u64 << 31) as f32 - 1.0;
                (r * 0.4 * 32768.0).clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    /// i16 样本的 RMS（线性幅度，非 dB）。
    fn rms_i16(s: &[i16]) -> f32 {
        (s.iter().map(|&x| (x as f32) * (x as f32)).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn nn_denoise_reduces_steady_noise() {
        // RNNoise 是真实的神经网络降噪：喂稳态宽带噪声（强档），输出能量应被明显压低。
        // 这是"降噪真实有效"的硬判据——只改标签的假实现压不下噪声。
        let input = make_broadband_noise(nnnoiseless::FRAME_SIZE * 64, 0x1234_5678);
        let mut p = MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Strong);
        let out = p.process(&input, 1);
        let in_rms = rms_i16(&input);
        let out_rms = rms_i16(&out);
        println!(
            "稳态噪声 RMS：输入 {in_rms:.1} → 强档 {out_rms:.1}（比值 {:.2}）",
            out_rms / in_rms.max(1e-6)
        );
        // 至少压 3 dB（比值 < 0.707）；RNNoise 在稳态噪声上远不止这点，留足余量防偶发。
        assert!(
            out_rms < in_rms * 0.707,
            "强档应压低稳态噪声：{in_rms:.1} → {out_rms:.1}"
        );
    }

    #[test]
    fn denoise_strength_changes_the_output() {
        // ★ 这条门禁钉的是"强度真的作用到了算法上" —— 只改注释/只改界面标签的
        //   "假实现"会让三档产出完全相同的数值。RNNoise 本身是固定强度的神经网络降噪，
        //   这里用「干湿混合比」区分档位（弱=50% 湿 / 中=80% / 强=100%），所以三档
        //   的输出必须两两不同、且都明显低于"关闭"（证明降噪真的发生了，不是原样透传）。
        //   注：残留 RMS 在干湿混合下并非严格单调（混合会把与原始噪声**去相关**的
        //   RNNoise 输出按比例混回噪声），所以这里只锁"两两不同 + 都低于关闭档"，
        //   不锁单调——那会锁错物理。
        let input = make_broadband_noise(nnnoiseless::FRAME_SIZE * 64, 0x1234_5678);
        let off_rms = rms_i16(
            &MicProcessor::new(DenoiseMode::Off, DenoiseStrength::Medium).process(&input, 1),
        );
        let weak = rms_i16(
            &MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Weak).process(&input, 1),
        );
        let medium = rms_i16(
            &MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Medium).process(&input, 1),
        );
        let strong = rms_i16(
            &MicProcessor::new(DenoiseMode::Rnnoise, DenoiseStrength::Strong).process(&input, 1),
        );
        println!("残留 RMS：关 {off_rms:.1} / 弱 {weak:.1} / 中 {medium:.1} / 强 {strong:.1}");
        // 三档两两必须明显不同（证明强度真的改变了处理、不是空标签）。
        assert!(
            (weak - medium).abs() > 50.0,
            "弱与中应明显不同：{weak:.1} vs {medium:.1}"
        );
        assert!(
            (medium - strong).abs() > 50.0,
            "中与强应明显不同：{medium:.1} vs {strong:.1}"
        );
        assert!(
            (weak - strong).abs() > 50.0,
            "弱与强应明显不同：{weak:.1} vs {strong:.1}"
        );
        // 并且三档都显著低于"关"（证明降噪确实发生了，不是原样透传）。
        assert!(
            weak < off_rms * 0.9 && strong < off_rms * 0.9,
            "三档都应明显低于关闭档：off={off_rms:.1} weak={weak:.1} strong={strong:.1}"
        );
    }
    #[test]
    fn denoise_level_mapping_is_total() {
        assert_eq!(DenoiseStrength::from_level(0), DenoiseStrength::Weak);
        assert_eq!(DenoiseStrength::from_level(1), DenoiseStrength::Medium);
        assert_eq!(DenoiseStrength::from_level(2), DenoiseStrength::Strong);
        // 越界回落中档（配置被手改坏时不至于静音或爆音）
        assert_eq!(DenoiseStrength::from_level(7), DenoiseStrength::Medium);
        assert_eq!(
            DenoiseStrength::from_level(u32::MAX),
            DenoiseStrength::Medium
        );
    }

    #[test]
    fn lists_audio_devices_without_corrupting_the_heap() {
        // 两个枚举都走 COM 属性表（PKEY_Device_FriendlyName + CoTaskMemFree），
        // 曾在这里踩出堆损坏（0xc0000374）；此测试单独守住这条 unsafe 路径。
        let mics = list_mic_devices();
        let spks = list_speaker_devices();
        println!("mic={mics:?} speaker={spks:?}");
    }

    #[test]
    fn denoise_mode_parsing() {
        // 单一下拉：off 不处理；weak/medium/strong RNNoise+对应干湿比；
        // 遗留 gate/spectral/rnnoise 并入 RNNoise·中；其它兜回 Off。
        assert_eq!(
            parse_mic_denoise("off"),
            (DenoiseMode::Off, DenoiseStrength::Medium)
        );
        assert_eq!(
            parse_mic_denoise("weak"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Weak)
        );
        assert_eq!(
            parse_mic_denoise("medium"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Medium)
        );
        assert_eq!(
            parse_mic_denoise("strong"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Strong)
        );
        assert_eq!(
            parse_mic_denoise("gate"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Medium)
        );
        assert_eq!(
            parse_mic_denoise("spectral"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Medium)
        );
        assert_eq!(
            parse_mic_denoise("rnnoise"),
            (DenoiseMode::Rnnoise, DenoiseStrength::Medium)
        );
        assert_eq!(
            parse_mic_denoise("garbage"),
            (DenoiseMode::Off, DenoiseStrength::Medium)
        );
    }

    /// 现场取证：三条 `Initialize`（系统声音 / 麦克风采集 / 试听采集端）都返回
    /// `E_INVALIDARG`（0x80070057）。判据是**同一个共因**：`GetMixFormat` 返回的其实是
    /// `WAVEFORMATEXTENSIBLE`（`cbSize = 22`，整块 40 字节），而 `let f = *ptr` 只拷出
    /// 前面的 `WAVEFORMATEX`（18 字节）——`cbSize` 却仍声明 22。WASAPI 照着 `cbSize`
    /// 去读那 22 字节扩展（`wValidBitsPerSample` / `dwChannelMask` / `SubFormat`），
    /// 读到的是栈上的邻居字节，格式校验必然失败。
    ///
    /// 本探针用**两个独立 client**对比（`IAudioClient::Initialize` 对同一实例只能调一次，
    /// 第二次会返回 `AUDCLNT_E_ALREADY_INITIALIZED`，用同一个 client 比对不出真结果）。
    #[test]
    fn probe_mix_format_truncation() {
        unsafe {
            let _com = ComGuard::new();
            let en: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            for (flow, label) in [(eRender, "系统声音"), (eCapture, "麦克风")] {
                let Ok(dev) = en.GetDefaultAudioEndpoint(flow, eConsole) else {
                    println!("[{label}] 无设备，跳过");
                    continue;
                };
                let flags: u32 = AUDCLNT_STREAMFLAGS_EVENTCALLBACK;

                // A：旧写法——只解引用出 18 字节的 WAVEFORMATEX。
                let ca: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
                let p = ca.GetMixFormat().unwrap();
                let f = *p;
                // `WAVEFORMATEX` 是 `#[repr(packed)]`：字段必须先读进局部变量，
                // 直接塞进 println! 会让格式化宏去取字段引用（E0793 未对齐引用）。
                let (tag, cb, ch, rate, align) = (
                    f.wFormatTag,
                    f.cbSize,
                    f.nChannels,
                    f.nSamplesPerSec,
                    f.nBlockAlign,
                );
                println!(
                    "[{label}] tag=0x{tag:04X} cbSize={cb} ch={ch} rate={rate} align={align} ⟹ 整块 {} 字节",
                    18 + cb as usize
                );
                let ra = ca.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    hns_from_ms(CAPTURE_BUFFER_MS),
                    0,
                    &f,
                    None,
                );
                println!("   A 截断成 18B   → {:?}", ra.map_err(|e| e.code()));
                CoTaskMemFree(Some(p as *const _));

                // B：新写法——把 `cbSize` 声明的整块原样传给 WASAPI。
                let cb: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
                let q = cb.GetMixFormat().unwrap();
                let rb = cb.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    hns_from_ms(CAPTURE_BUFFER_MS),
                    0,
                    q,
                    None,
                );
                println!("   B 完整指针     → {:?}", rb.map_err(|e| e.code()));
                CoTaskMemFree(Some(q as *const _));
            }
        }
    }

    /// 回归锁：`MixFormat` 必须把**整块** `WAVEFORMATEXTENSIBLE` 交给 `Initialize`。
    ///
    /// 这是"勾了麦克风/系统声音却一点声音都没录到"的直接病灶 —— 两条采集路径 + 试听
    /// 的采集端三条 `Initialize` 全部返回 `E_INVALIDARG`。锁的写法是**真开设备**：
    /// 把 `MixFormat::as_ptr()` 喂进 `Initialize`，要求 `Ok`。设备缺失时跳过而非假红。
    #[test]
    fn mix_format_passes_whole_waveformatextensible_to_initialize() {
        unsafe {
            let _com = ComGuard::new();
            let en: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let mut checked = 0;
            for (flow, label) in [(eRender, "系统声音"), (eCapture, "麦克风")] {
                let Ok(dev) = en.GetDefaultAudioEndpoint(flow, eConsole) else {
                    continue;
                };
                let c: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
                let fmt = MixFormat(c.GetMixFormat().unwrap());
                // 前提：共享模式下拿到的必须是 EXTENSIBLE（cbSize = 22 → 整块 40 字节）。
                // 真出现 cbSize = 0 的老式 18 字节格式，说明"守卫的存在前提"变了。
                let cb = (*fmt.0).cbSize;
                assert!(cb > 0, "[{label}] 混音格式不是 EXTENSIBLE（cbSize=0）");
                let r = c.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    hns_from_ms(CAPTURE_BUFFER_MS),
                    0,
                    fmt.as_ptr(),
                    None,
                );
                assert!(
                    r.is_ok(),
                    "[{label}] Initialize 被拒: {:?}",
                    r.map_err(|e| e.code())
                );
                checked += 1;
            }
            if checked == 0 {
                println!("本机没有可用音频端点，跳过（不算失败）");
            }
        }
    }

    /// 纯算术的换算锁：`hnsBufferDuration` 的单位是**100 纳秒**。
    ///
    /// ★ 这条用例存在的唯一理由是"这个错误不报错"。旧代码把 `1_000_000 * 60` 当 60ms
    ///   （实际 6 秒），编译、运行、日志全部正常，只有用户听得到"前 6 秒没声"。
    ///   把换算钉死在数字上，任何人再改回错的值都会当场红。
    #[test]
    fn hns_unit_is_one_hundred_nanoseconds() {
        assert_eq!(hns_from_ms(1), 10_000, "1ms = 10^4 hns");
        assert_eq!(hns_from_ms(1000), 10_000_000, "1s = 10^7 hns");
        assert_eq!(hns_from_ms(CAPTURE_BUFFER_MS), 1_000_000, "100ms");
        assert_eq!(hns_from_ms(MONITOR_RENDER_BUFFER_MS), 600_000, "60ms");
        // 反例：老写法（把 1_000_000 当成 1ms）算出来的是 6 秒，是我们实际想要的 100 倍。
        assert_eq!(1_000_000i64 * 60, hns_from_ms(6_000));
        assert_ne!(1_000_000i64 * 60, hns_from_ms(MONITOR_RENDER_BUFFER_MS));
    }

    /// 端到端锁：**请求的时长必须真的等于 `GetBufferSize()` 折算出来的时长**。
    ///
    /// 只锁换算常数还不够 —— 万一将来有人把参数挪到别的位、或改用别的 API，
    /// 只有"开一条真流、量它的实际缓冲"能兜住。渲染流尤其要锁：它的缓冲时长
    /// **就是试听延迟**（铺满静音后新数据只能排在后面），所以卡一条 1 秒的上限。
    #[test]
    #[ignore = "需要真实音频设备（会开一条渲染流）"]
    fn requested_buffer_duration_shows_up_in_get_buffer_size() {
        let _com = match ComGuard::new() {
            Some(g) => g,
            None => {
                println!("COM 初始化失败，跳过");
                return;
            }
        };
        unsafe {
            let en: IMMDeviceEnumerator =
                match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                    Ok(e) => e,
                    Err(e) => {
                        println!("创建设备枚举器失败，跳过: {e}");
                        return;
                    }
                };
            for (flow, label, want_ms) in [
                (eRender, "渲染", MONITOR_RENDER_BUFFER_MS),
                (eCapture, "采集", CAPTURE_BUFFER_MS),
            ] {
                let Ok(dev) = en.GetDefaultAudioEndpoint(flow, eConsole) else {
                    continue;
                };
                let c: IAudioClient = dev.Activate(CLSCTX_ALL, None).unwrap();
                let fmt = MixFormat(c.GetMixFormat().unwrap());
                let rate = fmt.rate() as f64;
                let flags = if flow == eRender {
                    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY
                } else {
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                };
                if c.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags,
                    hns_from_ms(want_ms),
                    0,
                    fmt.as_ptr(),
                    None,
                )
                .is_err()
                {
                    println!("[{label}] Initialize 失败，跳过");
                    continue;
                }
                let frames = c.GetBufferSize().unwrap_or(0) as f64;
                let secs = frames / rate;
                println!(
                    "[{label}] 请求 {want_ms}ms → {} 帧 @ {rate:.0}Hz = {secs:.3}s",
                    frames as u64
                );
                // 引擎会把缓冲对齐到自己的周期，所以给 3 倍容差；但绝不允许出现
                // "请求 60ms、实得 6 秒"这种 100 倍量级的偏差。
                assert!(
                    secs <= want_ms as f64 / 1000.0 * 3.0,
                    "[{label}] 实际缓冲 {secs:.3}s 远超请求的 {want_ms}ms —— \
                     hnsBufferDuration 的单位又写错了（1ms = 10_000 hns）"
                );
            }
        }
    }
}
