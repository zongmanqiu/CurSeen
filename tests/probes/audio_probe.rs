//! 录屏产物音频验收：把 MP4 的音轨**真解成 PCM**，用 RMS + 目标频率能量当判据。
//!
//! # 为什么不能只看"音轨有几个样本"
//!
//! `mp4_table.py` 能数出 `stts` 里的样本数，但那是**容器层的计数**。AAC 是 CBR 的，
//! 一段纯数字静音同样会编出时长正确、每帧 ~341 字节的包 —— 于是"样本数正常"与
//! "真的录到了声音"是两件事。要区分它们，只有解码。
//!
//! # 判据
//!
//! 配合 `temp/audio_tone_probe.py` 使用：录制期间往默认输出设备放一段 **1kHz 正弦**。
//! 于是产物里应该同时看到
//!   - `RMS` 明显大于静音底噪（本机实测静音 RMS ≈ 0；1kHz 满幅正弦 RMS ≈ 9000）；
//!   - **1kHz 处的能量显著高于邻频**（Goertzel）——这一条把"录到了别的噪声/环境声"
//!     也排除掉，证明回路真的把系统声音送进来了。
//!
//! 只测 loopback（系统声音）这条链：麦克风那路依赖现场有没有声音，无法自证。
//!
//! 跑法（要有现成的产物文件）：
//! ```text
//! $env:CURSEEN_AUDIO_PROBE_MP4="E:\...\CurSeen_*.mp4"
//! cargo test --release --test audio_probe -- --ignored --nocapture
//! ```

#![cfg(windows)]

use std::sync::OnceLock;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

/// 探针默认找的目标频率（与 `audio_tone_probe.py` 放的一致）。
const TONE_HZ: f64 = 1000.0;
/// 判定"有声音"的 RMS 下限（int16 满幅 = 32767）。
const RMS_FLOOR: f64 = 300.0;

unsafe fn read_all_pcm(path: &str) -> std::result::Result<(Vec<i16>, u32, u32), String> {
    let hpath = windows::core::HSTRING::from(path);
    let reader: IMFSourceReader =
        MFCreateSourceReaderFromURL(&hpath, None).map_err(|e| format!("打开产物失败: {e}"))?;

    let mut sel = 0u32;
    let mut found_audio = false;
    while reader.GetStreamSelection(sel).is_ok() {
        let mt = reader
            .GetNativeMediaType(sel, 0)
            .map_err(|e| format!("取原生类型失败(流{sel}): {e}"))?;
        let major = mt.GetGUID(&MF_MT_MAJOR_TYPE).map_err(|e| e.to_string())?;
        if major == MFMediaType_Audio {
            found_audio = true;
            break;
        }
        sel += 1;
    }
    if !found_audio {
        return Err("产物里没有音频流".to_string());
    }

    // 让 Source Reader 插一个解码器，把 AAC 解成 16bit PCM。
    let pcm: IMFMediaType = MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    pcm.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
        .map_err(|e| e.to_string())?;
    pcm.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
        .map_err(|e| e.to_string())?;
    reader
        .SetCurrentMediaType(sel, None, &pcm)
        .map_err(|e| format!("设 PCM 输出类型失败: {e}"))?;
    let cur = reader
        .GetCurrentMediaType(sel)
        .map_err(|e| format!("取当前类型失败: {e}"))?;
    let rate = cur
        .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
        .unwrap_or(48000);
    let ch = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2);

    let mut out: Vec<i16> = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut ts = 0i64;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(
                sel,
                0,
                None,
                Some(&mut flags),
                Some(&mut ts),
                Some(&mut sample),
            )
            .map_err(|e| format!("ReadSample 失败: {e}"))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(s) = sample else { continue };
        let buf: IMFMediaBuffer = match s.ConvertToContiguousBuffer() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut data = std::ptr::null_mut();
        let mut max_len = 0u32;
        if buf.Lock(&mut data, Some(&mut max_len), None).is_err() {
            continue;
        }
        if !data.is_null() {
            let n = (max_len / 2) as usize;
            let sl = std::slice::from_raw_parts(data as *const i16, n);
            out.extend_from_slice(sl);
        }
        let _ = buf.Unlock();
    }
    Ok((out, rate, ch))
}

/// Goertzel：算某个频率上的能量（比整段 FFT 轻得多，只要一个 bin）。
fn goertzel(samples: &[i16], rate: u32, freq: f64) -> f64 {
    let n = samples.len() as f64;
    if n == 0.0 {
        return 0.0;
    }
    let w = 2.0 * std::f64::consts::PI * freq / rate as f64;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in samples {
        let s0 = x as f64 + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).sqrt() / n
}

fn env_mp4() -> Option<String> {
    std::env::var("CURSEEN_AUDIO_PROBE_MP4")
        .ok()
        .filter(|s| !s.is_empty())
}

#[test]
#[ignore]
fn recorded_audio_carries_real_sound() {
    let Some(path) = env_mp4() else {
        println!("跳过：未设置 CURSEEN_AUDIO_PROBE_MP4");
        return;
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE).expect("MFStartup 失败");

        let static_path: OnceLock<String> = OnceLock::new();
        let path = static_path.get_or_init(|| path.clone()).clone();
        match read_all_pcm(&path) {
            Ok((pcm, rate, ch)) => {
                let secs = pcm.len() as f64 / (rate as f64 * ch as f64);
                let rms = if pcm.is_empty() {
                    0.0
                } else {
                    (pcm.iter().map(|&x| (x as f64) * (x as f64)).sum::<f64>() / pcm.len() as f64)
                        .sqrt()
                };
                let e_tone = goertzel(&pcm, rate, TONE_HZ);
                let e_near = goertzel(&pcm, rate, TONE_HZ * 0.8);
                let e_far = goertzel(&pcm, rate, TONE_HZ * 2.5);
                println!(
                    "解码：{} 样本, {} Hz × {} ch = {:.2}s",
                    pcm.len(),
                    rate,
                    ch,
                    secs
                );
                println!(
                    "RMS={rms:.1}  能量@{}Hz={e_tone:.1}  @{}Hz={e_near:.1}  @{}Hz={e_far:.1}",
                    TONE_HZ as u32,
                    (TONE_HZ * 0.8) as u32,
                    (TONE_HZ * 2.5) as u32
                );
                let loud = rms > RMS_FLOOR;
                let tonal = e_tone > e_near * 3.0 && e_tone > e_far * 3.0;
                println!(
                    "判定：{}  响度{}  目标频率{}",
                    if loud && tonal {
                        "PASS（录到了真实声音，且就是那支测试音）"
                    } else if loud {
                        "PASS(弱)：有声音，但不是测试音的频率"
                    } else {
                        "FAIL：整段是静音"
                    },
                    if loud { "够" } else { "不够" },
                    if tonal { "突出" } else { "不突出" }
                );
                assert!(loud, "RMS {rms:.1} 低于阈值 {RMS_FLOOR}，产物音轨是静音");
                assert!(tonal, "{} Hz 能量未显著高于邻频，录到的不是测试音", TONE_HZ);
            }
            Err(e) => {
                println!("解码失败（无法判定，不当作通过）：{e}");
                panic!("音频验收无法完成: {e}");
            }
        }
        MFShutdown().expect("MFShutdown 失败");
    }
}
