//! MP4 预览解码探针：对给定的真实 MP4 逐环节跑 [`platform::mp4_trim`] 的
//! `mp4_duration` / `frame_rgba_at`，把每一步的成功/失败与耗时打到 stdout。
//!
//! 跑法：`cargo run --release --example mp4_probe -- <mp4路径> [第二个路径 ...]`
//! 没有参数时内置一组候选路径（找不到就跳过）。

use std::path::PathBuf;
use std::time::Instant;

fn probe(path: &std::path::Path) {
    println!("════ {}", path.display());
    if !path.exists() {
        println!("  文件不存在，跳过");
        return;
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    println!("  大小 {size} 字节");

    let t = Instant::now();
    let dur = curseen_ui::platform::mp4_trim::mp4_duration(path);
    println!("  mp4_duration = {dur:?}（{} ms）", t.elapsed().as_millis());

    let Some(d) = dur else {
        println!("  ✗ 拿不到时长，后面没法试");
        return;
    };
    for frac in [0.0f64, 0.5, 0.999] {
        let t_s = d * frac;
        let t = Instant::now();
        match curseen_ui::platform::mp4_trim::frame_rgba_at(path, t_s) {
            Ok((w, h, rgba)) => {
                // 判据两级：① 中心像素非全 0（解出来了）；② 抽样里有多少种颜色（**不是纯色
                // 帧**）。只有①会漏掉"整帧纯白/纯黑"——那也是解错的一种，看上去仍是"没画面"。
                let n = rgba.len();
                let mid = (n / 2 / 4) * 4;
                println!(
                    "  frame @ {t_s:.2}s → {w}×{h}，{} 字节，耗时 {} ms，中心像素 {:?}，抽样色数 {}",
                    n,
                    t.elapsed().as_millis(),
                    &rgba[mid..mid + 4],
                    distinct_colors(&rgba),
                );
                // 想**亲眼看**这一帧时：设 `MP4_PROBE_DUMP_RAW=<目录>`，解出来的原始 RGBA
                // 直接落盘（用 PIL 转 PNG：`Image.frombytes('RGBA',(w,h),open(..,'rb').read())`）。
                // 行序 / 通道序搞反时统计看不出来，只有看图才抓得到。
                if let Some(dir) = std::env::var_os("MP4_PROBE_DUMP_RAW") {
                    let out = PathBuf::from(dir).join(format!("frame_{t_s:.2}.raw"));
                    match std::fs::write(&out, &rgba) {
                        Ok(()) => println!("  原始帧已落盘 {}", out.display()),
                        Err(e) => println!("  ✗ 落盘失败：{e}"),
                    }
                }
            }
            Err(e) => println!(
                "  ✗ frame @ {t_s:.2}s 失败（{} ms）：{e}",
                t.elapsed().as_millis()
            ),
        }
    }

    // 产物自检：裁出 1~3s 一段，再对**产物本身**跑时长 + 解码（自家产物也要能打开）。
    if let Some(dst) = std::env::var_os("MP4_PROBE_TRIM_OUT").map(PathBuf::from) {
        let t = Instant::now();
        let keep = [(0.5f64, 2.5f64), (d * 0.6, (d * 0.6 + 1.0).min(d))];
        let mut last_pct = -1i32;
        match curseen_ui::platform::mp4_trim::trim_ranges_progress(path, &dst, &keep, &mut |p| {
            let pct = (p * 100.0) as i32;
            if pct / 10 != last_pct / 10 {
                last_pct = pct;
                println!("    {pct}%");
            }
        }) {
            Ok(st) => {
                println!(
                    "  trim_ranges → {} 保留 {:.1}s（{} ms），产物 {} 字节",
                    dst.display(),
                    st.kept_s,
                    t.elapsed().as_millis(),
                    std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0)
                );
                let dur2 = curseen_ui::platform::mp4_trim::mp4_duration(&dst);
                println!("  产物时长 = {dur2:?}");
                let t = Instant::now();
                match curseen_ui::platform::mp4_trim::frame_rgba_at(&dst, 0.0) {
                    Ok((w, h, rgba)) => println!(
                        "  产物 frame @ 0s → {w}×{h}，耗时 {} ms，中心像素 {:?}，抽样色数 {}",
                        t.elapsed().as_millis(),
                        &rgba[rgba.len() / 2..][..4],
                        distinct_colors(&rgba),
                    ),
                    Err(e) => println!("  ✗ 产物 frame @ 0s 失败：{e}"),
                }
            }
            Err(e) => println!("  ✗ trim_ranges 失败：{e}"),
        }
    }
}

/// 抽样统计**不同颜色的种数**（上限 512 后提前收工）。
///
/// 纯色帧（整帧纯白 / 纯黑 / 未初始化缓冲）会返回 1 —— 那是"解码出了形状但内容不对"，
/// 只查中心像素看不出来，所以探针要这一列。
fn distinct_colors(rgba: &[u8]) -> usize {
    let px = rgba.len() / 4;
    if px == 0 {
        return 0;
    }
    let step = (px / 4096).max(1);
    let mut seen = std::collections::HashSet::new();
    let mut i = 0usize;
    while i < px {
        seen.insert((rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2]));
        if seen.len() >= 512 {
            break;
        }
        i += step;
    }
    seen.len()
}

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths: Vec<PathBuf> = if args.is_empty() {
        vec![PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../temp/record/sample.mp4"
        ))]
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    for p in &paths {
        probe(p);
    }
}
