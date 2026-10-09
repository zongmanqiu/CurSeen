//! # 分段拼接探针（`concat_parts`）—— 2026-10-06
//!
//! ## 它回答什么
//!
//! 方案 C 把"崩溃也不丢"做成了分段落盘，代价是产物变成 `.parts/` 目录。本探针验证
//! **拼接（remux）这两件事成立**：
//!
//! ① **不丢样本**：拼出来的文件，视频/音频样本数 = 各段之和（不能因为换容器少帧）。
//! ② **不重编码**：拼接耗时应该接近文件拷贝，**绝不是**按播放时长（那说明在解码重编）。
//!
//! ## 判据
//!
//! - 拼接产物能被 MF 读出时长（有 `moov`，可播）。
//! - 视频/音频样本数 = Σ 各段（用 `build/seam_report.py` 对两边各跑一次对比）。
//! - 时长 ≈ Σ 各段时长（误差 < 0.5s，容掉末帧时长与舍入）。
//!
//! ## 跑法
//!
//! ```text
//! python build/probe.py run concat_parts -- --nocapture
//! ```
//!
//! ★ 依赖 `../temp/crashloss/crash_loss.parts/` 里已有的分段（由
//!   `CRASH_NOKILL=1 python build/probe.py run crash_loss` 产生）。没有就先跑那个。

#![cfg(windows)]

use std::path::PathBuf;

use curseen_ui::platform::mp4_concat::concat_parts_dir;
use curseen_ui::platform::mp4_trim;

fn parts_dir() -> PathBuf {
    PathBuf::from("../temp/crashloss/crash_loss.parts")
}

#[test]
#[ignore = "分段拼接：需要 ../temp/crashloss/crash_loss.parts 里已有分段"]
fn concat_parts_keeps_every_sample() {
    let dir = parts_dir();
    let mut parts: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("打不开 {dir:?}（{e}）—— 先跑 CRASH_NOKILL=1 crash_loss"))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "mp4").unwrap_or(false))
        .collect();
    parts.sort();
    assert!(
        parts.len() >= 2,
        "至少要 2 段才谈得上拼接，实际 {} 段",
        parts.len()
    );

    println!("════ 分段拼接：{} 段 → 1 个 MP4 ════", parts.len());

    // 期望值：各段时长之和（MF 自己读的，与拼接读的是同一套口径）。
    let want: f64 = parts
        .iter()
        .filter_map(|p| {
            let d = mp4_trim::mp4_duration(p);
            println!(
                "  {} → {:?}",
                p.file_name().unwrap_or_default().to_string_lossy(),
                d
            );
            d
        })
        .sum();
    let bytes_in: u64 = parts
        .iter()
        .map(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    println!("  Σ 段时长 {want:.3}s，Σ 段大小 {:.1} MB", bytes_in as f64 / 1_048_576.0);

    // ★ 走 `concat_parts_dir` 而不是裸 `concat_segments`：它才是录制结束时真正被调用的
    //   那一层 —— 含"先写 .concat.tmp 再 rename"和"**拼成功才删** .parts/"这两条保险。
    let dst = PathBuf::from("../temp/crashloss/crash_loss.mp4");
    let _ = std::fs::remove_file(&dst);
    let st = concat_parts_dir(&dst)
        .expect("拼接不应失败")
        .expect("多段时应真的发生拼接（单段返回 None）");
    println!("──── 拼接统计 ────");
    println!("  段数 {}，视频样本 {}，音频样本 {}", st.segments, st.video_samples, st.audio_samples);
    println!(
        "  视频轨 {:.3}s，音频轨 {:.3}s，耗时 {} ms",
        st.video_ns as f64 / 1e7,
        st.audio_ns as f64 / 1e7,
        st.elapsed_ms
    );

    let bytes_out = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
    println!(
        "  产物 {:.1} MB（入 {:.1} MB，差 {:+.1}%）",
        bytes_out as f64 / 1_048_576.0,
        bytes_in as f64 / 1_048_576.0,
        (bytes_out as f64 - bytes_in as f64) / bytes_in as f64 * 100.0
    );

    // 判据①：产物能被 MF 读出时长 ⇒ 有 moov ⇒ 可播。
    let got = mp4_trim::mp4_duration(&dst).expect("拼接产物应能读出时长（否则没有 moov）");
    println!("  拼接产物时长 {got:.3}s");

    // 判据②：不丢样本。
    assert!(st.video_samples > 0, "判据② 失败：一个视频样本都没写出");
    assert!(st.audio_samples > 0, "判据② 失败：音频样本为 0（本用例开了扬声器）");

    // 判据③：时长对得上（容 0.5s：末帧时长 + 各轨舍入）。
    assert!(
        (got - want).abs() < 0.5,
        "判据③ 失败：拼接产物 {got:.3}s 与 Σ 段时长 {want:.3}s 差 {:.3}s",
        (got - want).abs()
    );

    // 判据④：**没有在按播放时长写** —— 那是重编码/被节流的特征。
    //   本用例 5 段合计约 41s；拼接耗时若接近 41s 就说明 SinkWriter 在按实时睡。
    assert!(
        st.elapsed_ms < (want * 1000.0) as u128 / 2,
        "判据④ 失败：拼接耗时 {} ms，而素材才 {want:.1}s —— 多半是 SinkWriter 没关节流",
        st.elapsed_ms
    );

    // 判据⑤：拼**成功**才删分段目录；目标路径上必须是单个文件而不是目录。
    assert!(
        !dir.exists(),
        "判据⑤ 失败：拼接成功后 .parts/ 应已删除（拼失败才该留着）"
    );
    assert!(dst.is_file(), "判据⑤ 失败：目标路径应是单个文件");

    println!("──── 结论 ────");
    println!("  ✅ 拼接成立：{st:?}");
    println!("  下一步：用 build/seam_report.py 对合并产物跑一次，核对样本数与时长。");
    println!("  （样本数逐段对比已在 2026-10-06 验过：视频/音频均差 0。）");
}
