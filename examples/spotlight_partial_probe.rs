//! 聚光灯增量帧探针（**补丁 032** 验收）：整窗路径 vs 增量路径的一帧成本对比。
//!
//! 背景：补丁 031 把聚光灯的"整屏多边形"光栅降到 ~1.5ms@1080p，但成本仍随屏幕
//! 像素数线性放大（4K 实测 ~19ms/帧，超 60fps 预算）。补丁 032 引入增量帧：
//! 暗罩恒定、只有光圈在动 ⇒ 每帧只重画「旧洞 ∪ 新洞」的小块，呈现也只交这块
//! （windui 局部呈现）。本探针走**生产同一套代码**（真实 `Runtime` 的
//! `detect_spotlight_partial` 会自动把指令表换成增量版），对比两条路径的
//! 清屏 / 推进 / 光栅 / 换序四段耗时。
//!
//! 跑法：`cargo run --release --example spotlight_partial_probe`

use std::time::Instant;

use curseen_ui::core::config::Config;
use curseen_ui::core::events::InputEvent;
use curseen_ui::core::geometry::ScreenRect;
use curseen_ui::runtime::render::translate_offset;
use curseen_ui::runtime::Runtime;
use windui::render::RenderTarget;

const FRAMES: usize = 120;

/// 与 windui fork 的 `swizzle_rb_inplace` / `swizzle_region_in_place` 保持一致的
/// 本地副本（那边是私有的）。改那边时这里要跟着改。
fn swizzle_rb_inplace(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

fn swizzle_region_in_place(
    data: &mut [u8],
    stride: usize,
    buf_h: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let bw = (stride / 4) as i32;
    let x0 = x.clamp(0, bw);
    let y0 = y.clamp(0, buf_h);
    let x1 = (x + w).clamp(0, bw);
    let y1 = (y + h).clamp(0, buf_h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for row in y0..y1 {
        let start = row as usize * stride + x0 as usize * 4;
        let end = row as usize * stride + x1 as usize * 4;
        swizzle_rb_inplace(&mut data[start..end]);
    }
}

fn config() -> Config {
    let mut cfg = Config::default();
    cfg.trail.hover_enabled = true;
    cfg.trail.hover_type = "spotlight".to_string();
    cfg.trail.trail_enabled = false;
    cfg.key_display.enabled = false;
    cfg.drawing.enabled = false;
    cfg.virtual_keyboard.enabled = false;
    cfg
}

struct Timing {
    clear: f64,
    advance: f64,
    translate: f64,
    swizzle: f64,
}

impl Timing {
    fn total(&self) -> f64 {
        self.clear + self.advance + self.translate + self.swizzle
    }
}

/// mode = "full"：每帧整窗清屏 + 整窗换序（补丁 032 之前的路径）。
/// mode = "partial"：走生产 detect 路径——增量帧跳过清屏、只换序脏区。
fn measure(partial_mode: bool, w: u32, h: u32) -> Timing {
    let mut rt = Runtime::new(config());
    rt.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: w as i32,
        h: h as i32,
    });
    rt.drawing.set_ui_scale(1.0);

    let mut buf = vec![0_u8; (w * h * 4) as usize];
    let (mut t_clear, mut t_adv, mut t_tr, mut t_sw) = (0f64, 0f64, 0f64, 0f64);
    let mut incremental = 0usize;

    for i in 0..FRAMES {
        let x = 200 + (i as i32 * 12) % (w as i32 - 400);
        let y = 200 + (i as i32 * 7) % (h as i32 - 400);
        rt.feed(InputEvent::MouseMove {
            x,
            y,
            time_ms: (i as u64) * 16,
        });

        let mut tgt = windui::render::DibTarget::new(&mut buf, w, h);
        let mut eng = windui::text::NullTextEngine;
        let mut canvas = tgt.make_canvas(&mut eng, 1.0);

        rt.frame();
        let t = Instant::now();
        rt.render();
        t_adv += t.elapsed().as_secs_f64() * 1000.0;

        let is_partial = partial_mode && rt.partial_frame_dirty().is_some();
        if is_partial {
            incremental += 1;
        } else {
            // 整窗路径：清屏。
            let t = Instant::now();
            canvas.clear_rect(0.0, 0.0, w as f32, h as f32);
            t_clear += t.elapsed().as_secs_f64() * 1000.0;
        }

        let t = Instant::now();
        translate_offset(&rt.draw_list, &mut *canvas, 1.0, 0.0, 0.0);
        t_tr += t.elapsed().as_secs_f64() * 1000.0;

        drop(canvas);
        drop(tgt);

        // 换序：增量帧只换脏区（windui 补丁 032 的行为），整窗帧换全缓冲。
        let t = Instant::now();
        if is_partial {
            let r = rt.partial_frame_dirty().unwrap();
            // overlay 的 to_device：scale=1、dx=dy=0，外扩 1px。
            let (dx, dy, dw, dh) = (
                r.x as i32 - 1,
                r.y as i32 - 1,
                r.w as i32 + 2,
                r.h as i32 + 2,
            );
            swizzle_region_in_place(&mut buf, w as usize * 4, h as i32, dx, dy, dw, dh);
        } else {
            swizzle_rb_inplace(&mut buf);
        }
        t_sw += t.elapsed().as_secs_f64() * 1000.0;
    }

    let n = FRAMES as f64;
    let tm = Timing {
        clear: t_clear / n,
        advance: t_adv / n,
        translate: t_tr / n,
        swizzle: t_sw / n,
    };
    let label = if partial_mode { "增量" } else { "整窗" };
    println!(
        "  {label:<8} 清屏 {:>6.2}  推进 {:>6.2}  光栅 {:>6.2}  换序 {:>6.2}  │ 一帧 {:>6.2} ms │ 60fps 占核 {:>5.1}% │ 增量帧 {incremental}/{FRAMES}",
        tm.clear,
        tm.advance,
        tm.translate,
        tm.swizzle,
        tm.total(),
        tm.total() * 60.0 / 10.0,
    );
    tm
}

fn main() {
    println!("聚光灯：整窗路径 vs 增量路径（{FRAMES} 帧均值，release，生产同一套 detect 代码）");
    println!();

    let _ = measure(false, 1920, 1080);

    for (w, h) in [(1920u32, 1080u32), (2560, 1440), (3840, 2160)] {
        println!("════════ {w}×{h} ════════");
        let full = measure(false, w, h);
        let part = measure(true, w, h);
        println!(
            "  ── 结论：一帧 {:.2} → {:.2} ms（{:.0}%），60fps 占核 {:.1}% → {:.1}%{}",
            full.total(),
            part.total(),
            (1.0 - part.total() / full.total()) * 100.0,
            full.total() * 6.0,
            part.total() * 6.0,
            if full.total() > 16.67 && part.total() <= 16.67 {
                "；整窗超预算 → 增量回到预算内 ✓"
            } else {
                ""
            }
        );
        println!();
    }
}
