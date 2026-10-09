//! GIF 动图解码：为悬浮标「图片」类型提供逐帧位图。
//!
//! 用 `gif` crate（RGBA 输出）逐帧解码并**合成到全局画布**上：正确处理子矩形帧、
//! 处置方式（Keep / Background / Previous）与 tRNS 透明；每帧按**整幅画布**原样产出
//! （2026-09-22 起不再裁正方形——宽高比交给绘制端按「缩放」整体缩放）。资源上限：单帧画布 ≤ 2048×2048、帧数 ≤ 120、总时长无上限——
//! 超限即停在当前帧数（宠物类 GIF 远够用）。
//!
//! 帧延时以毫秒计（GIF 原生单位是 10ms）；延时 <20ms 的帧按浏览器惯例提为 100ms，
//! 否则会把动画放成快闪。

use std::path::Path;
use std::rc::Rc;

use super::image::RgbaBitmap;

/// 单帧最大画布边长。
const MAX_DIM: u32 = 2048;
/// 最大帧数。
const MAX_FRAMES: usize = 120;

/// 已解码的 GIF 动画：所有帧均为**整幅画布**尺寸（不再裁正方形，见模块头），按 `delays_ms` 循环播放。
///
/// `Clone`：多开悬浮标的每个 png 实例共用同一份解码结果（帧是 `Rc`，克隆只拷句柄）。
#[derive(Clone)]
pub struct GifAnim {
    pub frames: Vec<Rc<RgbaBitmap>>,
    /// 每帧停留毫秒（与 frames 等长）。
    pub delays_ms: Vec<f32>,
    /// 总时长（毫秒）。
    pub total_ms: f32,
}

impl GifAnim {
    /// 当前累计毫秒对应的帧下标。
    pub fn frame_index_at(&self, acc_ms: f32) -> usize {
        let mut t = acc_ms % self.total_ms.max(1.0);
        for (i, d) in self.delays_ms.iter().enumerate() {
            if t < *d {
                return i;
            }
            t -= d;
        }
        self.delays_ms.len().saturating_sub(1)
    }
}

/// 清空画布矩形区域（Background 处置）。
fn clear_rect(canvas: &mut [u8], gw: u32, l: u32, t: u32, fw: u32, fh: u32) {
    let gh = canvas.len() as u32 / (gw * 4);
    for y in t..(t + fh).min(gh) {
        for x in l..(l + fw).min(gw) {
            let i = ((y * gw + x) * 4) as usize;
            canvas[i..i + 4].copy_from_slice(&[0, 0, 0, 0]);
        }
    }
}

/// 把 RGBA 帧按 SourceOver 混合进画布（GIF 帧是二值透明：a=0 保留画布，a>0 覆盖）。
#[allow(clippy::too_many_arguments)]
fn blit(canvas: &mut [u8], gw: u32, gh: u32, l: u32, t: u32, buf: &[u8], fw: u32, fh: u32) {
    for row in 0..fh {
        let cy = t + row;
        if cy >= gh {
            break;
        }
        for col in 0..fw {
            let cxx = l + col;
            if cxx >= gw {
                break;
            }
            let si = ((row * fw + col) * 4) as usize;
            let di = ((cy * gw + cxx) * 4) as usize;
            if si + 4 > buf.len() || di + 4 > canvas.len() {
                continue;
            }
            let a = buf[si + 3];
            if a > 0 {
                canvas[di..di + 4].copy_from_slice(&buf[si..si + 4]);
            }
        }
    }
}

/// 解码 GIF 为帧序列。失败返回 None 并写警告日志。
pub fn load_gif_frames(path: &Path) -> Option<GifAnim> {
    let file = std::fs::File::open(path).ok()?;
    let mut options = gif::DecodeOptions::new();
    options.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = match options.read_info(file) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("悬浮标动图 {} 不是有效 GIF：{e}", path.display());
            return None;
        }
    };
    let (gw, gh) = (decoder.width() as u32, decoder.height() as u32);
    if gw == 0 || gh == 0 || gw > MAX_DIM || gh > MAX_DIM {
        log::warn!("悬浮标动图 {} 画布尺寸越界：{}x{}", path.display(), gw, gh);
        return None;
    }

    let mut canvas = vec![0u8; (gw * gh * 4) as usize];
    let mut frames: Vec<Rc<RgbaBitmap>> = Vec::new();
    let mut delays: Vec<f32> = Vec::new();
    let mut prev_snap: Option<Vec<u8>> = None;
    let mut pending_dispose = gif::DisposalMethod::Keep;
    let mut last_rect: Option<(u32, u32, u32, u32)> = None;

    while frames.len() < MAX_FRAMES {
        let frame = match decoder.read_next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                log::warn!(
                    "悬浮标动图 {} 第 {} 帧解码失败：{e}",
                    path.display(),
                    frames.len() + 1
                );
                break;
            }
        };

        // 上一帧的处置：Background 清矩形、Previous 还原快照、Keep 不动。
        match pending_dispose {
            gif::DisposalMethod::Background => {
                if let Some((l, t, fw, fh)) = last_rect {
                    clear_rect(&mut canvas, gw, l, t, fw, fh);
                }
            }
            gif::DisposalMethod::Previous => {
                if let Some(snap) = &prev_snap {
                    canvas.copy_from_slice(snap);
                }
            }
            _ => {}
        }
        prev_snap = if frame.dispose == gif::DisposalMethod::Previous {
            Some(canvas.clone())
        } else {
            None
        };

        blit(
            &mut canvas,
            gw,
            gh,
            frame.left as u32,
            frame.top as u32,
            &frame.buffer,
            frame.width as u32,
            frame.height as u32,
        );

        // ★ 帧按**整幅画布**原样产出（8×4 的动图不会被裁成 4×4）：宽高比由绘制端
        //   按「缩放」倍数整体缩放保持（与 `core::image::load_png` 同一条纪律）。
        let bmp = RgbaBitmap::new(gw, gh, canvas.clone())?;
        frames.push(Rc::new(bmp));
        let mut dms = frame.delay as f32 * 10.0;
        if dms < 20.0 {
            dms = 100.0;
        }
        delays.push(dms);

        pending_dispose = frame.dispose;
        last_rect = Some((
            frame.left as u32,
            frame.top as u32,
            frame.width as u32,
            frame.height as u32,
        ));
    }

    if frames.is_empty() {
        log::warn!("悬浮标动图 {} 没有任何帧", path.display());
        return None;
    }
    let total_ms: f32 = delays.iter().sum();
    Some(GifAnim {
        frames,
        delays_ms: delays,
        total_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 2 帧测试 GIF：第 1 帧纯红、第 2 帧纯蓝（全帧替换，延时 20ms）。
    fn write_two_frame_gif(path: &Path) {
        let (w, h) = (8u16, 8u16);
        let file = std::fs::File::create(path).unwrap();
        let mut enc = gif::Encoder::new(file, w, h, &[255, 0, 0, 0, 0, 255]).unwrap();
        enc.set_repeat(gif::Repeat::Infinite).unwrap();
        let mut f1 = gif::Frame {
            width: w,
            height: h,
            buffer: vec![0u8; (w * h) as usize].into(),
            delay: 2,
            ..Default::default()
        };
        f1.buffer = vec![0u8; (w * h) as usize].into(); // 红
        enc.write_frame(&f1).unwrap();
        f1.buffer = vec![1u8; (w * h) as usize].into(); // 蓝
        enc.write_frame(&f1).unwrap();
    }

    #[test]
    fn two_frame_gif_decodes_with_delays() {
        let dir = std::env::temp_dir().join(format!("curseen_gif_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("two.gif");
        write_two_frame_gif(&p);

        let anim = load_gif_frames(&p).expect("GIF 应可解码");
        assert_eq!(anim.frames.len(), 2);
        assert_eq!(anim.delays_ms.len(), 2);
        assert!(anim.total_ms >= 40.0);
        // 两帧颜色不同（红 vs 蓝），尺寸就是画布原尺寸 8x8（不再裁正方形）
        assert_eq!((anim.frames[0].w, anim.frames[0].h), (8, 8));
        let f0 = &anim.frames[0].rgba;
        let f1 = &anim.frames[1].rgba;
        assert_eq!(&f0[..4], &[255, 0, 0, 255], "第一帧应为红");
        assert_eq!(&f1[..4], &[0, 0, 255, 255], "第二帧应为蓝");
        // 帧索引随累计时间切换
        assert_eq!(anim.frame_index_at(0.0), 0);
        assert_eq!(anim.frame_index_at(10.0), 0);
        assert_eq!(anim.frame_index_at(20.0), 1);
        assert_eq!(anim.frame_index_at(21.0), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ 宽 > 高的动图**不再被裁正方形**：8×4 每帧仍是 8×4（宽高比由绘制端保持）。
    #[test]
    fn wide_gif_keeps_its_own_aspect_ratio() {
        let dir = std::env::temp_dir().join(format!("curseen_gif_wide_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("wide.gif");
        let (w, h) = (8u16, 4u16);
        let file = std::fs::File::create(&p).unwrap();
        let mut enc = gif::Encoder::new(file, w, h, &[255, 0, 0, 0, 0, 255]).unwrap();
        let f = gif::Frame {
            width: w,
            height: h,
            buffer: vec![0u8; (w * h) as usize].into(),
            delay: 2,
            ..Default::default()
        };
        enc.write_frame(&f).unwrap();
        drop(enc);

        let anim = load_gif_frames(&p).expect("宽幅 GIF 应可解码");
        assert_eq!((anim.frames[0].w, anim.frames[0].h), (8, 4), "不该裁成 4×4");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
