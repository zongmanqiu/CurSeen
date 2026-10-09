//! 全屏放大（`magnifier::fullscreen`）—— 画中画放大同一套底层的另一种形态。
//!
//! # 与画中画只差"几何 + 锚点"
//!
//! 链路**完全相同**：`screenshot::grab`（带 `CAPTUREBLT`）抓一块 → `platform::pip_window`
//! 用 `StretchDIBits` 拉满面板 → `UpdateLayeredWindow` 提交。窗自带
//! `WDA_EXCLUDEFROMCAPTURE`，所以不会把自己抓进源图（那正是画中画当初"无限套娃"的根因，
//! 见 `pip` 模块头）。
//!
//! 差别在**面板多大 + 取样区怎么锚**：
//!
//! ```text
//! 画中画：面板 = 屏幕 × 0.4 × 0.25（一小块），取样区 = 面板 ÷ 倍率、以鼠标为中心
//! 全屏：  面板 = 整个虚拟屏，          取样区 = 屏幕 ÷ 倍率、**锚住光标**
//! ```
//!
//! 面板一旦等于整屏，它"该放哪"就没有自由度了——恒等于屏幕左上角，"跟随鼠标"只能落到
//! 取样区上。但**锚法必须换**：画中画是"面板与取样区**都**以鼠标为中心"、两个居中相抵，
//! 光标才落在自己身上；全屏的面板钉死在原点、只剩取样区居中 ⇒ 光标下的内容会被显示到
//! **屏幕中心**去，用户按看到的画面点、点到的是别处（现象就是"鼠标无法点击"）。所以全屏
//! 用 [`super::anchored_sample_rect`] 的位移映射把光标钉在自身：**看到什么就点到什么**。
//!
//! 这也避免了"面板要撑到 2 倍屏幕"那条死路——那条路会让取样区越出虚拟屏、`BitBlt` 拿黑边。
//!
//! # 为什么必须有一把快捷键
//!
//! 这个窗是**置顶全屏不透明**的，会把 CurSeen 自己的控制面板一起盖住。没有快捷键
//! （或 Esc）就是"打开就关不掉"。脱身靠 **Esc**（不依赖配置）；启停快捷键出厂留空
//! （2026-09-26 用户口径，见 `default_hk_fullscreen_zoom`），用户自配。

use std::time::{Duration, Instant};

use super::anchored_sample_rect;
use crate::core::config::{Config, FULLSCREEN_ZOOM_MAX, FULLSCREEN_ZOOM_MIN};
use crate::platform::display::ScreenRect;
use crate::platform::pip_window::{PipLook, PipWindow, FULLSCREEN_WINDOW_TITLE};
use crate::platform::screenshot;

/// 重新抓帧的最小间隔（约 30 FPS 上限）。
///
/// ★ 与画中画同值不是巧合：两者共用同一条"抓屏 → 拉伸 → 提交"链路，节流口径一致时
///   体感才一致。全屏的绝对成本更高（面板大），但提交是 GDI 的合成、不进 tiny-skia。
const MIN_INTERVAL: Duration = Duration::from_millis(30);

/// 全屏放大运行时。
///
/// 自身不画——呈现由 [`PipWindow`] 完成，本模块决定**抓哪里 / 抓多大**，并做节流与
/// 状态复用。纯几何，可单测。
pub struct FullscreenMagnifier {
    /// 自己的原生窗（惰性：真正第一次 present 才建窗）。
    window: PipWindow,
    /// 上次成功呈现的取样区尺寸（`屏幕 ÷ 倍率`）；`None` = 还没成功地出过画。
    frame: Option<(i32, i32)>,
    /// 上次抓屏所依赖的倍率。倍率变 → 强制重抓。
    cap_key: Option<u32>,
    /// 上次抓帧时刻。
    last_capture: Option<Instant>,
    /// 上次抓帧对应的鼠标位置。☆ 2026-09-21 起抓帧门改为纯时间驱动，
    /// 字段暂时只写不读（保留理由同 `pip` ）。
    last_mouse: (f32, f32),
    /// 失败是否已报过（同 `pip`，这是每帧都跑的热路径）。
    fault_logged: bool,
}

impl FullscreenMagnifier {
    #[must_use]
    pub fn new() -> Self {
        Self {
            window: PipWindow::with_title(FULLSCREEN_WINDOW_TITLE),
            frame: None,
            cap_key: None,
            last_capture: None,
            last_mouse: (f32::NAN, f32::NAN),
            fault_logged: false,
        }
    }

    /// 是否已具备可画的源（供 `frame_demand` 判"刚开启要强制出帧"）。
    #[must_use]
    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// 上次**成功出画**时真正用到的取样区尺寸（`屏幕 ÷ 倍率`，物理像素）。
    #[must_use]
    pub fn last_sample(&self) -> Option<(i32, i32)> {
        self.frame
    }

    /// 全屏窗是否真的在屏上（不是"配置是否打开"——那是另一个概念）。
    #[must_use]
    pub fn is_shown(&self) -> bool {
        self.window.is_visible()
    }

    /// 面板尺寸 + 倍率：**面板就是整个虚拟屏**；取样区 = 屏幕 ÷ 倍率。
    ///
    /// 返回的 `(pw, ph)` 恒等于 `screen` 的宽高——特地把它算出来而不是直接用 `screen`，
    /// 是为了让"面板 = 屏幕"这件事在单测里**可判**（`panel_is_the_whole_screen`）。
    #[must_use]
    pub fn geom(
        p: &crate::core::config::MagnifierFullscreenSettings,
        screen: ScreenRect,
    ) -> (u32, u32, u32) {
        let zoom = p.zoom.clamp(FULLSCREEN_ZOOM_MIN, FULLSCREEN_ZOOM_MAX);
        (screen.w as u32, screen.h as u32, zoom)
    }

    /// 倍率是否被改过（供 `frame_demand` 在鼠标静止时强制出一帧）。
    #[must_use]
    pub fn needs_fresh(&self, cfg: &Config) -> bool {
        if !cfg.fullscreen_running() {
            return false;
        }
        let screen = crate::platform::display::virtual_screen();
        let (_, _, zoom) = Self::geom(&cfg.magnifier_fullscreen, screen);
        self.cap_key != Some(zoom)
    }

    /// 本帧推进。`mouse` 为虚拟桌面**物理像素**的鼠标坐标。
    pub fn draw(&mut self, cfg: &Config, mouse: (f32, f32)) {
        let p = &cfg.magnifier_fullscreen;
        // ★ 判据必须是 `fullscreen_running()`（勾选 × 快捷键激活，2026-09-26 两段语义）：
        //   只判 `enabled` 的话，用户勾选卡片的下一帧窗就整个糊上屏幕——"勾选只是载入
        //   许可、上屏只归快捷键"就被这条旁路绕穿了（用户实测踩中）。
        if !cfg.fullscreen_running() {
            // 关掉即清态：下次开启立刻重新抓，不残留旧帧。窗要藏走——
            // 它是置顶全屏的真窗，不清就会一直糊在屏幕上挡住一切。
            if self.window.is_visible() {
                self.window.hide();
            }
            self.frame = None;
            self.cap_key = None;
            self.last_capture = None;
            self.last_mouse = (f32::NAN, f32::NAN);
            self.fault_logged = false;
            return;
        }
        let screen = crate::platform::display::virtual_screen();
        let (pw, ph, zoom) = Self::geom(p, screen);
        // 取样区 = 屏幕 ÷ 倍率（向上取整，至少 1px 能出画；再钳到不比屏幕大）。
        let cw = (pw as u64)
            .div_ceil(zoom as u64)
            .max(1)
            .min(screen.w as u64) as u32;
        let ch = (ph as u64)
            .div_ceil(zoom as u64)
            .max(1)
            .min(screen.h as u64) as u32;

        let cfg_dirty = self.cap_key != Some(zoom);
        // 与画中画同一条时间驱动门（2026-09-21）：到节流间隔就重抓。
        // 全屏是"锚住光标"的位移映射，鼠标静止时取样区不动，但屏幕内容自己会变
        // （弹出菜单 / 进度条 / 视频），必须继续按节拍重抓才不会定格。首帧由
        // `last_capture` 为 None 时 `is_none_or` 恒真覆盖，无需单独判鼠标。
        let due = self
            .last_capture
            .is_none_or(|t| t.elapsed() >= MIN_INTERVAL);
        if !due && !cfg_dirty {
            return;
        }

        // ★ 面板矩形 = 整个虚拟屏（这就是"全屏"）；取样区**锚住光标**而不是居中——居中会
        //   把光标下的内容显示到屏幕中心，用户按看到的画面点就点错（见 `anchored_sample_rect`）。
        let panel = ScreenRect::new(screen.x, screen.y, pw as i32, ph as i32);
        let shot = screenshot::grab(anchored_sample_rect(mouse, cw, ch, screen));
        let shown = shot.and_then(|s| self.window.present(&s, panel, &fullscreen_look()));
        match shown {
            Ok(()) => {
                self.frame = Some((cw as i32, ch as i32));
                self.fault_logged = false;
            }
            Err(e) => {
                // ★ 只在第一次失败时报：抓屏/GDI 一旦持续失败，不节流就是一秒三十行。
                if !self.fault_logged {
                    log::error!("全屏放大：{e}");
                    self.fault_logged = true;
                }
            }
        }
        self.cap_key = Some(zoom);
        self.last_capture = Some(Instant::now());
        self.last_mouse = mouse;
    }

    /// 关掉整个功能时调用：藏窗 + 弃所有缓存状态。
    pub fn shutdown(&mut self) {
        self.window.hide();
        self.frame = None;
        self.cap_key = None;
        self.last_capture = None;
        self.last_mouse = (f32::NAN, f32::NAN);
        self.fault_logged = false;
    }
}

impl Default for FullscreenMagnifier {
    fn default() -> Self {
        Self::new()
    }
}

/// 面板外观：全屏时**不画边框、不画十字**。
///
/// - 边框压在屏幕最外沿，画了只挡任务栏/边缘内容；
/// - 十字恒在**面板中心 = 屏幕中心**，而全屏是"锚住光标"的位移映射（见
///   [`super::anchored_sample_rect`]），光标只在屏幕正中与中心重合、别处都不在 ⇒
///   画一条指不准的十字纯属误导。用户 2026-09-21 直接要求去掉这个选项，故这里用"不画
///   准星"的出厂外观（`PipLook::default()` 的 `crosshair_at` 就是 `None`）。
fn fullscreen_look() -> PipLook {
    PipLook {
        border_width: 0,
        ..PipLook::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 面板就是整个虚拟屏 —— 全屏放大与画中画在**尺寸**上的分界（另一个分界是取样锚点）。
    #[test]
    fn panel_is_the_whole_screen() {
        let cfg = Config::default();
        for s in [
            ScreenRect::new(0, 0, 1920, 1080),
            ScreenRect::new(-1920, -100, 3840, 1080),
        ] {
            let (pw, ph, _) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, s);
            assert_eq!(
                (pw as i32, ph as i32),
                (s.w, s.h),
                "全屏面板必须等于虚拟屏尺寸"
            );
        }
    }

    /// 倍率变大 ⇒ 取样区变小，且 `屏幕 ÷ 取样区 ≈ 倍率`（与画中画同一条判据）。
    #[test]
    fn zoom_shrinks_the_sampled_region_exactly_inversely() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        let mouse = (960.0, 540.0);
        for zoom in 1..=FULLSCREEN_ZOOM_MAX {
            cfg.magnifier_fullscreen.zoom = zoom;
            let (pw, ph, z) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, screen);
            let cw = (pw as u64).div_ceil(z as u64).max(1) as i32;
            let ch = (ph as u64).div_ceil(z as u64).max(1) as i32;
            let r = anchored_sample_rect(mouse, cw as u32, ch as u32, screen);
            assert_eq!((r.w, r.h), (cw, ch), "取样区尺寸必须是算出来的那个");

            let eff = pw as f32 / r.w as f32;
            assert!(
                (eff - zoom as f32).abs() <= 0.25,
                "zoom={zoom} 时有效倍率 {eff:.3} 偏离目标超过一个源像素"
            );
        }
    }

    /// ★★★ 全屏放大的**可用性判据**：光标正下方的内容必须显示在光标处。
    ///
    /// 不成立时的现象就是"鼠标无法点击"——用户按看到的画面点，点到的是别处。判据复刻生产
    /// 链路：`显示(光标) = 面板原点 +（光标 − 取样原点）× 有效倍率`（有效倍率 = 面板 ÷ 取样）。
    /// 容差半个源像素（`anchor_axis` 取整）——真正的回归（比如退回"取样区居中"）会差出
    /// 几十上百像素，这条照样红。
    #[test]
    fn the_point_under_the_cursor_shows_up_under_the_cursor() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        for zoom in 1..=FULLSCREEN_ZOOM_MAX {
            cfg.magnifier_fullscreen.zoom = zoom;
            let (pw, ph, z) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, screen);
            let cw = (pw as u64).div_ceil(z as u64).max(1) as u32;
            let ch = (ph as u64).div_ceil(z as u64).max(1) as u32;
            let panel = ScreenRect::new(screen.x, screen.y, pw as i32, ph as i32);
            for mouse in [
                (0.0, 0.0),
                (960.0, 540.0),
                (1919.0, 1079.0),
                (300.0, 900.0),
                (1700.0, 120.0),
            ] {
                let s = anchored_sample_rect(mouse, cw, ch, screen);
                let zx = panel.w as f32 / s.w as f32;
                let zy = panel.h as f32 / s.h as f32;
                let dx = panel.x as f32 + (mouse.0 - s.x as f32) * zx;
                let dy = panel.y as f32 + (mouse.1 - s.y as f32) * zy;
                assert!(
                    (dx - mouse.0).abs() <= zx * 0.5 + 1.0
                        && (dy - mouse.1).abs() <= zy * 0.5 + 1.0,
                    "zoom={zoom} 鼠标 {mouse:?} 的内容被显示到 ({dx:.1},{dy:.1})，应≈光标本身"
                );
            }
        }
    }

    /// ★ 鼠标推到四角：取样区不许越出屏幕（`BitBlt` 出界只拿回黑边）。
    ///
    /// 锚定取样在数学上天然不越界（两端点即"贴原点"与"贴右/下边"），这条把它钉住——
    /// 也是"面板不必撑成 2 倍屏幕"的依据。
    #[test]
    fn sample_stays_inside_when_the_mouse_hits_every_corner() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        cfg.magnifier_fullscreen.zoom = 4;
        let (pw, ph, z) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, screen);
        let cw = (pw as u64).div_ceil(z as u64).max(1) as u32;
        let ch = (ph as u64).div_ceil(z as u64).max(1) as u32;
        for mouse in [
            (0.0, 0.0),
            (1919.0, 0.0),
            (0.0, 1079.0),
            (1919.0, 1079.0),
            (-9999.0, 9999.0),
        ] {
            let r = anchored_sample_rect(mouse, cw, ch, screen);
            assert!(
                r.x >= screen.x
                    && r.y >= screen.y
                    && r.x + r.w <= screen.x + screen.w
                    && r.y + r.h <= screen.y + screen.h,
                "鼠标 {mouse:?} 时取样区出界: {r:?}"
            );
        }
    }

    /// 倍率要被判成"需要重抓"，否则鼠标静止时改倍率永远不上屏。
    #[test]
    fn changing_zoom_needs_a_refresh() {
        let mut m = FullscreenMagnifier::new();
        let mut cfg = Config::default();
        cfg.magnifier_fullscreen.enabled = true;
        cfg.magnifier_fullscreen.active = true; // 两段语义：载入之外还要激活才真正上屏
        m.draw(&cfg, (100.0, 100.0));
        assert!(!m.needs_fresh(&cfg), "参数没变就不该要求重抓");
        cfg.magnifier_fullscreen.zoom = 3;
        assert!(m.needs_fresh(&cfg), "改了倍率必须要求重抓");
    }

    /// 关闭时窗必须藏走：它是置顶全屏的，留着就挡住一切。
    #[test]
    fn disabled_hides_the_window() {
        let mut m = FullscreenMagnifier::new();
        let cfg = Config::default();
        m.draw(&cfg, (500.0, 400.0));
        assert!(!m.has_frame(), "禁用时不该有画");
        assert!(!m.is_shown(), "禁用时不该留着一个全屏窗");
    }

    /// ★★ 回归（2026-09-26 用户实测踩中）：**只勾选、未按快捷键**时，无论渲染多少帧
    /// 都不得上屏/抓屏。勾选只是载入许可，上屏唯一入口是启停快捷键（`active`）。
    /// 旧版 draw 只判 `enabled`，勾选的下一帧整个屏幕就被放大画面糊住。
    #[test]
    fn checking_alone_never_shows_the_window() {
        let mut m = FullscreenMagnifier::new();
        let mut cfg = Config::default();
        cfg.magnifier_fullscreen.enabled = true; // 只载入
        for _ in 0..3 {
            m.draw(&cfg, (500.0, 400.0));
        }
        assert!(!m.has_frame(), "未激活不得抓屏");
        assert!(!m.is_shown(), "未激活不得上屏");
        assert!(!m.needs_fresh(&cfg), "未激活也不该唤醒帧门");
    }

    /// 倍率钳制与画中画那对常量无关：全屏量程更宽（`FULLSCREEN_ZOOM_MAX`）。
    #[test]
    fn zoom_is_clamped_to_the_fullscreen_range() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        cfg.magnifier_fullscreen.zoom = 999;
        let (_, _, z) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, screen);
        assert_eq!(z, FULLSCREEN_ZOOM_MAX, "超上限要被钳住");
        cfg.magnifier_fullscreen.zoom = 0;
        let (_, _, z) = FullscreenMagnifier::geom(&cfg.magnifier_fullscreen, screen);
        assert_eq!(z, FULLSCREEN_ZOOM_MIN, "低于下限要被钳住");
    }
}
