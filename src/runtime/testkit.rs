//! 测试脚手架：跨用例共享的构造与事件辅助。
//!
//! 只在 `#[cfg(test)]` 下编译。放这里是为了让各域的测试模块（`frame::tests` 等）
//! 都取到同一份 `drawing_only()` / `dt_tick()` / 鼠标事件构造器，而不是各抄一份。

use super::*;

use crate::core::tick::Tick;

/// `sync_input_gate` 往**全局静态量**（`ZOOM_WHEEL_ARMED` / `MOUSE_ECHO_ARMED`）写值，
/// 而 lib 测试默认
/// 多线程并行 —— 两个用例同时 `sync_input_gate` 就会互相踩，断言偶发失败
/// （"改了没坏的东西"这种假红最耗时间）。凡是要断言这条门状态的用例，先抢这把锁。
pub(crate) static INPUT_GATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn dt_tick(dt: f32) -> Tick {
    Tick {
        raw_ms: dt,
        smooth_ms: dt,
        dt_ms: dt,
        elapsed_ms: dt as f64,
    }
}

/// 一个只开标注的运行时：拖尾/击键回显明确关掉，让"谁在排帧"没有歧义。
pub(crate) fn drawing_only() -> Runtime {
    let mut cfg = Config::default();
    cfg.drawing.enabled = true;
    cfg.key_display.enabled = false;
    let mut rt = Runtime::new(cfg);
    rt.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    });
    rt
}

/// 造一个编辑会话（绕过真实框选：把冻结帧与选区直接塞进去）。
///
/// 走 `start_with_backdrop` 而不是直接改字段：那样才顺带钉住"进编辑态 = 画笔同时
/// 换到 `ShotEdit` 档"，而 `is_drawing()` / `click_through()` / `input_needs_frame()`
/// 三处判据全靠它。
pub(crate) fn enter_edit(rt: &mut Runtime, sel: ScreenRect) {
    let (w, h) = (rt.screen.w as u32, rt.screen.h as u32);
    let frame = Rc::new(RgbaBitmap::new(w, h, vec![255_u8; (w * h * 4) as usize]).expect("白帧"));
    rt.drawing.start_with_backdrop(
        crate::core::backdrop::Backdrop::Frozen(frame.clone()),
        Rect::new(sel.x as f32, sel.y as f32, sel.w as f32, sel.h as f32),
    );
    rt.edit = Some(shot_edit::ShotEdit::new(sel, frame, false));
}

pub(crate) fn mouse_down(x: i32, y: i32) -> InputEvent {
    InputEvent::MouseDown {
        button: MouseButton::Left,
        x,
        y,
        time_ms: 0,
    }
}

pub(crate) fn mouse_move(x: i32, y: i32) -> InputEvent {
    InputEvent::MouseMove { x, y, time_ms: 0 }
}

pub(crate) fn mouse_up(x: i32, y: i32) -> InputEvent {
    InputEvent::MouseUp {
        button: MouseButton::Left,
        x,
        y,
        time_ms: 0,
    }
}

/// 记账用 mock Canvas（**跨测试模块共享**）：`dpi_scale()` 可指定（用来扮演"窗口实际
/// DPI"），并记录所有 `fill_circle` 收到的**逻辑**坐标。
///
/// 存在的理由：DPI 判据必须在**没有真实窗口 / 没有双屏**的前提下注入"两套 DPI 分歧"
/// （见 `runtime::render::dpi_offset_tests` 与 `runtime::overlay::tests`），而 `Canvas`
/// 正是 `dpi_scale()` 的宿主。类型一律写全限定路径，避免与 `super::*` 的导入撞名。
pub(crate) struct RecordingCanvas {
    pub actual_dpi: f32,
    pub fill_circles: Vec<(f32, f32, f32)>, // (cx, cy, r)，逻辑像素
}

impl RecordingCanvas {
    pub(crate) fn new(actual_dpi: f32) -> Self {
        Self {
            actual_dpi,
            fill_circles: Vec::new(),
        }
    }
}

impl windui::render::Canvas for RecordingCanvas {
    fn dpi_scale(&self) -> f32 {
        self.actual_dpi
    }
    fn fill_rect(&mut self, _x: f32, _y: f32, _w: f32, _h: f32, _p: &windui::render::Paint) {}
    fn fill_round_rect(
        &mut self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _r: f32,
        _p: &windui::render::Paint,
    ) {
    }
    fn stroke_round_rect(
        &mut self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _r: f32,
        _wd: f32,
        _p: &windui::render::Paint,
    ) {
    }
    fn draw_line(
        &mut self,
        _x0: f32,
        _y0: f32,
        _x1: f32,
        _y1: f32,
        _w: f32,
        _p: &windui::render::Paint,
    ) {
    }
    fn fill_circle(&mut self, cx: f32, cy: f32, r: f32, _p: &windui::render::Paint) {
        self.fill_circles.push((cx, cy, r));
    }
    fn draw_shadow(
        &mut self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _r: f32,
        _b: f32,
        _c: windui::geometry::Color,
    ) {
    }
    fn draw_image(
        &mut self,
        _img: &windui::render::image::Image,
        _dst: windui::geometry::Rect,
        _fit: windui::render::image::Fit,
        _r: f32,
        _o: f32,
    ) {
    }
    fn draw_text(
        &mut self,
        _t: &str,
        _rect: windui::geometry::Rect,
        _c: windui::geometry::Color,
        _a: windui::spec::Align,
        _ts: &windui::text::TextStyle,
    ) {
    }
    fn measure_text(&mut self, _t: &str, _ts: &windui::text::TextStyle) -> windui::geometry::Size {
        windui::geometry::Size::ZERO
    }
    fn push_layer(&mut self, _o: f32) {}
    fn pop_layer(&mut self) {}
    fn save(&mut self) {}
    fn restore(&mut self) {}
    fn clip_rect(&mut self, _r: windui::geometry::Rect) {}
}
