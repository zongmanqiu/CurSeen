//! 贴图窗：把一张图**无边框**钉在桌面上。
//!
//! 这是"叠层"的第二个消费方（[`crate::runtime::layer::region`] 是第一个）。它不铺底图，
//! 只显示贴图内容本身——内容已经是位图（截图裁出来的，或剪贴板文本栅格化的）。
//!
//! # 交互（用户口径）
//!
//! | 手势 | 语义 |
//! |---|---|
//! | 左键按下 | **该贴图升到最上层 + 被选中**，并开始拖动（窗口内**任意点**都能拖，没有拖动条） |
//! | 左键双击 | **关闭** |
//! | 右键单击 | **弹出操作菜单**（另存/关闭/旋转/镜像/透明度，见「右键菜单」节） |
//! | 滚轮 | 放大 / 缩小，**一格 10%**，下限 10%（见下节） |
//! | Ctrl+滚轮 | **整窗透明度**，一格 10%，钳 10%~100%（用户口径） |
//! | Esc | 关闭**选中的那一张**（由 `Runtime` 统一派发，见 `LayerSession::escape_target`） |
//! | Ctrl+S | 把**选中的那一张**另存为 PNG（派发在 `overlay.rs`，弹原生保存对话框） |
//!
//! 「选中」是跨窗口的唯一真相，存在 `Runtime`（`SnapMgr::selected_pin`）：点中某张贴图即
//! 选中它（并置顶、画出实边高亮），点到贴图以外的任何地方即取消选中。`Esc` / `Ctrl+S`
//! 都只作用于它。为什么点外面必须取消：`Ctrl+S` 是**全局**键（走低级键盘钩子），选中态
//! 若粘住不放，用户在别的程序里按 `Ctrl+S` 会被我们一直拦下来另存图片。
//!
//! ## ★ Esc 必须由本窗**自己吃掉**（否则一次按键关掉两张）
//!
//! 贴图窗是独立子窗，它有键盘焦点时照样收得到 `WM_KEYDOWN`。而 windui 对**没被消费的**
//! Escape 有一条关窗兜底（`app::UiHost::on_key`：`Key::Escape if resolve_close()`）；
//! 主窗能用 `App::on_shortcut` 拦住它，子窗此前不能（装配子窗时那个槽位恒填 `None`）。
//! 于是按下 Esc 会**同时**发生两件事：
//!
//! 1. 我们的低级键盘钩子 → `Runtime` 派发 → 关掉**选中的那一张**；
//! 2. 那个持有键盘焦点的贴图窗自己 → 兜底 → 把自己关掉。
//!
//! 两张图一起消失，其中"另一张"就是当时有焦点的那个（多半是最后 pin 的那张）——
//! 这正是用户报的"选中的消失了，最后一张被 pin 的也消失了"。修法是补丁 026 给子窗开的
//! 口子：`Window::on_shortcut(|_, ev| ev.key == Key::Escape)`，本窗声明 Escape 归自己，
//! 兜底就轮不到了（见 `open()`）。**别再把这行删掉**，删了就是"一次 Esc 关两张"复发。
//!
//! # 缩放：滚轮 10% 一档，光标下的那一点不动
//!
//! 用户口径（2026-09-17）：*"图片被 pin 后，我希望可以通过滚轮进行放大/缩小。并且在图片
//! 左上角以百分比的样式显示缩放因子。缩放，每次以10%为滚轮尺度。最低只能到10%。"*
//!
//! 三个约束定下了实现形状：
//!
//! - **窗口必须跟着缩放**。贴图窗是**非分层**窗口，"透明"区域实际是黑（见 `paint` 的
//!   注释），所以不能"窗口不变、图在窗口里缩小"——那会露出一圈黑边。
//! - **缩放锚在光标**：`zoom_anchor` 保持光标底下那个**图片像素**不动（图像查看器的标准
//!   手感）。锚点必须算在**图片**左上角上（不是窗口左上角），因为窗口还多出让给描边的
//!   `FRAME` 圈，两者缩放比不同，混着算会每滚一格漂一点。
//! - **改尺寸不能在 `on_event` 里做**。`SetWindowPos` 会**同步**派发 `WM_SIZE`，而
//!   `WM_SIZE` 的处理里要重新借一次窗口状态去 `paint`——那正是 windui 铁律 6 说的
//!   `&mut` 别名。所以尺寸走 `ctx.defer_blocking`（框架给的延后通道，事件分发完全返回后
//!   才跑），与 `ctx.open_window` 同一个理由、同一条路。
//!
//! 还有**第四个**约束，是 2026-10-24 用户报"长图放大之后图片并没有放大，只是外部的边框
//! 在放大"之后补上的：
//!
//! - **窗口样式必须是 `WS_POPUP`**（windui 补丁 095，`open()` 里的 `.popup(true)`）。
//!   固定大小 + `frameless` 的窗样式仍是 `WS_OVERLAPPEDWINDOW` 一族，`CreateWindowExW` 与
//!   `SetWindowPos` 都按 `WM_GETMINMAXINFO` 的跟踪尺寸收口：宽硬下限 `SM_CXMINTRACK`
//!   （本机 136，`ptMinTrackSize` 写 1 也压不到更低），高上限约一屏（本机 1100）。
//!   于是窄长条图（多行短文字栅格化出来的，12:1 那种）窗口被顶宽、`fit_aspect` 把图居中
//!   露出两侧黑边；长图放大到窗高越过上限后**图片高度被钉死**，而窗口另一维还在按百分比
//!   长——图片尺寸冻结、外框继续变大，正是用户看到的现象。换成 `WS_POPUP` 后两项收口
//!   全部消失（`tests/probes/pin_zoom_clamp_probe.rs` 变体 3 实测 2×80 / 3000×80 /
//!   60×4000 原样授予），窗口尺寸才真正等于"图片 × 缩放 + 2×FRAME"。
//!   **别删 `.popup(true)`**：删了就是"极端宽高比贴图图片不缩放"复发
//!   （`pin_zoom_e2e_probe` 会红，已实测）。
//!
//! 每格 10% 的判定挂在**累计滚轮量**上（`WHEEL_DELTA = 120` 一格，见
//! `PointerKind::Wheel`）：精密触控板会连着发好几个小 delta，只看符号会把 10% 当一格发出去。
//!
//! 多张贴图各自是一个窗口（同一 owner 的 owned 子窗），所以"点哪张动哪张"要靠**三条**
//! 保证：
//!   ① 捕获必须**谁申请谁归还**——`ctx.capture()` 之后漏掉 `ctx.release_capture()`，
//!      那个窗口就永久持有 OS 鼠标捕获，全屏的按下全归它（症状：只有一张贴图能被拖动）；
//!   ② 点击时 `SetWindowPos(HWND_TOPMOST)` 把它提到同级最顶（不然 z 序只是创建顺序）；
//!   ③ 点击时把"选中"写进 `Runtime`，并让上一张重绘掉高亮。
//! 三条都由 `pin.rs::tests` 的回归测试钉住。
//!
//! 窗口**不抢焦点**（贴图不该打断用户正在打的字），所以 Esc / Ctrl+S 都不能靠窗口自己的
//! 按键——它们走全局钩子那条路。上一条"子窗要自己吃 Escape"说的是**别**让宿主的兜底把
//! 窗口关掉，不是让本窗去处理 Esc 语义：语义只有 `Runtime` 那一处。
//!
//! # 描边：2px 分层边框（不是发光）
//!
//! 用户口径（2026-09-17）：*"pin 图时，图片边缘还是要向外发光比较好……我主要是担心，我们
//! 当前只有 1px 的灰色边框，有时候不太好分辨。又或者图片本身灰色、边框灰色、并且我们屏幕
//! 背景也灰色，那就完全无法分辨了……又或者，我们增加 2 层边框线？外层灰色，外外层黑色。
//! 这样可能操作既简单又有层次？"*
//!
//! 采纳"两层"这个方向，理由：发光（向外朦胧）要求贴图子窗走**分层**（逐像素透明），而
//! `layered` 后端的 `PARKED` 是**进程级**原子（覆盖层休眠时省 8~33MB 用）——一旦给子窗开
//! 分层，覆盖层休眠会把贴图一起抹掉。而"分层边框"**不需要任何分层**：它只是多画一圈像素。
//!
//! 落地方式是**两层描边 + 一圈留白**（`FRAME = 2` 物理像素）：
//!
//! | 环 | 半径 | 颜色 | 作用 |
//! |---|---|---|---|
//! | 外环 | 最外 1px | 近黑 `(24,28,32)` | 压住**浅色**背景，也给内圈一个对比底 |
//! | 内环 | 第 2 个像素 | 近白 `(246,248,251)` | 压住**深色**背景；把图和桌面隔开一档 |
//! | 选中 | 外环换成主色蓝 `(76,139,245)` | | "我点的就是这张"（Esc / Ctrl+S 作用对象） |
//!
//! 两层一深一浅，**任何背景色下至少有一层能和背景拉开**——这正是"层次感"的本质（发光好看
//! 也只是因为它同时提供了明暗两极）。窗口因此比图片**大 2×FRAME**：图片仍然 1:1 落在截图
//! 原位（见 `pin_placement`），多出来的那两圈用于描边。
//!
//! # 右键菜单为什么用原生 `TrackPopupMenu`，而不是 `Element` 上下文菜单
//!
//! 贴图窗只有图片那么大，windui 的上下文菜单是**窗内浮层**（按窗边界钳位）塞不进去。
//! 所以右键菜单改走 **Win32 原生弹出菜单**（`platform::win_util::menu_*`）——它在屏幕任意
//! 位置弹出、天然支持子菜单（透明度那档）。原生菜单自带嵌套消息泵，会阻塞到用户松开为止，
//! 因此**必须在 `ctx.defer_blocking` 的延后任务里弹**，事件分发里同步弹会重入窗口过程
//! （`&mut` 别名 UB，见 `PinWidget`）。
//!
//! 菜单落地在 `show_context_menu` + `PinWidget`（`Rc<RefCell<PinCanvas>>`
//! 封装让延后任务能访问画布）。Ctrl+S 与右键「另存为」**都存当前（含变换后）的位图**：
//! Ctrl+S 作用于选中的那一张，右键作用于这一张。m16 之前 Ctrl+S 存的是出厂那一份，
//! 于是"我看到灰度的图、存出来是彩色的"——脚本能回写之后这会变成常态，故收敛成一条语义。
//! 出厂那份仍可从「恢复原图」拿回来（它存在 `OpenPin.content` 里，不受变换影响）。
//!
//! # 为什么不用 `Element::window_drag()`
//!
//! 那个返回 `HTCAPTION`，之后鼠标消息**进不了客户区**，`PointerEvent::click_count`
//! 拿不到 → 双击取消直接失效。自己缓存 HWND 调 `SetWindowPos` 才是正解。
//! 缓存 HWND 还顺带省掉了"每次鼠标移动都 `FindWindowW` 扫一遍顶层窗口"。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use windui::core::{EventCtx, Widget};
use windui::event::{Event, Key, MouseButton, PointerKind};
use windui::geometry::{Color, Point, Rect};
use windui::platform::native_window_handle;
use windui::prelude::*;
use windui::render::image::{Fit, Image as WImage};
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::spec::Align;
use windui::text::{TextEngine, TextStyle};

use crate::core::exit_policy::ExitPolicy;
use crate::core::geometry::Rect as LRect;
use crate::core::image::RgbaBitmap;
use crate::platform::display::{cursor_pos, ScreenRect};
use crate::runtime::layer::WorldMap;
use crate::runtime::snap::PinContent;
use crate::runtime::Runtime;

/// 贴图窗标题前缀。**天然唯一**（带序号），所以 `post_close_by_title` 可以直接按标题关。
pub const PIN_TITLE_PREFIX: &str = "CurSeen 贴图 #";

/// 贴图窗标题。
pub fn pin_title(seq: u32) -> String {
    format!("{PIN_TITLE_PREFIX}{seq}")
}

/// 图片四周预留的**描边宽度**（物理像素，每边）。
///
/// 窗口 = 图片 + `2 * FRAME`。为什么要有它：用户要"边缘有层次、任何背景下都能分辨"
/// （见模块头的表）。描边必须画在图片**外面**，否则灰色图片 + 灰色边照样分不出来；
/// 画在外面就需要窗口比图片大一点，否则会盖掉图片自己的边缘像素。
const FRAME: i32 = 2;

/// 外环（最外层 1px）颜色：近黑。压浅色背景。
const RING_OUTER: (u8, u8, u8) = (24, 28, 32);
/// 内环（第 2 个像素）颜色：近白。压深色背景，把图片和桌面隔开一档。
const RING_INNER: (u8, u8, u8) = (246, 248, 251);
/// 选中时的外环颜色（主色，与"我点的就是这张"一一对应）。
const RING_SELECTED: (u8, u8, u8) = (76, 139, 245);
/// 悬停（未选中）时的外环颜色：主色的淡版。
const RING_HOVER: (u8, u8, u8) = (150, 180, 225);

// ── 右键菜单命令 ID 与透明度 ──────────────────────────────────────────────
//
// 右键菜单改用 **原生 `TrackPopupMenu`**（不用 windui 的 `show_context_menu`：那是
// 窗内浮层、贴图窗太小塞不下，所以换成 Win32 原生菜单，支持子菜单；理由详见模块头）。
// 命令 ID 从 [`CMD_BASE`] 起连续分配，`0` 留给 `TrackPopupMenu` 的"没选中"哨兵（不重叠）。

/// 右键菜单命令 ID 基底（< 它的保留给 0 哨兵）。
const CMD_BASE: i32 = 0x2000;
/// 另存为 → 弹原生保存对话框。
const CMD_SAVE: i32 = CMD_BASE;
/// 关闭 → 按标题投递 `WM_CLOSE`。
const CMD_CLOSE: i32 = CMD_BASE + 1;
/// 顺时针 90°。
const CMD_ROT_CW: i32 = CMD_BASE + 2;
/// 水平镜像。
const CMD_MIRROR_H: i32 = CMD_BASE + 3;
/// 垂直镜像。
const CMD_MIRROR_V: i32 = CMD_BASE + 4;
/// 取色：进入吸管模式（2026-10-02 用户口径）。光标变十字，悬停显示光标下图片像素的
/// 颜色（`#RRGGBB`），单击复制并显示「已复制」，`Esc` / 再右键 / 点图片外退出。
const CMD_PICK: i32 = CMD_BASE + 5;
/// 恢复原图：把这张贴图的像素换回**钉进来那一刻那张**。走的是与旋转/镜像/脚本回写
/// **同一条缝**（`set_canvas_bitmap`），所以尺寸重算、两段式借用一格都不省。
/// 占的是回收掉的 0x2006 空号（`取色`=+5 与 `锁定`=+7 之间那个洞，全仓无其它引用；
/// 命令号唯一性由 `menu_command_ids_are_unique_and_reserve_zero` 钉住，两个菜单项同号是最难查的一类串味）。
const CMD_RESTORE: i32 = CMD_BASE + 6;
/// 锁定图片：**完全锁定**（2026-09-29 用户口径）——Esc / 双击关不掉、拖不动、
/// 滚轮缩放与 Ctrl+滚轮透明度全部禁掉；只有右键菜单「锁定图片」解除或「关闭」才会变。
const CMD_LOCK: i32 = CMD_BASE + 7;
/// 透明度子菜单项基底：`CMD_ALPHA_BASE + alpha/10`（10%→1 … 100%→10）。
const CMD_ALPHA_BASE: i32 = CMD_BASE + 8;
/// 「脚本」子菜单项基底：`CMD_LUA_MENU_BASE + idx`（脚本在 `menu_items()` 里的稳定索引）。
const CMD_LUA_MENU_BASE: i32 = CMD_BASE + 100;
/// 「脚本」子菜单**没有任何脚本**时的灰色占位项文本（用户口径："没有相关脚本就留空"）。
///
/// 子菜单恒挂载，但不允许"点了弹一片白"——留一条置灰说明，把"目录里没脚本"讲清楚。
/// ★ 改这个串要同步下面的宽度估算表（它是按字面宽度算右缘让位的）。
const LUA_SUB_EMPTY_HINT: &str = "（暂无脚本）";
/// ★ 编译期钉死：脚本命令区间（`+100` 起）必须与透明度区间（`+1`…`+10`）错开。
///
/// 两段区间都是常量、结论编译期就已知，写成运行期 `assert!` 会被 clippy 判成
/// "断言结果恒定"（除了占地什么也没挡住）——所以塞进 const 断言。
const _: () = assert!(CMD_LUA_MENU_BASE > CMD_ALPHA_BASE + 10);

/// 透明度下限（用户口径：10%~100%）。
const ALPHA_MIN: u8 = 10;
/// 透明度上限。
const ALPHA_MAX: u8 = 100;
/// 透明度一档（子菜单项 / Ctrl+滚轮都是它）。
const ALPHA_STEP: u8 = 10;

/// 透明度一步：`notches` 格（正=更不透明），钳在 [`ALPHA_MIN`]-[`ALPHA_MAX`]。纯函数。
fn alpha_step(alpha: u8, notches: i32) -> u8 {
    let next = alpha as i32 + notches * ALPHA_STEP as i32;
    next.clamp(ALPHA_MIN as i32, ALPHA_MAX as i32) as u8
}

/// 百分比 → `SetLayeredWindowAttributes` 的整窗 alpha（0..=255）。
fn pct_to_alpha(pct: u8) -> u8 {
    ((pct as f32 / 100.0) * 255.0).round().clamp(0.0, 255.0) as u8
}

// ── 取色（吸管）──
//
// 三个纯函数 + 一个共用矩形来源：`paint` 画图片、`on_event` 取样必须用**同一个**
// 显示矩形，否则"看到哪、取到哪"会差一个缩放。抽 `image_pixel_at` 为纯函数是为了
// 可测——映射错误（DPI 取整、fit 居中偏移）在真机上肉眼难查，单测逐值钉死。

/// 窗口内逻辑坐标 → 图片像素。`img` 是图片的显示矩形（显示尺寸 ↔ 位图尺寸线性对应，
/// 见 `PinCanvas::image_rect`）；光标不在图片上时 `None`。
fn image_pixel_at(img_w: u32, img_h: u32, img: Rect, pos: (f32, f32)) -> Option<(u32, u32)> {
    if img_w == 0 || img_h == 0 || img.w <= 0 || img.h <= 0 {
        return None;
    }
    let fx = pos.0 - img.x as f32;
    let fy = pos.1 - img.y as f32;
    if fx < 0.0 || fy < 0.0 {
        return None;
    }
    let px = (fx * img_w as f32 / img.w as f32).floor() as i32;
    let py = (fy * img_h as f32 / img.h as f32).floor() as i32;
    if px >= img_w as i32 || py >= img_h as i32 {
        None
    } else {
        Some((px as u32, py as u32))
    }
}

/// 取位图指定像素的 RGB。越界返 `None`：显示矩形与位图在旋转/缩放的瞬间可能短暂
/// 不同步，取色宁可空一手，绝不越界读。
fn sample_rgb(bmp: &RgbaBitmap, px: (u32, u32)) -> Option<[u8; 3]> {
    if px.0 >= bmp.w || px.1 >= bmp.h {
        return None;
    }
    let i = (px.1 as usize * bmp.w as usize + px.0 as usize) * 4;
    Some([bmp.rgba[i], bmp.rgba[i + 1], bmp.rgba[i + 2]])
}

/// 取色徽标的**分行文本**（用户口径 2026-10-02：不同颜色系统分行显示、带名字、左对齐）：
///
/// ```text
/// HEX: #0A141E
/// RGB: 10,20,30
/// HSL: 204°,50%,8%
/// ```
///
/// HEX/RGB/HSL 是取色器三件套（HEX 网页、RGB 编程、HSL 设计调色最常用）；刚复制过时
/// 首行前缀「已复制 · 」。纯函数。
fn pick_lines(rgb: [u8; 3], copied: bool) -> Vec<String> {
    let head = if copied { "已复制 · " } else { "" };
    vec![
        format!("{head}HEX: {}", hex_value(rgb)),
        format!("RGB: {}", rgb_value(rgb)),
        format!("HSL: {}", hsl_value(rgb)),
    ]
}

/// `#RRGGBB`（大写，与截图取色同走 `core::color`，不手写格式串）。
fn hex_value(rgb: [u8; 3]) -> String {
    crate::core::color::Rgba::from_array([rgb[0], rgb[1], rgb[2], 255]).to_hex_rgb()
}

/// `R,G,B` 十进制三元组（无空格，用户样例口径）。
fn rgb_value(rgb: [u8; 3]) -> String {
    format!("{},{},{}", rgb[0], rgb[1], rgb[2])
}

/// `H°,S%,L%`（HSL，CSS/设计工具通用口径）。
fn hsl_value(rgb: [u8; 3]) -> String {
    let (h, s, l) = rgb_to_hsl(rgb);
    format!("{h}°,{s}%,{l}%")
}

/// RGB → HSL（标准公式，`max==min` 时 S=0 免除除零）。角度钳回 0..360，S/L 取整到
/// 百分数。纯函数——换算错了（色相偏移、明度颠倒）在真机上肉眼难查，单测逐值钉死。
fn rgb_to_hsl(rgb: [u8; 3]) -> (i32, u8, u8) {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0, 0, (l * 100.0).round() as u8);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    let h = (h * 60.0).round() as i32;
    (
        h.rem_euclid(360),
        (s * 100.0).round() as u8,
        (l * 100.0).round() as u8,
    )
}

/// 滚轮缩放：**一格 10%**（用户口径）。
const ZOOM_STEP: u32 = 10;
/// 缩放下限 10%（用户口径："最低只能到10%"）。
const ZOOM_MIN: u32 = 10;
/// 缩放上限 800%。
///
/// 用户没要求上限，但**必须有**：窗口尺寸 = 图片 × 百分数，而窗口的像素缓冲是按尺寸
/// 实打实分配的——没有上限时，一张 800×600 的截图滚几十格就能要到几 GB。8 倍已经足够
/// 看清截图里的任何细节，代价却是可控的。
const ZOOM_MAX: u32 = 800;
/// 一个滚轮刻度（`WHEEL_DELTA`；`PointerKind::Wheel` 带的是**原始 delta**，±120 一档）。
const WHEEL_DELTA: i32 = 120;

/// 百分比角标的字号（逻辑 dp，与 `region.rs` 的尺寸角标同款）。
const LABEL_FONT: f32 = 13.0;
/// 角标底色（深色半透明，压在任何图片内容上都读得出来）。
const LABEL_BG: (u8, u8, u8) = (18, 22, 26);
/// 角标底色透明度（留一点图片透出来，免得像贴了块膏药）。
const LABEL_ALPHA: u8 = 208;
/// 角标文字色。
const LABEL_FG: (u8, u8, u8) = (240, 243, 246);

/// 缩放一步：`notches` 格（正=放大），每格 [`ZOOM_STEP`]%，钳在
/// [`ZOOM_MIN`]-[`ZOOM_MAX`]。纯函数，边界由单测钉住。
fn zoom_step(pct: u32, notches: i32) -> u32 {
    let next = pct as i32 + notches.saturating_mul(ZOOM_STEP as i32);
    next.clamp(ZOOM_MIN as i32, ZOOM_MAX as i32) as u32
}

/// 某缩放百分比下的**图片**物理尺寸（至少 1×1——0 会让窗口尺寸退化成只剩描边）。
fn zoomed_size(img_w: u32, img_h: u32, pct: u32) -> (i32, i32) {
    let k = pct as f32 / 100.0;
    (
        ((img_w as f32 * k).round() as i32).max(1),
        ((img_h as f32 * k).round() as i32).max(1),
    )
}

/// 在 `avail` 矩形内取**保持 `src` 宽高比**的最大矩形（纯函数）。
///
/// 贴图绘制用：windui 的 `draw_image(Fit::Fill)` 是**按目标矩形非等比拉伸**（见
/// `skia.rs` 的 `Fit::Fill => (pw/iw, ph/ih)`）——目标矩形一旦和原图差一丁点比例，图就被
/// 拉变形。窗口虽然按原图比例开窗，但 DPI 取整 / `WM_SIZE` 时序下 `paint` 拿到的 `bounds`
/// 会和目标尺寸差 1px，直接拿它减描边当目标矩形就会把这点误差放大成肉眼可见的"缩小到 10%
/// 变成横向长条"。所以这里**显式**按原图比例在可用区里取最大矩形并居中，彻底和任何取整
/// 误差解耦：无论 `bounds` 偏多少，画出来的图永远是原比例（最多 1px 留白，绝不拉伸）。
fn fit_aspect(src_w: i32, src_h: i32, avail_w: i32, avail_h: i32) -> (i32, i32) {
    let (avail_w, avail_h) = (avail_w.max(1), avail_h.max(1));
    if src_w <= 0 || src_h <= 0 {
        return (avail_w, avail_h);
    }
    let r = src_w as f32 / src_h as f32;
    let mut w = avail_w;
    let mut h = (w as f32 / r).round() as i32;
    if h > avail_h {
        h = avail_h;
        w = (h as f32 * r).round() as i32;
    }
    (w.max(1), h.max(1))
}

/// 缩放锚点：`origin` 是图片在屏幕上的左上角（物理像素），`old_len`/`new_len` 是缩放前后
/// 的图片边长，`cursor` 是光标那一维的屏幕坐标。返回**新的左上角**，使得光标底下那个
/// 图片像素保持不动（`(cursor - origin) / len` 不变）。
///
/// 用 `f32` 算再取整：每格 10% 的比值不是整数（`110/100`），整数运算会把误差一点点攒成
/// 可见的漂移。
fn zoom_anchor(origin: i32, old_len: i32, new_len: i32, cursor: i32) -> i32 {
    let ratio = new_len as f32 / old_len.max(1) as f32;
    let offset = (cursor - origin) as f32 * ratio;
    (cursor as f32 - offset).round() as i32
}

/// 图片矩形 → 窗口矩形（四周各放大 [`FRAME`]）。纯函数，便于单测钉住"预留了几像素"。
///
/// 图片位置/尺寸**不变**（截图原位 1:1 是硬口径），只把窗口往外撑——多出来的那两圈就是描边。
/// 贴屏幕边缘时窗口会往外溢出几个像素，无害（用户要的是"图的位置一样"）。
fn frame_inflate(image: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = image;
    (x - FRAME, y - FRAME, w + 2 * FRAME, h + 2 * FRAME)
}

fn color(c: (u8, u8, u8), a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}

/// 在 `bounds` 内缩 `inset` 像素处画一圈 **1px** 边框（四条边各一个填充矩形）。
///
/// 为什么用"四条边"而不是 `stroke_rect`：Canvas 的描边会以路径为中心线压住内外各半像素，
/// 相邻两圈就会互相啃掉一半。这里四边都是整数像素填充，两圈 1px 恰好相邻、互不覆盖——
/// 这正是"两层描边"能对上像素的前提。
//
// ★ 参数是**逻辑单位**，调用方把物理像素除以 dpi 再传进来（`FRAME as f32 / dpi`）：
//   窗口按物理像素撑开、Canvas 却按逻辑坐标画，混用会让两圈描边在非 100% 缩放下各变成
//   1.5 物理像素，还会啃掉图片 1px 边缘。
fn stroke_ring(canvas: &mut dyn Canvas, bounds: Rect, inset: f32, thickness: f32, c: (u8, u8, u8)) {
    let x = bounds.x as f32 + inset;
    let y = bounds.y as f32 + inset;
    let w = (bounds.w as f32 - 2.0 * inset).max(thickness);
    let h = (bounds.h as f32 - 2.0 * inset).max(thickness);
    for r in [
        (x, y, w, thickness),
        (x, y + h - thickness, w, thickness),
        (x, y, thickness, h),
        (x + w - thickness, y, thickness, h),
    ] {
        canvas.fill_rect(r.0, r.1, r.2, r.3, &WPaint::fill(color(c, 255)));
    }
}

/// 贴图画布：一张图 + 两层描边，可拖动、可滚轮缩放。
struct PinCanvas {
    rt: Rc<std::cell::RefCell<Runtime>>,
    seq: u32,
    /// **当前位图**（可能已被右键菜单旋转/镜像过）。渲染时与 `img` 是一致的像素，但 `img`
    /// 是渲染后端建好的不可变字面（windui 的 `WImage`），变换后要重建 —— 所以这里额外存一份
    /// 纯数据位图，右键菜单的「另存为」从它取当前内容，不必反推渲染对象。
    bitmap: Rc<RgbaBitmap>,
    img: WImage,
    /// 本窗口的 DPI 比例（物理/逻辑）。**必须由 `open()` 从 `WorldMap` 传进来**，不能在
    /// `paint` 里现调 `display::dpi_scale()`：那会让单元测试依赖跑测试那台机器的缩放，
    /// 像素断言（`pin_draws_two_layers_of_border_around_the_image`）就会随机器变色。
    dpi: f32,
    /// 缩放百分比（[`ZOOM_MIN`]-[`ZOOM_MAX`]，一档 [`ZOOM_STEP`]）。100 = 图片原始像素。
    pct: Cell<u32>,
    /// 滚轮累计量（攒够一个 `WHEEL_DELTA` 才动一档，见模块头）。
    wheel: Cell<i32>,
    /// 自身 HWND（首次事件时缓存，避免每次移动都按标题找窗）。
    hwnd: Cell<isize>,
    /// 抓取点（**物理**客户区坐标）。`PointerEvent::pos` 是逻辑坐标，见 `on_event`。
    drag: Cell<Option<Point>>,
    /// 指针是否在窗内（悬停描边用）。
    hover: Cell<bool>,
    /// 命令窗口的**逻辑**尺寸（`on_wheel` 缩放时算好并写入）。`paint` 用真实窗口尺寸作画，
    /// 离屏单测没有真窗口（hwnd=0，`GetWindowRect` 查不到）——这一格作为确定性回退，
    /// 让"图片必须填满命令尺寸"可被单测钉死（见 `pin_image_fills_the_commanded_window_size`）。
    target_size: Cell<Option<(i32, i32)>>,
    /// 窗口整窗不透明度（百分比，[`ALPHA_MIN`]-[`ALPHA_MAX`]，默认 100 = 完全不透明）。
    /// 只读不改像素：经 `platform::win_util::set_window_alpha` 设到 OS 层（`WS_EX_LAYERED`）。
    alpha_pct: Cell<u8>,
    /// ★ 取色模式（右键菜单「取色」进入，2026-10-02）：悬停显示光标下图片像素的颜色，
    ///   单击复制。只在本窗的图片上取样（吸管不吸图外——那有截图取色）。
    pick: Cell<bool>,
    /// 取色悬停点：窗口**逻辑**本地坐标 + 该点图片像素的 RGB。光标不在图片上时 `None`。
    pick_at: Cell<Option<(Point, [u8; 3])>>,
    /// 刚复制过（徽标显示「已复制 …」）；光标一移动就过期，不引入任何定时器。
    pick_copied: Cell<bool>,
}

impl PinCanvas {
    /// 关窗（双击 / 菜单「关闭」）：先摘记账再请求关窗。
    ///
    /// 记账这里做一次、`on_close_request` 再做一次（幂等）——因为 Esc 与超上限淘汰走的是
    /// 外部 `WM_CLOSE`，那条路进不了 `on_event`，只能靠 `on_close_request`。
    fn close(&self, ctx: &mut EventCtx) {
        self.rt.borrow_mut().note_pin_closed(self.seq);
        ctx.request_close();
    }

    /// 自身 HWND（首次事件时缓存，避免每次鼠标移动都按标题 `FindWindowW` 扫一遍）。
    ///
    /// 必须是**本窗口**的句柄：`native_window_handle()` 在 `wnd_proc` 入口按当前正在分发
    /// 消息的窗口写入，所以在这里读到的一定是收到这次事件的那个贴图窗，不会串到别的贴图。
    fn own_hwnd(&self) -> isize {
        let mut h = self.hwnd.get();
        if h == 0 {
            h = native_window_handle().unwrap_or(0);
            self.hwnd.set(h);
        }
        h
    }

    /// 真实窗口客户区（逻辑 dp）：`GetWindowRect` 再除 dpi，与 windui 的
    /// `logical = 物理/scale` 同义。
    ///
    /// **为什么必须查真实窗口**：本窗是被 `on_wheel` 用 `SetWindowPos` **外部**缩放的，
    /// 而 windui 的 `WM_SIZE` 只重设后端缓冲、不请求重排（`needs_relayout` 不置位），于是
    /// 缩放后 windui 传入 `paint` 的 `bounds` 停留在 100% 旧尺寸——直接拿它作画，图片就按
    /// 旧尺寸画、窗口却已变大/变小，图片与描边之间露出一圈黑。所以这里**拿真实窗口尺寸
    /// 作画**，彻底绕开布局陈旧。
    ///
    /// ★ 用户 2026-10-24 报的"长图放大之后图片并没有放大，只是外部的边框在放大"有**两个**
    ///   各自独立的原因，都踩过：① 上面这条布局陈旧（早已修）；② 系统跟踪尺寸收口把窗口
    ///   顶宽 / 限高（windui 补丁 095，见 `open()` 的 `.popup(true)`）。别把两者混为一谈——
    ///   `live_bounds` 治的是①，`.popup(true)` 治的是②。
    ///
    /// 优先级：① 真窗口查得到（`hwnd≠0`）→ 用查到的精确值；② 查不到（离屏单测 hwnd=0）
    /// 但 `on_wheel` 算好了命令尺寸 → 用命令尺寸（确定性回退）；③ 都没有 → 退回布局 `bounds`。
    fn live_bounds(&self, fallback: Rect) -> Rect {
        let hwnd = self.own_hwnd();
        if hwnd != 0 {
            if let Some((_, _, w, h)) = crate::platform::win_util::window_rect_hwnd(hwnd) {
                let lw = ((w as f32 / self.dpi).round() as i32).max(1);
                let lh = ((h as f32 / self.dpi).round() as i32).max(1);
                return Rect::new(fallback.x, fallback.y, lw, lh);
            }
        }
        if let Some((w, h)) = self.target_size.get() {
            return Rect::new(fallback.x, fallback.y, w.max(1), h.max(1));
        }
        fallback
    }

    /// 本窗是不是"当前选中的那一张"（决定要不要画实边高亮）。
    ///
    /// 选中态存于 `Runtime`（跨窗口唯一真相）：`Esc` 关它、`Ctrl+S` 另存它。
    fn selected(&self) -> bool {
        self.rt.borrow().pin_is_selected(self.seq)
    }

    /// 图片的**显示矩形**（逻辑坐标）：`paint` 画图与取色取样共用这一个来源——
    /// 两处各算各的就会"看到哪、取到哪"差一个缩放/居中偏移。
    fn image_rect(&self, bounds: Rect) -> Rect {
        let f = FRAME as f32;
        let inner_w = ((bounds.w as f32 - 2.0 * f) as i32).max(1);
        let inner_h = ((bounds.h as f32 - 2.0 * f) as i32).max(1);
        let (iw, ih) = fit_aspect(
            self.img.width() as i32,
            self.img.height() as i32,
            inner_w,
            inner_h,
        );
        Rect {
            x: bounds.x + FRAME + (inner_w - iw) / 2,
            y: bounds.y + FRAME + (inner_h - ih) / 2,
            w: iw,
            h: ih,
        }
    }

    /// 退出吸管模式（再右键 / `Esc`）。光标不用恢复：WM_SETCURSOR 每条移动都会把
    /// 光标重置回窗口类光标，我们只是不再抢。
    fn exit_pick(&self) {
        self.pick.set(false);
        self.pick_at.set(None);
        self.pick_copied.set(false);
    }

    /// 取色：给定窗口内逻辑坐标，取该点图片像素的 RGB。不在图片上时 `None`。
    fn pick_sample(&self, pos: Point) -> Option<([u8; 3], Rect)> {
        let bounds = self.live_bounds(Rect::new(0, 0, 1, 1));
        let img = self.image_rect(bounds);
        image_pixel_at(
            self.img.width(),
            self.img.height(),
            img,
            (pos.x as f32, pos.y as f32),
        )
        .and_then(|p| sample_rgb(&self.bitmap, p))
        .map(|rgb| (rgb, img))
    }

    /// 滚轮 → 缩放（用户口径：一格 10%，下限 10%）。
    ///
    /// `delta` 是**原始**滚轮量（`WHEEL_DELTA = 120` 一格，见 `PointerKind::Wheel`）。
    /// 先攒够一格再动：精密触控板会连着一串小 delta，只看符号等于每个小 delta 都当一格。
    ///
    /// ★ 尺寸必须**延后**改（`ctx.defer_blocking`）：`SetWindowPos` 会同步派发 `WM_SIZE`，
    /// 那条路上要再借一次窗口状态去重绘，在事件回调栈里做就是 `&mut` 别名（windui 铁律 6，
    /// `EventCtx::open_window` 的注释里写着同一件事）。
    fn on_wheel(&self, ctx: &mut EventCtx, delta: i32, cursor: Point) {
        let acc = self.wheel.get() + delta;
        let notches = acc / WHEEL_DELTA;
        self.wheel.set(acc - notches * WHEEL_DELTA);
        if notches == 0 {
            return;
        }
        let old = self.pct.get();
        let new = zoom_step(old, notches);
        if new == old {
            return;
        }
        self.pct.set(new);

        let hwnd = self.own_hwnd();
        if hwnd == 0 {
            return;
        }
        let Some((wx, wy, _ww, _wh)) = crate::platform::win_util::window_rect_hwnd(hwnd) else {
            return;
        };
        let (old_w, old_h) = zoomed_size(self.img.width(), self.img.height(), old);
        let (new_w, new_h) = zoomed_size(self.img.width(), self.img.height(), new);
        // 光标**屏幕**坐标 = 窗口左上角 + 物理客户区坐标。`pe.pos` 是逻辑客户区坐标
        // （宿主已 /scale，见 windui `UiHost::on_pointer`），乘回 dpi 才是物理。
        // 锚点算在**图片**左上角上（窗口左上角还要再让出 FRAME 圈描边）——两者缩放比不同，
        // 混着算会让光标下的内容每滚一格漂一点。
        let cx = wx + (cursor.x as f32 * self.dpi).round() as i32;
        let cy = wy + (cursor.y as f32 * self.dpi).round() as i32;
        let img_ox = zoom_anchor(wx + FRAME, old_w, new_w, cx);
        let img_oy = zoom_anchor(wy + FRAME, old_h, new_h, cy);
        let (nx, ny, nw, nh) = frame_inflate((img_ox, img_oy, new_w, new_h));
        // ★ 记下命令窗口的**逻辑**尺寸：缩放后 windui 不重排、`paint` 拿到的 `bounds` 会停在
        //   旧尺寸 → 图片不随窗口缩放、露黑边。`paint` 优先用 `GetWindowRect` 的真实尺寸，这一
        //   格是没真窗口时（离屏单测）的确定性回退，也让运行期在查窗口之前就有正确值可用。
        self.target_size.set(Some((
            ((nw as f32 / self.dpi).round() as i32).max(1),
            ((nh as f32 / self.dpi).round() as i32).max(1),
        )));
        ctx.defer_blocking(move || {
            crate::platform::win_util::set_window_rect_hwnd(hwnd, nx, ny, nw, nh);
        });
        // 尺寸恰好取整到同一像素时不会有 `WM_SIZE`，这一句保证百分比角标也会更新。
        ctx.mark_dirty_all();
    }

    /// Ctrl+滚轮 → 调整**整窗透明度**（用户口径："可以按住Ctrl+滚轮调整"，一格 10%，
    /// 钳在 [`ALPHA_MIN`]-[`ALPHA_MAX`]）。
    ///
    /// 与 [`on_wheel`] 共用同一个 `wheel` 累计器——两次手势靠 Ctrl 区分（一次滚轮只能对应
    /// 一种语义），互不干扰。透明度只调 OS 层属性、不改窗口尺寸，无需 `defer_blocking`
    /// （不触发 `WM_SIZE`，也就不存在 `&mut` 别名问题）。
    fn on_wheel_alpha(&self, _ctx: &mut EventCtx, delta: i32) {
        let acc = self.wheel.get() + delta;
        let notches = acc / WHEEL_DELTA;
        self.wheel.set(acc - notches * WHEEL_DELTA);
        if notches == 0 {
            return;
        }
        let old = self.alpha_pct.get();
        let new = alpha_step(old, notches);
        if new == old {
            return;
        }
        self.alpha_pct.set(new);
        let hwnd = self.own_hwnd();
        if hwnd != 0 {
            crate::platform::win_util::set_window_alpha(hwnd, pct_to_alpha(new));
        }
    }
}

impl Widget for PinCanvas {
    fn measure(&self, avail: Size, _style: &Style, _text: &mut dyn TextEngine) -> Size {
        avail
    }

    fn paint(
        &self,
        bounds: Rect,
        _content: Rect,
        _focused: bool,
        _enabled: bool,
        canvas: &mut dyn Canvas,
        _style: &Style,
    ) {
        // ★★ `bounds` 是 windui 按布局算出的逻辑客户区。但本窗是被 `on_wheel` 用 `SetWindowPos`
        //   外部缩放的，windui 的 `WM_SIZE` 不请求重排 → 缩放后 `bounds` 停在 100% 旧尺寸，
        //   图片按旧尺寸画、窗口却已变 → 图片与描边之间露出一圈黑。下面拿**真实窗口尺寸**
        //   作画，绕开布局陈旧。（用户 2026-10-24 报的同一现象还有第二个原因——系统跟踪尺寸
        //   收口，已由 windui 补丁 095 的 `.popup(true)` 修掉，见 `live_bounds` 的注释。）
        let layout = bounds;
        let bounds = self.live_bounds(layout);
        canvas.clear_rect(
            bounds.x as f32,
            bounds.y as f32,
            bounds.w as f32,
            bounds.h as f32,
        );
        // ★ 整块客户区 = 图片 + 2×FRAME 的描边。图片画在描边以内的 `inner` 区。
        //
        // ★★ 关键：图片矩形必须**严格保持原图宽高比**（见 `fit_aspect`）。不能直接拿
        //   `bounds` 减描边当目标矩形交给 `draw_image(Fit::Fill)`——Fill 是按目标矩形
        //   非等比拉伸的，DPI 取整 / `WM_SIZE` 时序下 `bounds` 与目标尺寸差 1px 就会被
        //   放大成"缩小到 10% 图片变成横向长条"这种变形。按原图比例在 `inner` 内取最大
        //   矩形并居中，任何取整误差最多留 1px 留白、绝不拉伸。
        // 描边宽度（FRAME）与 fit 居中都收在 `image_rect` 里——与取色取样共用同一个
        // 矩形来源，保证"看到哪、取到哪"。
        let img = self.image_rect(bounds);
        canvas.draw_image(&self.img, img, Fit::Fill, 0.0, 1.0);
        // 两层描边（详见模块头）：外环深、内环浅 —— 任何背景色下至少有一层能和背景拉开。
        let selected = self.selected();
        let outer = if selected {
            RING_SELECTED
        } else if self.hover.get() {
            RING_HOVER
        } else {
            RING_OUTER
        };
        stroke_ring(canvas, bounds, 0.0, 1.0, outer);
        stroke_ring(canvas, bounds, 1.0, 1.0, RING_INNER);

        // ── 缩放百分比角标（图片左上角，用户口径："在图片左上角以百分比的样式显示缩放因子"）──
        //
        // ★ 只在**非 100%**（用户手动缩放过）时显示：100% 是出厂默认，没必要占地方；一旦
        //   滚轮动过才浮现，告诉用户"当前是几倍、这里支持滚轮缩放"。
        // ★ **直角**（无圆角）：用户要的是和截图尺寸标签一致的硬朗风格，不是胶囊。
        // ★ **贴合图片左上角的描边线**：徽标左上角对齐图片内缘左上角（覆盖图片角、不压描边）。
        // 它是画在图片之上的**纯叠加层**——只改画布上几个像素，绝不影响图片本身尺寸，
        // 也不影响窗口尺寸（窗口尺寸只由缩放百分比决定，见 `on_wheel`）。所以"比例和图片分离、
        // 不影响图片尺寸"天然成立。底色深色半透明，压在任何图片内容上都能读出来。
        let pct = self.pct.get();
        if pct != 100 {
            let label = format!("{pct}%");
            let ts = TextStyle::new(LABEL_FONT);
            let m = canvas.measure_text(&label, &ts);
            let bw = m.w.max(1) + 8;
            let bh = m.h.max(16) + 4;
            // 贴合图片左上角的描边内缘（徽标左上角 = 图片左上角）。
            let bx = img.x as f32;
            let by = img.y as f32;
            canvas.fill_rect(
                bx,
                by,
                bw as f32,
                bh as f32,
                &WPaint::fill(color(LABEL_BG, LABEL_ALPHA)),
            );
            // 内缩 2px：`draw_text` 盒子高度取自然行高（垂直居中退化为顶对齐），上下各留 2px
            // 才是真正的视觉居中。
            canvas.draw_text(
                &label,
                Rect::new(bx as i32, (by + 2.0) as i32, bw, m.h.max(16)),
                color(LABEL_FG, 255),
                Align::Center,
                &ts,
            );
        }
        // 真实尺寸与布局尺寸差一截（说明布局没跟上外部缩放）→ 请求下一帧重排，让 windui 的
        // `bounds` 收敛到真实窗口尺寸，后续局部重绘的脏区才不会停在旧尺寸上（否则仍可能
        // 在放大后的窗口里留一圈没被重画的黑）。阈值 2px 容忍 DPI 取整的 1px 噪声，只在
        // "真的缩放过"时才重排，避免每帧无谓重排。
        if (bounds.w - layout.w).abs() > 2 || (bounds.h - layout.h).abs() > 2 {
            windui::anim::request_relayout();
        }

        // ── 取色徽标（吸管模式，2026-10-02）：跟随光标；最左色块一列，右侧三种颜色
        //    系统分行左对齐（用户口径："HEX: … / RGB: …"）──
        //
        // ★ 画在**本窗画布**上而不是另开小窗：贴图是分层窗、一窗一 alpha，再开窗就要
        //   走便签那套 owner 拆窗；而徽标只在吸管悬停时存在，画进画布零新窗口。
        // ★ 不放大（用户口径："用户可以自己对图片放大"——滚轮缩放照常可用，映射按
        //   `image_rect` 现算，缩放中取色依然逐像素对准）。
        if self.pick.get() {
            if let Some((pos, rgb)) = self.pick_at.get() {
                let lines = pick_lines(rgb, self.pick_copied.get());
                let ts = TextStyle::new(LABEL_FONT);
                let ms: Vec<_> = lines.iter().map(|l| canvas.measure_text(l, &ts)).collect();
                let text_w = ms.iter().map(|m| m.w.max(1)).max().unwrap_or(1);
                let line_h = ms.iter().map(|m| m.h.max(16)).max().unwrap_or(16);
                let sw = 12i32; // 色块边长
                let bw = 2 + sw + 4 + text_w + 4;
                let bh = 2 + line_h * lines.len() as i32 + 2;
                // 挂在光标右下，越界就往回钳进窗口。
                let bx = (pos.x + 14).clamp(bounds.x, (bounds.x + bounds.w - bw).max(bounds.x));
                let by = (pos.y + 20).clamp(bounds.y, (bounds.y + bounds.h - bh).max(bounds.y));
                canvas.fill_rect(
                    bx as f32,
                    by as f32,
                    bw as f32,
                    bh as f32,
                    &WPaint::fill(color(LABEL_BG, LABEL_ALPHA)),
                );
                // 色块：当前色的实色样本（取色的"眼见为实"），独占最左一列、垂直居中。
                canvas.fill_rect(
                    (bx + 2) as f32,
                    (by + (bh - sw) / 2) as f32,
                    sw as f32,
                    sw as f32,
                    &WPaint::fill(color((rgb[0], rgb[1], rgb[2]), 255)),
                );
                // 文字：每行独立画在同一 x 上（`Align::Start` = 左对齐），行高统一。
                let tx = bx + 2 + sw + 4;
                for (i, m) in ms.iter().enumerate() {
                    canvas.draw_text(
                        &lines[i],
                        Rect::new(tx, by + 2 + line_h * i as i32, m.w.max(1) + 4, m.h.max(16)),
                        color(LABEL_FG, 255),
                        Align::Start,
                        &ts,
                    );
                }
            }
        }
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        match ev {
            Event::Pointer(pe) => match pe.kind {
                PointerKind::Down if pe.button == MouseButton::Left => {
                    // ★ 取色单击：复制当前色并**保持吸管**（继续点别的像素），
                    //   徽标显示「已复制」直到下一次移动（见 `pick_copied`）。不进入
                    //   拖动/置顶/选中——吸管期间窗口的手势语义全部让位给取色。
                    //   ★ 一次复制**全部三种格式**（用户口径 2026-10-02："复制全部颜色，
                    //   而不是只有 16 进制"）——三行文本（HEX/RGB/HSL）一起进剪贴板，
                    //   用的时候各取所需。
                    if self.pick.get() {
                        if let Some((rgb, _)) = self.pick_sample(pe.pos) {
                            self.pick_at.set(Some((pe.pos, rgb)));
                            let text = pick_lines(rgb, false).join(
                                "
",
                            );
                            crate::platform::clipboard_history::copy_text(&text);
                            self.pick_copied.set(true);
                            log::info!("贴图取色：复制全部格式");
                        }
                        ctx.mark_dirty_all();
                        return true;
                    }
                    // ★ 双击取消：前提是「窗口内任意点都能收到 LButtonDown」——无边框窗的
                    //   外圈 8px 曾被 windui 判成缩放热区（补丁 022-a 已修），否则靠近边缘
                    //   双击会被 WM_NCHITTEST 截走。
                    //   手势归 `ExitPolicy::PIN` 管（`core::exit_policy`）：这里不写死 `>= 2`，
                    //   免得以后"改画板顺手把贴图也改了"。
                    // ★ 锁定豁免：双击关闭被「锁定图片」拦下（锁了就只能右键解除 / 关闭）。
                    if ExitPolicy::PIN.double_click && pe.click_count >= 2 {
                        if !self.rt.borrow().pin_locked(self.seq) {
                            self.close(ctx);
                        }
                        return true;
                    }
                    let hwnd = self.own_hwnd();
                    // ★ 点击即置顶（用户要求："允许点击已 pin 的任意图片，被点击的图片上升到
                    //   最高层"）。必须在取捕获**之前**做：置顶会改 z 序，若先 `ctx.capture()`
                    //   则 SetWindowPos 有可能把捕获状态一并换掉。
                    //
                    //   贴图窗互为同级（同一个 owner 的 owned 子窗），`HWND_TOPMOST` 会把
                    //   被点的这一个提到最顶那一档——与 `move_window_hwnd` 拖动时用的档位一致，
                    //   不会出现"拖到一半又被别的贴图盖住"。
                    crate::platform::win_util::raise_window_hwnd(hwnd);
                    // ★ 点击即**选中**（用户口径 2026-09-17）：`Esc` 只关选中的这一张、
                    //   `Ctrl+S` 只另存这一张。选中态存在 `Runtime` 里（跨窗口唯一），
                    //   本窗因此要在置顶之后重绘一次，把"选中"实边画出来。
                    self.rt.borrow_mut().select_pin(self.seq);
                    ctx.mark_dirty_all();
                    // ★ 锁定 = 完全锁定（2026-09-29 用户口径）：不进入拖动——点击仍置顶
                    //   与选中（选中是 Ctrl+S 另存的前提，锁定不减这个能力），但移动禁掉。
                    if !self.rt.borrow().pin_locked(self.seq) {
                        self.drag.set(Some(pe.pos));
                        ctx.capture();
                    }
                    true
                }
                PointerKind::Move => {
                    // ★ 取色移动：刷新徽标（不走拖动分支）。不换光标（2026-10-02 口径）。
                    if self.pick.get() {
                        self.pick_at
                            .set(self.pick_sample(pe.pos).map(|(rgb, _)| (pe.pos, rgb)));
                        // 「已复制」反馈只在原地等——一动就是新的取样点，反馈过期。
                        self.pick_copied.set(false);
                        ctx.mark_dirty_all();
                        return true;
                    }
                    let Some(g) = self.drag.get() else {
                        return false;
                    };
                    let hwnd = self.own_hwnd();
                    if hwnd == 0 {
                        return false;
                    }
                    // 新原点 = 当前鼠标**屏幕坐标** − 抓取点（窗口本地坐标）。
                    // 不做增量累加：鼠标可能被别的窗口"吃掉"几条移动消息，累加会越拖越偏。
                    let (mx, my) = cursor_pos();
                    // `cursor_pos()` 与 `move_window_hwnd` 都是**物理像素**；抓取点 `g` 是
                    // windui 给的**逻辑**窗口本地坐标，须乘 DPI 转回物理再减，否则非 100% DPI
                    // 下拖动会整体偏移。
                    let gx = (g.x as f32 * self.dpi).round() as i32;
                    let gy = (g.y as f32 * self.dpi).round() as i32;
                    crate::platform::win_util::move_window_hwnd(hwnd, mx - gx, my - gy);
                    true
                }
                PointerKind::Up if pe.button == MouseButton::Left => {
                    self.drag.set(None);
                    // ★ 必须显式还捕获（用户报"只能移动其中一张贴图"的根因）。
                    //
                    //   windui 的捕获是**控件自己申请、自己归还**：`dispatch_pointer` 只在
                    //   `ctx.capture()/release_capture()` 时改捕获态，**Up 不会自动解除**。
                    //   漏掉这一句的后果不是"多抓一会儿"，而是那一个窗口**永久**持有 OS 鼠标
                    //   捕获——此后全屏的按下都被它收走（指针在别的贴图上也一样），于是只有
                    //   第一张被点过的贴图能拖动。窗口被关闭时才由系统兜底释放。
                    ctx.release_capture();
                    true
                }
                PointerKind::Down if pe.button == MouseButton::Right => {
                    // ★ 吸管期间右键 = 退出取色（不是再弹菜单）：给自己留一条
                    //   "点错了随手退出"的路，菜单按钮太远。
                    if self.pick.get() {
                        self.exit_pick();
                        ctx.mark_dirty_all();
                        return true;
                    }
                    // ★ 右键不再直接关窗，交给 `PinWidget` 在**事件返回后**弹原生菜单
                    //   （`TrackPopupMenu` 自带嵌套消息泵，直接在事件里同步弹会重入窗口过程 →
                    //   `&mut` 别名 UB，见模块头"缩放：不能用 on_event 同步改尺寸"同一条纪律）。
                    //   此处返回 false：本画布不吞右键，`PinWidget` 层已经拦截处理过了。
                    false
                }
                PointerKind::Enter => {
                    self.hover.set(true);
                    ctx.mark_dirty_all();
                    true
                }
                PointerKind::Leave => {
                    self.hover.set(false);
                    // 吸管移出图片/窗口：徽标消失（回来再显）。
                    if self.pick.get() {
                        self.pick_at.set(None);
                    }
                    ctx.mark_dirty_all();
                    true
                }
                // ★ 滚轮缩放（用户口径：一格 10%，下限 10%）。`pe.pos` 是逻辑客户区坐标，
                //   乘回 dpi 才是物理屏幕坐标（见 `on_wheel`）。滚轮事件只在光标位于本窗
                //   上方时投递到这里——"点哪张动哪张"是 OS 自然给的，不用额外判定命中。
                //   按住 Ctrl 时退化为**调透明度**（用户口径："可以按住Ctrl+滚轮调整"）。
                //   ★ 锁定 = 完全锁定（2026-09-29 用户口径）：缩放与透明度一起禁——
                //   "锁定"就是这张图连位置带状态全都定死，只有右键能解锁。
                PointerKind::Wheel(delta) => {
                    if self.rt.borrow().pin_locked(self.seq) {
                        return true;
                    }
                    if crate::platform::win_util::control_down() {
                        self.on_wheel_alpha(ctx, delta);
                    } else {
                        self.on_wheel(ctx, delta, pe.pos);
                    }
                    true
                }
                _ => false,
            },
            // ★ 吸管期间 `Esc` = 退出取色（吃掉按键，别让它走到关窗）。
            // （守卫并进 match 臂：满足 clippy collapsible-if，行为不变）
            Event::Key(ke) if ke.pressed && self.pick.get() && ke.key == Key::Escape => {
                self.exit_pick();
                ctx.mark_dirty_all();
                true
            }
            _ => false,
        }
    }
}

/// 右键菜单对 **`PinCanvas`** 的 `Rc<RefCell<_>>` 透明封装。
///
/// 为什么需要这一层：windui 的 `EventCtx::defer_blocking` 闭包由框架持有、**在事件分发
/// 完全返回之后**才跑，闭包要么 `'static`、要么不借调用方的栈——`show_context_menu` 里要
/// 同步 `SetWindowPos`（旋转/镜像换尺寸）出自这个**延后任务**。
/// 它不能借 `&mut self`（事件回调还挂在那），所以用 `Rc<RefCell<PinCanvas>>` 共享：
/// `PinWidget` 只持这份 `Rc`（`Clone` 只是加了个引用计数），`on_event` 里克隆一份塞进
/// 闭包，`RefCell` 让变换/弹菜单在任务里 `borrow_mut` 改写画布状态。
///
/// 同时也把**右键弹菜单**从 `PinCanvas::on_event` 里抽出来放在这里拦截：`TrackPopupMenu`
/// 自带嵌套消息泵，直接同步弹会重入窗口过程（`&mut` 别名 UB），必须 `defer_blocking`。
#[derive(Clone)]
struct PinWidget(Rc<RefCell<PinCanvas>>);

impl Widget for PinWidget {
    fn measure(&self, avail: Size, style: &Style, text: &mut dyn TextEngine) -> Size {
        self.0.borrow().measure(avail, style, text)
    }

    fn paint(
        &self,
        bounds: Rect,
        content: Rect,
        focused: bool,
        enabled: bool,
        canvas: &mut dyn Canvas,
        style: &Style,
    ) {
        self.0
            .borrow()
            .paint(bounds, content, focused, enabled, canvas, style)
    }

    // ★ 必须显式声明想收右键：windui 的事件分发对**非左键** Down/Up 默认不投递到控件，
    //   除非控件 override `wants_right_click()`（见 windui `core.rs` 的 `dispatch`）。
    //   少了它，右键 Down 根本到不了下面的 `on_event` —— 右键菜单永不会弹出。
    fn wants_right_click(&self) -> bool {
        true
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        // 右键按下：**延后**弹原生菜单（见 struct 文档）。`ctx.defer_blocking` 的闭包不能借
        // self，先克隆一份 `Rc` 进去；离屏单测没有真窗口（hwnd==0）时跳过，避免
        // `TrackPopupMenu` 在没有窗口的循环里挂死。左键触发的双击/拖动等仍走画布自身。
        if matches!(
            ev,
            Event::Pointer(pe)
                if pe.kind == PointerKind::Down && pe.button == MouseButton::Right
        ) {
            let (hwnd, x, y) = {
                let c = self.0.borrow();
                // 把被右键的那一张抬到最顶，保证菜单显示时它不被别的贴图盖住。
                crate::platform::win_util::raise_window_hwnd(c.own_hwnd());
                let (mx, my) = cursor_pos();
                (c.own_hwnd(), mx, my)
            };
            let rc = self.0.clone();
            if hwnd != 0 {
                ctx.defer_blocking(move || show_context_menu(&rc, x, y));
            }
            return true;
        }
        self.0.borrow_mut().on_event(ctx, ev)
    }
}

// ── 右键菜单：构建、弹出、命令分发 ─────────────────────────────────────────
//
// 全部挂在 `ctx.defer_blocking` 的延后任务里跑（`PinWidget::on_event` 只在真有窗口时才
// defer），菜单自带的嵌套消息泵、同步 `SetWindowPos` 都安全——此时事件分发
// 已经返回，没有任何活动的窗口 `&mut` 借用。

/// 菜单文字的"显示列"数：ASCII 算半列，其余（汉字 / `°` / `…`）算整列。
///
/// 只服务 [`est_menu_width`] 的宽度估算，精度要求是"宁大勿小"而不是逐像素。
fn label_cols(s: &str) -> usize {
    s.chars().map(|c| if c.is_ascii() { 1 } else { 2 }).sum()
}

/// 菜单宽度的**宁大勿小**估算（物理像素）。
///
/// 96dpi 下菜单字体（Segoe UI 9pt）每列约 8px，取 9 留余量；勾选槽 + 左右边距 +
/// 子菜单箭头约 56px。估算偏高只会让菜单离屏幕右缘更远一点（无害），偏低则防不住
/// 子菜单左翻盖住父菜单（见 [`anchor_x_with_submenu_room`]）。
fn est_menu_width(max_cols: usize, dpi: f32) -> i32 {
    ((max_cols as f32 * 9.0 + 56.0) * dpi.max(1.0)) as i32
}

/// 右键菜单锚点 x 的留白钳位（纯函数）。
///
/// Windows 的规矩：子菜单在右侧放不下时会**向左翻转、盖住父菜单**（系统标准行为，
/// 但观感上"新菜单挡住老菜单"是错的——2026-09-26 用户口径）。原生 `TrackPopupMenu`
/// 不给子菜单定位的口子，唯一能做的是把**锚点**往屏内挪：保证「根菜单右缘 + 最宽
/// 子菜单」整个留在落点显示器的工作区内——子菜单有地方向右展开，就不会左翻。
///
/// 返回值恒在 `[wa_x, wa_x + wa_w - root_w - sub_w]` 内（屏幕窄到放不下时退到 `wa_x`）。
fn anchor_x_with_submenu_room(x: i32, root_w: i32, sub_w: i32, wa_x: i32, wa_w: i32) -> i32 {
    let limit = wa_x + wa_w - root_w - sub_w;
    if limit < wa_x {
        return wa_x;
    }
    x.min(limit).max(wa_x)
}

/// 「脚本」子菜单要挂的行：`(命令号, 文本, 是否置灰)`。
///
/// 抽成纯函数是为了能把"恒挂载 + 没脚本就留空"钉在单测里——这类菜单 bug 只能看见、
/// 摸不到，靠手点验一轮就丢。
/// ★ 子菜单本身由调用方恒挂载，**这里永不使用"（脚本数 > 0）才加子菜单"的写法**：
///   入口跟着配置漂移，用户会以为功能坏了。
fn lua_submenu_rows(items: &[(String, String)]) -> Vec<(i32, String, bool)> {
    if items.is_empty() {
        return vec![(0, LUA_SUB_EMPTY_HINT.to_string(), true)];
    }
    items
        .iter()
        .enumerate()
        .map(|(idx, (_id, text))| (CMD_LUA_MENU_BASE + idx as i32, text.clone(), false))
        .collect()
}

/// 弹出贴图右键菜单并分发所选命令。`x`/`y` 为光标屏幕坐标（物理像素）。
///
/// 菜单项与命令 ID 的对应见头部 [`CMD_*`] 常量。菜单用 `TrackPopupMenu` 同步弹出
/// （`TPM_RETURNCMD`），用户点选后返回命令号，再按命令分发。菜单用 `isize` 句柄传递，
/// 用完 `DestroyMenu` 释放（弹出子菜单随父菜单一并销毁，不单独释放）。
fn show_context_menu(rc: &Rc<RefCell<PinCanvas>>, x: i32, y: i32) {
    let seq = rc.borrow().seq;
    let hwnd = rc.borrow().own_hwnd();
    // 菜单打开时读一遍锁定态：决定「锁定图片」是否打勾。
    let locked = rc.borrow().rt.borrow().pin_locked(seq);

    let menu = crate::platform::win_util::menu_new();
    if menu == 0 {
        return;
    }
    crate::platform::win_util::menu_append(menu, CMD_ROT_CW, "顺时针 90°");
    crate::platform::win_util::menu_append(menu, CMD_MIRROR_H, "水平镜像");
    crate::platform::win_util::menu_append(menu, CMD_MIRROR_V, "垂直镜像");
    // 「恢复原图」只在**确实不同**时出现：一张从没动过的贴图挂着这么条灰项，会让人怀疑
    // "我是不是改过了"。判据含逐字节比较（不是只比宽高）——脚本换一张同尺寸的灰度图，
    // 光看宽高是看不出来的。
    if pin_was_transformed(rc, seq) {
        crate::platform::win_util::menu_append(menu, CMD_RESTORE, "恢复原图");
    }
    // ★ 取色 = 开关（用户口径 2026-10-02："选项前面打勾，再选一次就退出"）：吸管激活时
    //   打勾，再选一次即退出。
    let picking = rc.borrow().pick.get();
    if picking {
        crate::platform::win_util::menu_append_checked(menu, CMD_PICK, "取色");
    } else {
        crate::platform::win_util::menu_append(menu, CMD_PICK, "取色");
    }
    crate::platform::win_util::menu_separator(menu);
    // 透明度子菜单：10%~100%，10% 一档（用户口径）。
    // ★ 当前档位打勾（2026-09-29 用户反馈：Ctrl+滚轮调过透明度后，菜单里看不出
    //   现在是哪档）。alpha_pct 恒是 ALPHA_STEP 的整数倍（滚轮与菜单都按步进走），
    //   直接按值比对即可。
    let alpha_sub = crate::platform::win_util::menu_submenu(menu, "透明度");
    if alpha_sub != 0 {
        let cur_alpha = rc.borrow().alpha_pct.get();
        let mut a = ALPHA_MIN;
        while a <= ALPHA_MAX {
            let id = CMD_ALPHA_BASE + (a as i32 / ALPHA_STEP as i32);
            if a == cur_alpha {
                crate::platform::win_util::menu_append_checked(alpha_sub, id, &format!("{a}%"));
            } else {
                crate::platform::win_util::menu_append(alpha_sub, id, &format!("{a}%"));
            }
            a += ALPHA_STEP;
        }
    }
    // ── 脚本：Lua 扩展（`assets/lua` 里声明了 menu 的脚本），子菜单项 ID=基底+稳定索引 ──
    // ★ 透明度与脚本之间只留一条间隔线（2026-09-26 用户口径：两条太挤）。
    // ★ 子菜单**恒挂载**，一个脚本都没勾开时才"留空"（挂一条灰色占位项，而不是把整个
    //   子菜单撤掉：撤掉 = 用户以为功能坏了 / 入口随配置漂移）。
    //   哪些脚本在里面由 `lua_ext_menu_items` 决定（= 磁盘扫描 ∧ `image_active` 开关），
    //   这里不再自己加第二道闸——两道闸同号才会一致。
    let lua_items = rc.borrow().rt.borrow().lua_ext_menu_items();
    crate::platform::win_util::menu_separator(menu);
    let lua_sub = crate::platform::win_util::menu_submenu(menu, "脚本");
    if lua_sub != 0 {
        for (cmd, text, disabled) in lua_submenu_rows(&lua_items) {
            if disabled {
                crate::platform::win_util::menu_append_disabled(lua_sub, &text);
            } else {
                crate::platform::win_util::menu_append(lua_sub, cmd, &text);
            }
        }
    }
    // ── 底部：锁定 / 另存为 / 关闭（用户口径：另存为与关闭在最底，另存为上方是锁定）──
    crate::platform::win_util::menu_separator(menu);
    if locked {
        crate::platform::win_util::menu_append_checked(menu, CMD_LOCK, "锁定图片");
    } else {
        crate::platform::win_util::menu_append(menu, CMD_LOCK, "锁定图片");
    }
    crate::platform::win_util::menu_append(menu, CMD_SAVE, "另存为（当前画面）…");
    crate::platform::win_util::menu_append(menu, CMD_CLOSE, "关闭");

    // ★ 子菜单防遮挡（2026-09-26 用户口径）：锚点太靠屏幕右缘时，「透明度 / 脚本」
    //   的子菜单会被 Windows 向左翻转、盖住父菜单。把锚点往屏内挪，给子菜单留出
    //   向右展开的空间（宽度用宁大勿小的估算，见 `est_menu_width`）。根级标签必须与
    //   上面逐字一致——它就在同一个函数里，改一处忘另一处编译不报错、这里会估错。
    let dpi = crate::platform::display::dpi_scale();
    let root_cols = [
        "顺时针 90°",
        "水平镜像",
        "垂直镜像",
        "取色",
        "透明度",
        "脚本",
        "锁定图片",
        "另存为…",
        "关闭",
    ]
    .iter()
    .map(|s| label_cols(s))
    .max()
    .unwrap_or(0);
    // 「脚本」子菜单的宽度：有脚本取最长脚本名，无脚本时那条灰色占位项就是最宽的。
    // 两个分支都保留 "100%"（透明度子菜单）作为下限——两张子菜单共享同一份估算。
    let mut sub_cols = label_cols("100%");
    if lua_items.is_empty() {
        sub_cols = sub_cols.max(label_cols(LUA_SUB_EMPTY_HINT));
    }
    for (_, text) in &lua_items {
        sub_cols = sub_cols.max(label_cols(text));
    }
    let wa = crate::platform::display::work_area_at(x, y);
    let x = anchor_x_with_submenu_room(
        x,
        est_menu_width(root_cols, dpi),
        est_menu_width(sub_cols, dpi),
        wa.x,
        wa.w,
    );

    let cmd = crate::platform::win_util::menu_track(menu, hwnd, x, y);
    crate::platform::win_util::menu_destroy(menu);
    let Some(cmd) = cmd else {
        return; // 用户点菜单外 / Esc，没选任何项。
    };

    // 透明度子菜单项区间（`CMD_ALPHA_BASE + 1`…`+10`）。
    if (CMD_ALPHA_BASE + 1..=CMD_ALPHA_BASE + 10).contains(&cmd) {
        let pct = ((cmd - CMD_ALPHA_BASE) as u8)
            .saturating_mul(ALPHA_STEP)
            .clamp(ALPHA_MIN, ALPHA_MAX);
        set_pin_alpha(rc, pct);
        return;
    }
    // 「脚本」脚本区间：`CMD_LUA_MENU_BASE + idx`（idx 对应 `lua_items` 的稳定索引）。
    let lua_len = lua_items.len() as i32;
    if (CMD_LUA_MENU_BASE..CMD_LUA_MENU_BASE + lua_len).contains(&cmd) {
        if let Some((id, _)) = lua_items.get((cmd - CMD_LUA_MENU_BASE) as usize) {
            run_lua_ext(rc, id);
        }
        return;
    }
    match cmd {
        CMD_LOCK => toggle_pin_locked(rc, seq),
        CMD_SAVE => save_view_png(seq, None),
        CMD_CLOSE => {
            crate::platform::win_util::post_close_by_title(&pin_title(seq));
        }
        CMD_RESTORE => restore_pin_original(rc, seq),
        CMD_ROT_CW => transform_pin(rc, |b| b.rotate_cw90()),
        CMD_MIRROR_H => transform_pin(rc, |b| b.flip_h()),
        CMD_MIRROR_V => transform_pin(rc, |b| b.flip_v()),
        CMD_PICK => toggle_pin_pick(rc),
        _ => {}
    }
}

/// 「取色」菜单开关（用户口径 2026-10-02："选项前面打勾，再选一次就退出"）：吸管未激活
/// 则进入，已激活则退出。菜单项在激活时打勾（见 `show_context_menu`）。
fn toggle_pin_pick(rc: &Rc<RefCell<PinCanvas>>) {
    if rc.borrow().pick.get() {
        rc.borrow().exit_pick();
        windui::anim::request_relayout();
    } else {
        enter_pin_pick(rc);
    }
}

/// 进入吸管模式（右键菜单「取色」）：悬停显示光标下图片像素的颜色、单击复制。
/// ★ 不再换光标（2026-10-02 用户口径："光标变化不要了，维持原来的"——自绘吸管光标
///   观感反而更差，已删）。
/// 重绘出口与 `transform_pin` 同款——菜单任务里没有 `EventCtx`，
/// 用 `request_relayout` 踢一帧。
fn enter_pin_pick(rc: &Rc<RefCell<PinCanvas>>) {
    {
        let c = rc.borrow();
        c.pick.set(true);
        c.pick_at.set(None);
        c.pick_copied.set(false);
    }
    windui::anim::request_relayout();
}

/// 切换「锁定图片」：**完全锁定**——Esc / 双击关不掉、拖不动、滚轮缩放与透明度全禁
/// （解除或右键「关闭」才消失）。
fn toggle_pin_locked(rc: &Rc<RefCell<PinCanvas>>, seq: u32) {
    let cur = rc.borrow().rt.borrow().pin_locked(seq);
    rc.borrow().rt.borrow_mut().set_pin_locked(seq, !cur);
    log::info!("贴图 #{seq} 锁定状态 -> {}", !cur);
}

/// 存 PNG：「另存为」与 Ctrl+S **共用这一条**（m16 语义收敛的落点）。
///
/// 只管"给一张位图 → 对话框 → 落盘"，像素由调用方交进来。线程边界因此只有一处要说清：
/// 画布像素只能从 [`current_bitmap`]（`thread_local` 登记表）取，而 windui 的
/// `defer_blocking` 排的闭包**同样在 UI 线程**跑（走 `DispatchResult::dialog`，不是新线程），
/// 所以两条入口都在本线程，不需要把图搬过线程。
pub fn save_bitmap_png(seq: u32, bmp: &RgbaBitmap) {
    let Some(path) = crate::platform::file_dialog::save_png(&format!("CurSeen_贴图_{seq}"))
    else {
        log::info!("另存为：用户在对话框里取消");
        return;
    };
    // 复用截图保存链（`bitmap_to_shot` + `save_png`）；α 原样带过去，不刷成实心。
    let Some(shot) = crate::core::backdrop::bitmap_to_shot(bmp) else {
        log::warn!("另存为：位图无法转回帧格式");
        return;
    };
    match crate::platform::screenshot::save_png(&shot, &path) {
        Ok(()) => log::info!("贴图 #{seq} 已另存为 {}", path.display()),
        Err(e) => log::warn!("贴图 #{seq} 另存为失败: {e}"),
    }
}

/// 存**画面上那一张**（旋转 / 镜像 / 脚本回写都算数）。UI 线程调。
///
/// 画布已经不在了（那张窗刚关）才退回 `fallback`——那是 Ctrl+S 那条路上唯一可能的兜底，
/// 且一定记一条日志：**静默把用户看到的内容换成别的**比失败更坏。右键菜单那条入口画布必然
/// 在（菜单就挂在那扇窗上），所以传 `None`：真取不到就如实报"存不了"，不拿出厂图冒充。
pub fn save_view_png(seq: u32, fallback: Option<&RgbaBitmap>) {
    let live = current_bitmap(seq);
    let src = match live.as_ref().or(fallback) {
        Some(s) => s,
        None => {
            log::warn!("另存为：#{seq} 既没有画布也没有出厂图，存不了");
            return;
        }
    };
    if live.is_none() {
        log::warn!("另存为：#{seq} 的画布已经不在了，存出厂那一份");
    }
    save_bitmap_png(seq, src);
}

/// 套用一次几何变换（旋转 / 镜像）到当前位图，并重建渲染对象、同步窗口尺寸。
///
/// 旋转会把宽高互换，所以变换后窗口必须跟着重设（保持左上角不动、缩放百分比不变）。
/// 全程在 `defer_blocking` 任务里跑——同步 `SetWindowPos` 派发的 `WM_SIZE` 不会再撞上
/// 活动的事件回调。变换是**原地生效**（不新贴一张）：`bitmap` 替换成新图、`img` 重建，
/// 窗口尺寸按新图 × 当前百分比重新计算。
fn transform_pin(rc: &Rc<RefCell<PinCanvas>>, f: impl FnOnce(RgbaBitmap) -> RgbaBitmap) {
    let new_bmp = f((*rc.borrow().bitmap).clone());
    set_canvas_bitmap(rc, new_bmp);
}

/// **换掉一张贴图的像素**——右键「旋转 / 镜像」与脚本 `ext:set_image` 共用这一条缝。
///
/// 顺序照 [`transform_pin`] 原本的形状，一处都不许省：
/// 1. 先重建渲染对象（`img` 与 `bitmap` 必须是同一份像素，漏了就拉伸）；
/// 2. 新窗口尺寸 = 新图 × 当前缩放 + 2×`FRAME`（左上角不动）；
/// 3. **写完画布状态、释放可变借用，再动窗口**——`SetWindowPos` 会同步重入本窗的 paint，
///    还持着 `borrow_mut` 就是 `RefCell already mutably borrowed` → panic 整进程崩
///    （用户实测：右键旋转必崩，见下方两段式借用注释）；
/// 4. 踢一次全局重排。
///
/// 返回 `false` = 渲染对象没建起来（像素为 0 或后端拒绝），此时**什么都不改**。
/// 一张位图以 `pct` 缩放显示时，贴图窗**外框**该是多大。
///
/// 单独抽出来是为了能被单测钉住：换图（旋转、脚本回写）最容易坏的就是这一步——
/// 算小了露黑边、算大了留空白，而两者都没有报错，只有眼睛看得见。
fn window_size_for_bitmap(iw: u32, ih: u32, pct: u32) -> (i32, i32) {
    let (iw2, ih2) = zoomed_size(iw, ih, pct);
    (iw2 + 2 * FRAME, ih2 + 2 * FRAME)
}

fn set_canvas_bitmap(rc: &Rc<RefCell<PinCanvas>>, new_bmp: RgbaBitmap) -> bool {
    let (hwnd, dpi, pct) = {
        let c = rc.borrow();
        (c.own_hwnd(), c.dpi, c.pct.get())
    };
    let Ok(new_img) = WImage::from_rgba(new_bmp.w, new_bmp.h, &new_bmp.rgba) else {
        log::warn!(
            "贴图换图：重建渲染对象失败（尺寸 {}×{}）",
            new_bmp.w,
            new_bmp.h
        );
        return false;
    };
    let (iw, ih) = (new_bmp.w, new_bmp.h);
    // 窗口 = 图片 + 2×FRAME 描边；只取尺寸（位置保持左上角不动）。
    let (nw, nh) = window_size_for_bitmap(iw, ih, pct);
    {
        let mut c = rc.borrow_mut();
        c.bitmap = Rc::new(new_bmp);
        c.img = new_img;
        c.target_size.set(Some((
            ((nw as f32 / dpi).round() as i32).max(1),
            ((nh as f32 / dpi).round() as i32).max(1),
        )));
    }
    // ★ 尺寸必须移出上面的 `borrow_mut` 块再改：`SetWindowPos` 会**同步**重入本窗的
    //   paint（WM_SIZE → measure → `rc.borrow()`）。若此刻还持着可变借用，就是
    //   `RefCell already mutably borrowed` → panic(abort) 整进程崩溃（用户实测：右键
    //   旋转/镜像必崩）。先写完画布状态、释放借用，再动窗口，两段式借用。
    if hwnd != 0 {
        // 保持左上角不动，只换尺寸（旋转换宽高）。
        if let Some((wx, wy, _, _)) = crate::platform::win_util::window_rect_hwnd(hwnd) {
            crate::platform::win_util::set_window_rect_hwnd(hwnd, wx, wy, nw, nh);
        }
    }
    windui::anim::request_relayout();
    true
}

// 贴图画布登记表：`seq → 那张窗的画布`。两个读者（m15 脚本回写、m16 的 Ctrl+S）
// 都要"给我一个 seq/hwnd，我把活干在那扇窗上"，而画布平时只活在贴图窗自己的闭包里。
//
// ★ 为什么是 `thread_local` 而不是 `static Mutex`：`Weak<RefCell<PinCanvas>>` 不是 `Send`
//   （`RefCell` 不是 `Sync`），写进 `static` 直接编不过；而登记（建窗时）与消费（覆盖层
//   消息回调、贴图自己的菜单任务）本来就都在 UI 线程。
//
// 存 `Weak`：贴图窗随时可能被用户关掉。握强引用会把关掉的窗吊住不销，那是比"这次没改成"
// 严重得多的泄漏；升级失败 ⇒ 如实报"那张贴图不在了"。死条目在下一次登记时顺手清掉。
thread_local! {
    static PINS: std::cell::RefCell<std::collections::HashMap<u32, std::rc::Weak<RefCell<PinCanvas>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// 建窗时登记（`seq` 在建窗之前就定了，而 `own_hwnd` 要到第一次事件才有值——所以登记
/// 挂在这里，不挂在"脚本触发时"：那会让 Ctrl+S / 回写只对"跑过脚本的贴图"生效）。
fn register_pin(seq: u32, rc: &Rc<RefCell<PinCanvas>>) {
    PINS.with(|m| {
        let mut m = m.borrow_mut();
        m.retain(|_, w| w.strong_count() > 0);
        m.insert(seq, Rc::downgrade(rc));
    });
}

/// 按 `seq` 取画布（Ctrl+S 用：它作用于"选中的那一张"）。
fn pin_by_seq(seq: u32) -> Option<Rc<RefCell<PinCanvas>>> {
    PINS.with(|m| m.borrow().get(&seq).and_then(|w| w.upgrade()))
}

/// 按 HWND 取画布（脚本回写用：脚本手里只有 `ctx.hwnd`）。
///
/// 扫值而不是再开一张 `hwnd → …` 的表：贴图数量是个位到几十的量，两把表只会有一天
/// 一把新一把旧（加图/关图各要记得改两处）。
fn pin_by_hwnd(hwnd: isize) -> Option<Rc<RefCell<PinCanvas>>> {
    if hwnd == 0 {
        return None;
    }
    PINS.with(|m| {
        let m = m.borrow();
        m.values()
            .filter_map(|w| w.upgrade())
            .find(|rc| rc.borrow().own_hwnd() == hwnd)
    })
}

/// UI 线程侧执行一次脚本回写。`Err` 里是给人看的原因（调用方原样回给脚本线程）。
pub fn apply_script_image(hwnd: isize, bitmap: RgbaBitmap) -> Result<(), String> {
    if hwnd == 0 {
        return Err("这一项要在贴图上右键触发：没有指定的那张贴图".to_string());
    }
    // ★ "必须在 UI 线程"这条不需要单独判据：登记表是线程局部的，站错线程只会查不到条目，
    //   自然落到下面那句"找不到那张贴图"。再立一套线程 id 比对，只会有一天两把尺子各说一遍。
    let rc = pin_by_hwnd(hwnd).ok_or_else(|| "找不到那张贴图（可能已经关掉了）".to_string())?;
    if set_canvas_bitmap(&rc, bitmap) {
        Ok(())
    } else {
        Err("换图失败：渲染对象没建起来（像素尺寸为 0 或后端拒绝）".to_string())
    }
}

/// 那张贴图**此刻**的像素（旋转 / 镜像 / 脚本回写之后的样子）。`None` = 那张窗已经不在了。
///
/// 给 Ctrl+S 用：m16 起它存的就是"用户屏幕上看到的那一张"，与右键「另存为」同一语义。
pub fn current_bitmap(seq: u32) -> Option<RgbaBitmap> {
    pin_by_seq(seq).map(|rc| (*rc.borrow().bitmap).clone())
}

/// 那张贴图此刻的窗口句柄（0 / 查不到 ⇒ `None`）。
///
/// 给热键触发的脚本用：脚本拿到 `ctx.hwnd` 才能把结果**回写到被处理的那一张**。
pub fn current_hwnd(seq: u32) -> Option<isize> {
    pin_by_seq(seq)
        .map(|rc| rc.borrow().own_hwnd())
        .filter(|h| *h != 0)
}

/// 这张贴图与"钉进来那一刻"是否已经不同（决定「恢复原图」要不要出现在菜单里）。
fn bitmap_differs(a: &RgbaBitmap, b: &RgbaBitmap) -> bool {
    a.w != b.w || a.h != b.h || a.rgba != b.rgba
}

/// 这张贴图与出厂那份不同吗？（决定「恢复原图」出不出现在菜单里）
///
/// 两段式：先把出厂那份**取成 owned**（`pin_content` 返回克隆），再碰画布。把
/// `rc.borrow().rt.borrow()` 写进 `match` 的 scrutinee 会让这两层借用活到整个 match 体，
/// 臂内再 `borrow()` 就是 panic——门禁 `no_refcell_borrow_in_a_let_chain_scrutinee` 拦的正是这个形状。
fn pin_was_transformed(rc: &Rc<RefCell<PinCanvas>>, seq: u32) -> bool {
    let orig = rc.borrow().rt.borrow().pin_content(seq);
    let cur = rc.borrow().bitmap.clone();
    match orig {
        Some(orig) => bitmap_differs(&cur, orig.bitmap()),
        // 记账里已经没有出厂内容 ⇒ 宁可让这一项出现，也别悄悄不可用。
        None => true,
    }
}

/// 把像素换回**钉进来那一刻那张**。与旋转 / 脚本回写共用 [`set_canvas_bitmap`]，所以窗口
/// 尺寸重算、两段式借用、重排提示都不另立规矩。已经是原图就直接返回（幂等，不闪一下）。
fn restore_pin_original(rc: &Rc<RefCell<PinCanvas>>, seq: u32) {
    if !pin_was_transformed(rc, seq) {
        return;
    }
    let Some(orig) = rc.borrow().rt.borrow().pin_content(seq) else {
        log::warn!("恢复原图：#{seq} 的出厂内容已经不在了");
        return;
    };
    // `bitmap()` 是 `&Rc<RgbaBitmap>`：先剥 `&` 再剥 `Rc`，否则 `.clone()` 复制的是引用。
    let target = (**orig.bitmap()).clone();
    set_canvas_bitmap(rc, target);
}

/// 设置整窗透明度（右键子菜单项 / 兜底入口）。
fn set_pin_alpha(rc: &Rc<RefCell<PinCanvas>>, pct: u8) {
    let (hwnd, p) = (rc.borrow().own_hwnd(), pct.clamp(ALPHA_MIN, ALPHA_MAX));
    rc.borrow().alpha_pct.set(p);
    if hwnd != 0 {
        crate::platform::win_util::set_window_alpha(hwnd, pct_to_alpha(p));
    }
}

/// 从「脚本」子菜单选中脚本 `id`，把当前贴图 + 该脚本的参数脱手给后台执行器跑。
/// 全程不碰 UI 线程（脚本可能阻塞 HTTP 60s），结果由执行器线程自己弹框。
fn run_lua_ext(rc: &Rc<RefCell<PinCanvas>>, id: &str) {
    let (bmp, hwnd) = (rc.borrow().bitmap.clone(), rc.borrow().own_hwnd());
    // 回写靠 [`pin_by_hwnd`] 找回画布；登记在建窗时（见 `open` 里的 `register_pin`）而不是
    // 这里——否则只有"跑过一次脚本的贴图"才回写得动，而 Ctrl+S /「恢复原图」用的是同一张表。
    let ext_dir = rc.borrow().rt.borrow().lua_ext_dir().to_path_buf();
    let params = rc.borrow().rt.borrow().lua_ext_params(id);
    crate::scripting::ext_runner::run_ext_detached(
        &ext_dir,
        id,
        &params,
        Some(crate::scripting::ext_runner::ExtInput {
            image: (*bmp).clone(),
            hwnd: hwnd as usize,
        }),
    );
}

/// 贴着屏幕的**尺寸上限比例**（只对"没有原位"的剪贴板内容生效）。
const CLIPBOARD_MAX_SCREEN_RATIO: f32 = 0.9;

/// 贴图窗的落点与尺寸（全部物理像素）：`(x, y, w, h)`。
///
/// 纯函数（不碰窗口、不碰 Runtime），所以"贴图到底贴在哪"这件事可以在单测里钉死——
/// 用户报过"贴图位置不对"，而位置只能量出来，不能靠看。
///
/// | `origin` | 落点 | 尺寸 |
/// |---|---|---|
/// | `Some(r)`（截图来的） | `screen.x + r.x`（含副屏负坐标） | `r.w × r.h`，**1:1 不缩放** |
/// | `None`（剪贴板 / 文字） | 虚拟桌面**居中** | 超屏才按 `0.9` 等比缩小 |
///
/// `origin` 一档刻意**不套 90% 上限**：整屏截图就该整屏贴回去（用户口径："位置要一样的"），
/// 缩放会让"位置一样"变成"位置差不多"。
fn pin_placement(
    bmp_w: u32,
    bmp_h: u32,
    screen: ScreenRect,
    origin: Option<LRect>,
) -> (i32, i32, i32, i32) {
    match origin {
        Some(r) => {
            let w = (r.w.round() as i32).max(16);
            let h = (r.h.round() as i32).max(16);
            let (x, y) = crate::platform::win_util::clamp_pos(
                screen.x.saturating_add(r.x.round() as i32),
                screen.y.saturating_add(r.y.round() as i32),
                w,
                h,
                screen,
            );
            (x, y, w, h)
        }
        None => {
            let max_w = (screen.w as f32 * CLIPBOARD_MAX_SCREEN_RATIO).max(40.0);
            let max_h = (screen.h as f32 * CLIPBOARD_MAX_SCREEN_RATIO).max(40.0);
            let scale = (max_w / bmp_w as f32).min(max_h / bmp_h as f32).min(1.0);
            let w = ((bmp_w as f32 * scale).round() as i32).max(16);
            let h = ((bmp_h as f32 * scale).round() as i32).max(16);
            let x = screen.x + (screen.w.saturating_sub(w)) / 2;
            let y = screen.y + (screen.h.saturating_sub(h)) / 2;
            let (x, y) = crate::platform::win_util::clamp_pos(x, y, w, h, screen);
            (x, y, w, h)
        }
    }
}

/// 打开一个贴图窗。
///
/// `screen` 是虚拟桌面（物理像素）；窗口尺寸按 `WorldMap` 换算成逻辑 dp 交给
/// `Window::new`，位置用物理像素交给 `Window::position`——两个 API 的单位不同，
/// 混用就是"贴图窗比预期大 1.5 倍"的来源。
///
/// `origin` = 这张图的**原位**（冻结帧坐标系，物理像素），见
/// [`crate::runtime::snap::PinRequest::origin`]：
///
/// | 来源 | 落点 | 尺寸 |
/// |---|---|---|
/// | 截图并贴图（`Some`） | **截图时那一块**（原位，含副屏负坐标） | 1:1，**不缩放** |
/// | 剪贴板图 / 剪贴板文字（`None`） | 屏幕居中 | 超屏时缩到 90% 以内 |
///
/// 用户口径（2026-09-17）：*"pin 图时，位置应该是截图的位置，位置要一样的"*、
/// *"如果不是我们截图的，而是其它途径复制到剪贴板的，或者剪贴板文字，那么就显示在
/// 屏幕居中位置"*。原位一档刻意**不套 90% 上限**：整屏截图就该整屏贴回去，缩放会让
/// "位置一样"变成"位置差不多"。
///
/// 窗口比**图片**大 `2 × FRAME`（四周各 [`FRAME`] 物理像素），那两圈是描边（见模块头）；
/// 图片本身仍按上表落位，1:1 不缩。
pub fn open(
    ctx: &mut EventCtx,
    rt: &Rc<std::cell::RefCell<Runtime>>,
    content: &PinContent,
    seq: u32,
    screen: ScreenRect,
    origin: Option<LRect>,
) {
    let bmp = content.bitmap();
    let map = WorldMap::new(screen);
    // `pin_placement` 给的是**图片**矩形（截图原位 1:1，位置不动）；窗口再往外撑出描边宽度。
    let (px, py, phys_w, phys_h) = pin_placement(bmp.w, bmp.h, screen, origin);
    let (wx, wy, ww, wh) = frame_inflate((px, py, phys_w, phys_h));

    let (lw, lh) = (
        (map.len_to_window(ww as f32).round() as i32).max(16),
        (map.len_to_window(wh as f32).round() as i32).max(16),
    );

    let img = match WImage::from_rgba(bmp.w, bmp.h, &bmp.rgba) {
        Ok(i) => i,
        Err(_) => return,
    };

    let title = pin_title(seq);
    let widget = PinWidget(Rc::new(RefCell::new(PinCanvas {
        rt: rt.clone(),
        seq,
        // 出厂位图 = 内容位图（未变换）。右键菜单旋转/镜像后这里被替换。
        // `bmp` 是 `&Rc<RgbaBitmap>`（见 `PinContent::bitmap`），clone 即复制这份引用计数。
        bitmap: bmp.clone(),
        img,
        // ★ DPI 比例必须从 `WorldMap` 传进来，不能在 `paint`/`on_wheel` 里现调
        //   `display::dpi_scale()`：那会让单测依赖跑测试那台机器的缩放（见 `PinCanvas::dpi`）。
        dpi: map.dpi(),
        // 出厂 100%（图片原始像素），滚轮才改它。
        pct: Cell::new(100),
        // 滚轮累计量（攒够一个 `WHEEL_DELTA` 才动一档）。
        wheel: Cell::new(0),
        hwnd: Cell::new(0),
        drag: Cell::new(None),
        hover: Cell::new(false),
        // 出厂无命令尺寸（100% 即图片原始像素，由布局 `bounds` 决定窗口大小）。
        target_size: Cell::new(None),
        // 出厂完全不透明（100%），右键菜单 / Ctrl+滚轮才改它。
        alpha_pct: Cell::new(100),
        // 取色模式只由右键菜单「取色」进入，出厂恒关。
        pick: Cell::new(false),
        pick_at: Cell::new(None),
        pick_copied: Cell::new(false),
    })));
    // 登记画布（m16）：脚本回写按 `hwnd` 找、Ctrl+S 与「恢复原图」按 `seq` 找，都靠这张表。
    register_pin(seq, &widget.0);
    let key = format!("snap_pin_{seq}");
    // 关窗收尾：不管谁关的（自身双击/右键、Esc 经 `post_close_by_title`、超上限淘汰），
    // 都要把记账摘掉。以前只有控件自己的 `close()` 摘——Esc 走的是 `WM_CLOSE`，
    // `on_close_request` 才是那条路的唯一入口，漏了它会留下陈旧序号：贴图计数虚高、
    // 还没到上限就开始淘汰，且选中态可能指向一个已经不在的窗口。
    let rt_close = rt.clone();
    ctx.open_window(
        Window::new(&title, lw, lh)
            .single(&key)
            .owned(true)
            // ★ 无边框 + 不可缩放：外圈不再吞掉 LButtonDown，双击取消才生效。
            .frameless(true)
            // ★ 弹出式样式（windui 补丁 095）：**建窗即 `WS_POPUP`**。
            //
            // 为什么必须有这一行：固定大小 + frameless 的窗样式仍是 `WS_OVERLAPPEDWINDOW`
            // 一族，`CreateWindowExW` / `SetWindowPos` 会按 `WM_GETMINMAXINFO` 的跟踪尺寸
            // 收口——宽硬下限 `SM_CXMINTRACK`（本机实测 136，`ptMinTrackSize` 写 1 也压不到
            // 更低），高上限约一屏（本机实测 1100）。于是：
            //   ① 窄长条图（多行短文字栅格化出来的，如 60×720）窗口被顶到 136 宽，
            //      `fit_aspect` 把图居中，两侧各露出一圈黑边；
            //   ② 长图放大到窗高越过上限后**图片高度被钉死**，而窗口另一维还在按百分比长
            //      ——图片尺寸冻结、外框继续变大，正是用户报的"图片没放大、只是边框在放大"。
            // `WS_POPUP` 两项收口全部消失（`tests/probes/pin_zoom_clamp_probe.rs` 变体 3
            // 实测：2×80 / 3000×80 / 60×4000 原样授予），窗口尺寸才真正等于
            // "图片 × 缩放 + 2×FRAME"。
            //
            // 与 `.frameless(true)` 不重复：那个只抹非客户区（`WM_NCCALCSIZE`），样式族不变；
            // 这个换掉整个样式族。换了之后 frameless 的其余机制不受影响——`WM_NCCALCSIZE`
            // 读宿主状态、`WM_NCHITTEST` 的缩放热区判据是 `WS_THICKFRAME`（新旧样式都没有）。
            // **别删**：删了就是"极端宽高比贴图图片不缩放"复发。
            .popup(true)
            .resizable(false)
            .position(wx, wy)
            // ★ 吃掉本窗的 Escape（补丁 026）：贴图窗不抢焦点，但一旦它拿到键盘焦点，
            //   没被消费的 Escape 会落进 windui 的 `Key::Escape if resolve_close()` 兜底
            //   把它自己关掉——叠加 Runtime 派发的"关选中那张"，就是用户报的"一次 Esc
            //   关两张"。这里声明 Escape 归本窗，兜底就轮不到（语义仍只由 Runtime 处理）。
            //   别删：删了就是一次 Esc 关两张复发。
            .on_shortcut(|_ctx, ev| ev.key == Key::Escape)
            .on_close_request(move |_ctx| {
                // ★ 每一次关窗都留一条日志（含序号）。用户报过"按一次 Esc 两张图一起消失"，
                //   而 Esc 那条路只会关**一个**序号——所以这条日志是分辨"真的一次关俩"还是
                //   "两次点击 + 两次 Esc 各关一个"的唯一凭据。别删。
                log::info!("贴图窗 #{seq} 关闭（记账摘除）");
                rt_close.borrow_mut().note_pin_closed(seq);
                true
            })
            // 不设窗口底色：贴图铺满整个客户区（见 `paint`），底色永远不会露出来。
            // 旧实现设了深色底（18,22,26），而客户区比图片大一圈（windui 补丁 024 之前
            // 的尺寸反算把标题栏/边框也算进了外框），那圈底色就是用户看到的"黑边"。
            // `widget` 是 `PinWidget`（持 `Rc<RefCell<PinCanvas>>`）：`clone()` 只是加一次引用
            // 计数，闭包每次构建元素都能拿到一份——所有权始终安全。
            .content(move || Element::leaf().widget(widget.clone()).fill()),
    );
    // 贴图不抢焦点：不打断用户当前输入。
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::event::PointerEvent;

    /// 一张 2×2 的白图 + 出厂配置的运行期（贴图手势不读运行时状态，够用）。
    ///
    /// 返回 `PinWidget`（对 `Rc<RefCell<PinCanvas>>` 的封装）：贴图窗现在以它为 widget，
    /// 右键菜单拦截就发生在这层（`Rc` 让 `defer_blocking` 任务能访问画布）。事件测试都
    /// 用 `PinWidget::on_event` 走真实路径。
    fn canvas() -> PinWidget {
        let cfg = crate::core::config::Config::default();
        let rt = Rc::new(std::cell::RefCell::new(Runtime::new(cfg)));
        let bmp = RgbaBitmap::new(2, 2, vec![255_u8; 2 * 2 * 4]).expect("2×2 位图");
        let img = WImage::from_rgba(2, 2, &bmp.rgba).expect("2×2 位图");
        PinWidget(Rc::new(RefCell::new(PinCanvas {
            rt,
            seq: 1,
            bitmap: Rc::new(bmp),
            img,
            dpi: 1.0,
            pct: Cell::new(100),
            wheel: Cell::new(0),
            hwnd: Cell::new(0),
            drag: Cell::new(None),
            hover: Cell::new(false),
            target_size: Cell::new(None),
            alpha_pct: Cell::new(100),
            pick: Cell::new(false),
            pick_at: Cell::new(None),
            pick_copied: Cell::new(false),
        })))
    }

    fn ev(kind: PointerKind, button: MouseButton) -> Event {
        Event::Pointer(PointerEvent::single(kind, Point::new(1, 1), button))
    }

    /// ★ 取色映射：窗口逻辑坐标 → 图片像素必须逐值正确（居中 fit 的偏移要吃掉）。
    /// "看到哪、取到哪"全押在这条上。
    #[test]
    fn image_pixel_at_maps_through_the_fitted_rect() {
        // 4×4 位图显示成 8×8 矩形（放大 2 倍），矩形左上角在 (10, 20)。
        let img = Rect::new(10, 20, 8, 8);
        assert_eq!(image_pixel_at(4, 4, img, (10.0, 20.0)), Some((0, 0)));
        assert_eq!(image_pixel_at(4, 4, img, (11.9, 21.9)), Some((0, 0)));
        assert_eq!(image_pixel_at(4, 4, img, (12.0, 20.0)), Some((1, 0)));
        assert_eq!(image_pixel_at(4, 4, img, (17.9, 27.9)), Some((3, 3)));
        // 越界：左/上一侧、右/下侧、远端都不许返回像素。
        assert_eq!(image_pixel_at(4, 4, img, (9.9, 20.0)), None);
        assert_eq!(image_pixel_at(4, 4, img, (10.0, 19.9)), None);
        assert_eq!(image_pixel_at(4, 4, img, (18.0, 20.0)), None);
        assert_eq!(image_pixel_at(4, 4, img, (10.0, 28.0)), None);
        // 退化：空位图 / 空矩形恒 None。
        assert_eq!(image_pixel_at(0, 0, img, (10.0, 20.0)), None);
        assert_eq!(
            image_pixel_at(4, 4, Rect::new(0, 0, 0, 0), (0.0, 0.0)),
            None
        );
    }

    /// 取样：越界拒收；RGB 三通道逐值对上（BGRA 内存序别拿反）。
    #[test]
    fn sample_rgb_reads_the_right_channels_and_rejects_out_of_range() {
        // 2×2：像素 (x,y) 的 RGB = (x*10+y, 200+x, 100+y)，A=255。
        let mut rgba = Vec::new();
        for y in 0..2u8 {
            for x in 0..2u8 {
                rgba.extend_from_slice(&[x * 10 + y, 200 + x, 100 + y, 255]);
            }
        }
        let bmp = RgbaBitmap::new(2, 2, rgba).expect("2×2 位图");
        assert_eq!(sample_rgb(&bmp, (0, 0)), Some([0, 200, 100]));
        assert_eq!(sample_rgb(&bmp, (1, 0)), Some([10, 201, 100]));
        assert_eq!(sample_rgb(&bmp, (0, 1)), Some([1, 200, 101]));
        assert_eq!(sample_rgb(&bmp, (1, 1)), Some([11, 201, 101]));
        assert_eq!(sample_rgb(&bmp, (2, 0)), None, "越界必须拒收");
        assert_eq!(sample_rgb(&bmp, (0, 2)), None);
    }

    /// 徽标分行文本：HEX/RGB/HSL 三行、带名字（用户口径 2026-10-02），复制过时首行加
    /// 「已复制 · 」前缀。
    #[test]
    fn pick_lines_formats_the_three_named_color_systems() {
        assert_eq!(
            pick_lines([0x0A, 0x14, 0x1E], false),
            vec!["HEX: #0A141E", "RGB: 10,20,30", "HSL: 210°,50%,8%"]
        );
        assert_eq!(
            pick_lines([255, 255, 255], false),
            vec!["HEX: #FFFFFF", "RGB: 255,255,255", "HSL: 0°,0%,100%"]
        );
        assert_eq!(
            pick_lines([12, 34, 56], true),
            vec![
                "已复制 · HEX: #0C2238",
                "RGB: 12,34,56",
                "HSL: 210°,65%,13%"
            ]
        );
    }

    /// RGB → HSL 换算逐值钉死：黑/白/灰（S=0）、纯红（0°）、纯绿（120°）、纯蓝（240°）
    /// ——色相偏移、明度颠倒这类错在真机上肉眼难查。
    #[test]
    fn rgb_to_hsl_matches_the_reference_values() {
        assert_eq!(rgb_to_hsl([0, 0, 0]), (0, 0, 0));
        assert_eq!(rgb_to_hsl([255, 255, 255]), (0, 0, 100));
        assert_eq!(rgb_to_hsl([128, 128, 128]), (0, 0, 50));
        assert_eq!(rgb_to_hsl([255, 0, 0]), (0, 100, 50));
        assert_eq!(rgb_to_hsl([0, 255, 0]), (120, 100, 50));
        assert_eq!(rgb_to_hsl([0, 0, 255]), (240, 100, 50));
        // 环绕：纯品红 300°，不得跑成负数。
        assert_eq!(rgb_to_hsl([255, 0, 255]), (300, 100, 50));
    }

    /// ★ 截图来的贴图必须**落在截图时那一块**，尺寸一模一样（1:1，不缩放）。
    ///
    /// 用户口径："pin 图时，位置应该是截图的位置，位置要一样的。" 这条把落点算成数字来钉：
    /// 冻结帧坐标 (410, 260) 300×180 在虚拟桌面原点 (100, 50) 下 → 屏幕 (510, 310)。
    #[test]
    fn screenshot_pin_lands_exactly_on_the_captured_rect() {
        let screen = ScreenRect {
            x: 100,
            y: 50,
            w: 1920,
            h: 1080,
        };
        let region = LRect {
            x: 410.0,
            y: 260.0,
            w: 300.0,
            h: 180.0,
        };
        let (x, y, w, h) = pin_placement(300, 180, screen, Some(region));
        assert_eq!((x, y), (510, 310), "落点 = 虚拟桌面原点 + 选区原点");
        assert_eq!((w, h), (300, 180), "尺寸 1:1，不许被缩放");
    }

    /// ★ 窗口比图片大 `2 × FRAME`：两层描边画在图片**外面**（灰色的图 + 灰色的桌面也能分辨），
    /// 而图片本身仍然 1:1 落在截图原位——用户要的"位置要一样的"针对的是图，不是窗口。
    #[test]
    fn pin_window_reserves_a_ring_around_the_image() {
        let screen = ScreenRect {
            x: 100,
            y: 50,
            w: 1920,
            h: 1080,
        };
        let region = LRect {
            x: 410.0,
            y: 260.0,
            w: 300.0,
            h: 180.0,
        };
        let image = pin_placement(300, 180, screen, Some(region));
        assert_eq!(image, (510, 310, 300, 180), "图片仍在截图原位、1:1");
        let win = frame_inflate(image);
        assert_eq!(win, (508, 308, 304, 184), "窗口四周各多 FRAME 像素用于描边");
        assert_eq!(
            (win.2 - image.2, win.3 - image.3),
            (2 * FRAME, 2 * FRAME),
            "多出来的宽度必须是 2×FRAME（两层各 1px，正好对上 stroke_ring 的 inset 0/1）"
        );
    }

    /// 整屏截图也要整屏贴回去——**不能被 90% 上限缩掉**（剪贴板那一档才缩）。
    #[test]
    fn full_screen_screenshot_is_not_shrunk() {
        let screen = ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        };
        let whole = LRect {
            x: 0.0,
            y: 0.0,
            w: 1920.0,
            h: 1080.0,
        };
        assert_eq!(
            pin_placement(1920, 1080, screen, Some(whole)),
            (0, 0, 1920, 1080),
            "整屏截图必须原样贴回（若套了 0.9 上限会变成 1728×972）"
        );
    }

    /// 副屏在主屏左侧（虚拟桌面负原点）时，原位贴图也要落在正确的负坐标上。
    #[test]
    fn origin_pin_handles_a_negative_virtual_desktop() {
        let screen = ScreenRect {
            x: -1920,
            y: 0,
            w: 3840,
            h: 1080,
        };
        let region = LRect {
            x: 100.0,
            y: 200.0,
            w: 640.0,
            h: 480.0,
        };
        let (x, y, w, h) = pin_placement(640, 480, screen, Some(region));
        assert_eq!((x, y, w, h), (-1820, 200, 640, 480));
    }

    /// ★ 没有原位（剪贴板图 / 剪贴板文字）→ **屏幕居中**；超屏的等比缩到 90% 以内。
    #[test]
    fn clipboard_pin_is_centered_and_capped() {
        let screen = ScreenRect {
            x: 0,
            y: 0,
            w: 1000,
            h: 800,
        };
        // 小图：不缩，居中
        let (x, y, w, h) = pin_placement(200, 100, screen, None);
        assert_eq!((w, h), (200, 100));
        assert_eq!((x, y), (400, 350), "居中：(1000-200)/2, (800-100)/2");
        // 大图：等比缩到 0.9 屏以内（1000×0.9 = 900 是长边上限）
        let (x, y, w, h) = pin_placement(2000, 1000, screen, None);
        assert_eq!((w, h), (900, 450));
        assert_eq!((x, y), (50, 175));
    }

    /// ★ 回归：**松开左键必须显式归还指针捕获**。
    ///
    /// 用户报的症状是"贴了多张，但只有其中一张能拖动"。根因在 `on_event`：按下时
    /// `ctx.capture()` 申请了捕获，松开时没还——而 windui 的捕获**不会**在 Up 自动解除
    /// （只有 `ctx.release_capture()` / `WM_CAPTURECHANGED` 才解），于是那一个窗口从此
    /// **永久**持有 OS 鼠标捕获，全屏的按下都被它收走：鼠标点在别的贴图上，事件也归它，
    /// 结果只有它能被拖动。这条测试直接盯住"Up 有没有上报释放"。
    #[test]
    fn left_button_up_releases_pointer_capture() {
        let mut c = canvas();
        let down = windui::testing::run_with_ctx(|ctx| {
            let _ = c.on_event(ctx, &ev(PointerKind::Down, MouseButton::Left));
        });
        assert!(
            matches!(down.capture, Some(Some(_))),
            "按下必须申请捕获：拖到窗外也要继续收到移动，否则贴图拖一半就断"
        );
        let up = windui::testing::run_with_ctx(|ctx| {
            let _ = c.on_event(ctx, &ev(PointerKind::Up, MouseButton::Left));
        });
        assert_eq!(
            up.capture,
            Some(None),
            "松开左键必须显式归还指针捕获（漏了它 = 这个窗口永久霸占鼠标）"
        );
    }

    /// ★ 左键按下即**选中本窗**（用户口径：Esc 只关选中的那张、Ctrl+S 只另存那张）。
    ///
    /// 选中态存在 `Runtime` 里（跨窗口唯一），所以这条断言读的是运行时的选中项——
    /// 一旦哪天"点击不再写选中态"，Esc / Ctrl+S 就会指向上一张，甚至指向空气。
    #[test]
    fn left_down_selects_this_pin() {
        let mut c = canvas();
        assert_eq!(
            c.0.borrow().rt.borrow().selected_pin(),
            None,
            "初始没有选中项"
        );
        let _ = windui::testing::run_with_ctx(|ctx| {
            let _ = c.on_event(ctx, &ev(PointerKind::Down, MouseButton::Left));
        });
        assert_eq!(
            c.0.borrow().rt.borrow().selected_pin(),
            Some(1),
            "点过的贴图即选中项"
        );
        assert!(c.0.borrow().rt.borrow().pin_is_selected(1));
        assert!(
            !c.0.borrow().rt.borrow().pin_is_selected(2),
            "别的贴图不该被连坐"
        );
    }

    /// ★ 右键不再关窗，改弹操作菜单：**不请求关窗**、也不申请捕获（右键菜单的弹出延后到
    /// `defer_blocking`，离屏单测没有真窗口 hwnd=0，跳过 `TrackPopupMenu`，只验"不关窗"）。
    #[test]
    fn right_click_opens_menu_without_closing_or_capture() {
        let mut c = canvas();
        let res = windui::testing::run_with_ctx(|ctx| {
            let _ = c.on_event(ctx, &ev(PointerKind::Down, MouseButton::Right));
        });
        assert!(!res.close, "右键现在弹操作菜单，不应直接关窗");
        assert_eq!(res.capture, None, "右键那条路不该动捕获");
    }

    /// 左键**双击**直接关窗，且同样不申请捕获。
    #[test]
    fn double_click_closes_without_taking_capture() {
        let mut c = canvas();
        let res = windui::testing::run_with_ctx(|ctx| {
            let mut pe =
                PointerEvent::single(PointerKind::Down, Point::new(1, 1), MouseButton::Left);
            pe.click_count = 2;
            let _ = c.on_event(ctx, &Event::Pointer(pe));
        });
        assert!(res.close, "双击应请求关窗");
        assert_eq!(res.capture, None, "关窗那条路不该动捕获");
    }

    // ── 描边像素验收（用户口径 3）──
    //
    // 几何上"画了哪些矩形"看不出来，而描边**几层、什么颜色**恰恰只能靠像素说话。
    // 用一张**纯中灰**的图（模拟"图灰、背景也灰"这种最难分辨的场合），离屏渲出来数像素。

    const PW: u32 = 40;
    const PH: u32 = 30;
    /// 探针图片的灰（`128,128,128`）——刻意选"和什么背景都容易糊在一起"的值。
    const IMG_GRAY: (u8, u8, u8) = (128, 128, 128);

    /// 离屏渲染贴图画布 → 不透明画面（预乘到白底，与平台 `UpdateLayeredWindow` 同义）。
    ///
    /// `win_w`/`win_h`：离屏缓冲（= 真实窗口客户区）像素尺寸。`stale_w`/`stale_h`：windui
    /// 传入 `paint` 的布局 `bounds`（缩放后 windui 不重排 → 它会停在旧尺寸，用来复现 bug）。
    /// `target`：本窗命令尺寸（`on_wheel` 算好的逻辑窗口尺寸，bug 修复后 `paint` 的回退源）。
    /// `pct` 注入缩放百分比（验证左上角百分比角标）。
    fn render_pin_window(
        win_w: u32,
        win_h: u32,
        stale_w: i32,
        stale_h: i32,
        target: Option<(i32, i32)>,
        selected: bool,
        pct: u32,
    ) -> Vec<u8> {
        let rt = Rc::new(std::cell::RefCell::new(Runtime::new(
            crate::core::config::Config::default(),
        )));
        if selected {
            rt.borrow_mut().select_pin(1);
        }
        let mut rgba = vec![0_u8; (PW * PH * 4) as usize];
        for c in rgba.chunks_exact_mut(4) {
            c[0] = IMG_GRAY.0;
            c[1] = IMG_GRAY.1;
            c[2] = IMG_GRAY.2;
            c[3] = 255;
        }
        let bmp = RgbaBitmap::new(PW, PH, rgba).expect("灰位图");
        let img = WImage::from_rgba(PW, PH, &bmp.rgba).expect("灰图");
        let widget = PinWidget(Rc::new(RefCell::new(PinCanvas {
            rt,
            seq: 1,
            bitmap: Rc::new(bmp),
            img,
            dpi: 1.0,
            pct: Cell::new(pct),
            wheel: Cell::new(0),
            hwnd: Cell::new(0),
            drag: Cell::new(None),
            hover: Cell::new(false),
            target_size: Cell::new(target),
            alpha_pct: Cell::new(100),
            pick: Cell::new(false),
            pick_at: Cell::new(None),
            pick_copied: Cell::new(false),
        })));

        use windui::render::{DibTarget, RenderTarget};
        use windui::text::DWriteEngine;
        let mut ann = vec![0_u8; (win_w * win_h * 4) as usize];
        let mut engine = DWriteEngine::new();
        {
            let mut target = DibTarget::new(&mut ann, win_w, win_h);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            Widget::paint(
                &widget,
                Rect::new(0, 0, stale_w, stale_h),
                Rect::new(0, 0, stale_w, stale_h),
                false,
                true,
                &mut *canvas,
                &Style::default(),
            );
        }
        let mut frame = vec![255_u8; (win_w * win_h * 4) as usize];
        for (d, s) in frame.chunks_exact_mut(4).zip(ann.chunks_exact(4)) {
            let a = s[3] as f32 / 255.0;
            let inv = 1.0 - a;
            d[0] = (s[0] as f32 + d[0] as f32 * inv).round() as u8;
            d[1] = (s[1] as f32 + d[1] as f32 * inv).round() as u8;
            d[2] = (s[2] as f32 + d[2] as f32 * inv).round() as u8;
            d[3] = 255;
        }
        frame
    }

    /// 默认离屏渲染：窗口 = 图片(40×30)+描边，`bounds` 与窗口一致（无缩放），`target=None`。
    fn render_pin(selected: bool, pct: u32) -> Vec<u8> {
        render_pin_window(PW, PH, PW as i32, PH as i32, None, selected, pct)
    }

    fn px(buf: &[u8], x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * PW + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    /// 同 `px` 但按实际缓冲宽度索引（回归测试用更宽的缓冲）。
    fn pxw(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * w + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    /// ★ 贴图四周是**两层**描边：最外 1px 深色、第 2 个像素浅色，再往里才是图片。
    ///
    /// 这是用户口径 3 正解（*"增加 2 层边框线……这样可能操作既简单又有层次"*）的像素证据。
    /// 两层一深一浅 → 任何背景色下至少有一层能和背景拉开；只有 1px 灰边时，"灰图 + 灰边 +
    /// 灰桌面"就完全分不出来（用户报的原话）。
    #[test]
    fn pin_draws_two_layers_of_border_around_the_image() {
        let buf = render_pin(false, 100);
        // 左上角：外环 → 内环 → 图片，三级递进
        assert_eq!(px(&buf, 0, 0), RING_OUTER, "最外 1px 应是深色环");
        assert_eq!(px(&buf, 1, 1), RING_INNER, "第 2 个像素应是浅色环");
        assert_eq!(px(&buf, 2, 2), IMG_GRAY, "再往里才是图片本身（描边不盖图）");
        // 四条边都要有（防"只画了左上两条边"）
        assert_eq!(px(&buf, 39, 0), RING_OUTER, "上边");
        assert_eq!(px(&buf, 0, 29), RING_OUTER, "左边");
        assert_eq!(px(&buf, 38, 1), RING_INNER, "右边内环");
        assert_eq!(px(&buf, 1, 28), RING_INNER, "下边内环");
        assert_eq!(px(&buf, 38, 28), RING_INNER, "右下角内环");
    }

    /// 选中时**外环**换成主色蓝（内环保持浅色，蓝色与图片之间仍有一道分隔）。
    ///
    /// 为什么换外环而不是内环：外环紧贴桌面，蓝色在那里对比最强；内环留着浅色，
    /// 免得深色选中框直接压在中灰图片上、边界反而糊掉。
    #[test]
    fn selected_pin_swaps_the_outer_ring_to_the_accent_colour() {
        let buf = render_pin(true, 100);
        assert_eq!(px(&buf, 0, 0), RING_SELECTED, "选中 → 外环主色蓝");
        assert_eq!(px(&buf, 1, 1), RING_INNER, "内环保持不变");
        assert_eq!(px(&buf, 2, 2), IMG_GRAY, "图片仍不被描边盖掉");
    }

    // ── 缩放纯函数验收（用户口径：一格 10%、下限 10%、上限 800%）──

    /// 每格 10% 步进，且钳在 [`ZOOM_MIN`]–[`ZOOM_MAX`]。
    #[test]
    fn zoom_step_clamps_to_the_min_and_max() {
        assert_eq!(zoom_step(100, 1), 110, "一格 +10%");
        assert_eq!(zoom_step(100, -1), 90, "一格 -10%");
        assert_eq!(zoom_step(20, -5), ZOOM_MIN, "下限 10% 钳住");
        assert_eq!(zoom_step(10, -100), ZOOM_MIN, "已 10% 还往下滚保持 10%");
        assert_eq!(zoom_step(700, 20), ZOOM_MAX, "上限 800% 钳住");
        assert_eq!(zoom_step(800, 100), ZOOM_MAX, "已 800% 还往上滚保持 800%");
    }

    // ── 右键菜单：列宽估算与锚点留白钳位（2026-09-26 子菜单防遮挡）──

    #[test]
    fn label_cols_counts_ascii_half_and_cjk_full() {
        assert_eq!(label_cols(""), 0);
        assert_eq!(label_cols("100%"), 4, "纯 ASCII 各半列");
        assert_eq!(
            label_cols("顺时针 90°"),
            6 + 1 + 2 + 2,
            "3 汉字 + 空格 + 90 + °（非 ASCII 整列）"
        );
        assert_eq!(label_cols("另存为…"), 6 + 2, "… 是 U+2026，整列");
    }

    #[test]
    fn menu_width_estimate_scales_with_cols_and_dpi() {
        assert_eq!(est_menu_width(0, 1.0), 56, "纯 chrome 下限");
        assert_eq!(est_menu_width(13, 1.0), 13 * 9 + 56, "96dpi 逐列累加");
        assert_eq!(
            est_menu_width(13, 1.5),
            ((13.0 * 9.0 + 56.0) * 1.5) as i32,
            "150% 整体放大"
        );
        assert_eq!(
            est_menu_width(10, 0.0),
            est_menu_width(10, 1.0),
            "dpi 退化按 1.0 算"
        );
    }

    #[test]
    fn menu_anchor_reserves_room_for_the_submenu() {
        // 1920 宽的屏：锚点 1800 太靠右，钳到 1920-170-160=1590，子菜单向右展开不翻转。
        assert_eq!(anchor_x_with_submenu_room(1800, 170, 160, 0, 1920), 1590);
        // 本来就有空间：原样保留。
        assert_eq!(anchor_x_with_submenu_room(100, 170, 160, 0, 1920), 100);
        // 落点比钳位线还靠左：不往右推（max 只兜"负出工作区"的底）。
        assert_eq!(anchor_x_with_submenu_room(50, 170, 160, 0, 1920), 50);
        // 屏幕窄到连基准都放不下：钳到工作区左缘。
        assert_eq!(anchor_x_with_submenu_room(500, 900, 800, 0, 1000), 0);
        // 副屏在主屏左侧（工作区 x=-1920）：负坐标同样成立。
        assert_eq!(
            anchor_x_with_submenu_room(-100, 170, 160, -1920, 1920),
            -330,
            "菜单+子菜单会越过本屏右缘（x=0），钳到 -330"
        );
        assert_eq!(
            anchor_x_with_submenu_room(-1700, 170, 160, -1920, 1920),
            -1700,
            "左侧空间充足时原样保留"
        );
    }

    /// 缩放后的图片物理尺寸 = 原尺寸 × 百分数（绝不退化成 0）。
    #[test]
    fn zoomed_size_scales_the_image_by_the_percentage() {
        assert_eq!(zoomed_size(100, 100, 100), (100, 100), "100% = 原尺寸");
        assert_eq!(zoomed_size(100, 100, 200), (200, 200), "200% 翻倍");
        assert_eq!(zoomed_size(10, 20, 50), (5, 10), "50% 减半");
        assert!(zoomed_size(100, 100, 10).0 >= 1, "下限 10% 也不退化成 0 宽");
    }

    /// ★ 极端宽高比下，窗口尺寸必须**精确**等于"图片 × 缩放 + 2×FRAME"。
    ///
    /// 这条几何不变式是 windui 补丁 095（贴图窗建窗即 `WS_POPUP`）要守住的东西。算式本身
    /// 一直是对的，但修前它**落不到窗口上**：固定大小 + frameless 的窗样式仍是
    /// `WS_OVERLAPPEDWINDOW` 一族，`CreateWindowExW` / `SetWindowPos` 按 `WM_GETMINMAXINFO`
    /// 的跟踪尺寸收口——窄图被顶到 `SM_CXMINTRACK`（本机 136）、长图被限到约一屏高
    /// （本机 1100）。于是 60×720 的窄长条（多行短文字栅格化出来的）窗口变成 136×724，
    /// `fit_aspect` 把图居中、两侧各 38px 黑边；放大到窗高越过 1100 后图片高度被钉死，
    /// 另一维还在长——用户报的"图片没放大、只是边框在放大"正是这个。
    ///
    /// 所以分工是：**本单测钉住算式**（谁改公式先红），**真机探针 `pin_zoom_e2e_probe`
    /// 钉住"算式真的落到窗口上"**（撤掉 `.popup(true)` 就红，已实测）。
    /// 期望值 64×724 / 82×940 就是那条探针量到的数。
    #[test]
    fn extreme_aspect_ratio_window_is_exactly_image_plus_frame() {
        // 窗口矩形 = frame_inflate(图片矩形)，图片矩形这里取原点 (0,0) 只看尺寸。
        let win = |w: i32, h: i32, pct: u32| {
            let (iw, ih) = zoomed_size(w as u32, h as u32, pct);
            let (_, _, ww, wh) = frame_inflate((0, 0, iw, ih));
            (ww, wh)
        };
        // 60×720（12:1，用户报的那一档）100%：图片 60×720 → 窗口 64×724。
        assert_eq!(win(60, 720, 100), (64, 724));
        // 130%（滚 3 格）：图片 78×936 → 窗口 82×940。
        assert_eq!(win(60, 720, 130), (82, 940));
        // 800% 上限：图片 480×5760 → 窗口 484×5764。**远超一屏高**也不许被谁钳掉——
        // 收口前这一格会被压到约 1100 高，图片高度当场冻结。
        assert_eq!(win(60, 720, 800), (484, 5764));
        // 反向：极宽极矮（3000×80）同样按算式给，不收口。
        assert_eq!(win(3000, 80, 100), (3004, 84));
    }

    /// ★ 缩放锚点：光标底下的那个图片像素在缩放前后保持不动（图像查看器标准手感）。
    #[test]
    fn zoom_anchor_keeps_the_pixel_under_the_cursor_fixed() {
        // 图片左缘在屏幕 0、长 100px、光标在 50（正中）。放大到 200% 后图片变成 [-50,150]，
        // 光标 50 仍落在原图片像素 50 处：(50-(-50))/200*100 = 50。
        assert_eq!(zoom_anchor(0, 100, 200, 50), -50);
        // 缩小到 50%：图片变成 [25,75]，光标 50 落 (50-25)/50*100 = 50。
        assert_eq!(zoom_anchor(0, 100, 50, 50), 25);
        // 光标就在图片左缘（0）→ 锚点不动，保持 0。
        assert_eq!(zoom_anchor(0, 100, 200, 0), 0);
    }

    /// ★ 缩放百分比角标（非 100% 时）确实画在图片**左上角、贴着描边线**：深色半透明直角底
    /// 压在灰图上读得出。
    ///
    /// 这是用户口径"在图片左上角以百分比的样式显示缩放因子"的像素证据。取角标**底色区**
    /// （不含文字笔画）的像素，应当明显比图片灰（128）暗——证明角标画出来了；同时角标
    /// 不覆盖图片主体（远右下仍是纯灰）。注意角标只在非 100% 出现，这里渲染 150%。
    #[test]
    fn zoom_label_draws_a_dark_badge_in_the_top_left_corner() {
        let buf = render_pin(false, 150);
        // (3,3) 落在角标盒子内、远离文字笔画（左下 padding），应是 LABEL_BG 半透明叠在灰图上的暗色。
        let (r, g, b) = px(&buf, 3, 3);
        let sum = r as u32 + g as u32 + b as u32;
        assert!(
            sum < 130,
            "角标底色区必须明显比图片灰(384)暗，实际 ({r},{g},{b}) sum={sum}"
        );
        // 角标应**贴左上角描边线**——图片左上角 (2,2) 就应是暗色（徽标左上角 = 图片左上角）。
        let (r0, g0, b0) = px(&buf, 2, 2);
        assert!(
            (r0 as u32 + g0 as u32 + b0 as u32) < 130,
            "角标必须贴合左上角描边线，图片左上角也应是暗色，实测 ({r0},{g0},{b0})"
        );
        // 远右下 (36,27) 不在角标覆盖范围（角标仅在左上角一小块），应仍是纯灰。
        let (r2, g2, b2) = px(&buf, 36, 27);
        assert_eq!((r2, g2, b2), IMG_GRAY, "角标不覆盖图片主体");
    }

    /// ★ 100% 是出厂默认，**不显示**角标：左上角应是纯图片灰，而非深色徽标。
    ///
    /// 用户口径："默认大小是 100%，但是不需要显示。只有我们手动进行缩放时才需要显示。"
    #[test]
    fn zoom_badge_is_hidden_at_the_default_100_percent() {
        let buf = render_pin(false, 100);
        let (r, g, b) = px(&buf, 6, 6);
        let sum = r as u32 + g as u32 + b as u32;
        assert!(
            sum > 200,
            "100% 时左上角不应有深色角标（应是图片灰 384），实测 ({r},{g},{b}) sum={sum}"
        );
    }

    /// ★ `fit_aspect` 必须在可用区内保持原图宽高比——这是"缩小不变形"的几何保证。
    #[test]
    fn fit_aspect_preserves_the_source_aspect_ratio() {
        // 宽图 2:1 在 50×50 可用区：宽约束 → 50×25。
        assert_eq!(fit_aspect(200, 100, 50, 50), (50, 25));
        // 高图 1:2 在 50×50 可用区：高约束 → 25×50。
        assert_eq!(fit_aspect(100, 200, 50, 50), (25, 50));
        // 正方形在 30×40 可用区：取满短边 30×30，绝不超出可用区。
        let (w, h) = fit_aspect(100, 100, 30, 40);
        assert_eq!((w, h), (30, 30));
        assert!(w <= 30 && h <= 40, "结果绝不超出可用区");
        // 结果宽高比 ≈ 原图（1:1）。
        assert!((w as f32 / h as f32 - 1.0).abs() < 0.05, "比例保持");
        // 退化输入：负/零可用区被钳到 1×1，不 panic、不返回 0。
        assert!(fit_aspect(100, 100, -5, -5) == (1, 1));
    }

    /// ★★ 回归（用户报的缩放黑边）：窗口已被外部缩放到 84×64，但 windui 传入 `paint` 的布局
    /// `bounds` 仍停在 100% 的 44×34（缩放后 windui 不重排）。修复前 `paint` 用旧 `bounds`
    /// 作画 → 图片按 44×34 画在 84×64 窗口左上角、四周填黑。修复后 `paint` 用真实（命令）
    /// 窗口尺寸作画 → 图片放大填满整个 84×64，描边贴真实窗口边缘。这条把"图片必须随窗口
    /// 缩放、不露黑边"钉死。
    #[test]
    fn pin_image_fills_the_commanded_window_size() {
        // 真实窗口 84×64；命令尺寸 = (84,64)；传入的 stale `bounds` = 44×34（100% 旧尺寸）。
        let buf = render_pin_window(84, 64, 44, 34, Some((84, 64)), false, 100);
        // 中心 (42,32) 应在放大后的图片内 → 灰（修复前这里是黑/边框）。
        assert_eq!(
            pxw(&buf, 84, 42, 32),
            IMG_GRAY,
            "图片应放大填满窗口，中心不能是黑或边框"
        );
        // 越过旧 bounds(44×34) 但仍在新窗口内的点 (60,30) 也必须是图片灰（证明填充到了新尺寸）。
        assert_eq!(
            pxw(&buf, 84, 60, 30),
            IMG_GRAY,
            "旧 bounds 之外、新窗口之内必须被图片填充（无黑边）"
        );
        // 外环仍在**真实**窗口边缘 (0,0)，不是旧 bounds 边缘。
        assert_eq!(
            pxw(&buf, 84, 0, 0),
            RING_OUTER,
            "描边必须贴真实窗口边缘而非旧 bounds 边缘"
        );
    }

    /// ★ 2026-09-26 晚用户口径：贴图右键「脚本」子菜单**恒挂载**，没有脚本就"留空"。
    ///
    /// 这里是钉过一次的形状：此前菜单按脚本开关过滤（开关出厂全关 → 磁盘上明明有
    /// `图片体检 / 平均颜色`，菜单里却一项没有），用户报"有脚本但没显示"。
    #[test]
    fn lua_submenu_rows_keep_the_entry_even_when_empty() {
        // 空表：不塌成 0 行（调用方恒挂载子菜单），而是留一条禁用说明项。
        let rows = lua_submenu_rows(&[]);
        assert_eq!(
            rows.len(),
            1,
            "没有脚本时子菜单应留一条占位项而不是彻底消失"
        );
        let (cmd, text, disabled) = &rows[0];
        assert!(
            *disabled,
            "占位项必须置灰（点了没反应，用户会以为是功能坏了）"
        );
        assert_eq!(*cmd, 0, "占位项命令号必须为 0：不可点，且不在脚本区间里");
        assert_eq!(text, LUA_SUB_EMPTY_HINT);

        // 有脚本：命令号必须与 `lua_ext_menu_items` 的下标严格对齐，否则点 A 跑 B。
        let items = vec![
            (
                String::from("scripts/1.image_report"),
                String::from("图片体检"),
            ),
            (
                String::from("scripts/2.avg_color"),
                String::from("平均颜色"),
            ),
        ];
        let rows = lua_submenu_rows(&items);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], (CMD_LUA_MENU_BASE, "图片体检".into(), false));
        assert_eq!(rows[1], (CMD_LUA_MENU_BASE + 1, "平均颜色".into(), false));
    }
    /// ★ 回写贴图（m15）的两档失败必须在**动任何画布之前**给人话原因。
    ///
    /// `hwnd=0` 的成因是"这脚本不是从贴图上触发的"（脚本页里点运行、或热键触发时没选中窗）——
    /// 那种情况不许猜"那就换最近那一张吧"，猜错就是改了用户没打算改的贴图。
    /// 陌生句柄的成因是"那张窗已经关了，登记表里的 `Weak` 升级失败"；这条同时证明
    /// **登记表只握弱引用**：窗没了不许留下还能写的壳。
    #[test]
    fn writeback_refuses_unknown_targets_before_touching_a_canvas() {
        let bmp = RgbaBitmap::new(2, 1, vec![7u8; 8]).expect("构造测试位图");
        let e0 = apply_script_image(0, bmp.clone()).expect_err("hwnd=0 必须被拒");
        assert!(e0.contains("右键"), "要说清该从哪儿触发：{e0}");
        let e1 = apply_script_image(-0x777, bmp).expect_err("没登记过的句柄必须被拒");
        assert!(e1.contains("找不到"), "要说清是那张贴图不在了：{e1}");
        // 登记动作本身也不许写脏表：0 句柄直接跳过。
        let n = PINS.with(|m| m.borrow().len());
        assert_eq!(
            n, 0,
            "不许有幽灵条目（0 句柄也登记，回写就会互相串到别的贴图）"
        );
    }
    /// ★ 换图后的窗口算式（m15）：**新图的宽高必须真的带动新尺寸**。
    ///
    /// 旋转与脚本回写共用这一条（`set_canvas_bitmap` 里只留这一个算式）。
    /// 三格分开量：100% 时精确"图 + 描边"、放大时描边**不跟着长**（它是窗体装饰不是图），
    /// 宽高互换时两维一起换。任何一处写死旧尺寸都会在这里红——而那正是
    /// "长图换过去只长了边框"的形状。
    #[test]
    fn a_new_bitmap_drives_the_new_window_size() {
        let f = 2 * FRAME;
        assert_eq!(
            window_size_for_bitmap(300, 40, 100),
            (300 + f, 40 + f),
            "100% 时窗口 = 图片 + 描边，一条像素都不许多"
        );
        assert_eq!(
            window_size_for_bitmap(300, 40, 250),
            (750 + f, 100 + f),
            "描边不随缩放长：只有图片部分 ×2.5"
        );
        // 宽高互换（= 右键旋转 90°，也= 脚本换一张横竖相反的图）。
        assert_eq!(
            window_size_for_bitmap(60, 720, 100),
            (60 + f, 720 + f),
            "竖长图：高该远大于宽"
        );
        assert_eq!(
            window_size_for_bitmap(720, 60, 100),
            (720 + f, 60 + f),
            "同一张图换个宽高，窗口两维必须一起换"
        );
        // 极小图不许算出 0（`SetWindowPos` 拿 0 尺寸的行为不做保证）。
        assert!(window_size_for_bitmap(1, 1, 1).0 > f);
        assert!(window_size_for_bitmap(1, 1, 1).1 > f);
    }
    /// ★ 菜单命令号必须互不相同（m16 加了「恢复原图」= 回收 0x2006 那个洞）。
    ///
    /// 两个菜单项同号是最难查的一类串味：点「恢复原图」去做「另存为」，功能全都还在，
    /// 只是串了——用户只会说"菜单乱了"。`0` 要留给 `TrackPopupMenu` 的"没选中"哨兵。
    /// 这条门禁读的是**常量本身**，加新命令却不登记就会被这里拦住。
    #[test]
    fn menu_command_ids_are_unique_and_reserve_zero() {
        let ids = [
            ("另存为", CMD_SAVE),
            ("关闭", CMD_CLOSE),
            ("顺时针 90°", CMD_ROT_CW),
            ("水平镜像", CMD_MIRROR_H),
            ("垂直镜像", CMD_MIRROR_V),
            ("恢复原图", CMD_RESTORE),
            ("取色", CMD_PICK),
            ("锁定图片", CMD_LOCK),
        ];
        for (name, id) in ids {
            assert!(id > 0, "{name} 的命令号不能是 0 哨兵");
            assert!(id < CMD_LUA_MENU_BASE, "{name} 越进了「脚本」子菜单的号段");
            for (other, oid) in ids {
                assert!(name == other || oid != id, "{name} 与 {other} 同号 {id:#x}");
            }
        }
        // 号段边界由**局部变量**读出（直接 `assert!(常量比较)` 会被 clippy 的
        // `assertions_on_constants` 判成"编译期就能算完的断言"——门禁要 0 警告）。
        let (alpha_end, lua_base, restore, lock, pick) = (
            CMD_ALPHA_BASE + 10,
            CMD_LUA_MENU_BASE,
            CMD_RESTORE,
            CMD_LOCK,
            CMD_PICK,
        );
        // 透明度子菜单占 [CMD_ALPHA_BASE, +10]，脚本子菜单从 +100 起：留出可见的空隙。
        assert!(alpha_end < lua_base, "透明度号段与脚本号段重叠");
        // 「恢复原图」占的是回收掉的那个空号（`取色`=+5 与 `锁定`=+7 之间的洞）。
        assert_eq!(CMD_BASE + 6, restore, "「恢复原图」用的是回收的那个空号");
        assert!(restore != lock && restore != pick, "恢复原图与邻号撞了");
    }

    /// ★★ 「脚本按**当前图片**处理」的地基（2026-10-28 用户口径：镜像了，就按镜像后的图处理）。
    ///
    /// 旋转 / 镜像 / 脚本回写全都走 `set_canvas_bitmap` 改**同一个** `canvas.bitmap`，
    /// 而 `run_lua_ext`（贴图右键）与 `Runtime::run_script_hotkey`（热键）读的都是
    /// `current_bitmap(seq)` ⇒ 这两条入口看到的必然是**变换之后**的像素。
    /// 这一格把其中一半钉死：换过像素之后，`current_bitmap` 交出来的必须就是新的那张，
    /// 而不是"钉进来那一刻"的出厂图（出厂图另有归属：「恢复原图」与贴图环）。
    #[test]
    fn current_bitmap_tracks_canvas_transforms() {
        let w = canvas();
        let rc = w.0.clone();
        rc.borrow_mut().seq = 93;
        register_pin(93, &rc);
        let before = current_bitmap(93).expect("登记后应有像素");
        assert!(before.rgba.iter().all(|v| *v == 255), "夹具出厂是 2×2 纯白");
        // 换成一张 4×2 全黑：既是"像素变了"，也是"尺寸变了"（与旋转同形）
        let black = RgbaBitmap::new(4, 2, [0u8, 0, 0, 255].repeat(8)).expect("位图");
        assert!(set_canvas_bitmap(&rc, black.clone()), "换像素必须成功");
        let after = current_bitmap(93).expect("换过之后仍取得到");
        assert_eq!(
            (after.w, after.h, after.rgba),
            (4, 2, black.rgba),
            "脚本拿到的必须是**换过之后**的像素"
        );
        // 出厂那份仍在（回写/旋转不动它）：这是「恢复原图」能工作的前提。
        assert!(
            rc.borrow().rt.borrow().pin_content(93).is_none(),
            "夹具没登记出厂内容，这条只是确认取的是画布而不是出厂记账"
        );
    }

    /// ★ 「恢复原图」的判据：**同尺寸但像素不同**也算变过。
    ///
    /// 只比宽高是最自然的写法，也正好是错的——脚本把贴图换成一张同样大小的灰度图，
    /// 宽高一模一样，菜单里就没有「恢复原图」了，用户等于丢了原图。
    #[test]
    fn the_restore_criterion_watches_pixels_not_just_size() {
        let a = RgbaBitmap::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        let same = RgbaBitmap::new(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        let gray = RgbaBitmap::new(2, 1, vec![9, 9, 9, 255, 9, 9, 9, 255]).unwrap();
        let other_size = RgbaBitmap::new(1, 2, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        assert!(!bitmap_differs(&a, &same), "一字不差就不该出现「恢复原图」");
        assert!(
            bitmap_differs(&a, &gray),
            "同尺寸换了像素也算变过（这条就是灰度图那一格）"
        );
        assert!(bitmap_differs(&a, &other_size), "尺寸不同当然更算");
    }

    /// ★ 登记表是三条入口共同的地基：脚本回写按 `hwnd`、Ctrl+S 按 `seq`、「恢复原图」按 `seq`。
    ///
    /// m15 把登记从"跑过脚本那一刻"挪到**建窗那一刻**、键从 `hwnd` 换成 `seq`，当时一格测试都没有
    /// ——写坏了不会有人红，症状只是"某张贴图的 Ctrl+S 存了出厂那一张"，而那要真窗才看得见。
    /// 这条把三件事钉住：按两种身份都找得到同一张、`0` 句柄一律不认、窗口关掉就查不到（而不是
    /// 被 `Weak` 的另一面——强引用——吊住不销）。
    #[test]
    fn the_canvas_registry_answers_seq_and_hwnd_and_dies_with_the_window() {
        let w = canvas();
        let rc = w.0.clone();
        rc.borrow_mut().seq = 91; // 换一个不会与别的测试撞的号
        rc.borrow_mut().hwnd.set(0x005A_0001); // 夹具没有真窗，句柄自己填
        register_pin(91, &rc);
        // 第二张：表里同时有两张活画布，"扫到哪个算哪个"这种写法必须在这条上红。
        let w2 = canvas();
        let rc2 = w2.0.clone();
        rc2.borrow_mut().seq = 92;
        rc2.borrow_mut().hwnd.set(0x005A_0002);
        register_pin(92, &rc2);

        let live = current_bitmap(91).expect("登记的画布应能按 seq 取到");
        assert_eq!(
            live.rgba,
            rc.borrow().bitmap.rgba,
            "按 seq 取到的就是画布此刻那一份"
        );
        assert!(
            Rc::ptr_eq(
                &pin_by_hwnd(0x005A_0001).expect("按 hwnd 也该找到同一张"),
                &rc
            ),
            "两种身份必须指向同一个画布，否则回写改的是另一扇窗"
        );
        assert!(
            Rc::ptr_eq(
                &pin_by_hwnd(0x005A_0002).expect("第二张也要按句柄认得出"),
                &rc2
            ),
            "两张都在表里时句柄要认得清是哪一张——认错了就是改了用户没点的那张贴图"
        );
        assert_eq!(
            pin_by_seq(92).expect("按 seq 也要认得第二张").borrow().seq,
            92,
            "两张都在表里时 seq 不许串"
        );
        assert!(
            pin_by_hwnd(0).is_none(),
            "0 句柄 = 没有贴图上下文，不许猜\"最近那一张\""
        );
        assert!(
            pin_by_seq(999_999).is_none(),
            "没登记过的号不能凭空造出画布"
        );
        // 「恢复原图」的兜底分支：记账里已经没有出厂内容 ⇒ 宁可让这一项出现。
        assert!(
            pin_was_transformed(&rc, 91),
            "查不到出厂内容时要让入口出现，不能悄悄不可用"
        );

        drop(rc);
        drop(w);
        assert!(
            pin_by_seq(91).is_none(),
            "窗关掉后必须查不到（存的是 Weak，不是吊着画布）"
        );
        assert!(
            current_bitmap(91).is_none(),
            "回写/Ctrl+S 那条路要如实落到\"那张贴图不在了\""
        );
        assert!(
            pin_by_hwnd(0x005A_0001).is_none(),
            "关掉的那张不能再被句柄找回（第二张还活着，扫到活的就红）"
        );
    }
}
