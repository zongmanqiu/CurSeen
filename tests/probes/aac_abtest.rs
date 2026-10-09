//! # AAC 码率离线 A/B 探针（`aac_abtest`）—— 2026-10-09
//!
//! ## 它回答一个问题
//!
//! 用户关掉降噪后仍觉得录出来的声音"接近原声，但怪怪的"。本机两路混音格式
//! 都是 48k×2ch（`mix_formats` 实测），降噪 Off 是纯透传 ⇒ 链路里唯一还会
//! "动声音"的环节就是 **MF AAC 编码（现行 128kbps）**。本探针拿同一段原始
//! 麦克风 PCM（`temp/buzz_raw.pcm`），分别过 **MF AAC 128 / 192 / 320 kbps**
//! 编码再解码回 PCM，量化各码率与原声的偏差——回答"怪怪的"是不是 128k
//! 编码损失、加大码率值不值。
//!
//! ## 跑法
//!
//! ```text
//! python build/probe.py run aac_abtest -- --nocapture
//! ```
//!
//! 输出：每个码率一行判据（RMS 偏差 / 高频能量保留率），解码 PCM 落盘
//! `temp/aac_<kbps>.pcm`（可与原声直接对比试听）。
//!
//! ## 判据（自红设计）
//!
//! - 各码率产物时长必须接近原声（±50ms），否则链路本身坏了。
//! - RMS 偏差必须随码率单调不增（320k ≤ 192k ≤ 128k），否则实验无效。
//! - 320k 的偏差必须 < 128k 的偏差（编码损失主导的证据）。
//!
//! probe-group: fast
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

const SRC_PCM: &str = "../temp/buzz_raw.pcm";
const RATE: u32 = 48000;
const CH: u32 = 2;
/// 每次写入的帧数（100ms）。
const CHUNK_FRAMES: usize = 4800;
const BITRATES: [u32; 3] = [128, 192, 320];

#[test]
#[ignore]
fn aac_abtest() {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().expect("CoInitialize 失败");
    let _mf = MfGuard::new();

    let orig = std::fs::read(Path::new(SRC_PCM)).expect("读原始 PCM 失败");
    let orig: Vec<i16> = orig
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let orig_secs = orig.len() as f64 / (RATE as usize * CH as usize) as f64;
    println!(
        "原始 PCM: {} 个 i16 ≈ {orig_secs:.2}s（{RATE}Hz × {CH}ch）",
        orig.len()
    );
    assert!(orig_secs > 5.0, "原始 PCM 太短，实验无效");

    let mut results = Vec::new();
    for kbps in BITRATES {
        let mp4 = format!("../temp/aac_{kbps}.mp4");
        encode_aac(Path::new(SRC_PCM), Path::new(&mp4), kbps);
        let dec = decode_pcm(Path::new(&mp4));
        let secs = dec.len() as f64 / (RATE as usize * CH as usize) as f64;
        println!("  {kbps}k: 解出 {secs:.3}s");
        assert!(
            (secs - orig_secs).abs() < 0.05,
            "{kbps}k 产物时长 {secs:.3}s 与原声 {orig_secs:.3}s 差太多，链路坏了"
        );
        std::fs::write(format!("../temp/aac_{kbps}.pcm"), slice_to_bytes(&dec))
            .expect("写解码 PCM 失败");
        results.push((kbps, rms_err(&orig, &dec), hf_energy_ratio(&dec)));
    }

    // ── 判据 ──
    println!("\n码率    RMS偏差(i16)  高频(>7kHz)能量比  解码PCM");
    let mut prev_err = f64::INFINITY;
    for (kbps, err, hf) in &results {
        println!(
            "{kbps:>3}k   {err:>10.2}    {hf:>14.4}    ../temp/aac_{kbps}.pcm"
        );
        assert!(*err <= prev_err * 1.05 + 1.0, "{kbps}k 偏差反而大于低码率，实验无效");
        prev_err = *err;
    }
    let lo = results[0].1;
    let hi = results[2].1;
    println!("\n结论：320k 偏差 {hi:.2} vs 128k 偏差 {lo:.2}（{:.0}%）",
        if lo > 0.0 { hi / lo * 100.0 } else { 0.0 });
    assert!(hi < lo, "加大码率没有降低偏差——'怪怪的'另有原因，编码不是主因");
    // 原声高频能量参照
    println!("原声高频能量比: {:.4}", hf_energy_ratio(&orig));
    }
}

/// MF AAC 编码：audio-only MP4。
unsafe fn encode_aac(src: &Path, dst: &Path, kbps: u32) {
    let dst_w: Vec<u16> = dst.as_os_str().to_string_lossy().encode_utf16().collect();
    let sink: IMFSinkWriter = MFCreateSinkWriterFromURL(PCWSTR(widen(&dst_w).as_ptr()), None, None)
        .expect("建 SinkWriter 失败");

    let out_ty = MFCreateMediaType().expect("建输出类型失败");
    out_ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
    out_ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).unwrap();
    out_ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, kbps * 1000 / 8).unwrap();
    out_ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE).unwrap();
    out_ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CH).unwrap();
    out_ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
    let stream = sink.AddStream(&out_ty).expect("音频 AddStream 失败");

    let in_ty = MFCreateMediaType().expect("建输入类型失败");
    in_ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
    in_ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
    in_ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE).unwrap();
    in_ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CH).unwrap();
    in_ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
    in_ty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, CH * 2).unwrap();
    in_ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * CH * 2).unwrap();
    sink.SetInputMediaType(stream, &in_ty, None).expect("音频 SetInputMediaType 失败");
    sink.BeginWriting().expect("BeginWriting 失败");

    let pcm = std::fs::read(src).expect("读 PCM 失败");
    let samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let frames = samples.len() / CH as usize;
    let mut pos = 0usize;
    let mut ts: i64 = 0;
    while pos < frames {
        let n = CHUNK_FRAMES.min(frames - pos);
        let byte_count = n * CH as usize * 2;
        let buf: IMFMediaBuffer = MFCreateMemoryBuffer(byte_count as u32).expect("建缓冲失败");
        {
            let mut data = std::ptr::null_mut();
            buf.Lock(&mut data, None, None).expect("Lock 失败");
            std::ptr::copy_nonoverlapping(
                samples[pos * CH as usize..(pos + n) * CH as usize].as_ptr(),
                data as *mut i16,
                n * CH as usize,
            );
            buf.Unlock().ok();
            buf.SetCurrentLength(byte_count as u32).ok();
        }
        let s: IMFSample = MFCreateSample().expect("建样本失败");
        s.AddBuffer(&buf).unwrap();
        s.SetSampleTime(ts).unwrap();
        let dur = (n as i64 * 10_000_000) / RATE as i64;
        s.SetSampleDuration(dur).unwrap();
        sink.WriteSample(stream, &s).expect("WriteSample 失败");
        ts += dur;
        pos += n;
    }
    sink.Finalize().expect("Finalize 失败");
}

/// MF SourceReader 解码回 PCM（16bit 交错）。
unsafe fn decode_pcm(path: &Path) -> Vec<i16> {
    let w: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
    let reader = MFCreateSourceReaderFromURL(PCWSTR(widen(&w).as_ptr()), None)
        .expect("建 SourceReader 失败");
    reader
        .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
        .unwrap();
    reader.SetStreamSelection(0, true).unwrap();

    let native = reader.GetNativeMediaType(0, 0).unwrap();
    let n_rate = native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(RATE);
    let n_ch = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(CH);
    let ty = MFCreateMediaType().unwrap();
    ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
    ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
    ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, n_rate).unwrap();
    ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, n_ch).unwrap();
    ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
    ty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, n_ch * 2).unwrap();
    ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, n_rate * n_ch * 2).unwrap();
    reader.SetCurrentMediaType(0, None, &ty).expect("SetCurrentMediaType 失败");

    // 编码是 48k×2ch 进的，解码也应同格式；如实测不同则失败（判据依赖对齐）。
    let got = reader.GetCurrentMediaType(0).unwrap();
    let mut wfx: *mut windows::Win32::Media::Audio::WAVEFORMATEX = std::ptr::null_mut();
    let mut sz = 0u32;
    MFCreateWaveFormatExFromMFMediaType(&got, &mut wfx, Some(&mut sz), 0).unwrap();
    let (ch, rate) = ((*wfx).nChannels, (*wfx).nSamplesPerSec);
    CoTaskMemFree(Some(wfx as *const _));
    assert_eq!((ch, rate), (CH as u16, RATE), "解码格式与原声不一致，无法对齐比对");

    let mut out: Vec<i16> = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut sample: Option<IMFSample> = None;
        reader
            .ReadSample(0, 0, None, Some(&mut flags), None, Some(&mut sample))
            .expect("ReadSample 失败");
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            break;
        }
        let Some(s) = sample else { break };
        let buf = s.ConvertToContiguousBuffer().expect("Convert 失败");
        let mut data = std::ptr::null_mut();
        let mut max_len = 0u32;
        let mut cur = 0u32;
        buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur)).expect("Lock 失败");
        let bytes = std::slice::from_raw_parts(data, cur as usize);
        out.extend(bytes.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])));
        buf.Unlock().ok();
    }
    out
}

/// 逐样本 RMS 偏差（长度取两者较短）。
fn rms_err(a: &[i16], b: &[i16]) -> f64 {
    let n = a.len().min(b.len());
    let sum: f64 = a[..n]
        .iter()
        .zip(&b[..n])
        .map(|(x, y)| {
            let d = *x as f64 - *y as f64;
            d * d
        })
        .sum();
    (sum / n.max(1) as f64).sqrt()
}

/// >7kHz 能量占比：用双一次高通（粗但稳定，可复现）。
fn hf_energy_ratio(s: &[i16]) -> f64 {
    // 简单一阶高通 y[n] = x[n] - x[n-1] + 0.995*y[n-1]（~48k 下转折 ~7kHz 量级）。
    let mut prev_x = 0f64;
    let mut prev_y = 0f64;
    let (mut hf, mut all) = (0f64, 0f64);
    for &v in s {
        let x = v as f64;
        let y = x - prev_x + 0.995 * prev_y;
        prev_x = x;
        prev_y = y;
        hf += y * y;
        all += x * x;
    }
    if all <= 0.0 {
        0.0
    } else {
        hf / all
    }
}

fn slice_to_bytes(s: &[i16]) -> Vec<u8> {
    s.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn widen(v: &[u16]) -> Vec<u16> {
    let mut w = v.to_vec();
    w.push(0);
    w
}

struct MfGuard;
impl MfGuard {
    unsafe fn new() -> Self {
        MFStartup(MF_VERSION, 0).expect("MFStartup 失败");
        Self
    }
}
impl Drop for MfGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
