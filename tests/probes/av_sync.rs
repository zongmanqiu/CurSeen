//! # 音画同步取证探针（`av_sync`）—— 2026-10-06
//!
//! ## 它回答一个问题
//!
//! 长录产物**音画是否同步**。配合 `build/av_marker_driver.py`（录制期间屏幕上
//! 每 5s 白闪 0.5s 一次、扬声器同时放"440Hz 连续 + 每 5s 一个 1760Hz 标记 0.3s"）：
//!
//! ```text
//! 视频轨：逐帧解 NV12（解码器原生输出），右缘竖条（宽 640px 全高）Y 均值找白闪起点
//! 音频轨：解 PCM，10ms 跳窗 Goertzel，在 1760Hz 找标记起点
//! 漂移[k] = 第 k 次白闪的视频时刻 − 第 k 个标记的音频时刻
//! ```
//!
//! 判据：
//! ① 白闪数与标记数一致（都 ≈ 时长/5s）；
//! ② 漂移去除固定偏移后的**波动 ≤ 100ms**（固定偏移 = 驱动起步对表的常量，不算漂移）；
//! ③ 440Hz 连续音无 >0.3s 的断流（断流 = 音轨掉内容）。
//!
//! ## 跑法（不进日常编译）
//!
//! ```text
//! AV_SYNC_MP4=../temp/soaklink/soak_link_7200s.mp4 python build/probe.py run av_sync
//! ```
#![cfg(windows)]

use windows::core::PCWSTR;
use windows::Win32::Media::Audio::WAVEFORMATEX;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::CoTaskMemFree;

/// 待分析的产物（环境变量 `AV_SYNC_MP4` 可覆盖）。
const DEFAULT_IN: &str = "../temp/soaklink/soak_link_120s.mp4";
/// 标记周期（与 av_marker_driver 的 BLOCK_S 一致）。
const BLOCK_S: f64 = 5.0;
/// 右缘竖条宽度（白闪窗 480px + 边距 24px 都在这条里，且与上下翻转无关）。
const STRIP_W: u32 = 640;
/// 亮度采样步长（4 = 每 4×4 像素取 1 个，均值精度足够、快 16 倍）。
const LUMA_STEP: usize = 4;
/// 白闪检测区：右上角 STRIP_W × STRIP_H（白闪窗 480×480 在 top+24 / 右缘-24，
/// NV12 是 top-down，行 0 = 画面顶部）。限定窗口实际所在区域，排除桌面其余
/// 内容变化造成的假闪。
const STRIP_H: u32 = 640;
/// 白闪判定：检测区均值高出滚动基线多少。
const FLASH_DELTA: f32 = 30.0;
/// 漂移波动判据（ms）。
const DRIFT_TOL_MS: f64 = 100.0;

#[test]
#[ignore = "音画同步取证：真解码整段产物（2h 视频约几分钟）；跑法见文件头"]
fn av_sync() {
    let path = std::env::var("AV_SYNC_MP4").unwrap_or_else(|_| DEFAULT_IN.to_string());
    let inp = std::path::PathBuf::from(&path);
    assert!(inp.exists(), "输入不存在: {path}");
    // 长录：配对数要求高（2h 产物上千对）。
    unsafe { analyze_av(&inp, 5) };
}

/// 音画同步分析主体（`av_sync` 长录 / `av_lag` 短录共用）。
///
/// `min_pairs`：最少可配对事件数——短录只有 3~4 个标记，别按长录的 5 来要求。
unsafe fn analyze_av(inp: &std::path::Path, min_pairs: usize) {
    unsafe {
        let _mf = MfStartupGuard::new();
        let mut wide: Vec<u16> = inp.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), None)
            .expect("建 SourceReader 失败");

        // ── 定位两条流（轨序 ≠ sink 写入序，按 major type 找）──
        let (mut vid, mut aud) = (None, None);
        for i in 0..4u32 {
            let major = reader
                .GetNativeMediaType(i, 0)
                .and_then(|t| t.GetGUID(&MF_MT_MAJOR_TYPE))
                .unwrap_or_default();
            if major == MFMediaType_Video && vid.is_none() {
                vid = Some(i);
            }
            if major == MFMediaType_Audio && aud.is_none() {
                aud = Some(i);
            }
        }
        let Some(vid) = vid else { panic!("没有视频轨") };
        let Some(aud) = aud else { panic!("没有音轨") };

        // ── 双流同读：先把两条流的输出类型都设好，再单循环 ANY_STREAM 读。
        // ★ 不能"先读完视频再读音频"：SourceReader 的读位置是**共享**的，
        //   视频读到 EOS 后位置在文件尾，音频一读就 EOS（实测 0 样本）。
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("全不选失败");
        reader.SetStreamSelection(vid, true).expect("选视频流失败");

        // 视频：部分类型（MAJOR+SUBTYPE）让 SourceReader 自动插 H.264 解码器，
        // 输出 NV12 —— 直接用 Y 平面做白闪检测，不做颜色转换（自拼"完整"
        // RGB32 类型会被拒：0xC00D36B4，"必须完整类型"同类坑）。
        // 白闪在 limited-range Y 平面上是 235 的尖峰，检测同样可靠。
        let vnative = reader.GetNativeMediaType(vid, 0).expect("取视频原生类型失败");
        let packed = vnative.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
        let (w, h) = ((packed >> 32) as u32, (packed & 0xFFFF_FFFF) as u32);
        // ★ MF_MT_FRAME_SIZE：高 32 位是**宽**（写反会让信箱缩放成中带）。
        println!("视频原生 {w}×{h}");
        assert!(w > 0 && h > 0, "帧尺寸解析失败");
        let vty = MFCreateMediaType().expect("建视频媒体类型失败");
        vty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
        vty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12).unwrap();
        reader
            .SetCurrentMediaType(vid, None, &vty)
            .expect("SetCurrentMediaType(NV12) 失败");
        let vgot = reader.GetCurrentMediaType(vid).expect("取实际视频类型失败");
        let vpacked = vgot.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(packed);
        let (w, h) = ((vpacked >> 32) as u32, (vpacked & 0xFFFF_FFFF) as u32);
        let stride =
            vgot.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(w) as usize;
        println!("视频输出 {w}×{h} stride={stride}");

        // 竖条：右缘 STRIP_W 像素全高（白闪窗在右上角，离右缘 ≤ 528px）。
        let strip_x0 = (w.saturating_sub(STRIP_W)) as usize;
        let mut strip_mean: Vec<f32> = Vec::new();
        let mut strip_t: Vec<f64> = Vec::new();
        let mut nv = 0usize;

        // ── 音频类型：显式构造**完整** PCM 类型（空白/半拷贝类型都会被拒）。──
        reader.SetStreamSelection(aud, true).expect("选音频流失败");
        let anative = reader.GetNativeMediaType(aud, 0).expect("取音频原生类型失败");
        let n_rate = anative.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48000);
        let n_ch = anative.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2);
        let aty = MFCreateMediaType().expect("建音频媒体类型失败");
        aty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).unwrap();
        aty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).unwrap();
        aty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, n_rate).unwrap();
        aty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, n_ch).unwrap();
        aty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).unwrap();
        aty.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, n_ch * 2).unwrap();
        aty.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, n_rate * n_ch * 2).unwrap();
        aty.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1).unwrap();
        reader
            .SetCurrentMediaType(aud, None, &aty)
            .expect("SetCurrentMediaType(PCM) 失败");
        let got = reader.GetCurrentMediaType(aud).expect("取实际音频类型失败");
        let mut wfx: *mut WAVEFORMATEX = std::ptr::null_mut();
        let mut wfx_size = 0u32;
        MFCreateWaveFormatExFromMFMediaType(&got, &mut wfx, Some(&mut wfx_size), 0)
            .expect("取 WAVEFORMATEX 失败");
        let (rate, ch) = ((*wfx).nSamplesPerSec, (*wfx).nChannels);
        CoTaskMemFree(Some(wfx as *const _));
        println!("音频解码 {rate}Hz×{ch}ch PCM16");

        // 10ms 跳窗 Goertzel：hop 起点 = 已累加的样本数
        const HOP_MS: f64 = 10.0;
        let hop_n = (rate as f64 * HOP_MS / 1000.0).round() as usize;
        let w17 = 2.0 * std::f64::consts::PI * 1760.0 / rate as f64;
        let w44 = 2.0 * std::f64::consts::PI * 440.0 / rate as f64;
        let (mut c17, mut s17) = (0.0f64, 0.0f64);
        let (mut c44, mut s44) = (0.0f64, 0.0f64);
        let (mut n_in_hop, mut n_total) = (0usize, 0usize);
        let (mut e17_hops, mut e44_hops) = (Vec::<f32>::new(), Vec::<f32>::new());

        // ── 主循环：ANY_STREAM，两流各自数 EOS，都到尾才停。──
        let (mut vid_eos, mut aud_eos) = (false, false);
        while !(vid_eos && aud_eos) {
            let mut flags = 0u32;
            let mut actual = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader
                .ReadSample(
                    MF_SOURCE_READER_ANY_STREAM.0 as u32,
                    0,
                    Some(&mut actual),
                    Some(&mut flags),
                    None,
                    Some(&mut sample),
                )
                .expect("ReadSample 失败");
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                if actual == vid {
                    vid_eos = true;
                } else {
                    aud_eos = true;
                }
                continue;
            }
            let Some(s) = sample else { continue };

            if actual == vid {
                // ── 视频帧：Y 平面竖条均值（NV12：Y 平面在前，每像素 1 字节）──
                let ts = s.GetSampleTime().unwrap_or(0) as f64 / 1e7;
                let buf = s.ConvertToContiguousBuffer().expect("视频 Convert 失败");
                let mut data = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur = 0u32;
                buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur))
                    .expect("视频 Lock 失败");
                let px = std::slice::from_raw_parts(data as *const u8, cur as usize);
                let mut sum = 0u64;
                let mut cnt = 0u64;
                let mut row = 0usize;
                let max_row = (h.min(STRIP_H)) as usize;
                while row < max_row {
                    let base = row * stride + strip_x0;
                    let mut col = 0usize;
                    while col < STRIP_W as usize {
                        let o = base + col;
                        if o < cur as usize {
                            sum += px[o] as u64;
                            cnt += 1;
                        }
                        col += LUMA_STEP;
                    }
                    row += LUMA_STEP;
                }
                buf.Unlock().ok();
                let mean = if cnt > 0 { sum as f32 / cnt as f32 } else { 0.0 };
                strip_t.push(ts);
                strip_mean.push(mean);
                nv += 1;
                if nv % (30 * 600) == 0 {
                    println!("  … 视频已解 {:.0}s（{nv} 帧）", ts);
                }
            } else {
                // ── 音频块：混成立体声均值后喂 Goertzel ──
                let buf = s.ConvertToContiguousBuffer().expect("音频 Convert 失败");
                let mut data = std::ptr::null_mut();
                let mut max_len = 0u32;
                let mut cur = 0u32;
                buf.Lock(&mut data, Some(&mut max_len), Some(&mut cur))
                    .expect("音频 Lock 失败");
                let px = std::slice::from_raw_parts(data as *const i16, (cur / 2) as usize);
                for frm in px.chunks(ch as usize) {
                    let m = (frm[0] as f64 + frm.get(1).copied().unwrap_or(frm[0]) as f64) / 2.0
                        / 32768.0;
                    c17 += m * (w17 * n_in_hop as f64).sin();
                    s17 += m * (w17 * n_in_hop as f64).cos();
                    c44 += m * (w44 * n_in_hop as f64).sin();
                    s44 += m * (w44 * n_in_hop as f64).cos();
                    n_in_hop += 1;
                    n_total += 1;
                    if n_in_hop == hop_n {
                        let p17 = c17 * c17 + s17 * s17;
                        let p44 = c44 * c44 + s44 * s44;
                        e17_hops.push(p17 as f32);
                        e44_hops.push(p44 as f32);
                        c17 = 0.0;
                        s17 = 0.0;
                        c44 = 0.0;
                        s44 = 0.0;
                        n_in_hop = 0;
                    }
                }
                buf.Unlock().ok();
            }
        }
        println!("视频帧 {nv}，末帧 {:.2}s", strip_t.last().unwrap_or(&0.0));
        let audio_secs = n_total as f64 / rate as f64;
        println!(
            "音频 {audio_secs:.2}s，hop 数 {}（{}ms/窗）",
            e17_hops.len(),
            HOP_MS * 1000.0
        );
        assert!(!e17_hops.is_empty(), "音频一个 hop 都没解出来（读位置/选流问题）");

        // ── 3. 标记/白闪检测 ──
        let mark_ts = detect_bursts(&e17_hops, None, 0.01, 8.0, 250, 10);
        println!(
            "音频标记 {} 个（应约 {:.0}）",
            mark_ts.len(),
            audio_secs / BLOCK_S
        );
        // 440Hz 断流
        let gaps = detect_silence(&e44_hops, HOP_MS / 1000.0, 0.25, 0.3);
        println!("440Hz 断流（>0.3s）：{} 段 {:?}", gaps.len(), &gaps[..gaps.len().min(5)]);

        let flash_ts = detect_bursts(&strip_mean, Some(&strip_t), 0.0, FLASH_DELTA, 60, 5);
        println!(
            "视频白闪 {} 个（应约 {:.0}）",
            flash_ts.len(),
            strip_t.last().unwrap_or(&0.0) / BLOCK_S
        );

        // ── 4. 漂移（最近邻配对：活动桌面上可能有假闪/漏闪，
        //        按序号硬配会错一位、漂移全变 ±5s 的垃圾值）──
        let n_pair = mark_ts.len().min(flash_ts.len());
        assert!(
            n_pair >= min_pairs,
            "可配对的标记/白闪太少（{n_pair} < {min_pairs}），驱动没跑或检测失败"
        );
        const PAIR_TOL: f64 = 2.5; // 秒；标记/白闪同钟起步，正常差距远小于 1 个周期
        let mut drift: Vec<f64> = Vec::new();
        let mut pairs: Vec<(f64, f64)> = Vec::new(); // (视频白闪时刻, 音频标记时刻)
        let mut used = vec![false; flash_ts.len()];
        for &m in &mark_ts {
            let mut best: Option<(usize, f64)> = None;
            for (i, &f) in flash_ts.iter().enumerate() {
                if used[i] {
                    continue;
                }
                let d = f - m;
                if d.abs() < PAIR_TOL && best.map_or(true, |(_, bd)| d.abs() < bd.abs()) {
                    best = Some((i, d));
                }
            }
            if let Some((i, d)) = best {
                used[i] = true;
                drift.push(d);
                pairs.push((flash_ts[i], m));
            }
        }
        assert!(drift.len() >= min_pairs, "最近邻配对后只剩 {} 对（标记 {}／白闪 {}）—— 两侧事件对不上",
            drift.len(), mark_ts.len(), flash_ts.len());
        if flash_ts.len() != mark_ts.len() {
            println!(
                "⚠ 白闪 {} 个 vs 标记 {} 个（桌面噪声造成假闪/漏闪，配对后不影响漂移结论）",
                flash_ts.len(),
                mark_ts.len()
            );
        }
        // 逐对明细：固定偏移只是中位数，**趋势（声音越来越晚）藏在斜率里**，
        // 光看中位数会漏掉它 —— 2026-10-09 用户报"声音比画面慢"就是靠这张表定位的。
        println!("  逐对明细（偏移 = 视频白闪 − 音频标记，正 = 声音早）：");
        for (i, &(v, a)) in pairs.iter().enumerate() {
            println!(
                "    #{i}: 视频 {v:.3}s｜音频 {a:.3}s｜偏移 {:+.1}ms",
                (v - a) * 1000.0
            );
        }
        let off = median(&drift);
        // 去固定偏移后的波动（斜率由线性拟合吸收：起点 vs 终点斜率也报出来）
        let n = drift.len() as f64;
        let (sx, sy, sxx, sxy) = drift
            .iter()
            .enumerate()
            .fold((0.0, 0.0, 0.0, 0.0), |(sx, sy, sxx, sxy), (i, d)| {
                let x = i as f64 * BLOCK_S;
                (sx + x, sy + d, sxx + x * x, sxy + x * d)
            });
        let denom = n * sxx - sx * sx;
        let slope = if denom.abs() > 1e-9 {
            (n * sxy - sx * sy) / denom
        } else {
            0.0
        };
        let mut max_dev = 0.0f64;
        for (i, d) in drift.iter().enumerate() {
            let fit = off + slope * (i as f64 * BLOCK_S);
            max_dev = max_dev.max((d - fit).abs());
        }
        println!("\n═══ 音画同步结论 ═══");
        let off_ms = off * 1000.0;
        let cum_ms = slope * 7200.0 * 1000.0;
        let dev_ms = max_dev * 1000.0;
        println!(
            "配对 {n_pair}｜固定偏移 {off_ms:.1}ms｜斜率 {slope:+.4}s/s（2h 累计 {cum_ms:+.1}ms）"
        );
        println!("去偏移+去斜率后最大波动 {dev_ms:.1}ms（判据 ≤ {DRIFT_TOL_MS:.0}ms）");
        // 标记间隔精确性（电流声不复发 = 时间线无吞/重）。
        // ★ 判据红时先取证：打印最差间隔的位置再断言——"多检出/漏检一个标记"
        //   会把 5s 间隔劈成 1s+4s（偏差恰 4s），而真有 9s 的时间线洞时最近邻
        //   配对（容差 2.5s）根本配不上——两者可用配对结果区分。
        let mut gaps: Vec<(usize, f64)> = mark_ts
            .windows(2)
            .enumerate()
            .map(|(i, w)| (i, w[1] - w[0]))
            .collect();
        gaps.sort_by(|a, b| {
            let da = (a.1 - BLOCK_S).abs();
            let db = (b.1 - BLOCK_S).abs();
            db.partial_cmp(&da).unwrap()
        });
        println!("  间隔最差 5 个（时刻 → 间隔）：");
        for (i, g) in gaps.iter().take(5) {
            println!("    #{i}: {:.2}s → {:.2}s，间隔 {g:.3}s（偏差 {:+.0}ms）",
                mark_ts[*i], mark_ts[*i + 1], (g - BLOCK_S) * 1000.0);
        }
        println!(
            "  首末标记：{:.3}s / {:.3}s（共 {} 个）",
            mark_ts.first().copied().unwrap_or(0.0),
            mark_ts.last().copied().unwrap_or(0.0),
            mark_ts.len()
        );
        // 时间线吞/重判据（网格语义）：
        //   **吞** = 标记格点缺位（红）——音频被吞掉 5s 才会缺一格；
        //   **重/漂移** = 漂移判据已覆盖（2h 实测最大波动 68ms）。
        //   孤立的"格点外多检出"不算红：击键音/点击的声画事件会骗过纯突发
        //   检测（2h 产物 231.43s 实录一次，声画同时出现且配对成功 ≤68ms）。
        //   间隔直方里"1s+4s"是多检出把 5s 格劈开的特征，不是时间线有洞——
        //   真有 9s 洞时最近邻配对（容差 2.5s）根本配不上，而本例 1441 全配。
        let t0g = mark_ts[0];
        let tend = *mark_ts.last().unwrap();
        let n_slot = ((tend - t0g) / BLOCK_S + 0.5).floor() as usize + 1;
        let mut missing: Vec<f64> = Vec::new();
        let mut extra: Vec<f64> = Vec::new();
        let mut k = 0usize;
        for s in 0..n_slot {
            let expect = t0g + s as f64 * BLOCK_S;
            let mut in_slot = 0usize;
            while k < mark_ts.len() && mark_ts[k] < expect + 0.5 {
                if mark_ts[k] >= expect - 0.5 {
                    in_slot += 1;
                    if in_slot > 1 {
                        extra.push(mark_ts[k]);
                    }
                } else {
                    // 落在所有格点窗口之外的孤立检出（非标记事件，如击键/点击）
                    extra.push(mark_ts[k]);
                }
                k += 1;
            }
            if in_slot == 0 {
                missing.push(expect);
            }
        }
        println!(
            "  标记网格：{} 格｜缺位（吞）{} 个｜格点外多检出 {} 个 {:?}",
            n_slot,
            missing.len(),
            extra.len(),
            &extra[..extra.len().min(5)]
        );
        assert!(
            missing.is_empty(),
            "标记网格缺位 {missing:?}（时间线吞样本，电流声机制复现）"
        );
        assert!(
            max_dev * 1000.0 <= DRIFT_TOL_MS,
            "音画漂移波动 {dev_ms:.1}ms 超过 {DRIFT_TOL_MS:.0}ms"
        );
        println!("\n✅ 音画同步全绿");
    }
}

/// 音画偏移**短**测（2026-10-09，查"声音比画面慢"用）。
///
/// 与 `av_sync` 的差别只在"产物从哪来"：那条分析别人录好的长录，这条**自己录**。
/// 流程 = 起录 → 拉起 `build/av_marker_driver.py`（置顶白闪窗 + 标记音，**同钟起步**）
/// → 录 20s → 收尾 → 交 `analyze_av` 解码比对。
///
/// 判据方向：`drift = 视频白闪时刻 − 音频标记时刻`。
///   - **负值 = 声音比画面晚**（用户报的"慢一会儿"就是它，绝对值即滞后毫秒数）；
///   - 正值 = 声音抢在画面前面。
///
/// 环境变量：`AV_LAG_RECS`（录多少秒，默认 20）、`AV_LAG_MARK`（驱动跑多少秒，默认 15）、
/// `AV_LAG_PATH`（产物，默认 `../temp/av_lag.mp4`）。
///
/// 跑法：`python build/probe.py run av_sync -- av_lag`
#[test]
#[ignore = "音画偏移短测：真录 20s + 解码（合计约 40s）；跑法见文件头"]
fn av_lag() {
    use curseen_ui::platform::recorder::{Hardware, Recorder, RecKind, RecOptions, RecordArea};
    use std::time::Duration;

    let rec_secs: u64 = std::env::var("AV_LAG_RECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let drv_secs: u64 = std::env::var("AV_LAG_MARK")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(15);
    let path = std::path::PathBuf::from(
        std::env::var("AV_LAG_PATH").unwrap_or_else(|_| "../temp/av_lag.mp4".into()),
    );
    let _ = std::fs::remove_file(&path);

    // ── 1. 起录（麦克风关，只收系统声音：用户报的是系统声音慢）──
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
        capture_mic: std::env::var("AV_LAG_MIC").is_ok(),
        capture_speaker: true,
        mic_denoise: "off".into(),
        mic_device: String::new(),
        speaker_device: String::new(),
        gif_loop: true,
    })
    .expect("起录制失败");
    println!(
        "AV_LAG 录制已起（{rec_secs}s）音源={}麦克风={}",
        if std::env::var("AV_LAG_CLICK").is_ok() {
            "点击（静默+一声）"
        } else if std::env::var("AV_LAG_SPARSE").is_ok() {
            "稀疏脉冲"
        } else {
            "连续音"
        },
        if std::env::var("AV_LAG_MIC").is_ok() { "开" } else { "关" }
    );

    // ── 2. 拉起标记驱动：白闪窗与标记音同一时刻起步 ──
    // `AV_LAG_SPARSE=1` ⇒ 稀疏模式：整段静默，只有每 5s 一声标记。
    // 这才是用户报"慢"的听音场景（连续参考音会掩盖静默期的音画行为）。
    let sparse = std::env::var("AV_LAG_SPARSE").is_ok();
    let mut drv = std::process::Command::new("python");
    drv.arg("build/av_marker_driver.py")
        .env("MARKER_SECS", drv_secs.to_string());
    if sparse {
        drv.env("AV_BASE_AMP", "0");
    }
    // `AV_LAG_CLICK=1` ⇒ 点击模式：只在白闪那一瞬播一声短响，其余时间系统
    // **完全没有播放会话**（loopback 零产出）——这才是用户听出"慢"的场景。
    if std::env::var("AV_LAG_CLICK").is_ok() {
        drv.env("AV_CLICK", "1");
    }
    let child = drv.spawn();
    match &child {
        Ok(c) => println!("AV_LAG 驱动已起 pid={}（{drv_secs}s）", c.id()),
        Err(e) => println!("AV_LAG 驱动起不来：{e}（白闪/标记音缺失，分析必然失败）"),
    }

    // ── 3. 录满、收尾、收工 ──
    std::thread::sleep(Duration::from_secs(rec_secs));
    let outcome = rec.finish();
    println!(
        "AV_LAG 收尾 bytes={:?} error={:?}",
        outcome.bytes, outcome.error
    );
    if let Ok(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    assert!(outcome.error.is_none(), "录制不应报错：{:?}", outcome.error);
    let data = std::fs::read(&path).expect("产物读不出来");
    assert!(data.windows(4).any(|w| w == b"moov"), "产物没有 moov 盒");

    // ── 4. 解两条轨比对（短录只有 3~4 个标记，配对下限取 2）──
    unsafe { analyze_av(&path, 2) };
}

/// 滚动中位数找"突发高值"段的起点（白闪 / 1760Hz 标记通用）。
/// `series_t = None` 时按等间隔 `dt` 取时刻。
fn detect_bursts(
    vals: &[f32],
    series_t: Option<&[f64]>,
    dt: f64,
    delta: f32,
    base_win: usize,
    min_run: usize,
) -> Vec<f64> {
    let mut on = vec![false; vals.len()];
    for i in 0..vals.len() {
        let a = i.saturating_sub(base_win);
        let b = (i + base_win + 1).min(vals.len());
        let mut win: Vec<f32> = vals[a..b].to_vec();
        win.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let med = win[win.len() / 2];
        if vals[i] > med + delta {
            on[i] = true;
        }
    }
    let mut out = Vec::new();
    let mut run = 0usize;
    for i in 0..=vals.len() {
        if i < vals.len() && on[i] {
            run += 1;
        } else {
            if run >= min_run {
                let start = i - run;
                let t = match series_t {
                    Some(ts) => ts[start],
                    None => start as f64 * dt,
                };
                out.push(t);
            }
            run = 0;
        }
    }
    out
}

/// 滚动中位数找"低值"段（440Hz 断流），返回段时刻。
fn detect_silence(vals: &[f32], _dt: f64, ratio: f32, min_secs: f64) -> Vec<f64> {
    let mut all: Vec<f32> = vals.to_vec();
    all.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let med = all[all.len() / 2].max(1e-9);
    let min_run = (min_secs / 0.01) as usize;
    let mut out = Vec::new();
    let mut run = 0usize;
    for (i, v) in vals.iter().enumerate() {
        if *v < med * ratio {
            run += 1;
        } else {
            if run >= min_run {
                out.push((i - run) as f64 * 0.01);
            }
            run = 0;
        }
    }
    if run >= min_run {
        out.push((vals.len() - run) as f64 * 0.01);
    }
    out
}

fn median(v: &[f64]) -> f64 {
    let mut s: Vec<f64> = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    s[s.len() / 2]
}

/// `MFStartup` 的 RAII 包装（探针生命周期内保持初始化）。
struct MfStartupGuard;

impl MfStartupGuard {
    unsafe fn new() -> Self {
        MFStartup(MF_VERSION, 0).expect("MFStartup 失败");
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
