//! 录制合成通道端到端探针（`rec_pip_share`）—— 2026-10-07。
//!
//! # 它回答一个问题
//!
//! 放大画面**到底有没有进录屏**。链路是：`capture_share` 共享内存（主进程发布
//! 面板位图）→ 录制进程 `Session::pump` 每帧 `composite_pip_frames` 贴进采集
//! 纹理。这条链在录制搬进子进程（2026-10-07 出进程隔离）时被切断过——进程内
//! 静态槽跨不了进程，录屏里放大画面静默消失（用户报告"录出来的画面没有放大"）。
//!
//! 判据必须**解码视频像素**，不能只看通道有帧：贴没贴上、贴的位置对不对，
//! 只有解码产物才知道（通道层面"一切正常"但没贴进纹理的假绿之前出过）。
//!
//! # 跑法
//!
//! ```text
//! python build/probe.py run rec_pip_share            # 子进程载体（生产默认路径）
//! python build/probe.py run rec_pip_share --in-process  # 进程内线程载体
//! ```
//!
//! 本探针扮演"主进程"：直接向 `capture_share` 发布一块**纯红测试面板**（真实
//! 场景里发布者是 `PipWindow::present`），录制 4 秒后解码产物前 3 帧，判据：
//!
//! - 面板矩形内取样点 = 红色（H.264 有损，用容差而非精确 255）；
//! - 面板矩形外取样点**不全**是红色（负对照：桌面内容任意，防"整屏恰好红"假绿）。
//!
//! ★ 面板区域是合成上去的：`WDA_EXCLUDEFROMCAPTURE` 使 DXGI 源帧里**没有**放大窗，
//!   所以"面板区是红的"只可能来自合成通道——这就是本探针的判别力来源。
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use curseen_ui::platform::capture_share::{self, PipFrame};
use curseen_ui::platform::recorder::{self, RecOptions};
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS,
};
use windows::Win32::Media::MediaFoundation::*;

/// 录制时长。
const REC_SECS: u64 = 4;
/// 测试面板尺寸（物理像素）。
const PANEL: (i32, i32) = (320, 180);

#[test]
#[ignore]
fn pip_panel_composited_into_recording() {
    let _ = env_logger::builder().is_test(false).try_init();

    // —— 面板位置：主屏内、避开角上系统 UI ——
    let (px, py, sw, sh) = primary_origin_and_size();
    let panel = (
        px + sw / 4,
        py + sh / 3,
        PANEL.0,
        PANEL.1,
    );
    println!(
        "主屏原点 ({px},{py}) {sw}x{sh}，测试面板 {panel:?}（纯红 BGRA）"
    );

    // —— 发布端：模仿 PipWindow::present（30FPS 连发，顺带压 seqlock 并发）——
    capture_share::set_enabled(true);
    let stop = Arc::new(AtomicBool::new(false));
    let frame = PipFrame {
        x: panel.0,
        y: panel.1,
        w: panel.2,
        h: panel.3,
        bgra: vec![0u8, 0, 255, 255].repeat((panel.2 * panel.3) as usize), // B,G,R,A = 红
    };
    let t_stop = stop.clone();
    let pub_thread = std::thread::Builder::new()
        .name("pip-share-pub".into())
        .spawn(move || {
            while !t_stop.load(Ordering::Relaxed) {
                capture_share::publish(
                    curseen_ui::platform::pip_window::PIP_WINDOW_TITLE,
                    PipFrame {
                        x: frame.x,
                        y: frame.y,
                        w: frame.w,
                        h: frame.h,
                        bgra: frame.bgra.clone(),
                    },
                );
                std::thread::sleep(Duration::from_millis(33));
            }
        })
        .expect("起发布线程失败");

    // —— 录 4 秒 ——
    let out = std::env::temp_dir().join("curseen_pip_share.mp4");
    let _ = std::fs::remove_file(&out);
    let rec = recorder::Recorder::start(opts(&out)).expect("应能起录制");
    let t0 = Instant::now();
    while rec.frames() == 0 && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(rec.frames() > 0, "10 秒没录到第一帧");
    std::thread::sleep(Duration::from_secs(REC_SECS));
    let dropped = rec.dropped();
    let done = rec.finish();
    stop.store(true, Ordering::Relaxed);
    let _ = pub_thread.join();
    capture_share::set_enabled(false);

    assert!(done.error.is_none(), "录制报错：{:?}", done.error);
    assert!(
        done.bytes.unwrap_or(0) > 0,
        "产物 0 字节 —— 合成通道探针没有产物可判"
    );
    println!(
        "录制完成：{} 字节 / 丢弃 {dropped} / 错误 {:?}",
        done.bytes.unwrap_or(0),
        done.error
    );

    // —— 解码前 3 帧，判像素 ——
    //
    // ★ 不按坐标点采样：MF 解 RGB32 的行序（top-down / bottom-up）取决于解码链，
    //   DEFAULT_STRIDE 属性实测缺省——点采样容易采到镜像行上的桌面像素（第一版
    //   就这样假红）。改为全帧扫描"红色块"，两种行序假设各算一个包围盒，只有
    //   一个假设能在面板位置扫出尺寸吻合的纯红块（自校验：桌面内容不会恰好
    //   在镜像位置也有同尺寸纯红块）。
    let frames = decode_first_frames(&out, 3);
    assert_eq!(frames.len(), 3, "解出的帧不足 3（{}）", frames.len());
    // 取证落盘：首帧原始 BGRA 导出，失败时离线分析用。
    let raw = std::env::temp_dir().join("curseen_pipshare_frame0.bgra");
    let _ = std::fs::write(&raw, &frames[0].data);
    let (cx, cy) = (
        panel.0 + panel.2 / 2,
        panel.1 + panel.3 / 2,
    );
    for (i, f) in frames.iter().enumerate() {
        let hit = f
            .red_block_at(panel.0, panel.1, panel.2, panel.3)
            .unwrap_or_else(|| {
                panic!(
                    "帧{i} 在面板 {panel:?} 处没扫到纯红块 —— 放大面板没有合成进录制帧"
                )
            });
        println!(
            "帧{i} 面板区红色块：{hit:?}（期望 {panel:?}，中心 ({cx},{cy})）"
        );
    }

    println!("\n✅ 录制合成：面板位图经共享内存跨进程进入录制帧（前 3 帧面板区均为纯红）");
}

fn opts(path: &PathBuf) -> RecOptions {
    RecOptions {
        kind: recorder::RecKind::Mp4,
        path: path.clone(),
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
    }
}

/// 主屏虚拟桌面原点与物理尺寸（面板矩形要在**虚拟桌面**坐标系里）。
fn primary_origin_and_size() -> (i32, i32, i32, i32) {
    unsafe {
        let mut dm: DEVMODEW = std::mem::zeroed();
        dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
        assert!(
            EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut dm).as_bool(),
            "读主屏设置失败"
        );
        // dmPosition 在 union 里（dmFields 没设位置位时的读取是定义良好的：
        // DEVMODEW 已 zeroed，union 读出全零/真实值均可）。
        let pos = dm.Anonymous1.Anonymous2.dmPosition;
        (pos.x, pos.y, dm.dmPelsWidth as i32, dm.dmPelsHeight as i32)
    }
}

// ————————————————————— 解码（MF SourceReader → RGB32）—————————————————————

struct DecodedFrame {
    w: i32,
    h: i32,
    data: Vec<u8>,
}

/// 红色判定（H.264 有损 + 色度抽样 ⇒ 用容差，不要求精确 255）。
fn is_red(b: u8, g: u8, r: u8) -> bool {
    r > 180 && g < 80 && b < 80
}

impl DecodedFrame {
    /// 期望矩形内"红像素占比"判据（行序双假设自校验）。
    ///
    /// 行序不确定：top-down / bottom-up 两个假设各算一次"期望矩形内的红像素数"。
    /// 真实布局的那个假设里矩形几乎全红（合成面板是纯红）；镜像假设里矩形落在
    /// 镜像位置 = 桌面内容，不可能也是纯红。判据：恰好一个假设 ≥90% 红，
    /// 且另一个 <20%（桌面在两处都恰好红的巧合被排除）。
    ///
    /// （不能用全帧 bbox：桌面自带的零散红点会把包围盒撑大，误判"没红块"。）
    fn red_block_at(&self, px: i32, py: i32, pw: i32, ph: i32) -> Option<(i32, i32, i32, i32)> {
        let top = self.red_in_rect(px, py, pw, ph, false);
        let bu = self.red_in_rect(px, py, pw, ph, true);
        let area = (pw * ph) as f64;
        let (hit_flip, ratio) = match (top as f64 / area, bu as f64 / area) {
            (t, b) if t >= 0.9 && b < 0.2 => (false, t),
            (t, b) if b >= 0.9 && t < 0.2 => (true, b),
            (t, b) => {
                println!("行序自校验失败：top_down 占比 {t:.2} / bottom_up 占比 {b:.2}");
                return None;
            }
        };
        // 命中行序下重算一次精确包围盒（只扫期望矩形，不受桌面杂红污染）。
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, 0, 0);
        let stride = self.w * 4;
        for dy in 0..ph {
            let row = if hit_flip { self.h - 1 - (py + dy) } else { py + dy };
            let base = (row * stride) as usize;
            for dx in 0..pw {
                let off = base + ((px + dx) * 4) as usize;
                let (b, g, r) = (self.data[off], self.data[off + 1], self.data[off + 2]);
                if is_red(b, g, r) {
                    x0 = x0.min(px + dx);
                    y0 = y0.min(py + dy);
                    x1 = x1.max(px + dx);
                    y1 = y1.max(py + dy);
                }
            }
        }
        println!("红占比 {ratio:.3}（行序 {}）", if hit_flip { "bottom-up" } else { "top-down" });
        Some((x0, y0, x1 - x0 + 1, y1 - y0 + 1))
    }

    /// 某行序假设下，期望矩形内的红像素数。
    fn red_in_rect(&self, px: i32, py: i32, pw: i32, ph: i32, flip: bool) -> u64 {
        let stride = self.w * 4;
        let mut cnt = 0u64;
        for dy in 0..ph {
            let row = if flip { self.h - 1 - (py + dy) } else { py + dy };
            let base = (row * stride) as usize;
            for dx in 0..pw {
                let off = base + ((px + dx) * 4) as usize;
                if is_red(self.data[off], self.data[off + 1], self.data[off + 2]) {
                    cnt += 1;
                }
            }
        }
        cnt
    }
}

/// 解码产物前 `n` 个视频帧为 RGB32。
fn decode_first_frames(path: &Path, n: usize) -> Vec<DecodedFrame> {
    unsafe {
        let _mf = MfStartupGuard::new();
        let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
        // ★ H.264 → RGB32 走 SourceReader 内建颜色转换，必须显式开高级视频处理，
        //   否则 SetCurrentMediaType(RGB32) 拒收（MF_E_INVALIDMEDIATYPE 0xC00D36B4）。
        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 2).expect("建 attributes 失败");
        let attrs = attrs.expect("attributes 为空");
        attrs
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .expect("开高级视频处理失败");
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &attrs)
            .expect("建 SourceReader 失败");

        // 定位视频流（轨序不定，按 major type 找）。
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
        let Some(vid) = vid else {
            panic!("产物里没有视频轨");
        };
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .expect("全不选失败");
        reader.SetStreamSelection(vid, true).expect("选视频流失败");

        let ty = MFCreateMediaType().expect("建媒体类型失败");
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).expect("MAJOR 失败");
        ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).expect("SUBTYPE 失败");
        ty.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1).expect("UNCOMP 失败");
        reader
            .SetCurrentMediaType(vid, None, &ty)
            .expect("SetCurrentMediaType(RGB32) 失败");

        let got = reader.GetCurrentMediaType(vid).expect("取实际类型失败");
        let size = got.GetUINT64(&MF_MT_FRAME_SIZE).expect("取帧尺寸失败");
        let w = (size >> 32) as u32 as i32;
        let h = (size & 0xFFFF_FFFF) as u32 as i32;
        // 行序（top-down/bottom-up）不假定——判据用双假设自校验（见 red_block_at）。
        println!("解码格式：{w}x{h}");

        let mut out = Vec::new();
        for _ in 0..n {
            let mut sample: Option<IMFSample> = None;
            let mut flags = 0u32;
            reader
                .ReadSample(vid, 0, None, Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            let Some(sample) = sample else {
                break; // EOF
            };
            let buf = sample.ConvertToContiguousBuffer().expect("转连续缓冲失败");
            let (ptr, len) = {
                let mut p = std::ptr::null_mut::<u8>();
                let mut l = 0u32;
                buf.Lock(&mut p, None, Some(&mut l)).expect("Lock 失败");
                (p, l)
            };
            let data = std::slice::from_raw_parts(ptr, len as usize).to_vec();
            let _ = buf.Unlock();
            out.push(DecodedFrame { w, h, data });
        }
        assert_eq!(out.len(), n, "解码出的帧数不足");
        out
    }
}

/// `MFStartup` 的 RAII 包装（同 audio_e2e）。
struct MfStartupGuard;

impl MfStartupGuard {
    #[allow(clippy::new_without_default)]
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
