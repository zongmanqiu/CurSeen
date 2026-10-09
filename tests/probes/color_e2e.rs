//! # 颜色端到端取证探针（`color_e2e`）—— 2026-10-06
//!
//! ## 它回答一个问题
//!
//! 用户报告录屏产物**整体偏蓝**。链路上游（BGRA→NV12 的 shader）已被
//! `yuv_probe` 用独立参考值钉死（容差 1.5，全绿）——所以偏色只能发生在
//! **编码 / 色彩声明 / 解码解释**这三层里。本探针把这三层用**真生产代码**
//! 一次跑通并逐像素比对：
//!
//! ```text
//! 已知纯色 NV12（CPU 按标准解析式填，不经 shader）
//!   → 生产 create_sink_writer（真硬编 + 真色彩声明）
//!   → 产出 mp4（打 colr 盒）
//!   → IMFSourceReader 解回 RGB32（声明感知的"参考播放器"）
//!   → 每色取中心 16×16 均值，与源 RGB 比对
//! ```
//!
//! ## 判据
//!
//! 六个纯色（红/绿/蓝/灰/白/黑）解回 RGB 的每通道误差 ≤ 3（H.264 对纯色块
//! 近乎无损，4:2:0 在整帧同色时无采样损失）。
//!
//! - **全绿** ⇒ 数据与声明自洽，MF 按声明解是对的 ⇒ 用户播放器的偏色来自
//!   "不认声明、按 709 limited 默认猜"的那类播放器 ⇒ 修复方向是把输出换成
//!   **最通用的 BT.709 limited**（HD 视频的事实标准，认不认声明的播放器都按它解）。
//! - **红** ⇒ 编码/声明层真有错，误差表会直接指出是哪一层（通道互换 / 矩阵 / 范围）。
//!
//! ## 跑法（不进日常编译）
//!
//! ```text
//! python build/probe.py run color_e2e
//! ```

#![cfg(windows)]

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

use curseen_ui::platform::recorder::{create_sink_writer, sample_from_texture, Hardware};

/// 640×360：与 `mft_probe` 同尺寸，硬编 MFT 无最小尺寸疑虑。
const W: u32 = 640;
const H: u32 = 360;
const FPS: u32 = 30;
/// 每色帧数：15 帧 = 0.5s，解码侧按时间戳分桶。
const FRAMES_PER_COLOR: usize = 15;

/// 测试色：(名称, R, G, B)。
const COLORS: [(&str, u8, u8, u8); 6] = [
    ("纯红", 255, 0, 0),
    ("纯绿", 0, 255, 0),
    ("纯蓝", 0, 0, 255),
    ("灰128", 128, 128, 128),
    ("纯白", 255, 255, 255),
    ("纯黑", 0, 0, 0),
];

// ── 独立参考值：BT.709 **limited range** 标准解析式（独立推导，非抄 shader）──
// 2026-10-06 二次修正：full range 数据自洽但播放器不认声明照样解错（偏蓝），
// 改为 HD 事实标准 limited（见 `yuv_convert.rs` 模块头「色彩范围」）。
fn ref_y(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    16.0 + (0.2126 * r + 0.7152 * g + 0.0722 * b) * 219.0 / 255.0
}
fn ref_u(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    128.0 + (-0.114572 * r - 0.385428 * g + 0.5 * b) * 224.0 / 255.0
}
fn ref_v(r: u8, g: u8, b: u8) -> f32 {
    let (r, g, b) = (r as f32, g as f32, b as f32);
    128.0 + (0.5 * r - 0.454153 * g - 0.045847 * b) * 224.0 / 255.0
}

#[test]
#[ignore = "颜色端到端取证：起真硬编编码器 + 真解码器；跑法见文件头"]
fn color_e2e_roundtrip() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        // MF 平台只起一次，进程退出自动收，无需配对 Shutdown（探针生命周期短）。
        MFStartup(MF_VERSION, MFSTARTUP_LITE).ok();

        let out = std::path::PathBuf::from("../temp/color_e2e.mp4");
        let _ = std::fs::remove_file(&out);
        let _ = std::fs::create_dir_all(out.parent().unwrap());

        // ── ① 编码侧设备 + D3D 管理器（与生产 Session 同构：这台设备交给 MF）──
        let (device, context) = make_device();
        let mut token = 0u32;
        let mut mgr_opt: Option<IMFDXGIDeviceManager> = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr_opt)
            .expect("MFCreateDXGIDeviceManager 失败");
        let mgr = mgr_opt.expect("MFCreateDXGIDeviceManager 无返回");
        let unk: windows::core::IUnknown = device.cast().expect("设备转 IUnknown 失败");
        mgr.ResetDevice(&unk, token).expect("ResetDevice 失败");

        // ── ② 生产同一套建链（含生产色彩声明）──
        let (writer, used_hw) = create_sink_writer(
            &out, FPS, 8_000, W, H, false, Hardware::Auto, &mgr,
        )
        .expect("create_sink_writer 失败");
        println!(
            "编码链就绪（{}），{}×{} @{}fps，{} 色 × {} 帧",
            if used_hw { "硬件编码" } else { "软件编码" },
            W, H, FPS,
            COLORS.len(),
            FRAMES_PER_COLOR
        );

        // ── ③ 逐色填 NV12 → 编码（每帧新样本，不复用同一 IMFSample）──
        for (ci, (_, r, g, b)) in COLORS.iter().enumerate() {
            let tex = make_nv12_texture(&device);
            fill_nv12(&context, &tex, *r, *g, *b);
            for f in 0..FRAMES_PER_COLOR {
                let n = (ci * FRAMES_PER_COLOR + f) as i64;
                let sample = sample_from_texture(&tex).expect("打包 NV12 样本失败");
                sample
                    .SetSampleTime(n * 10_000_000 / FPS as i64)
                    .expect("SetSampleTime 失败");
                sample
                    .SetSampleDuration(10_000_000 / FPS as i64)
                    .expect("SetSampleDuration 失败");
                writer.WriteSample(0, &sample).expect("WriteSample 失败");
            }
        }
        writer.Finalize().expect("Finalize 失败");
        // 与生产一致：容器 colr 盒也钉成声明的值。
        curseen_ui::platform::mp4_concat::patch_colr_709_limited(&out);
        println!(
            "编码完成：{}（{:.1} KB）",
            out.display(),
            std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) as f64 / 1024.0
        );

        // ── ④a 先用原生 NV12 解码：分清"编码就错了"还是"RGB32 转换错了"──
        let mut wide: Vec<u16> = out.as_os_str().to_string_lossy().encode_utf16().collect();
        wide.push(0);
        // 解码属性：打开高级视频处理（NV12→RGB32 的颜色转换要靠它插视频处理器；
        // 实测不加时 SetCurrentMediaType(RGB32) 报 0xC00D36B4）。
        let rattr = MFCreateMediaType().expect("建 reader 属性失败");
        rattr
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .expect("开高级视频处理失败");
        let reader = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &rattr)
            .expect("建 SourceReader 失败");

        let ty_nv12 = MFCreateMediaType().expect("建媒体类型失败");
        ty_nv12
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .expect("MAJOR_TYPE 失败");
        ty_nv12
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .expect("SUBTYPE 失败");
        reader
            .SetCurrentMediaType(0, None, &ty_nv12)
            .expect("SetCurrentMediaType(NV12) 失败");
        println!("\n──── 原生 NV12 解码（中心点 Y/U/V，顺序读帧 0..5）────");
        for k in 0..COLORS.len() {
            let mut s: Option<IMFSample> = None;
            let mut flags = 0u32;
            reader
                .ReadSample(0, 0, None, Some(&mut flags), None, Some(&mut s))
                .expect("ReadSample 失败");
            let Some(s) = s else { break };
            let ts = s.GetSampleTime().unwrap_or(-1);
            let frame = (ts.max(0) as usize) * FPS as usize / 10_000_000;
            let (y, u, v) = center_yuv(&s);
            let want = frame / FRAMES_PER_COLOR;
            let (er, eg, eb) = (
                COLORS[want.min(COLORS.len() - 1)].1,
                COLORS[want.min(COLORS.len() - 1)].2,
                COLORS[want.min(COLORS.len() - 1)].3,
            );
            let (ey, eu, ev) = (
                ref_y(er, eg, eb).round() as u8,
                ref_u(er, eg, eb).round() as u8,
                ref_v(er, eg, eb).round() as u8,
            );
            println!(
                "  帧{frame}（期望色 {}）：Y={y} U={u} V={v} ｜ 期望 Y={ey} U={eu} V={ev}",
                COLORS[want.min(COLORS.len() - 1)].0
            );
        }
        // ── ④b RGB32 判据：另起一个 SourceReader（免 seek，PROPVARIANT 太绕）──
        let reader_rgb = MFCreateSourceReaderFromURL(PCWSTR(wide.as_ptr()), &rattr)
            .expect("建 RGB32 SourceReader 失败");
        let ty = MFCreateMediaType().expect("建媒体类型失败");
        ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .expect("MAJOR_TYPE 失败");
        ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .expect("SUBTYPE 失败");
        // ★ MF_MT_FRAME_SIZE 打包口径：**高 32 位 = 宽、低 32 位 = 高**（写反了
        //   视频处理器会按 360×640 出图再信箱缩放，画面上只剩中间一条色带）。
        ty.SetUINT64(&MF_MT_FRAME_SIZE, ((W as u64) << 32) | H as u64)
            .expect("FRAME_SIZE 失败");
        reader_rgb
            .SetCurrentMediaType(0, None, &ty)
            .expect("SetCurrentMediaType 失败");

        // 每色累计中心 16×16 的均值。
        let mut sums = vec![[0u64; 3]; COLORS.len()];
        let mut counts = vec![0u64; COLORS.len()];
        let mut reads = 0usize;
        let mut dumped = false;
        while reads < 600 {
            reads += 1;
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            reader_rgb
                .ReadSample(0, 0, None, Some(&mut flags), None, Some(&mut sample))
                .expect("ReadSample 失败");
            // 诊断：第一帧做原始 dump，看缓冲里到底有没有颜色数据、长什么样
            if !dumped {
                dumped = true;
                if let Some(s0) = sample.as_ref() {
                    if let Ok(b) = s0.GetBufferByIndex(0) {
                        let mut data = std::ptr::null_mut();
                        let mut len = 0u32;
                        if b.Lock(&mut data, Some(&mut len), None).is_ok() {
                            let slice = std::slice::from_raw_parts(data, len as usize);
                            let nz = slice.iter().filter(|&&x| x != 0).count();
                            println!(
                                "  [dump] 连续口径 len={len} 非零={nz} 首16B={:02x?}",
                                &slice[..16.min(len as usize)]
                            );
                            // 行距 2560 假设下取几个采样点的像素
                            let pitch = W as usize * 4;
                            for (ry, rx) in [(0usize, 0usize), (180, 320), (359, 639)] {
                                let off = ry * pitch + rx * 4;
                                println!(
                                    "  [dump] 像素({rx},{ry}) = {:02x?}",
                                    &slice[off..off + 4]
                                );
                            }
                            let _ = b.Unlock();
                            // 原始落盘供离线分析（640×360×4 = 921600 字节）
                            let _ = std::fs::write("../temp/color_frame0.bin", slice);
                        } else {
                            println!("  [dump] 连续 Lock 失败（纯 2D 缓冲）");
                        }
                        if let Ok(b2) = b.cast::<IMF2DBuffer2>() {
                            let mut p: *mut u8 = std::ptr::null_mut();
                            let mut pitch: i32 = 0;
                            let mut start: *mut u8 = std::ptr::null_mut();
                            let mut blen: u32 = 0;
                            b2.Lock2DSize(
                                MF2DBuffer_LockFlags_Read,
                                &mut p,
                                &mut pitch,
                                &mut start,
                                &mut blen,
                            )
                            .expect("Lock2DSize(诊断) 失败");
                            println!(
                                "  [dump] 2D 口径 pitch={pitch} 扫描行0首4B={:02x?} 中行首4B={:02x?}",
                                std::slice::from_raw_parts(p, 4),
                                std::slice::from_raw_parts(
                                    p.add((H as usize / 2) * pitch.unsigned_abs() as usize),
                                    4
                                )
                            );
                            let _ = b2.Unlock2D();
                        }
                    }
                }
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(s) = sample else { continue };
            let Ok(ts) = s.GetSampleTime() else { continue };
            if ts < 0 {
                continue;
            }
            let frame = ts as usize * FPS as usize / 10_000_000;
            let ci = frame / FRAMES_PER_COLOR;
            if ci >= COLORS.len() {
                continue;
            }
            let (r, g, b) = avg_center_rgb(&s);
            sums[ci][0] += r as u64;
            sums[ci][1] += g as u64;
            sums[ci][2] += b as u64;
            counts[ci] += 1;
        }

        // ── ⑤ 判据：先打印全部实测值（判据纪律：红时先看到被测物），再断言 ──
        println!("\n──── 解码实测（SourceReader 按声明解）────");
        println!("  颜色     源RGB         解码RGB       每通道误差");
        let mut worst = 0i32;
        for (i, (name, r, g, b)) in COLORS.iter().enumerate() {
            let n = counts[i].max(1);
            let dr = (sums[i][0] / n) as i32;
            let dg = (sums[i][1] / n) as i32;
            let db = (sums[i][2] / n) as i32;
            let er = (dr - *r as i32).abs();
            let eg = (dg - *g as i32).abs();
            let eb = (db - *b as i32).abs();
            worst = worst.max(er.max(eg).max(eb));
            println!(
                "  {:<6} ({:3},{:3},{:3})  ({:3},{:3},{:3})   ΔR={er} ΔG={eg} ΔB={eb}  [{}帧]",
                name, r, g, b, dr, dg, db, counts[i]
            );
        }

        // 参考信息：反过来，如果哪天有人把数据换回 full range、或有播放器对
        // limited 数据按 full 解释，同一份 limited 数据会被解成什么样
        //（纯白 → 92% 灰 = 当年的"蒙版"，这就是 limited 才是安全解的反证）。
        println!("\n──── 参考：按 709 full 误解 limited 数据会得到 ───");
        for (name, r, g, b) in COLORS {
            let (y, u, v) = (ref_y(r, g, b), ref_u(r, g, b), ref_v(r, g, b));
            let (rr, gg, bb) = decode_as_full(y, u, v);
            println!("  {name:<6} → ({rr},{gg},{bb})");
        }

        assert!(
            worst <= 3,
            "颜色端到端判据红：最大通道误差 {worst} > 3（见上表定位出错层）"
        );
        println!("\n✅ 端到端颜色判据通过（最大通道误差 {worst} ≤ 3）");
    }
}

/// 模拟"对 limited 数据按 full 解释"的播放器：limited 样本值被当 full 解
/// （纯白会变成 92% 灰 —— 当年"蒙版"问题的逆演出）。
fn decode_as_full(y: f32, u: f32, v: f32) -> (i32, i32, i32) {
    // 先把 limited 样本值当作 full YUV（不做 16-235 还原），按 709 full 还原 RGB
    let r = y + 1.5748 * (v - 128.0);
    let g = y - 0.1873 * (u - 128.0) - 0.4681 * (v - 128.0);
    let b = y + 1.8556 * (u - 128.0);
    (
        r.round().clamp(0.0, 255.0) as i32,
        g.round().clamp(0.0, 255.0) as i32,
        b.round().clamp(0.0, 255.0) as i32,
    )
}

/// 从 NV12 原生缓冲取中心点的 Y / U / V。
unsafe fn center_yuv(sample: &IMFSample) -> (u8, u8, u8) {
    let buf = sample.GetBufferByIndex(0).expect("取解码缓冲失败");
    let mut data = std::ptr::null_mut();
    let mut len = 0u32;
    buf.Lock(&mut data, Some(&mut len), None).expect("Lock 失败");
    // 连续 NV12：Y = w*h，UV = w*h/2（w*h 都按行距连续时的理想布局；
    // 解码器输出的连续缓冲一般行距=宽，红则以 2D 路径重写）。
    let y = *data.add((H as usize / 2) * W as usize + W as usize / 2);
    let uv_off = W as usize * H as usize;
    let uv_row = H as usize / 4;
    let uv_col = W as usize / 4;
    let u = *data.add(uv_off + uv_row * (W as usize) + uv_col * 2);
    let v = *data.add(uv_off + uv_row * (W as usize) + uv_col * 2 + 1);
    let _ = buf.Unlock();
    (y, u, v)
}

/// 取一帧 RGB32 样本中心 16×16 的均值，返回 (R, G, B)。
///
/// ★ 优先走 `IMF2DBuffer2::Lock2DSize` 拿**真实行距**（解码器给的 RGB32 缓冲
///   行距带对齐，按"宽×4"读会错位花屏）；连续缓冲路径只作回退。
unsafe fn avg_center_rgb(sample: &IMFSample) -> (u8, u8, u8) {
    let buf = sample.GetBufferByIndex(0).expect("取解码缓冲失败");

    if let Ok(b2d) = buf.cast::<IMF2DBuffer2>() {
        let mut p: *mut u8 = std::ptr::null_mut();
        let mut pitch: i32 = 0;
        let mut start: *mut u8 = std::ptr::null_mut();
        let mut len: u32 = 0;
        b2d.Lock2DSize(
            MF2DBuffer_LockFlags_Read,
            &mut p,
            &mut pitch,
            &mut start,
            &mut len,
        )
        .expect("Lock2DSize 失败");
        // ★ RGB32 在 MF 里是 **bottom-up**：pitch 为负、scanline0 指向内存首行
        //   （图像最底行）。纯色中心均值与上下翻转无关，取 |pitch| 做行距即可。
        if pitch < 0 {
            println!("  （RGB32 bottom-up，pitch={pitch}）");
        }
        let rgb = avg_from_ptr(p, pitch.unsigned_abs() as usize);
        let _ = b2d.Unlock2D();
        return rgb;
    }

    // 回退：连续缓冲（RGB32 行距 = 宽×4）
    let mut data = std::ptr::null_mut();
    let mut len = 0u32;
    buf.Lock(&mut data, Some(&mut len), None).expect("Lock 失败");
    let rgb = avg_from_ptr(data, W as usize * 4);
    let _ = buf.Unlock();
    rgb
}

/// 从 RGB32（B,G,R,X 字节序）缓冲取中心 16×16 均值。
unsafe fn avg_from_ptr(ptr: *mut u8, pitch: usize) -> (u8, u8, u8) {
    let half = 8usize;
    let cx = W as usize / 2;
    let cy = H as usize / 2;
    let mut sr = 0u64;
    let mut sg = 0u64;
    let mut sb = 0u64;
    for dy in -(half as isize)..half as isize {
        let row = ptr.add(((cy as isize + dy) as usize) * pitch);
        for dx in -(half as isize)..half as isize {
            let p = row.add(((cx as isize + dx) as usize) * 4);
            sb += *p as u64;
            sg += *p.add(1) as u64;
            sr += *p.add(2) as u64;
        }
    }
    let n = (4 * half * half) as u64;
    ((sr / n) as u8, (sg / n) as u8, (sb / n) as u8)
}

/// 建编码侧 D3D11 设备。
unsafe fn make_device() -> (ID3D11Device, ID3D11DeviceContext) {
    let mut dev: Option<ID3D11Device> = None;
    let mut ctx: Option<ID3D11DeviceContext> = None;
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&[D3D_FEATURE_LEVEL_11_0]),
        D3D11_SDK_VERSION,
        Some(&mut dev),
        None,
        Some(&mut ctx),
    )
    .expect("创建 D3D11 设备失败");
    (dev.expect("设备无返回"), ctx.expect("上下文无返回"))
}

/// 建生产同款的 NV12 纹理（三个 bind 缺一不可，见 `yuv_convert.rs` 的注释）。
unsafe fn make_nv12_texture(device: &ID3D11Device) -> ID3D11Texture2D {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_UNORDERED_ACCESS.0
            | D3D11_BIND_SHADER_RESOURCE.0
            | D3D11_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut t: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut t))
        .expect("创建 NV12 纹理失败");
    t.expect("NV12 纹理无返回")
}

/// 把一帧纯色 NV12 写进纹理：staging 填好 → 逐平面 CopySubresourceRegion。
///
/// ★ staging NV12 的平面布局与读回一致：Y 平面从 0 开始、UV 平面从 `H*pitch`
///   字节开始（`yuv_probe` 实测口径）。CopyResource 不支持多平面，必须逐平面。
unsafe fn fill_nv12(context: &ID3D11DeviceContext, tex: &ID3D11Texture2D, r: u8, g: u8, b: u8) {
    let y = ref_y(r, g, b).round().clamp(0.0, 255.0) as u8;
    let u = ref_u(r, g, b).round().clamp(0.0, 255.0) as u8;
    let v = ref_v(r, g, b).round().clamp(0.0, 255.0) as u8;

    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
    };
    let mut s: Option<ID3D11Texture2D> = None;
    let dev = context.GetDevice().expect("取设备失败");
    dev.CreateTexture2D(&staging_desc, None, Some(&mut s))
        .expect("创建 staging 失败");
    let staging = s.expect("staging 无返回");

    let mut m = D3D11_MAPPED_SUBRESOURCE::default();
    context
        .Map(&staging, 0, D3D11_MAP_WRITE, 0, Some(&mut m))
        .expect("映射 staging 失败");
    let pitch = m.RowPitch as usize;
    let base = m.pData as *mut u8;
    // Y 平面：H 行，每行前 W 字节
    for row in 0..H as usize {
        std::ptr::write_bytes(base.add(row * pitch), y, W as usize);
    }
    // UV 平面：起点 = H*pitch，(W/2) 对 ×2 字节 = (U, V)
    let uv_base = base.add(H as usize * pitch);
    for row in 0..H as usize / 2 {
        let p = uv_base.add(row * pitch);
        for col in 0..W as usize / 2 {
            *p.add(col * 2) = u;
            *p.add(col * 2 + 1) = v;
        }
    }
    context.Unmap(&staging, 0);

    // 逐平面拷（CopyResource 对 NV12 静默只拷 Y 平面，见 yuv_probe 实测）
    let src: ID3D11Resource = staging.cast().expect("staging 转型失败");
    let dst: ID3D11Resource = tex.cast().expect("NV12 转型失败");
    context.CopySubresourceRegion(Some(&dst), 0, 0, 0, 0, Some(&src), 0, None);
    context.CopySubresourceRegion(Some(&dst), 1, 0, 0, 0, Some(&src), 1, None);
}
