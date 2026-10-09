//! 环境突变探针：录制**中途改分辨率**，逼桌面复制走 `ACCESS_LOST` 重建分支。
//!
//! # 为什么单开一条
//!
//! 2 小时联动长测（soak_link）里，桌面复制会话从头到尾没失效过 —— 而日常最容易
//! 撞的恰恰是"环境突变"：改分辨率、锁屏、UAC 弹安全桌面、显卡驱动重置，都会让
//! `AcquireNextFrame` 返回 `DXGI_ERROR_ACCESS_LOST`。这条分支在长测里**一次都没走到**，
//! 是目前覆盖最薄的一环。
//!
//! 本探针只自动化**可安全自愈**的那一种（分辨率切换：切过去、切回来，脚本全程可控）。
//! 锁屏 / UAC 会留下等待用户交互的安全桌面，自动化会把机器锁死，留作人工试用项。
//!
//! # 判据：不冒充，也不要求一定救回来
//!
//! 重建失败是**允许的**（已经写进文件的那部分仍在盘上），但必须如实报：
//!
//! - 说是"录完了"（`is_clean()`）就必须真的在突变之后**继续出帧**；
//! - 没继续出帧就必须带 `error`，让界面报"中断"而不是报"完成"；
//! - 任何情况下都**不许楔住**（`finish` 要在看门狗时限内返回）、**不许崩溃**。
//!
//! 跑：`python build/probe.py run rec_env_shock`

#![cfg(windows)]

use std::time::{Duration, Instant};

use curseen_ui::platform::recorder::{self, RecOptions};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsW, EnumDisplaySettingsW, CDS_FULLSCREEN, CDS_RESET, DEVMODEW,
    DEVMODE_FIELD_FLAGS, DISP_CHANGE_SUCCESSFUL, DM_BITSPERPEL, DM_DISPLAYFREQUENCY,
    DM_PELSHEIGHT, DM_PELSWIDTH, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};

/// 突变前 / 突变期间 / 突变后各录这么久（总时长约 20 秒）。
const SEG: Duration = Duration::from_secs(6);
/// `finish` 的硬性时限：看门狗 15s + 收尾余量。超过就是楔住了。
const FINISH_LIMIT: Duration = Duration::from_secs(30);

#[test]
#[ignore]
fn resolution_change_does_not_wedge_or_silently_stop() {
    let _ = env_logger::builder().is_test(false).try_init();

    let cur = current_mode();
    let (w0, h0) = (cur.dmPelsWidth, cur.dmPelsHeight);
    println!(
        "当前模式：{}x{} @{}Hz {}bpp",
        w0, h0, cur.dmDisplayFrequency, cur.dmBitsPerPel
    );
    let alt = pick_alt(&cur)
        .unwrap_or_else(|| panic!("没找到可切换的其它分辨率（当前 {w0}x{h0}）——本机不支持这个测试"));
    println!("突变目标：{}x{} @{}Hz", alt.0, alt.1, alt.2);

    let out = std::env::temp_dir().join("curseen_env_shock.mp4");
    let rec = recorder::Recorder::start(RecOptions {
        kind: recorder::RecKind::Mp4,
        path: out.clone(),
        area: recorder::RecordArea::Primary,
        crop: None,
        fps: 30,
        bitrate_kbps: None,
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

    let t0 = wait_first_frame(&rec);
    std::thread::sleep(SEG);
    let f_before = rec.frames();

    // —— 突变：切到另一个分辨率（CDS_FULLSCREEN = 临时模式，不写进注册表）——
    let change = apply_mode(alt.0, alt.1, alt.2, cur.dmBitsPerPel, CDS_FULLSCREEN);
    println!("切到 {}x{}：{change}", alt.0, alt.1);
    std::thread::sleep(SEG);
    let f_during = rec.frames();

    // —— 恢复：显式切回原模式，失败再 CDS_RESET ——
    let back = apply_mode(w0, h0, cur.dmDisplayFrequency, cur.dmBitsPerPel, CDS_FULLSCREEN);
    if back != 0 {
        let r = unsafe { ChangeDisplaySettingsW(None, CDS_RESET) };
        println!("显式切回失败（{back}），已 CDS_RESET：{r:?}");
    }
    std::thread::sleep(SEG);
    let f_after = rec.frames();

    // ★ `dropped` 要在 `finish` 之前取（finish 消耗掉 Recorder）。
    let dropped = rec.dropped();
    let t_finish = Instant::now();
    let done = rec.finish();
    let finish_secs = t_finish.elapsed().as_secs_f64();
    let real = t0.elapsed().as_secs_f64();

    // ★ 显示器必须回到原样：这个测试不留下任何副作用。
    let now = current_mode();
    assert_eq!(
        (now.dmPelsWidth, now.dmPelsHeight),
        (w0, h0),
        "测试结束显示器没回到原分辨率（{}x{}）——请手动改回",
        now.dmPelsWidth,
        now.dmPelsHeight
    );

    println!(
        "\n帧数：突变前 {f_before} → 突变中 {f_during} → 突变后 {f_after} / 真实 {real:.2}s\n\
         收尾 {finish_secs:.2}s / 写入 {} 字节 / 丢弃 {dropped} / 错误 {:?}",
        done.bytes.unwrap_or(0),
        done.error
    );

    // 判据 1：不许楔住 —— 环境突变最容易把收尾拖成永久等待。
    assert!(
        finish_secs < FINISH_LIMIT.as_secs_f64(),
        "收尾 {finish_secs:.2}s 超限 —— 环境突变把收尾拖死了"
    );
    // 判据 2：不许崩溃（子进程异常退出必须被认出来，不是"安静地结束"）。
    // 判据 3：不冒充 —— 报完成就必须真的还在出帧。
    if done.error.is_none() {
        assert!(
            done.is_clean(),
            "没有错误却不算干净收尾：{:?}",
            done.error
        );
        assert!(
            f_after > f_during + 10,
            "报了\"录完了\"，但突变后几乎没再出帧（{f_during} → {f_after}）—— \
             静默停止比报错更糟"
        );
    } else {
        assert!(
            !done.is_clean(),
            "有错误却仍被判为完整录制（{:?}）——界面会把残缺产物说成完成",
            done.error
        );
    }
    // 判据 4：突变之前的内容必须保住（能救回多少是多少，但不能连前面的都丢了）。
    if let Some(n) = mp4_video_sample_count(&out) {
        println!("产物视频样本数 {n}（突变前已采 {f_before} 帧）");
        assert!(
            n + 30 >= f_before,
            "产物只剩 {n} 个视频样本，突变前明明已经采了 {f_before} 帧 —— \
             前面的内容也被丢了"
        );
        assert!(
            !mp4_top_boxes(&out).is_empty() && mp4_top_boxes(&out).contains(&"moov".to_string()),
            "产物缺 moov（顶层盒 {:?}）——不可播放",
            mp4_top_boxes(&out)
        );
    } else {
        // 解析不出样本数 = 没 moov：只在"已如实报错"时才允许。
        assert!(
            done.error.is_some(),
            "产物解不出 moov，却没有报错 —— 残缺产物被当成录完了"
        );
    }

    println!("\n✅ 环境突变：收尾未楔住、状态如实（错误 {:?}）、显示器已复原", done.error);
}

// ——————————————————————— 显示模式（Win32 GDI）———————————————————————

fn current_mode() -> DEVMODEW {
    unsafe {
        let mut dm: DEVMODEW = std::mem::zeroed();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        let ok = EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut dm);
        assert!(ok.as_bool(), "读当前显示模式失败");
        dm
    }
}

/// 枚举所有合法模式，挑一个与当前**尺寸不同**的（刷新率优先取相同的，减少变量）。
fn pick_alt(cur: &DEVMODEW) -> Option<(u32, u32, u32)> {
    let mut cands: Vec<(u32, u32, u32)> = Vec::new();
    unsafe {
        let mut i = 0u32;
        loop {
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            if !EnumDisplaySettingsW(PCWSTR::null(), ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm)
                .as_bool()
            {
                break;
            }
            if dm.dmPelsWidth != cur.dmPelsWidth
                && dm.dmBitsPerPel == cur.dmBitsPerPel
                && dm.dmPelsWidth >= 800
            {
                cands.push((dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency));
            }
            i += 1;
            if i > 512 {
                break;
            }
        }
    }
    // 同刷新率优先；否则退而求其次取频率最高的那个（切换动作本身才是被测物）。
    let same = cands.iter().find(|c| c.2 == cur.dmDisplayFrequency).copied();
    same.or_else(|| cands.iter().max_by_key(|c| c.2).copied())
}

/// 切模式，返回 0 = 成功（DISP_CHANGE_SUCCESSFUL）。
fn apply_mode(w: u32, h: u32, freq: u32, bpp: u32, flags: windows::Win32::Graphics::Gdi::CDS_TYPE) -> i32 {
    unsafe {
        let mut dm: DEVMODEW = std::mem::zeroed();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        dm.dmPelsWidth = w;
        dm.dmPelsHeight = h;
        dm.dmDisplayFrequency = freq;
        dm.dmBitsPerPel = bpp;
        dm.dmFields = DEVMODE_FIELD_FLAGS(
            DM_PELSWIDTH.0 | DM_PELSHEIGHT.0 | DM_DISPLAYFREQUENCY.0 | DM_BITSPERPEL.0,
        );
        let r = ChangeDisplaySettingsW(Some(&dm), flags);
        if r == DISP_CHANGE_SUCCESSFUL { 0 } else { r.0 }
    }
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

// ——————————————————— MP4 判据（盒索引，禁裸搜）—————————————————————

/// 顶层盒索引：seek 流式扫描（★ 不整文件读内存）。返回 (类型, 起始偏移, 盒总长)。
///
/// 64 位盒长（`size==1` + largesize）下 largesize **含 16 字节头**，`end = off + largesize`
/// （曾多加 16 越过 moov 头读垃圾，误报"缺 moov"）。
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

/// moov 盒内容（偏移由顶部盒索引走出，★ 绝不裸搜 `"moov"` —— mdat 里会假命中）。
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

/// 视频轨样本数（`stsz`）。★ `mvhd` 报的是最长轨，音频会掩盖视频截断 ⇒ 必须单独数。
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
