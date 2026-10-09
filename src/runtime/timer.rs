//! 计时器（`docs/DESIGN.md` §1 #15 的那一块）。
//!
//! 核心只是一个状态机：快捷键按下即开始，再按一下即停。
//!
//! # 显示位置 / 大小（2026-09-25 起）
//!
//! 横幅不再钉死屏幕中央：`TimerSettings` 里有**锚点 + 双向百分比偏移 + 字号**三组配置，
//! 量纲与画中画 `MagnifierPipSettings` 严格同款（见 [`timer_center`]）。

use super::*;
use crate::core::config::{TIMER_FONT_MAX, TIMER_FONT_MIN};
use crate::core::text_metrics;

/// 把计时毫秒格式化为 `MM:SS`（≥1h 用 `HH:MM:SS`）。
///
/// 两种模式的格式相同（正计时显示已走过、倒计时显示剩余，区别只在喂进来的毫秒），
/// 所以不再按模式分流。秒数**向上取整**：倒计时刚起步时显示满时长（05:00），走到
/// 0.2s 也不跳成 00:00；正计时则是"跨过 1s 才显示 00:01"，符合直觉。
pub(crate) fn format_timer(ms: f64) -> String {
    let total = (ms / 1000.0).ceil().max(0.0) as u64;
    let hh = total / 3600;
    let mm = (total % 3600) / 60;
    let ss = total % 60;
    if hh > 0 {
        format!("{hh:02}:{mm:02}:{ss:02}")
    } else {
        format!("{mm:02}:{ss:02}")
    }
}

impl Runtime {
    /// 计时器开 / 停（快捷键）。
    pub(crate) fn toggle_timer(&mut self) {
        if self.timer_running {
            self.stop_timer();
            return;
        }
        self.timer_running = true;
        self.timer_start = Some(Instant::now());
        self.timer_ms = 0.0;
    }

    /// 停掉计时器并清零。关掉功能开关时也走这里（见 `Runtime::reconcile_disabled_tasks`）。
    ///
    /// ★ 单列一个函数，是因为"停止"有三个入口（快捷键、倒计时到点、关开关），而它们
    ///   必须清**同样**的三样：`timer_running` / `timer_start` / `timer_ms`。
    ///   漏掉 `timer_start` 的话下次开启会从旧起点算，`timer_ms` 不清则读秒先跳一下。
    pub(crate) fn stop_timer(&mut self) {
        if !self.timer_running && self.timer_start.is_none() {
            return;
        }
        self.timer_running = false;
        self.timer_start = None;
        self.timer_ms = 0.0;
    }

    /// 推进计时器（基于墙钟）。仅当 `timer_running` 为真时调用。
    pub(crate) fn advance_timer(&mut self) {
        let elapsed = match self.timer_start {
            Some(t) => t.elapsed().as_secs_f64() * 1000.0,
            None => {
                // 状态不一致（不该发生）：自愈成"停止"。
                self.timer_running = false;
                return;
            }
        };
        match self.config.global.timer.mode.as_str() {
            // 正计时：无限累计，只有手动停止。
            "countup" => {
                self.timer_ms = elapsed;
            }
            // 倒计时：到点自动停 + 提示。
            "countdown" => {
                let dur = self.config.global.timer.duration_sec as f64 * 1000.0;
                let rem = dur - elapsed;
                if rem <= 0.0 {
                    self.timer_running = false;
                    self.timer_start = None;
                    self.timer_ms = 0.0;
                    self.action_notify.show("计时结束");
                } else {
                    self.timer_ms = rem;
                }
            }
            _ => {}
        }
    }

    /// 画计时器：半透明深底条 + 白字（共享横幅函数 [`DrawList::draw_banner_at`]，
    /// 观感与倒计时/标注一致）；位置 = 配置的锚点 + 偏移，大小 = 配置字号。
    pub(crate) fn draw_timer(&mut self) {
        if !self.timer_running {
            return;
        }
        let t = &self.config.global.timer;
        let text = format_timer(self.timer_ms);
        // 字号是"整体大小"的唯一旋钮：内边距 / 圆角随字号等比走（72pt → pad 24、
        // 圆角 16，即位置可调落地前的硬编码值），默认观感逐像素不变。
        let font = t.font_pt.clamp(TIMER_FONT_MIN, TIMER_FONT_MAX) as f32;
        let pad = font / 3.0;
        let radius = pad * 2.0 / 3.0;
        let bw = text_metrics::estimate_width(&text, font) + pad * 2.0;
        let bh = text_metrics::line_height(font) + pad * 2.0;
        let center = timer_center(self.screen, t.anchor, t.offset_x, t.offset_y, bw, bh);
        self.draw_list.draw_banner_at(
            center,
            &text,
            font,
            pad,
            pad,
            Argb::new(180, 0x20, 0x20, 0x20),
            Argb::new(255, 0xFF, 0xFF, 0xFF),
            radius,
        );
    }
}

/// 横幅中心的几何：锚点贴边 + 百分比偏移往屏内挪，最后钳回屏内。
///
/// 语义与画中画 `modules::magnifier::pip::panel_rect` 严格同款：偏移是屏幕宽/高的
/// 百分比（0–50），把横幅从贴定的边往屏内推；**中央锚点没有"贴边"可言，偏移不生效**
/// （与画中画"鼠标跟随时偏移不生效"同一句话）。`bw`/`bh` 是横幅底框宽高（文字量得 +
/// 内边距），锚点对齐的是底框外沿。
///
/// 锚点取值（`TimerSettings::anchor`）：0=中央 / 1=顶部居中 / 2=底部居中 /
/// 3=左上 / 4=右上 / 5=左下 / 6=右下；≥7 落回中央（画中画同款"未知取值取模兜底"）。
fn timer_center(screen: ScreenRect, anchor: u32, off_x: u8, off_y: u8, bw: f32, bh: f32) -> Point {
    let (sx, sy, sw, sh) = (
        screen.x as f32,
        screen.y as f32,
        screen.w as f32,
        screen.h as f32,
    );
    let dx = sw * (off_x.min(50) as f32 / 100.0);
    let dy = sh * (off_y.min(50) as f32 / 100.0);
    let cx = sx + sw / 2.0;
    let cy = sy + sh / 2.0;
    // 贴边坐标：底框外沿贴住边（含偏移），中心再往屏内收半个底框。
    let left = sx + bw / 2.0 + dx;
    let right = sx + sw - bw / 2.0 - dx;
    let top = sy + bh / 2.0 + dy;
    let bottom = sy + sh - bh / 2.0 - dy;
    let (x, y) = match anchor {
        1 => (cx, top),
        2 => (cx, bottom),
        3 => (left, top),
        4 => (right, top),
        5 => (left, bottom),
        6 => (right, bottom),
        // 0 与一切未知取值：屏幕中央（偏移不生效）。
        _ => (cx, cy),
    };
    // 兜底钳制：极端字号（横幅比屏幕还宽/高）时保证底框尽量完整落在屏内。
    // 先 max 后 min 而不是 `clamp`：横幅宽于屏幕时 min 边 > max 边，`clamp` 会 panic。
    let x = x.max(sx + bw / 2.0).min(sx + sw - bw / 2.0);
    let y = y.max(sy + bh / 2.0).min(sy + sh - bh / 2.0);
    Point::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计时器字符串：向上取整 + 跨 1 小时。
    #[test]
    fn timer_text_formats_each_mode() {
        // 正计时：秒向上取整 ⇒ 4.2s 与 5.0s 都是 00:05，0 点是 00:00。
        assert_eq!(format_timer(0.0), "00:00");
        assert_eq!(format_timer(4200.0), "00:05");
        // 倒计时：显示剩余；跨 1 小时切换成 HH:MM:SS。
        assert_eq!(format_timer(299_000.0), "04:59");
        assert_eq!(format_timer(3_661_000.0), "01:01:01");
    }

    /// 屏幕矩形：原点 (100, 200)、1920×1080（带负坐标的多屏环境里 screen.x 可非 0，
    /// 判据里全部相对原点推，不依赖具体数值）。
    fn scr() -> ScreenRect {
        ScreenRect::new(100, 200, 1920, 1080)
    }

    /// 中央锚点 = 屏幕几何中心；**偏移不生效**（画中画"鼠标跟随不吃偏移"同款语义）。
    #[test]
    fn center_anchor_ignores_offsets() {
        let c0 = timer_center(scr(), 0, 0, 0, 200.0, 100.0);
        let c1 = timer_center(scr(), 0, 30, 20, 200.0, 100.0);
        assert_eq!(c0, Point::new(100.0 + 960.0, 200.0 + 540.0));
        assert_eq!(c0, c1, "中央锚点不吃偏移");
    }

    /// 顶部/底部居中：水平恒居中，垂直贴边 + 垂直偏移往屏内挪。
    #[test]
    fn top_and_bottom_anchors_offset_inward() {
        let t = timer_center(scr(), 1, 0, 10, 200.0, 100.0);
        assert_eq!(t.x, 100.0 + 960.0, "顶部居中的水平恒为屏心");
        assert_eq!(t.y, 200.0 + 50.0 + 108.0, "y = 顶 + 半高 + 屏高 10%");
        let b = timer_center(scr(), 2, 0, 10, 200.0, 100.0);
        assert_eq!(
            b.y,
            200.0 + 1080.0 - 50.0 - 108.0,
            "y = 底 − 半高 − 屏高 10%"
        );
    }

    /// 四角锚点：两向贴边 + 两向偏移都往屏内挪；≥7 落回中央。
    #[test]
    fn corner_anchors_and_unknown_fallback() {
        // 左上 + 偏移 (5%, 5%)：外沿贴 (sx+dx, sy+dy)。
        let tl = timer_center(scr(), 3, 5, 5, 200.0, 100.0);
        assert_eq!(tl.x, 100.0 + 96.0 + 100.0, "x = 左 + 半宽 + 屏宽 5%");
        assert_eq!(tl.y, 200.0 + 54.0 + 50.0, "y = 顶 + 半高 + 屏高 5%");
        // 右下：外沿贴 (右−dx, 下−dy)。
        let br = timer_center(scr(), 6, 5, 5, 200.0, 100.0);
        assert_eq!(br.x, 100.0 + 1920.0 - 96.0 - 100.0);
        assert_eq!(br.y, 200.0 + 1080.0 - 54.0 - 50.0);
        // 未知锚点兜底 = 中央。
        let center = timer_center(scr(), 0, 0, 0, 200.0, 100.0);
        assert_eq!(timer_center(scr(), 99, 0, 0, 200.0, 100.0), center);
    }

    /// 横幅宽于屏幕时不 panic、且贴住屏右沿（先 max 后 min，不用会 panic 的 clamp）。
    #[test]
    fn oversized_banner_is_clamped_without_panicking() {
        let huge = 4000.0;
        let c = timer_center(scr(), 6, 0, 0, huge, 100.0);
        // x 被钳到"右边缘 − 半宽"：底框右沿恰好贴住屏幕右沿（左沿越界是横幅太宽的必然）。
        assert_eq!(c.x, 100.0 + 1920.0 - huge / 2.0);
        assert_eq!(c.y, 200.0 + 1080.0 - 50.0, "高度未超限，贴底不受影响");
    }
}
