//! # I-beam 光标闪烁取证探针（`cursor_flicker`）—— 2026-10-09
//!
//! ## 它回答一个问题
//!
//! 录屏「显示光标」产物里，**文本 I-beam 光标为什么会闪烁**（用户实测：TIM 里输入
//! 数字，I-beam 一闪一闪）。已知的结构性嫌疑：
//!
//! 1. **超时帧重复画光标**：主循环每 tick 恒写一帧，而 `pump()` 超时帧（桌面无变化）
//!    的 `acq` 纹理里**还留着上一帧画过的光标**，`draw_cursor` 又画一次 —— 单色
//!    AND/XOR 光标（I-beam 就是）的 XOR 部分画两次 = 恢复原样。打字场景桌面大部分
//!    tick 不动（只有插入符 2Hz 闪），光标就「fresh 帧全显 / 超时帧缺一块」交替。
//! 2. **flags 等值比较**：`ci.flags != CURSOR_SHOWING` 是等值而非按位 —— Windows 10+
//!    还有 `CURSOR_SUPPRESSDISPLAYCAPTURE`（0x2）可能与 SHOWING（0x1）同置，flags==3
//!    时光标被整帧跳过。
//!
//! ## 怎么复现
//!
//! 配合 `build/cursor_blink_driver.py`：一个类光标为 IDC_IBEAM 的浅灰窗口、鼠标停在
//! 正中，插入符 2Hz 闪烁（= 打字时的桌面更新节奏）。真录 6s（30fps、显示光标），
//! 同时 100Hz 轮询 `GetCursorInfo` 记 flags 分布；产物解 NV12，逐帧数光标区暗像素
//! （I-beam 黑柱），统计可见率与帧间波动。
//!
//! 跑法：`python build/probe.py run cursor_flicker`
#![cfg(windows)]

use curseen_ui::platform::recorder::{Hardware, RecKind, RecOptions, RecordArea, Recorder};
use std::io::{BufRead, BufReader};
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetSystemMetrics, LoadCursorW, CURSORINFO, IDC_IBEAM, SM_CXSCREEN,
};

const REC_SECS: u64 = 6;
const FPS: u32 = 30;
/// 光标检测框（视频像素，中心对准鼠标位置）：I-beam 高约 24~32 物理像素。
const BOX_W: i64 = 32;
const BOX_H: i64 = 44;
/// 「暗像素」阈值：黑柱 Y < 100，浅灰背景 Y ≈ 240。
const DARK_Y: u8 = 100;
/// 判据：可见帧占比下限。
const VISIBLE_MIN: f64 = 0.90;

#[test]
#[ignore = "真产物闪烁分析：纯解码，跑法见文件头（CF_MP4=路径）"]
fn cf_analyze() {
    unsafe {
        let path = std::path::PathBuf::from(std::env::var("CF_MP4").unwrap_or_default());
        assert!(path.exists(), "产物不存在：{}", path.display());
        let _mf = MfStartupGuard::new();
        let max_frames: usize = std::env::var("CF_MAX_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000);

        let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)
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
        let Some(vid) = vid else { panic!("没有视频轨") };
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("全不选失败");
        reader.SetStreamSelection(vid, true).expect("选视频失败");
        let vty = MFCreateMediaType().expect("建类型失败");
        vty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
        vty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
        reader.SetCurrentMediaType(vid, None, &vty).expect("设 NV12 失败");
        let got = reader.GetCurrentMediaType(vid).expect("取实际类型失败");
        let packed = got.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (w, h) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(w) as usize;
        println!("视频 {w}×{h}");

        // ── 32×32 块级帧差统计：闪烁 = 某个小块反复变化 ──
        const BLOCK: usize = 32;
        let bw = (w as usize) / BLOCK;
        let bh = (h as usize) / BLOCK;
        let mut diff_cnt = vec![0u32; bw * bh];
        // 记录 diff 最多的块的逐帧块内"暗像素数 + 均值"序列。
        let mut prev_mean = vec![0f64; bw * bh];
        let mut prev_dark = vec![0i64; bw * bh];
        let mut series: Vec<(f64, Vec<u32>, Vec<f64>)> = Vec::new(); // (t, 每块暗像素, 每块均值)
        let mut frames = 0usize;

        loop {
            if frames >= max_frames {
                break;
            }
            let mut flags = 0u32;
            let mut actual = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(vid, 0, Some(&mut actual), Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 || sample.is_none() {
                break;
            }
            let s = sample.unwrap();
            let ts = s.GetSampleTime().unwrap_or(0) as f64 / 1e7;
            let buf = s.ConvertToContiguousBuffer().expect("Convert 失败");
            let mut data = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur = 0u32;
            buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur)).expect("Lock 失败");
            let px = std::slice::from_raw_parts(data as *const u8, cur as usize);

            let mut blk_mean = vec![0f64; bw * bh];
            let mut blk_dark = vec![0u32; bw * bh];
            for by in 0..bh {
                for bx in 0..bw {
                    let (mut sum, mut cnt, mut dark) = (0u64, 0u64, 0u32);
                    for y in by * BLOCK..(by + 1) * BLOCK {
                        let row = y * stride;
                        for x in bx * BLOCK..(bx + 1) * BLOCK {
                            let v = px[row + x] as u64;
                            sum += v;
                            cnt += 1;
                            if v < 100 {
                                dark += 1;
                            }
                        }
                    }
                    let m = sum as f64 / cnt as f64;
                    blk_mean[by * bw + bx] = m;
                    blk_dark[by * bw + bx] = dark;
                    // 两个灵敏度都算：均值（整块变亮/变暗）与暗像素数（细光标
                    // 出现/消失——I-beam 黑柱只占块内 ~60px，均值变化 ~13 太边缘）。
                    if frames > 0
                        && ((m - prev_mean[by * bw + bx]).abs() > 12.0
                            || (dark as i64 - prev_dark[by * bw + bx]).abs() > 8)
                    {
                        diff_cnt[by * bw + bx] += 1;
                    }
                }
            }
            prev_mean = blk_mean.clone();
            prev_dark = blk_dark.iter().map(|&v| v as i64).collect();
            let hot = diff_cnt.iter().max().cloned().unwrap_or(0);
            if frames < 400 {
                series.push((ts, blk_dark.clone(), blk_mean.clone()));
            }
            frames += 1;
            let _ = buf.Unlock();
        }

        // 变化次数最多的 8 个块
        let mut order: Vec<usize> = (0..diff_cnt.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(diff_cnt[i]));
        println!(
            "共 {frames} 帧，变化最频繁的块（块号 bx,by / 变化次数 / 平均暗像素）："
        );
        for &i in order.iter().take(8) {
            let (bx, by) = (i % bw, i / bw);
            let avg_dark = series.iter().map(|(_, d, _)| d[i] as f64).sum::<f64>()
                / series.len().max(1) as f64;
            println!(
                "  块({bx:>3},{by:>3}) px=({}, {}) 变化 {}/{} 帧，平均暗像素 {avg_dark:.1}",
                bx * BLOCK,
                by * BLOCK,
                diff_cnt[i],
                frames
            );
        }

        // 最热块的暗像素 + 均值时间序列（前 90 帧），看闪烁节奏。
        let hot_i = order[0];
        let (hx, hy) = ((hot_i % bw) * BLOCK, (hot_i / bw) * BLOCK);
        println!(
            "
最热块 ({hx},{hy}) 的逐帧（帧号: 暗像素/均值Y），前 90 帧："
        );
        for (k, (t, d, m)) in series.iter().take(90).enumerate() {
            if k % 6 == 0 {
                print!("
  ");
            }
            print!("{}:{:.0}/{} ", k, d[hot_i], m[hot_i] as i64);
        }
        println!();

        // 判据：正常录屏里 UI 静止区域不应高频变化；热块变化超过总帧数一半即为闪烁。
        let top = diff_cnt[order[0]];
        assert!(
            top as usize * 2 < frames,
            "块 ({hx},{hy}) 在 {}/{} 帧发生跳变：光标在录制中反复出现/消失（闪烁实锤）",
            top,
            frames
        );
        println!("
✅ 无高频闪烁块");
    }
}

#[test]
#[ignore = "I-beam 光标闪烁取证：真录 6s + 驱动窗 + 解码（合计约 20s）；跑法见文件头"]
fn cursor_flicker() {
    unsafe {
        // ── 1. 拉起驱动窗，等 READY（拿光标绝对坐标）──
        let mut drv = std::process::Command::new("python");
        drv.arg("build/cursor_blink_driver.py")
            .env("MARKER_SECS", (REC_SECS + 4).to_string())
            .env("BLINK_MS", "500")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        let mut child = drv.spawn().expect("拉起驱动失败");
        let (cx, cy) = wait_ready(BufReader::new(child.stdout.take().expect("驱动无输出")));
        println!("驱动就绪：光标停在绝对 ({cx},{cy})");

        // ── 2. 起录（生产链路：显示光标 + 30fps + 主屏）──
        let path = std::path::PathBuf::from("../temp/cursor_flicker.mp4");
        let _ = std::fs::remove_file(&path);
        let rec = Recorder::start(RecOptions {
            kind: RecKind::Mp4,
            path: path.clone(),
            area: RecordArea::Primary,
            crop: None,
            fps: FPS,
            bitrate_kbps: Some(4000),
            max_long_edge: 1280,
            hardware: Hardware::Auto,
            show_cursor: true,
            capture_mic: false,
            capture_speaker: false,
            mic_denoise: "off".into(),
            mic_device: String::new(),
            speaker_device: String::new(),
            gif_loop: true,
        })
        .expect("起录制失败");

        // ── 3. 录制期间 100Hz 轮询光标状态，记 flags 分布 ──
        let mut flags_hist: std::collections::BTreeMap<u32, u32> = Default::default();
        let mut polls = 0u32;
        let mut hcursor_nonnull = 0u32;
        let mut ibeam_hits = 0u32;
        let ibeam = LoadCursorW(None, IDC_IBEAM).expect("LoadCursorW(IDC_IBEAM) 失败");
        let t0 = std::time::Instant::now();
        while t0.elapsed() < Duration::from_secs(REC_SECS) {
            let mut ci = CURSORINFO {
                cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };
            if GetCursorInfo(&mut ci).is_ok() {
                *flags_hist.entry(ci.flags.0).or_insert(0) += 1;
                if !ci.hCursor.0.is_null() {
                    hcursor_nonnull += 1;
                    if ci.hCursor == ibeam {
                        ibeam_hits += 1;
                    }
                }
            }
            polls += 1;
            std::thread::sleep(Duration::from_millis(10));
        }
        let outcome = rec.finish();
        let _ = child.kill();
        assert!(outcome.error.is_none(), "录制报错：{:?}", outcome.error);
        println!(
            "GetCursorInfo 轮询 {polls} 次，flags 分布：{flags_hist:?}，hCursor 非空 {hcursor_nonnull} 次，其中 I-beam {ibeam_hits} 次"
        );
        assert!(
            ibeam_hits >= polls * 8 / 10,
            "光标不是 I-beam（{ibeam_hits}/{polls}）：驱动窗没接到鼠标或类光标没生效，判据作废"
        );

        // ── 4. 解 NV12，逐帧数光标区暗像素 ──
        let _mf = MfStartupGuard::new();
        let (dark_seq, (w, h)) = decode_dark_seq(&path, cx, cy);
        let n = dark_seq.len();
        assert!(n >= ((REC_SECS * FPS as u64) / 2) as usize, "解码帧数太少：{n}");
        let scale_w = w as f64 / GetSystemMetrics(SM_CXSCREEN) as f64;
        println!("视频 {w}×{h}，主屏缩放系数 {scale_w:.3}；共 {n} 帧");

        let visible = dark_seq.iter().filter(|&&c| c >= 10).count();
        let mean = dark_seq.iter().sum::<u32>() as f64 / n as f64;
        let var = dark_seq
            .iter()
            .map(|&c| (c as f64 - mean) * (c as f64 - mean))
            .sum::<f64>()
            / n as f64;
        let changed = dark_seq.windows(2).filter(|p| p[0].abs_diff(p[1]) > 5).count();
        println!(
            "暗像素/帧：均值 {mean:.1} 标准差 {:.1} 最小 {} 最大 {}",
            var.sqrt(),
            dark_seq.iter().min().unwrap(),
            dark_seq.iter().max().unwrap()
        );
        println!(
            "可见帧（暗像素≥10）：{visible}/{n} = {:.1}%",
            visible as f64 / n as f64 * 100.0
        );
        println!("帧间跳变（|Δ|>5）：{changed}/{n} 次相邻帧切换");

        // ★ 判据 = 稳定性而不是绝对可见数：缩放 0.667 后 I-beam 黑柱只剩 ~4 个
        //   Y<100 像素（4:2:0 再压一半），按"≥10 才算可见"会误判。闪烁的本质是
        //   帧间振荡（实测改前 std 3.0、0↔15 跳；修复后 std 0、恒 4），所以：
        //   均值 >0（光标在）且帧间跳变率 ≤5%。
        assert!(
            mean >= 1.0,
            "光标框平均暗像素 {mean:.1} ≈ 0：光标根本没画上，判据作废"
        );
        assert!(
            changed * 20 <= n,
            "帧间跳变 {changed}/{n}（{:.1}% > 5%）：光标在帧间反复出现/消失（闪烁）",
            changed as f64 / n as f64 * 100.0
        );
        println!("\n✅ I-beam 光标逐帧稳定");
    }
}

/// 读驱动 stdout 直到 READY 行，返回光标绝对坐标。
fn wait_ready(out: impl BufRead) -> (i32, i32) {
    for line in out.lines() {
        let line = line.expect("读驱动输出失败");
        if let Some(rest) = line.strip_prefix("CURSOR_DRV_READY ") {
            let mut it = rest.split_whitespace();
            let x = it.next().and_then(|v| v.parse().ok()).expect("READY 无 x");
            let y = it.next().and_then(|v| v.parse().ok()).expect("READY 无 y");
            return (x, y);
        }
        if line.starts_with("CURSOR_DRV_ERR") {
            panic!("驱动报错：{line}");
        }
    }
    panic!("驱动没打 READY 行");
}

/// 解产物视频轨为 NV12，逐帧统计光标框内暗像素数。返回（每帧暗像素, 视频尺寸）。
unsafe fn decode_dark_seq(path: &std::path::Path, cx: i32, cy: i32) -> (Vec<u32>, (u32, u32)) {
    unsafe {
        let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let reader =
            MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None).expect("建 SourceReader 失败");

        // 找视频流
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
        let Some(vid) = vid else { panic!("没有视频轨") };
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("全不选失败");
        reader.SetStreamSelection(vid, true).expect("选视频失败");
        let vty = MFCreateMediaType().expect("建类型失败");
        vty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
        vty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
        reader
            .SetCurrentMediaType(vid, None, &vty)
            .expect("设 NV12 失败");
        let got = reader.GetCurrentMediaType(vid).expect("取实际类型失败");
        let packed = got.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (w, h) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        let stride = got.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(w) as usize;
        assert!(w > 0 && h > 0, "帧尺寸解析失败");

        // 光标框（视频坐标）：主屏原点 (0,0)，绝对坐标 × 缩放。
        let scale = w as f64 / GetSystemMetrics(SM_CXSCREEN) as f64;
        let icx = (cx as f64 * scale) as i64;
        let icy = (cy as f64 * scale) as i64;
        let x0 = (icx - BOX_W / 2).clamp(0, w as i64 - 1);
        let x1 = (icx + BOX_W / 2).clamp(0, w as i64 - 1);
        let y0 = (icy - BOX_H / 2).clamp(0, h as i64 - 1);
        let y1 = (icy + BOX_H / 2).clamp(0, h as i64 - 1);
        println!("光标检测框：视频坐标 x[{x0},{x1}) y[{y0},{y1})");

        let mut seq = Vec::new();
        let (mut ysum, mut ycnt) = (0u64, 0u64);
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
            let s = sample.unwrap();
            let buf = s.ConvertToContiguousBuffer().expect("Convert 失败");
            let mut data = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur = 0u32;
            buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur))
                .expect("Lock 失败");
            let px = std::slice::from_raw_parts(data as *const u8, cur as usize);
            // Y 平面在 NV12 头部，每像素 1 字节、按 stride 排。
            let mut dark = 0u32;
            for y in y0..y1 {
                for x in x0..x1 {
                    let v = px[y as usize * stride + x as usize] as u64;
                    ysum += v;
                    ycnt += 1;
                    if v < DARK_Y as u64 {
                        dark += 1;
                    }
                }
            }
            seq.push(dark);
            let _ = buf.Unlock();
        }
        // 背景 sanity：光标框必须落在驱动窗的浅灰背景上（均值 >200），否则
        // 判据测的是别的窗口/壁纸（实测：驱动窗没置顶时整个框全暗 = 假绿）。
        let ymean = ysum as f64 / ycnt.max(1) as f64;
        println!("光标框平均 Y = {ymean:.1}（浅灰背景应 >200）");
        assert!(
            ymean > 200.0,
            "光标框背景不是驱动窗（平均 Y {ymean:.1}）：窗口被盖或坐标错，判据作废"
        );
        (seq, (w, h))
    }
}
struct MfStartupGuard;

impl MfStartupGuard {
    fn new() -> Self {
        unsafe { MFStartup(MF_VERSION, 0).expect("MFStartup 失败") };
        Self
    }
}

impl Drop for MfStartupGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
        }
    }
}
