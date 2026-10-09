//! 区域选择窗的**鼠标放大镜**：把光标附近那一小块按整数倍放大，贴在光标旁边。
//!
//! 框选一块区域时，屏幕上真正决定"差一个像素"的东西是**边界压在哪个像素上** —— 文字
//! 的基线、控件的 1px 描边、两张图的接缝。原生分辨率下这些点肉眼分不清，所以截图工具
//! 普遍在光标旁配一个放大镜（同类截图工具的通行做法，也是用户点名要的形态）。
//!
//! # ★ 为什么复用 `platform::pip_window`（而不是画在区域选择窗上）
//!
//! 因为**画中画已经把这件事做完了**：`PipWindow` 就是一个"把源图按整数倍放大铺满一块
//! 面板、带边框、带准星、自己置顶、且不会把自己抓进源图"的原生分层窗。区域选择窗要的
//! 东西与它一模一样，唯一的区别是**源**：画中画的源是"现抓屏幕"，这里的是"打开选区时
//! 冻住的那一帧"。
//!
//! 而"画在区域选择窗的画布上"这条路反而更难走：windui 的 `Canvas::draw_image` 没有
//! 源矩形参数（只能整图铺一个矩形），要在画布上放大一小块，就得每个光标位置新建一张裁
//! 剪好的 `WImage` —— 那是在热路径上反复建/毁 GDI 位图资源（见 `platform::pip_window`
//! 里 `Panel` 为什么缓存缓冲）。独立窗这条路上，缓冲由 `PipWindow` 自己按尺寸缓存。
//!
//! # 与"画中画/全屏放大"的关系
//!
//! 共用底层窗与几何助手（[`crate::modules::magnifier::sample_rect`]），但**不共用配置**：
//! 画中画的面板尺寸/倍率/边框色是用户可调的（面板上限 50% 屏），拿来当框选时的放大镜会
//! 大到盖住选区；这里要的是一块固定的小取景器。所以面板尺寸与倍率是本地常量。
//!
//! # 几何
//!
//! ```text
//! 面板   = PANEL_W × PANEL_H          （物理像素，恒 210×154）
//! 取样区 = 固定 15×11 源像素          （与显示器无关，CELLS_W/CELLS_H）
//! 倍率   = 面板 ÷ 取样 = 14           （CELL_PX，逐源像素画格线）
//! 位置   = 光标右下方 GAP 处；贴边时翻到左/上
//! 准星   = 光标那一个像素在面板里的位置；所在格描纯黑边
//! ```

use std::time::{Duration, Instant};

use crate::core::color::Argb;
use crate::core::image::{RgbaBitmap, Shot};
use crate::modules::magnifier::sample_rect;
use crate::platform::display::ScreenRect;
use crate::platform::pip_window::{PipLook, PipWindow};

/// 窗口标题。
///
/// ★ 进 `TopmostKeeper::guard_aux` 的名单（见 `runtime::overlay`）：它自带
///   `WS_EX_TOPMOST`，但"在置顶带里"与"在置顶带最上面"是两件事——不守的话，任何后来
///   被抬起的置顶窗都会把它盖住，表现就是"放大镜时有时无"。
pub const REGION_MAG_TITLE: &str = "CurSeen 截图放大镜";

/// 取样区固定的**源像素数**（用户口径 2026-10-02："固定的像素放大，无论在什么显示器上
/// 都如此"；同日追加"宽 15 高 11，显示屏大部分都是宽的"）。与显示器分辨率/DPI 完全
/// 无关——放大镜看的永远是光标周围 15×11 个**源像素**。
const CELLS_W: i32 = 15;
const CELLS_H: i32 = 11;

/// 每个源像素在面板上占的物理像素（用户口径 2026-10-02："单元格可以稍微小一点点"，
/// 从首版的 16 收到 14）→ 面板恒为 210×154 物理像素。格线由 `PipLook::grid` 逐源像素
/// 均分画出，天然对齐。
const CELL_PX: i32 = 14;

/// 面板尺寸（物理像素）。
const PANEL_W: i32 = CELLS_W * CELL_PX;
const PANEL_H: i32 = CELLS_H * CELL_PX;

/// 面板与光标之间的间隙（物理像素）：留出一点，免得面板外框压住光标本身。
const GAP: f32 = 26.0;

/// 面板边框宽度（物理像素）+ 颜色。
const BORDER_W: i32 = 2;
/// 边框用**主色蓝**（与选区边界、压暗遮罩同源），不跟画中画走用户配置：
/// 这里是"截图界面的一个部件"，与截图界面的其余部件同色才像一家的。
const ACCENT: (u8, u8, u8) = (76, 139, 245);

/// 同一个像素位置的最短重出画间隔。
///
/// 鼠标移动事件可以到几百 Hz，而每次出画 = 一次小面积 GDI 合成提交
/// （`StretchDIBits` + `UpdateLayeredWindow`）。10ms 的地板把出画次数压在 100 Hz 以内，
/// 代价只是"光标停下来那一刻可能差最后一两个像素"——而像素级去重（见
/// [`RegionMagnifier::update`] 的第一道门）会在下一次移动时立刻补上。
const MIN_INTERVAL: Duration = Duration::from_millis(10);

/// 区域选择窗的放大镜。
///
/// 生命周期绑在**一次区域选择**上：`Runtime` 持有一个实例（`Drop` 会销毁窗），
/// 每次确认/取消都显式 [`hide`](Self::hide)。它在屏上是一块独立置顶窗，**不会**随区域
/// 选择窗一起消失 —— 漏掉隐藏就是"框选结束了放大镜还挂在屏幕上"。
pub struct RegionMagnifier {
    window: PipWindow,
    /// 上次出画对应的整数光标位置（像素级去重，见 [`Self::update`]）。
    shown_key: Option<(i32, i32)>,
    /// 上次出画时刻（时间地板）。
    shown_at: Option<Instant>,
}

impl RegionMagnifier {
    pub fn new() -> Self {
        Self {
            window: PipWindow::with_title(REGION_MAG_TITLE),
            shown_key: None,
            shown_at: None,
        }
    }

    /// 面板是否真的在屏上。
    pub fn is_shown(&self) -> bool {
        self.window.is_visible()
    }

    /// 收起来（保留窗与缓冲）：确认/取消/关窗时调，以及每次开新窗之前调一次。
    pub fn hide(&mut self) {
        self.window.hide();
        self.shown_key = None;
        self.shown_at = None;
    }

    /// 两道节流门：同一个像素位置不重复出画；同一时刻最多 [`MIN_INTERVAL`] 一次。
    ///
    /// 返回 `true` = **这次被拦下了**。被拦下的那一次不算出画，于是下一次移动会立刻补上
    /// 当前真实位置。
    fn throttled(&self, cursor: (f32, f32)) -> bool {
        let key = (cursor.0.floor() as i32, cursor.1.floor() as i32);
        if self.shown_key == Some(key) {
            return true;
        }
        self.shown_at.is_some_and(|t| t.elapsed() < MIN_INTERVAL)
    }

    /// 取样区：以光标为中心、钳进虚拟屏（`sample_rect` 是画中画/全屏放大共用的那一个）。
    ///
    /// ★ 固定 15×11 源像素（[`CELLS_W`]/[`CELLS_H`]）——不是"面板÷倍率"：放大镜的职责是
    ///   数清光标周围那几个像素，取样窗口恒定，倍率才恒等于 [`CELL_PX`]（面板÷取样）。
    fn sample_of(cursor: (f32, f32), screen: ScreenRect) -> ScreenRect {
        sample_rect(cursor, CELLS_W as u32, CELLS_H as u32, screen)
    }

    /// 出画一次（[`update`](Self::update) / [`update_live`](Self::update_live) 共用的后半段）。
    ///
    /// 先记状态再提交：提交失败（无桌面会话等）时不必无限重试同一帧，
    /// 下一次光标移动自然会再来一次。
    fn draw(
        &mut self,
        shot: &Shot,
        sample: ScreenRect,
        cursor: (f32, f32),
        screen: ScreenRect,
    ) -> bool {
        let panel = magnifier_panel(cursor, PANEL_W, PANEL_H, screen);
        let look = PipLook {
            border: Argb::new(255, ACCENT.0, ACCENT.1, ACCENT.2),
            border_width: BORDER_W,
            // 准星**就是放大镜的结论**：它指出"光标这一个像素"在面板的哪个格子上。
            // 画中画那条链路把这个能力关掉了（用户口径"完全不需要这个"），这里正是它
            // 当初存在的理由。配合 `grid`，准星所在的那一格还会被描一圈纯黑边。
            crosshair_at: Some(crosshair_in_panel(sample, panel, cursor)),
            // ★ 像素格（用户口径 2026-10-02："要绘制像素格子……一眼看出来是中心"）：
            //   15×11 源像素逐格画浅色线，中心格纯黑描边。
            grid: Some((CELLS_W, CELLS_H)),
        };
        self.shown_key = Some((cursor.0.floor() as i32, cursor.1.floor() as i32));
        self.shown_at = Some(Instant::now());
        self.window.present(shot, panel, &look).is_ok()
    }

    /// 用**冻结帧**刷新放大镜（截图 / GIF 的**区域选择**阶段）。
    ///
    /// - `frame`：打开区域选择窗时冻住的整虚拟桌面位图（RGBA）。
    /// - `cursor`：光标（虚拟桌面物理像素）。
    /// - `screen`：虚拟桌面矩形（冻结帧的原点就是它）。
    ///
    /// 返回"这次真的出画了没有"。
    pub fn update(&mut self, frame: &RgbaBitmap, cursor: (f32, f32), screen: ScreenRect) -> bool {
        if self.throttled(cursor) {
            return false;
        }
        let sample = Self::sample_of(cursor, screen);
        // 冻结帧的坐标原点 = 虚拟桌面原点，所以取样矩形要减去它才是帧内偏移。
        let Some(shot) =
            frame.crop_to_shot(sample.x - screen.x, sample.y - screen.y, sample.w, sample.h)
        else {
            self.hide();
            return false;
        };
        self.draw(&shot, sample, cursor, screen)
    }

    /// 用**实时抓屏**刷新放大镜（GIF 选区**调整**阶段）。
    ///
    /// 与 [`update`](Self::update) 只差一个"源"：那边是"打开选区时冻住的那一帧"，这边是
    /// **每次现抓**光标附近那一小块。GIF 调整阶段屏幕是活的（用户正对着它做精细对位），
    /// 冻结帧在这里没有意义；而且不需要整屏抓 —— 取样区只有 15×11 个源像素，一次
    /// `BitBlt` 的成本可忽略（对比：整屏抓是 1920×1080）。
    pub fn update_live(&mut self, cursor: (f32, f32), screen: ScreenRect) -> bool {
        if self.throttled(cursor) {
            return false;
        }
        let sample = Self::sample_of(cursor, screen);
        let Ok(shot) = crate::platform::screenshot::grab(sample) else {
            self.hide();
            return false;
        };
        self.draw(&shot, sample, cursor, screen)
    }
}

impl Default for RegionMagnifier {
    fn default() -> Self {
        Self::new()
    }
}

/// 放大镜面板的位置：默认贴在光标**右下**，右边/下边放不下就翻到左/上，最后钳进屏内。
///
/// ★ 为什么不把面板中心对准光标（画中画的"鼠标跟随"锚点就是那样）：放大镜的全部意义是
///   "看清光标正要落在哪个像素上"，面板压住光标等于把答案盖住了。贴边的两条翻转让面板
///   在屏幕角落也能待在"离光标最近的空处"。
fn magnifier_panel(cursor: (f32, f32), pw: i32, ph: i32, screen: ScreenRect) -> ScreenRect {
    let (x0, y0) = (screen.x as f32, screen.y as f32);
    let (x1, y1) = (x0 + screen.w as f32, y0 + screen.h as f32);
    let (pw_f, ph_f) = (pw as f32, ph as f32);

    let mut x = cursor.0 + GAP;
    if x + pw_f > x1 {
        x = cursor.0 - GAP - pw_f;
    }
    let mut y = cursor.1 + GAP;
    if y + ph_f > y1 {
        y = cursor.1 - GAP - ph_f;
    }
    // 屏比面板还小（或光标贴死角落）时钳住：宁可压住边角，也不能让面板跑到屏外去
    // ——跑去屏外的那部分 `UpdateLayeredWindow` 会直接不显示，看着像"放大镜缺了一块"。
    ScreenRect::new(
        x.clamp(x0, (x1 - pw_f).max(x0)).round() as i32,
        y.clamp(y0, (y1 - ph_f).max(y0)).round() as i32,
        pw,
        ph,
    )
}

/// 光标那一个像素在面板里的位置（面板局部坐标，物理像素）。
///
/// 有效倍率从**实际**取样区与面板尺寸算（`StretchDIBits` 用的就是这个比值），所以取样区
/// 被 `sample_rect` 钳过（贴屏幕边时不居中）也不会让准星漂走。
fn crosshair_in_panel(sample: ScreenRect, panel: ScreenRect, cursor: (f32, f32)) -> (i32, i32) {
    let zx = panel.w as f32 / sample.w.max(1) as f32;
    let zy = panel.h as f32 / sample.h.max(1) as f32;
    (
        ((cursor.0 - sample.x as f32) * zx).round() as i32,
        ((cursor.1 - sample.y as f32) * zy).round() as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: ScreenRect = ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    /// 屏幕中央：面板贴在光标右下，且**一个像素都不压住光标**。
    #[test]
    fn panel_sits_below_right_of_the_cursor_without_covering_it() {
        let p = magnifier_panel((500.0, 400.0), PANEL_W, PANEL_H, S);
        assert_eq!((p.x, p.y), ((500.0 + GAP) as i32, (400.0 + GAP) as i32));
        assert!(
            p.x > 500 && p.y > 400,
            "面板必须在光标右下方（压住光标就等于盖住答案）"
        );
        assert_eq!((p.w, p.h), (PANEL_W, PANEL_H));
    }

    /// ★ 贴边翻转：右下放不下就翻到左/上，两个方向各自独立。
    #[test]
    fn panel_flips_when_it_would_leave_the_screen() {
        // 贴右下角：两个方向都翻
        let p = magnifier_panel((1910.0, 1070.0), PANEL_W, PANEL_H, S);
        assert!(
            p.x + p.w < 1910 && p.y + p.h < 1070,
            "右下角应翻到光标的左上方：{p:?}"
        );
        // 只贴右边
        let p = magnifier_panel((1910.0, 300.0), PANEL_W, PANEL_H, S);
        assert!(p.x + p.w < 1910, "右边放不下应翻到左侧");
        assert!(p.y > 300, "下边放得下就仍留在下方");
        // 只贴下边
        let p = magnifier_panel((300.0, 1070.0), PANEL_W, PANEL_H, S);
        assert!(p.x > 300, "右边放得下就仍留在右侧");
        assert!(p.y + p.h < 1070, "下边放不下应翻到上方");
    }

    /// 面板恒在屏内（跑去屏外的部分 `UpdateLayeredWindow` 不显示，看着像缺一块）。
    #[test]
    fn panel_never_leaves_the_screen() {
        let s = ScreenRect::new(-1920, 0, 1920, 1080);
        for cursor in [
            (-1920.0, 0.0),
            (-1.0, 1079.0),
            (-960.0, 540.0),
            (-5000.0, -5000.0),
        ] {
            let p = magnifier_panel(cursor, PANEL_W, PANEL_H, s);
            assert!(p.x >= s.x && p.y >= s.y, "面板左上角出界: {p:?}");
            assert!(
                p.x + p.w <= s.x + s.w && p.y + p.h <= s.y + s.h,
                "面板右下角出界: {p:?}"
            );
        }
        // 屏比面板还小：仍然钳在屏内，不越界。
        let tiny = ScreenRect::new(0, 0, 100, 80);
        let p = magnifier_panel((50.0, 40.0), PANEL_W, PANEL_H, tiny);
        assert_eq!((p.x, p.y), (0, 0));
    }

    /// ★ 准星落在光标那一个像素对应的格子上。
    ///
    /// 取样 15×11 是**奇数×奇数**：`sample_rect` 取整居中后光标未必恰在面板正中（偏半格），
    /// 所以判据不是"等于面板中心"，而是"等于映射公式的精确值"（与被钳用例同一口径）。
    #[test]
    fn crosshair_marks_the_cursor_pixel() {
        let cursor = (960.0, 540.0);
        let sample = sample_rect(cursor, CELLS_W as u32, CELLS_H as u32, S);
        let panel = magnifier_panel(cursor, PANEL_W, PANEL_H, S);
        let (cx, cy) = crosshair_in_panel(sample, panel, cursor);
        let zx = panel.w as f32 / sample.w as f32;
        let zy = panel.h as f32 / sample.h as f32;
        assert_eq!(
            cx,
            ((cursor.0 - sample.x as f32) * zx).round() as i32,
            "准星横向必须落在光标像素的格子上"
        );
        assert_eq!(
            cy,
            ((cursor.1 - sample.y as f32) * zy).round() as i32,
            "准星纵向必须落在光标像素的格子上"
        );
        // 中心格描边判据：光标格 = 准星 ÷ CELL_PX，必须在 15×11 网格内。
        let col = cx * CELLS_W / PANEL_W;
        let row = cy * CELLS_H / PANEL_H;
        assert!((0..CELLS_W).contains(&col) && (0..CELLS_H).contains(&row));
        assert!((0..PANEL_W).contains(&cx) && (0..PANEL_H).contains(&cy));
    }

    /// 取样区被屏幕边界钳过（不居中）时，准星仍要指回光标那个像素：
    /// 判据是"准星到面板中心的偏移 = 光标到取样区中心的偏移 × 有效倍率"。
    #[test]
    fn crosshair_still_points_at_the_cursor_when_the_sample_is_clamped() {
        let cursor = (2.0, 1.0); // 贴左上角 → 取样区必然被钳
        let (cw, ch) = (CELLS_W, CELLS_H);
        let sample = sample_rect(cursor, cw as u32, ch as u32, S);
        assert_eq!((sample.x, sample.y), (0, 0), "左上角处取样区应被钳到原点");
        let panel = magnifier_panel(cursor, PANEL_W, PANEL_H, S);
        let (cx, cy) = crosshair_in_panel(sample, panel, cursor);
        let zx = PANEL_W as f32 / sample.w as f32;
        let zy = PANEL_H as f32 / sample.h as f32;
        assert_eq!(cx, (cursor.0 * zx).round() as i32);
        assert_eq!(cy, (cursor.1 * zy).round() as i32);
        assert!((0..PANEL_W).contains(&cx) && (0..PANEL_H).contains(&cy));
    }

    /// 关掉后状态清干净（下次开新窗不会因为"跟上一次同一个像素"而拒绝出画）。
    #[test]
    fn hide_clears_the_dedupe_state() {
        let mut m = RegionMagnifier::new();
        m.shown_key = Some((10, 20));
        m.shown_at = Some(Instant::now());
        m.hide();
        assert!(m.shown_key.is_none() && m.shown_at.is_none());
        assert!(!m.is_shown());
    }

    /// 没有像素的冻结帧（或裁不出东西）时不许把窗留在屏上。
    #[test]
    fn empty_frame_keeps_the_window_hidden() {
        let mut m = RegionMagnifier::new();
        let empty = RgbaBitmap {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        };
        assert!(!m.update(&empty, (500.0, 400.0), S));
        assert!(!m.is_shown());
    }

    /// ★ 两道节流门的判据（`update` / `update_live` 共用）——**两道都要单独钉住**：
    /// 同一个像素不出画；且同一时刻最多 [`MIN_INTERVAL`] 一次。
    ///
    /// 取 `floor` 之后的整数比对：亚像素抖动不该触发一次抓屏 + 一次 GDI 提交。
    /// 反过来，**换了像素但时间没到仍要拦下** —— 只测"换像素就放行"会让这条用例
    /// 在时间门被删掉之后依然全绿（假绿）。
    #[test]
    fn throttled_blocks_the_same_pixel_and_allows_a_new_one() {
        let mut m = RegionMagnifier::new();
        assert!(!m.throttled((500.0, 400.0)), "没出过画，第一次必须放行");

        // ① 像素门：floor 之后仍是 (500,400) → 拦下。
        m.shown_key = Some((500, 400));
        m.shown_at = Some(Instant::now() - MIN_INTERVAL * 2);
        assert!(
            m.throttled((500.4, 400.9)),
            "floor 后还是 (500,400)，即使过了时间地板也要被拦下"
        );

        // ② 时间门：换了像素，但离上次出画不足 MIN_INTERVAL → 仍拦下。
        m.shown_at = Some(Instant::now());
        assert!(
            m.throttled((501.0, 400.0)),
            "换了像素但时间地板未过，仍要拦下"
        );

        // ③ 两道门都过 → 放行（这是「下一次移动立刻补上」的保证）。
        m.shown_at = Some(Instant::now() - MIN_INTERVAL * 2);
        assert!(
            !m.throttled((501.0, 400.0)),
            "换了像素且过了时间地板，必须放行"
        );
    }

    /// 取样区跟着光标走、且恒被钳进屏内（`update_live` 就是拿它去抓屏的）。
    #[test]
    fn sample_follows_the_cursor_and_stays_on_screen() {
        let a = RegionMagnifier::sample_of((960.0, 540.0), S);
        assert_eq!((a.w, a.h), (CELLS_W, CELLS_H), "取样区恒为 15×11 源像素");
        let b = RegionMagnifier::sample_of((980.0, 540.0), S);
        assert_eq!(b.x - a.x, 20, "光标右移 20，取样区应跟着右移 20");

        let corner = RegionMagnifier::sample_of((0.0, 0.0), S);
        assert_eq!((corner.x, corner.y), (0, 0), "左上角处应被钳到原点");
        assert!(corner.w > 0 && corner.h > 0, "被钳过也必须有像素可抓");
    }
}
