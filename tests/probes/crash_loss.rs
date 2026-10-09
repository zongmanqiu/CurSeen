//! # 崩溃丢数据取证探针（`crash_loss`）—— 2026-10-05
//!
//! ## 它只回答一个问题
//!
//! 用户录了几个小时后"录屏失败、内容全没"。本探针把这件事**压缩到 90 秒复现**：
//! 起真录制 → 录 90 秒 → **硬杀进程**（`TerminateProcess`，等价于任务管理器结束、
//! 系统更新强制重启、显卡驱动 TDR 把进程带走）→ 看磁盘上那段已经录进去的东西
//! 还能不能播。
//!
//! ## 为什么"硬杀"是最该测的一种失败
//!
//! MP4 的索引 `moov` **只在 `Finalize` 时才写**。现有的两道保险都要求"进程还活着、
//! 还能执行到收尾代码"：
//!
//! | 保险 | 前提 | 硬杀下还成立吗 |
//! |---|---|---|
//! | `SinkFinisher` 守卫 | 栈能展开（panic / `?` 返回） | ✗ 栈不展开，永不 Drop |
//! | `SinkSlot` 跨线程接管 | 收尾线程能跑到 | ✗ 进程整个没了 |
//!
//! ⇒ 磁盘上剩下 `ftyp` + `mdat`，几百 MB 但没有任何播放器认 —— 这正是"几小时白录"。
//!
//! ## 判据
//!
//! ① 录了 90 秒 ⇒ 产物必须显著大于 1 MB（证明确实写进去了，不是空文件）。
//! ② 硬杀之后产物**仍含 `moov`** ⇒ 前面录的内容可播放。
//!
//! ★ ② 在当前架构下**必然失败**，本探针就是为了把这个"必然"钉成可见的事实。
//!   分段落盘落地后，② 应当翻绿（那时本探针升级为门禁）。
//!
//! ## 跑法（**不进日常编译**，见 `Cargo.toml` 探针说明）
//!
//! ```text
//! python build/probe.py run crash_loss -- --nocapture        # 默认 90 秒
//! CRASH_SECS=300 python build/probe.py run crash_loss -- --nocapture
//! ```
//!
//! ## 量接缝（不硬杀，正常停止）
//!
//! `CRASH_NOKILL=1` 让子进程走完整收尾再退出，用来看**段与段的接缝质量**：
//!
//! ```text
//! CRASH_NOKILL=1 CRASH_SECS=42 CRASH_SEG_SECS=10 \
//!   python build/probe.py run crash_loss -- --nocapture
//! ```
//!
//! ★ 子进程会初始化 `env_logger`（`info` 级），`recorder.rs` 那条
//!   "分段：第 N 段已落盘 … 切换耗时 X ms" 因此可见 —— 那是段切换代价的唯一实测来源。
//!
//! ★ 必须 `--test-threads=1`（`probe.py` 已带）：DXGI 桌面复制全进程只容一份。

#![cfg(windows)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{Hardware, RecKind, RecOptions, RecordArea, Recorder};

/// 子进程角色标记：设了这个就只起录制、然后等死（不许走收尾）。
const CHILD_ENV: &str = "CRASH_LOSS_CHILD";
/// 父进程告诉子进程录到哪（子进程其实用不上，仅用于日志对齐）。
const SECS_ENV: &str = "CRASH_SECS";

fn out_path() -> PathBuf {
    let d = PathBuf::from("../temp/crashloss");
    let _ = std::fs::create_dir_all(&d);
    d.join("crash_loss.mp4")
}

/// 分段目录（`recorder.rs` 里 `parts_dir_for` 的同款规则）。
fn parts_dir_of(final_path: &std::path::Path) -> PathBuf {
    let mut p = final_path.to_path_buf();
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rec".into());
    p.set_file_name(format!("{stem}.parts"));
    p
}

/// 一个文件是不是"自带索引、能播"的成品 MP4。
fn is_playable(p: &std::path::Path) -> (bool, u64) {
    let d = std::fs::read(p).unwrap_or_default();
    let bytes = d.len() as u64;
    (
        bytes > 0 && d.windows(4).any(|w| w == b"moov"),
        bytes,
    )
}

/// 子进程：起真录制，然后**永不退出**（被父进程硬杀 ⇒ 栈不展开 ⇒ 不收尾）。
///
/// ★ `CRASH_NOKILL=1` 时改为**正常停止**（走完整收尾）：用于量"段与段的接缝"——
///   硬杀模式下最后一段永远没收尾，量不出接缝质量。
fn child() {
    // ★ logger 必须自己初始化：`recorder.rs` 里那条"段切换耗时 X ms"是 `log::info`，
    //   不初始化就永远看不到，切换代价只能靠猜。
    let _ = env_logger::builder()
        .parse_filters("info")
        .is_test(false)
        .try_init();

    let out = out_path();
    let _ = std::fs::remove_file(&out);
    println!(
        "[child] pid={} 起录制 → {}",
        std::process::id(),
        out.display()
    );
    let rec = Recorder::start(RecOptions {
        kind: RecKind::Mp4,
        path: out.clone(),
        area: RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(8_000),
        max_long_edge: 0,
        hardware: Hardware::Auto,
        show_cursor: true,
        capture_mic: false,
        capture_speaker: true,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("[child] 应能起录制");

    // 每秒报一次进度，父进程的日志里能看出录了多少帧。
    let base = Instant::now();
    let nokill = std::env::var("CRASH_NOKILL").is_ok();
    let secs: u64 = std::env::var(SECS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        println!(
            "[child] 已录 {:.0}s / {} 帧（{}）",
            base.elapsed().as_secs_f64(),
            rec.frames(),
            rec.progress().0.name()
        );
        if nokill && base.elapsed().as_secs() >= secs {
            break;
        }
    }

    if !nokill {
        // 硬杀模式：永不返回（父进程会从外面把整个进程干掉）。
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    // ── 正常停止（量接缝用）──
    println!("[child] 到点了，正常停止（走完整收尾）…");
    let done = Arc::new(AtomicBool::new(false));
    let flag = done.clone();
    rec.finish_detached(Duration::from_secs(90), move |o| {
        println!(
            "[child] 收尾完成：clean={} bytes={:?} err={:?}",
            o.is_clean(),
            o.bytes,
            o.error
        );
        flag.store(true, Ordering::SeqCst);
    });
    let mut w = 0u32;
    while !done.load(Ordering::SeqCst) && w < 900 {
        std::thread::sleep(Duration::from_millis(100));
        w += 1;
    }
    println!("[child] 退出（等了 {} ms）", w * 100);
}

#[test]
#[ignore = "崩溃丢数据取证：会硬杀一个真录制的子进程；跑法见文件头"]
fn hard_kill_loses_everything() {
    let secs: u64 = std::env::var(SECS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(90);
    // ★ 段长**不设时就不传**环境变量，让 `recorder` 用它自己的出厂默认（180s）——
    //   长跑验证要的正是"默认配置下的真实行为"，写死 30 会测成另一套东西。
    //   只有想压缩验证时间时才显式给 `CRASH_SEG_SECS`。
    let seg_secs: Option<u64> = std::env::var("CRASH_SEG_SECS")
        .ok()
        .and_then(|v| v.parse().ok());
    let seg_for_report = seg_secs.unwrap_or(180);

    // ── 子进程角色 ──
    if std::env::var(CHILD_ENV).is_ok() {
        child();
        return;
    }
    let nokill = std::env::var("CRASH_NOKILL").is_ok();

    let out = out_path();
    let _ = std::fs::remove_file(&out);

    println!("════ 崩溃丢数据取证：录 {secs} 秒后硬杀进程 ════");
    println!("  等价于：任务管理器结束 / 系统更新强制重启 / 显卡驱动崩溃带走进程");

    let exe = std::env::current_exe().expect("拿不到测试二进制路径");
    let mut cmd = Command::new(exe);
    // ★ `--ignored` 不能少：本探针是 `#[ignore]`，子进程不加就什么也不跑（实测踩过）。
    cmd.args([
        "--exact",
        "hard_kill_loses_everything",
        "--ignored",
        "--nocapture",
    ])
    .env(CHILD_ENV, "1")
    .env(SECS_ENV, secs.to_string())
    .env("CRASH_NOKILL", if nokill { "1" } else { "0" });
    if let Some(s) = seg_secs {
        cmd.env("CURSEEN_REC_SEG_SECS", s.to_string());
    }
    let mut child = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("起子进程失败");

    // 等它真的录起来（分段目录里的第一段已写入数据），再等满 secs。
    let pdir = parts_dir_of(&out);
    let first = pdir.join("part_0001.mp4");
    let mut waited = 0u32;
    while waited < 60 {
        std::thread::sleep(Duration::from_secs(1));
        waited += 1;
        if std::fs::metadata(&first).map(|m| m.len()).unwrap_or(0) > 200_000 {
            break;
        }
    }
    println!(
        "[parent] 已录 {waited}s，第一段 {} 字节",
        std::fs::metadata(&first).map(|m| m.len()).unwrap_or(0)
    );
    if u64::from(waited) < secs {
        let rest = secs - u64::from(waited);
        println!("[parent] 再录 {rest}s …");
        std::thread::sleep(Duration::from_secs(rest));
    }

    if nokill {
        println!("[parent] 正常停止模式：等子进程自己收尾退出…");
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(500));
    } else {
        println!("[parent] ★ 现在硬杀子进程（TerminateProcess，不给任何收尾机会）");
        let _ = child.kill();
        let _ = child.wait();
        std::thread::sleep(Duration::from_millis(500));
    }

    // ── 取证：扫描分段目录 ──
    let mut parts: Vec<(String, u64, bool)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&pdir) {
        let mut names: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "mp4").unwrap_or(false))
            .collect();
        names.sort();
        for p in names {
            let (ok, bytes) = is_playable(&p);
            parts.push((
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                bytes,
                ok,
            ));
        }
    }
    // ★ 单段归位（NOKILL + 段长 > 录制时长）时产物直接是 `crash_loss.mp4`，
    //   `.parts/` 会被删 —— 此时把最终文件本身当作"一段"纳入取证，否则判据①
    //   会对着 0 字节的分段目录误报（2026-10-06 实测：录 60s 收尾成功、判据却红）。
    if parts.is_empty() {
        if let Ok(md) = std::fs::metadata(&out) {
            let (ok, bytes) = is_playable(&out);
            parts.push((out.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), bytes, ok));
            println!("  （单段模式：取证对象为最终文件 {}，{:.1} MB）", out.display(), md.len() as f64 / 1_048_576.0);
        }
    }
    let total: u64 = parts.iter().map(|(_, b, _)| *b).sum();
    let playable = parts.iter().filter(|(_, _, ok)| *ok).count();

    println!("──── 取证结果 ────");
    println!("  分段目录：{}", pdir.display());
    for (n, b, ok) in &parts {
        println!(
            "    {}  {:>8.1} MB  {}",
            n,
            *b as f64 / 1_048_576.0,
            if *ok { "可播 ✅" } else { "未收尾 ❌" }
        );
    }
    println!(
        "  合计 {:.1} MB，可播 {playable} / {} 段",
        total as f64 / 1_048_576.0,
        parts.len()
    );

    // 预期：硬杀下至少 floor(secs/seg) - 1 段已收尾（最后一段正在写，允许缺索引）；
    //      正常停止下**每一段都走了 Finalize**，一段都不许缺索引。
    let expected: usize = if nokill {
        parts.len()
    } else {
        ((secs / seg_for_report).saturating_sub(1).max(1)) as usize
    };
    println!("  预期至少 {expected} 段可播（录 {secs}s / 段长 {seg_for_report}s）");

    assert!(
        total > 1_048_576,
        "判据① 失败：应录出 > 1MB 数据，实际 {total} 字节"
    );
    assert!(
        playable >= expected as usize,
        "判据② 失败：硬杀后只有 {playable} 段可播，应 >= {expected}"
    );
    // ★ 判据③ 才是"不丢"的实质：**除正在写的最后一段外，每一段都必须是成品**。
    //   最后一段允许没有 moov（它还没到点收尾就被杀了）—— 那正好就是"最坏丢多少"。
    if let Some(finished) = parts.get(..parts.len().saturating_sub(1)) {
        let bad: Vec<&String> = finished
            .iter()
            .filter(|(_, _, ok)| !*ok)
            .map(|(n, _, _)| n)
            .collect();
        assert!(
            bad.is_empty(),
            "判据③ 失败：已收尾的段里有没有索引的：{bad:?}"
        );
    }

    // ── 正常停止时，再走一遍真实收尾：多段 → 合并成单个 MP4 ──
    //
    // 录制结束时 `runtime::record` 会在**后台线程**做这一步；这里同步做，好当场判据。
    //
    // `CRASH_NO_CONCAT=1` ⇒ **停在分段形态不合并**，用于人工查看"到底拆成了几段"
    //   （合并成功后 `.parts/` 会被删掉，不这么做就看不到中间形态）。
    let no_concat = std::env::var("CRASH_NO_CONCAT").map(|v| v == "1").unwrap_or(false);
    if nokill && no_concat {
        println!("──── 保留分段（CRASH_NO_CONCAT=1），人工查看用 ────");
        println!("  分段目录：{}", pdir.display());
        println!("  合并命令：python build/probe.py run concat_parts");
        return;
    }
    if nokill {
        println!("──── 合并（remux）────");
        match curseen_ui::platform::mp4_concat::concat_parts_dir(&out) {
            Ok(Some(st)) => {
                println!(
                    "  ✅ 已合并：{} 段 / 视频 {} 样本 / 音频 {} 样本 / 耗时 {} ms",
                    st.segments, st.video_samples, st.audio_samples, st.elapsed_ms
                );
                let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
                println!("  合并产物 {bytes} 字节");
                assert!(out.is_file(), "判据④ 失败：合并后目标路径应是单个文件");
                assert!(
                    !pdir.exists(),
                    "判据④ 失败：合并成功后 .parts/ 应已删除（失败才该留着）"
                );
                assert!(
                    bytes > 1_048_576,
                    "判据④ 失败：合并产物只有 {bytes} 字节，显然没拼全"
                );
            }
            // 只有一段 = 录不满一段，`run_mp4` 已 rename 归位，本来就该无感。
            Ok(None) => println!("  只有一段，无需合并（与分段之前完全一致）"),
            Err(e) => panic!("判据④ 失败：合并失败（**内容不该丢**）：{e}"),
        }
    }
}
