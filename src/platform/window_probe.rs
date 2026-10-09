//! 智能窗口检测：**光标下那个程序窗口的可见外框**，供区域选择窗"推荐选框"。
//!
//! 截图 / GIF 选区都要框一块屏幕，而用户十有八九想框的正是某个窗口。手动拖到"跟窗口
//! 严丝合缝"既费劲又对不准（差一两像素就带出一条桌面边），所以这里把窗口外框先算出来，
//! 让用户**单击**即可选中（Windows 自带截图工具的"贴靠辅助"是同一件事）。
//!
//! # ★ 为什么不用 `WindowFromPoint`
//!
//! 因为调用它的那一刻，**我们自己的区域选择窗正铺满整个虚拟屏**（`layer::region` 建的
//! 无边框全屏窗）。`WindowFromPoint` 返回的是"这个点上会收到鼠标的那个窗"，于是恒返回
//! 我们自己的窗，永远看不到底下的目标。`WS_EX_TRANSPARENT` 之类也救不了：那是"穿透"，
//! 不是"这一层不算数"。
//!
//! 所以走**枚举 + 命中**：`EnumWindows` 按 z 序**自顶向下**给出全部顶层窗，逐层判"外框
//! 是否含着光标"，第一个含住的就是用户眼里的那个。判据只依赖列表顺序，与我们自己盖了
//! 多少层、盖在哪无关 —— 代价是每层都要问一次外框（见下）。
//!
//! # ★★★ 外框怎么取：`EXTENDED_FRAME_BOUNDS`，**原样，不内缩**
//!
//! ## 第一步：不能用 `GetWindowRect`
//!
//! `GetWindowRect` 在 Win10/11 上会把**不可见的缩放边框**算进去（本机实测差 `L7 T0 R7 B7`），
//! 拿它当选框会**胖出一圈桌面**；`DWMWA_EXTENDED_FRAME_BOUNDS` 才是 DWM 真正画出来的可见外框。
//!
//! ## 第二步：`EXT` 就是"含边框线"的那一圈 —— 曾内缩过，**2026-09-23 用户实测推翻**
//!
//! 真机实测（`temp/verify_fit_border.py` + `temp/probe_border_alpha.py`，探针窗 = 纯绿满屏
//! 背景上一个 640×420 客户区的窗口）：
//!
//! ```text
//! ① 窗口真实可见边界 == EXT，四边零误差（EXT 642×452，逐值一致）
//! ② EXT 的最外圈 1px = **背景的 60% + 灰(117.5) 的 40%**（反解式 `obs = 0.6*bg + 47`，
//!    绿/品红/红/蓝四色背景逐值吻合）—— 这就是 DWM 给窗口画的那圈**边框线**，
//!    它是半透明的，所以像素值看起来像"被冲淡的背景色"。
//! ③ EXT 之内 1px 已是**不透明**的窗口内容（实测纯白 255,255,255）
//! ```
//!
//! ② 那圈半透明让上一版把它当成"窗口**外**的颜色"剔掉了（`inset(DWMWA_VISIBLE_FRAME_
//! BORDER_THICKNESS)`，本机 = 1px，flashot 也是这么干的）。用户两次实测反馈把这件事钉死了：
//!
//! ```text
//! 不内缩（= EXT 原样）："边框线向外多了 1px … 图片边缘有边框外的颜色"
//! 内缩 1px          ："边框线向内少了 1px … 裁剪太多了，连程序窗口的边框线都没有"
//! ```
//!
//! 两句抱怨指着**同一圈像素**，方向相反；而 ② ③ 说明这圈像素**就是**窗口自己的边框线
//! （DWM 画的，40% 不透明度），不是窗口外的东西 —— 少了它就是每边啃掉 1px 的窗口本体。
//! 所以正解是**照 EXT 原样取**：推荐框的外沿与裁剪图的最外圈都落在**边框线的外沿**上。
//!
//! ⚠️ 代价写在明处：产物最外圈那 1px 是 40% 不透明的边框线，它会透出窗口背后的桌面色
//! （"看着像被冲淡的背景"）。这不是脏像素，是窗口边框本身的性质 —— 想不要它，就只能
//! 连边框一起裁掉（那正是被推翻的那一版）。
//!
//! # 成本
//!
//! 每次探测 = 一轮 `EnumWindows`。回调里先用**便宜的** `GetWindowRect` 做粗筛（可见
//! 外框必是它的子集，所以"粗框不含光标"⇒"一定不命中"），只有粗筛通过的少数窗口才去问
//! 一次 DWM 拿精确外框。调用方（`layer::region`）另有位移 + 时间双重节流，见那里的
//! `PROBE_MIN_PX` / `PROBE_MIN_MS`。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;

use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE,
    GW_OWNER, WS_CAPTION, WS_CHILD, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    WS_SYSMENU, WS_THICKFRAME,
};

use crate::platform::display::ScreenRect;

/// 推荐框的最小边长（物理像素）。比这还小的窗口不值得推荐 —— 系统 helper 里 0×0 与
/// 1×1 的窗口一大把（见 `topmost` 模块头那段实测），它们不是"用户眼里的窗口"。
const MIN_SIDE: i32 = 8;

/// 桌面壳层窗口类：**不推荐**。
///
/// 它们铺满整屏且恒在 z 序最底，只要光标没落在任何窗口上就必然命中它们 —— 于是"推荐"
/// 退化成"永远推荐全屏"，把"悬停一下看看能框哪个窗口"这条信息彻底淹掉。用户想框整屏
/// 仍有两条老路：拖满全屏，或者按 Enter（无选区即整屏）。
const DESKTOP_CLASSES: [&str; 2] = ["Progman", "WorkerW"];

/// 永不推荐的系统窗口类：提示条、阴影、输入法候选、托盘溢出面板等。它们要么是别的窗口
/// 的一部分，要么一闪就没，框下来只会得到一张莫名其妙的图。
const DENY_CLASSES: [&str; 7] = [
    "NotifyIconOverflowWindow",
    "Button",
    "SysShadow",
    "tooltips_class32",
    "#32768",
    "MSCTFIME UI",
    "IME",
];

/// 例外的**系统外壳**：它们本身带 `WS_EX_TOOLWINDOW`（任务栏就有），按工具窗一律排除会
/// 把"想框任务栏"这条正常诉求一起毙掉，所以单独放行。
const SHELL_CLASSES: [&str; 2] = ["Shell_TrayWnd", "Shell_SecondaryTrayWnd"];

/// 我们自己窗口里**值得推荐**的那几类的标题前缀（2026-09-28 用户报"无法识别本体窗口"）。
///
/// 贴图、便签、画中画放大都是"摆在桌面上的一块内容"，用户完全可能想把它们截进图里；
/// 冻结帧（BitBlt）也确实抓到了它们的像素。放行的判据只认**标题前缀**——这几类窗口
/// 的标题都以 `CurSeen ` 开头且种类固定，比认类名/样式稳（贴图是工具窗、便签正文带
/// owner，按样式走会被后面的通用规则毙掉）。
///
/// 不在名单里的自窗（全屏覆盖层、区域选择窗、放大镜、控件条…）照旧排除：
/// 选择窗铺满整屏且恒置顶，不排除的话它永远第一个命中，智能检测等于没有。
/// 控制面板不在这里——它按标题精确匹配放行，见 [`OWN_SELECTABLE_EXACT`]。
const OWN_SELECTABLE_PREFIXES: [&str; 3] = ["CurSeen 贴图", "CurSeen 便签", "CurSeen 画中画放大"];

/// 同上，但按**类名**放行：便签的正文窗标题为空（`sticky.rs` 里 `PCWSTR::null()`），
/// 光标悬在正文上时按标题匹配不到 —— 正文才是便签的大头，必须认类名。
const OWN_SELECTABLE_CLASSES: [&str; 1] = ["CurSeenStickyBodyClass"];

/// 按**标题精确相等**放行（2026-10-08 用户报"选区对 CurSeen 本体窗口不起作用"）：
/// 控制面板的标题就是 `CurSeen`（`ui::panel::TITLE`）。不能用前缀——全仓所有自窗标题
/// 都以 `CurSeen ` 开头（覆盖层 "CurSeen Overlay"、选区窗 "CurSeen 截图区域"…），
/// 前缀匹配等于把全部门放穿。精确相等只有面板一个，用户就是想把自己的软件截进图里
/// （做演示图/文档截图时这是刚需）。
const OWN_SELECTABLE_EXACT: [&str; 1] = ["CurSeen"];

/// 一个候选窗口的**可判据事实**。
///
/// 刻意全部是纯数据（不带 `HWND` / `RECT` 这些 Win32 类型）：判据本身能脱离真机单测，
/// 而"枚举 + 过滤"这段最容易写错的逻辑恰恰就是判据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    /// 可见外框（已钳进虚拟桌面）。
    pub rect: ScreenRect,
    pub class: String,
    /// 窗口标题全文。自窗白名单（[`OWN_SELECTABLE_PREFIXES`]）按前缀匹配用；
    /// 别的判据仍只看 [`Candidate::has_title`]。
    pub title: String,
    /// 有没有标题。用户口径里截图界面上不该有任何文字提示，标题内容不外显。
    pub has_title: bool,
    pub style: u32,
    pub ex_style: u32,
    pub has_owner: bool,
    pub visible: bool,
    pub iconic: bool,
    pub cloaked: bool,
    /// 属于 CurSeen 自己（覆盖层 / 控制面板 / 区域选择窗 / 放大镜 / 控件条…）。
    pub own_process: bool,
}

/// 光标下最上层那个值得推荐的窗口的外框（虚拟桌面物理像素）。
///
/// 没有合格的窗口（桌面、任务栏以外的空处、纯系统 helper…）时返回 `None`。
pub fn detect_at(x: i32, y: i32) -> Option<ScreenRect> {
    let mut scan = Scan {
        x,
        y,
        cands: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(probe_proc),
            LPARAM(std::ptr::from_mut(&mut scan) as isize),
        );
    }
    pick_at(&scan.cands, x, y).map(|c| c.rect)
}

/// DWM 那圈可见边框的**不透明度**（本机实测 `0.4`，见模块头 §第二步）。
///
/// 抓屏拿到的是它**已经与背景合成**的结果：`obs = (1 - a) * bg + a * border`
/// （GDI `BitBlt` 不给半透明，alpha 恒 255，两种抓法都是这个值 —— 见
/// `temp/probe_grab_alpha.py`：黑底上 EXT 那圈 = `(47,47,47,255)`，反解
/// `47 / 0.4 = 117.5 = #767676`，正是 Windows 默认窗口边框色）。
///
/// 已知它，就能把产物最外圈那 1px 从"混了 60% 桌面色的边框"**反解回不透明本色**：
/// `border = (obs - (1 - a) * bg) / a`。
pub const FRAME_BORDER_ALPHA: f32 = 0.4;

/// 这个矩形是否**逐值等于**某个可选窗口的可见外框。
///
/// 用途：确认截图时判"这一刀是不是正好贴着某个窗口切"。是的话产物最外圈 1px 就是那圈
/// 半透明边框线，值得按 [`FRAME_BORDER_ALPHA`] 反解成不透明（否则边缘会混进 60% 桌面色
/// —— 用户报了三次的"多了 1px"）。
///
/// ★ 只认**别人的窗**：我们自己的区域选择窗此刻正铺满整个虚拟桌面，用户截全屏时它的
/// 外框恰好等于选区 —— 不排除 `own_process` 就会把"截全屏"误判成"贴窗口边框"。
///
/// 比对用 `clip_to_desktop` 后的外框，与 [`candidate_of`] 给推荐框的口径一致（窗口可以
/// 有一半在屏外，推荐框会被钳，这里必须同款钳法才能对上）。
pub fn rect_is_window_frame(want: ScreenRect) -> bool {
    let mut scan = FrameScan { want, hit: false };
    unsafe {
        let _ = EnumWindows(
            Some(frame_proc),
            LPARAM(std::ptr::from_mut(&mut scan) as isize),
        );
    }
    scan.hit
}

/// [`rect_is_window_frame`] 的枚举状态。
struct FrameScan {
    want: ScreenRect,
    hit: bool,
}

/// # Safety
/// 仅作为 `EnumWindows` 的回调使用，`lparam` 必须指向存活到调用结束的 [`FrameScan`]。
unsafe extern "system" fn frame_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = unsafe { &mut *(lparam.0 as *mut FrameScan) };
    if scan.hit {
        return BOOL(0); // 已经找到了，提前收工
    }
    // 粗筛：尺寸对不上就不必问 DWM（逐值相等 ⇒ 尺寸必然相等）。
    let mut coarse = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut coarse) }.is_err() {
        return BOOL(1);
    }
    // 钳进虚拟桌面后还比 want 小 ⇒ 不可能是同一个矩形（比对的是钳过的外框）。
    if coarse.right - coarse.left < scan.want.w || coarse.bottom - coarse.top < scan.want.h {
        return BOOL(1);
    }
    let c = match unsafe { candidate_of(hwnd, GetCurrentProcessId()) } {
        Some(c) => c,
        None => return BOOL(1),
    };
    if c.own_process || !is_selectable(&c) {
        return BOOL(1);
    }
    if c.rect.x == scan.want.x
        && c.rect.y == scan.want.y
        && c.rect.w == scan.want.w
        && c.rect.h == scan.want.h
    {
        scan.hit = true;
    }
    BOOL(1)
}

/// 探测状态（`EnumWindows` 回调的 `lparam` 指向它）。
struct Scan {
    x: i32,
    y: i32,
    /// z 序自上而下收集到的候选（只装"粗框含光标"的窗口，通常只有一两个）。
    cands: Vec<Candidate>,
}

/// # Safety
/// 仅作为 `EnumWindows` 的回调使用，`lparam` 必须指向存活到调用结束的 [`Scan`]。
unsafe extern "system" fn probe_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = unsafe { &mut *(lparam.0 as *mut Scan) };
    // 粗筛：可见外框一定落在 `GetWindowRect` 之内，所以粗框不含光标就一定不命中。
    // 这一步把一个点上"绝大多数窗口"挡在外面，省掉它们各自一次 DWM 调用。
    let mut coarse = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut coarse) }.is_err() {
        return BOOL(1);
    }
    if !(scan.x >= coarse.left
        && scan.x < coarse.right
        && scan.y >= coarse.top
        && scan.y < coarse.bottom)
    {
        return BOOL(1);
    }
    if let Some(c) = unsafe { candidate_of(hwnd, GetCurrentProcessId()) } {
        scan.cands.push(c);
    }
    BOOL(1)
}

/// 取一个窗口的可判据事实。任何一步拿不到关键信息就返回 `None`（当作不可推荐）。
///
/// # Safety
/// `hwnd` 必须是有效的顶层窗口句柄（枚举回调给的都满足）。
unsafe fn candidate_of(hwnd: HWND, own_pid: u32) -> Option<Candidate> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let own_process = pid == own_pid;

    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let visible = unsafe { IsWindowVisible(hwnd) }.as_bool();

    // 外框：先 DWM 的可见外框，拿不到再退 `GetWindowRect`。
    // ★ `EXT` **原样使用，不内缩**（见模块头）：它的最外圈就是 DWM 画给窗口的边框线，
    //   内缩 = 把边框线一起裁掉（用户实测："裁剪太多了，连程序窗口的边框线都没有"）。
    let mut r = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            std::ptr::from_mut(&mut r).cast(),
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .is_ok()
        && r.right > r.left
        && r.bottom > r.top;
    if !ok && unsafe { GetWindowRect(hwnd, &mut r) }.is_err() {
        return None;
    }
    let rect = clip_to_desktop(r)?;

    // 标题全文：只有自窗白名单消费它，但还是统一读（有标题的窗口一次 GetWindowText，
    // 没标题的返回空串，成本可忽略）。
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let mut buf = vec![0u16; (len as usize + 1).max(1)];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    let title = OsString::from_wide(&buf[..n.max(0) as usize])
        .to_string_lossy()
        .into_owned();

    Some(Candidate {
        rect,
        class: unsafe { class_name(hwnd) },
        title,
        has_title: len > 0,
        style,
        ex_style,
        has_owner: unsafe { GetWindow(hwnd, GW_OWNER) }
            .map(|o| !o.0.is_null())
            .unwrap_or(false),
        visible,
        iconic: unsafe { IsIconic(hwnd) }.as_bool(),
        cloaked: unsafe { is_cloaked(hwnd) },
        own_process,
    })
}

/// `DWMWA_CLOAKED`：UWP 的"幽灵窗"——在 Alt-Tab 与 z 序里都看不见，但 `IsWindowVisible`
/// 仍报真。不放行的话，悬停在某个 UWP 应用上会推荐出一块**不存在**的框。
unsafe fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            std::ptr::from_mut(&mut cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    }
    .is_ok();
    ok && cloaked != 0
}

unsafe fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return String::new();
    }
    OsString::from_wide(&buf[..n as usize])
        .to_string_lossy()
        .into_owned()
}

/// 把外框钳进虚拟桌面：窗口可以有一半在屏幕外（甚至整个在副屏外），而冻结帧只覆盖虚拟
/// 桌面 —— 不钳的话推荐框会伸到抓不到的地方，裁出来的图带着无效区。
fn clip_to_desktop(r: RECT) -> Option<ScreenRect> {
    let vs = crate::platform::display::virtual_screen();
    let x0 = r.left.max(vs.x);
    let y0 = r.top.max(vs.y);
    let x1 = r.right.min(vs.x + vs.w);
    let y1 = r.bottom.min(vs.y + vs.h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(ScreenRect::new(x0, y0, x1 - x0, y1 - y0))
}

/// 这个候选值不值得推荐。
///
/// 顺序刻意"先便宜后昂贵"：绝大多数窗口在前几条就被否掉。
pub(crate) fn is_selectable(c: &Candidate) -> bool {
    if !c.visible || c.iconic || c.cloaked {
        // 最小化的窗口 `GetWindowRect` 会给出"恢复后的位置"，拿它当推荐框是错的地址。
        return false;
    }
    if c.own_process {
        // 自窗默认全排除（区域选择窗铺满整屏、覆盖层恒置顶）：不排除的话，第一个含住
        // 光标的永远是它们自己，智能检测等于没有。但贴图 / 便签 / 画中画是"摆在桌面上
        // 的一块内容"，用户就是想把它们截进来 —— 按标题前缀放行（见常量处注释）。
        // 放行的仍要过最小尺寸这条底线。
        return (OWN_SELECTABLE_PREFIXES
            .iter()
            .any(|p| c.title.starts_with(p))
            || OWN_SELECTABLE_CLASSES.contains(&c.class.as_str())
            || OWN_SELECTABLE_EXACT.contains(&c.title.as_str()))
            && c.rect.w >= MIN_SIDE
            && c.rect.h >= MIN_SIDE;
    }
    if DENY_CLASSES.contains(&c.class.as_str()) || DESKTOP_CLASSES.contains(&c.class.as_str()) {
        return false;
    }
    if c.rect.w < MIN_SIDE || c.rect.h < MIN_SIDE {
        return false;
    }
    // 系统外壳（任务栏）在工具窗规则之前放行。
    if SHELL_CLASSES.contains(&c.class.as_str()) {
        return true;
    }
    if c.style & WS_CHILD.0 != 0 || c.ex_style & WS_EX_TOOLWINDOW.0 != 0 {
        return false;
    }
    if c.ex_style & WS_EX_NOACTIVATE.0 != 0 && c.ex_style & WS_EX_APPWINDOW.0 == 0 {
        return false;
    }
    // 有 owner 的顶层窗是"附属弹层"（下拉、气泡、对话框的辅助窗），关掉宿主就没了。
    // 带 `WS_EX_APPWINDOW` 的除外 —— 那是刻意要求进任务栏的独立窗。
    if c.has_owner && c.ex_style & WS_EX_APPWINDOW.0 == 0 {
        return false;
    }
    let chrome = WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_POPUP.0;
    c.has_title || c.ex_style & WS_EX_APPWINDOW.0 != 0 || c.style & chrome != 0
}

/// 从 z 序自上而下的候选里挑第一个"含着光标且值得推荐"的。
///
/// `EnumWindows` 自顶向下枚举，所以**先遇到的就是用户眼里的那个** —— 两个窗口重叠时，
/// 这条顺序就是全部判据。
pub(crate) fn pick_at(cands: &[Candidate], x: i32, y: i32) -> Option<&Candidate> {
    cands
        .iter()
        .find(|c| is_selectable(c) && contains(c.rect, x, y))
}

fn contains(r: ScreenRect, x: i32, y: i32) -> bool {
    x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
}

#[cfg(test)]
mod tests {
    use super::*;
    // 只在测试里用（任务栏真实样式含 `WS_EX_TOPMOST`）：放模块级会在非测试构建里
    // 变成 unused import 警告。
    use windows::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST;

    /// 一个"标准应用窗"的候选（带标题 + 常见边框样式）。
    fn app(rect: ScreenRect) -> Candidate {
        Candidate {
            rect,
            class: "Chrome_WidgetWin_1".into(),
            title: String::new(),
            has_title: true,
            style: WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0,
            ex_style: 0,
            has_owner: false,
            visible: true,
            iconic: false,
            cloaked: false,
            own_process: false,
        }
    }

    const R: ScreenRect = ScreenRect {
        x: 100,
        y: 100,
        w: 800,
        h: 600,
    };

    /// 正常应用窗要能推荐。
    #[test]
    fn keeps_an_ordinary_application_window() {
        assert!(is_selectable(&app(R)));
    }

    /// ★ 我们自己的窗默认不能推荐 —— 区域选择窗铺满整屏，不排除的话它永远第一个命中。
    #[test]
    fn rejects_our_own_windows() {
        let c = Candidate {
            own_process: true,
            ..app(R)
        };
        assert!(
            !is_selectable(&c),
            "自己进程的窗（选区窗/覆盖层/放大镜）不该被推荐"
        );
    }

    /// ★ 但贴图 / 便签 / 画中画是用户想截进图里的"桌面内容"，要按标题前缀放行；
    ///   控制面板按标题**精确相等**放行（2026-10-08 用户报"选区对本体窗口不起作用"）；
    ///   且放行后工具体/owner 之类的通用规则不再适用（贴图就是工具窗、便签正文带 owner）。
    #[test]
    fn keeps_our_pin_sticky_pip_and_panel_windows() {
        for title in ["CurSeen 贴图 #3", "CurSeen 便签", "CurSeen 画中画放大"] {
            let c = Candidate {
                own_process: true,
                title: title.into(),
                has_title: true,
                ex_style: WS_EX_TOOLWINDOW.0, // 贴图的实际样式
                has_owner: true,              // 便签正文的实际结构
                ..app(R)
            };
            assert!(is_selectable(&c), "{title} 应当被推荐");
        }
        // 便签正文窗：标题为空，靠类名放行
        let c = Candidate {
            own_process: true,
            title: String::new(),
            has_title: false,
            class: "CurSeenStickyBodyClass".into(),
            ex_style: WS_EX_TOOLWINDOW.0,
            has_owner: true,
            ..app(R)
        };
        assert!(is_selectable(&c), "便签正文应按类名被推荐");
        // ★ 控制面板：标题精确等于 "CurSeen"（ui::panel::TITLE），按精确匹配放行。
        let c = Candidate {
            own_process: true,
            title: "CurSeen".into(),
            has_title: true,
            ..app(R)
        };
        assert!(is_selectable(&c), "控制面板（标题恰为 CurSeen）应当被推荐");
        // 其余自窗（覆盖层 / 选择窗 / 放大镜）照旧排除
        for title in ["CurSeen Overlay", "CurSeen 截图区域", "CurSeen 截图放大镜"] {
            let c = Candidate {
                own_process: true,
                title: title.into(),
                has_title: true,
                ..app(R)
            };
            assert!(!is_selectable(&c), "{title} 不该被推荐");
        }
        // 白名单窗太小也不行
        let tiny = ScreenRect::new(0, 0, 4, 4);
        let c = Candidate {
            own_process: true,
            title: "CurSeen 贴图 #1".into(),
            rect: tiny,
            ..app(tiny)
        };
        assert!(!is_selectable(&c), "小于最小边长的白名单窗照样不推荐");
    }

    /// 隐藏 / 最小化 / cloaked 的窗口都不是"看得见的那个"。
    #[test]
    fn rejects_invisible_minimised_and_cloaked_windows() {
        for (name, c) in [
            (
                "隐藏",
                Candidate {
                    visible: false,
                    ..app(R)
                },
            ),
            (
                "最小化",
                Candidate {
                    iconic: true,
                    ..app(R)
                },
            ),
            (
                "cloaked",
                Candidate {
                    cloaked: true,
                    ..app(R)
                },
            ),
        ] {
            assert!(!is_selectable(&c), "{name}窗口不该被推荐");
        }
    }

    /// ★ 桌面壳层不推荐（否则"永远推荐全屏"），但**任务栏要留下**（它带工具窗样式，
    /// 靠 `SHELL_CLASSES` 放行）。
    #[test]
    fn rejects_the_shell_desktop_but_keeps_the_taskbar() {
        for class in DESKTOP_CLASSES {
            let c = Candidate {
                class: class.into(),
                ..app(R)
            };
            assert!(!is_selectable(&c), "桌面壳层 {class} 不该被推荐");
        }
        let bar = Candidate {
            class: "Shell_TrayWnd".into(),
            has_title: false,
            // 任务栏真实样式：无标题、带工具窗样式。
            style: WS_POPUP.0,
            ex_style: WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0,
            ..app(R)
        };
        assert!(is_selectable(&bar), "任务栏应当可以框");
    }

    /// 工具窗 / 无激活窗 / 附属弹层：都不是用户眼里的"程序窗口"。
    #[test]
    fn rejects_tool_windows_and_owned_popups() {
        let cases = [
            (
                "工具窗",
                Candidate {
                    ex_style: WS_EX_TOOLWINDOW.0,
                    ..app(R)
                },
            ),
            (
                "无激活窗",
                Candidate {
                    ex_style: WS_EX_NOACTIVATE.0,
                    ..app(R)
                },
            ),
            (
                "附属弹层",
                Candidate {
                    has_owner: true,
                    ..app(R)
                },
            ),
            (
                "子窗",
                Candidate {
                    style: WS_CHILD.0,
                    ..app(R)
                },
            ),
        ];
        for (name, c) in cases {
            assert!(!is_selectable(&c), "{name}不该被推荐");
        }
        // 带 `WS_EX_APPWINDOW` 的附属窗是刻意要进任务栏的独立窗 → 放行。
        let standalone = Candidate {
            has_owner: true,
            ex_style: WS_EX_APPWINDOW.0,
            ..app(R)
        };
        assert!(is_selectable(&standalone));
    }

    /// 系统 helper（0×0 / 1×1）与无标题无边框的怪窗：都不推荐。
    #[test]
    fn rejects_tiny_and_faceless_windows() {
        let tiny = Candidate {
            rect: ScreenRect::new(0, 0, 1, 1),
            ..app(R)
        };
        assert!(!is_selectable(&tiny), "1×1 的系统 helper 不该被推荐");
        let faceless = Candidate {
            has_title: false,
            style: 0,
            ex_style: 0,
            ..app(R)
        };
        assert!(!is_selectable(&faceless), "无标题无边框的窗不该被推荐");
    }

    /// ★ z 序即全部判据：重叠时取**先出现**的那个（`EnumWindows` 自顶向下），
    /// 只有它不含光标时才落到下一个。
    #[test]
    fn picks_the_topmost_window_containing_the_cursor() {
        let top = app(ScreenRect::new(100, 100, 400, 400));
        let bottom = app(ScreenRect::new(300, 300, 400, 400));
        let cands = vec![top.clone(), bottom.clone()];

        // 重叠区：最上面那个赢。
        assert_eq!(pick_at(&cands, 350, 350), Some(&top));
        // 只有下面那个含住光标：落到它。
        assert_eq!(pick_at(&cands, 650, 650), Some(&bottom));
        // 都不在：没有推荐。
        assert_eq!(pick_at(&cands, 20, 20), None);

        // 上层的窗"不值得推荐"时，不该把下面的也一起挡住。
        let invisible_top = Candidate {
            visible: false,
            ..top.clone()
        };
        let cands = vec![invisible_top, bottom.clone()];
        assert_eq!(pick_at(&cands, 350, 350), Some(&bottom));
    }

    /// 边界：右/下边**不含**（半开区间），与 Win32 的 `PtInRect` 同义。
    #[test]
    fn hit_test_is_half_open() {
        let cands = vec![app(R)];
        for (x, y, hit) in [
            (100, 100, true),
            (899, 699, true),
            (900, 500, false),
            (500, 700, false),
            (99, 500, false),
        ] {
            assert_eq!(
                pick_at(&cands, x, y).is_some(),
                hit,
                "({x},{y}) 的命中判定错了"
            );
        }
    }

    /// ★ 内缩那一圈半透明可见边框：**已删除**（2026-09-23 用户实测推翻）。
    ///
    /// 那条路曾把 EXT 的最外圈当成"窗口外的颜色"剔掉，判据是"每边各让 1px、尺寸小 2"。
    /// 真机两次反馈说明那一圈**就是**窗口的边框线（DWM 画的 40% 不透明灰），剔掉等于
    /// 每边啃 1px 窗口本体 —— 见模块头。这里只留一条**行为断言**：外框原样透传。
    #[test]
    fn the_visible_frame_is_used_as_is() {
        // `candidate_of` 里外框的唯一加工是 `clip_to_desktop`；内缩那条路已不存在。
        // 直接把 EXT 的形状喂给钳位函数，确认它不会自己缩 —— 若哪天有人"顺手"再插一个
        // inset 进去，这里必红。
        let ext = RECT {
            left: 607,
            top: 300,
            right: 1249,
            bottom: 752,
        };
        let got = clip_to_desktop(ext);
        if let Some(r) = got {
            assert_eq!(
                (r.x, r.y, r.w, r.h),
                (
                    ext.left,
                    ext.top,
                    ext.right - ext.left,
                    ext.bottom - ext.top
                ),
                "外框必须原样使用：EXT 的最外圈就是窗口的边框线"
            );
        }
    }

    /// 一个真窗口上量到的全部事实（供多条真窗口测试共用，避免各写一遍建窗）。
    struct RealWindow {
        /// `candidate_of` 给的推荐框。
        rect: ScreenRect,
        /// `DWMWA_EXTENDED_FRAME_BOUNDS`（`(x, y, w, h)`）。
        ext: (i32, i32, i32, i32),
        /// `GetWindowRect`（`(x, y, w, h)`）—— 与 `ext` 的差值即不可见缩放边框。
        gw: (i32, i32, i32, i32),
        /// 拿**这个探测窗自己**的外框去问 [`rect_is_window_frame`] 的结果。
        /// 必须为 `false`：它属于本进程，而该函数刻意排除我们自己的窗。
        own_hit: bool,
    }

    /// 建一个**真窗口**（本进程、带标题栏的普通顶层窗），把"推荐框 / EXT / GetWindowRect"
    /// 三个值一次算齐，销毁窗口后返回。拿不到就返回 `None`（调用方按"本机不支持"跳过）。
    ///
    /// 刻意只用最小的一组 Win32 调用：不注入输入、不动系统状态、不需要独占机器 ——
    /// 判据要落在 `candidate_of` 那一层，而不是靠真机手点。
    ///
    /// # Safety
    /// 内部全是 Win32 调用；窗口在返回前一定销毁。
    unsafe fn real_window_rects() -> Option<RealWindow> {
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{HINSTANCE, LRESULT, WPARAM};
        use windows::Win32::Graphics::Gdi::HBRUSH;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::System::Threading::Sleep;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, ShowWindow, HCURSOR,
            HICON, SW_SHOWNA, WINDOW_EX_STYLE, WNDCLASSW, WNDCLASS_STYLES, WS_OVERLAPPEDWINDOW,
        };

        unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }

        let class: Vec<u16> = "CurSeenFitProbe\0".encode_utf16().collect();
        let title: Vec<u16> = "CurSeenFitProbe\0".encode_utf16().collect();

        unsafe {
            let hinst = HINSTANCE(GetModuleHandleW(None::<&PCWSTR>).ok()?.0);
            let wc = WNDCLASSW {
                style: WNDCLASS_STYLES(0),
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: hinst,
                hIcon: HICON::default(),
                hCursor: HCURSOR::default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR(class.as_ptr()),
            };
            // 同一个测试进程里重复注册会拿到 0（类已存在）—— 那正是我们要的状态，继续。
            let _ = RegisterClassW(&wc);

            // 落在虚拟桌面内部（多屏时原点可能为负），免得 `clip_to_desktop` 把它裁掉。
            let vs = crate::platform::display::virtual_screen();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                vs.x + 120,
                vs.y + 120,
                360,
                240,
                None,
                None,
                Some(hinst),
                None,
            )
            .ok()?;
            let _ = ShowWindow(hwnd, SW_SHOWNA);

            // DWM 的外框属性在窗口刚建好那一瞬未必就绪。不重试的话，本测试在忙机器上会变成
            // "偶发红" —— 那比不测更糟（假红最耗时间）。
            let mut ext = RECT::default();
            let mut ext_ok = false;
            for _ in 0..40 {
                let r = DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    std::ptr::from_mut(&mut ext).cast(),
                    std::mem::size_of::<RECT>() as u32,
                );
                if r.is_ok() && ext.right > ext.left && ext.bottom > ext.top {
                    ext_ok = true;
                    break;
                }
                Sleep(5);
            }

            let mut gw = RECT::default();
            let _ = GetWindowRect(hwnd, &mut gw);
            let cand = candidate_of(hwnd, 0);
            // ★ `rect_is_window_frame` 必须在**窗口还活着**的时候问：它靠枚举可见顶层窗，
            //   窗口一销毁就只剩 false（那会把判据变成"恒 false"的假绿）。
            let own_hit = cand.as_ref().is_some_and(|c| rect_is_window_frame(c.rect));
            let _ = DestroyWindow(hwnd);

            if !ext_ok {
                return None;
            }
            let c = cand?;
            // 交叉验证：这里传的 own_pid = 0，而我们的探测窗属于**真实**进程（pid ≠ 0），
            // 所以 `own_process` 必须是 false —— 顺带证明 pid 比对这条路是活的。
            assert!(!c.own_process, "own_process 判定失效（pid 比对）");
            Some(RealWindow {
                rect: c.rect,
                ext: (
                    ext.left,
                    ext.top,
                    ext.right - ext.left,
                    ext.bottom - ext.top,
                ),
                gw: (gw.left, gw.top, gw.right - gw.left, gw.bottom - gw.top),
                own_hit,
            })
        }
    }

    /// ★★ **真窗口实测**：`candidate_of` 取到的外框必须**逐值等于** `EXTENDED_FRAME_BOUNDS`。
    ///
    /// 这一条是为上一版那条"假绿"补的：`the_visible_frame_is_used_as_is` 只喂
    /// `clip_to_desktop`，**捉不到"在它之前又插一次内缩"**这类回归 —— 而用户这次报的
    /// 恰恰就是内缩（*"裁剪太多了，连程序窗口的边框线都没有"*）。只有拿一个真窗口走完整条
    /// `candidate_of`，才能把"多 1px / 少 1px"两件事同时钉死。
    ///
    /// 判据（两条一起看）：
    ///   ① 推荐框 == EXT，**四边差全 0**；
    ///   ② 与 `GetWindowRect` 的关系 —— 两者在本机不同时，推荐框必须落在 EXT 那一侧。
    ///      少了 ②，"推荐框 == EXT"有可能只是"两条路碰巧一样"，证伪不了误用。
    ///
    /// 环境不支持（建窗失败 / 拿不到 EXT）时**跳过**，不制造假红。
    #[test]
    fn candidate_rect_equals_the_extended_frame_verbatim() {
        let Some(rw) = (unsafe { real_window_rects() }) else {
            eprintln!(
                "跳过 candidate_rect_equals_the_extended_frame_verbatim：本机建窗或取 EXT 失败"
            );
            return;
        };
        let (rect, ext, gw) = (rw.rect, rw.ext, rw.gw);
        let got = (rect.x, rect.y, rect.w, rect.h);

        // ① 主判据。
        assert_eq!(
            got, ext,
            "推荐框必须**逐值等于** EXTENDED_FRAME_BOUNDS（多 1px / 少 1px 都是用户报过的 bug）"
        );
        assert!(
            rect.w >= 8 && rect.h >= 8,
            "窗口太小，判据没有意义：{rect:?}"
        );

        // ② 对照。
        if got == gw {
            eprintln!(
                "提示：本机 GetWindowRect == EXT（{gw:?}）—— 这个窗口上两条路恰好重合，\
                 判据①对\"有没有误用 GetWindowRect\"不具区分力"
            );
        } else {
            eprintln!("对照通过：推荐框 {got:?} ≠ GetWindowRect {gw:?}");
        }
    }

    /// ★★ 真窗口实测：`rect_is_window_frame` **只认逐值相等**的外框，且**排除自己**。
    ///
    /// 这条是"最外圈反解"的触发开关。它若恒真，每次截图都会被当成贴边框处理（随便拖一个
    /// 框，最外圈 1px 的颜色就被改掉）；它若恒假，反解永远不生效（"多了 1px"照旧）。
    /// 两者都是假绿，所以**正负各测一次**，而且两半的样本必须不同（见下）。
    ///
    /// ★ 判据刻意分成两半，因为"自己的窗"这一半**必然是 false**：
    ///   - 负向用**本进程**建的探测窗 —— 稳定、不依赖外部环境，钉住"不许把 CurSeen 自己的
    ///     窗当目标"（区域选择窗铺满全屏，截全屏时它的外框恰好等于选区）；
    ///   - 正向只能用**别人的**应用窗口（本进程的窗本来就该被排除）。机器上没有别的应用
    ///     窗口时跳过这一半，不制造假红。
    #[test]
    fn rect_is_window_frame_only_matches_verbatim() {
        let Some(rw) = (unsafe { real_window_rects() }) else {
            eprintln!("跳过 rect_is_window_frame_only_matches_verbatim：本机建窗或取 EXT 失败");
            return;
        };
        // ① 负向（稳定）：自己的窗不许被认成目标。
        assert!(
            !rw.own_hit,
            "本进程的窗必须被 `rect_is_window_frame` 排除 —— 否则截全屏会被当成贴窗口边框"
        );

        // ② 正向（依赖环境）：找一个**别人的**可见应用窗口，它的外框必须命中；挪 1px 即失配。
        let Some(foreign) = (unsafe { first_foreign_window_frame() }) else {
            eprintln!("跳过正向判据：本机没有别的应用窗口（只有桌面 / CurSeen 自己）");
            return;
        };
        assert!(
            rect_is_window_frame(foreign),
            "别人的窗口外框必须命中（否则反解永远不生效，'多了 1px'照旧）"
        );
        let shifted = ScreenRect::new(foreign.x + 1, foreign.y, foreign.w, foreign.h);
        assert!(
            !rect_is_window_frame(shifted),
            "横向挪 1px 就不该再匹配：只认逐值相等，不许有容差 \
             （有容差就会去改不相关截图的边缘）"
        );
    }

    /// 第一个**非本进程**的可选窗口的外框（没有则 `None`）。只给测试用。
    ///
    /// # Safety
    /// 内部全是 Win32 调用；只读，不建窗不改状态。
    unsafe fn first_foreign_window_frame() -> Option<ScreenRect> {
        struct Find {
            found: Option<ScreenRect>,
        }
        unsafe extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
            let f = unsafe { &mut *(lparam.0 as *mut Find) };
            if f.found.is_some() {
                return BOOL(0);
            }
            let c = match unsafe { candidate_of(hwnd, GetCurrentProcessId()) } {
                Some(c) => c,
                None => return BOOL(1),
            };
            if c.own_process || !is_selectable(&c) {
                return BOOL(1);
            }
            f.found = Some(c.rect);
            BOOL(0)
        }
        let mut f = Find { found: None };
        unsafe {
            let _ = EnumWindows(Some(proc), LPARAM(std::ptr::from_mut(&mut f) as isize));
        }
        f.found
    }
}
