//! 叠层基座：几何换算的**唯一出处** + 穿透/取消手势的**唯一裁决点**。
//!
//! 三个消费方（截图区域选择 / 屏幕注释·标注 / 贴图窗）共用这套设施：
//!
//! - [`WorldMap`]：世界坐标（物理像素，虚拟桌面坐标系）↔ 窗口本地逻辑坐标。
//! - [`LayerSession`]：叠层活跃态 → 覆盖层该不该穿透、Esc 该关谁。
//! - [`region`] / [`pin`]：区域选择窗与贴图窗两个控件。
//!
//! 底图本身在 [`crate::core::backdrop`]（`modules/drawing` 也要用它，那里不能依赖 runtime）。
//!
//! # 为什么必须有个 `WorldMap`
//!
//! windui 有**两个单位不一致**的入口：
//!
//! | API | 单位 |
//! |---|---|
//! | `Window::position(x, y)` / `App::pos(x, y)` | **物理像素**（虚拟桌面坐标，含副屏负值） |
//! | `Window::new(title, w, h)` / `App::new(title, w, h)` | **逻辑 dp**（平台按 `GetDpiForSystem` 反算外框） |
//! | `Canvas` 的绘制坐标 | **逻辑**（平台按 dpi 乘回物理再光栅化） |
//! | GDI 冻结帧 / `ScreenRect` | **物理** |
//!
//! 把物理尺寸直接当逻辑尺寸传（旧代码就是这么写的）在 100% 缩放下看不出问题，150% 下
//! 窗口会大出 1.5 倍、缓冲多占 2.25 倍。换算只允许经这两个函数，禁止在 consumer 里手写
//! `as i32` 直传。

pub mod gif_bar;
pub mod gif_trim;
pub mod mp4_trim;
pub mod pin;
pub mod region;
pub mod region_magnifier;

use crate::core::geometry::Rect;
use crate::platform::display::{dpi_scale, ScreenRect};

/// 世界坐标（物理像素）↔ 窗口本地逻辑坐标的换算器。
///
/// 由虚拟桌面矩形与系统 DPI 唯一确定，可自由 `Copy`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldMap {
    origin: ScreenRect,
    dpi: f32,
}

impl WorldMap {
    /// 取系统 DPI 构造。
    pub fn new(screen: ScreenRect) -> Self {
        Self::with_dpi(screen, dpi_scale())
    }

    /// 显式指定 dpi 构造（单测注入 1.5 / 2.0 用）。
    pub fn with_dpi(screen: ScreenRect, dpi: f32) -> Self {
        let dpi = if dpi.is_finite() && dpi > 0.0 {
            dpi
        } else {
            1.0
        };
        Self {
            origin: screen,
            dpi,
        }
    }

    pub fn dpi(&self) -> f32 {
        self.dpi
    }

    pub fn origin(&self) -> ScreenRect {
        self.origin
    }

    /// 世界 → 窗口本地逻辑坐标。
    pub fn to_window(&self, x: f32, y: f32) -> (f32, f32) {
        (
            (x - self.origin.x as f32) / self.dpi,
            (y - self.origin.y as f32) / self.dpi,
        )
    }

    /// 窗口本地逻辑坐标 → 世界。
    pub fn to_world(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.origin.x as f32 + x * self.dpi,
            self.origin.y as f32 + y * self.dpi,
        )
    }

    /// 世界长度 → 窗口逻辑长度。
    pub fn len_to_window(&self, v: f32) -> f32 {
        v / self.dpi
    }

    /// 窗口逻辑长度 → 世界长度。
    pub fn len_to_world(&self, v: f32) -> f32 {
        v * self.dpi
    }

    /// 世界矩形 → 窗口逻辑矩形。
    pub fn rect_to_window(&self, r: Rect) -> Rect {
        let (x, y) = self.to_window(r.x, r.y);
        Rect {
            x,
            y,
            w: r.w / self.dpi,
            h: r.h / self.dpi,
        }
    }

    /// 整虚拟桌面在窗口里的**逻辑尺寸**——`App::new` / `Window::new` 要的就是它
    /// （它们的参数是逻辑 dp，不是物理像素）。
    pub fn logical_size(&self) -> (i32, i32) {
        (
            ((self.origin.w as f32 / self.dpi).round() as i32).max(1),
            ((self.origin.h as f32 / self.dpi).round() as i32).max(1),
        )
    }

    /// 整虚拟桌面在窗口里的逻辑矩形（原点恒为 (0,0)）。
    pub fn logical_rect(&self) -> Rect {
        let (w, h) = self.logical_size();
        Rect {
            x: 0.0,
            y: 0.0,
            w: w as f32,
            h: h as f32,
        }
    }

    /// 整虚拟桌面的**世界**矩形（铺底图给 `DrawList` 用）。
    pub fn world_rect(&self) -> Rect {
        Rect {
            x: self.origin.x as f32,
            y: self.origin.y as f32,
            w: self.origin.w as f32,
            h: self.origin.h as f32,
        }
    }
}

/// 叠层的活跃态快照（每帧按当前状态取一次，不缓存、不订阅——缓存就会有两个真相）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayerSession {
    /// 截图区域选择正在进行（独立全屏窗开着）。
    pub region: bool,
    /// 屏幕注释 / 白板模式中。
    pub annotate: bool,
    /// 存活中的贴图窗数量（**不影响穿透**，只影响 Esc 语义）。
    pub pins: usize,
    /// ★ 贴图窗里有**选中**的那一张（最后一次左键点中的）。
    ///
    /// 用户口径（2026-09-17）：Esc 只关选中的那一张。一张都没选中时 Esc 不归贴图管
    /// ——否则"随手按个 Esc"会把所有贴图一次清空（用户报的正是这个）。
    pub pin_selected: bool,
}

/// Esc 该关谁（优先级表的输出）。
///
/// 表本身在 [`LayerSession::escape_target`]，**只有那一个出处**——散在多处写
/// `if drawing && !panel` 是"Esc 关错东西"这类 bug 的唯一来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeTarget {
    /// 文字输入窗开着：Esc 归输入窗（取消本次文字），不动别的。
    TextWindow,
    /// 区域选择窗开着：Esc 取消区域选择。该窗自己有焦点，本就走它自己的按键处理。
    Region,
    /// 控制面板开着：Esc 归面板（windui 关窗）。
    Panel,
    /// 标注模式中：Esc 退出标注（墨迹保留）。
    Annotate,
    /// ★ 其余都不占：Esc 关掉**被选中的那一张**贴图窗（不是全部）。
    Pins,
    /// 没有任何叠层在消耗 Esc。
    None,
}

impl LayerSession {
    /// 叠层活跃 ⇒ 覆盖层必须**不穿透**（用户要求：截图 / 屏幕注释不支持鼠标穿透）。
    pub fn active(&self) -> bool {
        self.region || self.annotate
    }

    /// 覆盖层当前的穿透态。
    ///
    /// `own_window_open` 这个例外**必须保留**（`Runtime::panel_open() || Runtime::gif_trim_open()`）：
    /// 面板与裁剪窗都是**独立顶层窗口**（没设 `Window::owned`），而覆盖层是全区最顶层，不让开
    /// 就把它盖住、点不动。两者打开期间标注输入都已整体挂起（`Runtime::input_suspended`），
    /// 所以让开穿透不会在点它们的时候顺手画出一笔。
    ///
    /// ★ 裁剪窗是 2026-09-20 夜补进来的：它原先靠 `HWND_TOPMOST` 压在覆盖层之上（也就压住了
    ///   自己的文件对话框，见《踩坑记录》§2.7），改成不置顶之后就必须由这里给它让路——**这是同一次
    ///   改动的两半，改一半就会从"对话框点不到"变成"窗口点不到"**。
    pub fn click_through(&self, own_window_open: bool) -> bool {
        !self.active() || own_window_open
    }

    /// Esc 的目标（优先级从高到低，见 [`EscapeTarget`] 各变体的注释）。
    pub fn escape_target(&self, panel_open: bool, text_window_open: bool) -> EscapeTarget {
        if text_window_open {
            EscapeTarget::TextWindow
        } else if self.region {
            EscapeTarget::Region
        } else if panel_open {
            EscapeTarget::Panel
        } else if self.annotate {
            EscapeTarget::Annotate
        } else if self.pin_selected {
            // ★ 只有"选中了某一张"时 Esc 才归贴图管（用户口径：Esc 只取消我选中的那张）。
            //   一张都没选中 → 落到 `None`：用户的 Esc 归他自己，不被我们顺手清空贴图。
            EscapeTarget::Pins
        } else {
            EscapeTarget::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scr(x: i32, y: i32, w: i32, h: i32) -> ScreenRect {
        ScreenRect { x, y, w, h }
    }

    /// 世界 ↔ 窗口往返恒等（几何换算的第一性要求）。
    #[test]
    fn world_window_roundtrip_is_exact() {
        for dpi in [1.0_f32, 1.25, 1.5, 2.0] {
            let m = WorldMap::with_dpi(scr(-1920, -100, 3840, 1180), dpi);
            for (wx, wy) in [(0.0, 0.0), (-1920.0, -100.0), (1234.0, 567.0)] {
                let (lx, ly) = m.to_window(wx, wy);
                let (bx, by) = m.to_world(lx, ly);
                assert!((bx - wx).abs() < 1e-3, "dpi={dpi} x 往返错：{wx} → {bx}");
                assert!((by - wy).abs() < 1e-3, "dpi={dpi} y 往返错：{wy} → {by}");
            }
        }
    }

    /// `Window::new(w,h)` 收的是**逻辑 dp**，所以整屏窗的尺寸必须是物理 / dpi。
    /// 旧代码把物理像素直接传进去，150% 下窗口大出 1.5 倍。
    #[test]
    fn logical_size_divides_by_dpi() {
        let phys = scr(0, 0, 1920, 1080);
        assert_eq!(WorldMap::with_dpi(phys, 1.0).logical_size(), (1920, 1080));
        assert_eq!(WorldMap::with_dpi(phys, 1.5).logical_size(), (1280, 720));
        assert_eq!(WorldMap::with_dpi(phys, 2.0).logical_size(), (960, 540));
    }

    /// 负原点（副屏在主屏左侧）不得被钳到 0。
    #[test]
    fn negative_origin_is_preserved() {
        let m = WorldMap::with_dpi(scr(-1920, -100, 3840, 1180), 1.0);
        let (x, y) = m.to_window(-1920.0, -100.0);
        assert_eq!((x, y), (0.0, 0.0));
        let (x, y) = m.to_window(0.0, 0.0);
        assert_eq!((x, y), (1920.0, 100.0));
        assert_eq!(m.logical_size(), (3840, 1180));
    }

    /// 非 100% DPI 下，逻辑坐标点 (100,100) 必须落在物理 (150,150)。
    #[test]
    fn scaled_point_maps_to_the_right_physical_pixel() {
        let m = WorldMap::with_dpi(scr(0, 0, 2560, 1440), 1.5);
        let (wx, wy) = m.to_world(100.0, 100.0);
        assert_eq!((wx, wy), (150.0, 150.0));
    }

    /// **穿透判据**：叠层活跃 ⇒ 不穿透；我们自己的独立顶层窗（面板 / 裁剪窗）开着是唯一例外；
    /// 贴图窗不参与。
    #[test]
    fn click_through_follows_the_layer_state() {
        let idle = LayerSession::default();
        assert!(idle.click_through(false));
        assert!(idle.click_through(true));

        let annotate = LayerSession {
            annotate: true,
            ..Default::default()
        };
        assert!(!annotate.click_through(false), "标注中绝不能穿透");
        assert!(
            annotate.click_through(true),
            "面板 / 裁剪窗开着时的例外必须保留 —— 覆盖层是全屏最顶层，不让开就把它们盖住、点不动"
        );

        let region = LayerSession {
            region: true,
            ..Default::default()
        };
        assert!(!region.click_through(false), "区域选择中绝不能穿透");
        assert!(
            region.click_through(true),
            "区域选择中打开裁剪窗，覆盖层同样要让路（否则裁剪窗被框选层盖住）"
        );

        let pins = LayerSession {
            pins: 3,
            ..Default::default()
        };
        assert!(
            pins.click_through(false),
            "贴图窗是独立子窗，不影响覆盖层穿透"
        );
        assert!(!pins.active());
    }

    /// ★ 有贴图但**没选中**时 Esc 不归贴图管（用户报的 bug：Esc 把所有贴图都关了）。
    #[test]
    fn escape_ignores_pins_when_none_is_selected() {
        let unselected = LayerSession {
            pins: 3,
            pin_selected: false,
            ..Default::default()
        };
        assert_eq!(
            unselected.escape_target(false, false),
            EscapeTarget::None,
            "没选中任何一张 → Esc 不该被贴图吃掉"
        );
        let selected = LayerSession {
            pin_selected: true,
            ..unselected
        };
        assert_eq!(
            selected.escape_target(false, false),
            EscapeTarget::Pins,
            "选中了一张 → Esc 关它"
        );
    }

    /// Esc 优先级表逐行核对。
    #[test]
    fn escape_priority_table() {
        let all = LayerSession {
            region: true,
            annotate: true,
            pins: 2,
            pin_selected: true,
        };
        assert_eq!(all.escape_target(false, true), EscapeTarget::TextWindow);
        assert_eq!(all.escape_target(true, false), EscapeTarget::Region);
        assert_eq!(
            LayerSession {
                region: false,
                ..all
            }
            .escape_target(true, false),
            EscapeTarget::Panel
        );
        assert_eq!(
            LayerSession {
                region: false,
                ..all
            }
            .escape_target(false, false),
            EscapeTarget::Annotate
        );
        assert_eq!(
            LayerSession {
                region: false,
                annotate: false,
                ..all
            }
            .escape_target(false, false),
            EscapeTarget::Pins
        );
        assert_eq!(
            LayerSession::default().escape_target(false, false),
            EscapeTarget::None
        );
    }
}
