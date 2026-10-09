//! 画中画放大（`magnifier::pip`）—— 独立原生模块。
//!
//! 抓"鼠标附近的取样区"→ 按**放大倍率**放大 → 显示在一个自己的原生窗里，另加一圈边框。
//!
//! # 关键几何（用户口径 2026-09-20）
//!
//! 用「面板宽 + 面板高 + 放大倍率」定画面：面板尺寸直接决定显示区域；取样区尺寸 =
//! 面板 ÷ 倍率——倍率越大，面板里装的源区越小、放得越大。位置支持四角贴边 + 鼠标跟随。
//!
//! # ★★ 面板为什么不再画在覆盖层上（2026-09-20 重写）
//!
//! 上一版把面板当一条 `DrawCmd::Image` 画在 CurSeen 的全屏覆盖层上，靠"不带
//! `CAPTUREBLT` 的 `BitBlt` 抓不到分层窗"来保证源图干净。**这条前提是错的**：实测
//! （见 `tests/pip_probe.rs`）无论带不带 `CAPTUREBLT`，`UpdateLayeredWindow` 提交的内
//! 容都能被抓到——于是放大镜把自己上一帧抓进源图、再放大：
//!
//! - **症状 1（无限套娃）**：面板里永远含着更小的一层面板，层层相套；
//! - **症状 2（倍率无效）**：倍率只改变"套娃某一层占多大"，画面看着没变大。
//!
//! 两个症状同一个根因。修法是给面板开一个**自己的窗**
//! （`platform::pip_window::PipWindow`）并给它
//! `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` —— 系统级的排除策略，GDI 与
//! DXGI 都遵守，且**按窗口**生效，不会连累覆盖层上别的特效。详见该模块头。

use std::time::{Duration, Instant};

use crate::core::color::Argb;
use crate::core::config::{Config, PIP_PANEL_MAX, PIP_PANEL_MIN, PIP_ZOOM_MAX, PIP_ZOOM_MIN};

use super::sample_rect;
use crate::core::geometry::Rect;
use crate::platform::display::ScreenRect;
use crate::platform::pip_window::{PipLook, PipWindow};
use crate::platform::screenshot;

/// 重新抓帧的最小间隔（约 30 FPS 上限）。
const MIN_INTERVAL: Duration = Duration::from_millis(30);
/// 鼠标**静止**时的最大重抓间隔。
///
/// # 为什么必须有（2026-09-21 用户报告）
///
/// 抓帧门原先写的是"鼠标动 ≥1px 才重抓"，于是屏幕上**自己会变**的内容——点击冒出的
/// 弹出菜单、右键菜单、进度条、视频——在指针停着不动时永远进不了面板；用户鼠标随便
/// 一晃画面才追上来。放大镜的本质是"屏幕该看哪就看哪"，不是"鼠标轨迹回放"，
/// 所以静止期必须继续按节拍重抓。
///
/// 取值与 [`MIN_INTERVAL`] 相同：放大就是 30FPS 的实时取景器，静止时维持同一帧率
/// 没有额外负担（每帧只是一次小面积 `BitBlt` + GDI 合成提交，见模块头的成本账）。
/// 也**不必取更长**：等"屏幕变了"的系统通知（DXGI dirty 区回调 / 事件驱动重抓）要引
/// 新的采集路径，收益撑不起复杂度——先给住体验，将来真有功耗诉求再切事件驱动。
const MAX_REFRESH_INTERVAL: Duration = MIN_INTERVAL;
/// 面板边框宽度（物理像素）。
const BORDER_W: i32 = 2;

/// 画中画放大运行时。
///
/// 自身不画——真正的呈现由 [`PipWindow`] 完成，本模块只负责决定
/// **抓哪里 / 抓多大 / 画到哪**，并做节流与状态复用。纯几何，可单测。
pub struct PipMagnifier {
    /// 自己的原生窗（惰性：真正第一次 present 才建窗）。
    window: PipWindow,
    /// 上次成功呈现的取样区尺寸（`面板 ÷ 倍率`）；`None` = 还没成功地出过画。
    frame: Option<(i32, i32)>,
    /// 上次抓屏所依赖的 (anchor, 面板宽, 面板高, 倍率)。参数变 → 强制重抓。
    cap_key: Option<(u32, u32, u32, u32)>,
    /// 上次抓帧时刻。
    last_capture: Option<Instant>,
    /// 上次抓帧对应的鼠标位置。☆ 2026-09-21 起抓帧门改为纯时间驱动，
    /// 字段暂时只写不读；保留它是为了将来若要加"位移去抖"（鼠标微动不重抓）
    /// 或日志诊断时直接恢复读端，不必再动结构。
    last_mouse: (f32, f32),
    /// 失败是否已报过（见 Self::draw 里"为什么做法是这一点"）。
    fault_logged: bool,
}

impl PipMagnifier {
    pub fn new() -> Self {
        Self {
            window: PipWindow::new(),
            frame: None,
            cap_key: None,
            last_capture: None,
            last_mouse: (f32::NAN, f32::NAN),
            fault_logged: false,
        }
    }

    /// 是否已具备可画的源（供 `frame_demand` 判"刚开启要强制出帧"）。
    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// 上次**成功出画**时真正用到的取样区尺寸（`面板 ÷ 倍率`，物理像素）。
    ///
    /// 与 `has_frame` 的区别是它带尺寸：门禁要能断言"倍率变了 ⇒ 取样区真的变了"，
    /// 而不只是"出过画"。只有成功那一支才写，所以它同时也是"抓屏/呈现是否还活着"的
    /// 探针——一直返回同一个值，说明后面每一帧都在 `Err` 分支上定格。
    pub fn last_sample(&self) -> Option<(i32, i32)> {
        self.frame
    }

    /// 面板是否真的在屏上（不是"配置是否打开"——那是另一个概念）。
    pub fn is_shown(&self) -> bool {
        self.window.is_visible()
    }

    /// 换算面板像素尺寸 + 倍率：面板 = 屏幕 × 比例；取样区 = 面板 ÷ 倍率。
    ///
    /// 钳制窗直接取 `core::config` 里那几个常量 —— 与面板滑条、`v2s/s2v` 换算同源。
    /// 这里再抄一份数字的话，量程一改就会出现"滑块能选到的值与运行时实际量纲错位"这类问题。
    fn geom(p: &crate::core::config::MagnifierPipSettings, screen: ScreenRect) -> (u32, u32, u32) {
        let pw = (screen.w as f32 * p.panel_width.clamp(PIP_PANEL_MIN, PIP_PANEL_MAX))
            .round()
            .max(1.0) as u32;
        let ph = (screen.h as f32 * p.panel_height.clamp(PIP_PANEL_MIN, PIP_PANEL_MAX))
            .round()
            .max(1.0) as u32;
        let zoom = p.zoom.clamp(PIP_ZOOM_MIN, PIP_ZOOM_MAX);
        (pw, ph, zoom)
    }

    /// 面板几何参数(位置/宽/高/倍率)是否相对上次绘制变了。供 `frame_demand` 判：
    /// 用户拖面板宽高或倍率滑条（鼠标可能停在设置窗上、桌面没动）时，也要强制出一帧——
    /// 否则帧门在"鼠标静止"下休眠，面板尺寸/倍率的改动永远不反映到屏上（用户报"没反应"）。
    pub fn needs_fresh(&self, cfg: &Config) -> bool {
        if !cfg.pip_running() {
            return false;
        }
        let p = &cfg.magnifier_pip;
        let screen = crate::platform::display::virtual_screen();
        let (pw, ph, zoom) = Self::geom(p, screen);
        let key = (p.anchor, pw, ph, zoom);
        // ★ 与 draw 的重抓门同源：静止期 draw 每 `MAX_REFRESH_INTERVAL` 就会出画，
        //   "上次出画"过去太久（说明这期间一帧都没出）就必须唤醒帧门。
        //   `MIN_INTERVAL` 一旦调大（例如降到 15FPS），两边同一个常量自动跟着变，
        //   不会出现"运行时 66ms 一抓、帧门 250ms 才唤醒"这类两份口径的错位。
        let stale = self
            .last_capture
            .is_none_or(|t| t.elapsed() > MAX_REFRESH_INTERVAL);
        self.cap_key != Some(key) || stale
    }

    /// 本帧推进。`mouse` 为虚拟桌面**物理像素**的鼠标坐标。
    pub fn draw(&mut self, cfg: &Config, mouse: (f32, f32)) {
        let p = &cfg.magnifier_pip;
        // ★ 判据必须是 `pip_running()`（勾选 × 快捷键激活，2026-09-26 两段语义）：
        //   只判 `enabled` 的话，勾选卡片的下一帧面板就自己上屏——"勾选只是载入许可、
        //   上屏只归快捷键"被这条旁路绕穿（全屏侧用户实测踩中，此处同修）。
        if !cfg.pip_running() {
            // 关掉即清态：下次开启立刻重新抓，不残留旧帧。窗要藏走——
            // 它是不受"帧门休眠"影响的真窗，不清就会一直挂在屏幕上。
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
        // 取样区 = 面板 ÷ 倍率（像素向上取整，至少 ≥1px 能出画）。
        let cw = (pw as u64).div_ceil(zoom as u64).max(1) as u32;
        let ch = (ph as u64).div_ceil(zoom as u64).max(1) as u32;
        // 屏幕比取样区还小时无解（极小分辨率），直接不画。
        if cw as i32 > screen.w || ch as i32 > screen.h {
            return;
        }

        let pkey = (p.anchor, pw, ph, zoom);
        let cfg_dirty = self.cap_key != Some(pkey);
        // ★ 时间驱动门（2026-09-21 重写）：到节流间隔就重抓——不再要求"鼠标动了才抓"。
        //   屏幕内容自己会变（点击冒出的菜单、右键菜单、进度条、视频），指针停着时
        //   它们也必须实时进面板（用户报告的"点出菜单面板不刷新，动一动才刷新"）。
        //   首帧不需要单独判：`last_capture` 为 None 时 `is_none_or` 恒真，自然放行。
        //   （曾把门写成 `!(moved && due)`——moved 只在首帧为真，静止期到点也会被
        //   拦下，被 `a_still_cursor_keeps_refreshing_on_the_look_cadence` 当场抓获。）
        let due = self
            .last_capture
            .is_none_or(|t| t.elapsed() >= MAX_REFRESH_INTERVAL);
        if !due && !cfg_dirty {
            return;
        }

        let panel = panel_rect(
            p.anchor, pw as f32, ph as f32, screen, mouse, p.offset_x, p.offset_y,
        );
        let sample = sample_rect(mouse, cw, ch, screen);
        let look = look_of(p);
        // ★ 用 `grab`（带 `CAPTUREBLT`）而不是旧版的 `grab_clean`：
        //   排己是靠 `PipWindow` 的 `WDA_EXCLUDEFROMCAPTURE` 做到的（系统级、可靠），
        //   不再需要指望"碰巧抓不到分层窗"；反过来带上 `CAPTUREBLT`，屏幕上**其它**
        //   软件的分层窗才会被如实放大——放大镜要放大的是"用户看到的画面"。
        let shot = screenshot::grab(sample);
        let shown = shot.and_then(|s| {
            self.window.present(
                &s,
                ScreenRect::new(panel.x as i32, panel.y as i32, pw as i32, ph as i32),
                &look,
            )
        });
        match shown {
            Ok(()) => {
                self.frame = Some((cw as i32, ch as i32));
                self.fault_logged = false;
            }
            Err(e) => {
                // ★ 只在**第一次**失败时报：这是每帧都在跑的热路径，抓屏/GDI 一旦持续
                //   失败，不节流的话一秒就是三十行日志，把真正有用的线索淹掉。
                if !self.fault_logged {
                    log::error!("画中画：{e}");
                    self.fault_logged = true;
                }
            }
        }
        self.cap_key = Some(pkey);
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

impl Default for PipMagnifier {
    fn default() -> Self {
        Self::new()
    }
}

/// 面板外观：边框色取自配置（解析不出来时退回出厂那一支，不至于整块不画）。
///
/// # 指针准星为什么没了（2026-09-21 用户拍板："完全不需要这个"）
///
/// 它原先负责"标出指针所指的那块内容在面板的哪一点"。四角贴边时它确实是面板内唯一的
/// 指针指示，但它也恒盖住正要被看清的那个像素、且随钳制漂移后反而添乱。删的是**配置项
/// `show_crosshair` + 面板行 + 几何映射 `crosshair_pos`**；`platform::pip_window` 的
/// `PipLook.crosshair_at` 画点能力保留——`Option=None` 时窗口层一个准星像素都不画，
/// 全屏放大也不受影响。
fn look_of(p: &crate::core::config::MagnifierPipSettings) -> PipLook {
    let base = PipLook::default();
    PipLook {
        border: Argb::parse_hex(&p.border_color).unwrap_or(base.border),
        border_width: BORDER_W,
        crosshair_at: None,
        grid: None,
    }
}

/// 面板矩形。`anchor`：0=鼠标跟随（面板以鼠标为中心），1–4=四角**贴边**。
///
/// `offset_x/y`：屏幕宽/高的百分比（0–50），把面板从贴定的边上往屏内挪——贴下边
/// 抬高即可让开任务栏（2026-09-21 用户报告）。鼠标跟随没有"贴边"可言，偏移不生效。
/// 面板最宽 50% 屏 + 偏移最多 50% 屏，相加恰好不越出屏幕，无需再钳。
fn panel_rect(
    anchor: u32,
    pw: f32,
    ph: f32,
    screen: ScreenRect,
    mouse: (f32, f32),
    offset_x: u8,
    offset_y: u8,
) -> Rect {
    let dx = screen.w as f32 * (offset_x.min(50) as f32 / 100.0);
    let dy = screen.h as f32 * (offset_y.min(50) as f32 / 100.0);
    let (sx, sy, sw, sh) = (
        screen.x as f32,
        screen.y as f32,
        screen.w as f32,
        screen.h as f32,
    );
    let (x, y) = match anchor {
        0 => {
            // 鼠标跟随：面板中心对准鼠标（左上角 = 中心 − 半宽高），钳在屏内避免飞出。
            let px = (mouse.0 - pw * 0.5).clamp(sx, sx + sw - pw);
            let py = (mouse.1 - ph * 0.5).clamp(sy, sy + sh - ph);
            (px, py)
        }
        1 => (sx + dx, sy + dy),
        2 => (sx + sw - pw - dx, sy + dy),
        3 => (sx + dx, sy + sh - ph - dy),
        _ => (sx + sw - pw - dx, sy + sh - ph - dy),
    };
    Rect::new(x, y, pw, ph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;

    #[test]
    fn disabled_hides_the_window() {
        let mut m = PipMagnifier::new();
        let cfg = Config::default();
        m.draw(&cfg, (500.0, 400.0));
        assert!(!m.has_frame(), "禁用时不该有画");
        assert!(!m.is_shown(), "禁用时不该留着一个窗");
    }

    /// ★★ 回归（与全屏侧 `checking_alone_never_shows_the_window` 同源）：只勾选、
    /// 未按快捷键（`active=false`）时，渲染多少帧都不得上屏/抓屏/唤醒帧门。
    #[test]
    fn checking_alone_never_shows_the_panel() {
        let mut m = PipMagnifier::new();
        let mut cfg = Config::default();
        cfg.magnifier_pip.enabled = true; // 只载入
        for _ in 0..3 {
            m.draw(&cfg, (500.0, 400.0));
        }
        assert!(!m.has_frame(), "未激活不得抓屏");
        assert!(!m.is_shown(), "未激活不得上屏");
        assert!(!m.needs_fresh(&cfg), "未激活也不该唤醒帧门");
    }

    #[test]
    fn panel_rect_places_in_each_corner_and_follow() {
        let s = ScreenRect::new(0, 0, 1000, 800);
        let tl = panel_rect(1, 100.0, 80.0, s, (500.0, 400.0), 0, 0);
        assert!((tl.x - 0.0).abs() < 1e-3 && (tl.y - 0.0).abs() < 1e-3);
        let tr = panel_rect(2, 100.0, 80.0, s, (0.0, 0.0), 0, 0);
        assert!((tr.x - (1000.0 - 100.0)).abs() < 1e-3);
        assert!((tr.y - 0.0).abs() < 1e-3);
        let bl = panel_rect(3, 100.0, 80.0, s, (0.0, 0.0), 0, 0);
        assert!((bl.x - 0.0).abs() < 1e-3);
        assert!((bl.y - (800.0 - 80.0)).abs() < 1e-3);
        // 鼠标跟随：面板以鼠标为中心
        let f = panel_rect(0, 100.0, 80.0, s, (500.0, 400.0), 0, 0);
        assert!((f.x - 450.0).abs() < 1e-3 && (f.y - 360.0).abs() < 1e-3);
        // 越界回环：anchor>=5 取模落到右下
        let br = panel_rect(99, 100.0, 80.0, s, (0.0, 0.0), 0, 0);
        assert_eq!((br.x, br.y), (tr.x, bl.y));

        // ★ 偏移（2026-09-21）：贴边位置把面板从边上往屏内挪——贴下边抬高让开任务栏。
        //    偏移量 = 屏幕宽/高 × 百分比；鼠标跟随不吃偏移；50% 偏移 + 最大面板
        //    恰好贴满另一半屏幕，绝不越界（面板上限 50% + 偏移上限 50% = 100%）。
        let o = panel_rect(3, 100.0, 80.0, s, (0.0, 0.0), 10, 5);
        assert!((o.x - 100.0).abs() < 1e-3, "水平偏移 = 屏宽 10% = 100px");
        assert!(
            (o.y - (800.0 - 80.0 - 40.0)).abs() < 1e-3,
            "垂直偏移 = 屏高 5% = 40px"
        );
        let f = panel_rect(0, 100.0, 80.0, s, (500.0, 400.0), 20, 20);
        assert!(
            (f.x - 450.0).abs() < 1e-3 && (f.y - 360.0).abs() < 1e-3,
            "鼠标跟随时偏移不生效"
        );
        let edge = panel_rect(1, 500.0, 400.0, s, (0.0, 0.0), 50, 50);
        assert!(
            (edge.x - 500.0).abs() < 1e-3 && (edge.y - 400.0).abs() < 1e-3,
            "50% 偏移 + 最大面板 = 恰好贴满另一半，不越界"
        );
    }

    /// ★★ 倍率变大 ⇒ 取样区变小，且**面板宽度 ÷ 取样宽度就是实际倍率**。
    ///
    /// 这条钉住的核心事实：倍率唯一的落地方式是缩小取样区，"把小图拉满面板"由
    /// `PipWindow` 做。若哪天有人在别处再乘一次倍率，画面会二次放大而两侧看起来都
    /// "能用"——只有这条会红。
    ///
    /// 容差 1 个源像素而不是判等号：`面板 ÷ 倍率` 用 `div_ceil`（保证取样区≥1px、
    /// 且不被除丢），除不尽时会差不到一个源像素（1080p 默认 zoom=4：270/4=67.5 取 68
    /// ⇒ 有效 3.97×，肉眼无从分辨），写成等号会在这些组合上误报。
    #[test]
    fn zoom_shrinks_the_sampled_region_exactly_inversely() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        cfg.magnifier_pip.panel_width = 0.4; // → 面板 768
        cfg.magnifier_pip.panel_height = 0.25; // → 面板 270
        let mouse = (960.0, 540.0);
        for zoom in 1..=4u32 {
            cfg.magnifier_pip.zoom = zoom;
            let (pw, ph, z) = PipMagnifier::geom(&cfg.magnifier_pip, screen);
            let cw = (pw as u64).div_ceil(z as u64).max(1) as i32;
            let ch = (ph as u64).div_ceil(z as u64).max(1) as i32;
            let r = sample_rect(mouse, cw as u32, ch as u32, screen);
            assert_eq!((r.w, r.h), (cw, ch), "取样区尺寸必须是算出来的那个");

            let eff_w = pw as f32 / r.w as f32;
            let eff_h = ph as f32 / r.h as f32;
            for (eff, name) in [(eff_w, "宽"), (eff_h, "高")] {
                assert!(
                    (eff - zoom as f32).abs() <= 0.25,
                    "zoom={zoom} 时{name}向有效倍率 {eff:.3} 偏离目标超过一个源像素"
                );
            }
        }
    }

    /// 倍率必须单调：倍数越大，取样区越小（这条把"反了"的符号错误钉死）。
    #[test]
    fn a_bigger_zoom_never_samples_more() {
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let mut cfg = Config::default();
        let mut prev = i32::MAX;
        for zoom in 1..=4u32 {
            cfg.magnifier_pip.zoom = zoom;
            let (pw, ph, z) = PipMagnifier::geom(&cfg.magnifier_pip, screen);
            let cw = (pw as u64).div_ceil(z as u64).max(1) as i32;
            let ch = (ph as u64).div_ceil(z as u64).max(1) as i32;
            assert!(cw * ch < prev, "zoom={zoom} 取样区没有变小（{cw}x{ch}）");
            prev = cw * ch;
        }
    }

    /// 几何链路：面板 = 屏幕 × 比例；默认值在 1920×1080 上是 384×216（2026-09-26 起 20%×20%）。
    #[test]
    fn geom_derives_panel_from_screen_ratio() {
        let cfg = Config::default();
        let screen = ScreenRect::new(0, 0, 1920, 1080);
        let (pw, ph, z) = PipMagnifier::geom(&cfg.magnifier_pip, screen);
        assert_eq!((pw, ph), (384, 216), "默认 20%×20% @1920×1080");
        assert_eq!(z, 2, "默认倍率 2");
    }

    /// ★★ 鼠标**静止**也必须按节拍重抓（2026-09-21 用户报"点出菜单面板不刷新"）。
    ///
    /// 旧门是"鼠标动 ≥1px 才抓"，指针停住时屏幕上自己会变的内容（点击冒出的菜单、
    /// 进度条、视频）永远进不了面板。这条钉住时间驱动门的三个时刻：刚抓完不重抓
    /// （节拍内）、超过间隔必须要求出帧（帧门据此定时唤醒）、重抓后重新计时。
    #[test]
    fn a_still_cursor_keeps_refreshing_on_the_look_cadence() {
        let mut m = PipMagnifier::new();
        let mut cfg = Config::default();
        cfg.magnifier_pip.enabled = true;
        cfg.magnifier_pip.active = true; // 两段语义：载入之外还要激活才真正上屏
        m.draw(&cfg, (500.0, 400.0));
        assert!(!m.needs_fresh(&cfg), "刚抓过一帧不该立刻要求重抓");
        std::thread::sleep(MAX_REFRESH_INTERVAL + Duration::from_millis(20));
        assert!(
            m.needs_fresh(&cfg),
            "静止超过重抓间隔后必须要求出帧——屏幕内容会自己变（弹出菜单等）"
        );
        // 再画一轮：时间驱动门必须放行（鼠标没动也重抓），抓完节拍重新计时。
        m.draw(&cfg, (500.0, 400.0));
        assert!(!m.needs_fresh(&cfg), "重抓之后节拍重新计时");
    }

    /// 倍率滑条被改（含钳制）要被判成"需要重抓"，否则静止的鼠标会让改动永不上屏。
    #[test]
    fn changing_zoom_markedly_needs_a_refresh() {
        let mut m = PipMagnifier::new();
        let mut cfg = Config::default();
        cfg.magnifier_pip.enabled = true;
        cfg.magnifier_pip.active = true; // 两段语义：载入之外还要激活才真正上屏
                                         // 走一次 draw 把 cap_key 写定（无论抓屏是否成功，几何状态都会落下去）。
        m.draw(&cfg, (100.0, 100.0));
        assert!(!m.needs_fresh(&cfg), "参数没变就不该要求重抓");
        cfg.magnifier_pip.zoom = 3;
        assert!(m.needs_fresh(&cfg), "改了倍率必须要求重抓");
        cfg.magnifier_pip.zoom = 3;
        m.draw(&cfg, (100.0, 100.0));
        cfg.magnifier_pip.anchor = 2;
        assert!(m.needs_fresh(&cfg), "改了位置也必须要求重抓");
    }
}
