//! 参考音录制探针（2026-10-06，查"电流声"用）。
//!
//! 用生产录制链路真录一段（单文件、无拼接），时长/产物路径由环境变量给：
//!   `REC_TONE_SECS`   录制秒数（默认 48）
//!   `REC_TONE_PATH`   产物路径（默认 `../temp/tone_rec.mp4`，相对 main/）
//!
//! 配合播放：录制开始后立刻往默认输出设备播 `temp/test_tone.wav`
//! （440Hz 连续 + 每 5s 一个 1760Hz 标记 0.3s）。产物交给 `audio_e2e` 解码
//! 后做标记间隔 + 不连续点（电流声）定量分析。
//!
//! 跑法：`python build/probe.py run rec_tone`
#![cfg(windows)]

use curseen_ui::platform::recorder::{Hardware, Recorder, RecKind, RecOptions, RecordArea};
use std::time::Duration;

#[test]
#[ignore]
fn rec_tone_record() {
    let secs: u64 = std::env::var("REC_TONE_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(48);
    let path = std::env::var("REC_TONE_PATH")
        .unwrap_or_else(|_| "../temp/tone_rec.mp4".into());
    let path = std::path::PathBuf::from(&path);
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
        capture_mic: false,
        capture_speaker: true,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    println!("REC_STARTED {}", chrono_secs());
    std::thread::sleep(Duration::from_secs(secs));
    let outcome = rec.finish();
    println!(
        "REC_FINISHED bytes={:?} error={:?} path={}",
        outcome.bytes,
        outcome.error,
        path.display()
    );
    assert!(outcome.error.is_none(), "录制不应报错：{:?}", outcome.error);
    let data = std::fs::read(&path).expect("产物读不出来");
    assert!(
        data.windows(4).any(|w| w == b"moov"),
        "产物没有 moov 盒"
    );
}

/// 墙钟时间戳（报告口径用）。
fn chrono_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}
