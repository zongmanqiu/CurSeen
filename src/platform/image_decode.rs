//! 通用读图（m21）＋ JPEG 编码（m25）：都在**自己的一条 worker** 上按 **WIC** 干活。
//!
//! # 为什么是独立线程，不是"就地解"
//!
//! 调用方是脚本的工作线程（`ext:load_image`）。COM 的公寓是**线程级**的，而一条线程
//! 只能有一种模式：`examples/wic_decode_probe.rs` 实测两种顺序的第二枪都是
//! `RPC_E_CHANGED_MODE`（0x80010106）。脚本线程上先按 MTA 起 WIC，之后它的
//! `ext:open_file`（`file_dialog` 要 STA）就再也初始化不上。所以把解码放到这条 worker：
//! 它 MTA、只做解码；脚本线程一次 COM 都不碰。
//!
//! # 与剪贴板历史的关系
//!
//! 复用**同一条** WIC 链（`clipboard_history::decode_image_with_target`），只是目标宽度传
//! `None`（原尺寸、跳过 scaler）。那条链的四步顺序与"必须过 converter"是实测结论，
//! 抄一份出来给读图用等于让两条路各记一套规则。
//!
//! # 拒绝面（每一条都给中文原因，不静默回 None）
//!
//! · 文件不存在 / 读不到；· 0 字节；· 超过 [`MAX_IMAGE_FILE_BYTES`]（与缩略图**同一个**上限）；
//! · 解出来的尺寸超过 [`MAX_DECODE_PIXELS`]（整张 BGRA 会吃掉 4 字节/像素，必须有头）；
//! · 格式不认识 / 本机缺该格式解码器 / 文件损坏（WIC 的失败一律归到这一句，因为
//!   `CreateDecoderFromFilename` 不区分"没解码器"与"文件坏了"——别假装分得清）。

use std::path::Path;
use std::sync::mpsc::Sender;
use std::sync::OnceLock;

use windows::core::HSTRING;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};

use super::clipboard_history::{decode_image_with_target, ComGuard, MAX_IMAGE_FILE_BYTES};
use super::screenshot::Shot;

/// 整张解码的像素上限（近似 8K×8K）。超过就**拒**：BGRA 是 4 字节/像素，
/// 1 亿像素就是 400 MB 的连续分配，桌面小工具不该在一条后台线程上去赌这个。
pub const MAX_DECODE_PIXELS: u64 = 64 * 1024 * 1024;

/// 交给 worker 的活。两种任务**共用同一条线程**（同一个 MTA 公寓、同一个 WIC 工厂）：
/// 解码是 m21 的读图，编码是 m25 的导出——起第二条线程只会多一个 COM 公寓要管。
enum Job {
    Decode {
        path: std::path::PathBuf,
        reply: Sender<Result<Shot, String>>,
    },
    EncodeJpeg {
        path: std::path::PathBuf,
        shot: Shot,
        /// 1–100（WIC 的 `ImageQuality` 是 0–1 的浮点，这里换算）。
        quality: u8,
        reply: Sender<Result<(), String>>,
    },
}

/// 解码 worker 的投递端（首次使用时起线程）。`OnceLock` + 普通 `mpsc`：
/// 线程**常驻**（WIC 工厂建一次就够），句柄活着就有人收。
fn worker() -> &'static Sender<Job> {
    static TX: OnceLock<Sender<Job>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new()
            .name("fx-image-decode".into())
            .spawn(move || {
                // 先声明后 drop：工厂必须在 `CoUninitialize` 之前析构（与剪贴板历史同一个理由）。
                let _com = ComGuard::new();
                let factory: Option<IWICImagingFactory> = unsafe {
                    CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
                };
                for job in rx {
                    match job {
                        Job::Decode { path, reply } => {
                            let r = match &factory {
                                Some(f) => decode_checked(f, &path),
                                None => Err("图像解码器起不来（WIC 工厂创建失败）".to_string()),
                            };
                            // 调用方可能已经走了（脚本被取消）：发送失败不算错误。
                            let _ = reply.send(r);
                        }
                        Job::EncodeJpeg {
                            path,
                            shot,
                            quality,
                            reply,
                        } => {
                            let r = match &factory {
                                Some(f) => encode_jpeg_on(f, &shot, &path, quality),
                                None => Err("图像解码器起不来（WIC 工厂创建失败）".to_string()),
                            };
                            let _ = reply.send(r);
                        }
                    }
                }
            });
        if let Err(e) = spawned {
            // 不起线程也不 panic：`tx` 的接收端随 `rx` 一起掉，后续 `send` 会失败，
            // 调用方拿到的是"图像解码线程已经退出"——比在脚本线程上抛 panic 诚实。
            log::error!("起图像解码线程失败：{e}");
        }
        tx
    })
}

/// 解一张图，**原尺寸**。阻塞调用方（内部把活交给常驻 worker 并等结果）。
pub fn decode(path: &Path) -> Result<Shot, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let job = Job::Decode {
        path: path.to_path_buf(),
        reply: tx,
    };
    if worker().send(job).is_err() {
        return Err("图像解码线程已经退出，读不了这张图".to_string());
    }
    match rx.recv() {
        Ok(r) => r,
        Err(_) => Err("图像解码没有得到回应（worker 异常退出）".to_string()),
    }
}

/// 把一张位图**编码成 JPEG**（有损）。`quality` = 1–100，越界会被夹到区间内。
///
/// ★ 透明像素**合成到白底**：JPEG 没有 α 通道，直接丢 α 会让"去背景后的图"变成黑框。
///   公式 `c' = a·c + (1−a)·255`（非预乘 BGRA，与 `Shot` 的语义一致）。
pub fn encode_jpeg(shot: &Shot, path: &Path, quality: u8) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let job = Job::EncodeJpeg {
        path: path.to_path_buf(),
        shot: shot.clone(),
        quality: quality.clamp(1, 100),
        reply: tx,
    };
    if worker().send(job).is_err() {
        return Err("图像编码线程已经退出，写不了这张图".to_string());
    }
    match rx.recv() {
        Ok(r) => r,
        Err(_) => Err("图像编码没有得到回应（worker 异常退出）".to_string()),
    }
}

/// WIC 的 JPEG 编码链：`stream → encoder → frame(props) → 24bppBGR → WritePixels → Commit`。
///
/// ★ 请求 24bppBGR 而不是 BGRA：JPEG 本来只存三通道，让 converter 去丢 α 不如我们自己
///   按白底合成一遍（见 [`encode_jpeg`] 的文档）。
fn encode_jpeg_on(
    factory: &IWICImagingFactory,
    shot: &Shot,
    path: &Path,
    quality: u8,
) -> Result<(), String> {
    use windows::core::Interface;
    use windows::core::PWSTR;
    use windows::Win32::Graphics::Imaging::{
        GUID_ContainerFormatJpeg, GUID_WICPixelFormat24bppBGR, IWICBitmapEncoder,
        IWICBitmapFrameEncode, IWICStream, WICBitmapEncoderNoCache,
    };
    use windows::Win32::System::Com::StructuredStorage::{IPropertyBag2, PROPBAG2};
    use windows::Win32::System::Variant::VARIANT;

    let (w, h) = (shot.w.max(0) as u32, shot.h.max(0) as u32);
    if w == 0 || h == 0 {
        return Err("这张图是空的（0 像素），写不出 JPEG".to_string());
    }
    unsafe {
        let stream: IWICStream = factory
            .CreateStream()
            .map_err(|e| format!("建输出流失败：{e}"))?;
        stream
            .InitializeFromFilename(
                &HSTRING::from(path.to_string_lossy().as_ref()),
                GENERIC_WRITE.0,
            )
            .map_err(|e| format!("打开输出文件失败（{}）：{e}", path.display()))?;
        let enc: IWICBitmapEncoder = factory
            .CreateEncoder(&GUID_ContainerFormatJpeg, std::ptr::null())
            .map_err(|e| format!("建 JPEG 编码器失败（本机可能没有 JPEG 编码器）：{e}"))?;
        let istream: windows::Win32::System::Com::IStream = stream
            .cast()
            .map_err(|e| format!("输出流转 IStream 失败：{e}"))?;
        enc.Initialize(&istream, WICBitmapEncoderNoCache)
            .map_err(|e| format!("初始化编码器失败：{e}"))?;

        let mut frame: Option<IWICBitmapFrameEncode> = None;
        let mut props: Option<IPropertyBag2> = None;
        enc.CreateNewFrame(&mut frame, &mut props)
            .map_err(|e| format!("建帧失败：{e}"))?;
        let frame = frame.ok_or_else(|| "编码器没有给帧对象".to_string())?;

        if let Some(p) = &props {
            // 质量写不进去不致命：退回编码器默认值（≈0.9），但要让日志里看得到。
            let mut name: Vec<u16> = "ImageQuality\u{0}".encode_utf16().collect();
            let prop = PROPBAG2 {
                pstrName: PWSTR(name.as_mut_ptr()),
                ..Default::default()
            };
            let var = VARIANT::from(f32::from(quality) / 100.0);
            if let Err(e) = p.Write(1, &prop, &var) {
                log::warn!("JPEG 质量参数没写进去（{e}），用编码器默认值");
            }
            name.clear();
        }
        frame
            .Initialize(props.as_ref())
            .map_err(|e| format!("初始化帧失败：{e}"))?;
        frame
            .SetSize(w, h)
            .map_err(|e| format!("设尺寸失败：{e}"))?;
        let mut fmt = GUID_WICPixelFormat24bppBGR;
        frame
            .SetPixelFormat(&mut fmt)
            .map_err(|e| format!("设像素格式失败：{e}"))?;

        // BGRA → BGR，同时按白底合成 α
        let mut bgr = Vec::with_capacity((w as usize) * (h as usize) * 3);
        for px in shot.bgra.chunks_exact(4) {
            let a = u32::from(px[3]);
            let blend = |c: u8| -> u8 { ((u32::from(c) * a + 255 * (255 - a)) / 255) as u8 };
            // `Shot.bgra` 的字节序是 B,G,R,A ⇒ 按 B、G、R 原样推（写反了就是整图红蓝互换）
            bgr.push(blend(px[0]));
            bgr.push(blend(px[1]));
            bgr.push(blend(px[2]));
        }
        frame
            .WritePixels(h, w * 3, &bgr)
            .map_err(|e| format!("写像素失败：{e}"))?;
        frame.Commit().map_err(|e| format!("提交帧失败：{e}"))?;
        enc.Commit().map_err(|e| format!("提交文件失败：{e}"))?;
    }
    Ok(())
}

fn decode_checked(factory: &IWICImagingFactory, path: &Path) -> Result<Shot, String> {
    let meta =
        std::fs::metadata(path).map_err(|_| format!("文件不存在或读不到：{}", path.display()))?;
    if meta.len() == 0 {
        return Err(format!("这个文件是 0 字节：{}", path.display()));
    }
    if meta.len() > MAX_IMAGE_FILE_BYTES {
        return Err(format!(
            "文件太大（{} MB）：上限 {} MB，与剪贴板缩略图同一个口径",
            meta.len() / (1024 * 1024),
            MAX_IMAGE_FILE_BYTES / (1024 * 1024)
        ));
    }
    // 先读尺寸再解：像素上限必须在**分配整幅缓冲之前**判，不能等 OOM 了再说。
    let size = frame_size(factory, path);
    if let Some((w, h)) = size {
        let px = (w as u64) * (h as u64);
        if px > MAX_DECODE_PIXELS {
            return Err(format!(
                "这张图 {w}×{h}（{:.1} 百万像素），超过整张解码上限 {} 百万像素——先缩再读，别整张解",
                px as f64 / 1_000_000.0,
                MAX_DECODE_PIXELS / 1_000_000
            ));
        }
    }
    let p = path.to_string_lossy();
    decode_image_with_target(factory, &p, None).ok_or_else(|| {
        format!(
            "解不了这个文件：{}（格式不认识 / 本机缺该格式的解码器 / 文件损坏——WIC 这三种只报同一个失败）",
            path.display()
        )
    })
}

/// 只读宽高（不解像素）。读不到就返回 `None`，让后面的解码去报具体原因。
fn frame_size(factory: &IWICImagingFactory, path: &Path) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::WICDecodeMetadataCacheOnLoad;
    unsafe {
        let dec = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path.to_string_lossy().as_ref()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnLoad,
            )
            .ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        Some((w, h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::screenshot;

    /// 造一张 PNG 写到临时目录（用宿主自己的编码器，与产品同一条链）。
    fn write_png(w: u32, h: u32, rgba: &[u8]) -> std::path::PathBuf {
        let bmp = crate::core::image::RgbaBitmap::new(w, h, rgba.to_vec()).expect("位图");
        let dir = std::env::temp_dir().join(format!("curseen_decode_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("probe_{w}x{h}.png"));
        bmp_save(&bmp, &path);
        path
    }

    fn bmp_save(bmp: &crate::core::image::RgbaBitmap, path: &Path) {
        let shot = crate::core::backdrop::bitmap_to_shot(bmp).expect("转帧");
        screenshot::save_png(&shot, path).expect("写 PNG");
    }

    /// ★ 超过 720 宽必须**原尺寸**回来。这条就是本轮的红证靶：把目标宽度写回
    /// `Some(720)`（缩略图那条口径）时，1200×800 会读成 720×480，而**小样本永远绿**
    /// ——所以同一条判据要喂两格（这一格与下一格）。
    #[test]
    fn decodes_above_720_at_original_size() {
        let path = write_png(1200, 800, &vec![200u8; 1200 * 800 * 4]);
        let shot = decode(&path).expect("该解得开");
        assert_eq!(
            (shot.w, shot.h),
            (1200, 800),
            "必须原尺寸，不许走缩略图口径"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 同一条判据的**另一半**：≤720 宽的图本来就不会被缩，所以这一格在小样本上也是绿的
    /// ——它证明的不是"没有缩小"，而是"这两格一起才表达得了'原尺寸'"。
    #[test]
    fn decodes_small_images_at_original_size_too() {
        let path = write_png(640, 400, &vec![10u8; 640 * 400 * 4]);
        let shot = decode(&path).expect("该解得开");
        assert_eq!((shot.w, shot.h), (640, 400));
        let _ = std::fs::remove_file(&path);
    }

    /// α 必须原样：WIC 走的是 `32bppBGRA`（非预乘），读回来透明像素还是 0。
    /// 红证靶：把它换成截屏那档 `shot_to_bitmap`（α 一律刷 255）。
    #[test]
    fn decoded_alpha_is_preserved() {
        let mut rgba = vec![0u8; 2 * 4];
        rgba[..4].copy_from_slice(&[255, 0, 0, 0]); // 全透明红
        rgba[4..].copy_from_slice(&[0, 255, 0, 255]);
        let path = write_png(2, 1, &rgba);
        let shot = decode(&path).expect("该解得开");
        let bmp = crate::core::backdrop::shot_to_bitmap_alpha(&shot).expect("转位图");
        assert_eq!(&bmp.rgba[0..4], &[255, 0, 0, 0], "透明像素的 α 必须还是 0");
        let _ = std::fs::remove_file(&path);
    }

    /// ★ m25 往返：编码 jpg → 用**同一个解码器**读回来。尺寸必须一模一样，
    /// 纯色像素每通道差 ≤ 4（有损允许小误差）——这条同时证明"写出来的确实是能被读的 jpg"。
    #[test]
    fn jpeg_roundtrip_keeps_size_and_close_colors() {
        let bmp =
            crate::core::image::RgbaBitmap::new(8, 8, [200, 30, 40, 255].repeat(64)).expect("位图");
        let shot = crate::core::backdrop::bitmap_to_shot(&bmp).expect("转帧");
        let path = std::env::temp_dir().join(format!("curseen_jpeg_rt_{}.jpg", std::process::id()));
        encode_jpeg(&shot, &path, 90).expect("编码");
        let bytes = std::fs::read(&path).expect("读回文件");
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "JPEG 必须从 SOI (FF D8) 开始");
        let back = decode(&path).expect("解码");
        let _ = std::fs::remove_file(&path);
        assert_eq!((back.w, back.h), (8, 8), "尺寸必须一致");
        let i = ((4 * 8) + 4) * 4;
        let px = &back.bgra[i..i + 4]; // BGRA
        let near = |got: u8, want: u8| (i32::from(got) - i32::from(want)).abs() <= 4;
        assert!(
            near(px[2], 200) && near(px[1], 30) && near(px[0], 40),
            "纯色经 jpg 往返后应基本不变，实际 BGR = {:?}",
            &px[..3]
        );
    }

    /// ★ 透明像素**合成到白底**（不是变黑、也不是保留 α 的假象）：α=0 红 → 白；
    /// α=128 红 → R 仍 255、G/B ≈ 127。
    /// ★ 透明像素**合成到白底**（不是变黑、也不是保留 α 的假象）：α=0 红 → 白；
    /// α=128 红 → R 仍 255、G/B ≈ 127。
    ///
    /// ★ 样本要**离边界足够远**：JPEG 是 8×8 DCT + 色度子采样，白/红交界处的振铃会把
    /// 邻近像素染色（实测 3×1 读到 [240,241,255]、6×6 读到 [255,237,236]）。这不是缺陷，
    /// 是 jpeg 本身 ⇒ 判据取**区域内部**的像素（隔边界十来像素），否则测的是振铃不是合成。
    #[test]
    fn jpeg_flattens_transparency_onto_white() {
        let mut rgba = Vec::new();
        for _row in 0..32 {
            for col in 0..32 {
                let a = match col {
                    0..=15 => 0,
                    16..=23 => 255,
                    _ => 128,
                };
                rgba.extend_from_slice(&[255, 0, 0, a]);
            }
        }
        let bmp = crate::core::image::RgbaBitmap::new(32, 32, rgba).expect("位图");
        let shot = crate::core::backdrop::bitmap_to_shot(&bmp).expect("转帧");
        let path =
            std::env::temp_dir().join(format!("curseen_jpeg_flat_{}.jpg", std::process::id()));
        encode_jpeg(&shot, &path, 95).expect("编码");
        let back = decode(&path).expect("解码");
        let _ = std::fs::remove_file(&path);
        // 取第 16 行（纵向离上下边界 16 像素）、横向离最近边界 ≥12 像素的两个点
        let px = |x: usize| {
            let i = (16 * 32 + x) * 4;
            [back.bgra[i + 2], back.bgra[i + 1], back.bgra[i]] // RGB
        };
        let clear = px(2);
        assert!(
            clear[0] >= 250 && clear[1] >= 250 && clear[2] >= 250,
            "全透明应是白底：{clear:?}"
        );
        let half = px(29);
        assert!(half[0] >= 245, "半透明红的 R 仍该接近 255：{half:?}");
        assert!(
            (110..=145).contains(&half[1]),
            "半透明红在白底上 G/B 应≈127：{half:?}"
        );
    }

    /// 三种拒绝面各给一句人话（安静地回 None 是最坏的一种）。
    #[test]
    fn refusals_carry_human_reasons() {
        let missing = std::env::temp_dir().join("curseen_decode_missing_xyz.png");
        let e = decode(&missing).unwrap_err();
        assert!(e.contains("不存在") || e.contains("读不到"), "{e}");

        let empty = std::env::temp_dir().join("curseen_decode_empty.png");
        std::fs::write(&empty, b"").unwrap();
        let e = decode(&empty).unwrap_err();
        assert!(e.contains("0 字节"), "{e}");
        let _ = std::fs::remove_file(&empty);

        let junk = std::env::temp_dir().join("curseen_decode_junk.png");
        std::fs::write(&junk, b"not a png at all").unwrap();
        let e = decode(&junk).unwrap_err();
        assert!(e.contains("解不了"), "格式坏掉要给一句人话：{e}");
        let _ = std::fs::remove_file(&junk);
    }
}
