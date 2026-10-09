//! # 音频端到端取证探针（`audio_e2e`）—— 2026-10-06
//!
//! ## 它回答一个问题
//!
//! 用户报告录屏产物"**声音抖动不正常**"。本探针把一个 mp4 的**音轨解成原始
//! PCM** 落盘，供离线分析（配合 `temp/test_tone.wav` 参考音：录制时用系统扬声器
//! 播放"连续 440Hz + 每 5 秒一个 0.3s 的 1760Hz 标记音"，解出 PCM 后量标记音的
//! 实际间隔——录制链路引入的任何抖动/漂移/拼接错位都会直接表现为间隔误差）。
//!
//! ## 跑法
//!
//! ```text
//! python build/probe.py run audio_e2e -- --nocapture
//! ```
//! 输入/输出路径写死在 `IN`/`OUT`（探针一次只对一份取证产物负责，不值得做参数化）。
//!
//! ## 依赖
//!
//! 只用 MF SourceReader 解码（AAC → PCM 由系统变换自动完成），不碰生产代码。
use std::path::Path;

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::WAVEFORMATEX;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

/// 待解码的取证产物（可用 `E2E_IN` 环境变量覆盖，指向任意 mp4）。
const IN: &str = "../temp/tone_ctrl.mp4";
/// 原始 PCM 落盘位置（16bit 交错，格式见探针打印；`E2E_IN` 时输出同目录 `.wav`）。
const OUT: &str = "../temp/rec_audio_ctrl.pcm";

#[test]
#[ignore]
fn audio_e2e() {
    unsafe {
        let _mf = MfStartupGuard::new();

        // `E2E_IN` 覆盖：探针复用同一套解码器分析任意产物（输出旁落成同名 .wav）。
        let owned = std::env::var("E2E_IN").ok().filter(|v| !v.is_empty());
        let (inp, outp): (&Path, String) = match &owned {
            Some(v) => (
                Path::new(v.as_str()),
                v.trim_end_matches(".mp4").to_string() + ".wav",
            ),
            None => (Path::new(IN), OUT.to_string()),
        };
        assert!(inp.exists(), "输入不存在: {}", inp.display());
        let mut wide: Vec<u16> = inp.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);

        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)
            .expect("建 SourceReader 失败");

        // 只选音频流（流 1）。
        // ★ 流索引不等于 sink 写入顺序：reader 按文件轨序给号（实测本探针产物
        //   音频不在 1）。按 major type == Audio 自动定位。
        let mut aud_stream = None;
        for i in 0..4u32 {
            let major = reader
                .GetNativeMediaType(i, 0)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
                .unwrap_or_default();
            if major == MFMediaType_Audio {
                aud_stream = Some(i);
                break;
            }
        }
        let Some(aud) = aud_stream else {
            panic!("文件里没有音轨（取证产物必须含音轨）");
        };
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("全不选失败");
        reader
            .SetStreamSelection(aud, true)
            .expect("选音频流失败");

        // 显式构造**完整** PCM 类型（从原生类型取采样率/声道；空白/半拷贝类型
        // 都会被拒 MF_E_INVALIDMEDIATYPE 0xC00D36B4）。
        let native = reader.GetNativeMediaType(aud, 0).expect("取原生类型失败");
        let n_sub = native.GetGUID(&MF_MT_SUBTYPE).unwrap_or_default();
        let n_rate = native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48000);
        let n_ch = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2);
        println!("原生音频 subtype={:?} {n_rate}Hz×{n_ch}ch", n_sub);
        let ty = MFCreateMediaType().expect("建媒体类型失败");
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .expect("MAJOR 失败");
        ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .expect("SUBTYPE 失败");
        ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, n_rate)
            .expect("RATE 失败");
        ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, n_ch)
            .expect("CH 失败");
        ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .expect("BITS 失败");
        ty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, n_ch * 2)
            .expect("ALIGN 失败");
        ty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, n_rate * n_ch * 2)
            .expect("BPS 失败");
        ty.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)
            .expect("UNCOMP 失败");
        reader
            .SetCurrentMediaType(aud, None, &ty)
            .expect("SetCurrentMediaType(PCM) 失败");

        // 打印实际解出的格式（借 WAVEFORMATEX 拿权威值；GetUINT32 曾返回 0）。
        let got = reader.GetCurrentMediaType(aud).expect("取实际类型失败");
        let mut wfx: *mut WAVEFORMATEX = std::ptr::null_mut();
        let mut wfx_size = 0u32;
        MFCreateWaveFormatExFromMFMediaType(&got, &mut wfx, Some(&mut wfx_size), 0)
            .expect("取 WAVEFORMATEX 失败");
        let w = &*wfx;
        // ★ WAVEFORMATEX 是 packed 结构：字段先拷到局部变量再打印（不能取引用）。
        let (tag, ch, rate, bits, align, bps) = (
            w.wFormatTag, w.nChannels, w.nSamplesPerSec, w.wBitsPerSample, w.nBlockAlign, w.nAvgBytesPerSec,
        );
        println!(
            "解码格式: tag=0x{tag:04X} ch={ch} rate={rate} bits={bits} align={align} bytes/s={bps}"
        );
        CoTaskMemFree(Some(wfx as *const _));

        let mut out = Vec::new();
        let mut n = 0usize;
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(aud, 0, None, Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(s) = sample else { break };
            if n < 5 {
                let len = s
                    .ConvertToContiguousBuffer()
                    .and_then(|b| b.GetCurrentLength())
                    .unwrap_or(0);
                println!("块{n}: ts={:.3}s len={len}B", s.GetSampleTime().unwrap_or(0) as f64 / 1e7);
            }
            let buf = s.ConvertToContiguousBuffer().expect("Convert 失败");
            let mut data = std::ptr::null_mut();
            // ★ Lock 出参顺序：第二 = 缓冲最大长度，第三 = **当前有效长度**（漏传第三
            //   会把整块 49KB 的缓冲尾垃圾也拷进来，时长凭空 ×12）。
            let mut max_len = 0u32;
            let mut cur = 0u32;
            buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur))
                .expect("Lock 失败");
            out.extend_from_slice(std::slice::from_raw_parts(data, cur as usize));
            buf.Unlock().ok();
            n += 1;
        }
        std::fs::write(&outp, &out).expect("写 PCM 失败");
        let secs = out.len() as f64 / (w.nAvgBytesPerSec.max(1) as f64);
        println!("样本块 {n}，PCM {} 字节 ≈ {secs:.3}s", out.len());
        // ★ 下限可配：写死的 30s 是"48 秒取证录制"那次留下的期望，后来录短段
        //   （`REC_TONE_SECS=18`）时它会把"录得短"误判成"产物不完整"。
        //   默认仍是 30 —— 判据没有放宽，只是不再跟录制时长绑死。
        let min = std::env::var("AUDIO_E2E_MIN_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30.0);
        assert!(
            secs > min,
            "解码出的音频太短（{secs:.1}s < 下限 {min:.1}s），取证产物不完整"
        );
    }
}

/// `MFStartup` 的 RAII 包装（探针生命周期内保持初始化）。
struct MfStartupGuard;

impl MfStartupGuard {
    unsafe fn new() -> Self {
        MFStartup(MF_VERSION, 0).expect("MFStartup 失败");
        Self
    }
}

impl Drop for MfStartupGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
