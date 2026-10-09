//! # 录制配置有效性审计探针（`settings_audit`）—— 2026-10-09
//!
//! 背景：用户问「视频模块里的其它设置都有效吗？例如硬件加速」。本探针对关键设置
//! 做**端到端实测**（真录 → 解产物 → 量特征），而不是只看日志或代码路径：
//!
//! 1. `hardware`：Auto vs Off 各录 5s，量**进程 CPU 增量**（GetProcessTimes）。
//!    硬件编码（NVENC）应显著省 CPU；产物解码帧数应基本一致。
//! 2. `fps`：15fps 录 4s，解码帧数应明显少于 30fps 的 5s（≈60 vs ≈150）。
//! 3. `resolution`（max_long_edge）：1280 档产物长边应缩到 1280。
//!
//! 顺带打印 `hardware_encoder_name()`（MFTEnumEx 实际枚举到的硬件编码器名），
//! 作为「本机到底有没有硬件编码」的地面真值。
//!
//! 跑法：`python build/probe.py run settings_audit`（真录 4 段，合计 ~19s）。
//! 分组：slow（见 build/probe.py 的分组规则）。

use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{
    hardware_encoder_name, Hardware, RecKind, RecOptions, RecordArea, Recorder,
};

const REC_SECS: u64 = 5;

/// 量一次录制的进程 CPU 增量（user+kernel，全线程）。
fn process_cpu_delta<F: FnOnce()>(f: F) -> f64 {
    unsafe {
        let pre = filetime_now();
        f();
        let post = filetime_now();
        // FILETIME 是 100ns 单位。
        ((post.0 - pre.0) + (post.1 - pre.1)) as f64 / 1e7
    }
}

/// (user, kernel) 两个 FILETIME 的当前值。
unsafe fn filetime_now() -> (i64, i64) {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut creation = 0i64;
    let mut exit = 0i64;
    let mut user = 0i64;
    let mut kernel = 0i64;
    GetProcessTimes(
        GetCurrentProcess(),
        &mut creation as *mut i64 as *mut _,
        &mut exit as *mut i64 as *mut _,
        &mut kernel as *mut i64 as *mut _,
        &mut user as *mut i64 as *mut _,
    )
    .expect("GetProcessTimes 失败");
    (user, kernel)
}

/// 录一段（硬件策略 / fps / max_long_edge 可调），返回产物路径与录前录后的 CPU 差。
fn record_one(
    tag: &str,
    hardware: Hardware,
    fps: u32,
    max_long_edge: u32,
) -> (std::path::PathBuf, f64) {
    let path = std::path::PathBuf::from(format!("../temp/audit_{tag}.mp4"));
    let _ = std::fs::remove_file(&path);
    let rec = Recorder::start(RecOptions {
        kind: RecKind::Mp4,
        path: path.clone(),
        area: RecordArea::Primary,
        crop: None,
        fps,
        bitrate_kbps: None, // 自动码率，与生产一致
        max_long_edge,
        hardware,
        show_cursor: false,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    let t0 = Instant::now();
    let cpu = process_cpu_delta(|| {
        while t0.elapsed() < Duration::from_secs(REC_SECS) {
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let outcome = rec.finish();
    assert!(outcome.error.is_none(), "[{tag}] 录制报错: {:?}", outcome.error);
    (path, cpu)
}

/// 解产物视频轨：返回 (帧数, 宽, 高)。
fn decode_frames(path: &std::path::Path) -> (usize, u32, u32) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::MediaFoundation::{
        MFCreateMediaType, MFCreateSourceReaderFromURL, MF_MT_DEFAULT_STRIDE,
        MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MFMediaType_Video,
        MFVideoFormat_NV12, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
        IMFSample, IMFSourceReader, MFSTARTUP_LITE, MFStartup, MFShutdown, MF_VERSION,
    };

    unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_LITE).expect("MFStartup 失败");
        let _mf_guard = ();
        let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let reader: IMFSourceReader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)
            .expect("建 SourceReader 失败");
        let mut vid = None;
        for i in 0..4u32 {
            let major = reader
                .GetNativeMediaType(i, 0)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
                .unwrap_or_default();
            if major == MFMediaType_Video {
                vid = Some(i);
                break;
            }
        }
        let vid = vid.expect("没有视频轨");
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .unwrap();
        reader.SetStreamSelection(vid, true).unwrap();
        let vty = MFCreateMediaType().unwrap();
        vty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
        vty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
        reader.SetCurrentMediaType(vid, None, &vty).unwrap();
        let got = reader.GetCurrentMediaType(vid).unwrap();
        let packed = got.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (w, h) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(w) as usize;

        let mut frames = 0usize;
        loop {
            let mut flags = 0u32;
            let mut actual = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(vid, 0, Some(&mut actual), Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || sample.is_none() {
                break;
            }
            // 遍历即可，不需要像素：Lock/Unlock 走一遍保证样本有效。
            let buf = sample.unwrap().ConvertToContiguousBuffer().unwrap();
            let mut data = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur = 0u32;
            buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur)).unwrap();
            let _ = std::slice::from_raw_parts(data as *const u8, cur as usize);
            let _ = buf.Unlock();
            let _ = stride;
            frames += 1;
        }
        let _ = MFShutdown();
        (frames, w, h)
    }
}

#[test]
#[ignore = "真录 4 段（合计 ~19s）+ 解码；跑法见文件头"]
fn settings_audit() {
    println!(
        "本机硬件 H264 编码器：{:?}",
        hardware_encoder_name().unwrap_or_else(|| "(无)".into())
    );

    // ── 1. hardware：Auto vs Off，比 CPU 增量 + 帧数 ──
    let (p_auto, cpu_auto) = record_one("hw_auto", Hardware::Auto, 30, 0);
    let (f_auto, w_auto, h_auto) = decode_frames(&p_auto);
    println!(
        "Auto : {} 帧 {}×{}，进程 CPU 增量 {:.2}s / {}s",
        f_auto, w_auto, h_auto, cpu_auto, REC_SECS
    );
    let (p_off, cpu_off) = record_one("hw_off", Hardware::Off, 30, 0);
    let (f_off, w_off, h_off) = decode_frames(&p_off);
    println!(
        "Off  : {} 帧 {}×{}，进程 CPU 增量 {:.2}s / {}s",
        f_off, w_off, h_off, cpu_off, REC_SECS
    );
    // 两种策略都应正常产出（帧数同量级，≥ 5s×30fps 的一半）。
    for (tag, n) in [("Auto", f_auto), ("Off", f_off)] {
        assert!(
            n >= REC_SECS as usize * 15,
            "{tag} 帧数 {n} < {}：连一半帧率都没到，管线有问题",
            REC_SECS * 15
        );
    }
    assert_eq!(
        (w_auto, h_auto),
        (w_off, h_off),
        "两种硬件策略产物尺寸不同"
    );
    // 硬件编码存在的机器上，Auto 的 CPU 增量应明显低于 Off（软编吃满一核以上）。
    assert!(
        cpu_auto < cpu_off,
        "Auto CPU {cpu_auto:.2}s ≥ Off {cpu_off:.2}s：硬件编码没省 CPU（本机无硬件编码器？或都在软件编码）"
    );

    // ── 2. fps：15fps 录 4s ──
    let (p15, _) = record_one("fps15", Hardware::Auto, 15, 0);
    let (f15, _, _) = decode_frames(&p15);
    println!("fps15: {} 帧（5s × 15fps = 75，容忍 35~85）", f15);
    assert!(
        (35..=85).contains(&f15),
        "15fps 产物 {f15} 帧，明显不是 15fps（30fps 会是 ~120+）"
    );

    // ── 3. resolution：max_long_edge=1280 ──
    let (p1280, _) = record_one("res1280", Hardware::Auto, 30, 1280);
    let (_, w1280, h1280) = decode_frames(&p1280);
    println!("res1280: {}×{}", w1280, h1280);
    assert!(
        w1280.max(h1280) == 1280,
        "max_long_edge=1280 产物却是 {}×{}：分辨率档无效",
        w1280,
        h1280
    );

    println!("\n✅ 硬件加速 / 帧率 / 分辨率档位全部实测有效");
}
