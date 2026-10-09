//! 麦克风试听链路的**定责探针**：把链路拆成两半，分别给出可复现的判据。
//!
//! # 为什么要定责
//!
//! 用户现象是"开着试听、对着麦克风说话，音响里听不到自己"。这条链上有两个**都可能**
//! 独立的失效点，而它们的现象**完全一样**（都是听不到）：
//!
//! 1. **采集端给的是静音** —— 麦克风在系统里被静音（`GetMute`）、音量拉到 0、隐私设置
//!    拦下、或设备上的硬件静音键按下。此时 WASAPI **不报错**，而是给每个缓冲置上
//!    `AUDCLNT_BUFFERFLAGS_SILENT`；我们的代码照实把静音写进播放端 → 一片安静。
//! 2. **播放端出不了声** —— 用**麦克风的混音格式** + `AUTOCONVERTPCM` 去初始化渲染流。
//!    若这条路走不通（被静音、被别的独占流占着、格式协商失败），同样是一片安静。
//!
//! 光看"听不到"分不出是哪一个，所以本探针把两件事**分开测**：
//!
//! - `A. 端点状态`：默认**播放**与默认**采集**端点的友好名 / `GetMute` / 音量标量。
//! - `B. 采集端**：按试听同款参数抓 N 秒，统计"被置 SILENT 的块数 / 总块数"与 RMS。
//!   **这是判据**：静音块占比 ≈ 100% ⇒ 责任在系统/麦克风，不在 CurSeen 的代码。
//! - `C. 播放端`：**不需要任何外部声音**。用一个 440Hz 正弦喂进"按麦克风格式初始化 +
//!   `AUTOCONVERTPCM`"的渲染流（与 `MicMonitor` 完全同款初始化），同时在**同一个播放
//!   端点上开 loopback 采集**，看回采里有没有 440Hz 的能量。
//!   回采得到 ⇒ 播放半段是好的，问题必在采集端；回采也是空的 ⇒ 播放半段坏了。
//!
//! # 跑法
//!
//! ```text
//! cargo test --release --test monitor_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! ★ **必须 `--test-threads=1`**：loopback 与麦克风各占一个音频会话，并行跑会互相干扰，
//!   且和 `capture_smoke`/`audio_probe` 同属"独占音频设备"那一类。
//! ★ 第 B 段想测真实人声就得**在跑的时候对着麦克风说话**；不说话时它照样能测出
//!   "麦克风是否整体被静音"（静音块占比），这已经足够定责。
//! ★ 探针只读不写：不碰配置、不改文件、不启进程。

#![cfg(windows)]

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioCaptureClient, IAudioClient, IAudioRenderClient,
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Threading::{CreateEventA, WaitForSingleObject};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

/// 第 C 段用的测试音。
const TONE_HZ: f64 = 440.0;

/// `hnsBufferDuration` 的换算：**单位是 100 纳秒**（`1 ms = 10_000 hns`）。
///
/// 探针**必须**和生产用同一套数字，否则就失去"复现生产行为"的资格 —— 本文件最早那版
/// 就是照抄了生产里写错的 `1_000_000 * 60`（注释当 60ms，实际 6 秒），于是每次跑都在
/// 第 6 秒才听见测试音，忠实地把 bug 复现了一遍。
fn hns(ms: u64) -> i64 {
    (ms * 10_000) as i64
}

/// 与生产 `MONITOR_RENDER_BUFFER_MS` 对齐：试听渲染流的缓冲时长 = 试听延迟。
const RENDER_BUFFER_MS: u64 = 60;
/// 与生产 `CAPTURE_BUFFER_MS` 对齐：采集流缓冲（事件唤醒 + 5ms 抽干，不构成延迟）。
const CAPTURE_BUFFER_MS: u64 = 100;
/// 判定"回采里真有这支音"的能量下限。实测：正确的配置下 0.25s 箱的 440Hz 能量
/// ≈ 0.35（0.7 幅正弦），而纯静音箱 ≈ 0.00 —— 差两个量级，阈值取 0.15 既不误判
/// 也不会漏掉"声音晚了几秒才来"这种半失效。
const TONE_ENERGY_FLOOR: f64 = 0.15;

// ─────────────────────────── 基础工具 ───────────────────────────

struct ComGuard;
impl ComGuard {
    fn new() -> Self {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        Self
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

/// `GetMixFormat` 缓冲区的守卫 —— **整块**原样传给 `Initialize`，绝不截成 18 字节。
///
/// 这条与生产代码 `audio_capture::MixFormat` 同源：40 字节的 `WAVEFORMATEXTENSIBLE`
/// 被截成 18 字节而 `cbSize` 仍写着 22 时，WASAPI 会越界读并回 `E_INVALIDARG`。
struct MixFormat(*mut WAVEFORMATEX);
impl MixFormat {
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
    fn describe(&self) -> String {
        format!(
            "{}Hz × {}ch × {}bit (blockAlign={}) tag=0x{:04X}{}",
            self.rate(),
            self.channels(),
            self.bits(),
            self.block_align(),
            self.tag(),
            self.subformat_hex()
        )
    }

    fn tag(&self) -> u16 {
        unsafe { (*self.0).wFormatTag }
    }

    /// `WAVE_FORMAT_EXTENSIBLE` 的 16 字节 SubFormat GUID（十六进制）。
    ///
    /// 直接打字节而不用 `GUID::from_u128`：GUID 的内存序是 Data1/2/3 小端、Data4 大端，
    /// 手工拼 u128 极易搞反；肉眼比对两个已知常量更省事：
    /// - PCM 32bit 整数 → `01 00 00 00 00 00 10 00 80 00 00 aa 00 38 9b 71`
    /// - IEEE float   → `03 00 00 00 00 00 10 00 80 00 00 aa 00 38 9b 71`
    ///
    /// ★ 这个区分**直接决定探针喂的数据对不对**：32bit 既可能是整数也可能是浮点，
    ///   按错的那一种填字节，听到的就是噪声或静音。
    fn subformat_hex(&self) -> String {
        unsafe {
            if (*self.0).wFormatTag != 0xFFFE {
                return String::new();
            }
            let base = self.0 as *const u8;
            let mut g = [0u8; 16];
            std::ptr::copy_nonoverlapping(base.add(24), g.as_mut_ptr(), 16);
            let s: Vec<String> = g.iter().map(|b| format!("{b:02x}")).collect();
            format!(" subformat=[{}]", s.join(" "))
        }
    }
}
impl Drop for MixFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
    }
}

unsafe fn friendly_name(device: &IMMDevice) -> String {
    let store: IPropertyStore = match device.OpenPropertyStore(STGM_READ) {
        Ok(s) => s,
        Err(_) => return "<属性表打不开>".into(),
    };
    let mut pv = match store.GetValue(&PKEY_Device_FriendlyName) {
        Ok(v) => v,
        Err(_) => return "<无友好名>".into(),
    };
    if pv.Anonymous.Anonymous.vt != VT_LPWSTR {
        let _ = PropVariantClear(&mut pv);
        return "<友好名不是字符串>".into();
    }
    let pw = pv.Anonymous.Anonymous.Anonymous.pwszVal;
    let s = String::from_utf16_lossy(pw.as_wide());
    let _ = PropVariantClear(&mut pv);
    if s.trim().is_empty() {
        "<空友好名>".into()
    } else {
        s
    }
}

/// 端点状态：友好名 + 是否静音 + 主音量标量。三项都是"用户能自己核对"的事实。
struct EndpointState {
    name: String,
    mute: Option<bool>,
    volume: Option<f32>,
}

unsafe fn endpoint_state(en: &IMMDeviceEnumerator, flow: EDataFlow) -> EndpointState {
    let dev: IMMDevice = match en.GetDefaultAudioEndpoint(flow, eConsole) {
        Ok(d) => d,
        Err(e) => {
            return EndpointState {
                name: format!("<取默认端点失败: {e}>"),
                mute: None,
                volume: None,
            }
        }
    };
    let name = friendly_name(&dev);
    let vol: Option<IAudioEndpointVolume> = dev.Activate(CLSCTX_ALL, None).ok();
    let (mute, volume) = match vol {
        Some(v) => (
            v.GetMute().ok().map(|b| b.as_bool()),
            v.GetMasterVolumeLevelScalar().ok(),
        ),
        None => (None, None),
    };
    EndpointState { name, mute, volume }
}

impl EndpointState {
    fn render(&self) -> String {
        let mute = match self.mute {
            Some(true) => "**已被静音**",
            Some(false) => "未静音",
            None => "静音状态读不到",
        };
        let vol = match self.volume {
            Some(v) => format!("音量 {:.0}%", v * 100.0),
            None => "音量读不到".into(),
        };
        format!("{}（{mute}，{vol}）", self.name)
    }
}

/// i16 交错样本 → 与该格式一致的裸字节（与生产 `i16_to_bytes` 同规则）。
fn i16_to_bytes(samples: &[i16], bits: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * (bits as usize / 8).max(2));
    for &s in samples {
        if bits == 32 {
            // 与生产同一套端点对齐：-32768 → -1.0。
            out.extend_from_slice(&(s as f32 / 32768.0).to_le_bytes());
        } else {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }
    out
}

/// 裸字节 → 归一化到 [-1,1] 的 f32（16bit 整数 / 32bit 浮点两种，其余按 16bit 解）。
fn bytes_to_f32(src: &[u8], bits: u16) -> Vec<f32> {
    if bits == 32 {
        src.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    } else {
        src.chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect()
    }
}

fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sum / samples.len() as f64).sqrt()
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &x| m.max(x.abs()))
}

/// 单频点能量（Goertzel）。窗长取整数个周期的近似值即可，判"有没有这支音"足够。
fn goertzel(samples: &[f32], rate: f64, target: f64) -> f64 {
    let n = samples.len();
    if n == 0 {
        return 0.0;
    }
    let nf = n as f64;
    let k = (0.5 + nf * target / rate).floor();
    let w = 2.0 * std::f64::consts::PI * k / nf;
    let coeff = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = x as f64 + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    ((s1 * s1 + s2 * s2 - coeff * s1 * s2).max(0.0)).sqrt() / nf
}

/// 取**每声道的第一路**，把交错样本拉成单声道（Goertzel 只看一个声道就够）。
fn deinterleave_first(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return samples.to_vec();
    }
    samples.iter().step_by(channels).copied().collect()
}

// ─────────────────────────── B 段：采集端 ───────────────────────────

/// 按试听同款参数抓 `secs` 秒麦克风，统计静音块占比与电平。
struct CaptureReport {
    fmt: String,
    blocks: u64,
    silent_blocks: u64,
    frames: u64,
    rms: f64,
    peak: f32,
}

unsafe fn mic_capture_report(en: &IMMDeviceEnumerator, secs: u32) -> Result<CaptureReport, String> {
    let dev: IMMDevice = en
        .GetDefaultAudioEndpoint(eCapture, eConsole)
        .map_err(|e| format!("取默认麦克风失败: {e}"))?;
    let client: IAudioClient = dev
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活麦克风失败: {e}"))?;
    let fmt = MixFormat(
        client
            .GetMixFormat()
            .map_err(|e| format!("取麦克风格式失败: {e}"))?,
    );
    let (bits, ch, ba) = (fmt.bits(), fmt.channels(), fmt.block_align());
    let ev = CreateEventA(None, false, false, None).map_err(|e| format!("建事件失败: {e}"))?;
    client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            1_000_000 * 60,
            0,
            fmt.as_ptr(),
            None,
        )
        .map_err(|e| format!("初始化麦克风失败: {e}"))?;
    client
        .SetEventHandle(ev)
        .map_err(|e| format!("挂事件失败: {e}"))?;
    let cap: IAudioCaptureClient = client
        .GetService()
        .map_err(|e| format!("取采集客户端失败: {e}"))?;
    client.Start().map_err(|e| format!("启动采集失败: {e}"))?;

    let mut rep = CaptureReport {
        fmt: fmt.describe(),
        blocks: 0,
        silent_blocks: 0,
        frames: 0,
        rms: 0.0,
        peak: 0.0,
    };
    let mut all: Vec<f32> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs as u64);
    while std::time::Instant::now() < deadline {
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
            rep.blocks += 1;
            rep.frames += frames as u64;
            let n = frames as usize * ba;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                // ★ 关键事实：WASAPI 在端点被静音时**不报错**，只是把每块都置上这个标志。
                rep.silent_blocks += 1;
                all.extend(std::iter::repeat_n(0.0f32, frames as usize * ch));
            } else if n > 0 && !data.is_null() {
                let src = std::slice::from_raw_parts(data, n);
                all.extend(bytes_to_f32(src, bits));
            }
            let _ = cap.ReleaseBuffer(frames);
            if frames == 0 {
                break;
            }
        }
        let _ = WaitForSingleObject(ev, 5);
    }
    let _ = client.Stop();
    rep.rms = rms(&all);
    rep.peak = peak(&all);
    Ok(rep)
}

// ─────────────────────────── C 段：播放端 ───────────────────────────

/// 把 440Hz 正弦喂进"麦克风格式 + AUTOCONVERTPCM"的渲染流，同时在**同一个端点**上
/// loopback 回采，证明这支音真的进了混音器。
///
/// 这一段**完全不需要外部输入**，所以它可以独立于"用户有没有说话"给出结论。
unsafe fn render_tone_loopback(
    en: &IMMDeviceEnumerator,
    secs: u32,
) -> Result<(String, f64), String> {
    // ① 拿麦克风的混音格式 —— 与 `MicMonitor` 用同一个格式去初始化渲染流。
    let mic_dev: IMMDevice = en
        .GetDefaultAudioEndpoint(eCapture, eConsole)
        .map_err(|e| format!("取默认麦克风失败: {e}"))?;
    let mic_client: IAudioClient = mic_dev
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活麦克风失败: {e}"))?;
    let mic_fmt = MixFormat(
        mic_client
            .GetMixFormat()
            .map_err(|e| format!("取麦克风格式失败: {e}"))?,
    );
    // `ba`（麦克风的 blockAlign）在这里用不上：写进去的字节由 `i16_to_bytes` 按 `bits` 自己算。
    let (bits, ch, rate) = (mic_fmt.bits(), mic_fmt.channels(), mic_fmt.rate());

    // ② 渲染流：麦克风格式 + AUTOCONVERTPCM（与 `monitor_loop` 逐字同款）。
    let ren_dev: IMMDevice = en
        .GetDefaultAudioEndpoint(eRender, eConsole)
        .map_err(|e| format!("取默认播放设备失败: {e}"))?;
    let ren_client: IAudioClient = ren_dev
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活播放设备失败: {e}"))?;
    ren_client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            hns(RENDER_BUFFER_MS),
            0,
            mic_fmt.as_ptr(),
            None,
        )
        .map_err(|e| format!("初始化播放流失败（这一步最可疑）: {e}"))?;
    let ren: IAudioRenderClient = ren_client
        .GetService()
        .map_err(|e| format!("取播放客户端失败: {e}"))?;
    let buf_frames = ren_client
        .GetBufferSize()
        .map_err(|e| format!("取播放缓冲失败: {e}"))?;

    // ③ 同一个播放端点上的 loopback 采集（用该端点**自己的**混音格式，loopback 不做转换）。
    let lb_client: IAudioClient = ren_dev
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("激活回采流失败: {e}"))?;
    let lb_fmt = MixFormat(
        lb_client
            .GetMixFormat()
            .map_err(|e| format!("取播放端点格式失败: {e}"))?,
    );
    let (lb_bits, lb_ch, lb_ba, lb_rate) = (
        lb_fmt.bits(),
        lb_fmt.channels(),
        lb_fmt.block_align(),
        lb_fmt.rate() as f64,
    );
    lb_client
        .Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            hns(CAPTURE_BUFFER_MS),
            0,
            lb_fmt.as_ptr(),
            None,
        )
        .map_err(|e| format!("初始化回采流失败: {e}"))?;
    let lb: IAudioCaptureClient = lb_client
        .GetService()
        .map_err(|e| format!("取回采客户端失败: {e}"))?;

    // ④ 起播前先铺满静音，避免欠载"啪"声（与生产同款）。
    //
    // ★★ 这一步在生产代码里是"整块 60ms"的意图，但 `hnsBufferDuration` 用错单位后
    //    `GetBufferSize()` 实测是 **288000 帧 @48kHz = 6 秒**。于是"铺满静音"变成了
    //    "把前 6 秒的输出全部占成静音" —— 后面写进去的实时声音只能排在它后面播。
    //    本探针把这三件事都打出来：铺了多少帧、Start 后 padding 是多少、音在时间轴上
    //    第几秒才出现。
    let prefill = match ren.GetBuffer(buf_frames) {
        Ok(p) => {
            let r = match ren.ReleaseBuffer(buf_frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) {
                Ok(()) => format!("成功铺 {buf_frames} 帧静音"),
                Err(e) => format!("ReleaseBuffer 失败: {e}"),
            };
            let _ = p;
            r
        }
        Err(e) => format!("GetBuffer 失败，未铺静音: {e}"),
    };
    let buf_secs = buf_frames as f64 / rate as f64;
    lb_client
        .Start()
        .map_err(|e| format!("启动回采失败: {e}"))?;
    ren_client
        .Start()
        .map_err(|e| format!("启动播放失败: {e}"))?;
    let padding_after_start = ren_client.GetCurrentPadding().unwrap_or(u32::MAX);

    let frames_per_chunk = (rate / 50).max(1) as usize; // 20ms
    let mut t = 0usize;
    // ── 插桩：把"写没写进去"变成可观测量 ──
    // 这三条计数把"渲染半段不出声"的两种可能分开：
    //   `write_ok` 恒为 0 ⇒ 卡在"拿不到可写缓冲"（预铺满静音后 free 一直是 0、或 GetBuffer 出错）；
    //   `write_ok` 很大而回采仍为空 ⇒ 数据进去了但没进混音器（格式/端点问题）。
    let mut write_ok = 0u64;
    let mut frames_written = 0usize;
    let mut write_err: Vec<String> = Vec::new();
    let mut busy_rounds = 0u64;
    let mut captured: Vec<f32> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs as u64);
    while std::time::Instant::now() < deadline {
        // 写一块正弦
        match ren_client.GetCurrentPadding() {
            Ok(padding) => {
                let free = buf_frames.saturating_sub(padding) as usize;
                let want = free.min(frames_per_chunk);
                if want == 0 {
                    busy_rounds += 1;
                } else {
                    match ren.GetBuffer(want as u32) {
                        Ok(p) => {
                            if !p.is_null() {
                                let mut pcm: Vec<i16> = Vec::with_capacity(want * ch);
                                for i in 0..want {
                                    let ph =
                                        2.0 * std::f64::consts::PI * TONE_HZ * ((t + i) as f64)
                                            / rate as f64;
                                    let v = (ph.sin() * 0.7 * 32767.0) as i16;
                                    for _ in 0..ch {
                                        pcm.push(v);
                                    }
                                }
                                let bytes = i16_to_bytes(&pcm, bits);
                                std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
                                let _ = ren.ReleaseBuffer(want as u32, 0);
                                t += want;
                                write_ok += 1;
                                frames_written += want;
                            }
                        }
                        Err(e) => {
                            if write_err.len() < 3 {
                                write_err.push(format!("{e}"));
                            }
                        }
                    }
                }
            }
            Err(e) => {
                if write_err.len() < 3 {
                    write_err.push(format!("GetCurrentPadding: {e}"));
                }
            }
        }
        // 收一块回采
        loop {
            let avail = match lb.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            if avail == 0 {
                break;
            }
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut frames = 0u32;
            let mut flags = 0u32;
            if lb
                .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                .is_err()
            {
                break;
            }
            let n = frames as usize * lb_ba;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 && n > 0 && !data.is_null() {
                let src = std::slice::from_raw_parts(data, n);
                captured.extend(bytes_to_f32(src, lb_bits));
            }
            let _ = lb.ReleaseBuffer(frames);
            if frames == 0 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    let _ = ren_client.Stop();
    let _ = lb_client.Stop();

    let mono = deinterleave_first(&captured, lb_ch);
    // 取中段 0.4s 做频率分析：首尾各有起播/停播边界。
    let win = (lb_rate * 0.4) as usize;
    let mid = if mono.len() > win * 2 {
        &mono[win..mono.len() - win]
    } else {
        &mono[..]
    };
    let r = rms(&mono);
    let pk = peak(&mono);
    let r_mid = rms(mid);
    let head: Vec<String> = mono.iter().take(8).map(|v| format!("{v:.4}")).collect();

    // 每 0.25 秒一箱 —— 这不只是"好看的时间线"，**箱内的 440Hz 能量就是试听延迟的量度**：
    // 第一箱能量冒头的那一刻，就是"说出去的话从音响里出来"的时刻。
    const BIN_SECS: f64 = 0.25;
    let bin = (lb_rate * BIN_SECS) as usize;
    let mut bins = String::new();
    let mut first_tone: Option<f64> = None;
    let mut best_e = 0.0f64;
    let mut best_off = 0.0f64;
    let mut best_t = 0.0f64;
    for (i, chunk) in mono.chunks(bin.max(1)).enumerate() {
        let t0 = i as f64 * BIN_SECS;
        let e = goertzel(chunk, lb_rate, TONE_HZ);
        let off =
            goertzel(chunk, lb_rate, TONE_HZ * 0.7).max(goertzel(chunk, lb_rate, TONE_HZ * 2.5));
        if e > TONE_ENERGY_FLOOR && first_tone.is_none() {
            first_tone = Some(t0);
        }
        if e > best_e {
            best_e = e;
            best_off = off;
            best_t = t0;
        }
        bins.push_str(&format!(
            "\n     [{t0:>5.2}s) RMS {:.4}  能量@440Hz {e:>7.2}  峰值 {:.4}",
            rms(chunk),
            peak(chunk)
        ));
    }
    let latency = first_tone.unwrap_or(f64::INFINITY);
    let verdict = if best_e <= 0.1 || best_e <= best_off * 2.0 {
        "FAIL —— 写进去的 440Hz 没能进混音器（格式或端点问题）".to_string()
    } else if latency <= 1.0 {
        format!("PASS —— 播放半段正常：440Hz 进得了混音器，且首声在 {latency:.2}s 内（说话即闻）")
    } else {
        format!(
            "FAIL —— 音确实出了声，但要等 {latency:.2}s 才听见：渲染缓冲过大。\
             这正是 `hnsBufferDuration` 单位写错（差 100 倍）的典型症状"
        )
    };

    Ok((
        format!(
            "麦克风格式 {}\n\
         \x20 渲染缓冲 {buf_frames} 帧 = {buf_secs:.2} 秒；预铺静音：{prefill}；\
         Start 后 padding {padding_after_start} 帧\n\
         \x20 写入成功 {write_ok} 次 / {frames_written} 帧；缓冲满而空转 {busy_rounds} 次；\
         GetBuffer 报错 {} 条 {:?}\n\
         \x20 回采格式 {}\n\
         \x20 回采样本 {} 个 / RMS {r:.4} / 中段RMS {r_mid:.4} / 峰值 {pk:.4}\n\
         \x20 回采前 8 个样本 {head:?}\n\
         \x20 最强的箱在 {best_t:.2}s：能量@440Hz {best_e:.2} vs 邻频 {best_off:.2}\n\
         \x20 首声时刻 {latency:.2}s\n\
         \x20 时间线（每 {BIN_SECS}s 一箱）：{bins}\n\
         \x20 判定：{verdict}",
            mic_fmt.describe(),
            write_err.len(),
            write_err,
            lb_fmt.describe(),
            mono.len(),
        ),
        latency,
    ))
}

// ─────────────────────────── 用例 ───────────────────────────

#[test]
#[ignore = "需要真实音频设备；跑法见文件头"]
fn audio_endpoints_state() {
    let _g = ComGuard::new();
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).expect("创建设备枚举器失败");
        println!("A. 默认端点状态");
        println!("   播放：{}", endpoint_state(&en, eRender).render());
        println!("   采集：{}", endpoint_state(&en, eCapture).render());
    }
}

#[test]
#[ignore = "需要真实麦克风；跑法见文件头"]
fn mic_capture_is_not_silence() {
    let _g = ComGuard::new();
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).expect("创建设备枚举器失败");
        println!("B. 采集端 3 秒实采（此段请对着麦克风说话，或保持环境声）");
        println!("   采集：{}", endpoint_state(&en, eCapture).render());
        let rep = mic_capture_report(&en, 3).expect("抓麦克风失败");
        let pct = if rep.blocks == 0 {
            0.0
        } else {
            rep.silent_blocks as f64 / rep.blocks as f64 * 100.0
        };
        println!("   格式 {}", rep.fmt);
        println!(
            "   收块 {} 个，其中被 OS 置 SILENT 的 {} 个（{pct:.0}%），共 {} 帧",
            rep.blocks, rep.silent_blocks, rep.frames
        );
        println!("   RMS {:.5} / 峰值 {:.5}", rep.rms, rep.peak);
        let verdict = if rep.blocks == 0 {
            "无数据 —— 设备打不开或被独占"
        } else if rep.silent_blocks == rep.blocks {
            "FAIL —— 麦克风全程静音：系统里被静音 / 音量为 0 / 隐私设置拦下（责任在系统侧，不是 CurSeen）"
        } else if rep.rms < 1.0 {
            "WARN —— 有块但电平近似于 0：可疑静音，需人工确认设备状态"
        } else {
            "PASS —— 麦克风在送真实信号"
        };
        println!("   判定：{verdict}");
    }
}

#[test]
#[ignore = "会在默认播放设备上放 440Hz 测试音；跑法见文件头"]
fn render_half_reaches_the_mixer() {
    let _g = ComGuard::new();
    unsafe {
        let en: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).expect("创建设备枚举器失败");
        println!("C. 播放端自证（无需外部声音）");
        println!("   播放：{}", endpoint_state(&en, eRender).render());
        let (out, latency) = render_tone_loopback(&en, 4).expect("播放端测试失败");
        println!("{out}");
        // ★ 回归锁：这条断言就是 2026-09-19 那个"前 6 秒听不到自己"的 bug。
        //   当时 `hnsBufferDuration` 写成 `1_000_000 * 60`（= 6 秒，注释却写 60ms），
        //   起播先铺满 6 秒静音 → 首声时刻 ≈ 6.0s。修好后应 ≤ 1s（实测 ~0.1s）。
        assert!(
            latency <= 1.0,
            "试听首声延迟 {latency:.2}s —— 渲染缓冲又变大了，检查 hnsBufferDuration 的单位 \
             （1ms = 10_000 hns；写成 `1_000_000 * N` 会放大 100 倍）"
        );
    }
}
