//! 帧成本探针：把覆盖层**一帧**的 CPU 成本拆成四段，量出真实毫秒数。
//!
//! 背景（2026-09-20 用户报"鼠标拖尾/悬浮标有时候卡顿，CPU 从 0 跳到 8%"）：
//! 分层窗后端（`windui::platform::win32::layered`）**恒整窗呈现** ——
//! ① 宿主要清整屏；② 要把 DrawList 光栅到整屏 DIB；③ 要就地 R/B 全缓冲交换；
//! ④ 要 `UpdateLayeredWindow` 把整块位图交给 DWM。
//! 四段加起来就是一帧的固定成本，与"屏幕上实际变了多少"无关。
//!
//! 本探针走**生产同一套代码**：真实 `Runtime` → 真实 `render()` → 真实
//! `translate_offset` → 真实 `DibTarget`，不复制一份近似实现。
//!
//! 多分辨率遍历版：验证"聚光灯成本随屏幕像素数线性放大"这一假设——
//! 这正是"只开聚光灯依然卡顿"在更高分辨率下的根因，也是 GPU 真正能帮上忙的点。
//!
//! 输出每段 ms、一帧合计 ms，以及"按 60fps 折算占满一个核的百分比"——
//! 那个百分比就是用户在任务管理器里看到的东西。
//!
//! 跑法：`cargo run --release --example frame_cost_probe`

use std::time::Instant;

use curseen_ui::core::config::Config;
use curseen_ui::core::events::InputEvent;
use curseen_ui::core::geometry::ScreenRect;
use curseen_ui::runtime::render::translate_offset;
use curseen_ui::runtime::Runtime;
use windui::render::RenderTarget;

/// 每个场景的采样帧数（帧内各自推进真实模块状态）。
const FRAMES: usize = 120;

/// 就地 R/B 交换。`windui::platform::win32::layered::swizzle_rb_inplace` 是私有的
/// （`pub(super)` 都不给），这里照抄同一份逻辑 —— 它只有四行，且**必须**与那里一致，
/// 否则量出来的第 ③ 段是假的。改那边时这里要跟着改。
fn swizzle_rb_inplace(data: &mut [u8]) {
    for px in data.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

/// 一个场景跑完的计时结果（毫秒，已除以帧数）。
struct Timing {
    clear: f64,
    /// `Runtime::frame()` + `Runtime::render()`（模块推进 + 填 DrawList）。
    advance: f64,
    /// `translate_offset`：DrawList → tiny-skia 光栅。
    translate: f64,
    /// 全缓冲 R/B 交换。
    swizzle: f64,
}

impl Timing {
    fn total(&self) -> f64 {
        self.clear + self.advance + self.translate + self.swizzle
    }
    /// 按 target_fps 折算：一帧成本 × 帧率 / 10ms = 单个核的占用百分比。
    fn core_pct(&self, fps: f64) -> f64 {
        self.total() * fps / 10.0
    }
}

/// 造一个接近用户现状的配置：拖尾开 + 悬浮标开，其余全关。
fn config_for(hover_type: &str, hover_on: bool, trail_on: bool) -> Config {
    let mut cfg = Config::default();
    cfg.trail.hover_enabled = hover_on;
    cfg.trail.hover_type = hover_type.to_string();
    cfg.trail.trail_enabled = trail_on;
    // 用户当前用的就是 pixie_dust（间隔类粒子，spawn 100ms / density 5）。
    cfg.trail.trail_effect = "pixie_dust".to_string();
    cfg.trail.trail_settings.density = 5;
    cfg.trail.trail_settings.spawn_interval_ms = 100;
    cfg.key_display.enabled = false;
    cfg.drawing.enabled = false;
    cfg.virtual_keyboard.enabled = false;
    cfg
}

/// 跑一个场景：每帧喂一个移动中的鼠标点，然后走完整条绘制链。
fn measure(name: &str, cfg: Config, w: u32, h: u32) -> Timing {
    let mut rt = Runtime::new(cfg);
    rt.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: w as i32,
        h: h as i32,
    });
    rt.drawing.set_ui_scale(1.0);

    // 整屏缓冲（生产里就是分层窗后端的那块 DIB）。
    let mut buf = vec![0_u8; (w * h * 4) as usize];

    let (mut t_clear, mut t_adv, mut t_tr, mut t_sw) = (0f64, 0f64, 0f64, 0f64);
    let mut cmds = 0usize;

    for i in 0..FRAMES {
        // 一条缓慢的斜向路径（约 480 px/s @60fps），保证粒子按间隔持续生成、
        // 悬浮标每帧都在动 —— 与用户"拖着鼠标划过屏幕"的负载一致。
        let x = 200 + (i as i32 * 8) % 1400;
        let y = 300 + (i as i32 * 4) % 500;
        rt.feed(InputEvent::MouseMove {
            x,
            y,
            time_ms: (i as u64) * 16,
        });

        let mut tgt = windui::render::DibTarget::new(&mut buf, w, h);
        let mut eng = windui::text::NullTextEngine;
        let mut canvas = tgt.make_canvas(&mut eng, 1.0);

        // ① 清屏（`OverlayCanvas::paint` 的第一句）
        let t = Instant::now();
        canvas.clear_rect(0.0, 0.0, w as f32, h as f32);
        t_clear += t.elapsed().as_secs_f64() * 1000.0;

        // ② 模块推进 + 填 DrawList
        let t = Instant::now();
        rt.frame();
        rt.render();
        t_adv += t.elapsed().as_secs_f64() * 1000.0;

        // ③ DrawList → 像素
        let t = Instant::now();
        translate_offset(&rt.draw_list, &mut *canvas, 1.0, 0.0, 0.0);
        t_tr += t.elapsed().as_secs_f64() * 1000.0;
        cmds = rt.draw_list.cmds().len();

        // 与真机一致：③ 的光栅与 ①④ 是同一块缓冲，故先渲完再交换。
        drop(canvas);
        drop(tgt);

        // ④ 全缓冲 R/B 交换（分层窗后端每帧都做整块）
        let t = Instant::now();
        swizzle_rb_inplace(&mut buf);
        t_sw += t.elapsed().as_secs_f64() * 1000.0;
    }

    let n = FRAMES as f64;
    let tm = Timing {
        clear: t_clear / n,
        advance: t_adv / n,
        translate: t_tr / n,
        swizzle: t_sw / n,
    };
    println!(
        "  {name:<30} 清屏 {:>6.2}  推进 {:>6.2}  光栅 {:>6.2}  换序 {:>6.2}  \
         │ 一帧 {:>6.2} ms │ 60fps 占核 {:>5.1}% │ 指令 {cmds}",
        tm.clear,
        tm.advance,
        tm.translate,
        tm.swizzle,
        tm.total(),
        tm.core_pct(60.0),
    );
    tm
}

fn main() {
    println!("帧成本拆分（{FRAMES} 帧均值，release）");
    println!("按 60fps 折算的「占核」= 一帧总毫秒 × 60 / 10，即单核占用百分比");
    println!("16.67 ms/帧 是 60fps 的预算上限：超过它就掉帧");
    println!();

    // 先热身：首次渲染要建字体/脚本引擎缓存，算进去会让第一个场景虚高。
    let _ = measure(
        "（热身，不计）",
        config_for("spotlight", true, true),
        1920,
        1080,
    );

    for (w, h) in [(1920u32, 1080u32), (2560, 1440), (3840, 2160)] {
        println!("════════ {w}×{h} ════════");
        let spot = measure(
            "拖尾 pixie_dust + 悬浮标 spotlight",
            config_for("spotlight", true, true),
            w,
            h,
        );
        let spot_only = measure(
            "只开悬浮标 spotlight",
            config_for("spotlight", true, false),
            w,
            h,
        );
        let solid = measure(
            "只开悬浮标 solid（小圆点）",
            config_for("solid", true, false),
            w,
            h,
        );
        let off = measure(
            "全关（只有清屏+换序的地板）",
            config_for("spotlight", false, false),
            w,
            h,
        );

        println!();
        println!("  ── {w}×{h} 结论 ──");
        println!(
            "  ① 一帧固定地板（清屏+换序，与画什么无关）：{:.2} ms",
            off.clear + off.swizzle
        );
        println!(
            "     其中全屏 R/B 交换 {:.2} ms —— 分层窗恒整窗呈现的必然代价",
            off.swizzle
        );
        println!(
            "  ② spotlight 的光栅成本：{:.2} ms（solid 小圆点只要 {:.2} ms）→ 全屏压暗矩形是最大单项",
            spot_only.translate, solid.translate
        );
        println!(
            "  ③ 当前配置（拖尾+spotlight）一帧 {:.2} ms，60fps 下占单核 {:.1}%，余量 {:.2} ms",
            spot.total(),
            spot.core_pct(60.0),
            16.67 - spot.total()
        );
        let over = spot.total() - 16.67;
        if over > 0.0 {
            println!("     ⚠ 已超过 60fps 预算 {over:.2} ms —— 此分辨率下必然掉帧/卡顿");
        } else {
            println!("     ✓ 未超预算；但光栅随像素数放大，更高分辨率会越界");
        }
        println!();
    }
}
