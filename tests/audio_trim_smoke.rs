//! M4A（纯录音产物）在 MF 裁剪链路上的冒烟：时长读取 + 流拷贝裁剪。
//!
//! ★ 这是 `platform::mp4_trim` 音频-only 支持（m81 #3）的**真 MF 回归锁**：
//! `open_passthrough_reader` 对无视频流文件必须成功（`video_idx=None` 不是错误），
//! `mp4_duration_ns` 扫音频流、`trim_ranges` 只拷音频轨——任何一处退化成"必须
//! 有视频流"的旧行为，这条用例当场红。夹具是录音探针的真实产物（5 秒 AAC）。
//!
//! 与 `record_smoke.rs` 同一条纪律：真 MF 调用跑在真机上，慢（秒级）但值得——
//! 这条链 2026-09-30 踩过"流索引写死 0"的坑，纯靠单测扫不出来。

use std::path::Path;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/audio/audio_5s.m4a")
}

use std::path::PathBuf;

#[test]
fn m4a_duration_reads_the_audio_stream() {
    let src = fixture();
    let ns = curseen_ui::platform::mp4_trim::mp4_duration_ns(&src)
        .expect("纯音频文件的时长必须能读出来（无视频流不是错误）");
    let secs = ns as f64 / 10_000_000.0;
    assert!(
        (4.0..=6.0).contains(&secs),
        "夹具是 5 秒录音，实测 {secs:.2}s"
    );
}

#[test]
fn m4a_trim_stream_copies_audio_only() {
    let src = fixture();
    let dst = std::env::temp_dir().join(format!("curseen_audio_trim_{}.m4a", std::process::id()));
    let stats = curseen_ui::platform::mp4_trim::trim_ranges(&src, &dst, &[(0.0, 1.0), (4.0, 5.0)])
        .expect("音频流拷贝裁剪应成功（保留首尾各 1 秒）");
    assert!(
        (1.5..=2.5).contains(&stats.kept_s),
        "保留时长应为 ~2s，实测 {:.2}s",
        stats.kept_s
    );
    let sz = std::fs::metadata(&dst).expect("产物存在").len();
    assert!(
        sz > 10_000,
        "2 秒 AAC ≈ 32KB，实测 {sz}B（太小说明没写进去）"
    );
    // 产物也必须能被同一个 reader 读回（moov 完整、可解析）。
    let ns = curseen_ui::platform::mp4_trim::mp4_duration_ns(&dst).expect("裁剪产物必须可读");
    let secs = ns as f64 / 10_000_000.0;
    assert!((1.5..=2.5).contains(&secs), "产物时长 ~2s，实测 {secs:.2}s");
    let _ = std::fs::remove_file(&dst);
}
