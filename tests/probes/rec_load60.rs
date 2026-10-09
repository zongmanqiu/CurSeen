//! 高负载短测：**60fps** 录一段（默认 300 秒），看编码器反压下的丢帧与收尾。
//!
//! # 为什么单开一条
//!
//! 楔死的历史现场是**编码器反压**（采集快、编码慢 ⇒ 队列堵 ⇒ 线程楔在
//! `WriteSample`）。2h 长测用的是 1080p/30fps，全程只丢 4 帧 —— 编码器一直游刃有余，
//! 等于**没压到过它**。把帧率翻倍是最省事的反压来源：同样的画面，编码吞吐要翻倍。
//!
//! 判据锁三件事：不楔住、不丢帧（<2%）、产物完整。
//!
//! 跑：`python build/probe.py run rec_load60 --secs 300`

#![cfg(windows)]

use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{self, RecOptions};

const FINISH_LIMIT: Duration = Duration::from_secs(30);
/// 60fps 下允许的实际帧率下限。取 90%：系统调度抖动不算丢帧，编码器真跟不上才算。
const MIN_FPS: f64 = 54.0;
const MAX_DROP_RATE: f64 = 0.02;

#[test]
#[ignore]
fn sixty_fps_does_not_wedge_or_drop_frames() {
    let _ = env_logger::builder().is_test(false).try_init();

    let secs: u64 = std::env::var("CURSEEN_LONG_REC_S")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(300);

    let out = std::env::temp_dir().join("curseen_load60.mp4");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        // ★ 被测的就是它：60fps（录屏上限，见 `opts.fps.clamp(5, 60)`）。
        fps: 60,
        // 自动码率：按输出尺寸 × 60fps 算，比 30fps 高一档 —— 编码器压力同步翻倍。
        bitrate_kbps: None,
        // 原始分辨率：缩放在 GPU 上做，缩了就变不成"高负载"了。
        max_long_edge: 0,
        hardware: recorder::Hardware::Auto,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("应能起录制");
    println!("输出 {}x{}", rec.out_size().0, rec.out_size().1);

    let t0 = wait_first_frame(&rec);
    // 分段看曲线：平均 fps 会掩盖周期性塌陷（`踩坑记录` §12），每 30 秒打一个点。
    let mut last = 0u64;
    for i in 1..=secs / 30 {
        std::thread::sleep(Duration::from_secs(30));
        let now = rec.frames();
        let seg_fps = (now - last) as f64 / 30.0;
        last = now;
        println!(
            "  {}s：累计 {now} 帧 / 本段 {seg_fps:.1}fps / 丢 {} / 停摆 {}ms",
            i * 30,
            rec.dropped(),
            rec.stall_ms()
        );
    }
    let tail = secs % 30;
    if tail > 0 {
        std::thread::sleep(Duration::from_secs(tail));
    }

    let dropped = rec.dropped();
    let frames = rec.frames();
    let real = t0.elapsed().as_secs_f64();
    let t_finish = Instant::now();
    let done = rec.finish();
    let finish_secs = t_finish.elapsed().as_secs_f64();

    let fps = frames as f64 / real;
    let drop_rate = if frames + dropped > 0 {
        dropped as f64 / (frames + dropped) as f64
    } else {
        0.0
    };
    println!(
        "\n60fps {secs}s：{frames} 帧 / 真实 {real:.2}s / 实际 {fps:.1}fps / \
         丢弃 {dropped}（{:.2}%）/ 收尾 {finish_secs:.2}s / {} 字节 / 错误 {:?}",
        drop_rate * 100.0,
        done.bytes.unwrap_or(0),
        done.error
    );

    // 判据 1：不楔住（高负载是楔死的历史诱因，这条是本探针存在的理由）。
    assert!(
        finish_secs < FINISH_LIMIT.as_secs_f64(),
        "收尾 {finish_secs:.2}s 超限 —— 60fps 下收尾被拖死了"
    );
    // 判据 2：不崩溃、不冒充。
    assert!(
        done.is_clean(),
        "60fps 完整录制不该带错误：{:?}",
        done.error
    );
    // 判据 3：帧率真到了（采集没被反压拖慢）。
    assert!(
        fps >= MIN_FPS,
        "实际 {fps:.1}fps < {MIN_FPS} —— 编码器反压把采集拖慢了"
    );
    // 判据 4：丢帧率（队列满导致的主动丢弃）。
    assert!(
        drop_rate <= MAX_DROP_RATE,
        "丢帧率 {:.2}% 超过 {}% —— 编码跟不上采集，正是楔死的前兆",
        drop_rate * 100.0,
        MAX_DROP_RATE * 100.0
    );
    // 判据 5：产物完整（盒索引定位 moov，禁裸搜）。
    let boxes = mp4_top_boxes(&out);
    assert!(boxes.contains(&"moov".to_string()), "产物缺 moov：{boxes:?}");
    let n = mp4_video_sample_count(&out).unwrap_or(0);
    println!("产物视频样本 {n}（采集 {frames} 帧）");
    assert!(
        n + 60 >= frames,
        "视频样本 {n} 远少于采集帧数 {frames} —— 尾部被截断了"
    );

    println!("\n✅ 60fps {secs}s：{fps:.1}fps / 丢帧 {:.2}% / 收尾 {finish_secs:.2}s / 产物完整",
        drop_rate * 100.0);
}

fn wait_first_frame(rec: &recorder::Recorder) -> Instant {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if rec.frames() > 0 {
            return Instant::now();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("10 秒内没录到第一帧：{:?}", rec.take_error());
}

fn mp4_top_box_index(path: &std::path::Path) -> Vec<(String, u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut out = Vec::new();
    let mut off = 0u64;
    let mut hdr = [0u8; 16];
    while off + 8 <= len {
        if f.seek(SeekFrom::Start(off)).is_err() {
            break;
        }
        let got = f.read(&mut hdr).unwrap_or(0);
        if got < 8 {
            break;
        }
        let size32 = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
        let ty = String::from_utf8_lossy(&hdr[4..8]).into_owned();
        let total = if size32 == 1 {
            if got < 16 || off + 16 > len {
                break;
            }
            u64::from_be_bytes(hdr[8..16].try_into().unwrap())
        } else if size32 == 0 {
            len - off
        } else {
            size32
        };
        if total == 0 {
            break;
        }
        out.push((ty, off, total));
        let end = off + total;
        if end > len {
            break;
        }
        off = end;
    }
    out
}

fn mp4_top_boxes(path: &std::path::Path) -> Vec<String> {
    mp4_top_box_index(path)
        .into_iter()
        .map(|(t, _, _)| t)
        .collect()
}

fn mp4_moov_bytes(path: &std::path::Path) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let (_, off, total) = mp4_top_box_index(path)
        .into_iter()
        .find(|(t, _, _)| t == "moov")?;
    if total == 0 || total > 256 * 1024 * 1024 {
        return None;
    }
    let mut f = std::fs::File::open(path).ok()?;
    f.seek(SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; total as usize];
    f.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn mp4_video_sample_count(path: &std::path::Path) -> Option<u64> {
    let d = mp4_moov_bytes(path)?;
    let m_end = d.len();
    let find = |needle: &[u8], from: usize| {
        d.get(from..)?
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    };
    let mut off = 4usize;
    while let Some(trak) = find(b"trak", off) {
        let trak_end = find(b"trak", trak + 4).unwrap_or(m_end);
        if find(b"vide", trak).is_some_and(|h| h < trak_end) {
            if let Some(s) = find(b"stsz", trak) {
                if s + 16 <= trak_end {
                    return Some(u32::from_be_bytes([d[s + 12], d[s + 13], d[s + 14], d[s + 15]]) as u64);
                }
            }
        }
        off = trak + 4;
    }
    None
}
