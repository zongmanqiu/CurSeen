//! probe-group: fast
//!
//! # 系统声音「回音」取证探针（`audio_echo`）—— 2026-10-09
//!
//! ## 要回答的问题
//!
//! 用户感觉：录屏产物的**系统声音**有"回音感"（不是人声，是系统声音本身）。
//! "回音"在信号上只有一个客观特征：**同一个事件在时间轴上出现了两次**。
//! 所以本探针不用连续正弦（`rec_tone` 那种 440Hz 看不出重复），而用
//! **稀疏脉冲串**：每 1.0 秒一个 40ms 的 1kHz burst，其余全静音。
//! 脉冲间隔 1s 远大于任何合理的回声延迟（几毫秒到几百毫秒），
//! 于是"一个脉冲变两个"是**不可能看错**的判据。
//!
//! ## 分层定位（两件事分开看，避免互相甩锅）
//!
//! - [`mix_formats`]：**第 0 层，秒级**。打印默认渲染/采集端点的混音格式。
//!   采集侧用的是 `GetMixFormat()` 的原样格式（`audio_capture.rs`），而写 MP4 时
//!   时长按 **48000×2 硬算**（`recorder.rs::write_audio_samples`）。两者一旦不一致
//!   （44100Hz、5.1 声道……），音频时间线就会整体漂移/变速 —— 那是"声画不对位"
//!   和"发闷有空间感"的廉价解释，先排除它。
//! - [`capture_echo`]：**第 1 层，采集层**。进程内一边播脉冲串、一边用生产代码的
//!   `AudioCapture` 抓系统声音，**不经过 MP4/AAC**。若这里就出现双脉冲，
//!   问题在 WASAPI 采集 / 系统侧（声卡音效、多端点重复）；若这里干净，
//!   采集层无罪，该去查录制封装（见 `rec_tone` + `audio_e2e` 那一条链路）。
//!
//! ## 跑法
//!
//! ```text
//! python build/probe.py run audio_echo -- --nocapture
//! ECHO_MIC=1 ...                     # 同时开麦克风（对照：扬声器外放被麦拾到）
//! ECHO_PULSES=6 ...                  # 脉冲个数（默认 8，总时长 ≈ 脉冲数 + 1 秒）
//! ```
//! 产物 `../temp/echo_spk.wav`：可以直接拖进播放器听，听到"哒——哒"而不是"哒哒"。
//!
//! ## 判据
//!
//! 打印每个脉冲的位置与间隔。**期望 = 8 个脉冲、间隔 ≈ 1000ms**。
//! 出现成对的脉冲（间隔 < 250ms）就是回音，成对间隔即回声延迟。

#![cfg(windows)]

use std::time::{Duration, Instant};

use curseen_ui::platform::audio_capture::{denoise_offline, AudioCapture};
use curseen_ui::platform::recorder::{Hardware, Recorder, RecKind, RecOptions, RecordArea};
use windows::core::PCWSTR;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, EDataFlow, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, PlaySoundW, SND_MEMORY, SND_NODEFAULT, SND_SYNC,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};

/// 单个脉冲的时长（ms）。短到像"哒"，长到包络能稳定检出。
const PULSE_MS: u32 = 40;
/// 脉冲频率。
const PULSE_HZ: f32 = 1000.0;
/// 相邻脉冲的间隔（ms）。必须远大于任何可能的回声延迟，否则无法区分。
const PULSE_GAP_MS: u32 = 1000;
/// 判定"这两个峰其实是同一次发声"的最大间隔（ms）。
const ECHO_GAP_MS: u32 = 250;

/// 第 0 层：打印两个默认端点的混音格式（采样率 / 声道 / 位深）。
///
/// ★ 采集侧拿的就是这个格式；写 MP4 的时长却按 48000×2 硬算。两者不一致就是
///   整条音轨的时基错误，先把它排掉再谈"回音"。
#[test]
#[ignore]
fn mix_formats() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        for (name, flow) in [("系统声音(渲染)", eRender), ("麦克风(采集)", eCapture)] {
            match mix_format_of(flow) {
                Some((rate, ch, bits)) => {
                    let per_sec = rate as u64 * ch as u64;
                    println!("{name}: {rate}Hz × {ch}ch × {bits}bit  ⇒ 每秒 {per_sec} 个 i16");
                    if rate != 48000 || ch != 2 {
                        println!(
                            "  ⚠ 与 MP4 写入侧写死的 48000×2 不一致：\
                             时长会被算成实际的 {:.3} 倍",
                            per_sec as f64 / 96_000.0
                        );
                    }
                }
                None => println!("{name}: 取不到混音格式（无默认端点？）"),
            }
        }
    }
}

/// 第 1 层：播脉冲串，同时用生产采集代码抓系统声音，落 WAV + 打印脉冲位置表。
#[test]
#[ignore]
fn capture_echo() {
    let pulses: usize = std::env::var("ECHO_PULSES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let with_mic = std::env::var("ECHO_MIC").is_ok();

    let (rate, ch, bits) = unsafe { mix_format_of(eRender) }.unwrap_or((48000, 2, 16));
    println!("渲染端点混音格式: {rate}Hz × {ch}ch × {bits}bit；麦克风 = {with_mic}");

    // ── 播：脉冲串 WAV，独立线程同步播放（`SND_SYNC` 会阻塞到播完）──
    let wav = pulse_wav(rate, pulses);
    let total_ms = wav_len_ms(&wav);
    let player = std::thread::spawn(move || {
        // SAFETY: `wav` 在本线程内活到播完（`SND_SYNC`），满足 SND_MEMORY 的要求。
        unsafe {
            let _ = PlaySoundW(
                PCWSTR::from_raw(wav.as_ptr().cast::<u16>()),
                None,
                SND_MEMORY | SND_SYNC | SND_NODEFAULT,
            );
        }
    });

    // ── 抓：生产代码的采集器，只开系统声音（麦克风按 ECHO_MIC 决定）──
    let ac = match AudioCapture::start(with_mic, true, "off", "", "") {
        Some(a) => a,
        None => panic!("起不来音频采集（两路都关了？）"),
    };
    let t0 = Instant::now();
    let mut pcm: Vec<i16> = Vec::new();
    let budget = Duration::from_millis(total_ms as u64 + 800);
    while t0.elapsed() < budget {
        pcm.extend(ac.take_samples());
        std::thread::sleep(Duration::from_millis(5));
    }
    ac.stop();
    drop(ac);
    let _ = player.join();

    let frames = pcm.len() / ch.max(1) as usize;
    let secs = frames as f64 / rate.max(1) as f64;
    println!(
        "抓到 {} 个 i16 ≈ {} 帧 ≈ {:.3}s（目标 {:.3}s），RMS {:.1}",
        pcm.len(),
        frames,
        secs,
        total_ms as f64 / 1000.0,
        rms(&pcm)
    );
    assert!(secs > 1.0, "几乎没抓到声音（{secs:.3}s）—— 采集没起来？");

    let path = std::path::PathBuf::from("../temp/echo_spk.wav");
    write_wav(&path, &pcm, rate, ch);
    println!("产物: {}（可直接播放听）", path.display());

    // ── 判：包络找峰，报位置与间隔 ──
    let peaks = find_peaks(&pcm, rate, ch);
    println!("检出 {} 个脉冲（期望 {pulses}）", peaks.len());
    let mut prev: Option<u32> = None;
    for p in &peaks {
        let gap = prev.map(|q| p.saturating_sub(q));
        println!(
            "  {:>7.0}ms{}",
            *p as f64,
            match gap {
                Some(g) => format!("  间隔 {g}ms"),
                None => String::new(),
            }
        );
        prev = Some(*p);
    }
    report(&peaks, pulses);
}

/// 第 1.2 层：两路同开时，输出速率还是不是实时的？（**这是"回音"的根因判据**）
///
/// # 假设
///
/// `AudioCapture::take_samples` 是这么混音的：
///
/// ```text
/// let n = mic.len().max(spk.len());      // 取两路里较长的
/// for i in 0..n { out.push(mic[i] + spk[i]) }   // 按"buffer 内相对位置"逐样本相加
/// ```
///
/// 它**没有时间戳对齐** —— 两路各是一条独立线程、各自 100ms 缓冲、各自被 WASAPI
/// 事件唤醒，谁先到谁写在前面。于是"第 i 个麦克风样本"与"第 i 个系统声音样本"
/// **根本不是同一时刻的声音**。
///
/// 后果分两条，都是用户能听见的：
/// 1. **同一段声音出现两次**：系统声音走 spk 路直达，又被麦克风拾到走 mic 路，
///    两路错开 Δ 毫秒后相加 —— Δ 在 20~50ms 就是明确的"回音"；
/// 2. **输出被拉长**：`max()` 逐轮取两路里较长的那个，两路交替积压时
///    `Σ max(mic_i, spk_i) > max(Σmic, Σspk)`，最多可达 2 倍。写进 MP4 的音频
///    比真实时间长 → 音画不同步、且听起来"慢半拍"。
///
/// # 判据
///
/// 各跑 `SECS` 秒，比较抓到的样本数：**单路应当 ≈ 1.00 倍实时**；
/// 若"两路同开"明显高于 1.00 倍，第 2 条就被证实了。
#[test]
#[ignore]
fn rate_check() {
    let secs = 3u64;
    let (rate, ch, _) = unsafe { mix_format_of(eRender) }.unwrap_or((48000, 2, 16));
    let per_sec = rate as f64 * ch.max(1) as f64;
    println!("参考：{rate}Hz × {ch}ch ⇒ 每秒 {per_sec:.0} 个 i16");

    // ★ 必须**开着播放**测：实测 loopback 在没有任何程序发声时**一个包都不产**
    //   （"只开系统声音 + 静音"那次抓到 0 个样本），那样两路对照就没有意义。
    for (name, mic, spk) in [
        ("只开系统声音", false, true),
        ("只开麦克风  ", true, false),
        ("两路同开    ", true, true),
    ] {
        let wav = tone_wav(rate, secs + 1);
        let player = std::thread::spawn(move || {
            // SAFETY: `wav` 在本线程内活到播完（`SND_SYNC`），满足 SND_MEMORY 的要求。
            unsafe {
                let _ = PlaySoundW(
                    PCWSTR::from_raw(wav.as_ptr().cast::<u16>()),
                    None,
                    SND_MEMORY | SND_SYNC | SND_NODEFAULT,
                );
            }
        });
        let Some(ac) = AudioCapture::start(mic, spk, "off", "", "") else {
            println!("{name}: 采集起不来");
            let _ = player.join();
            continue;
        };
        let t0 = Instant::now();
        let mut n = 0usize;
        while t0.elapsed() < Duration::from_secs(secs) {
            n += ac.take_samples().len();
            std::thread::sleep(Duration::from_millis(5));
        }
        ac.stop();
        drop(ac);
        let _ = player.join();
        let got = n as f64 / per_sec;
        println!(
            "{name}: {n} 个 i16 ≈ {got:.3}s / 真实 {secs}s ⇒ **{:.3} 倍**",
            got / secs as f64
        );
    }
}

/// 一段连续低音量正弦（`rate` 采样率、`secs` 秒）——只用来让 loopback 有声音可采。
fn tone_wav(rate: u32, secs: u64) -> Vec<u8> {
    let frames = secs as usize * rate as usize;
    let mut s = vec![0i16; frames * 2];
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        let v = (2.0 * std::f32::consts::PI * PULSE_HZ * t).sin() * 0.3;
        let q = (v * 32767.0) as i16;
        s[i * 2] = q;
        s[i * 2 + 1] = q;
    }
    wrap_wav(&s, rate, 2)
}

/// 第 1.5 层：降噪器给信号加了多少延迟、把波形改了多少？（**纯逻辑，不需要声卡**）
///
/// ★ 为什么单测它：系统声音那路**根本不过降噪** —— `run_capture_loop` 里只有
///   `is_mic` 才建 `MicProcessor`，系统声音传的是 `None`（原样透传）。所以
///   "降噪导致系统声音回音"要成立，只能走一条间接链：
///
///   **麦克风拾到了扬声器外放 → 被 RNNoise 处理（带缓冲延迟 + 波形扭曲）→
///     与系统声音逐样本相加**
///
///   相加的两路里，一路是原样的系统声音，另一路是它的**延迟副本** —— 这在信号上
///   正是"回声"的定义。所以这里量两件事：延迟多少毫秒、波形被改了多少。
#[test]
#[ignore]
fn denoise_delay() {
    let rate = 48000u32;
    let total = rate as usize * 4;
    let mut input = vec![0i16; total];
    // 6 个 10ms 脉冲，间隔 500ms。
    for k in 0..6usize {
        let a = (100 + k * 500) * rate as usize / 1000;
        for j in 0..(rate as usize / 100) {
            if a + j < total {
                let t = j as f32 / rate as f32;
                let v = (2.0 * std::f32::consts::PI * PULSE_HZ * t).sin();
                input[a + j] = (v * 0.6 * 32767.0) as i16;
            }
        }
    }
    let inp = find_peaks(&input, rate, 1);
    println!("输入脉冲位置 {inp:?}（{:?}）", inp);

    for setting in ["off", "weak", "medium", "strong"] {
        let out = denoise_offline(&input, 1, setting);
        let peaks = find_peaks(&out, rate, 1);
        let (ri, ro) = (rms(&input), rms(&out));
        println!(
            "  {setting:>6}: 输出 {} / 输入 {} 样本，整段 RMS {ri:.0} → {ro:.0}（保留 {:.0}%）",
            out.len(),
            input.len(),
            ro / ri.max(1e-9) * 100.0
        );
        if peaks.len() == inp.len() {
            let d: Vec<i32> = inp
                .iter()
                .zip(&peaks)
                .map(|(a, b)| *b as i32 - *a as i32)
                .collect();
            println!("        脉冲位置 {peaks:?} → 延迟 {d:?} ms");
        } else {
            println!(
                "        脉冲只检出 {} 个（输入 {} 个）——RNNoise 把它当非语音压掉了",
                peaks.len(),
                inp.len()
            );
        }
    }
}

/// 第 2 层：**真录一段**（走完整的 MP4 + AAC 链路），解码后再数脉冲。
///
/// ★ 第 1 层证明"进编码器的 PCM 是干净的"，但用户听的是**解码后的产物**。
///   用户点名的是系统音量"叮"声 —— 那是**极短瞬态**，而 AAC 这类变换编码器对
///   瞬态有著名的 **pre-echo（前回声）**缺陷：一帧（1024 样本 ≈ 21ms）内的能量
///   被摊到整帧，听感就是"哒"变成"哒——"。这一层是唯一能看见它的地方。
#[test]
#[ignore]
fn rec_echo() {
    let pulses = 6usize;
    let (rate, _ch, _) = unsafe { mix_format_of(eRender) }.unwrap_or((48000, 2, 16));
    let wav = pulse_wav(rate, pulses);
    let total_ms = wav_len_ms(&wav);

    let path = std::path::PathBuf::from("../temp/echo_rec.mp4");
    let _ = std::fs::remove_file(&path);

    // 先让录制跑起来（DXGI/MF 初始化要几百毫秒），过 0.5s 再播，免得漏掉前几个脉冲。
    let rec = Recorder::start(RecOptions {
        kind: RecKind::Mp4,
        path: path.clone(),
        area: RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 1280,
        hardware: Hardware::Auto,
        show_cursor: false,
        capture_mic: false,
        capture_speaker: true,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    std::thread::sleep(Duration::from_millis(500));

    let player = std::thread::spawn(move || {
        // SAFETY: `wav` 在本线程内活到播完（`SND_SYNC`），满足 SND_MEMORY 的要求。
        unsafe {
            let _ = PlaySoundW(
                PCWSTR::from_raw(wav.as_ptr().cast::<u16>()),
                None,
                SND_MEMORY | SND_SYNC | SND_NODEFAULT,
            );
        }
    });
    std::thread::sleep(Duration::from_millis(total_ms as u64 + 1500));
    let outcome = rec.finish();
    let _ = player.join();

    println!(
        "录制结束 bytes={:?} error={:?}",
        outcome.bytes,
        outcome.error
    );
    assert!(outcome.error.is_none(), "录制不应报错: {:?}", outcome.error);
    let data = std::fs::read(&path).expect("产物读不出来");
    assert!(data.windows(4).any(|w| w == b"moov"), "产物没有 moov 盒");

    let decoded = unsafe { decode_mp4_audio(&path) };
    let (pcm, arate, ach) = decoded.expect("解码音轨失败");
    let secs = pcm.len() as f64 / arate.max(1) as f64 / ach.max(1) as f64;
    println!(
        "解码出 {} 个 i16：{arate}Hz × {ach}ch ≈ {secs:.3}s，RMS {:.0}",
        pcm.len(),
        rms(&pcm)
    );
    write_wav(&std::path::PathBuf::from("../temp/echo_rec.wav"), &pcm, arate, ach);
    println!("产物: ../temp/echo_rec.wav（可直接播放听）");

    let peaks = find_peaks(&pcm, arate, ach);
    println!("检出 {} 个脉冲（期望 {pulses}）", peaks.len());
    let mut prev: Option<u32> = None;
    for p in &peaks {
        let gap = prev.map(|q| p.saturating_sub(q));
        println!(
            "  {:>7}ms{}",
            *p,
            match gap {
                Some(g) => format!("  间隔 {g}ms"),
                None => String::new(),
            }
        );
        prev = Some(*p);
    }
    // ★ 录制链路的判据比采集层松一档：AAC 会**展宽**脉冲（pre-echo 就是这么来的），
    //   所以这里不要求"段数恰好"，只要求**不出现成对的重复脉冲**。
    let echoes: Vec<u32> = peaks
        .windows(2)
        .filter_map(|w| {
            let g = w[1].saturating_sub(w[0]);
            (g > 0 && g < ECHO_GAP_MS).then_some(g)
        })
        .collect();
    println!("成对脉冲（间隔 < {ECHO_GAP_MS}ms）: {echoes:?}");
    // 顺带量"脉冲被展宽了多少"：这是 pre-echo 的量化指标。
    if let (Some(a), Some(b)) = (peaks.first(), peaks.last()) {
        println!(
            "首末脉冲跨度 {}ms（播放端跨度 {}ms）",
            b - a,
            (pulses as u32 - 1) * PULSE_GAP_MS
        );
    }
    assert!(echoes.is_empty(), "录屏产物出现回音：{echoes:?}ms（成对脉冲）");
}

/// `pitch_check`：全链路音高取证。播放连续 1kHz 标准音，走用户的真实配置
/// （麦克风+系统声音同开、denoise=off）真录 ~7s，解码写 WAV 交给 Python FFT。
/// 判据在 Python 侧：FFT 主峰若明显偏离 1kHz（>±2%），管道在变速变调。
#[test]
#[ignore = "真录 7s + 播放（约 12s）；跑法见文件头"]
fn pitch_check() {
    let (rate, _ch, _) = unsafe { mix_format_of(eRender) }.unwrap_or((48000, 2, 16));
    // 连续 1kHz 正弦 4.0s，立体声，幅度 0.5（够响又不削波）。
    let frames = 4 * rate as usize;
    let mut s = vec![0i16; frames * 2];
    for i in 0..frames {
        let t = i as f32 / rate as f32;
        let fade = (i as f32 / (rate / 100) as f32).min(1.0)
            * (((frames - i) as f32) / (rate / 100) as f32).min(1.0);
        let v = (2.0 * std::f32::consts::PI * 1000.0 * t).sin() * 0.5 * fade;
        let q = (v * 32767.0) as i16;
        s[i * 2] = q;
        s[i * 2 + 1] = q;
    }
    let wav = wrap_wav(&s, rate, 2);

    let path = std::path::PathBuf::from("../temp/pitch_rec.mp4");
    let _ = std::fs::remove_file(&path);
    let rec = Recorder::start(RecOptions {
        kind: RecKind::Mp4,
        path: path.clone(),
        area: RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 1280,
        hardware: Hardware::Auto,
        show_cursor: false,
        capture_mic: true,      // ← 用户配置：两路同开
        capture_speaker: true,
        mic_denoise: "off".into(), // ← 用户配置：降噪关
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    std::thread::sleep(Duration::from_millis(500));
    let player = std::thread::spawn(move || unsafe {
        let _ = PlaySoundW(
            PCWSTR::from_raw(wav.as_ptr().cast::<u16>()),
            None,
            SND_MEMORY | SND_SYNC | SND_NODEFAULT,
        );
    });
    std::thread::sleep(Duration::from_millis(500 + 4000 + 1500));
    let outcome = rec.finish();
    let _ = player.join();
    assert!(outcome.error.is_none(), "录制不应报错: {:?}", outcome.error);

    let (pcm, arate, ach) = unsafe { decode_mp4_audio(&path) }.expect("解码音轨失败");
    let secs = pcm.len() as f64 / arate.max(1) as f64 / ach.max(1) as f64;
    println!("解码 {arate}Hz × {ach}ch ≈ {secs:.3}s");
    write_wav(
        &std::path::PathBuf::from("../temp/pitch_rec.wav"),
        &pcm,
        arate,
        ach,
    );
    println!("产物: ../temp/pitch_rec.wav（交给 Python FFT 量主峰频率）");
}

/// 把一个 MP4 的音轨解成 16-bit 交错 PCM，返回 `(样本, 采样率, 声道数)`。
///
/// 走 MF `SourceReader`：AAC→PCM 的解码由系统变换自动完成，不碰生产代码，
/// 于是"生产代码写进去的东西"与"这里读出来的东西"是两条独立实现，能互相印证。
unsafe fn decode_mp4_audio(path: &std::path::Path) -> Option<(Vec<i16>, u32, u16)> {
    MFStartup(MF_VERSION, 0).ok()?;
    let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
    wide.push(0);
    let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None).ok()?;

    // ★ 流索引不等于 sink 写入顺序：按 major type == Audio 自动定位（audio_e2e 实测）。
    let mut aud = None;
    for i in 0..4u32 {
        let major = reader
            .GetNativeMediaType(i, 0)
            .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
            .unwrap_or_default();
        if major == MFMediaType_Audio {
            aud = Some(i);
            break;
        }
    }
    let aud = aud?;
    reader
        .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
        .ok()?;
    reader.SetStreamSelection(aud, true).ok()?;

    let native = reader.GetNativeMediaType(aud, 0).ok()?;
    let rate = native
        .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
        .unwrap_or(48000);
    let ch = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2) as u16;

    // 必须构造**完整**的 PCM 类型（缺任一属性都会被拒 MF_E_INVALIDMEDIATYPE）。
    let ty = MFCreateMediaType().ok()?;
    ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).ok()?;
    ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).ok()?;
    ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).ok()?;
    ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch as u32).ok()?;
    ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).ok()?;
    ty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch as u32 * 2).ok()?;
    ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * ch as u32 * 2)
        .ok()?;
    ty.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1).ok()?;
    reader.SetCurrentMediaType(aud, None, &ty).ok()?;

    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(aud, 0, None, Some(&mut flags), None, Some(&mut sample))
            .ok()?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(s) = sample else { break };
        let buf = s.ConvertToContiguousBuffer().ok()?;
        let mut data = std::ptr::null_mut();
        // ★ Lock 第三参是**当前有效长度**：漏传会把整块缓冲尾的垃圾也拷进来。
        let mut max_len = 0u32;
        let mut cur = 0u32;
        buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur)).ok()?;
        bytes.extend_from_slice(std::slice::from_raw_parts(data, cur as usize));
        buf.Unlock().ok();
    }
    let pcm: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some((pcm, rate, ch))
}

/// 依据峰位置给出结论并断言：不允许"成对脉冲"。
fn report(peaks: &[u32], expect: usize) {
    let echoes: Vec<u32> = peaks
        .windows(2)
        .filter_map(|w| {
            let g = w[1].saturating_sub(w[0]);
            (g > 0 && g < ECHO_GAP_MS).then_some(g)
        })
        .collect();
    if echoes.is_empty() {
        println!(
            "结论：未检出成对脉冲（相邻间隔全部 ≥ {ECHO_GAP_MS}ms）→ 采集层没有回音"
        );
    } else {
        println!(
            "结论：检出 {} 处成对脉冲，回声延迟 {:?}ms → 采集层确实有重复",
            echoes.len(),
            echoes
        );
    }
    // 间隔明显小于 1s 的成对峰 = 回音。个数多于期望则是脉冲被切碎，一并报出来。
    assert!(
        echoes.is_empty(),
        "系统声音出现回音：{echoes:?}ms（成对脉冲）"
    );
    assert!(
        peaks.len() <= expect + 2,
        "脉冲被切碎了：检出 {} 个 > 期望 {expect} 个",
        peaks.len()
    );
}

/// 包络找峰，返回每个脉冲的中心毫秒位置。
///
/// ★ 三个参数都是被"假红"教育出来的（首轮用 1ms 窗 + 单阈值 25%，把一段 360ms 的
///   外来声音切成了 60 个碎片，误报"59 处回音"）：
/// - **10ms 窗**：1ms 窗对一个周期就 1ms 的 1kHz 信号太窄，包络抖得厉害；
/// - **迟滞阈值**（起 40% / 落 15%）：单阈值会让包络在阈值附近反复穿越，
///   一段连续音被切成一串碎片。迟滞保证"一旦算起就算到底"；
/// - **长度上限 150ms**：脉冲本身 40ms，回声也是短脉冲；系统里飘进来的
///   音乐 / 通知音是几百毫秒的连续音，不该被当成脉冲参与判据。
fn find_peaks(pcm: &[i16], rate: u32, ch: u16) -> Vec<u32> {
    let ch = ch.max(1) as usize;
    let per_win = (rate.max(1) / 100).max(1) as usize * ch; // 10ms
    let mut env: Vec<f64> = Vec::new();
    let mut i = 0usize;
    while i + per_win <= pcm.len() {
        let win = &pcm[i..i + per_win];
        let s: f64 = win.iter().map(|v| (*v as f64) * (*v as f64)).sum();
        env.push((s / win.len() as f64).sqrt());
        i += per_win;
    }
    if env.is_empty() {
        return Vec::new();
    }
    let top = env.iter().cloned().fold(0.0f64, f64::max);
    let rise = (top * 0.40).max(1.0);
    let fall = top * 0.15;
    let mut out = Vec::new();
    let mut run: Option<usize> = None;
    for (k, v) in env.iter().enumerate() {
        match run {
            Some(a) if *v < fall => {
                if (k - a) as u32 * 10 <= MAX_PULSE_MS {
                    out.push(((a + k) * 5) as u32); // 窗序号 ×10ms → ms，取中心
                }
                run = None;
            }
            Some(_) => {}
            None if *v >= rise => run = Some(k),
            None => {}
        }
    }
    if let Some(a) = run {
        if (env.len() - a) as u32 * 10 <= MAX_PULSE_MS {
            out.push(((a + env.len()) * 5) as u32);
        }
    }
    out
}

/// 一个脉冲的最大长度（ms）——超过它就不是脉冲，是飘进来的连续音。
const MAX_PULSE_MS: u32 = 150;

fn rms(pcm: &[i16]) -> f64 {
    if pcm.is_empty() {
        return 0.0;
    }
    let s: f64 = pcm.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    (s / pcm.len() as f64).sqrt()
}

/// 合成脉冲串 WAV：每 `PULSE_GAP_MS` 一个 `PULSE_MS` 的 1kHz burst，其余静音。
fn pulse_wav(rate: u32, pulses: usize) -> Vec<u8> {
    let rate = rate.max(8000);
    let total_ms = 400 + pulses as u32 * PULSE_GAP_MS + 400;
    let frames = total_ms as usize * rate as usize / 1000;
    let mut s = vec![0i16; frames * 2];
    for k in 0..pulses {
        let start_ms = 400 + k as u32 * PULSE_GAP_MS;
        let a = start_ms as usize * rate as usize / 1000;
        let n = PULSE_MS as usize * rate as usize / 1000;
        let fade = (rate / 1000 * 3).max(1) as usize; // 3ms 淡入淡出，防咔哒声
        for j in 0..n {
            let t = j as f32 / rate as f32;
            let env = {
                let up = (j as f32 / fade as f32).min(1.0);
                let down = ((n - j) as f32 / fade as f32).min(1.0);
                up.min(down)
            };
            let v = (2.0 * std::f32::consts::PI * PULSE_HZ * t).sin() * env * 0.6;
            let q = (v * 32767.0) as i16;
            if a + j < frames {
                s[(a + j) * 2] = q;
                s[(a + j) * 2 + 1] = q;
            }
        }
    }
    wrap_wav(&s, rate, 2)
}

/// 给 PCM 加 44 字节 WAV 头。
fn wrap_wav(s: &[i16], rate: u32, ch: u16) -> Vec<u8> {
    let data_len = s.len() * 2;
    let mut v = Vec::with_capacity(44 + data_len);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&ch.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * ch as u32 * 2).to_le_bytes());
    v.extend_from_slice(&(ch * 2).to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    for x in s {
        v.extend_from_slice(&x.to_le_bytes());
    }
    v
}

/// 从 WAV 字节读总时长（ms）。
fn wav_len_ms(wav: &[u8]) -> u32 {
    if wav.len() < 44 {
        return 0;
    }
    let data_len = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]) as usize;
    let rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]).max(1);
    let ch = u16::from_le_bytes([wav[22], wav[23]]).max(1) as usize;
    (data_len as u64 / 2 / ch as u64 * 1000 / rate as u64) as u32
}

fn write_wav(path: &std::path::Path, pcm: &[i16], rate: u32, ch: u16) {
    let bytes = wrap_wav(pcm, rate, ch);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(path, &bytes) {
        Ok(()) => {}
        Err(e) => println!("写 {path:?} 失败: {e}"),
    }
}

/// 取默认端点的混音格式 `(采样率, 声道, 位深)`。
unsafe fn mix_format_of(flow: EDataFlow) -> Option<(u32, u16, u16)> {
    let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
    let dev = en.GetDefaultAudioEndpoint(flow, eConsole).ok()?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).ok()?;
    let p = client.GetMixFormat().ok()?;
    let w = &*p;
    let got = (w.nSamplesPerSec, w.nChannels, w.wBitsPerSample);
    CoTaskMemFree(Some(p as *const _));
    Some(got)
}
