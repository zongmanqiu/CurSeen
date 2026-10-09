//! # 真录制冒烟测试（2026-10-06，修"不确定性录制失败"的回归判据）
//!
//! 旧版（0930）的两个真实失败症状——**保存/停止时软件卡死** 与 **产物打不开/损坏**——
//! 根因链是同一条：编码器反压/收尾等待楔住录制线程 → `finish()` 无限 join 把 UI 拖死
//! → 用户强杀 → `moov` 没写 → 产物打不开。修复 = 关节流 + 空音频流补静音 + 限时 join。
//!
//! 本测试用**真 DXGI + 真 MF + 真 SinkWriter**（不用 mock——三个修复全是平台行为，
//! mock 验不出）钉住两条底线：
//!
//! 1. 正常录完 ⇒ 产物有 `moov`（可播的必要条件）且有体量；
//! 2. **中途注入故障** ⇒ `finish()` 如实报错，且产物**仍然被 `SinkFinisher` 收尾出
//!    `moov`**——"录到的那部分不白录"。
//!
//! 跑法：`cargo test --release -- --test-threads=1`（DXGI 桌面复制全进程只容一份，
//! 这两个用例**必须串行**；仓库门禁本来就是单线程跑）。
use curseen_ui::platform::recorder::{Hardware, RecKind, RecOptions, RecordArea, Recorder};
use std::time::Duration;

fn mp4_opts(path: std::path::PathBuf) -> RecOptions {
    RecOptions {
        kind: RecKind::Mp4,
        path,
        area: RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: Some(4000),
        max_long_edge: 1280,
        hardware: Hardware::Auto,
        show_cursor: false,
        capture_mic: false,
        // 开 loopback：音频流**存在**。机器此刻有没有声音都能过——
        // 没声音 ⇒ 走静音填充（修复 B 的正路径）；有声音 ⇒ 真实样本。
        capture_speaker: true,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    }
}

/// 产物底线：`moov` 盒存在（Finalize 完成过）且有体量。
fn assert_finalized(path: &std::path::Path) {
    let data = std::fs::read(path).expect("产物读不出来");
    assert!(
        data.len() > 100_000,
        "产物只有 {} 字节，等于没录上",
        data.len()
    );
    assert!(
        data.windows(4).any(|w| w == b"moov"),
        "产物没有 moov 盒 = 没有收尾 = 打不开（这正是要防的失败）"
    );
}

#[test]
fn record_normal_finishes_playable() {
    let dir = std::env::temp_dir().join("curseen_record_smoke");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("smoke_normal.mp4");
    let _ = std::fs::remove_file(&path);

    let rec = Recorder::start(mp4_opts(path.clone())).expect("起录制失败");
    std::thread::sleep(Duration::from_secs(8));
    let outcome = rec.finish();
    assert!(
        outcome.error.is_none(),
        "正常录制不应报错：{:?}",
        outcome.error
    );
    assert!(outcome.bytes.unwrap_or(0) > 100_000);
    assert_finalized(&path);
}

#[test]
fn record_fault_midway_still_finalizes() {
    let dir = std::env::temp_dir().join("curseen_record_smoke");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("smoke_fault.mp4");
    let _ = std::fs::remove_file(&path);

    // 约 3 秒（第 90 帧）时在录制线程内注入 Err —— 模拟"录到一半硬件抽风"。
    // 修复 C 之后 `finish()` 的限时 join 也在本用例的覆盖面里（正常路径秒退）。
    // ★ 环境变量在录制线程启动**前**设置才生效，这里是先设后 start，顺序对。
    std::env::set_var("CURSEEN_REC_FAULT_AFTER", "90");
    let rec = Recorder::start(mp4_opts(path.clone())).expect("起录制失败");
    std::thread::sleep(Duration::from_secs(8));
    let outcome = rec.finish();
    std::env::remove_var("CURSEEN_REC_FAULT_AFTER");

    assert!(outcome.error.is_some(), "注入了故障，finish 必须如实报错");
    assert_finalized(&path);
}
