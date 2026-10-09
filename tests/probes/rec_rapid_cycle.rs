//! 快速连录探针：**收尾还没结束就立刻开下一段**。
//!
//! # 为什么单开一条
//!
//! 桌面复制是**进程级单例**：上一段的 `DuplicateOutput` 对象要等它真的收尾完才释放，
//! 在那之前新的一段根本建不起来（`DuplicateOutput` 直接失败）。历史上这个场景的表现
//! 不是报错，而是**界面一直挂着"在录"** —— 用户以为在录，其实什么都没写。
//!
//! 所以本探针钉的不是"能不能连录"，而是**如实告知**：
//!
//! - 起不来必须 `Err`（带得清原因），绝不能返回一个"看起来在录"的空壳；
//! - 起的来就必须真有帧、真有 moov；
//! - 单例最终一定要释放（最后隔 2 秒再录一段必须成功）—— 不能泄漏。
//!
//! 末尾再补一轮 **MP4 → GIF → MP4 跨类型切换**：GIF 与 MP4 共用同一条采集会话、
//! 只换封装器，而 `runtime` 层是"串行切换"（桌面复制只有一份），快捷键连按就会撞上。
//!
//! 跑：`python build/probe.py run rec_rapid_cycle`

#![cfg(windows)]

use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{self, RecOptions};

const ROUNDS: usize = 8;
const SEG: Duration = Duration::from_secs(3);
const FINISH_LIMIT: Duration = Duration::from_secs(30);

#[test]
#[ignore]
fn back_to_back_sessions_never_silently_fail() {
    let _ = env_logger::builder().is_test(false).try_init();

    let mut ok = 0usize;
    let mut refused: Vec<String> = Vec::new();

    for i in 0..ROUNDS {
        let out = std::env::temp_dir().join(format!("curseen_rapid_{i}.mp4"));
        // ★ 关键点：**不睡**。上一段 finish 一返回就立刻起下一段 —— 收尾线程多半还
        //   没把桌面复制对象放掉，这正是历史上"界面挂着在录"的现场。
        let started = recorder::Recorder::start(opts(&out, false));
        let rec = match started {
            Ok(r) => r,
            Err(e) => {
                // 起不来必须是**明确的 Err**：静默失败（返回空壳）才是真 bug。
                println!("  第 {i} 段：起不来（{e}）");
                refused.push(e.to_string());
                continue;
            }
        };
        wait_first_frame(&rec);
        std::thread::sleep(SEG);
        let frames = rec.frames();
        let t = Instant::now();
        let done = rec.finish();
        let secs = t.elapsed().as_secs_f64();
        assert!(
            secs < FINISH_LIMIT.as_secs_f64(),
            "第 {i} 段收尾 {secs:.2}s 超限"
        );
        assert!(done.is_clean(), "第 {i} 段不该带错误：{:?}", done.error);
        assert!(frames > 10, "第 {i} 段 3 秒只有 {frames} 帧");
        let boxes = mp4_top_boxes(&out);
        assert!(
            boxes.contains(&"moov".to_string()),
            "第 {i} 段产物缺 moov：{boxes:?}"
        );
        println!(
            "  第 {i} 段：{frames} 帧 / 收尾 {secs:.2}s / {} 字节",
            done.bytes.unwrap_or(0)
        );
        ok += 1;
    }

    println!("\n连录 {ROUNDS} 段：成功 {ok}，被拒 {}（原因见上）", refused.len());

    // 判据 1：绝大多数段必须真录出来 —— 若 singleton 从不释放，这里会大面积失败。
    assert!(
        ok >= ROUNDS - 2,
        "{ROUNDS} 段里只成功 {ok} 段 —— 桌面复制单例没在收尾时释放"
    );
    // 判据 2：被拒的必须**说得清原因**（"桌面复制"相关），不能是含糊的空错误。
    for e in &refused {
        assert!(
            !e.trim().is_empty(),
            "起新段失败却没给原因 —— 用户界面只能干瞪眼"
        );
        println!("  被拒原因：{e}");
    }
    // 判据 3：单例最终一定释放 —— 歇 2 秒后再录一段必须成功（泄漏检测）。
    std::thread::sleep(Duration::from_secs(2));
    let out = std::env::temp_dir().join("curseen_rapid_final.mp4");
    let rec = recorder::Recorder::start(opts(&out, false)).expect("歇 2 秒后应能再录 —— 单例泄漏了");
    wait_first_frame(&rec);
    std::thread::sleep(SEG);
    let frames = rec.frames();
    let done = rec.finish();
    assert!(done.is_clean(), "收尾段不该带错误：{:?}", done.error);
    assert!(
        mp4_top_boxes(&out).contains(&"moov".to_string()),
        "收尾段产物缺 moov"
    );

    println!(
        "\n✅ 连录：{ok}/{ROUNDS} 成功、被拒 {} 次且次次有原因、歇 2 秒后仍可录（{frames} 帧）",
        refused.len()
    );

    // ——— 跨类型切换：MP4 → GIF → MP4，全程不等（快捷键连按的现场）———
    println!("\n跨类型切换（不等收尾）：");
    for (i, kind) in [0, 1, 2].iter().enumerate() {
        let gif = *kind == 1;
        let out = std::env::temp_dir().join(format!(
            "curseen_rapid_mix{i}.{}",
            if gif { "gif" } else { "mp4" }
        ));
        let rec = match recorder::Recorder::start(opts(&out, gif)) {
            Ok(r) => r,
            Err(e) => panic!("第 {i} 段（{}）起不来：{e}", if gif { "GIF" } else { "MP4" }),
        };
        wait_first_frame(&rec);
        std::thread::sleep(SEG);
        let f = rec.frames();
        let t = Instant::now();
        let done = rec.finish();
        let secs = t.elapsed().as_secs_f64();
        assert!(secs < FINISH_LIMIT.as_secs_f64(), "混合第 {i} 段收尾 {secs:.2}s 超限");
        assert!(done.is_clean(), "混合第 {i} 段不该带错误：{:?}", done.error);
        if gif {
            let b = std::fs::read(&out).expect("GIF 应能读回");
            assert!(b.starts_with(b"GIF89a"), "混合第 {i} 段不是 GIF89a");
            assert!(b.last() == Some(&0x3B), "混合第 {i} 段 GIF 缺 trailer（收尾没写完）");
        } else {
            assert!(
                mp4_top_boxes(&out).contains(&"moov".to_string()),
                "混合第 {i} 段 MP4 缺 moov"
            );
        }
        println!(
            "  {}：{f} 帧 / 收尾 {secs:.2}s / {} 字节",
            if gif { "GIF" } else { "MP4" },
            done.bytes.unwrap_or(0)
        );
    }

    println!("\n✅ 跨类型切换：MP4 → GIF → MP4 全部干净收尾、无卡死");
}

fn opts(path: &std::path::Path, gif: bool) -> RecOptions {
    RecOptions {
        kind: if gif {
            recorder::RecKind::Gif
        } else {
            recorder::RecKind::Mp4
        },
        path: path.to_path_buf(),
        area: recorder::RecordArea::Primary,
        crop: None,
        // GIF 档用 15fps + 1280 长边（与 capture_smoke 的 GIF 用例一致），
        // 缩放通路一并走到。
        fps: if gif { 15 } else { 30 },
        max_long_edge: if gif { 1280 } else { 0 },
        hardware: if gif {
            recorder::Hardware::Off
        } else {
            recorder::Hardware::Auto
        },
        bitrate_kbps: None,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: false,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    }
}

fn wait_first_frame(rec: &recorder::Recorder) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if rec.frames() > 0 {
            return;
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
