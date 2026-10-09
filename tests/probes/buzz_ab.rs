//! # 降噪"电流声" A/B 取证探针（`buzz_ab`）—— 2026-10-09
//!
//! 用户报：denoise=weak 时产物有轻微"电流声"。频谱初查（temp/buzz_analyze.py）
//! 发现底噪里有大量孤立音调尖刺、非工频谐波——指向 RNNoise 的**音乐噪声**伪影。
//! 本探针做决定性 A/B：同一环境连录两段（仅差 denoise 档位），解码落盘交
//! Python 比对底噪谱。若尖刺只在降噪档出现，伪影实锤。
//!
//! 跑法：`python build/probe.py run buzz_ab`（真录两段 6s，合计 ~14s）。

use std::time::Duration;

use curseen_ui::platform::recorder::{
    RecKind, RecOptions, RecordArea, Recorder,
};

fn record_one(tag: &str, mic_denoise: &str) {
    let path = std::path::PathBuf::from(format!("../temp/buzz_{tag}.mp4"));
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
        capture_mic: true,
        capture_speaker: true,
        mic_denoise: mic_denoise.into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    std::thread::sleep(Duration::from_secs(6));
    let outcome = rec.finish();
    assert!(outcome.error.is_none(), "[{tag}] 录制报错: {:?}", outcome.error);
}

use curseen_ui::platform::recorder::Hardware;

/// 解产物音轨为原始 PCM（与 audio_e2e 同法），落盘裸 PCM 交 Python。
fn decode_audio(path: &std::path::Path, out: &std::path::Path) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::MediaFoundation::*;
    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_LITE).expect("MFStartup 失败");
        let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)
            .expect("建 SourceReader 失败");
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
        let aud = aud.expect("没有音轨");
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .unwrap();
        reader.SetStreamSelection(aud, true).unwrap();
        let aty = MFCreateMediaType().unwrap();
        aty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
        aty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
        reader.SetCurrentMediaType(aud, None, &aty).unwrap();
        let mut out_bytes: Vec<u8> = Vec::new();
        loop {
            let mut flags = 0u32;
            let mut actual = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(aud, 0, Some(&mut actual), Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || sample.is_none() {
                break;
            }
            let buf = sample.unwrap().ConvertToContiguousBuffer().unwrap();
            let mut data = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur = 0u32;
            buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur)).unwrap();
            out_bytes
                .extend_from_slice(std::slice::from_raw_parts(data, cur as usize));
            buf.Unlock().ok();
        }
        std::fs::write(out, &out_bytes).expect("写 PCM 失败");
        println!("[out] {} = {} 字节", out.display(), out_bytes.len());
        let _ = MFShutdown();
    }
}

#[test]
#[ignore = "真录两段（合计 ~14s）+ 解码；跑法见文件头"]
fn buzz_ab() {
    record_one("off", "off");
    record_one("weak", "weak");
    decode_audio(
        &std::path::PathBuf::from("../temp/buzz_off.mp4"),
        &std::path::PathBuf::from("../temp/buzz_off.pcm"),
    );
    decode_audio(
        &std::path::PathBuf::from("../temp/buzz_weak.mp4"),
        &std::path::PathBuf::from("../temp/buzz_weak.pcm"),
    );
    println!("产物: ../temp/buzz_off.pcm / buzz_weak.pcm（交 Python 比对底噪谱）");
}
