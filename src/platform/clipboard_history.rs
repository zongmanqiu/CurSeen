//! 剪贴板历史管理器（独立原生 Win32 模块）。
//!
//! 承载「剪贴板」模块的 7 个基础功能：监控复制、分类过滤、搜索、收藏、悬停预览、
//! 快捷键调出面板、点击粘贴。单后台线程持有全部状态（内存历史 + 监听窗 + 面板窗 +
//! 预览窗），模块与 UI 只经 `PostMessageW` 派发命令，绝不在 windui 每帧画布里画。
//!
//! 分层：`modules::clipboard` 只依赖 `core`（配置/快捷键/事件）与本模块（平台层在 modules 之下，
//! 合法）。本模块只依赖 `windows`、既有 `screenshot`（PNG 编解码 / CF_DIB 写回）、既有
//! `clipboard`（读文本 / 读图像）、`paths`（便携落盘）——零新依赖。
//!
//! 设计铁律（剪贴板工具实战 + 便签踩坑）：
//! - 格式优先级 files(CF_HDROP) > image(CF_DIB) > text(CF_UNICODETEXT)；
//! - 自写屏蔽用命名格式 `CurSeen.ClipboardSelf`（避免粘贴回环，比时间窗稳）；
//! - 去重命中只更新时间戳并上浮，不新增；
//! - 粘贴时序：写剪贴板 → 隐藏面板 → 150ms → `SendInput` Ctrl+V（残留 Shift 先补 KEYUP）；
//! - 搜索 280ms 防抖、预览 400ms 触发（便签的 `TrackMouseEvent(TME_LEAVE)` 坑已规避）；
//! - 原生 EDIT 必须真 `SetFocus`（`WM_CTLCOLOREDIT` 的 DC 在 wParam）。

// 本模块大量调用 Win32「发了就走」的 API（SetWindowPos / SetFocus / InvalidateRect /
// PostMessageW 等），其返回的错误/BOOL 在单泵线程里无需逐处判定，统一在此抑制
// `unused_must_use`，避免噪音；关键路径（PostMessageW 派发命令）已显式 `let _ =`。
#![allow(unused_must_use, function_casts_as_integer)]

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::*;
// WIC：单文件图片的列表缩略图解码（[`decode_image_file`]）。系统自带解码器，
// 覆盖 jpg/png/gif/bmp —— 比 `png`+`gif` 两个 crate 更全，也不多一个第三方依赖。
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapScaler, IWICBitmapSource,
    IWICFormatConverter, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Controls::{EM_SETRECT, WM_MOUSELEAVE};
// ★ SYSTEMTIME 本体在 Foundation（已 glob 导入）；两个转换函数分属不同模块。
use std::path::PathBuf;
use windows::Win32::Storage::FileSystem::FileTimeToLocalFileTime;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::System::Time::FileTimeToSystemTime;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, ReleaseCapture, SendInput, SetCapture, SetFocus, TrackMouseEvent, INPUT,
    INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEINPUT, TME_LEAVE, TRACKMOUSEEVENT, VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::core::paths::{from_app_relative, module_temp_dir, to_app_relative};
use crate::platform::clipboard::{get_image, get_text};
use crate::platform::screenshot::{save_png, to_clipboard, Shot};

// ── 常量 ───────────────────────────────────────────────────────────────
const CF_HDROP_U: u32 = 15;
const CF_TEXT_U: u32 = 13;
const WM_APP_CLIP_CMD: u32 = WM_APP + 1;

const CMD_TOGGLE: usize = 1;
const CMD_SETCFG: usize = 2;
const CMD_CLEAR: usize = 3;
// ★ 打开面板时"补采当前剪贴板"改用异步投递：show_panel 先让面板即时显隐，
//   再把 capture() 投回消息循环跑，避免同步解码当前图片把面板线程卡住、打开即转圈
//   （2026-09-29「打开依然转圈」根因之一：show_panel 直接 self.capture()）。
const CMD_CAPTURE: usize = 4;
/// 收起面板（无条件）。与 [`CMD_TOGGLE`] 的区别是它**不看当前可见性** —— 面板已经收起时
/// 是空操作。功能开关被关掉时用这个：那时没人知道面板是开着还是收着，而"toggle"会把
/// 收着的面板重新打开（用户报"关掉剪贴板开关，面板自己冒出来了"）。
const CMD_HIDE: usize = 5;

const SEARCH_TIMER: usize = 1;
const HOVER_TIMER: usize = 2;
const PASTE_TIMER: usize = 3;
/// 存储告知条的自毁计时：显示 [`WARN_SHOW_MS`] 毫秒后自动收回，让位给条目计数。
const WARN_TIMER: usize = 4;
/// ★ 告知条**出世即开始倒计时**（2026-10-02 用户口径："达到 5G 的时候给出提示就行了"）。
/// 它是"告知"不是"告警"—不设上限、不自动清理是用户的选择，所以不该常驻占着计数那条
/// 位置；8 秒足够读完一行字，之后计数照常回来。
const WARN_SHOW_MS: u32 = 8000;
/// ★ 捕获读失败重试：剪贴板是全系统互斥资源，别的监听
/// 程序抢先打开的那几十毫秒里 `OpenClipboard` 必失败；而 `WM_CLIPBOARDUPDATE` 已消费
/// 掉不会再来 ⇒ 格式明明在、这条复制却永久缺失。重试走 `SetTimer`（**绝不**在面板
/// 消息泵线程上 sleep），共 [`CAP_RETRY_MAX`] 次、每次隔 [`CAP_RETRY_MS`]。
const RETRY_TIMER: usize = 5;
const CAP_RETRY_MAX: u32 = 2;
const CAP_RETRY_MS: u32 = 60;
/// ★ 粘贴目标轮询（textus `remember_foreground_target` 同款）：面板可见期间每 500ms
/// 记一次「当前外部前台窗」到 `last_foreground`/`saved_focus`。WM_ACTIVATE(WA_INACTIVE)
/// 的失焦刷新只覆盖"面板被抢焦点"这一条路径；轮询兜底其余一切（Alt+Tab、点别的窗、
/// 目标窗自己切换焦点控件……），保证 do_paste 拿到的目标永远是"用户最近在用的窗"。
const TARGET_POLL_TIMER: usize = 6;
const TARGET_POLL_MS: u32 = 500;
/// ★ 粘贴发键二段：`do_paste` 切完前台后**不立即发键**，等目标应用消化完 WM_ACTIVATE、
/// 把内部焦点挂回输入框再发（QQ NT 等 Electron/Qt 系应用在切前台后 1ms 内收到注入键
/// 会落空 + 报错误音，2026-10-07 日志实证；textus 同场景 sleep 50ms 后发 ⇒ 通吃）。
/// 不在消息泵上 sleep，走计时器。
const PASTE_SEND_TIMER: usize = 7;
const PASTE_SEND_DELAY_MS: u32 = 60;
/// ★ 发键前「前台稳定」判据（2026-10-07 驱动实验定案）：写回剪贴板会触发目标应用
/// （QQ/TIM）自己的剪贴板助手气泡弹出抢前台/焦点，固定 60ms 后直接发键 = 赌运气
/// （同一批条目这一轮全到货、下一轮半数落空，实测截图显示落空时输入框 caret 消失）。
/// 发键线程轮询：前台==目标**连续稳定** [`PASTE_STABLE_HOLD_MS`] 且总等待≥
/// [`PASTE_STABLE_MIN_MS`] 才发；期间被抢就切回来（textus `ensure_external_focus`
/// 的循环化，80ms 节流防 alt-tap 刷屏）；[`PASTE_SEND_DEADLINE_MS`] 兜底，超时按
/// 现状直接发、不无限等。
const PASTE_STABLE_MIN_MS: u32 = 350;
const PASTE_STABLE_HOLD_MS: u32 = 250;
const PASTE_SEND_DEADLINE_MS: u32 = 1500;
/// 缩略图解码完一张 → 经此消息唤醒命令窗消息泵，去抽回传通道并刷新面板。
/// ★ 解码本身在**独立 worker 线程**做（见 `run`），UI 线程不参与解码——这是
/// 「首次打开剪贴板不再转圈」的根因修复（之前在 UI 线程 `WM_TIMER` 里同步解，
/// 解码期间消息泵被占、光标悬在面板上就转圈，且只首次缓存空时触发）。
const WM_APP_THUMB_WAKE: u32 = WM_APP + 2;

/// ★ 用户第 4 点（2026-09-24 定案）："再少一点点，300 差不多" → 430 → **300**。
const PANEL_W: i32 = 300;
/// 高度按"顶栏 + 页签 + 10 行 + 四边 GAP"倒推（542 = 60 + 2 + 10×48），
/// 保证最后一行正好落在边距上、底部不留半行的空档。
const PANEL_H: i32 = LIST_TOP + GAP + 10 * ITEM_H;
/// ★ 统一间隔（2026-09-24 用户定案）：外边距、顶栏→页签、页签→列表、行与行**全用这一个**。
/// 旧版外边距 `PAD=10`、页签上下各 6px、行间 4px 混着来，四边宽窄不一眼就能看出来。
const GAP: i32 = 2;
/// 顶栏（放大镜拖动区 + 搜索框 + 置顶/收起/关闭）：自 `GAP` 起，高 `TOP_H`。
///
/// ★★ 顶栏**内部不再留上下内边距**：按钮与搜索框都做成 `TOP_H` 整高（便签同款），
/// 于是"顶栏上下那一圈"就是外面那个 `GAP`，跟条目之间的间隔一模一样（用户第三次
/// 指出间隔不统一后才这么定）。
const TOP_Y: i32 = GAP;
const TOP_H: i32 = 28;
const TOP_B: i32 = TOP_Y + TOP_H;
const BTN_W: i32 = 28;
const DRAG_W: i32 = 28;
/// 顶栏内搜索框高度（与 `TOP_H` 同高，上下零内边距）。
const SEARCH_BOX_H: i32 = 28;
/// ★★ 搜索框**固定宽度**（用户第 2 点定案的排版）。
///
/// 旧版是"搜索框吃满剩下的空间、计数只留固定 54px 贴在最右"——于是计数被顶到贴着
/// 置顶按钮的边框（用户第 2 点报的"'条'字快碰到按钮边框"），而且数字一大就挤出边界。
/// 反过来做：搜索框**先**拿一个固定宽度（顶栏里输入的是关键词，88px ≈ 11 个英文字符
/// / 5 个汉字，够用），**剩下的整条全归计数**。
const EDIT_W: i32 = 88;
/// 搜索框（原生 EDIT）文本左右内边距。
const EDIT_PAD_X: i32 = 6;
/// ★ 搜索框文本垂直居中的**视觉补偿**（用户第 3 点）。
///
/// 见 [`edit_format_rect`]：字体 `ascent > descent`，按"ascent+descent 框"算出来的
/// 居中会让字形重心偏上，所以整行再往下摁 1px（textus 那边是同一条经验，只不过它
/// 是 windui 自绘、用 padding 补偿；这里是原生 EDIT，走 `EM_SETRECT`）。
const EDIT_VCENTER_NUDGE: i32 = 1;
/// 收起态 = 只剩顶栏。
const COLLAPSED_H: i32 = TOP_B + GAP;

/// 面板当前高度（收起只剩顶栏）。双缓冲的位图尺寸、背景矩形、边框**三处共用**——
/// 收起态按 `PANEL_H` 去贴会多画一截（无害但白付），按这个来才对得上。
const fn panel_h(collapsed: bool) -> i32 {
    if collapsed {
        COLLAPSED_H
    } else {
        PANEL_H
    }
}
/// 分类页签条：紧贴顶栏下方，间隔同样是 `GAP`。
const TAB_Y: i32 = TOP_B + GAP;
const TAB_H: i32 = 26;
const LIST_TOP: i32 = TAB_Y + TAB_H + GAP;
/// 自绘垂直滚动条宽度（见 [`scroll_thumb`]）。
const SCROLL_W: i32 = 6;
/// 行距 = 可见卡片 `ROW_H` + 行间 `GAP`。
const ITEM_H: i32 = 48;
const ROW_H: i32 = ITEM_H - GAP;
/// 行内类型图标边长（矢量手绘，见 [`draw_kind_icon`]）。
const ICON_W: i32 = 24;
/// 行右侧三个动作按钮（复制置顶 / 删除 / 收藏）的边长。
const ACT_W: i32 = 22;

/// 列表可视行数（纯函数：几何只由上面这组常量决定，绘制/命中/滚动三处共用）。
const fn visible_rows() -> i32 {
    (PANEL_H - LIST_TOP - GAP) / ITEM_H
}

/// 列表右边界（让出滚动条那一条）。
const fn list_right() -> i32 {
    PANEL_W - GAP - SCROLL_W - GAP
}

/// 搜索框左边界（顶栏里放大镜拖动区之后）。
const fn edit_left() -> i32 {
    GAP + DRAG_W + GAP
}

/// 搜索框右边界（固定宽度，不再随面板宽度漂）。
const fn edit_right() -> i32 {
    edit_left() + EDIT_W
}

/// 顶栏右侧那条"计数区"的矩形（纯函数，可单测）。
///
/// ★★ 用户第 2 点定案的排版：搜索框固定 `EDIT_W`，**剩下的整条**（到三颗按钮为止）
/// 全归计数，文本在这条里**水平居中**——于是计数不再贴着按钮边框，数字变长也只是
/// 从中心往两头长，永远不会去挤按钮。
const fn count_rect() -> RECT {
    RECT {
        left: edit_right(),
        top: TOP_Y,
        right: PANEL_W - GAP - BTN_W * 3,
        bottom: TOP_B,
    }
}

/// 计数能保证**不截断**地显示的量级上限（百万级）。
///
/// 「不限」档理论上是 `usize::MAX`（20 位数字），不可能也没必要显示；百万级往上统一
/// 收成 `"99万+"`，计数区的宽度就有了一个硬上界（见单测 [`count_fits_its_slot`]）。
const COUNT_SAFE_MAX: usize = 999_999;

/// 计数用的数字压缩（纯函数）。
///
/// ★ 用户担心"以后有 123454 条，装得下吗"。计数区宽度是固定的（搜索框固定后剩下的
/// 那一条），所以数字本身也必须有长度上界：
///   * `< 1 万` 原样显示（日常就是这个量级，精确值更有用）；
///   * `<= 99.9 万` 用「万」取整（123454 → `"12万"`）；
///   * 再大一律收成 `"99万+"`（`usize::MAX` 那 20 位数字不可能显示，也不能让它撑破
///     顶栏）。
fn compact_num(n: usize) -> String {
    if n < 10_000 {
        format!("{n}")
    } else if n <= COUNT_SAFE_MAX {
        format!("{}万", n / 10_000)
    } else {
        "99万+".to_string()
    }
}

/// 顶栏那条计数（纯函数，可单测）。
///
/// ★ 用户第 7 点的答案：不新增底部状态栏，把计数塞进顶栏右侧那条本来就空着的缝里。
/// 于是——**没筛选**时是「128 条」；**正在筛选**时变成「3 / 128」（命中 / 该页签总数），
/// 筛选中也一样看得到总数，UI 一行都没多。
fn count_text(total: usize, shown: usize) -> String {
    if shown == total {
        format!("{} 条", compact_num(total))
    } else {
        format!("{} / {}", compact_num(shown), compact_num(total))
    }
}

/// 存储告知条文案（临时占用顶栏那条计数空位，见 [`WARN_SHOW_MS`]）。
///
/// 只报量级，单位直接写作 GB——用户自己说的是"到 5G 的时候就提示"，那就按这句口头习惯
/// 走，不去争 1024 / 1000 那笔换算账。
///
/// ★ 字数是有硬约束的：这条 borrow 的是顶栏那条**计数**空位（宽 94px，见
///   [`count_fits_its_slot_even_when_the_number_gets_long`]），汉字比数字贵得多，所以
///   只写"已占 N GB"而不是"图片原件已占用 N GB"——后者实测 102px，会直接画到三颗按钮上。
fn warn_bytes_text(b: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    if b >= GB {
        format!("已占 {} GB", b / GB)
    } else {
        format!("已占 {} MB", b / (1024 * 1024))
    }
}

/// 顶栏告知条的两类内容（临时借用计数那条空位，见 [`WARN_SHOW_MS`]）。
enum TopNotice {
    /// 图片原件目录越过告知线，报量级。
    Storage(u64),
    /// 粘贴的文件条目其外部原文件已全部不在原处——这次粘贴**没有**执行。
    SrcMissing,
}

/// 源文件失效的告知文案。★ 硬约束同 [`warn_bytes_text`]：借用的计数槽位宽度有限，
/// 字数必须压在能实量放下的范围内（见单测 `notice_text_fits_the_count_slot`）。
const SRC_MISSING_NOTICE: &str = "源文件已不在";

/// 告知条文案分派。
fn notice_text(n: &TopNotice) -> String {
    match n {
        TopNotice::Storage(b) => warn_bytes_text(*b),
        TopNotice::SrcMissing => SRC_MISSING_NOTICE.to_string(),
    }
}

/// 搜索框（原生 EDIT）的格式化矩形：把文本行摁到**垂直居中**的那一条（纯函数）。
///
/// ★★ 用户第 3 点："搜索输入框文本不是垂直居中（Windows 固有缺陷）"。
/// 根因：单行 EDIT 按"字体 ascent+descent 框"居中，而 Segoe UI 的 ascent 明显大于
/// descent，字形的**视觉重心**因此偏上。textus 给的是同一条经验（顶部微量 padding
/// 补偿 ascent>descent），只是它走 windui 自绘、能直接加 padding；这里是**原生 EDIT**，
/// 唯一认的通道是 `EM_SETRECT`——而 `EM_SETRECT` **只对多行 EDIT 生效**（单行控件直接
/// 忽略它）。所以搜索框改成多行 EDIT（不带 `ES_WANTRETURN`，回车照旧被子类吃掉），
/// 再用这条矩形把文本行摁到 `(SEARCH_BOX_H - line_h) / 2 + 1` 的位置。
const fn edit_format_rect(w: i32, line_h: i32) -> RECT {
    let top = (SEARCH_BOX_H - line_h) / 2 + EDIT_VCENTER_NUDGE;
    RECT {
        left: EDIT_PAD_X,
        top,
        right: w - EDIT_PAD_X,
        bottom: top + line_h,
    }
}

/// 顶栏的**可拖动矩形**（`WM_NCHITTEST` 的单一来源）：顶栏除交互控件外**整条都能
/// 按住拖窗**——含放大镜、左侧空白、右侧那条计数、以及顶栏上下那圈 `GAP`
/// （用户口径 2026-09-27：只有搜索框与三颗按钮属于交互控件，其余都该能拖）。
///
/// 右边界取 [`count_rect`] 的 right（三颗按钮左沿），上边界取 0（顶栏上方那条 `GAP`）
/// ——"整条顶栏 + 外圈"都属于标题栏，比旧版（只有放大镜那 28px）才好拖得多。
const fn title_drag_rect() -> RECT {
    RECT {
        left: 0,
        top: 0,
        right: count_rect().right,
        bottom: TOP_B,
    }
}

/// 屏幕（客户区）坐标是否落在"可按住拖窗"的顶栏区域（纯函数，可单测）。
///
/// ★ 唯一的例外是 **搜索框**：它是**原生 EDIT 子窗**，抢了 `HTCAPTION` 就等于把点击
///   从 EDIT 手里偷走——点进去不聚焦、也选不中文字（旧版只放大镜子那块区域，正因
///   为如此才没出这个 bug）。三颗按钮由 [`top_btn_at`] 在 `WM_LBUTTONDOWN` 里处理，
///   走默认的 `HTCLIENT` 即可。
const fn title_drag_at(x: i32, y: i32) -> bool {
    let r = title_drag_rect();
    if y < r.top || y >= r.bottom || x < r.left || x >= r.right {
        return false;
    }
    // 搜索框：只在它真正占的那条里让路（上下那条 GAP 仍可拖）。
    if y >= TOP_Y && y < TOP_Y + SEARCH_BOX_H && x >= edit_left() && x < edit_right() {
        return false;
    }
    true
}

/// 顶栏右侧三颗按钮里命中的是哪一颗（自右往左 `0`=关闭 `1`=收起 `2`=置顶）；
/// 不在按钮上返回 `None`。
///
/// 几何单一来源：绘制、悬停、点击三处都走它，免得"画在 A 处、点在 B 处"。
const fn top_btn_at(x: i32, y: i32) -> Option<i32> {
    if y < TOP_Y || y > TOP_B {
        return None;
    }
    let d = PANEL_W - GAP - x;
    if d < 0 || d > BTN_W * 3 {
        return None;
    }
    Some(d / BTN_W)
}

/// 滚动条轨道矩形（纯函数）。
const fn scroll_track() -> RECT {
    RECT {
        left: PANEL_W - GAP - SCROLL_W,
        top: LIST_TOP,
        right: PANEL_W - GAP,
        bottom: PANEL_H - GAP,
    }
}

// 虚拟键原始值（稳定，避免依赖枚举变体名称）。
const VK_SHIFT_R: u32 = 0x10;
const VK_CONTROL_R: u32 = 0x11;
const VK_MENU_R: u32 = 0x12;
const VK_V_R: u32 = 0x56;
const VK_UP_R: u32 = 0x26;
const VK_DOWN_R: u32 = 0x28;
const VK_RETURN_R: u32 = 0x0D;
const VK_ESCAPE_R: u32 = 0x1B;
const EN_CHANGE_R: u16 = 0x0300;

const MAX_TEXT_BYTES: usize = 100 * 1024;

/// 超限大文本的**预览**保留字节数（按字符边界安全截断）。
///
/// 用户口径（2026-09-29）：单条大文本不该整个塞进内存——全文落盘到
/// `texts/<内容哈希>.txt`，条目里只留这点预览供列表展示；粘贴/复制时从盘上读全文。
const TEXT_PREVIEW_BYTES: usize = 400;

/// 按**字符边界**安全截取前缀（`&s[..n]` 系 str 切片撞多字节字符会 panic，禁用）。
fn safe_prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    &s[..cut]
}
const THUMB_MAX_W: i32 = 720;

/// 给单个图片文件补列表缩略图时，**源文件超过这么大就不解了**。
///
/// 列表要的只是一张 720px 小图，为它去读一份几百 MB 的 PSD/TIFF 不值得（解码前的
/// IO 与解码器的准备工作都按原尺寸来）。这一刀只影响"有没有预览"，不影响粘贴——
/// 文件条目本来就是引用原路径，粘回去仍是原文件。
const FILE_THUMB_MAX_BYTES: u64 = 128 * 1024 * 1024;

/// 全尺寸原件目录（`images/`）的**告知线**：达到就对用户提示一次。
///
/// 为什么不设硬上限（2026-10-02 用户口径）：剪贴板历史是"我不想丢的东西"，为了省空间
/// 偷偷停止保存原件，会让同一张图第一次粘贴有分辨率、过些天就没有——不可预测比占空间更糟。
/// 所以这里只**告知**，不出手清理，删什么由用户自己点。
const IMG_DIR_WARN_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// 自上次数完目录以来，累计写够这么多字节才重新数一次。数目录是 O(文件数) 的系统调用，
/// 每次捕图都数没必要；而 32MB 对 5GB 的判定精度绰绰有余。
const IMG_DIR_RECOUNT_BYTES: u64 = 32 * 1024 * 1024;

/// `images/` 目录当前占用字节数（只合计顶层文件，不递归）。目录不存在记 0。
fn images_dir_bytes(dir: &std::path::Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|p| p.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// 原件目录占用账：为的是不必每次都把整个目录重数一遍。
///
/// `known` = 上一次实数出来的值；`pending` = 那之后新增的（每次写盘顺手加文件的字节数，
/// 一次 stat，几乎免费）。两者之和突破 [`IMG_DIR_RECOUNT_BYTES`] 才真的去重数一次。
struct ImgBudget {
    known: u64,
    pending: u64,
    warned: bool,
}

impl ImgBudget {
    fn new(dir: &std::path::Path) -> Self {
        let known = images_dir_bytes(dir);
        Self {
            warned: known >= IMG_DIR_WARN_BYTES,
            known,
            pending: 0,
        }
    }

    /// 计入本次写入的 `n` 字节；返回 `true` = 该提示了（且保证**每个进程只提示一次**）。
    fn add(&mut self, n: u64, dir: &std::path::Path) -> bool {
        self.pending += n;
        if self.pending.max(self.known) >= IMG_DIR_RECOUNT_BYTES || n == 0 {
            self.known = images_dir_bytes(dir);
            self.pending = 0;
        }
        if self.warned || self.known + self.pending < IMG_DIR_WARN_BYTES {
            return false;
        }
        self.warned = true;
        true
    }
}

/// windows 0.62 不导出 `RGB` 宏，这里按 Win32 `RGB(r,g,b)` 约定（BGR 布局）补一个。
#[inline]
#[allow(non_snake_case)]
fn RGB(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

// ── 跨线程命令派发 ─────────────────────────────────────────────────────
static CMD_HWND: OnceLock<AtomicUsize> = OnceLock::new();
static START_ONCE: Once = Once::new();
static ORIG_EDIT: AtomicUsize = AtomicUsize::new(0);

/// 模块配置镜像（命令载荷）。
#[derive(Clone, Copy)]
pub struct ClipCfg {
    pub enabled: bool,
    pub max_entries: usize,
    /// 粘贴方法：**true = 双击粘贴**（出厂默认，2026-09-30 起），false = 单击粘贴（旧行为）。
    /// 面板窗口记忆（config.json 的 `clipboard.panel`；`None` = 无记忆 → 首开居中）。
    pub panel: Option<crate::core::config::ClipPanelGeom>,
}

/// 分类页签。
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabKind {
    All,
    Text,
    File,
    Image,
    Link,
    Fav,
}

/// 页签顺序（2026-09-24 用户定案）：全部 → 文本 → 链接 → 图片 → 文件 → ★收藏。
/// 文本在前因为它就是"最常翻的东西"，链接紧跟（URL 本质也是文本），图片/文件这两个
/// 重体量的排后面，收藏永远在最右当"快捷视图"。
const TABS: [(&str, TabKind); 6] = [
    ("全部", TabKind::All),
    ("文本", TabKind::Text),
    ("链接", TabKind::Link),
    ("图片", TabKind::Image),
    ("文件", TabKind::File),
    ("★", TabKind::Fav),
];

/// 条目类型。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Text,
    File,
    Image,
    Link,
}

/// 一条历史条目（JSONL 持久化）。
#[derive(Serialize, Deserialize, Clone)]
struct ClipEntry {
    id: u64,
    kind: Kind,
    text: String,
    files: Vec<String>,
    /// 缩略图路径（app-relative）；仅图片有。
    thumb: Option<String>,
    /// 原图尺寸（图片显示用）。
    w: i32,
    h: i32,
    /// 超限大文本的全文落盘路径（app-relative）；只有超 `MAX_TEXT_BYTES` 的文本条目有。
    /// `text` 字段此时只存**预览**（前 [`TEXT_PREVIEW_BYTES`] 字节，按字符边界截断）。
    #[serde(default)]
    full: Option<String>,
    /// 富文本保真（CF_HTML）落盘路径（app-relative，`texts/<内容哈希>.html`）。
    /// 存的是 "HTML Format" 的**原样字节**（含头），粘贴时原样写回 —— 零解析零失真。
    /// 复制时剪贴板上没有 HTML（记事本等纯文本源）或超 [`super::clipboard_html::MAX_HTML_BYTES`] 时为 `None`。
    #[serde(default)]
    html: Option<String>,
    fav: bool,
    ts: u64,
    hash: u64,
    /// ★ 运行态：`files[]` 引用的外部文件是否**全部**不在原处（懒校验得出，
    ///   **不落盘**——U 盘插回来就翻假，持久化会把暂时状态钉死）。
    #[serde(skip)]
    missing: bool,
}

// ── 公共入口（供 modules/clipboard 与 UI 调用）───────────────────────────

/// 启动后台管理器线程（幂等）。线程就绪后 `CMD_HWND` 才可用。
pub fn ensure_started() {
    START_ONCE.call_once(|| {
        std::thread::spawn(run);
    });
    while CMD_HWND.get().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// 推送配置（容量 / 启用）。每帧配置变化时才应调用。
pub fn set_config(cfg: ClipCfg) {
    ensure_started();
    let p = Box::into_raw(Box::new(cfg)) as isize;
    if let Some(a) = CMD_HWND.get() {
        let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_CLIP_CMD, WPARAM(CMD_SETCFG), LPARAM(p));
        }
    } else {
        // ensure_started 后应已就绪；极端情况下丢弃 Box 避免泄漏。
        unsafe {
            drop(Box::from_raw(p as *mut ClipCfg));
        }
    }
}

/// 快捷键：切换面板显隐。
pub fn toggle_panel() {
    ensure_started();
    if let Some(a) = CMD_HWND.get() {
        let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
        unsafe {
            let ok = PostMessageW(Some(h), WM_APP_CLIP_CMD, WPARAM(CMD_TOGGLE), LPARAM(0));
            log::info!("剪贴板: toggle 已投递 ok={}", ok.is_ok());
        }
    } else {
        log::warn!("剪贴板: toggle 时 CMD_HWND 未就绪");
    }
}

/// 功能开关被关掉时收起面板（无条件，不看当前可见性——见 [`CMD_HIDE`]）。
pub fn hide_panel() {
    ensure_started();
    if let Some(a) = CMD_HWND.get() {
        let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_CLIP_CMD, WPARAM(CMD_HIDE), LPARAM(0));
        }
    }
}

/// 「清空历史」按钮（tool.rs 调用）。
pub fn request_clear_history() {
    ensure_started();
    if let Some(a) = CMD_HWND.get() {
        let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
        unsafe {
            let _ = PostMessageW(Some(h), WM_APP_CLIP_CMD, WPARAM(CMD_CLEAR), LPARAM(0));
        }
    }
}

// ── 工具 ───────────────────────────────────────────────────────────────
/// C 字符串（带结尾 NUL）：只给 `CreateWindowExW` / `RegisterClassExW` 这类要 `PCWSTR` 的。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `DrawTextW` 专用：UTF-16 **不带结尾 NUL**。
///
/// ★★★ windows-rs 的 `DrawTextW(hdc, lpchtext: &mut [u16], ..)` 拿 `lpchtext.len()` 当
/// `cchText`，**不会**在 NUL 处停。把 [`wide`]（自带结尾 NUL）直接喂进去，那个 U+0000
/// 就会被当成一个字符画出来——表现是"图标右侧紧贴一个长方形"（豆腐块）。顶栏四个按钮
/// 和垃圾桶图标全中招（2026-09-24 用户报）。所有 `DrawTextW` 一律走这里。
fn text16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// 界面字体（Microsoft YaHei）。`px` 传正值，内部按 GDI 习惯取负（负值 = 按磅高取字形）。
unsafe fn make_ui_font(px: i32) -> HFONT {
    CreateFontW(
        -px,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_DEFAULT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        CLEARTYPE_QUALITY,
        DEFAULT_PITCH.0 as u32,
        PCWSTR(wide("Microsoft YaHei").as_ptr()),
    )
}

/// 符号字体：**Segoe UI Symbol**（📌 ✕ 🔍 🗑 在里面都是单色轮廓字形，不会被回退成
/// 彩色 emoji，颜色完全跟 `SetTextColor` 走——与便签 `platform::sticky` 同口径）。
///
/// ★ 质量档用 **ANTIALIASED（灰阶抗锯齿）而不是 CLEARTYPE**（2026-09-28 用户二报
/// "图标全是锯齿"）：ClearType 是**亚像素**渲染，靠红蓝边配平白底——那是给正文的；
/// 套在 17px 的小图标上就是一圈彩色镶边 + 阶梯感。灰阶 AA 对图标才是正解。
/// 实测（`temp/probe_glyph.py`，同一 📌、同一 17px）：
/// CLEARTYPE 7 级灰阶 + 彩边；ANTIALIASED **13 级灰阶**、边缘干净。
unsafe fn make_sym_font(px: i32) -> HFONT {
    CreateFontW(
        -px,
        0,
        0,
        0,
        FW_NORMAL.0 as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET,
        OUT_TT_PRECIS,
        CLIP_DEFAULT_PRECIS,
        ANTIALIASED_QUALITY,
        (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
        PCWSTR(wide("Segoe UI Symbol").as_ptr()),
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_url(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() || t.contains('\n') || t.contains('\r') {
        return false;
    }
    t.starts_with("http://")
        || t.starts_with("https://")
        || t.starts_with("ftp://")
        || t.starts_with("www.")
}

fn thumbnail_of(shot: &Shot, max_w: i32) -> Shot {
    if shot.w <= max_w || shot.w <= 0 {
        return shot.clone();
    }
    let scale = max_w as f32 / shot.w as f32;
    let nw = max_w;
    let nh = (shot.h as f32 * scale).max(1.0) as i32;
    let stride = (shot.w as usize) * 4;
    let nstride = (nw as usize) * 4;
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    for y in 0..nh as usize {
        let sy = (y as f32 / scale) as usize;
        for x in 0..nw as usize {
            let sx = (x as f32 / scale) as usize;
            let s = sy * stride + sx * 4;
            let d = y * nstride + x * 4;
            out[d..d + 4].copy_from_slice(&shot.bgra[s..s + 4]);
        }
    }
    Shot {
        w: nw,
        h: nh,
        bgra: out,
    }
}

fn shot_from_png(path: &std::path::Path) -> Option<Shot> {
    use std::io::BufReader;
    let f = std::fs::File::open(path).ok()?;
    let dec = png::Decoder::new(BufReader::new(f));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0u8; r.output_buffer_size().unwrap_or(0)];
    let info = r.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let (w, h) = (info.width as i32, info.height as i32);
    let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
    match info.color_type {
        png::ColorType::Rgba => {
            for (px, b) in bgra.chunks_exact_mut(4).zip(buf.chunks_exact(4)) {
                px[0] = b[2];
                px[1] = b[1];
                px[2] = b[0];
                px[3] = b[3];
            }
        }
        png::ColorType::Rgb => {
            for (px, b) in bgra.chunks_exact_mut(4).zip(buf.chunks_exact(3)) {
                px[0] = b[2];
                px[1] = b[1];
                px[2] = b[0];
                px[3] = 255;
            }
        }
        _ => return None,
    }
    Some(Shot { w, h, bgra })
}

/// 看着像图片扩展名？只用来挡住明显不是图片的文件（exe / zip / 视频…），别让
/// worker 为它们白跑一次 IO。能不能真的解出来交给 WIC——系统没装对应解码器时
/// （本机 WebP 就缺），`CreateDecoderFromFilename` 会直接失败，这里静默跳过。
fn is_image_ext(path: &str) -> bool {
    let ext = path
        .rsplit(['\\', '/'])
        .next()
        .and_then(|n| n.rsplit_once('.'))
        .map_or("", |(_, e)| e);
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "jfif"
            | "gif"
            | "bmp"
            | "webp"
            | "ico"
            | "tif"
            | "tiff"
            | "heic"
            | "avif"
    )
}

/// 单文件图片 → 列表缩略图（也是预览大图的唯一来源）。
///
/// ★★★ WIC 的必经链路是 **`frame → scaler → converter → CopyPixels`**，三步都不能省：
///
/// ① **`CopyPixels` 不做像素格式转换**——它只按你给的 stride 把源字节原样拷出来，
///    "请求 BGRA" 不等于 "得到 BGRA"。2026-10-02 实测同一张图四种容器：JPEG/PNG/BMP
///    的 alpha 是垃圾（25% 为 0，无法画），GIF 更彻底——它的原生格式是 `8bppIndexed`，
///    拷出来的是**调色板索引**，中心像素 `[0,0,0,0]`（全透明），画上去一片空白。
///    插上 [`IWICFormatConverter`] 显式转到 `GUID_WICPixelFormat32bppBGRA` 后，
///    alpha 归位（全部 255）、颜色也对得上（背景白 `[255,255,255,255]`）。
///    ★ 与录屏那条 MF 经验同型：「不显式声明 subtype/stride 就得不到 RGB32」——这里
///      是「不显式过 converter 就得不到 BGRA」。请求的格式 ≠ 拿到的格式，两次都栽在这。
///
/// ★★ 2026-10-02 追加实测，**别因此对 converter 掉以轻心**：这台机器上只拆掉
///    converter 那一步，测试仍然通过——scaler 自身也会把调色板图转成 BGRA。但微软
///    文档写的是 scaler 保持源像素格式，本机行为属于未文档化的实现细节；留着
///    converter 是几乎不花钱的双重保险。（而**两个都拆**就一定炸：见那条测试的判据。）
///
/// ② **顺序必须是先 scaler 再 converter**。反过来的话会先把全尺寸解码出来再缩，
///    4000×3000 实测峰值 **45.8MB**（decoder 会拉整幅缓冲）；先缩则全程只占
///    720×540×4 = **1.48MB**，而且更快（同一张 big.jpg：全解 21ms → 先缩 9ms）。
fn decode_image_file(factory: &IWICImagingFactory, path: &str) -> Option<Shot> {
    unsafe {
        let dec = factory
            .CreateDecoderFromFilename(
                &HSTRING::from(path),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnLoad,
            )
            .ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        // WIC 的 Fant 重采样比 [`thumbnail_of`] 的最近邻好得多，所以这里直接缩到目标宽，
        // 不再二次采样。窄图也不放大，与 `thumbnail_of` 同一口径。
        let nw = (THUMB_MAX_W as u32).min(w);
        let nh = ((h as f64) * (nw as f64 / w as f64)).max(1.0) as u32;
        let scaler: IWICBitmapScaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, nw, nh, WICBitmapInterpolationModeFant)
            .ok()?;

        let conv: IWICFormatConverter = factory.CreateFormatConverter().ok()?;
        conv.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppBGRA as *const _,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .ok()?;

        let src: IWICBitmapSource = conv.cast::<IWICBitmapSource>().ok()?;
        let stride = (nw as usize) * 4;
        let mut bgra = vec![0u8; stride * (nh as usize)];
        src.CopyPixels(std::ptr::null(), stride as u32, &mut bgra)
            .ok()?;
        Some(Shot {
            w: nw as i32,
            h: nh as i32,
            bgra,
        })
    }
}

/// COM 初始化守卫：保证 `CoInitializeEx` / `CoUninitialize` 配对。
///
/// WIC 走 COM，而 worker 是个**独立的后台线程**——COM 的公寓是线程级的，面板线程
/// 那边初始化过不代表这边有了。这里用 MTA（`COINIT_MULTITHREADED`），与 UI 线程的
/// STA 互不影响。
///
/// ★ 用守卫不手写配对：worker 平时跑 `recv()` 卡着不退出，但如果 SendError 让它
///   掉出循环（面板关闭），退出路径也得回收——RAII 覆盖全部路径。
struct ComGuard {
    live: bool,
}

impl ComGuard {
    fn new() -> Self {
        let live = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() };
        Self { live }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.live {
            unsafe { CoUninitialize() };
        }
    }
}

/// 单个图片文件 → 列表缩略图（**不存原件**）。
///
/// 与 [`capture_image_bg`] 的关键差别：**这里刻意不存原件**。文件条目本来就是引用
/// 用户的原路径，粘回去自然是原文件、原分辨率，我们再往自己目录里拷一份毫无收益，
/// 只是平白长占磁盘。
fn capture_file_thumb_bg(
    factory: Option<&IWICImagingFactory>,
    path: &std::path::Path,
    dir: &std::path::Path,
) -> Option<CapturedImg> {
    let factory = factory?;
    // 先问大小再决定要不要解：为一张 720px 的小图去读几百 MB 的原始文件不值。
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > FILE_THUMB_MAX_BYTES {
        return None;
    }
    let shot = decode_image_file(factory, &path.to_string_lossy())?;
    // 与 Image 同一条纪律：按内容（缩略图像素）哈希命名，同一张图反复复制覆盖同一文件，
    // 不会在 `thumbs/` 里堆出历史。
    let hash = compute_sig(Kind::File, "", &[], shot.w, shot.h, Some(&shot)) ^ full_hash(&None);
    let p = dir.join(format!("{hash:016x}.png"));
    if save_png(&shot, &p).is_err() {
        return None;
    }
    Some(CapturedImg {
        rel: to_app_relative(&p),
        w: shot.w,
        h: shot.h,
        thumb: shot,
        // 恒 `None`：这一路**故意不存原件**——它是用户自己的文件，原分辨率一直在
        // 原路径上，不需要我们替他再保管一份。（详情见本函数上方的注释。）
        full: None,
    })
}

/// worker 线程：捕获当前剪贴板图片（DIB→RGBA 解码 + 缩略 + PNG 编码 + 落盘）。
///
/// 返回 `None` = 自写屏蔽命中 / 剪贴板打开失败 / 已不是图片——与旧同步路径同一
/// 语义（拿不到就放弃，等下一次事件再试）。`last` 缓存上一次结果：同一张图
/// （哈希相同）跳过缩略 + PNG 编码 + 写盘直接复用——重复开面板不再反复编码
/// 同一张大图。哈希与文件名都和 `push` 的去重签名完全一致（同参重算）。
/// 返回值 = `(捕获结果, 本次新写入 `images/` 的字节数)`；字节数交给调用方累加做总量提示。
fn capture_image_bg(
    self_fmt: u32,
    dir: &std::path::Path,
    full_dir: Option<&std::path::Path>,
    last: &mut Option<(u64, CapturedImg)>,
) -> (Option<CapturedImg>, u64) {
    unsafe {
        // 自写屏蔽（与面板线程 `capture` 同判据）：我们粘贴写回触发的更新不收。
        if self_fmt != 0 && IsClipboardFormatAvailable(self_fmt).is_ok() {
            return (None, 0);
        }
        if IsClipboardFormatAvailable(8).is_err() {
            return (None, 0); // 8 = CF_DIB：投递与执行之间剪贴板可能已变。
        }
        let shot = match get_image() {
            Some(s) => s,
            None => return (None, 0),
        };
        let thumb = thumbnail_of(&shot, THUMB_MAX_W);
        // 与 `push` 完全同一套签名（kind=Image、全文 None）——面板侧 `push`
        // 用同参重算哈希去重，两边必须逐参一致。
        let hash =
            compute_sig(Kind::Image, "", &[], shot.w, shot.h, Some(&thumb)) ^ full_hash(&None);
        if let Some((h0, cap)) = last {
            if *h0 == hash {
                return (Some(cap.clone()), 0);
            }
        }
        let path = dir.join(format!("{hash:016x}.png"));
        let _ = save_png(&thumb, &path);
        // ★ 全尺寸原件：只在**真的被降采样过**时才落。`thumbnail_of` 对窄于阈值的图原样
        //   返回（见其首行 `shot.w <= max_w` 分支），此时 `thumb` 与 `shot` 是同一份像素，
        //   再写一遍纯属浪费；反过来，一旦降采样发生，这是全项目唯一一次经手整张像素的
        //   机会——错过就再也拿不回来。文件名沿用同一个哈希，同图反复捕获覆盖同一文件。
        let full = full_dir
            .filter(|_| thumb.w != shot.w)
            .and_then(|d| {
                let p = d.join(format!("{hash:016x}.png"));
                save_png(&shot, &p).ok()?;
                Some((
                    to_app_relative(&p),
                    std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0),
                ))
            })
            .unwrap_or((String::new(), 0));
        let nbytes = full.1;
        let cap = CapturedImg {
            rel: to_app_relative(&path),
            w: shot.w,
            h: shot.h,
            thumb,
            full: Some(full.0).filter(|s| !s.is_empty()),
        };
        *last = Some((hash, cap.clone()));
        (Some(cap), nbytes)
    }
}

fn compute_sig(
    kind: Kind,
    text: &str,
    files: &[String],
    w: i32,
    h: i32,
    shot: Option<&Shot>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    kind.hash(&mut hasher);
    text.hash(&mut hasher);
    files.hash(&mut hasher);
    w.hash(&mut hasher);
    h.hash(&mut hasher);
    if let Some(s) = shot {
        let n = s.bgra.len().min(4096);
        s.bgra[..n].hash(&mut hasher);
    }
    hasher.finish()
}

/// `full` 落盘路径的独立哈希（与 [`compute_sig`] 异或掺和，见 `push` 注释）。
fn full_hash(full: &Option<String>) -> u64 {
    let mut hasher = DefaultHasher::new();
    full.hash(&mut hasher);
    hasher.finish()
}

/// 纯函数：返回命中的条目索引（指向 `entries`，顺序为最新在前）。
fn visible(entries: &[ClipEntry], tab: TabKind, query: &str) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let tab_ok = match tab {
                TabKind::All => true,
                TabKind::Fav => e.fav,
                TabKind::Text => e.kind == Kind::Text,
                TabKind::File => e.kind == Kind::File,
                TabKind::Image => e.kind == Kind::Image,
                TabKind::Link => e.kind == Kind::Link,
            };
            if !tab_ok {
                return false;
            }
            if q.is_empty() {
                return true;
            }
            let hay = match e.kind {
                Kind::File => e.files.join("\n"),
                _ => e.text.clone(),
            };
            hay.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// 条目**第二行**：内容 / 文件名（★ 一行卡片只有两行：第一行 [`meta_line`]，第二行这个）。
///
/// 旧版是三行（meta + 摘要 + "N 字符"），用户点名"第三行不要"——字符数属于 meta 那一档
/// 的信息，塞在第三行只会把行高顶起来、一屏能看的条数还变少了。
fn content_line(e: &ClipEntry) -> String {
    match e.kind {
        Kind::Text | Kind::Link => e.text.lines().next().unwrap_or("").trim().to_string(),
        Kind::File => match e.files.first() {
            None => String::new(),
            Some(f) => {
                let name = file_name_of(f);
                if e.files.len() > 1 {
                    format!("{name} 等 {} 项", e.files.len())
                } else {
                    name
                }
            }
        },
        // 尺寸已在第一行（meta）里给过，第二行给**文件名**（2026-09-29 用户口径）：
        // 图片条目本来就落盘在 `temp/clipboard/thumbs/<id>.png`、条目只是"连接到那里"，
        // 把文件名亮出来，用户要去安装目录找这张图时直接照着抄；没有文件名的兜底标类型。
        Kind::Image => e
            .thumb
            .as_deref()
            .map(file_name_of)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "图片".to_string()),
    }
}

/// 取路径末段（文件名 / 文件夹名）。
///
/// ★ 按**字节**找最后一个分隔符：`\\` 与 `/` 都是 ASCII，绝不可能落在多字节字符中间，
/// 用 `chars()` 下标既慢又容易 panic（本项目铁律）。
fn file_name_of(p: &str) -> String {
    let b = p.as_bytes();
    let start = b
        .iter()
        .rposition(|&c| c == b'\\' || c == b'/')
        .map(|i| i + 1)
        .unwrap_or(0);
    String::from_utf8_lossy(&b[start..]).to_string()
}

/// 文件条目的外部引用是否**全部**不在原处。
///
/// 判"全部"而不是"任一"：一次拷贝一批文件时只要还有一条能落，粘贴就照常执行
/// （写回时只写存活的）；只有整体失效才把这条标成"源文件已不在"。空列表不算失效。
fn files_all_missing(files: &[String]) -> bool {
    !files.is_empty() && files.iter().all(|p| !std::path::Path::new(p).exists())
}

/// 源程序 opt-out 判定（纯函数）：密码管理器等复制时会额外放一个"请勿记录我"的
/// 标记格式，任一存在即请求不被收录。注册失败的 id 恒为 0，必须跳过——0 恰好是
/// `CF_*` 标准格式的取值域，拿去问 `IsClipboardFormatAvailable` 会误命中。
fn optout_requested(fmts: &[u32], has: &dyn Fn(u32) -> bool) -> bool {
    fmts.iter().any(|f| *f != 0 && has(*f))
}

/// 捕获重试决策（纯函数）：`(handled, wanted, 已重试次数) → (新计数, 是否再定时)`。
///
/// 只有"有我们认识的格式却没读到"（`!handled && wanted`）才重试，且最多
/// [`CAP_RETRY_MAX`] 次；成功或确实无内容一律归零——为空剪贴板空转定时器没有意义。
fn retry_decision(handled: bool, wanted: bool, done: u32) -> (u32, bool) {
    if handled || !wanted {
        return (0, false);
    }
    if done < CAP_RETRY_MAX {
        (done + 1, true)
    } else {
        (0, false)
    }
}

/// 这一条是不是"文件夹"：文件类条目里**每个**路径都是目录才画文件夹图标。
///
/// 为什么只改图标不新开分类（2026-09-24 拍板）：一次 `CF_HDROP` 拷贝可以同时含文件和
/// 文件夹，拆成两类就得给混合拷贝造一个"既属于文件又属于文件夹"的第三条路，页签反而
/// 变啰嗦。分类保持 文本/链接/图片/文件 四项，靠图标把"拷的是整个文件夹"这件事说出来。
fn entry_is_folder(e: &ClipEntry) -> bool {
    e.kind == Kind::File
        && !e.files.is_empty()
        && e.files.iter().all(|p| std::path::Path::new(p).is_dir())
}

/// 滚动条滑块几何（纯函数）。`total` = 可见条目数，`scroll` = 当前偏移。
///
/// 内容不满一屏时返回**铺满整条**（视觉上等于"没有可滚的"），不隐藏——用户报的就是
/// "条目多时看不到滚动条"，让它始终有形状、只靠长短表达比例，比忽有忽无好认。
fn scroll_thumb(total: usize, rows: usize, scroll: usize) -> (i32, i32) {
    let t = scroll_track();
    let track_h = t.bottom - t.top;
    let rows = rows.max(1);
    if total <= rows {
        return (t.top, track_h);
    }
    let h = ((track_h as f64 * rows as f64 / total as f64) as i32)
        .max(16)
        .min(track_h);
    let max_scroll = (total - rows).max(1);
    let span = (track_h - h).max(1);
    let y = t.top + ((span as f64 * (scroll.min(max_scroll)) as f64 / max_scroll as f64) as i32);
    (y, h)
}

/// 最大滚动偏移（纯函数）。
fn max_scroll(total: usize, rows: usize) -> usize {
    total.saturating_sub(rows)
}

/// unix 秒 → "MM-DD HH:MM"（纯函数，civil_from_days 算法，避免引 chrono；按 UTC 算）。
///
/// ★ 只给测试用：面板显示走 [`fmt_time_hm_local`]（要跟系统时区/DST）。留着它是因为
///   这条纯函数把 `civil_from_days` 的日期换算钉在单测里，而 Win32 那条链路测不了纪元。
#[cfg(test)]
fn fmt_time_hm(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let secs = ts % 86_400;
    let (h, mi) = ((secs / 3600) as u32, (secs % 3600 / 60) as u32);
    // Howard Hinnant 的 civil_from_days（z 从 1970-01-01 起的天数）。
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    // 年算出来本该用于"1/2 月归到下一年"，但本函数只输出"月-日 时:分"，不回带。
    let _ = y;
    format!("{:02}-{:02} {:02}:{:02}", m, d, h, mi)
}

/// unix 秒 → 本地时区 "MM-DD HH:MM"（Win32 FileTime 链路，DST 由系统处理）。
fn fmt_time_hm_local(ts: u64) -> String {
    unsafe {
        // unix → FILETIME（1601 起 100ns）
        let fts = ts * 10_000_000 + 116_444_736_000_000_000;
        let ft = FILETIME {
            dwLowDateTime: (fts & 0xFFFF_FFFF) as u32,
            dwHighDateTime: (fts >> 32) as u32,
        };
        let mut lft = FILETIME::default();
        let _ = FileTimeToLocalFileTime(&ft, &mut lft);
        let mut st = SYSTEMTIME::default();
        let _ = FileTimeToSystemTime(&lft, &mut st);
        format!(
            "{:02}-{:02} {:02}:{:02}",
            st.wMonth, st.wDay, st.wHour, st.wMinute
        )
    }
}

/// 字节数人性化："N B" / "N KB" / "N.N MB"。
fn human_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{} KB", n / 1024)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// 条目**第一行**："MM-DD HH:MM · 20 KB"。
///
/// 图片带尺寸、文本/链接带字节数；文件没有体积可查（要 `fs::metadata` 逐条问盘），
/// 就写"文件 / 文件夹"——项数交给第二行（"报告.docx 等 3 项"）说，两行不重复。
fn meta_line(e: &ClipEntry) -> String {
    let size = match e.kind {
        Kind::Image => format!("{}×{}", e.w, e.h),
        Kind::File => {
            // ★ 外部引用失效时第一行直接说原因（第二行文件名转警示色见行绘制处）：
            //   这条不是错误数据，删不得——U 盘插回来它就自己好了。
            if e.missing {
                "源文件已不在".to_string()
            } else if entry_is_folder(e) {
                "文件夹".to_string()
            } else {
                "文件".to_string()
            }
        }
        _ => human_bytes(e.text.len()),
    };
    format!("{} · {}", fmt_time_hm_local(e.ts), size)
}

/// panel.json 一行式解析（旧文件/损坏 → 宽容回退）。
fn parse_panel_state(s: &str) -> (Option<(i32, i32)>, bool) {
    let v: serde_json::Value = match serde_json::from_str(s) {
        Ok(v) => v,
        Err(_) => return (None, false),
    };
    let pos = match (
        v.get("x").and_then(|x| x.as_i64()),
        v.get("y").and_then(|y| y.as_i64()),
    ) {
        (Some(x), Some(y)) => Some((x as i32, y as i32)),
        _ => None,
    };
    let pinned = v.get("pinned").and_then(|p| p.as_bool()).unwrap_or(false);
    (pos, pinned)
}

// ── 面板窗口记忆：线程间移交（剪贴板线程 → 帧循环）────────────────────────
//
// config.json 的唯一写者是运行时线程（`Config::save` 原子写）。剪贴板线程改了窗口
// 位置/置顶态后不能直接写它（双写者会互相踩），于是入这个槽位，由
// `Runtime` 的帧循环每帧取走并入内存 Config 再落盘。
fn panel_state_slot() -> &'static std::sync::Mutex<Vec<crate::core::config::ClipPanelGeom>> {
    static SLOT: OnceLock<std::sync::Mutex<Vec<crate::core::config::ClipPanelGeom>>> =
        OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn queue_panel_state(g: crate::core::config::ClipPanelGeom) {
    if let Ok(mut q) = panel_state_slot().lock() {
        q.push(g);
    }
}

/// 帧循环取走最新一条面板窗口记忆（后进优先：只认最后一次变更）。
pub fn take_pending_panel_state() -> Option<crate::core::config::ClipPanelGeom> {
    let mut q = panel_state_slot().lock().ok()?;
    q.pop()
}

/// 一次性迁移：旧的 `temp/clipboard/panel.json` → `config.json` 的 `clipboard.panel`。
///
/// 仅当总配置里还没有面板记忆时执行；迁完删旧文件。由 lib.rs 启动时调用
/// （那里 `Config` 可变、且是单实例，迁移副作用安全）。
pub fn migrate_legacy_panel_state(config: &mut crate::core::config::Config) {
    if config.clipboard.panel.is_some() {
        return;
    }
    let legacy = module_temp_dir("clipboard").join("panel.json");
    let Ok(s) = std::fs::read_to_string(&legacy) else {
        return;
    };
    let (pos, pinned) = parse_panel_state(&s);
    let Some((x, y)) = pos else {
        let _ = std::fs::remove_file(&legacy);
        return;
    };
    config.clipboard.panel = Some(crate::core::config::ClipPanelGeom { x, y, pinned });
    let _ = config.save();
    let _ = std::fs::remove_file(&legacy);
    log::info!("剪贴板: 旧 panel.json 窗口记忆已并入 config.json 并删除旧文件");
}

// ── 管理器（单后台线程独占）─────────────────────────────────────────────

/// worker 任务（面板线程 → worker）：重活全在后台线程，面板线程只投递/收结果。
enum CapJob {
    /// 解码一张历史缩略图（懒加载可见行，见 [`ClipboardMgr::ensure_visible_thumbs`]）。
    Thumb(u64, PathBuf),
    /// 捕获当前剪贴板图片：DIB→RGBA 解码 + 缩略 + PNG 编码 + 落盘全在 worker。
    /// （2026-09-29 用户报"打开面板悬停转圈好几秒"：旧实现这套同步跑在面板线程，
    /// 且去重在 `push` 里——同一张图每次开面板都白跑全套，消息泵停转 = 忙碌光标。）
    ///
    /// `full_dir` = 全尺寸原件目录（`images/`）；传 `None` 表示这一路不需要原件。
    Capture {
        self_fmt: u32,
        dir: PathBuf,
        full_dir: Option<PathBuf>,
    },
    /// 给**单个**图片文件补一张列表缩略图（见 [`is_image_ext`]）。
    ///
    /// ★ 注意这不复制文件本身：文件条目一向只是引用原路径（`files[]`），这里额外
    ///   生成的是一张 720px 预览小图，落 `thumbs/`——粘回去仍走原文件，原分辨率不变。
    ///
    /// `id` = 已经入账的那条 File 条目。worker 只回填它的 `thumb`/`w`/`h`，
    /// **不动时间戳与顺序**：用户此刻已经在历史里看见它了，补一张预览不该让它重新上浮。
    FileThumb {
        id: u64,
        path: PathBuf,
        dir: PathBuf,
    },
}

/// worker 回传（对应 [`CapJob`]），`run` 消息循环抽干。
enum CapMsg {
    /// 缩略图解码完成；失败回传空 `Shot` 占位，避免面板对这条反复重试。
    Thumb(u64, Shot),
    /// 剪贴板图片捕获完成；`None` = 剪贴板打开失败 / 自写屏蔽命中 / 已不是图片
    /// （投递与执行之间剪贴板可能已变，与旧同步路径同一语义：拿不到就放弃）。
    Captured(Option<CapturedImg>),
    /// ★ 全尺寸原件目录已越过 [`IMG_DIR_WARN_BYTES`]：带当前总字节数，面板侧展示一次。
    /// 只告知不处理——不设限、不自动清理是用户口径，删什么由用户点了算。
    StorageWarn(u64),
    /// 单文件图片的缩略图就绪。`None` = 不是图片 / 系统缺这类解码器（本机 WebP 就缺）/
    /// 文件超过 [`FILE_THUMB_MAX_BYTES`]——这几种情况都是"静默没有预览"，不该打扰用户。
    FileThumb(u64, Option<CapturedImg>),
}

/// worker 捕获完成的剪贴板图片。面板线程收到后只做入账（`push` 同一套哈希去重）。
#[derive(Clone)]
struct CapturedImg {
    /// 缩略图 app 相对路径（按内容哈希命名：同图反复捕获覆盖同一文件）。
    rel: String,
    /// 原图尺寸（哈希与条目元数据用全尺寸；缩略只是显示用）。
    w: i32,
    h: i32,
    /// 已解码缩略图：入账即插 `thumbs`，首屏不用再解码。
    thumb: Shot,
    /// ★ 全尺寸原件路径（`images/<同一哈希>.png`，app 相对）。
    ///
    /// 为什么必须另存一份：`thumbs/` 里那份是 `thumbnail_of` 的**降采样副本**，面板显示
    /// 用它是对的，但粘贴时若也用它写回，用户拿到的就是 720px 宽的重采样图，原分辨率
    /// 永久丢失（`Capture` 只在入账这一刻经手整张像素，错过就再也拿不回来）。
    ///
    /// `None` 有两种成因，都算正常：一是这张图**没被降采样**（窄于 `THUMB_MAX_W`，
    /// `thumbnail_of` 原样返回），`rel` 那份本身就是全尺寸，没必要再存一遍；二是写盘失败，
    /// 此时自动退回旧行为（粘贴仍给缩略图），不让 IO 问题拖垮整条捕获链。
    full: Option<String>,
}

struct ClipboardMgr {
    cfg: ClipCfg,
    entries: Vec<ClipEntry>,
    thumbs: HashMap<u64, Shot>,
    /// 任务通道（面板线程 → worker）：只投递**可见行**的缩略图与"捕获当前剪贴板"，
    /// 任何解码/编码都不进面板线程（见 [`Self::ensure_visible_thumbs`]）。
    job_tx: mpsc::Sender<CapJob>,
    /// 回传通道（worker → 面板线程），在 `run` 消息循环里抽干并插 `thumbs` / 入账。
    thumb_rx: mpsc::Receiver<CapMsg>,
    /// 已投递但尚未回传的缩略图 id（去重回压，避免同一条重复解码）。
    in_flight: HashSet<u64>,
    next_id: u64,
    dir: std::path::PathBuf,
    jsonl: std::path::PathBuf,
    thumbs_dir: std::path::PathBuf,
    /// 超限大文本的全文落盘目录（`texts/<内容哈希>.txt`，条目里只留预览）。
    texts_dir: std::path::PathBuf,
    /// 全尺寸图片原件目录（`images/<内容哈希>.png`，显示用 `thumbs/`、粘贴用这里）。
    images_dir: std::path::PathBuf,
    /// ★ 顶栏告知条当前内容（借用计数槽位亮相 [`WARN_SHOW_MS`] 毫秒；`None` = 没事，
    ///   点掉即清，不再复读）。
    top_notice: Option<TopNotice>,
    /// ★ 本次剪贴板变更已经因"格式在却读不到"重试过几次（见 [`RETRY_TIMER`]）。
    /// 任何一次成功捕获或"本来就没内容"都会归零。
    clip_retry: u32,
    self_fmt: u32,
    /// ★ 源程序 opt-out 标记格式（"ExcludeClipboardContentFromMonitorProcessing" /
    /// "Clipboard Viewer Ignore"）：任一在剪贴板上即跳过收录（见构造处的注册注释）。
    optout_fmts: [u32; 2],

    panel: HWND,
    preview: HWND,
    edit: HWND,
    last_foreground: HWND,
    /// 面板弹出前目标线程的键盘焦点控件（焦点保存与归还思路）：
    /// 粘贴时用它把光标还回目标输入框——这是"点击条目后光标失焦"的修复。
    saved_focus: HWND,

    query: String,
    tab: TabKind,
    selected: usize,
    scroll: usize,
    hover: Option<usize>,
    /// 顶栏按钮的悬停序号（自右往左 `0`=关闭 `1`=收起 `2`=置顶）；`None` = 没压在按钮上。
    ///
    /// ★ 用户第 2 点：旧版顶栏图标悬停**没有任何背景变化**。便签的做法是"悬停时铺一层
    ///   底纹"，这里照抄——但 `WM_MOUSEMOVE` 不会让窗口变脏，改了必须自己重绘（同
    ///   [`Self::hover`]）。
    hover_btn: Option<i32>,
    /// 行内动作按钮的悬停（`(行全局下标 gi, 按钮号 0=↑复制置顶 1=🗑 2=★)`）。
    ///
    /// ★ 2026-09-28 用户要求：行右侧三颗动作按钮悬停也要有底纹（与顶栏按钮同款待遇）。
    /// 带上 gi 是为了鼠标换行时旧按钮底纹能正确消掉（只比按钮号不够）。
    hover_act: Option<(usize, u8)>,
    /// 置顶：blur 不再自动隐藏。
    pinned: bool,
    /// 收起：只剩顶栏。
    collapsed: bool,
    /// 上次面板位置（panel.json 记忆）。
    pos_saved: Option<(i32, i32)>,

    font: HFONT,
    /// 小一号字（meta 行 / 页签）：卡片两行里 meta 是"说明"，不该跟内容抢字号。
    font_sm: HFONT,
    /// Segoe UI Symbol 单色字形（📌 ✕ 🔍 🗑 ★），颜色跟 SetTextColor（便签定案）。
    sym_font: HFONT,
    bg_brush: HBRUSH,
    sel_brush: HBRUSH,
    hover_brush: HBRUSH,
    /// 卡片行底（未选中未悬停那档）。
    row_brush: HBRUSH,
    /// 搜索框（原生 EDIT）底。
    edit_bg_brush: HBRUSH,
    /// 页签条底（未激活）。
    tab_brush: HBRUSH,
    track_brush: HBRUSH,
    thumb_brush: HBRUSH,
    /// 顶栏按钮悬停底纹（[`mix`] 出来的那一档，见 `BTN_HOVER_MIX`）。
    btn_hover_brush: HBRUSH,
    /// 滚动条滑块"正在拖"的深一档。
    thumb_drag_brush: HBRUSH,
    /// 1px 外框（[`C_BORDER`]）。
    border_brush: HBRUSH,
    /// 拖动滚动条时的抓取偏移（`None` = 没在拖）。
    scroll_drag: Option<i32>,

    preview_entry: Option<ClipEntry>,
    preview_shot: Option<Shot>,
}

impl ClipboardMgr {
    fn new() -> Self {
        // ★ 数据目录 = `temp/clipboard`（2026-09-29 用户口径："既然 temp 默认不删，
        //   剪贴板文件还是移回 temp，全放外面太麻烦"）。上一版曾搬到 `<exe>/clipboard/`，
        //   这里做一次**反向迁移**把数据搬回来，用户历史不丢。
        let dir = module_temp_dir("clipboard");
        let stray = crate::core::paths::data_dir().join("clipboard");
        if stray.is_dir() {
            let _ = std::fs::create_dir_all(&dir);
            for name in ["history.jsonl", "panel.json"] {
                let old = stray.join(name);
                let new = dir.join(name);
                if old.is_file() && !new.exists() {
                    let _ = std::fs::rename(&old, &new);
                }
            }
            let old_thumbs = stray.join("thumbs");
            let new_thumbs = dir.join("thumbs");
            if old_thumbs.is_dir() && !new_thumbs.exists() {
                let _ = std::fs::rename(&old_thumbs, &new_thumbs);
            }
            let old_texts = stray.join("texts");
            let new_texts = dir.join("texts");
            if old_texts.is_dir() && !new_texts.exists() {
                let _ = std::fs::rename(&old_texts, &new_texts);
            }
            // 搬空后删掉外面的残留目录（搬不动的就不强求）。
            let _ = std::fs::remove_dir(&stray);
        }
        let thumbs_dir = dir.join("thumbs");
        let _ = std::fs::create_dir_all(&thumbs_dir);
        // 超限大文本的落盘目录（条目里只留预览，全文在这里）。
        let texts_dir = dir.join("texts");
        let _ = std::fs::create_dir_all(&texts_dir);
        // ★ 全尺寸图片原件目录（`images/`）：`thumbs/` 之外单独放一处，一个是给眼睛看的
        //   降采样副本、一个是给粘贴用的原件，两者同名（内容哈希）但不相干，分目录便于
        //   各自清点，也让"这两个目录里的东西都是我们自己写的"在磁盘上一眼可辨。
        let images_dir = dir.join("images");
        let _ = std::fs::create_dir_all(&images_dir);
        let self_fmt =
            unsafe { RegisterClipboardFormatW(PCWSTR(wide("CurSeen.ClipboardSelf").as_ptr())) };
        // ★ opt-out 标记（2026-10-02 用户拍板）：密码管理器（KeePass/1Password 等）复制时
        //   会额外放一个"请勿记录我"的标记格式，Windows 自带剪贴板历史也尊重它。我们只认
        //   信号、不读内容——这与"扫内容猜密码"（已永久否决）是两回事。注册名大小写不敏感。
        let optout_fmts: [u32; 2] = [
            unsafe {
                RegisterClipboardFormatW(PCWSTR(
                    wide("ExcludeClipboardContentFromMonitorProcessing").as_ptr(),
                ))
            },
            unsafe { RegisterClipboardFormatW(PCWSTR(wide("Clipboard Viewer Ignore").as_ptr())) },
        ];
        let (font, font_sm, sym_font) =
            unsafe { (make_ui_font(16), make_ui_font(14), make_sym_font(17)) };
        // panel.json：**已废除**（2026-09-29 用户口径，窗口记忆并入 config.json 的
        //   `clipboard.panel`）。旧文件的迁移见 `migrate_legacy_panel_state`（lib.rs 启动
        //   时调用）；这里只留安全的空初值，真实值经 `apply_config` 首次送达。
        let (pos_saved, pinned) = (None, false);
        // ★ 浅灰主题（2026-09-24 用户定案）。色值全在文件末尾那组常量里，画刷只是一层
        //   包装——改主题改常量即可，别在这儿写字面量。
        // ★ 四档必须**拉得开**：旧版 hover 与 bg 只差 13/255，鼠标划过几乎看不出变化
        //   （用户直接报"没有 hover 变色"）；现在底 240 → 行 250 → 悬停 226 → 选中 200。
        let btn_hover = mix(C_BG, C_TEXT, BTN_HOVER_MIX);
        let (
            bg_brush,
            sel_brush,
            hover_brush,
            row_brush,
            edit_bg_brush,
            tab_brush,
            track_brush,
            thumb_brush,
            btn_hover_brush,
            thumb_drag_brush,
            border_brush,
        ) = unsafe {
            (
                CreateSolidBrush(RGB(C_BG.0, C_BG.1, C_BG.2)),
                CreateSolidBrush(RGB(C_SEL.0, C_SEL.1, C_SEL.2)),
                CreateSolidBrush(RGB(C_HOVER.0, C_HOVER.1, C_HOVER.2)),
                CreateSolidBrush(RGB(C_ROW.0, C_ROW.1, C_ROW.2)),
                CreateSolidBrush(RGB(C_EDIT_BG.0, C_EDIT_BG.1, C_EDIT_BG.2)),
                CreateSolidBrush(RGB(C_TAB.0, C_TAB.1, C_TAB.2)),
                CreateSolidBrush(RGB(C_TRACK.0, C_TRACK.1, C_TRACK.2)),
                CreateSolidBrush(RGB(C_THUMB.0, C_THUMB.1, C_THUMB.2)),
                CreateSolidBrush(RGB(btn_hover.0, btn_hover.1, btn_hover.2)),
                CreateSolidBrush(RGB(C_THUMB_DRAG.0, C_THUMB_DRAG.1, C_THUMB_DRAG.2)),
                CreateSolidBrush(RGB(C_BORDER.0, C_BORDER.1, C_BORDER.2)),
            )
        };
        // ★ 缩略图解码：面板线程只**懒加载可见行**的图片（见 `ensure_visible_thumbs`）。
        //   通道在构造期建好、worker 独立线程常驻；worker 经 `CMD_HWND` 静态量取命令窗
        //   句柄回传唤醒（命令窗创建晚于本闭包，但任务只在面板可见后才投递，届时已就绪）。
        let (job_tx, job_rx) = mpsc::channel::<CapJob>();
        let (thumb_tx, thumb_rx) = mpsc::channel::<CapMsg>();
        let worker_dir = images_dir.clone();
        std::thread::spawn(move || {
            // 上一次捕获缓存：同一张图反复捕获（每次开面板 / 每次复制都投任务）时，
            // get_image 的解码与哈希判定仍要做，但缩略 + PNG 编码 + 写盘只在哈希
            // 变化时重跑——重复开面板不再反复编码同一张大图（后台线程，不挡消息泵）。
            let mut last_cap: Option<(u64, CapturedImg)> = None;
            // 原件目录占用账：起手先数一次，覆盖"上次运行就已经很大"的情况。
            let mut budget = ImgBudget::new(&worker_dir);
            // ★ COM 与 WIC 工厂都先于循环建好：`CoCreateInstance` 每 decode 一次都重做是纯浪费，
            //   而 WIC 工厂本身线程内复用是安全的。**先声明的后 drop**，所以 `_com` 写在最前
            //   —— 保证 WIC 工厂在 `CoUninitialize` 之前先析构。
            let _com = ComGuard::new();
            let wic: Option<IWICImagingFactory> = unsafe {
                CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()
            };
            while let Ok(job) = job_rx.recv() {
                let msg = match job {
                    CapJob::Thumb(id, path) => {
                        // 解码失败（文件丢了）也回传空 `Shot` 占位，避免面板对这条反复重试。
                        CapMsg::Thumb(id, shot_from_png(&path).unwrap_or_default())
                    }
                    CapJob::Capture {
                        self_fmt,
                        dir,
                        full_dir,
                    } => {
                        let (cap, n) =
                            capture_image_bg(self_fmt, &dir, full_dir.as_deref(), &mut last_cap);
                        // 只在真的写入了原件时才过账（重复捕获同一张图 n=0，不必重数目录）。
                        if n > 0 && budget.add(n, &worker_dir) {
                            let total = budget.known + budget.pending;
                            let _ = thumb_tx.send(CapMsg::StorageWarn(total));
                        }
                        CapMsg::Captured(cap)
                    }
                    CapJob::FileThumb { id, path, dir } => {
                        CapMsg::FileThumb(id, capture_file_thumb_bg(wic.as_ref(), &path, &dir))
                    }
                };
                let _ = thumb_tx.send(msg);
                if let Some(a) = CMD_HWND.get() {
                    let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
                    let _ =
                        unsafe { PostMessageW(Some(h), WM_APP_THUMB_WAKE, WPARAM(0), LPARAM(0)) };
                }
            }
        });
        ClipboardMgr {
            cfg: ClipCfg {
                enabled: false,
                max_entries: 500,
                panel: None,
            },
            entries: Vec::new(),
            thumbs: HashMap::new(),
            job_tx,
            thumb_rx,
            in_flight: HashSet::new(),
            next_id: 1,
            dir,
            jsonl: PathBuf::new(),
            thumbs_dir,
            texts_dir,
            images_dir,
            top_notice: None,
            clip_retry: 0,
            self_fmt,
            optout_fmts,
            panel: HWND::default(),
            preview: HWND::default(),
            edit: HWND::default(),
            last_foreground: HWND::default(),
            saved_focus: HWND::default(),
            query: String::new(),
            tab: TabKind::All,
            selected: 0,
            scroll: 0,
            hover: None,
            hover_btn: None,
            hover_act: None,
            pinned,
            collapsed: false,
            pos_saved,
            font,
            font_sm,
            sym_font,
            bg_brush,
            sel_brush,
            hover_brush,
            row_brush,
            edit_bg_brush,
            tab_brush,
            track_brush,
            thumb_brush,
            btn_hover_brush,
            thumb_drag_brush,
            border_brush,
            scroll_drag: None,
            preview_entry: None,
            preview_shot: None,
        }
    }

    fn load(&mut self) {
        self.jsonl = self.dir.join("history.jsonl");
        if let Ok(s) = std::fs::read_to_string(&self.jsonl) {
            for line in s.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(e) = serde_json::from_str::<ClipEntry>(line) {
                    if e.id >= self.next_id {
                        self.next_id = e.id + 1;
                    }
                    // ★ 缩略图**不在这里解码**（2026-09-29 用户报"首次打开转圈好几秒"）：
                    //   历史里每张图片缩略图都是一次 PNG 解码（720px 宽的一张几十毫秒），
                    //   几百条图片历史若一次性全解 = 卡死数秒。改为只载元数据，像素由
                    //   `ensure_visible_thumbs` 懒加载——只解当前**可见行**，经 worker 线程
                    //   解码后 `WM_APP_THUMB_WAKE` 回传刷面板（见 `run` 消息循环抽干）。
                    self.entries.push(e);
                }
            }
        }
    }

    /// 原子重写 JSONL（临时文件 → rename）。
    fn persist(&self) {
        let tmp = self.dir.join("history.jsonl.tmp");
        let mut out = String::new();
        for e in &self.entries {
            if let Ok(s) = serde_json::to_string(e) {
                out.push_str(&s);
                out.push('\n');
            }
        }
        if std::fs::write(&tmp, &out).is_ok() {
            let _ = std::fs::rename(&tmp, &self.jsonl);
        }
    }

    fn refresh_panel(&mut self) {
        // ★ 懒加载收口：每次重绘前确保**可见行**的图片已投递 worker 解码。幂等
        //   （已解码 `thumbs` / 已投递 `in_flight` 跳过），滚动 / 切页 / 搜索后新进入
        //   视野的行会在这里被补解——首开只解屏幕上那 ~20 行，面板始终即时响应。
        self.ensure_visible_thumbs();
        self.recheck_visible_files();
        unsafe {
            if IsWindowVisible(self.panel).as_bool() {
                // ★ `erase = false`：`WM_ERASEBKGND` 由 `panel_proc` 直接吞掉（返回 1），
                //   绘制走双缓冲——"先擦一遍再画一遍"那一趟中间态是闪烁的一半来源。
                let _ = InvalidateRect(Some(self.panel), None, false);
            }
        }
    }

    /// 只把**当前可见行**的图片缩略图投递给 worker 解码（懒加载）。
    ///
    /// ★ 彻底修复「首开转圈」：历史可能有成百上千条图片，若一次性全解码，哪怕在 worker
    ///   线程，成百上千次 `PostMessageW` + 重绘也会把面板线程饿到抽不动消息（光标转圈）。
    ///   只解屏幕上那 ~20 行，滚动 / 切页 / 搜索时再补解新进入视野的行；已解码或已投递
    ///   的绝不重复。同类工具有的把缩略图在**捕获时**就生成好小尺寸
    ///   RGBA 存进条目，打开历史只直接 blit——我们落盘的是整图 PNG，故改用"按需懒解码
    ///   可见行"达到同样"打开零解码等待"的效果。
    fn ensure_visible_thumbs(&mut self) {
        let vis = visible(&self.entries, self.tab, &self.query);
        let rows = visible_rows() as usize;
        let end = (self.scroll + rows).min(vis.len());
        // 直接切片迭代可见段的全局下标，避免 `vi` 仅作索引（clippy: needless_range_loop）。
        let s = self.scroll.min(vis.len());
        for &gi in &vis[s..end] {
            let e = &self.entries[gi];
            // ★ 判据是"这条有没有缩略图"而不是"是不是 Kind::Image"：复制单个图片文件时
            //   也会给它补一张预览，那种条目挂在 `Kind::File` 下——用 kind 当门会静默漏掉。
            let Some(p) = e.thumb.as_deref() else {
                continue;
            };
            let id = e.id;
            if self.thumbs.contains_key(&id) || self.in_flight.contains(&id) {
                continue;
            }
            self.in_flight.insert(id);
            let path = from_app_relative(p);
            let _ = self.job_tx.send(CapJob::Thumb(id, path));
        }
    }

    /// 懒校验**可见的 File 行**：外部引用是否还在原处，结果写进运行态 `missing`。
    ///
    /// ★ 刻意不做的事：不监听文件系统、不后台轮询全表。失效发现得晚一点没关系
    ///   （反正粘贴那一刻还有一道权威校验兜底），为它常驻一个 ReadDirectoryChangesW
    ///   或定时 stat 全表，与"空载零成本"的产品底线不成比例。这里只在列表刷新时
    ///   对可见的 ~20 行各问一次 `exists()`——微秒级，且不发生在绘制帧里。
    fn recheck_visible_files(&mut self) {
        let vis = visible(&self.entries, self.tab, &self.query);
        let rows = visible_rows() as usize;
        let end = (self.scroll + rows).min(vis.len());
        let s = self.scroll.min(vis.len());
        for &gi in &vis[s..end] {
            let e = &mut self.entries[gi];
            if e.kind != Kind::File || e.files.is_empty() {
                continue;
            }
            let m = files_all_missing(&e.files);
            if m != e.missing {
                e.missing = m;
            }
        }
    }

    /// 顶栏告知条亮相一轮（借用计数槽位，[`WARN_SHOW_MS`] 毫秒后 WARN_TIMER 自毁）。
    fn show_top_notice(&mut self, n: TopNotice) {
        self.top_notice = Some(n);
        unsafe {
            SetTimer(Some(self.panel), WARN_TIMER, WARN_SHOW_MS, None);
        }
        self.refresh_panel();
    }

    /// 只重画**列表那一块**（含右边滚动条那一列）。
    ///
    /// ★ 用户第 1 点"鼠标一动就闪"的主因之一：hover 一变就整窗 `InvalidateRect`，顶栏
    ///   （含原生 EDIT 搜索框）和页签条跟着一起重画，看上去就是整片在抖。hover 只影响
    ///   列表 → 只脏列表。
    fn invalidate_list(&self) {
        unsafe {
            if IsWindowVisible(self.panel).as_bool() {
                let r = RECT {
                    left: GAP,
                    top: LIST_TOP,
                    right: PANEL_W - GAP,
                    bottom: PANEL_H - GAP,
                };
                let _ = InvalidateRect(Some(self.panel), Some(&r), false);
            }
        }
    }

    /// 只重画顶栏（顶栏按钮悬停底纹变化用这个，别惊动下面整片列表）。
    fn invalidate_top(&self) {
        unsafe {
            if IsWindowVisible(self.panel).as_bool() {
                let r = RECT {
                    left: 0,
                    top: 0,
                    right: PANEL_W,
                    bottom: TOP_B,
                };
                let _ = InvalidateRect(Some(self.panel), Some(&r), false);
            }
        }
    }

    /// 写入一条新内容（去重上浮 + 容量淘汰）。
    // ★ clippy::too_many_arguments：一条剪贴项的全部字段（类型/文本/文件/缩略图/宽高/
    //   截图/富文本），9 个参数在唯一写入口一次给齐；拆 struct 反而多一道包装。
    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        kind: Kind,
        text: String,
        files: Vec<String>,
        thumb: Option<String>,
        w: i32,
        h: i32,
        shot: Option<Shot>,
        full: Option<String>,
        html: Option<String>,
    ) {
        // ★ hash 掺入 `full`（全文落盘路径，路径名本身含全文内容哈希）：超限文本的
        //   `text` 只剩预览，不同全文若共用同一段前 400 字节，预览相同——不掺入的话
        //   会被误判成同一条而丢更新。`html` 同理（路径名含 CF_HTML 内容哈希：
        //   同文字不同排版的两次复制必须各成一条，否则富文本会被纯文本覆盖）。
        let hash = compute_sig(kind, &text, &files, w, h, shot.as_ref())
            ^ full_hash(&full)
            ^ full_hash(&html);
        if let Some(pos) = self.entries.iter().position(|e| e.hash == hash) {
            let mut e = self.entries.remove(pos);
            e.ts = now_secs();
            self.entries.insert(0, e);
            self.persist();
            self.refresh_panel();
            return;
        }
        let id = self.next_id;
        self.next_id += 1;
        let entry = ClipEntry {
            id,
            kind,
            text,
            files,
            thumb,
            w,
            h,
            full,
            html,
            fav: false,
            ts: now_secs(),
            hash,
            missing: false,
        };
        self.entries.insert(0, entry);
        if let Some(shot) = shot {
            self.thumbs.insert(id, shot);
        }
        self.evict();
        self.persist();
        self.refresh_panel();
    }

    /// 超出容量：删除最旧的非收藏条目。
    ///
    /// ★ 用户档「不限」= `usize::MAX`（见 `config.rs` 的 `CLIP_ENTRIES_VALUES`），
    /// 淘汰判据 `len() > max` 恒不成立 → **真·不淘汰**（2026-09-29 用户口径：
    /// "要允许真无限"，此前加的 10_000 硬上限按该口径拆除）。每条文本本身有
    /// `MAX_TEXT_BYTES` 上限、缩略图有尺寸上限，单条体积有界；条目数的增长由
    /// 用户自己负责（收藏在「收藏」页，不受淘汰影响）。
    fn evict(&mut self) {
        let cap = self.cfg.max_entries;
        while self.entries.len() > cap {
            // 从尾部找第一个非收藏。
            if let Some(pos) = self.entries.iter().rposition(|e| !e.fav) {
                let e = self.entries.remove(pos);
                self.thumbs.remove(&e.id);
                // 淘汰条目的盘上文件一并清：`full`（超限文本全文 / 全尺寸原件）**和** `thumb`
                // （列表缩略图）——两者必须一起动手。以前这里只清 `full`，而单个图片文件
                // 的条目**只有 `thumb` 没有 `full`**，不补这一刀，被淘汰的那种预览图会
                // 永久堆在 `thumbs/` 里（`clear`/`delete_entry` 两条路径早就两个都清了）。
                if let Some(rel) = &e.full {
                    let _ = std::fs::remove_file(from_app_relative(rel));
                }
                if let Some(rel) = &e.html {
                    let _ = std::fs::remove_file(from_app_relative(rel));
                }
                if let Some(rel) = &e.thumb {
                    let _ = std::fs::remove_file(from_app_relative(rel));
                }
            } else {
                break; // 全是收藏，不再淘汰。
            }
        }
    }

    /// 单个图片文件：投递一张列表缩略图的补做任务（worker 解好后回填，**不动条目顺序**）。
    ///
    /// 为什么只管"单个"文件：`files` 是一次拷贝里的一批路径，多于一张时画哪张都会误导
    /// （看着是那张图，粘出去却是一批）。多文件仍走原来的矢量图标，第二行文件名说得清楚。
    fn request_file_thumb(&mut self, files: &[String]) {
        if files.len() != 1 || !is_image_ext(&files[0]) {
            return;
        }
        let Some(e) = self
            .entries
            .iter()
            .find(|e| e.kind == Kind::File && e.files == files)
        else {
            return;
        };
        // 已经有预览（`thumb` 有值 = 这张预览图已经落盘了）或正在跑，都跳过——
        // 同一个文件反复复制不该反复解码。解不出来的情况（缺 WebP 解码器之类）
        // 会保持 `None`，下次再复制时才重试。
        if e.thumb.is_some() || self.in_flight.contains(&e.id) {
            return;
        }
        self.in_flight.insert(e.id);
        let _ = self.job_tx.send(CapJob::FileThumb {
            id: e.id,
            path: std::path::PathBuf::from(&files[0]),
            dir: self.thumbs_dir.clone(),
        });
    }

    // ── 捕获 ──
    fn capture(&mut self) {
        if !self.cfg.enabled {
            return;
        }
        unsafe {
            // 自写屏蔽：见到命名格式即跳过（这是我们粘贴写回触发的 WM_CLIPBOARDUPDATE）。
            if self.self_fmt != 0 && IsClipboardFormatAvailable(self.self_fmt).is_ok() {
                return;
            }
            // ★ 源程序 opt-out：密码管理器等复制时放"请勿记录我"标记，尊重之。
            //   与自写屏蔽同一待遇——直接 return，不进重试判定（不是"读不到"，是"不要"）。
            if optout_requested(&self.optout_fmts, &|f| {
                IsClipboardFormatAvailable(f).is_ok()
            }) {
                return;
            }
        }
        let (handled, wanted) = self.capture_inner();
        let (retry, again) = retry_decision(handled, wanted, self.clip_retry);
        self.clip_retry = retry;
        if again {
            unsafe {
                // ★ 重试也走 SetTimer：这条消息泵线程不能睡（卡住面板 = 悬停转圈老路）。
                SetTimer(Some(self.panel), RETRY_TIMER, CAP_RETRY_MS, None);
            }
        }
    }

    /// 实际读取一次。返回 `(handled, wanted)`：
    /// `handled` = 这次内容已入账/已投递/本来就没东西可存；`wanted` = 剪贴板上**有**
    /// 我们认识的格式。两者都 false 才值得重试——"有格式却读不到"（多半是别的
    /// 监听程序正握着剪贴板）才是要救的场景。
    fn capture_inner(&mut self) -> (bool, bool) {
        let mut wanted = false;
        unsafe {
            if IsClipboardFormatAvailable(CF_HDROP_U).is_ok() {
                wanted = true;
                if let Some(files) = read_files_opened() {
                    let files = if files.is_empty() {
                        return (true, true);
                    } else {
                        files
                    };
                    // ★ 条目**先照旧入账**，预览图交给 worker 补：解码一张上千像素的图片
                    //   不该在消息泵线程上跑（2026-09-29 用户报"悬停转圈好几秒"就是这么来的）。
                    //   这样即使 worker 慢，列表也已经有一条完整的记录可用。
                    self.push(
                        Kind::File,
                        String::new(),
                        files.clone(),
                        None,
                        0,
                        0,
                        None,
                        None,
                        None,
                    );
                    self.request_file_thumb(&files);
                    return (true, true);
                }
            }
            if IsClipboardFormatAvailable(8).is_ok() {
                // ★ 8 = CF_DIB。重活（DIB→RGBA 解码 + 缩略 + PNG 编码 + 写盘）全在
                //   worker 线程（`capture_image_bg`），面板线程只投递——消息泵不停转，
                //   悬停不再转圈（2026-09-29 用户报）。入账在 `run` 消息循环收到
                //   `CapMsg::Captured` 后走同一个 `push`（哈希去重语义不变）。
                let _ = self.job_tx.send(CapJob::Capture {
                    self_fmt: self.self_fmt,
                    dir: self.thumbs_dir.clone(),
                    full_dir: Some(self.images_dir.clone()),
                });
                return (true, true);
            }
            if IsClipboardFormatAvailable(13).is_ok() {
                wanted = true;
                if let Some(text) = get_text() {
                    if text.is_empty() {
                        return (true, true);
                    }
                    let kind = if is_url(&text) {
                        Kind::Link
                    } else {
                        Kind::Text
                    };
                    // ★ 超限大文本落盘（2026-09-29 用户口径）：全文写到
                    //   `texts/<内容哈希>.txt`，条目只留 `TEXT_PREVIEW_BYTES` 预览
                    //   —— 内存里不再抱着一整篇大文本。旧实现直接截断到 100KB，
                    //   全文后半段无声丢失，还埋着 `&text[..n]` 撞字符边界的 panic。
                    let (body, full) = if text.len() > MAX_TEXT_BYTES {
                        let name =
                            format!("{:016x}.txt", compute_sig(kind, &text, &[], 0, 0, None));
                        let path = self.texts_dir.join(&name);
                        match std::fs::write(&path, &text) {
                            Ok(_) => (
                                format!(
                                    "{}\n…（全文已存文件，共 {} 字节）",
                                    safe_prefix(&text, TEXT_PREVIEW_BYTES),
                                    text.len()
                                ),
                                Some(to_app_relative(&path)),
                            ),
                            Err(_) => (safe_prefix(&text, MAX_TEXT_BYTES).to_string(), None),
                        }
                    } else {
                        (text, None)
                    };
                    // ★ 富文本保真（CF_HTML）：浏览器/Office 复制时剪贴板上同时有
                    //   "HTML Format"，原样字节落盘（零解析零失真，见 `clipboard_html`
                    //   模块头）。纯文本源（记事本）没有这个格式，read() 即 None，零开销。
                    //   超限不存（粘贴回落纯文本）；落盘路径掺进条目哈希（见 `push`）。
                    let html = super::clipboard_html::read().filter(|b| {
                        super::clipboard_html::plausible(b)
                            && b.len() <= super::clipboard_html::MAX_HTML_BYTES
                    });
                    let html_rel = html.and_then(|blob| {
                        let name = format!("{:016x}.html", {
                            use std::hash::{Hash, Hasher};
                            let mut hs = DefaultHasher::new();
                            blob.hash(&mut hs);
                            hs.finish()
                        });
                        let path = self.texts_dir.join(&name);
                        std::fs::write(&path, &blob)
                            .ok()
                            .map(|_| to_app_relative(&path))
                    });
                    self.push(kind, body, Vec::new(), None, 0, 0, None, full, html_rel);
                    return (true, true);
                }
            }
        }
        (false, wanted)
    }

    // ── 配置 ──
    fn apply_config(&mut self, cfg: ClipCfg) {
        let max_changed = cfg.max_entries != self.cfg.max_entries;
        let was_enabled = self.cfg.enabled;
        // ★ 面板窗口记忆：只在**首次**送达（`self.cfg.panel` 还是空）时采纳。之后不再
        //   回灌——面板线程自己的（更新的）置顶/位置态不允许被一次迟到的 set_config
        //   覆盖回旧值；运行时侧的最新值走 pending 槽位（见 `take_pending_panel_state`）。
        if self.cfg.panel.is_none() {
            if let Some(g) = cfg.panel {
                self.pos_saved = Some((g.x, g.y));
                self.pinned = g.pinned;
            }
        }
        self.cfg = cfg;
        // ★ 从关到开的瞬间补采当前系统剪贴板一条（与 `show_panel` 同一动机：
        //   模块关着期间复制的内容，开启时把"当前这一条"接进来）。
        if cfg.enabled && !was_enabled {
            self.capture();
        }
        if max_changed {
            self.evict();
        }
        self.persist();
        self.refresh_panel();
    }

    fn clear(&mut self) {
        // ★ 收藏（★）不被清空（2026-09-29 用户口径）：收藏是用户主动标记的长期保留
        //   内容，"清空"清的是临时复制流。收藏条目与其缩略图、全文文件原样保留。
        let fav_ids: std::collections::HashSet<u64> = self
            .entries
            .iter()
            .filter(|e| e.fav)
            .map(|e| e.id)
            .collect();
        // ★ 保留集合按**文件名**判定，而不是从文件名倒推条目 id：缩略图与原件都按**内容
        //   哈希**命名（`"images/{hash}.png"` / `"thumbs/{hash}.png"`），十六进制名里只要
        //   出现 `a–f`，`file_stem().parse::<u64>()` 就失败，旧实现据此判"不保留"，于是
        //   连收藏条目的图也被删掉——而它旁边那条注释偏偏写着"收藏的原样保留"。
        //   同名即同图：`thumb` 与 `full` 共用同一个哈希，因此一个文件名对这对文件同时生效。
        let keep: std::collections::HashSet<String> = self
            .entries
            .iter()
            .filter(|e| fav_ids.contains(&e.id))
            .flat_map(|e| [e.thumb.as_ref(), e.full.as_ref()].into_iter().flatten())
            .flat_map(|rel| {
                std::path::Path::new(rel)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .collect();
        self.entries.retain(|e| e.fav);
        self.thumbs.retain(|id, _| fav_ids.contains(id));
        // 三个目录一起理：`thumbs/` `texts/` `images/`。旧逻辑只扫 `thumbs/`，非收藏条目
        //   一被清空，它的全文文件就永久留在盘上；新增的 `images/` 也会重蹈这处泄漏。
        //   顺带把"条目已不在、文件还留在盘上"的孤儿一并清掉。
        for d in [&self.thumbs_dir, &self.texts_dir, &self.images_dir] {
            if let Ok(rd) = std::fs::read_dir(d) {
                for p in rd.flatten() {
                    let name = p.file_name().to_string_lossy().into_owned();
                    if !keep.contains(&name) {
                        let _ = std::fs::remove_file(p.path());
                    }
                }
            }
        }
        self.persist();
        self.selected = 0;
        self.scroll = 0;
        self.refresh_panel();
    }

    // ── 面板显隐 ──
    fn save_panel_state(&mut self) {
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(self.panel, &mut r).is_ok() {
                self.pos_saved = Some((r.left, r.top));
            }
        }
        let (x, y) = match self.pos_saved {
            Some((x, y)) => (x, y),
            None => return,
        };
        // ★ 窗口记忆并入 config.json（2026-09-29 用户口径）：不再写 panel.json，改投
        //   全局槽位，由帧循环取走并 `Config::save()`——config.json 保持单写者。
        //   代价：面板挪动/置顶后若程序一直 park（零帧）又直接退出，这次记忆会丢
        //   （下次回退到更早的位置）——概率与代价都可接受，换来配置单文件。
        queue_panel_state(crate::core::config::ClipPanelGeom {
            x,
            y,
            pinned: self.pinned,
        });
    }

    fn show_panel(&mut self) {
        // ★ 打开面板时补采当前系统剪贴板（2026-09-29 用户需求）。系统剪贴板只有
        //   "当前一条"可读（没有可回溯的历史），所以能补的是开启瞬间的这一条：
        //   内容与最新条目相同则只是上浮刷新，不同则补录——面板关着期间复制的内容
        //   由此进入历史。`capture()` 自带自写屏蔽，不会把我们自己刚才的粘贴当新捕获。
        // ★ 2026-09-29 修复「打开即转圈」：不在 show_panel 里同步跑 capture()（会
        //   同步解码当前剪贴板图片、把面板线程卡住），改投 `CMD_CAPTURE` 回消息循环
        //   异步执行——面板先即时显隐，capture 随后在消息泵里跑，光标不再转圈。
        if let Some(a) = CMD_HWND.get() {
            let h = HWND(a.load(Ordering::Relaxed) as *mut c_void);
            let _ =
                unsafe { PostMessageW(Some(h), WM_APP_CLIP_CMD, WPARAM(CMD_CAPTURE), LPARAM(0)) };
        }
        self.query.clear();
        unsafe {
            if !self.edit.is_invalid() {
                let _ = SetWindowTextW(self.edit, PCWSTR(wide("").as_ptr()));
            }
        }
        self.selected = 0;
        self.scroll = 0;
        self.hover = None;
        // ★ 收起态高度**必须**参与位置与夹取（旧版夹取写死 `PANEL_H`，收起态一打开
        //   就会被往上顶一截——收起时高度只有 `COLLAPSED_H`）。
        let h = if self.collapsed { COLLAPSED_H } else { PANEL_H };
        // 位置记忆：优先用上次的（[`Self::save_panel_state`] 落进 `panel.json`）；
        //   没有（首次 / 配置被删）→ **居中**（用户口径 2026-09-27，旧版是跟着光标 +12，
        //   手不在屏幕上时开出来完全找不到窗口）。
        let (mut x, mut y) = match self.pos_saved {
            Some((x, y)) => (x, y),
            None => {
                let wr = work_rect_primary();
                (
                    wr.left + (wr.right - wr.left - PANEL_W) / 2,
                    wr.top + (wr.bottom - wr.top - h) / 2,
                )
            }
        };
        let wr = work_rect_at(x.max(0), y.max(0));
        if x + PANEL_W > wr.right {
            x = wr.right - PANEL_W;
        }
        if y + h > wr.bottom {
            y = wr.bottom - h;
        }
        x = x.clamp(wr.left, (wr.right - PANEL_W).max(wr.left));
        y = y.clamp(wr.top, (wr.bottom - h).max(wr.top));
        unsafe {
            self.last_foreground = GetForegroundWindow();
            // ★ 顺路保存目标线程的焦点控件（粘贴时恢复光标，见 do_paste）。
            self.saved_focus = thread_focus_of(self.last_foreground);
            let ok = SetWindowPos(
                self.panel,
                Some(HWND_TOPMOST),
                x,
                y,
                PANEL_W,
                h,
                SWP_SHOWWINDOW,
            );
            log::info!(
                "剪贴板: show_panel pos=({x},{y}) setpos_ok={} visible={}",
                ok.is_ok(),
                IsWindowVisible(self.panel).as_bool()
            );
            ShowWindow(self.panel, SW_SHOW);
            // ★ 首开激活兜底（2026-10-07 晚定案）：SetForegroundWindow 在「explorer
            //   启动 + 自提权重启」链路上首次调用会被前台锁**静默拒绝**（F4 热键不算
            //   "用户刚给过输入"），面板悬在未激活态——系统把它当"还在启动的进程"，
            //   鼠标悬停在面板上就一直显示启动转圈（实测悬停 30s+；点一下标题栏真激活
            //   即消失）。这里照 do_paste 的同款手法：切不动就注入一次无害 Alt 抬落
            //   （向系统证明"用户刚操作过"，换取前台切换许可）再切一次，并落日志。
            let _ = SetForegroundWindow(self.panel);
            if GetForegroundWindow() != self.panel {
                send_alt_tap();
                let _ = SetForegroundWindow(self.panel);
            }
            log::info!(
                "剪贴板: show_panel 激活 ok={}",
                GetForegroundWindow() == self.panel
            );
            // ★ 清除「应用启动中」锁存：激活（SetForegroundWindow）不解锁，只有真实
            //   输入事件才解锁（用户手点标题栏即消圈实证）。每进程只做一次。
            static LATCH_CLEAR: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !LATCH_CLEAR.swap(true, std::sync::atomic::Ordering::Relaxed) {
                clear_appstart_latch(self.panel);
            }
            if !self.edit.is_invalid() {
                let _ = SetFocus(Some(self.edit));
            }
            // ★ 粘贴目标轮询：面板活着期间每 500ms 刷新一次目标（见 [`TARGET_POLL_TIMER`]）。
            SetTimer(Some(self.panel), TARGET_POLL_TIMER, TARGET_POLL_MS, None);
        }
        self.refresh_panel();
    }

    fn hide_panel(&mut self) {
        self.save_panel_state();
        self.hide_preview();
        unsafe {
            ShowWindow(self.panel, SW_HIDE);
            KillTimer(Some(self.panel), SEARCH_TIMER);
            KillTimer(Some(self.panel), HOVER_TIMER);
            KillTimer(Some(self.panel), PASTE_TIMER);
            KillTimer(Some(self.panel), RETRY_TIMER);
            KillTimer(Some(self.panel), TARGET_POLL_TIMER);
            KillTimer(Some(self.panel), PASTE_SEND_TIMER);
        }
    }

    fn toggle(&mut self) {
        unsafe {
            if IsWindowVisible(self.panel).as_bool() {
                self.hide_panel();
            } else {
                self.show_panel();
            }
        }
    }

    /// 置顶切换：置顶后失焦不再自动隐藏（关闭/快捷键/Esc 仍可收起）。
    fn toggle_pinned(&mut self) {
        self.pinned = !self.pinned;
        self.save_panel_state();
        self.refresh_panel();
    }

    /// 收起/展开：收起 = 只剩顶栏（拖动 / 搜索 / 三个按钮仍在）。
    fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
        self.hide_preview();
        unsafe {
            let _ = SetWindowPos(
                self.panel,
                None,
                0,
                0,
                PANEL_W,
                if self.collapsed { COLLAPSED_H } else { PANEL_H },
                SWP_NOMOVE | SWP_NOZORDER,
            );
        }
        self.refresh_panel();
    }

    /// 删除条目（缩略图文件一并清理）。
    fn delete_entry(&mut self, gi: usize) {
        if gi >= self.entries.len() {
            return;
        }
        let e = self.entries.remove(gi);
        if let Some(shot) = self.thumbs.remove(&e.id) {
            drop(shot);
        }
        if let Some(rel) = &e.thumb {
            let _ = std::fs::remove_file(from_app_relative(rel));
        }
        if let Some(rel) = &e.full {
            let _ = std::fs::remove_file(from_app_relative(rel));
        }
        if let Some(rel) = &e.html {
            let _ = std::fs::remove_file(from_app_relative(rel));
        }
        self.hover = None;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
        self.persist();
        self.refresh_panel();
    }

    fn set_tab(&mut self, tab: TabKind) {
        self.tab = tab;
        self.selected = 0;
        self.scroll = 0;
        self.refresh_panel();
    }

    fn set_query(&mut self, q: String) {
        self.query = q;
        self.selected = 0;
        self.scroll = 0;
        self.refresh_panel();
    }

    fn move_sel(&mut self, delta: isize) {
        let vis = visible(&self.entries, self.tab, &self.query);
        if vis.is_empty() {
            return;
        }
        let cur = if self.selected >= vis.len() {
            0
        } else {
            self.selected
        };
        let nxt = (cur as isize + delta).clamp(0, (vis.len() as isize) - 1) as usize;
        self.selected = nxt;
        // 让选中行可见
        let rows = visible_rows() as usize;
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + rows {
            self.scroll = self.selected - rows + 1;
        }
        self.refresh_panel();
    }

    fn toggle_fav(&mut self, global_idx: usize) {
        if let Some(e) = self.entries.get_mut(global_idx) {
            e.fav = !e.fav;
            self.persist();
            self.refresh_panel();
        }
    }

    /// 这条图片条目该写回剪贴板的像素：**优先全尺寸原件**，取不到才退到显示用的缩略图。
    ///
    /// 次序不能反：`thumbs/` 里那份是 `thumbnail_of` 的降采样副本，用它写回等于把用户当初
    /// 截的分辨率永久抹掉——粘贴是整个模块里唯一"把像素交出去"的动作，必须给原件。
    /// 缺失原件有两种情况，都不是错误：这条是旧历史（落盘原件之前就已入库），或当初写盘
    /// 失过手；此时退回缩略图，至少还能粘出一张图，不至于整个粘贴动作失败。
    fn paste_shot(&self, entry: &ClipEntry) -> Option<Shot> {
        if let Some(rel) = &entry.full {
            if let Some(s) = shot_from_png(&from_app_relative(rel)) {
                return Some(s);
            }
        }
        self.thumbs.get(&entry.id).cloned()
    }

    fn paste_selected(&mut self) {
        let vis = visible(&self.entries, self.tab, &self.query);
        if self.selected >= vis.len() {
            return;
        }
        let idx = vis[self.selected];
        let entry = match self.entries.get(idx) {
            Some(e) => e.clone(),
            None => return,
        };
        match entry.kind {
            Kind::File => {
                // ★ 粘贴那一刻是权威校验：缓存可能已过期（U 盘刚插回来 / 刚被删）。
                //   只写存活的路径——往剪贴板里放一条明知指向不存在的 CF_HDROP，只会
                //   让目标程序弹一个更莫名其妙的"找不到项目"；全失效则干脆不写、不收
                //   面板（收了用户就看不到行上的标记和告知了），让顶栏告知条说明原因。
                let survivors: Vec<String> = entry
                    .files
                    .iter()
                    .filter(|p| std::path::Path::new(p).exists())
                    .cloned()
                    .collect();
                if survivors.is_empty() {
                    if let Some(e) = self.entries.get_mut(idx) {
                        e.missing = true;
                    }
                    self.show_top_notice(TopNotice::SrcMissing);
                    return;
                }
                unsafe { write_files(&survivors) };
            }
            Kind::Image => {
                if let Some(shot) = self.paste_shot(&entry) {
                    let _ = to_clipboard(&shot);
                }
            }
            // ★ 超限大文本：条目里只有预览，粘贴时从 `texts/<hash>.txt` 读全文。
            //   富文本（CF_HTML）条目：同时把落盘的原样字节写回 "HTML Format" ——
            //   目标程序认 HTML 就粘富文本，不认就回落纯文本（双格式是保真前提）。
            _ => {
                let body = match &entry.full {
                    Some(rel) => std::fs::read_to_string(from_app_relative(rel))
                        .unwrap_or_else(|_| entry.text.clone()),
                    None => entry.text.clone(),
                };
                let html = entry
                    .html
                    .as_ref()
                    .and_then(|rel| std::fs::read(from_app_relative(rel)).ok());
                unsafe { super::clipboard_html::write_text_and_html(&body, html.as_deref()) };
            }
        }
        // ★ 面板处于「置顶」（pinned）时不自动收起（2026-09-29 用户口径）：置顶意味着
        //   "我要连续取用"，粘贴后消失是反的；收起只走手动关闭 / Esc / 快捷键。
        //   未置顶保持原行为：粘贴即收，不打断打字流。
        if !self.pinned {
            self.hide_panel();
        }
        unsafe {
            // 残留 Shift 先补 KEYUP（否则 Ctrl+V 变 Ctrl+Shift+V = 无格式粘贴）。
            if GetAsyncKeyState(VK_SHIFT_R as i32) < 0 {
                send_key(VK_SHIFT_R, true);
            }
            SetTimer(Some(self.panel), PASTE_TIMER, 150, None);
        }
    }

    fn do_paste(&self) {
        unsafe {
            let target = self.last_foreground;
            if !target.is_invalid() && IsWindow(Some(target)).as_bool() {
                // ── ① 前台切换 ──
                // 前台锁（2026-09-29 探针实证 temp/probe/do_paste_probe.py）：面板「置顶」
                // 后不再先隐藏，本进程窗口仍是前台，直接 `SetForegroundWindow(目标)` 会被
                // 系统**静默拒绝**（返回成功但 `GetForegroundWindow` 不变）。解法：先直切；
                // 真没切成，注入一次无害的 Alt 按下+抬起（向系统证明"用户刚操作过"，
                // 换取本次前台切换许可）再切一次。老流程靠"粘贴即收"先隐藏面板、系统自动
                // 归还前台才侥幸没暴露。
                if GetForegroundWindow() != target {
                    // ★ 取证：记下是谁在「写回剪贴板 → do_paste」这 ~170ms 里抢了前台
                    //   （嫌疑：TIM 自己的剪贴板助手气泡 / 我们面板被激活）。
                    let thief = GetForegroundWindow();
                    let mut cls = [0u16; 64];
                    let cls_len = GetClassNameW(thief, &mut cls);
                    let cls = String::from_utf16_lossy(&cls[..cls_len as usize]);
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(thief, Some(&mut pid));
                    let mine = pid == GetCurrentProcessId();
                    log::info!(
                        "剪贴板: do_paste 抢前台者 hwnd=0x{:x} class={cls:?} pid={pid} 本进程={mine}",
                        thief.0 as isize
                    );
                    let _ = SetForegroundWindow(target);
                    if GetForegroundWindow() != target {
                        send_alt_tap();
                        let _ = SetForegroundWindow(target);
                    }
                    log::info!(
                        "剪贴板: do_paste 前台切换 ok={}",
                        GetForegroundWindow() == target
                    );
                }
                // ★ 切完前台**就停**：QQ NT（Electron/Qt 系）这类应用要等自己消化完
                //   WM_ACTIVATE、把内部焦点挂回输入框，键才落得进去（2026-10-07 日志实证：
                //   切前台 ok、立刻 SetFocus+SendInput 相差 1ms ⇒ QQ 落空且报错误音；
                //   textus 同场景只 sleep 50ms 不碰焦点 ⇒ 通吃）。发键走
                //   [`PASTE_SEND_TIMER`] 二段（见 `do_paste_send`），绝不在消息泵上 sleep。
                SetTimer(
                    Some(self.panel),
                    PASTE_SEND_TIMER,
                    PASTE_SEND_DELAY_MS,
                    None,
                );
                return;
            }
            // 目标已失效：键发给当前前台碰运气（与旧版一致）。
            send_ctrl_v();
        }
    }

    /// 粘贴二段（`PASTE_SEND_TIMER` 到点）：目标应用已消化完激活，现在恢复焦点（仅
    /// 必要时）并发键。焦点恢复从"无条件执行"降级为**兜底**——目标自己会把焦点挂回
    /// 原控件（系统激活线程时本就归还焦点记忆），外部 `SetFocus` 只在它真没挂回去时
    /// 才插手，且尽量选 `saved_focus`（轮询 500ms 刷新的用户最后焦点控件）。
    fn do_paste_send(&mut self) {
        unsafe {
            let target = self.last_foreground;
            let mut fg = GetForegroundWindow();
            if !target.is_invalid() && IsWindow(Some(target)).as_bool() && fg != target {
                // 一段切换被系统回吐（60ms 内前台又变了）：再争取一次，真不行才认命。
                let _ = SetForegroundWindow(target);
                if GetForegroundWindow() != target {
                    send_alt_tap();
                    let _ = SetForegroundWindow(target);
                }
                fg = GetForegroundWindow();
            }
            if target.is_invalid() || !IsWindow(Some(target)).as_bool() || fg != target {
                log::info!("剪贴板: do_paste_send 目标失效或前台漂移（fg≠target），直接发键");
                send_ctrl_v();
                return;
            }
            // ── 发键：完全照抄 textus（src/paste.rs simulate_paste）──
            // 2026-10-07 QQ 实测结论：60ms 后 QQ 的控件焦点依然不在 Win32 意义的
            // "目标窗内"（CEF/Qt 内部焦点模型，GetGUIThreadInfo 看不懂），我们的
            // AttachThreadInput+SetFocus 手术救不了它、WM_PASTE 直发更摸不到它；
            // textus 对 QQ 通吃的全部秘密 = 什么都不做，只逐键发 Ctrl+V。
            // 发键挪到独立线程（内部有 10ms 间隔 sleep，不占面板消息泵）。
            log::info!(
                "剪贴板: do_paste_send 派发 Ctrl+V（fg=0x{:x}，发键前等前台稳定）",
                fg.0 as isize
            );
            // ★ HWND 是裸指针包装不带 Send，跨线程只传 isize，线程内再还原。
            let target_raw = target.0 as isize;
            std::thread::spawn(move || {
                // ★ 发键前等「前台==目标且连续稳定」：写回剪贴板后 TIM 的剪贴板助手
                //   气泡会弹出抢前台/焦点（2026-10-07 驱动实验实证），固定 60ms 是赌
                //   运气。轮询至 fg==target 连续稳定 PASTE_STABLE_HOLD_MS 且总等待≥
                //   PASTE_STABLE_MIN_MS 再发；期间被抢就切回（80ms 节流）；
                //   PASTE_SEND_DEADLINE_MS 兜底，超时按现状直接发。
                let target = HWND(target_raw as *mut core::ffi::c_void);
                let start = std::time::Instant::now();
                let deadline =
                    start + std::time::Duration::from_millis(PASTE_SEND_DEADLINE_MS as u64);
                let min_wait = std::time::Duration::from_millis(PASTE_STABLE_MIN_MS as u64);
                let hold_need = std::time::Duration::from_millis(PASTE_STABLE_HOLD_MS as u64);
                let mut hold_since: Option<std::time::Instant> = None;
                let mut last_switch = start - std::time::Duration::from_secs(1);
                let mut switches = 0usize;
                loop {
                    let fg = GetForegroundWindow();
                    if !target.is_invalid() && fg == target {
                        match hold_since {
                            Some(t) if t.elapsed() >= hold_need && start.elapsed() >= min_wait => {
                                break;
                            }
                            Some(_) => {}
                            None => hold_since = Some(std::time::Instant::now()),
                        }
                    } else {
                        hold_since = None;
                        if std::time::Instant::now() >= deadline {
                            break;
                        }
                        if std::time::Instant::now().duration_since(last_switch)
                            >= std::time::Duration::from_millis(80)
                        {
                            last_switch = std::time::Instant::now();
                            switches += 1;
                            if IsWindow(Some(target)).as_bool() {
                                let _ = SetForegroundWindow(target);
                                if GetForegroundWindow() != target {
                                    send_alt_tap();
                                    let _ = SetForegroundWindow(target);
                                }
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                log::info!(
                    "剪贴板: 粘贴发键：等前台稳定 {}ms（重切 {} 次）",
                    start.elapsed().as_millis(),
                    switches
                );
                send_paste_keys_textus();
            });
        }
    }

    /// 把某条**重新写回剪贴板**（= 成为剪贴板第一条），不粘贴、不收面板。
    ///
    /// 2026-09-28 用户要求：每条记录多一颗"↑"按钮 —— 点了之后这条内容重新成为剪贴板
    /// 当前内容，但不动光标焦点、不往任何窗口里粘贴。
    ///
    /// 写回走 `write_text` / `write_files` / `to_clipboard`，它们都会打自写屏蔽
    /// （`set_self_shield`）→ `capture()` 见到屏蔽格式会跳过，**不会**把这次写回当成
    /// 新捕获。所以"上浮到第一位"必须在这里亲手做（与 `push` 的去重上浮同一套动作：
    /// 摘出来 → 刷新时间戳 → 插回下标 0 → 落盘 + 刷新面板）。
    fn recopy_entry(&mut self, gi: usize) {
        let entry = match self.entries.get(gi) {
            Some(e) => e.clone(),
            None => return,
        };
        log::info!(
            "剪贴板: recopy 开始 gi={gi} kind={}",
            match entry.kind {
                Kind::Text => "Text",
                Kind::Link => "Link",
                Kind::Image => "Image",
                Kind::File => "File",
            }
        );
        match entry.kind {
            Kind::File => {
                // ★ 同 paste_selected：全失效就不写回（不往剪贴板放坏引用），只标行 +
                //   顶栏告知；有存活的则照常，但只写存活的那部分。
                let survivors: Vec<String> = entry
                    .files
                    .iter()
                    .filter(|p| std::path::Path::new(p).exists())
                    .cloned()
                    .collect();
                if survivors.is_empty() {
                    if let Some(e) = self.entries.get_mut(gi) {
                        e.missing = true;
                    }
                    self.show_top_notice(TopNotice::SrcMissing);
                    return;
                }
                unsafe { write_files(&survivors) };
            }
            Kind::Image => {
                if let Some(shot) = self.paste_shot(&entry) {
                    log::info!(
                        "剪贴板: recopy 写回图片 {}×{}（像素 {} 字节，声明需要 {}）",
                        shot.w,
                        shot.h,
                        shot.bgra.len(),
                        (shot.w as usize) * (shot.h as usize) * 4
                    );
                    if let Err(e) = crate::platform::screenshot::to_clipboard(&shot) {
                        log::warn!("剪贴板: recopy 写回图片失败：{e}");
                    } else {
                        // ★ 补一道"这是我们自己写回的"标记：不然监听端会把自己刚写的
                        //   那张 DIB 又读回来、重算签名、再存一份 PNG —— 白跑一趟，
                        //   而回读那段是全项目最吃 `unsafe` 指针的一处。
                        unsafe { shield_after_write() };
                    }
                }
            }
            // ★ 超限大文本：从全文文件读回（同 `paste_selected`）。
            _ => {
                let body = match &entry.full {
                    Some(rel) => std::fs::read_to_string(from_app_relative(rel))
                        .unwrap_or_else(|_| entry.text.clone()),
                    None => entry.text.clone(),
                };
                unsafe { write_text(&body) };
            }
        }
        log::info!("剪贴板: recopy 写回完成");
        if let Some(pos) = self.entries.iter().position(|e| e.id == entry.id) {
            let mut e = self.entries.remove(pos);
            e.ts = now_secs();
            self.entries.insert(0, e);
            self.persist();
            self.refresh_panel();
        }
    }

    // ── 滚动（自绘滚动条）──
    /// 把"滑块顶端应落到的 y"换算成 `scroll`。
    fn drag_scroll_to(&mut self, thumb_top: i32) {
        let vis = visible(&self.entries, self.tab, &self.query);
        let rows = visible_rows() as usize;
        let top = max_scroll(vis.len(), rows);
        if top == 0 {
            self.scroll = 0;
            return;
        }
        let t = scroll_track();
        let track_h = t.bottom - t.top;
        let (_ty, th) = scroll_thumb(vis.len(), rows, self.scroll);
        let span = (track_h - th).max(1);
        let dy = (thumb_top - t.top).clamp(0, span);
        self.scroll = ((dy as f64 * top as f64 / span as f64) as usize).min(top);
        self.refresh_panel();
    }

    /// 滚动条轨道被按下：滑块上方 / 下方 = 翻页，压在滑块上 = 开始拖动。
    /// 返回 `true` 表示这次点击已被滚动条吃掉（调用方别再当行点击处理）。
    fn on_scroll_down(&mut self, y: i32) -> bool {
        let vis = visible(&self.entries, self.tab, &self.query);
        let rows = visible_rows() as usize;
        let top = max_scroll(vis.len(), rows);
        if top == 0 {
            return true;
        }
        let (ty, th) = scroll_thumb(vis.len(), rows, self.scroll);
        if y < ty {
            self.scroll = self.scroll.saturating_sub(rows);
            self.refresh_panel();
        } else if y > ty + th {
            self.scroll = (self.scroll + rows).min(top);
            self.refresh_panel();
        } else {
            // 抓滑块：记"按下处 − 滑块顶端"，拖动全程保持这个相对位置（不跳位）。
            self.scroll_drag = Some(y - ty);
            unsafe {
                SetCapture(self.panel);
            }
        }
        true
    }

    fn on_lbutton_up(&mut self) {
        if self.scroll_drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
    }

    // ── 预览 ──
    fn on_mouse_move(&mut self, lparam: LPARAM) {
        let x = lparam_x(lparam);
        let y = lparam_y(lparam);
        // 正在拖滚动条：只跟滑块走，不做 hover / 预览。
        if let Some(grab) = self.scroll_drag {
            self.drag_scroll_to(y - grab);
            return;
        }
        // ── 顶栏按钮悬停（用户第 2 点）：命中就记序号，底纹由 `draw_panel` 照便签画。
        let btn = top_btn_at(x, y);
        if btn != self.hover_btn {
            self.hover_btn = btn;
            self.invalidate_top();
        }
        // ── 列表行悬停：先算"应该是谁"，再跟旧值比一次，只重绘真正变了的那块。
        let mut next: Option<usize> = None;
        let mut next_act: Option<(usize, u8)> = None;
        if (GAP..=list_right()).contains(&x) && (LIST_TOP..=PANEL_H - GAP).contains(&y) {
            let row = (y - LIST_TOP) / ITEM_H;
            let vi = self.scroll as i32 + row;
            let vis = visible(&self.entries, self.tab, &self.query);
            if row < visible_rows() && vi >= 0 && (vi as usize) < vis.len() {
                let gi = vis[vi as usize];
                next = Some(gi);
                // 行内三颗动作按钮（与 WM_LBUTTONDOWN / draw 三处同一份几何）：
                // 自右往左 ★ / 🗑 / ↑，命中哪颗记哪颗。
                let r = list_right();
                let act = if x > r - GAP - ACT_W {
                    2_u8
                } else if x > r - GAP - 2 * ACT_W - GAP {
                    1_u8
                } else if x > r - GAP - 3 * ACT_W - 2 * GAP {
                    0_u8
                } else {
                    u8::MAX // 行内但不在按钮上
                };
                if act != u8::MAX {
                    next_act = Some((gi, act));
                }
            }
        }
        if next != self.hover {
            self.hover = next;
            // ★ hover 变了必须自己触发重绘：`WM_MOUSEMOVE` 不会让窗口变脏，不刷新的话
            //   "鼠标划过变色"根本不会出现在屏幕上（旧版正是只有选中态能看到变化）。
            // ★ 另外旧版在"移出列表"的两个分支里只把 `hover` 清成 `None` 就 `return`，
            //   **没有重绘**——高亮会赖在屏幕上直到下一次别的重绘，也像在闪。
            self.invalidate_list();
        }
        if next_act != self.hover_act {
            self.hover_act = next_act;
            self.invalidate_list();
        }
        unsafe {
            // 重新计时 400ms
            KillTimer(Some(self.panel), HOVER_TIMER);
            SetTimer(Some(self.panel), HOVER_TIMER, 400, None);
        }
    }

    fn on_mouse_leave(&mut self) {
        let had = self.hover.take();
        let had_btn = self.hover_btn.take();
        let had_act = self.hover_act.take();
        // ★ 鼠标移出面板也要把"悬停高亮 / 按钮底纹"抹掉：`WM_MOUSELEAVE` 同样不会让窗口
        //   变脏，不重绘这两处高亮会一直留在屏上（看着就像随机闪了一下）。
        if had.is_some() {
            self.invalidate_list();
        }
        if had_btn.is_some() {
            self.invalidate_top();
        }
        if had_act.is_some() {
            self.invalidate_list();
        }
        unsafe {
            KillTimer(Some(self.panel), HOVER_TIMER);
        }
        self.hide_preview();
    }

    fn show_preview_for(&mut self, global_idx: usize) {
        let entry = match self.entries.get(global_idx) {
            Some(e) => e.clone(),
            None => return,
        };
        // 有缩略图就给预览（不论它是内存截图还是单个图片文件补出来的那张）。
        let shot = self.thumbs.get(&entry.id).cloned();
        self.preview_entry = Some(entry);
        self.preview_shot = shot;
        let (w, h) = self.preview_size();
        let (cx, cy) = unsafe { cursor_pos() };
        let wr = work_rect_at(cx, cy);
        // 默认放在光标右侧；越屏翻到左侧。
        let mut x = cx + 24;
        if x + w > wr.right {
            x = cx - 24 - w;
        }
        let mut y = cy;
        if y + h > wr.bottom {
            y = (wr.bottom - h).max(wr.top);
        }
        if y < wr.top {
            y = wr.top;
        }
        if x < wr.left {
            x = wr.left;
        }
        unsafe {
            let _ = SetWindowPos(
                self.preview,
                Some(HWND_TOPMOST),
                x,
                y,
                w,
                h,
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            ShowWindow(self.preview, SW_SHOWNA);
            let _ = InvalidateRect(Some(self.preview), None, true);
        }
    }

    fn hide_preview(&mut self) {
        unsafe {
            ShowWindow(self.preview, SW_HIDE);
        }
        self.preview_entry = None;
    }

    /// 预览窗尺寸（按内容计算）。
    fn preview_size(&self) -> (i32, i32) {
        const MAX_W: i32 = 520;
        const MAX_H: i32 = 360;
        match &self.preview_entry {
            Some(e) if e.kind == Kind::Image => {
                if let Some(shot) = &self.preview_shot {
                    let scale = (MAX_W as f32 / shot.w as f32)
                        .min(MAX_H as f32 / shot.h as f32)
                        .min(1.0);
                    let w = ((shot.w as f32 * scale) as i32).max(1);
                    let h = ((shot.h as f32 * scale) as i32).max(1);
                    (w, h)
                } else {
                    (200, 120)
                }
            }
            Some(e) if e.kind == Kind::File => {
                let n = e.files.len().min(8);
                let w = e
                    .files
                    .iter()
                    .map(|f| (f.chars().count() * 8).min(MAX_W as usize) as i32)
                    .max()
                    .unwrap_or(200)
                    .clamp(200, MAX_W);
                (w + 20, (n as i32) * 22 + 16)
            }
            Some(e) => {
                // 文本/链接：宽 420，按 9 行截断。
                const PW: i32 = 420;
                let text = match e.kind {
                    Kind::Link => e.text.clone(),
                    _ => e.text.clone(),
                };
                let lines: Vec<&str> = text.lines().collect();
                let shown: Vec<&str> = lines.iter().take(9).copied().collect();
                let h = (((shown.len() as i32) * 20) + 16).min(9 * 20 + 16);
                (PW + 20, h.max(40))
            }
            None => (10, 10),
        }
    }

    // ── 绘制 ──

    /// 画一个符号字形。★ 结尾 NUL 绝不能带进 `DrawTextW`（见 [`text16`]）。
    ///
    /// `tilted` = 绕按钮中心转 −45°：**已置顶的图钉钉尖朝下**。与便签 `platform::sticky`
    /// 标题栏完全同款——📌 字形天生斜躺（针头朝左下），转 −45° 后立正；旋转走世界变换
    /// 绕中心做（`DrawTextW` 仍按未旋转的矩形排版，用 `lfEscapement` 会让字形整体漂出
    /// 按钮）；转之前先上移 3px，因为立正姿态的视觉重心比排版中心偏下（便签真机实测）。
    unsafe fn draw_sym(&self, dc: HDC, r: RECT, ch: char, tilted: bool) {
        let mut buf = [0u16; 2];
        let n = ch.encode_utf16(&mut buf).len();
        let old = SelectObject(dc, HGDIOBJ(self.sym_font.0));
        let mut rr = r;
        if !tilted {
            let _ = DrawTextW(
                dc,
                &mut buf[..n],
                &mut rr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, old);
            return;
        }
        rr.top -= 3;
        rr.bottom -= 3;
        let f = std::f32::consts::FRAC_1_SQRT_2;
        let (fw, fh) = ((rr.right - rr.left) as f32, (rr.bottom - rr.top) as f32);
        let (mx, my) = (rr.left as f32 + fw / 2.0, rr.top as f32 + fh / 2.0);
        let xf = XFORM {
            eM11: f,
            eM12: -f,
            eM21: f,
            eM22: f,
            eDx: mx - f * mx - f * my,
            eDy: my + f * mx - f * my,
        };
        SetGraphicsMode(dc, GM_ADVANCED);
        SetWorldTransform(dc, &xf);
        let _ = DrawTextW(
            dc,
            &mut buf[..n],
            &mut rr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        let identity = XFORM {
            eM11: 1.0,
            eM12: 0.0,
            eM21: 0.0,
            eM22: 1.0,
            eDx: 0.0,
            eDy: 0.0,
        };
        SetWorldTransform(dc, &identity);
        SetGraphicsMode(dc, GM_COMPATIBLE);
        SelectObject(dc, old);
    }

    /// ★ 双缓冲：全部内容先拼进内存 DC，最后一次 `BitBlt` 上屏。
    ///
    /// 用户第 1 点"鼠标在界面移动经常闪烁"的另一半根因就在这儿——旧版是十几趟
    /// `FillRect` / `DrawTextW` **直接**落在窗口 DC 上，hover 一变就整窗重绘，这些中间
    /// 步骤肉眼是看得见的。内存里拼完再一次性贴，屏幕上永远只有"完整的一帧"。
    unsafe fn draw_panel(&self, hwnd: HWND) {
        let t0 = std::time::Instant::now();
        static FIRST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
        let first = FIRST.swap(false, std::sync::atomic::Ordering::Relaxed);
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let h = panel_h(self.collapsed);
        let mem = CreateCompatibleDC(Some(hdc));
        let bmp = CreateCompatibleBitmap(hdc, PANEL_W, h);
        let old_bmp = SelectObject(mem, HGDIOBJ(bmp.0));
        self.paint_into(mem);
        let _ = BitBlt(hdc, 0, 0, PANEL_W, h, Some(mem), 0, 0, SRCCOPY);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        EndPaint(hwnd, &ps);
        let ms = t0.elapsed().as_millis();
        if first || ms >= 30 {
            log::warn!("剪贴板: draw_panel 耗时 {ms}ms（first={first}，≥30ms 上报）");
        }
    }

    /// 面板内容（画进任意 DC——`draw_panel` 传的是内存 DC，所以这里不碰 `BeginPaint`）。
    unsafe fn paint_into(&self, hdc: HDC) {
        // 背景（整客户区铺满——分层窗未上色区 = 真透明，便签铁律）
        let h = panel_h(self.collapsed);
        let bg = RECT {
            left: 0,
            top: 0,
            right: PANEL_W,
            bottom: h,
        };
        FillRect(hdc, &bg, self.bg_brush);
        SetBkMode(hdc, TRANSPARENT);
        // ★★ 1px 外框（用户第 5 点）。
        //
        // 画在最前面而不是最后面：列表段会 `IntersectClipRect` 把裁剪区收进列表，
        // 收完之后再画就只剩列表那一块了。外框占的是最外一圈（0 与 W-1 / H-1），
        // 所有内容都从 `GAP` 起画，谁也盖不到它。
        //
        // 颜色用**深灰** `C_BORDER` 而不是纯黑：面板底是 240 的浅灰，纯黑 1px 在上面
        // 太跳（而且跟内部那条 `C_LINE` 分隔线抢戏）；深灰既把边界交代清楚，又比内部
        // 线更深一档，形成"外框 > 内线"的层级。
        FrameRect(hdc, &bg, self.border_brush);

        // ── 顶栏 ──
        // 左：放大镜拖动区（NCHITTEST → HTCAPTION，原生拖动）
        let drag = RECT {
            left: GAP,
            top: TOP_Y,
            right: GAP + DRAG_W,
            bottom: TOP_B,
        };
        SetTextColor(hdc, RGB(C_ICON.0, C_ICON.1, C_ICON.2));
        self.draw_sym(hdc, drag, '\u{1F50D}', false);
        // 右：置顶 📌 / 收起 ▼▲ / 关闭 ✕
        //
        // ★★ 三颗按钮**占满 `TOP_H` 整高**（跟便签标题栏同款）：按钮自己就是这一行的
        // 高度，于是"顶栏上下那一圈"只剩外面那个 `GAP`——跟条目之间的间隔一模一样
        // （用户第 3 点）。旧版是"按钮 28×28 在 32 高的顶栏里居中"，上下各留 2px
        // 内边距，跟外面 2px 的 GAP 叠成 4px，一眼就看出来顶栏更松。
        let btn = |i: i32| RECT {
            left: PANEL_W - GAP - (i + 1) * BTN_W,
            top: TOP_Y,
            right: PANEL_W - GAP - i * BTN_W,
            bottom: TOP_B,
        };
        for i in [2, 1, 0] {
            let r = btn(i);
            // ★ 悬停底纹（用户第 2 点）：便签是"底色朝前景提 18%"的深色版，浅色主题
            //   下反过来按下压 14%（`C_BTN_HOVER`），同一套 [`mix`] 换个方向而已。
            if self.hover_btn == Some(i) {
                FillRect(hdc, &r, self.btn_hover_brush);
            }
            let (ch, tilted, col) = match i {
                2 => (
                    '\u{1F4CC}',
                    self.pinned,
                    if self.pinned { C_ICON_ON } else { C_ICON_OFF },
                ),
                1 => (
                    if self.collapsed {
                        '\u{25B2}'
                    } else {
                        '\u{25BC}'
                    },
                    false,
                    C_ICON,
                ),
                _ => ('\u{2715}', false, C_ICON),
            };
            SetTextColor(hdc, RGB(col.0, col.1, col.2));
            self.draw_sym(hdc, r, ch, tilted);
        }
        // 中：条目计数（用户第 7 点——**不新增底部状态栏**，就填在搜索框右边这条空里。
        // 顶栏本来就有这条空白：搜索框到三颗按钮之间，把计数塞进去一行都不多占）
        //
        // ★★ 用户第 2 点：计数**水平居中**在"搜索框 → 三颗按钮"这整条里（旧版是
        // 右对齐贴着按钮边框）。数字变长时从中心往两头长，永远碰不到按钮。
        // `DT_END_ELLIPSIS` 只是兜底：正常量级放得下（见单测），真到"9999万+ / 9999万+"
        // 这种不可能出现的规模时优雅截断，也不会画到按钮上去。
        let total = visible(&self.entries, self.tab, "").len();
        let shown = visible(&self.entries, self.tab, &self.query).len();
        SelectObject(hdc, HGDIOBJ(self.font_sm.0));
        // ★ 存储告知条临时借用计数这条空位，到点自动还回去；不为此新增任何布局条——
        //   `LIST_TOP` 牵着滚动与命中测试十几处引用，为一条偶现的告知去动它不值得。
        let warn = self.top_notice.as_ref().map(notice_text);
        let col = if warn.is_some() {
            C_WARN_TEXT
        } else {
            C_TEXT_DIM
        };
        SetTextColor(hdc, RGB(col.0, col.1, col.2));
        let mut cnt = text16(&warn.unwrap_or_else(|| count_text(total, shown)));
        let mut cr = count_rect();
        let _ = DrawTextW(
            hdc,
            &mut cnt,
            &mut cr,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
        );

        if self.collapsed {
            // 收起态：只剩顶栏（搜索框 / 计数 / 三颗按钮照旧可用）。
            return;
        }

        // ── 分类页签 ──
        let tab_w = (PANEL_W - 2 * GAP) / TABS.len() as i32;
        SelectObject(hdc, HGDIOBJ(self.font_sm.0));
        for (i, (label, kind)) in TABS.iter().enumerate() {
            let mut r = RECT {
                left: GAP + i as i32 * tab_w,
                top: TAB_Y,
                right: GAP + (i as i32 + 1) * tab_w,
                bottom: TAB_Y + TAB_H,
            };
            let active = *kind == self.tab;
            FillRect(
                hdc,
                &r,
                if active {
                    self.sel_brush
                } else {
                    self.tab_brush
                },
            );
            let c = if active { C_TAB_ON_TEXT } else { C_TAB_TEXT };
            SetTextColor(hdc, RGB(c.0, c.1, c.2));
            let mut t = text16(label);
            let _ = DrawTextW(hdc, &mut t, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        }

        // ── 记录列表 ──
        let vis = visible(&self.entries, self.tab, &self.query);
        let rows = visible_rows();
        // 裁剪区带出滚动条那一列（它在列表右外侧，但属于同一条）。
        IntersectClipRect(hdc, GAP, LIST_TOP, PANEL_W - GAP, PANEL_H - GAP);

        for ri in 0..rows {
            let vi = self.scroll + ri as usize;
            if vi >= vis.len() {
                break;
            }
            let gi = vis[vi];
            let e = &self.entries[gi];
            let top = LIST_TOP + ri * ITEM_H;
            let row = RECT {
                left: GAP,
                top,
                right: list_right(),
                bottom: top + ROW_H,
            };
            let is_sel = self.selected == vi;
            let is_hov = self.hover == Some(gi);
            FillRect(
                hdc,
                &row,
                if is_sel {
                    self.sel_brush
                } else if is_hov {
                    self.hover_brush
                } else {
                    self.row_brush
                },
            );
            // 图标格：图片**直接画缩略图**（比任何矢量图标都直白，缩略图本来就在内存里），
            // 其余类型画矢量图标（见 [`draw_kind_icon`]）。
            let ix = row.left + 4;
            let iy = row.top + (ROW_H - ICON_W) / 2;
            let folder = entry_is_folder(e);
            // ★ 只看"这条有没有可用的缩略图"，不用 `Kind::Image` 当门：单个图片文件
            //   补做的预览挂在 `Kind::File` 下，按 kind 判的话它永远显示不出来。
            let drew_thumb = match self.thumbs.get(&e.id) {
                Some(shot) if shot.w > 0 && shot.h > 0 => {
                    // 等比缩进格子（短边留白），绝不拉伸变形。
                    let sc = (ICON_W as f32 / shot.w as f32).min(ICON_W as f32 / shot.h as f32);
                    let dw = ((shot.w as f32 * sc) as i32).clamp(1, ICON_W);
                    let dh = ((shot.h as f32 * sc) as i32).clamp(1, ICON_W);
                    draw_shot_to(
                        hdc,
                        ix + (ICON_W - dw) / 2,
                        iy + (ICON_W - dh) / 2,
                        dw,
                        dh,
                        shot,
                    );
                    true
                }
                _ => false,
            };
            if !drew_thumb {
                draw_kind_icon(
                    hdc,
                    ix,
                    iy,
                    e.kind,
                    folder,
                    kind_color(e.kind, folder),
                    row_bg(is_sel, is_hov),
                );
            }
            // 右侧三颗动作按钮（自右往左：★ 收藏 / 🗑 删除 / ↑ 复制置顶）
            let act_y = row.top + (ROW_H - ACT_W) / 2;
            let mut star = RECT {
                left: row.right - GAP - ACT_W,
                top: act_y,
                right: row.right - GAP,
                bottom: act_y + ACT_W,
            };
            let mut trash = RECT {
                left: star.left - GAP - ACT_W,
                top: act_y,
                right: star.left - GAP,
                bottom: act_y + ACT_W,
            };
            let recopy = RECT {
                left: trash.left - GAP - ACT_W,
                top: act_y,
                right: trash.left - GAP,
                bottom: act_y + ACT_W,
            };
            let text_r = recopy.left - 4;
            let text_l = row.left + 4 + ICON_W + 4;
            // 第一行：时间 · 体积（小字）
            SelectObject(hdc, HGDIOBJ(self.font_sm.0));
            SetTextColor(hdc, RGB(C_TEXT_DIM.0, C_TEXT_DIM.1, C_TEXT_DIM.2));
            let mut m = text16(&meta_line(e));
            let mut r1 = RECT {
                left: text_l,
                top: row.top + 2,
                right: text_r,
                bottom: row.top + 22,
            };
            let _ = DrawTextW(
                hdc,
                &mut m,
                &mut r1,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            // 第二行：内容 / 文件名（★ 就两行，没有"N 字符"那第三行）
            // ★ 源文件不在原处：文件名转暖橙——第一行（meta）已说明原因，这里只换色不加字，
            //   行高与布局一概不动。
            SelectObject(hdc, HGDIOBJ(self.font.0));
            if e.missing {
                SetTextColor(hdc, RGB(C_WARN_TEXT.0, C_WARN_TEXT.1, C_WARN_TEXT.2));
            } else {
                SetTextColor(hdc, RGB(C_TEXT.0, C_TEXT.1, C_TEXT.2));
            }
            let mut c = text16(&content_line(e));
            let mut r2 = RECT {
                left: text_l,
                top: row.top + 22,
                right: text_r,
                bottom: row.top + 44,
            };
            let _ = DrawTextW(
                hdc,
                &mut c,
                &mut r2,
                DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            // 删除 🗑 / 收藏 ★ / 复制置顶 📌
            // ★ 2026-09-28 用户两点：① 图标太淡看不清 → C_ACT 加深；② 悬停要有底纹。
            //    底纹 = 命中按钮先铺一层 `btn_hover_brush`（与顶栏按钮同一档混色）。
            // ③ 置顶就是**向上箭头**（用户要的图形），但要抗锯齿：手绘 GDI `Polygon`
            //   零抗锯齿（用户报"全是锯齿"），字体字形在 17px 上又太细。最终方案是
            //   4×4 超采样自绘箭头，见 [`draw_arrow_up`]。
            let hover_act = self.hover_act.filter(|(g, _)| *g == gi).map(|(_, a)| a);
            if hover_act == Some(0) {
                FillRect(hdc, &recopy, self.btn_hover_brush);
            }
            if hover_act == Some(1) {
                FillRect(hdc, &trash, self.btn_hover_brush);
            }
            if hover_act == Some(2) {
                FillRect(hdc, &star, self.btn_hover_brush);
            }
            SelectObject(hdc, HGDIOBJ(self.sym_font.0));
            SetTextColor(hdc, RGB(C_ACT.0, C_ACT.1, C_ACT.2));
            let mut tl = text16("\u{1F5D1}");
            let _ = DrawTextW(
                hdc,
                &mut tl,
                &mut trash,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            // ★ 复制置顶 = **向上箭头**（用户点名要的图形）。底色必须按"这颗按钮此刻
            //   有没有被悬停"取：悬停时铺的是 `btn_hover_brush` 那一档，箭头边缘要按
            //   **那一档**混，否则会看到一圈错色描边。
            let bg = if hover_act == Some(0) {
                mix(C_BG, C_TEXT, BTN_HOVER_MIX)
            } else if is_sel {
                C_SEL
            } else if is_hov {
                C_HOVER
            } else {
                C_ROW
            };
            draw_arrow_up(hdc, recopy, C_ACT, bg);
            let sc = if e.fav { C_STAR } else { C_STAR_OFF };
            SetTextColor(hdc, RGB(sc.0, sc.1, sc.2));
            let mut sl = text16("★");
            let _ = DrawTextW(
                hdc,
                &mut sl,
                &mut star,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }

        // ── 垂直滚动条 ──
        let t = scroll_track();
        FillRect(hdc, &t, self.track_brush);
        let (ty, th) = scroll_thumb(vis.len(), rows as usize, self.scroll);
        let thumb = RECT {
            left: t.left,
            top: ty,
            right: t.right,
            bottom: ty + th,
        };
        FillRect(
            hdc,
            &thumb,
            if self.scroll_drag.is_some() {
                self.thumb_drag_brush
            } else {
                self.thumb_brush
            },
        );
    }

    unsafe fn draw_preview(&self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let _ = SelectObject(hdc, self.font.into());
        let mut bg = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        GetClientRect(hwnd, &mut bg);
        FillRect(hdc, &bg, self.bg_brush);
        // 边框
        let pen = CreatePen(PS_SOLID, 1, RGB(C_LINE.0, C_LINE.1, C_LINE.2));
        let old = SelectObject(hdc, pen.into());
        Rectangle(hdc, 0, 0, bg.right - 1, bg.bottom - 1);
        SelectObject(hdc, old);
        let _ = DeleteObject(pen.into());

        let e = match &self.preview_entry {
            Some(e) => e,
            None => {
                EndPaint(hwnd, &ps);
                return;
            }
        };
        match e.kind {
            Kind::Image => {
                if let Some(shot) = &self.preview_shot {
                    draw_shot_to(hdc, 0, 0, bg.right, bg.bottom, shot);
                }
            }
            Kind::File => {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, RGB(C_TEXT.0, C_TEXT.1, C_TEXT.2));
                for (i, f) in e.files.iter().take(8).enumerate() {
                    let mut r = RECT {
                        left: 10,
                        top: 8 + i as i32 * 22,
                        right: bg.right - 10,
                        bottom: 8 + i as i32 * 22 + 20,
                    };
                    DrawTextW(
                        hdc,
                        &mut text16(f),
                        &mut r,
                        DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                    );
                }
            }
            _ => {
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, RGB(C_TEXT.0, C_TEXT.1, C_TEXT.2));
                let lines: Vec<&str> = e.text.lines().take(9).collect();
                let joined = lines.join("\n");
                let mut r = RECT {
                    left: 10,
                    top: 8,
                    right: bg.right - 10,
                    bottom: bg.bottom - 8,
                };
                DrawTextW(
                    hdc,
                    &mut text16(&joined),
                    &mut r,
                    DT_LEFT | DT_TOP | DT_WORDBREAK | DT_END_ELLIPSIS,
                );
            }
        }
        EndPaint(hwnd, &ps);
    }
}

// ── 行内类型图标（矢量手绘）───────────────────────────────────────────

/// ★ 2026-09-24 用户定案：整体翻成**浅灰底 + 暗色文字**（原深灰底 30/42/64/86 + 亮字
/// 被明确否掉）。档次一路**往下压**：底 240 → 行 250（比底还浅一档，卡片才浮得出来）
/// → 悬停 226 → 选中 200；文字 34 / 次要说明 104 全在暗端。
const C_BG: (u8, u8, u8) = (240, 240, 243);
const C_ROW: (u8, u8, u8) = (250, 250, 252);
const C_HOVER: (u8, u8, u8) = (226, 233, 245);
const C_SEL: (u8, u8, u8) = (200, 214, 238);
/// 正文 / 次要说明（都在暗端）。
const C_TEXT: (u8, u8, u8) = (34, 36, 42);
const C_TEXT_DIM: (u8, u8, u8) = (104, 110, 122);
/// 存储告知条的字色：比计数（[`C_TEXT_DIM`]）更醒目的暖橙，但**不用告警红**——它说的是
/// "你攒了这么多"，不是"出错了"；红色会把一次中性告知读成故障。
const C_WARN_TEXT: (u8, u8, u8) = (176, 106, 24);
/// 顶栏图标：**暗色**（用户第 5 点）。未置顶再淡一档，置顶走强调蓝。
const C_ICON: (u8, u8, u8) = (86, 92, 104);
const C_ICON_OFF: (u8, u8, u8) = (152, 158, 170);
const C_ICON_ON: (u8, u8, u8) = (22, 106, 196);
/// 页签：未激活底 / 未激活字 / 激活字（激活底复用 `C_SEL`）。
const C_TAB: (u8, u8, u8) = (232, 234, 239);
const C_TAB_TEXT: (u8, u8, u8) = (96, 102, 114);
const C_TAB_ON_TEXT: (u8, u8, u8) = (28, 30, 36);
/// 顶栏按钮悬停底纹朝文字色压多少（0..1）。
///
/// 浅色主题下必须**往下压**才看得见（便签在深色主题下是往上提 18%，同一个 `mix` 反过来
/// 用）。所以底纹色由 [`mix`] **现算**而不写死色号——以后翻主题或微调深浅只改这个数。
const BTN_HOVER_MIX: f32 = 0.14;
/// 滚动条：轨道 / 滑块 / 拖动中。
const C_TRACK: (u8, u8, u8) = (228, 231, 236);
const C_THUMB: (u8, u8, u8) = (170, 178, 192);
const C_THUMB_DRAG: (u8, u8, u8) = (130, 142, 162);
/// 搜索框（原生 EDIT）：白底 + 暗字，跟浅灰卡片拉开层次。
const C_EDIT_BG: (u8, u8, u8) = (255, 255, 255);
const C_EDIT_TEXT: (u8, u8, u8) = (30, 32, 38);
/// 分隔线 / 预览窗边框。
const C_LINE: (u8, u8, u8) = (206, 210, 218);
/// ★ 1px 外框（用户第 5 点）。比内部 `C_LINE` 深一档：外框交代"面板到这儿为止"，
/// 内线只负责分组，两者不该一样重。
const C_BORDER: (u8, u8, u8) = (110, 116, 128);
/// 行右侧三颗动作按钮：🗑 删除 / ★ 收藏（未收藏时压到几乎看不见，别跟正文抢）/ ↑ 复制置顶。
const C_ACT: (u8, u8, u8) = (92, 98, 110);
const C_STAR: (u8, u8, u8) = (232, 168, 24);
const C_STAR_OFF: (u8, u8, u8) = (162, 166, 176);

/// 线性插值（`t` = 朝 `b` 走多少，0..1）。悬停底纹照便签的做法：取底色与前景色之间
/// 的一档——主题翻转时不必重新挑色，改 `t` 就行。
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// 这一行的底色（图标折角要按它抹除）。
fn row_bg(is_sel: bool, is_hov: bool) -> COLORREF {
    let c = if is_sel {
        C_SEL
    } else if is_hov {
        C_HOVER
    } else {
        C_ROW
    };
    RGB(c.0, c.1, c.2)
}

/// 各类型的图标色（浅灰底上要压得住，所以比旧版整体调深）。
fn kind_color(kind: Kind, folder: bool) -> COLORREF {
    if folder {
        return RGB(198, 146, 34);
    }
    match kind {
        Kind::Text => RGB(108, 118, 134),
        Kind::Link => RGB(24, 112, 196),
        Kind::Image => RGB(36, 132, 84),
        Kind::File => RGB(176, 132, 44),
    }
}

unsafe fn fill_rect(dc: HDC, brush: HBRUSH, l: i32, t: i32, r: i32, b: i32) {
    let rc = RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    };
    FillRect(dc, &rc, brush);
}

/// 1px 描边矩形（GDI `FrameRect`：比四条 `FillRect` 省事，四角也不重叠加深）。
unsafe fn frame_rect(dc: HDC, brush: HBRUSH, l: i32, t: i32, r: i32, b: i32) {
    let rc = RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    };
    FrameRect(dc, &rc, brush);
}

/// 条目类型图标：在 `ICON_W` 见方的格子（左上角 `x,y`）里**手绘**，不碰任何符号字体。
///
/// ★ 为什么手绘而不是"找个现成图标"：① 旧版是 T/F/I/L 字母块，用户直接说"图标不太对"；
///   ② 换 Segoe UI Symbol 又有回退风险——顶栏字形刚因为把结尾 NUL 一起递给 `DrawTextW`
///   而多画出豆腐块（见 [`text16`]）；③ 矢量顺带把**颜色**也用上，四类一眼分开。
///
/// 几何纪律（与便签 `draw_icon` 同）：横平竖直的笔画一律 `FillRect` + 显式矩形（GDI 宽笔
/// 端点会向右/下多铺一像素，笔画两端不对称）；斜线只允许 45°，像素阶梯最干净。
unsafe fn draw_kind_icon(
    dc: HDC,
    x: i32,
    y: i32,
    kind: Kind,
    folder: bool,
    icon: COLORREF,
    rowbg: COLORREF,
) {
    let brush = CreateSolidBrush(icon);
    let pen = CreatePen(PS_SOLID, 1, icon);
    let old_pen = SelectObject(dc, HGDIOBJ(pen.0));
    let old_brush = SelectObject(dc, HGDIOBJ(brush.0));
    match kind {
        // 文本：一页纸 + 三行字
        Kind::Text => {
            frame_rect(dc, brush, x + 5, y + 2, x + 19, y + 22);
            fill_rect(dc, brush, x + 8, y + 7, x + 16, y + 9);
            fill_rect(dc, brush, x + 8, y + 11, x + 16, y + 13);
            fill_rect(dc, brush, x + 8, y + 15, x + 16, y + 17);
        }
        // 链接：地球仪（外圈 + 赤道 + 本初子午线）。
        //
        // ★ 旧版是"两个环 + 中间一横"的链环，24px 下两个小环挤成一团，用户直接说"太丑
        //   陋"。改成地球仪：三个椭圆**互为相切**——外圈 18px，赤道 rx=9/ry=2（在左右
        //   极点与外圈重合），子午线 rx=5/ry=9（在上下极点与外圈重合）。三个都画满格，
        //   不用裁剪区也不用算弦长，笔画端点天然落在圆上。
        Kind::Link => {
            let null_brush = HBRUSH(GetStockObject(NULL_BRUSH).0);
            let prev = SelectObject(dc, HGDIOBJ(null_brush.0));
            let _ = Ellipse(dc, x + 3, y + 3, x + 21, y + 21);
            let _ = Ellipse(dc, x + 3, y + 10, x + 21, y + 14);
            let _ = Ellipse(dc, x + 7, y + 3, x + 17, y + 21);
            SelectObject(dc, prev);
        }
        // 图片：画框 + 太阳 + 山（两条 45° 边）
        Kind::Image => {
            frame_rect(dc, brush, x + 3, y + 4, x + 21, y + 20);
            let _ = Ellipse(dc, x + 6, y + 7, x + 10, y + 11);
            let _ = MoveToEx(dc, x + 4, y + 18, None);
            let _ = LineTo(dc, x + 11, y + 11);
            let _ = LineTo(dc, x + 18, y + 18);
        }
        // 文件：一页纸带折角；整条都是目录时改画文件夹
        Kind::File => {
            if folder {
                fill_rect(dc, brush, x + 3, y + 6, x + 11, y + 10);
                frame_rect(dc, brush, x + 3, y + 10, x + 21, y + 21);
            } else {
                frame_rect(dc, brush, x + 5, y + 3, x + 19, y + 21);
                // 折角：先按行底色抹掉右上角那一小块，再补一条 45° 折线。
                let bg = CreateSolidBrush(rowbg);
                fill_rect(dc, bg, x + 14, y + 3, x + 19, y + 8);
                let _ = DeleteObject(HGDIOBJ(bg.0));
                let _ = MoveToEx(dc, x + 14, y + 3, None);
                let _ = LineTo(dc, x + 18, y + 7);
            }
        }
    }
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
    let _ = DeleteObject(HGDIOBJ(pen.0));
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

// ── 自由函数：剪贴板写回 ───────────────────────────────────────────────

/// ★ 模块外的纯文本复制入口（截屏取色 2026-10-02）。
///
/// 走同一条 [`write_text`]：`CF_TEXT` + 自写屏蔽（我们写回触发的剪贴板更新
/// 不会被自己的历史收录——一段色值进历史只是噪声）。
pub fn copy_text(s: &str) {
    unsafe { write_text(s) };
}

unsafe fn write_text(s: &str) {
    let utf16: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    let hg = match GlobalAlloc(GHND, (utf16.len() * 2) as usize) {
        Ok(h) => h,
        Err(_) => return,
    };
    let p = GlobalLock(hg);
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    std::ptr::copy_nonoverlapping(utf16.as_ptr(), p as *mut u16, utf16.len());
    GlobalUnlock(hg);
    if OpenClipboard(None).is_ok() {
        let _ = EmptyClipboard();
        SetClipboardData(CF_TEXT_U, Some(HANDLE(hg.0)));
        set_self_shield();
        let _ = CloseClipboard();
    } else {
        let _ = GlobalFree(Some(hg));
    }
}

unsafe fn write_files(files: &[String]) {
    // 构造 DROPFILES：pFiles=20, fWide=TRUE，后接若干以 NUL 结尾的宽字符串 + 双 NUL。
    let mut paths_w: usize = 0;
    for f in files {
        paths_w += (f.encode_utf16().count() + 1) * 2;
    }
    let total = 20 + paths_w + 2;
    let hg = match GlobalAlloc(GHND, total) {
        Ok(h) => h,
        Err(_) => return,
    };
    let p = GlobalLock(hg);
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    let bytes = std::slice::from_raw_parts_mut(p as *mut u8, total);
    // pFiles = 20
    bytes[0..4].copy_from_slice(&20u32.to_le_bytes());
    // fWide = TRUE（offset 16）
    bytes[16..20].copy_from_slice(&1u32.to_le_bytes());
    let mut off = 20usize;
    for f in files {
        for w in f.encode_utf16() {
            bytes[off..off + 2].copy_from_slice(&w.to_le_bytes());
            off += 2;
        }
        bytes[off..off + 2].copy_from_slice(&0u16.to_le_bytes());
        off += 2;
    }
    bytes[off..off + 2].copy_from_slice(&0u16.to_le_bytes());
    GlobalUnlock(hg);
    if OpenClipboard(None).is_ok() {
        let _ = EmptyClipboard();
        SetClipboardData(CF_HDROP_U, Some(HANDLE(hg.0)));
        set_self_shield();
        let _ = CloseClipboard();
    } else {
        let _ = GlobalFree(Some(hg));
    }
}

/// 写一条命名格式（空 DWORD）标记"这是 CurSeen 自己写回的"，捕获端见之即跳过。
unsafe fn set_self_shield() {
    let fmt = RegisterClipboardFormatW(PCWSTR(wide("CurSeen.ClipboardSelf").as_ptr()));
    if fmt == 0 {
        return;
    }
    let hg = match GlobalAlloc(GHND, 4) {
        Ok(h) => h,
        Err(_) => return,
    };
    let p = GlobalLock(hg);
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    *(p as *mut u32) = 0;
    GlobalUnlock(hg);
    SetClipboardData(fmt, Some(HANDLE(hg.0)));
}

/// 写回剪贴板**之后**再补一道自写标记（此刻剪贴板已关，这里自己开一次）。
///
/// [`set_self_shield`] 只能在"剪贴板已打开且还没关"时用；而通用的
/// `screenshot::to_clipboard` 自己会开关，调用方插不进去 —— 所以补这个后手。
/// 目的：让监听端跳过"我们自己刚写回的内容"，不去做一次无谓的回读（回读要
/// `GlobalLock` + 逐行拷贝 + PNG 重编码）。
unsafe fn shield_after_write() {
    if OpenClipboard(None).is_ok() {
        set_self_shield();
        let _ = CloseClipboard();
    }
}

unsafe fn read_files_opened() -> Option<Vec<String>> {
    if OpenClipboard(None).is_err() {
        return None;
    }
    let _guard = ClipboardCloser;
    if IsClipboardFormatAvailable(CF_HDROP_U).is_err() {
        return None;
    }
    let h = GetClipboardData(CF_HDROP_U).ok()?;
    let hg = HGLOBAL(h.0);
    let ptr = GlobalLock(hg);
    if ptr.is_null() {
        return None;
    }
    let _unlock = MemoryUnlocker(ptr as *mut u8);
    let hd = HDROP(h.0);
    let n = DragQueryFileW(hd, 0xFFFF_FFFF, None);
    if n == 0 {
        return None;
    }
    let mut files = Vec::with_capacity(n as usize);
    for i in 0..n {
        let len = DragQueryFileW(hd, i, None);
        if len == 0 {
            continue;
        }
        let mut buf = vec![0u16; len as usize + 1];
        DragQueryFileW(hd, i, Some(buf.as_mut_slice()));
        files.push(String::from_utf16_lossy(&buf[..len as usize]));
    }
    Some(files)
}

unsafe fn send_key(vk: u32, up: bool) {
    let inp = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: 0,
                dwFlags: if up {
                    KEYBD_EVENT_FLAGS(0x0002)
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let _ = SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
}

/// 目标顶层窗所在线程当前的键盘焦点控件：
/// 查不到（线程挂了/无焦点）返回空 HWND。
unsafe fn thread_focus_of(target: HWND) -> HWND {
    if target.is_invalid() || !IsWindow(Some(target)).as_bool() {
        return HWND::default();
    }
    let tid = GetWindowThreadProcessId(target, None);
    if tid == 0 {
        return HWND::default();
    }
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    if GetGUIThreadInfo(tid, &mut info).is_ok() {
        info.hwndFocus
    } else {
        HWND::default()
    }
}

/// 清除系统的「应用启动中」锁存（2026-10-07 深夜定案，转圈根因终局）。
///
/// 机制：explorer 双击启动 → 自提权重启后，系统一直认为该进程"还没处理过任何
/// 真实输入"，鼠标悬停面板就显示启动转圈（用户实测：手动点一下标题栏立刻消失；
/// SetForegroundWindow/激活不解锁——API 调用不是输入事件；脚本启动复现不了，
/// 因为不经过 explorer）。修 = 首开时把光标移到顶栏计数区（[`count_rect`]，标题
/// 拖动区里离搜索框/按钮/页签都最远），SendInput 注入一次**真实**左键单击（进
/// 系统输入队列，与用户手点等效；PostMessage 不算输入、无效），点完立刻还原
/// 光标。每进程只做一次（调用方静态旗标）。
unsafe fn clear_appstart_latch(panel: HWND) {
    let t0 = std::time::Instant::now();
    let mut orig = POINT::default();
    let _ = GetCursorPos(&mut orig);
    // 计数区中心（客户区坐标 → 屏幕坐标）。
    let r = count_rect();
    let mut pt = POINT {
        x: (r.left + r.right) / 2,
        y: (r.top + r.bottom) / 2,
    };
    let _ = ClientToScreen(panel, &mut pt);
    let _ = SetCursorPos(pt.x, pt.y);
    let mut down: INPUT = std::mem::zeroed();
    down.r#type = INPUT_MOUSE;
    down.Anonymous.mi = MOUSEINPUT {
        mouseData: 0,
        dwFlags: MOUSEEVENTF_LEFTDOWN,
        ..Default::default()
    };
    let mut up: INPUT = std::mem::zeroed();
    up.r#type = INPUT_MOUSE;
    up.Anonymous.mi = MOUSEINPUT {
        mouseData: 0,
        dwFlags: MOUSEEVENTF_LEFTUP,
        ..Default::default()
    };
    // 合并发送：对我们自己的窗口无需键间隔（那是 QQ 逐键规矩，鼠标单击无此坑）。
    let _ = SendInput(&[down, up], std::mem::size_of::<INPUT>() as i32);
    std::thread::sleep(std::time::Duration::from_millis(30));
    let _ = SetCursorPos(orig.x, orig.y);
    log::info!(
        "剪贴板: 启动锁存清除点击已注入 耗时={}ms",
        t0.elapsed().as_millis()
    );
}

/// 单键 SendInput（textus send_one 同款），返回是否成功。
unsafe fn send_one_key(vk: u16, flags: KEYBD_EVENT_FLAGS) -> bool {
    let mut input: INPUT = std::mem::zeroed();
    input.r#type = INPUT_KEYBOARD;
    input.Anonymous.ki = KEYBDINPUT {
        wVk: VIRTUAL_KEY(vk),
        wScan: 0,
        dwFlags: flags,
        time: 0,
        dwExtraInfo: 0,
    };
    SendInput(&[input], std::mem::size_of::<INPUT>() as i32) == 1
}

/// ★ textus 式 Ctrl+V（照抄 textus src/paste.rs send_ctrl_key(VK_V)）：
/// **逐个单事件** SendInput、每键间隔 10ms、左 Ctrl(0x11) + EXTENDEDKEY、
/// 中途失败也保证 Ctrl 抬起。QQ 等 CEF/Qt 应用对"一次 SendInput 打包四事件"
/// 的注入序列不认（2026-10-07 实测落空+报错误音），逐键发才吃。
unsafe fn send_paste_keys_textus() {
    const VK_CTRL: u16 = 0x11; // VK_CONTROL（左）
    const VK_KEY_V: u16 = 0x56;
    const KEYUP: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0x0002); // KEYEVENTF_KEYUP
    const EXT: KEYBD_EVENT_FLAGS = KEYBD_EVENT_FLAGS(0x0001); // KEYEVENTF_EXTENDEDKEY
    unsafe {
        if !send_one_key(VK_CTRL, EXT) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        if !send_one_key(VK_KEY_V, KEYBD_EVENT_FLAGS(0)) {
            let _ = send_one_key(VK_CTRL, EXT | KEYUP);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = send_one_key(VK_KEY_V, KEYUP);
        std::thread::sleep(std::time::Duration::from_millis(10));
        let _ = send_one_key(VK_CTRL, EXT | KEYUP);
    }
}

/// 注入一次无害的 Alt 按下+抬起（ down/up 相邻，中间无其它键，不会组成组合键）。
///
/// 用途见 [`Self::do_paste`]：向系统证明"用户刚操作过"，换取一次前台切换许可。
unsafe fn send_alt_tap() {
    let mut inputs: [INPUT; 2] = std::mem::zeroed();
    for (i, up) in [false, true].iter().enumerate() {
        inputs[i].r#type = INPUT_KEYBOARD;
        inputs[i].Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(VK_MENU_R as u16),
            wScan: 0,
            dwFlags: if *up {
                KEYBD_EVENT_FLAGS(0x0002)
            } else {
                KEYBD_EVENT_FLAGS(0)
            },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn send_ctrl_v() {
    let mut inputs: [INPUT; 4] = std::mem::zeroed();
    let seq: [(u32, bool); 4] = [
        (VK_CONTROL_R, false),
        (VK_V_R, false),
        (VK_V_R, true),
        (VK_CONTROL_R, true),
    ];
    for (i, (vk, up)) in seq.iter().enumerate() {
        inputs[i].r#type = INPUT_KEYBOARD;
        inputs[i].Anonymous.ki = KEYBDINPUT {
            wVk: VIRTUAL_KEY(*vk as u16),
            wScan: 0,
            dwFlags: if *up {
                KEYBD_EVENT_FLAGS(0x0002)
            } else {
                KEYBD_EVENT_FLAGS(0)
            },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    let _ = SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
}

unsafe fn draw_shot_to(hdc: HDC, dx: i32, dy: i32, dw: i32, dh: i32, shot: &Shot) {
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: shot.w,
            biHeight: -shot.h, // 负 = 自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    StretchDIBits(
        hdc,
        dx,
        dy,
        dw,
        dh,
        0,
        0,
        shot.w,
        shot.h,
        Some(shot.bgra.as_ptr() as *const c_void),
        &bmi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
}

// ── 向上箭头（抗锯齿自绘）────────────────────────────────────────────────
//
// 为什么自己栅格化，而不用 GDI 图元或字体字形：
// · GDI 的 `Polygon` / `LineTo` **零抗锯齿**——用户报"向上的箭头全是锯齿"就是它；
// · 字体字形（↑ U+2191 / ▲ U+25B2）在 17px 上只有 4 级灰阶，笔画还偏细，看着发虚。
// 这里用 4×4 超采样把"三角头 ∪ 方杆"栅格化成一张 **alpha 覆盖掩码**，再按按钮底色
// 现混成不透明位图贴上去——边缘是 17 级灰阶，任何 DPI / 缩放都平滑，且不依赖字体。

/// 每个像素方向上的超采样数（4 → 每像素 16 个采样点 → 17 级灰阶）。
const ARROW_SS: i32 = 4;

/// 箭头边长上限（按钮格是 `ACT_W`=22，铺满会顶到格边显得笨重 —— 2026-09-29 用户
/// 口径："向上箭头图标特别好，就是太大了，要缩小一点点"）。16 是 22 格的 ~0.73，
/// 四边各留 3px 余量，视觉上与旁边 🗑/★ 的视觉重量相当。
const ARROW_PX: i32 = 16;

/// 单位方格内的箭头形状：三角头 ∪ 方杆。`x` / `y` ∈ [0,1]，y 向下。
fn arrow_up_shape(x: f32, y: f32) -> bool {
    // 方杆（先判，与三角头是并集关系）
    if (0.402..=0.598).contains(&x) && (0.430..=0.960).contains(&y) {
        return true;
    }
    // 三角头：顶点 (0.5, 0.06)，底边 y = 0.50 处半宽 0.44。
    let t = (y - 0.06) / 0.44;
    if (0.0..=1.0).contains(&t) && (x - 0.5).abs() <= 0.44 * t {
        return true;
    }
    false
}

/// `size × size` 的箭头覆盖掩码：0 = 空，255 = 实心，中间值 = 抗锯齿边缘。
fn arrow_up_mask(size: i32) -> Vec<u8> {
    let n = size.max(1);
    let per_px = ARROW_SS * ARROW_SS;
    let mut out = Vec::with_capacity((n * n) as usize);
    for py in 0..n {
        for px in 0..n {
            let mut hit = 0i32;
            for sy in 0..ARROW_SS {
                for sx in 0..ARROW_SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / ARROW_SS as f32) / n as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / ARROW_SS as f32) / n as f32;
                    if arrow_up_shape(x, y) {
                        hit += 1;
                    }
                }
            }
            out.push((hit * 255 / per_px) as u8);
        }
    }
    out
}

/// 把抗锯齿向上箭头画进按钮矩形（中心对齐，不拉伸）。
///
/// `fg` / `bg` 都是 `(R, G, B)`：掩码的每一档按 `bg → fg` 现混，得到**不透明**位图，
/// 直接 `StretchDIBits` 上去 —— 不做 alpha 混合，就不必依赖 `AlphaBlend` 的预乘约定。
unsafe fn draw_arrow_up(hdc: HDC, r: RECT, fg: (u8, u8, u8), bg: (u8, u8, u8)) {
    // 不再"铺满按钮格"，改按 [`ARROW_PX`] 定边长（格内居中，四边留余量）。
    let n = ARROW_PX.min(r.right - r.left).min(r.bottom - r.top);
    if n <= 0 {
        return;
    }
    let mask = arrow_up_mask(n);
    let mut bgra = Vec::with_capacity((n * n * 4) as usize);
    for a in &mask {
        let t = *a as u32;
        let ch = |b: u8, f: u8| ((b as u32 * (255 - t) + f as u32 * t) / 255) as u8;
        // `Shot::bgra` 是 **B**GRA 顺序
        bgra.push(ch(bg.2, fg.2));
        bgra.push(ch(bg.1, fg.1));
        bgra.push(ch(bg.0, fg.0));
        bgra.push(255);
    }
    let shot = Shot { w: n, h: n, bgra };
    draw_shot_to(
        hdc,
        r.left + ((r.right - r.left) - n) / 2,
        r.top + ((r.bottom - r.top) - n) / 2,
        n,
        n,
        &shot,
    );
}

// ── 全局辅助 ───────────────────────────────────────────────────────────
unsafe fn cursor_pos() -> (i32, i32) {
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    (pt.x, pt.y)
}

fn work_rect_at(x: i32, y: i32) -> RECT {
    unsafe {
        let pt = POINT { x, y };
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        }
    }
}

/// 主显示器的工作区（"居中"的基准）。
///
/// ★ 不能拿 [`work_rect_at`] 顶替：那个要一个点，而"没记忆、首次打开"恰恰是**没有点**
///   （跟着光标走才需要点）。取 `MONITOR_DEFAULTTOPRIMARY` 才稳：光标可能在别的屏上，
///   但新用户第一次开面板就该落在这块屏幕的正中。
fn work_rect_primary() -> RECT {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            mi.rcWork
        } else {
            RECT {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            }
        }
    }
}

fn lparam_x(l: LPARAM) -> i32 {
    ((l.0 as u32) & 0xFFFF) as i16 as i32
}
fn lparam_y(l: LPARAM) -> i32 {
    ((l.0 as u32) >> 16) as i16 as i32
}

struct ClipboardCloser;
impl Drop for ClipboardCloser {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
struct MemoryUnlocker(*mut u8);
impl Drop for MemoryUnlocker {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(HGLOBAL(self.0 as *mut _));
        }
    }
}

// ── 窗口过程 ───────────────────────────────────────────────────────────
unsafe extern "system" fn listener_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_CLIPBOARDUPDATE {
        let mgr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClipboardMgr;
        if !mgr.is_null() {
            (*mgr).capture();
        }
        return LRESULT(0);
    }
    if msg == WM_DESTROY {
        let _ = RemoveClipboardFormatListener(hwnd);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn cmd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_APP_CLIP_CMD {
        let mgr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClipboardMgr;
        if !mgr.is_null() {
            match wparam.0 {
                CMD_TOGGLE => {
                    let t0 = std::time::Instant::now();
                    log::info!("剪贴板: 收到 toggle");
                    (*mgr).toggle();
                    let ms = t0.elapsed().as_millis();
                    if ms >= 20 {
                        log::warn!("剪贴板: toggle 处理耗时 {ms}ms（≥20ms，首次打开转圈嫌疑）");
                    }
                }
                CMD_SETCFG => {
                    let cfg = *Box::from_raw(lparam.0 as *mut ClipCfg);
                    (*mgr).apply_config(cfg);
                }
                CMD_CLEAR => (*mgr).clear(),
                // ★ 异步补采当前剪贴板（见 show_panel）：仅唤醒消息泵去跑 capture()，
                //   此时面板已经可见、正在抽消息，不会因同步解码图片而转圈。
                CMD_CAPTURE => {
                    let t0 = std::time::Instant::now();
                    (*mgr).capture();
                    let ms = t0.elapsed().as_millis();
                    if ms >= 10 {
                        log::warn!("剪贴板: capture 耗时 {ms}ms（≥10ms，开面板后泵卡嫌疑）");
                    }
                }
                // ★ 只在**可见**时收：已经收着就别再动（重开一次会丢滚动位置）。
                // （守卫写进 match 臂：满足 clippy collapsible-if，行为不变）
                CMD_HIDE if unsafe { IsWindowVisible((*mgr).panel) }.as_bool() => {
                    (*mgr).hide_panel();
                }
                CMD_HIDE => {}
                _ => {}
            }
        }
        return LRESULT(0);
    }
    if msg == WM_APP_THUMB_WAKE {
        // 仅唤醒命令窗消息泵去抽缩略图回传通道（真正的插入 + 刷新在 `run` 的循环里）。
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe extern "system" fn panel_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mgr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClipboardMgr;
    match msg {
        WM_PAINT => {
            if !mgr.is_null() {
                (*mgr).draw_panel(hwnd);
            }
            LRESULT(0)
        }
        WM_CTLCOLOREDIT => {
            // ★ DC 在 wParam，lParam 是控件 HWND（便签踩过的坑）。
            let hdc = HDC(wparam.0 as *mut _);
            if !mgr.is_null() {
                SetTextColor(hdc, RGB(C_EDIT_TEXT.0, C_EDIT_TEXT.1, C_EDIT_TEXT.2));
                SetBkColor(hdc, RGB(C_EDIT_BG.0, C_EDIT_BG.1, C_EDIT_BG.2));
            }
            let brush = if !mgr.is_null() {
                (*mgr).edit_bg_brush
            } else {
                HBRUSH(GetStockObject(WHITE_BRUSH).0)
            };
            LRESULT(brush.0 as isize)
        }
        WM_ERASEBKGND => {
            // ★ 闪烁的另一半：默认流程会在 `WM_PAINT` 前先把客户区擦一遍（"擦"和"画"
            //   之间那几毫秒，屏幕上是空白/旧色）。绘制走双缓冲、每帧都铺满整个客户区，
            //   这一趟擦除纯属多余——直接吞掉。
            LRESULT(1)
        }
        WM_COMMAND => {
            let code = (wparam.0 >> 16) as u16;
            if code == EN_CHANGE_R && !mgr.is_null() {
                let _ = SetTimer(Some((*mgr).panel), SEARCH_TIMER, 280, None);
            }
            LRESULT(0)
        }
        WM_KEYDOWN => {
            if !mgr.is_null() {
                match wparam.0 as u32 {
                    VK_UP_R => (*mgr).move_sel(-1),
                    VK_DOWN_R => (*mgr).move_sel(1),
                    VK_ESCAPE_R => (*mgr).hide_panel(),
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if !mgr.is_null() {
                let delta = ((wparam.0 >> 16) as i16) as i32;
                let step = if delta < 0 { 3 } else { -3 };
                let vis = visible(&(*mgr).entries, (*mgr).tab, &(*mgr).query);
                let rows = visible_rows() as usize;
                let max_scroll = max_scroll(vis.len(), rows);
                let ns =
                    ((*mgr).scroll as isize + step as isize).clamp(0, max_scroll as isize) as usize;
                (*mgr).scroll = ns;
                (*mgr).refresh_panel();
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            if !mgr.is_null() {
                (*mgr).on_mouse_move(lparam);
                // 跟踪移出
                let mut tme = TRACKMOUSEEVENT {
                    cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: hwnd,
                    dwHoverTime: 0,
                };
                TrackMouseEvent(&mut tme);
            }
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if !mgr.is_null() {
                (*mgr).on_mouse_leave();
            }
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            log::info!(
                "剪贴板: 面板收到 LBUTTONDOWN x={} y={}",
                lparam_x(lparam),
                lparam_y(lparam)
            );
            if !mgr.is_null() {
                let x = lparam_x(lparam);
                let y = lparam_y(lparam);
                // 顶栏按钮：置顶 / 收起 / 关闭（放大镜区走 NCHITTEST 拖动，到不了这）
                // ★ 命中走 [`top_btn_at`]——跟绘制、悬停共用同一份几何，不会出现
                //   "画在一处、点在另一处"（旧版这里自己又抄了一遍算式）。
                if let Some(i) = top_btn_at(x, y) {
                    // 自右往左：0=关闭 1=收起 2=置顶
                    match i {
                        0 => (*mgr).hide_panel(),
                        1 => (*mgr).toggle_collapsed(),
                        _ => (*mgr).toggle_pinned(),
                    }
                    return LRESULT(0);
                }
                // 页签点击（顶栏之下、列表之上）
                let tab_w = (PANEL_W - 2 * GAP) / TABS.len() as i32;
                if (TAB_Y..=TAB_Y + TAB_H).contains(&y) && (GAP..=PANEL_W - GAP).contains(&x) {
                    let mut i = (x - GAP) / tab_w;
                    if i < 0 {
                        i = 0;
                    }
                    if i as usize >= TABS.len() {
                        i = (TABS.len() - 1) as i32;
                    }
                    (*mgr).set_tab(TABS[i as usize].1);
                    return LRESULT(0);
                }
                // 滚动条（在列表右外侧，必须先于行判定，否则边缘那几列会被当行点击）
                let t = scroll_track();
                if x >= t.left && x <= t.right && y >= t.top && y <= t.bottom {
                    (*mgr).on_scroll_down(y);
                    return LRESULT(0);
                }
                if x >= GAP && x <= list_right() && (LIST_TOP..=PANEL_H - GAP).contains(&y) {
                    let row = (y - LIST_TOP) / ITEM_H;
                    let rows = visible_rows();
                    let vi = (*mgr).scroll as i32 + row;
                    let vis = visible(&(*mgr).entries, (*mgr).tab, &(*mgr).query);
                    if vi >= 0 && (vi as usize) < vis.len() && row < rows {
                        let gi = vis[vi as usize];
                        // ★ 收藏 / 🗑 删除 / ↑ 复制置顶（自右往左三颗 ACT_W 见方）
                        let r = list_right();
                        if x > r - GAP - ACT_W {
                            log::info!("剪贴板: 行点击 star gi={gi}");
                            (*mgr).toggle_fav(gi);
                        } else if x > r - GAP - ACT_W - GAP - ACT_W {
                            log::info!("剪贴板: 行点击 trash gi={gi}");
                            (*mgr).delete_entry(gi);
                        } else if x > r - GAP - 3 * ACT_W - 2 * GAP {
                            log::info!("剪贴板: 行点击 recopy gi={gi}");
                            (*mgr).recopy_entry(gi);
                        } else {
                            // ★ 单击即粘（2026-10-07 用户定案：双击粘贴档废弃）。防误触
                            //   靠"动作钮区先于行判定"挡住，扫历史误触的成本可接受。
                            log::info!("剪贴板: 行点击 paste gi={gi} x={x} y={y} row={row}");
                            (*mgr).selected = vi as usize;
                            (*mgr).paste_selected();
                        }
                    } else {
                        log::info!(
                            "剪贴板: 行点击落空 x={x} y={y} row={row} vi={vi} vis={}",
                            vis.len()
                        );
                    }
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // 拖滚动条的手松开：清抓取态 + 释放鼠标捕获（不清的话鼠标离开面板还在滚）。
            if !mgr.is_null() {
                (*mgr).on_lbutton_up();
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if !mgr.is_null() {
                match wparam.0 {
                    SEARCH_TIMER => {
                        KillTimer(Some((*mgr).panel), SEARCH_TIMER);
                        let mut buf = vec![0u16; 256];
                        let n = GetWindowTextW((*mgr).edit, &mut buf);
                        let q = String::from_utf16_lossy(&buf[..n as usize]);
                        (*mgr).set_query(q);
                    }
                    HOVER_TIMER => {
                        KillTimer(Some((*mgr).panel), HOVER_TIMER);
                        if let Some(gi) = (*mgr).hover {
                            (*mgr).show_preview_for(gi);
                        }
                    }
                    PASTE_TIMER => {
                        KillTimer(Some((*mgr).panel), PASTE_TIMER);
                        (*mgr).do_paste();
                    }
                    PASTE_SEND_TIMER => {
                        KillTimer(Some((*mgr).panel), PASTE_SEND_TIMER);
                        (*mgr).do_paste_send();
                    }
                    WARN_TIMER => {
                        // 告知条到点自毁：让计数回到它自己的位置（见 [`WARN_SHOW_MS`]）。
                        KillTimer(Some((*mgr).panel), WARN_TIMER);
                        if (*mgr).top_notice.take().is_some() {
                            (*mgr).refresh_panel();
                        }
                    }
                    RETRY_TIMER => {
                        // 捕获读失败重试：重新走一遍 capture（内部自带归零/再定时的判定）。
                        KillTimer(Some((*mgr).panel), RETRY_TIMER);
                        (*mgr).capture();
                    }
                    TARGET_POLL_TIMER if IsWindowVisible((*mgr).panel).as_bool() => {
                        // ★ 粘贴目标轮询（见 [`TARGET_POLL_TIMER`]）：只记**外部**窗口
                        //   （本进程任何窗口 / 系统菜单 #32768 都不算）。这里不 kill ——
                        //   周期计时器随 show/hide 挂/摘。
                        let fg = GetForegroundWindow();
                        if !fg.is_invalid() && fg != (*mgr).panel {
                            let mut pid = 0u32;
                            GetWindowThreadProcessId(fg, Some(&mut pid));
                            if pid != 0 && pid != GetCurrentProcessId() {
                                let mut buf = [0u16; 64];
                                let n = GetClassNameW(fg, &mut buf);
                                let cls = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                                if cls != "#32768" {
                                    (*mgr).last_foreground = fg;
                                    (*mgr).saved_focus = thread_focus_of(fg);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            LRESULT(0)
        }
        WM_ACTIVATE => {
            if (wparam.0 as u16) == 0 {
                // WA_INACTIVE：失焦自动隐藏（blur-hide）；置顶时不隐藏。
                if !mgr.is_null() {
                    // ★★ 失焦时**刷新粘贴目标**（2026-10-07 粘不出去的根因修复）：
                    //   `last_foreground` 原本只在 show_panel 记一次并冻结。置顶面板下
                    //   用户"点目标应用 → 点面板行粘贴"，目标仍是**开面板那一刻**的窗口
                    //   ⇒ Ctrl+V 切到一个过期窗口（日志全 ok=true 但用户看着没粘上）。
                    //   有效粘贴目标跟着前台走：谁抢走焦点谁就是新目标。
                    //   只记外部窗口（系统菜单 / 自己的其它窗口不算）。
                    let new_fg = GetForegroundWindow();
                    if !new_fg.is_invalid() && new_fg != (*mgr).panel {
                        let mut buf = [0u16; 64];
                        let n = GetClassNameW(new_fg, &mut buf);
                        let cls = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
                        if cls != "#32768" {
                            (*mgr).last_foreground = new_fg;
                            (*mgr).saved_focus = thread_focus_of(new_fg);
                        }
                    }
                    if !(*mgr).pinned {
                        (*mgr).hide_panel();
                    }
                }
            }
            LRESULT(0)
        }
        WM_NCHITTEST => {
            // 顶栏除交互控件外整条 = 原生拖动把手（HTCAPTION），其余走默认。
            // ★ 判据走 [`title_drag_at`]，与绘制/悬停共用同一份几何。
            if !mgr.is_null() {
                let mut pt = POINT {
                    x: (lparam.0 & 0xFFFF) as i16 as i32,
                    y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                };
                let _ = ScreenToClient(hwnd, &mut pt);
                if title_drag_at(pt.x, pt.y) {
                    return LRESULT(HTCAPTION as isize);
                }
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        WM_EXITSIZEMOVE => {
            // 拖动结束：记住位置。
            if !mgr.is_null() {
                (*mgr).save_panel_state();
            }
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => {
            // ★ 2026-10-07 晚间实测推翻"点击不抢焦点"方案（原 MA_NOACTIVATE 已删）：
            //   驱动实验对照（temp/panel_drive + 取证日志）：面板被激活的点击（TIM 走完
            //   失活→再激活周期）粘贴到货；MA_NOACTIVATE 让 TIM 保持前台的点击反而
            //   MISS——写回剪贴板触发 TIM 自己的剪贴板助手气泡弹出又消，前台被打成
            //   0x0 悬空、TIM 内部焦点丢在已销毁的气泡上，Ctrl+V 落空报错误音。
            //   textus（TIM 下粘贴可靠）正是故意让窗口激活（见其 main.rs：不设
            //   WS_EX_NOACTIVATE）。所以这里**不再拦截**，让面板正常激活，切前台
            //   回目标应用交给 do_paste 的既有流程。
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn preview_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mgr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClipboardMgr;
    match msg {
        WM_PAINT => {
            if !mgr.is_null() {
                (*mgr).draw_preview(hwnd);
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn edit_subclass(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let mgr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClipboardMgr;
    if !mgr.is_null() && msg == WM_KEYDOWN {
        match wparam.0 as u32 {
            VK_UP_R => {
                (*mgr).move_sel(-1);
                return LRESULT(0);
            }
            VK_DOWN_R => {
                (*mgr).move_sel(1);
                return LRESULT(0);
            }
            VK_RETURN_R => {
                (*mgr).paste_selected();
                return LRESULT(0);
            }
            VK_ESCAPE_R => {
                (*mgr).hide_panel();
                return LRESULT(0);
            }
            _ => {}
        }
    }
    let orig = ORIG_EDIT.load(Ordering::Relaxed) as *mut ();
    CallWindowProcW(
        Some(std::mem::transmute::<
            *mut (),
            unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        >(orig)),
        hwnd,
        msg,
        wparam,
        lparam,
    )
}

// ── 线程入口 ───────────────────────────────────────────────────────────
fn run() {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let mut mgr = ClipboardMgr::new();
        // ★ `mgr.load()` 挪到 CMD_HWND.set 之后（见下方注释）——`ensure_started()`
        //   在主线程上忙等这个句柄，load 放在前面等于让主线程替后台线程的启动买单。
        let mgr_ptr = &mut mgr as *mut ClipboardMgr;

        // 注册类
        let _ = register_class("CurSeenClipCmd", cmd_proc, hinst);
        let _ = register_class("CurSeenClipListener", listener_proc, hinst);
        let _ = register_class("CurSeenClipPanel", panel_proc, hinst);
        let _ = register_class("CurSeenClipPreview", preview_proc, hinst);

        // 命令窗（message-only）
        let cmd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("CurSeenClipCmd").as_ptr()),
            PCWSTR(wide("clipcmd").as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst),
            None,
        )
        .expect("clip cmd window");
        SetWindowLongPtrW(cmd, GWLP_USERDATA, mgr_ptr as isize);

        // 监听窗（message-only）
        let listener = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("CurSeenClipListener").as_ptr()),
            PCWSTR(wide("clipln").as_ptr()),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinst),
            None,
        )
        .expect("clip listener window");
        SetWindowLongPtrW(listener, GWLP_USERDATA, mgr_ptr as isize);
        let _ = AddClipboardFormatListener(listener);

        // 面板窗
        let panel = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            PCWSTR(wide("CurSeenClipPanel").as_ptr()),
            PCWSTR(wide("剪贴板").as_ptr()),
            WS_POPUP | WS_CLIPCHILDREN,
            0,
            0,
            PANEL_W,
            PANEL_H,
            None,
            None,
            Some(hinst),
            None,
        )
        .expect("clip panel window");
        SetWindowLongPtrW(panel, GWLP_USERDATA, mgr_ptr as isize);
        mgr.panel = panel;

        // ★ 窗口级圆角（2026-09-25 UI 统一，与面板卡片/控件的对齐见 ui/tokens.rs）：
        //   Win11 用 DWM 合成期硬件圆角（抗锯齿），GDI 的方角 FillRect **无需改动**——
        //   DWM 裁四角的效果与系统对普通窗的做法完全一致。Win10（build<22000）上属性
        //   号 33 无合法占用、返回 E_INVALIDARG 被丢弃 → 自动降级方角，与 windui
        //   frameless 分支 DONOTROUND 同一套机制（那边取值相反：内容即窗口要直角）。
        //   本窗非 layered（WS_EX_TOPMOST|TOOLWINDOW 而已），DWM 圆角对其生效。
        //   预览浮窗刻意不加：它显示的是图片内容，矩形就该是矩形。
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            panel,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const std::ffi::c_void,
            std::mem::size_of_val(&pref) as u32,
        );

        // 搜索框（原生 EDIT 子控件，真焦点）
        //
        // ★★ 用户第 3 点：文本不垂直居中。
        //   * 单行 EDIT 用系统那套"按 ascent+descent 框居中"，Segoe UI 的 ascent 明显
        //     大于 descent，字形**视觉重心**偏上——这是 Windows 的老毛病。
        //   * 唯一能摁住文本行的通道是 `EM_SETRECT`，而它**只对多行 EDIT 生效**（单行
        //     控件直接忽略）。所以这里改成多行 EDIT：`ES_MULTILINE` + `ES_AUTOHSCROLL`
        //     （后者让多行也不自动换行，行为跟单行一致），**不带** `ES_WANTRETURN`
        //     （回车不插换行，且子类里本来就把 VK_RETURN 吃掉了 → 走粘贴）。
        let edit_w = edit_right() - edit_left();
        let edit = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("EDIT").as_ptr()),
            PCWSTR(wide("").as_ptr()),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | ES_MULTILINE as u32 | ES_AUTOHSCROLL as u32),
            edit_left(),
            TOP_Y,
            edit_w,
            SEARCH_BOX_H,
            Some(panel),
            None,
            Some(hinst),
            None,
        )
        .expect("clip edit window");
        SetWindowLongPtrW(edit, GWLP_USERDATA, mgr_ptr as isize);
        SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(mgr.font.0 as usize)),
            Some(LPARAM(1)),
        );
        // ★ 把文本行摁到垂直居中（+1px 视觉补偿），左右各留 `EDIT_PAD_X`。
        //   行高问的是**装进去之后**的字体度量——字体是在上面 `WM_SETFONT` 才给的，
        //   必须排在它后面取。
        let line_h = {
            let edc = GetDC(Some(edit));
            let old = SelectObject(edc, HGDIOBJ(mgr.font.0));
            let mut tm = TEXTMETRICW::default();
            let _ = GetTextMetricsW(edc, &mut tm);
            SelectObject(edc, old);
            let _ = ReleaseDC(Some(edit), edc);
            tm.tmHeight
        };
        let fr = edit_format_rect(edit_w, line_h);
        SendMessageW(
            edit,
            EM_SETRECT,
            None,
            Some(LPARAM(&fr as *const RECT as isize)),
        );
        SetFocus(Some(edit));
        // 子类 EDIT 以拦截方向键/回车/Esc
        let orig = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_subclass as *const () as isize);
        ORIG_EDIT.store(orig as usize, Ordering::Relaxed);
        mgr.edit = edit;

        // 预览窗（无激活，不抢焦点；★ WS_EX_TRANSPARENT 点击穿透——
        // hover 400ms 后预览弹出，用户此刻点击必须落到下面的列表行，
        // 而不是被预览窗吃掉变成死点击）
        let preview = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            PCWSTR(wide("CurSeenClipPreview").as_ptr()),
            PCWSTR(wide("预览").as_ptr()),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(hinst),
            None,
        )
        .expect("clip preview window");
        SetWindowLongPtrW(preview, GWLP_USERDATA, mgr_ptr as isize);
        mgr.preview = preview;

        CMD_HWND.set(AtomicUsize::new(cmd.0 as usize)).ok();
        log::info!("剪贴板: 后台线程就绪（窗口已创建）");

        // ★ 主线程此刻已解锁。历史元数据在这里载（纯文本行解析，毫秒级；不解码像素）。
        mgr.load();

        // ★ 缩略图**不在这里解码**（2026-09-29 用户报"首次打开转圈好几秒"）：解码任务
        //   通道与 worker 已在 `ClipboardMgr::new()` 建好；首开只解**可见行**——`show_panel`
        //   / 滚动 / 切页 / 搜索时 `ensure_visible_thumbs` 把屏幕上那 ~20 行投递给 worker，
        //   worker 独立线程解码后经 `WM_APP_THUMB_WAKE` 唤醒本循环抽通道、插 `thumbs`、
        //   刷面板。面板线程全程在响应消息，光标不再转圈（且只首开缓存空时触发）。

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            // 抽干 worker 回传通道：缩略图插缓存、清在途标记；剪贴板捕获入账
            // （`push` 自带哈希去重：已存在 → 上浮，新条目 → 入账+落盘元数据+刷面板，
            // 与旧同步路径完全同一入口）。一次抽干后最多一次重绘，不阻塞消息泵。
            let mut got = false;
            while let Ok(m) = mgr.thumb_rx.try_recv() {
                match m {
                    CapMsg::Thumb(id, shot) => {
                        mgr.thumbs.insert(id, shot);
                        mgr.in_flight.remove(&id);
                        got = true;
                    }
                    CapMsg::Captured(cap) => {
                        if let Some(c) = cap {
                            mgr.push(
                                Kind::Image,
                                String::new(),
                                Vec::new(),
                                Some(c.rel),
                                c.w,
                                c.h,
                                Some(c.thumb),
                                // ★ 全尺寸原件：`full` 字段本来是给超限大文本留的，语义完全一致
                                //   （"正文放盘上、条目里留个预览"），所以直接复用，不去新开字段。
                                //   顺带白拿三条既有清理路径（`evict`/`clear`/`delete_entry`
                                //   都在删 `full`），也不必担心它们误删——那里存的恒是我们自己创建的相对路径。
                                c.full,
                                None,
                            );
                        }
                    }
                    CapMsg::FileThumb(id, cap) => {
                        mgr.in_flight.remove(&id);
                        // 回填的是"给这条补一张预览"，不是新内容：只动 `thumb` 相关，
                        // **不改时间戳、不上浮**——用户此刻已经看见它躺在历史里了，
                        // 一张预览迟到了几百毫秒没有理由把它顶回第一位。
                        let mut decoded = None;
                        if let Some(c) = cap {
                            if let Some(e) = mgr.entries.iter_mut().find(|e| e.id == id) {
                                e.thumb = Some(c.rel);
                                // ★ 刻意不写 `w`/`h`：那两个字段在条目里的语义是"我们持有的
                                //   这张图的尺寸"，而我们在家里没存原图，只有这张 720px 预览。
                                //   填上去会让人误以为列表里躺着的就是这个分辨率。
                                decoded = Some(c.thumb);
                            }
                        }
                        if let Some(shot) = decoded {
                            mgr.thumbs.insert(id, shot);
                            mgr.persist();
                            got = true;
                        }
                    }
                    CapMsg::StorageWarn(total) => {
                        mgr.top_notice = Some(TopNotice::Storage(total));
                        got = true;
                        // 出世即倒计时（重复投递只会把同一个计时器重新开一遍）。
                        SetTimer(Some(mgr.panel), WARN_TIMER, WARN_SHOW_MS, None);
                    }
                }
            }
            if got {
                mgr.refresh_panel();
            }
        }
    }
}

unsafe fn register_class(
    name: &str,
    wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
    hinst: HINSTANCE,
) -> u16 {
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        // ★ 不用 `CS_HREDRAW | CS_VREDRAW`：这两个 class style 会在**任何**尺寸变化时
        //   强制整客户区重画一次；本模块每次 `SetWindowPos` 后都自己 `refresh_panel`，
        //   那份"自动重画"是白付的，收起/展开（高度变化）时它会先闪一下旧高度的残影。
        // ★ 要 `CS_DBLCLKS`：双击粘贴模式靠 `WM_LBUTTONDBLCLK` 触发粘贴（2026-09-30）。
        //   命令窗共用本类，多收一条 DBLCLK 无害（没人处理它）。
        style: WNDCLASS_STYLES(CS_DBLCLKS.0),
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        lpszClassName: PCWSTR(wide(name).as_ptr()),
        ..Default::default()
    };
    RegisterClassExW(&wc)
}

// ── 纯函数单测（过滤 / 去重 / 淘汰 / URL 判定 / 缩略图 / 签名）──────────
#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind, text: &str, files: &[&str], w: i32, h: i32, sig: u64) -> ClipEntry {
        ClipEntry {
            id: 0,
            kind,
            text: text.into(),
            files: files.iter().map(|s| s.to_string()).collect(),
            thumb: None,
            w,
            h,
            full: None,
            html: None,
            fav: false,
            ts: 0,
            hash: sig,
            missing: false,
        }
    }

    /// ★ 超限大文本预览截断必须按**字符边界**（多字节中文不能被拦腰切成半个字，
    ///   更不能像旧实现 `&text[..n]` 那样直接 panic）。
    #[test]
    fn safe_prefix_cuts_on_char_boundaries_only() {
        // "红色" 每个 3 字节：max=4 落在第二字中间 → 回退到 3。
        assert_eq!(safe_prefix("红色红", 4), "红");
        assert_eq!(safe_prefix("红色红", 3), "红");
        assert_eq!(safe_prefix("红色红", 6), "红色");
        // 短文本原样。
        assert_eq!(safe_prefix("abc", 100), "abc");
        assert_eq!(safe_prefix("", 10), "");
        // emoji（4 字节）也不panic。
        assert_eq!(safe_prefix("😀😀", 5), "😀");
        // 长度语义：结果一定是合法 str 且字节长度 ≤ max。
        let big = "中".repeat(1000);
        let cut = safe_prefix(&big, 999);
        assert!(cut.len() <= 999 && cut.chars().all(|c| c == '中'));
    }

    /// 超限文本条目的 hash 必须掺入 `full`（不同全文、同预览不能互相吞并）。
    #[test]
    fn push_hash_distinguishes_different_full_paths() {
        let a = Some("temp/clipboard/texts/aaaa.txt".to_string());
        let b = Some("temp/clipboard/texts/bbbb.txt".to_string());
        assert_ne!(full_hash(&a), full_hash(&b));
        assert_eq!(full_hash(&a), full_hash(&a.clone()));
        assert_eq!(full_hash(&None), full_hash(&None));
    }

    #[test]
    fn url_judgment() {
        assert!(is_url("https://example.com"));
        assert!(is_url("http://a.b/c"));
        assert!(is_url("www.example.com/x"));
        assert!(is_url("ftp://host/file"));
        assert!(!is_url("not a url"));
        assert!(!is_url("line1\nline2"));
        assert!(!is_url(""));
    }

    #[test]
    fn title_bar_drag_excludes_only_the_interactive_bits() {
        // 可拖：放大镜、左侧空白、计数那条、顶栏上下那圈 GAP。
        for (x, y) in [
            (GAP, TOP_Y + 4),                   // 放大镜（旧版唯一能拖的一块）
            (GAP + DRAG_W + 1, TOP_Y),          // 放大镜右缘与搜索框之间的缝
            (count_rect().left + 8, TOP_B - 4), // 计数文字那条
            (edit_left() - 1, TOP_Y + 2),       // 搜索框左侧那道缝
            (12, TOP_Y - 1),                    // 顶栏上方的 GAP（x=12 已在搜索框列内）
            (120, TOP_Y - 1),                   // 同上：GAP 那条通吃
            (count_rect().right - 1, TOP_B - 1),
            (edit_right(), TOP_Y + 4), // 搜索框右沿外 1px：还差一格，仍归拖动
        ] {
            assert!(title_drag_at(x, y), "({x},{y}) 本该能拖");
        }
        // 不可拖：搜索框（原生 EDIT 子窗）——含它左沿内 1px；三颗按钮整条带；顶栏以下。
        for (x, y) in [
            (edit_left() + 4, TOP_Y + 4),
            (edit_right() - 1, TOP_Y + 4),
            (edit_left() + 4, TOP_B - 1),
            (PANEL_W - GAP - BTN_W * 3, TOP_Y + 4), // 置顶按钮左沿
            (PANEL_W - GAP - 1, TOP_Y + 4),         // 关闭按钮
            (PANEL_W - GAP - 1, TOP_B - 1),
        ] {
            assert!(!title_drag_at(x, y), "({x},{y}) 是交互控件，不该抢点击");
        }
        // 顶栏以下一律不是标题栏（页签 / 列表归各自的处理）。
        assert!(!title_drag_at(150, TOP_B));
        assert!(!title_drag_at(150, PANEL_H - 10));
    }

    #[test]
    fn visible_filters_by_tab_and_query() {
        let mut v = vec![
            entry(Kind::Text, "hello world", &[], 0, 0, 1),
            entry(Kind::File, "", &["C:\\a.txt"], 0, 0, 2),
            entry(Kind::Link, "https://x.com", &[], 0, 0, 3),
            entry(Kind::Image, "", &[], 100, 80, 4),
        ];
        // 最新在前：Image, Link, File, Text
        assert_eq!(visible(&v, TabKind::All, "").len(), 4);
        assert_eq!(visible(&v, TabKind::Text, "").len(), 1);
        assert_eq!(visible(&v, TabKind::File, "").len(), 1);
        assert_eq!(visible(&v, TabKind::Link, "").len(), 1);
        assert_eq!(visible(&v, TabKind::Image, "").len(), 1);
        // 搜索
        assert_eq!(visible(&v, TabKind::All, "hello").len(), 1);
        assert_eq!(visible(&v, TabKind::All, "x.com").len(), 1);
        assert_eq!(visible(&v, TabKind::All, "a.txt").len(), 1);
        assert_eq!(visible(&v, TabKind::All, "nope").len(), 0);
        // 收藏：无收藏则为空
        assert_eq!(visible(&v, TabKind::Fav, "").len(), 0);
        v[1].fav = true;
        assert_eq!(visible(&v, TabKind::Fav, "").len(), 1);
    }

    #[test]
    fn signature_is_content_stable() {
        let a = compute_sig(Kind::Text, "abc", &[], 0, 0, None);
        let b = compute_sig(Kind::Text, "abc", &[], 0, 0, None);
        let c = compute_sig(Kind::Text, "abd", &[], 0, 0, None);
        assert_eq!(a, b);
        assert_ne!(a, c);
        // 文件顺序不同视为不同（路径列表参与签名）
        let f1 = compute_sig(
            Kind::File,
            "",
            &["a".to_string(), "b".to_string()],
            0,
            0,
            None,
        );
        let f2 = compute_sig(
            Kind::File,
            "",
            &["b".to_string(), "a".to_string()],
            0,
            0,
            None,
        );
        assert_ne!(f1, f2);
    }

    /// ★★ 这条钉的是整个改动的**收益本身**：写回去的必须是全尺寸，不是那张 720px 的副本。
    ///
    /// `paste_shot` 走的就是这条路径（先试 `full` 再到 `thumbs`），一旦它退化成直接拿
    /// `thumbs` 里的像素交差，用户粘回去的永远是降采样图，而这个错**肉眼几乎看不出来**——
    /// 只有真的去比对尺寸才发现。所以这里把"原图 → 落盘 → 读回"整条跑一遍。
    #[test]
    fn original_size_survives_the_disk_round_trip() {
        let full_res = Shot {
            w: 1920,
            h: 1080,
            bgra: vec![0x77u8; (1920 * 1080 * 4) as usize],
        };
        // ① 先确认这张图确实是要被缩的：不存原件的话，能拿回来的就只有这份。
        let thumb = thumbnail_of(&full_res, THUMB_MAX_W);
        assert_eq!(
            thumb.w, THUMB_MAX_W,
            "1920 宽必须真被降采样，否则本测试无意义"
        );

        // ② 走真实落盘 + 真实解码（与运行时完全同一条 `save_png`/`shot_from_png`）。
        let dir = crate::core::paths::temp_dir().join("clip_test_roundtrip");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("orig.png");
        save_png(&full_res, &p).expect("原图应能落盘");
        let back = shot_from_png(&p).expect("原图应能读回");
        assert_eq!(
            (back.w, back.h),
            (full_res.w, full_res.h),
            "从 full 路径读回的必须是未降采样的 {}×{}，实际 {}×{}",
            full_res.w,
            full_res.h,
            back.w,
            back.h
        );
        assert_eq!(back.bgra.len(), full_res.bgra.len());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }

    /// 一张 8×8 的索引色 GIF（4 色调色板，56 字节）。**素材不是随手挑的**，理由见下一条测试。
    const TINY_INDEXED_GIF: &[u8] = &[
        0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x08, 0x00, 0x08, 0x00, 0x81, 0x00, 0x00, 0xff, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00,
        0x08, 0x00, 0x08, 0x00, 0x00, 0x08, 0x0f, 0x00, 0x01, 0x08, 0x1c, 0x48, 0xb0, 0xa0, 0xc1,
        0x83, 0x08, 0x13, 0x2a, 0x4c, 0x18, 0x10, 0x00, 0x3b,
    ];

    /// ★★★ 这条钉的是 WIC 那个必踩的坑，而素材**必须**是索引色图才测得出来。
    ///
    /// 坑本身：[`decode_image_file`] 注释里写的——**`CopyPixels` 不做像素格式转换**，
    /// 请求 BGRA 不等于得到 BGRA。踩过的实测：JPEG/PNG/BMP 的 alpha 有 25% 是垃圾，
    /// GIF（原生 `8bppIndexed`）更彻底，拷出来的是**调色板索引**，画上去一整片空白。
    ///
    /// ★ 曾经这条测试用的是纯色 PNG，结果把 converter 整段删掉它照样绿——因为那种图
    ///   原生就是 32bppBGRA，根本不需要转换。测试看着在跑，其实什么都没看住。
    #[test]
    fn wic_decode_converts_paletted_images_into_opaque_pixels() {
        let dir = std::env::temp_dir().join("curseen_test_wic_gif");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("tiny.gif");
        std::fs::write(&p, TINY_INDEXED_GIF).expect("测试图应能写盘");

        let _com = ComGuard::new();
        let factory: IWICImagingFactory = unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .expect("WIC 工厂应该建得起来（系统自带）")
        };
        let shot = decode_image_file(&factory, &p.to_string_lossy()).expect("应该解得出来");
        assert_eq!((shot.w, shot.h), (8, 8), "小图不该被放大");
        // ★★★ 真正的判据：一个透明像素都不许有。
        assert!(
            shot.bgra.chunks_exact(4).all(|px| px[3] == 255),
            "少了 IWICFormatConverter：8bppIndexed 拷出来的是调色板索引，alpha 全 0，画上去是空白"
        );
        // 调色板第 0 项是红色，整张图应还原成红色（BGRA = 0,0,255,255），而不是吐索引值。
        assert_eq!(&shot.bgra[..4], &[0u8, 0, 255, 255], "索引色应还原成颜色");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 宽图必须被缩到 [`THUMB_MAX_W`]，且还原色值不被这条链路改掉。
    #[test]
    fn wic_decode_scales_a_wide_source_down() {
        let dir = std::env::temp_dir().join("curseen_test_wic_scale");
        let _ = std::fs::create_dir_all(&dir);
        let (w, h) = (1000i32, 500i32);
        let bgra = std::iter::repeat_n([200u8, 100, 40, 255], (w * h) as usize)
            .flatten()
            .collect::<Vec<u8>>();
        let p = dir.join("src.png");
        assert!(
            save_png(&Shot { w, h, bgra }, &p).is_ok(),
            "测试用的底图应该能写盘"
        );

        let _com = ComGuard::new();
        let factory: IWICImagingFactory = unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .expect("WIC 工厂")
        };
        let shot = decode_image_file(&factory, &p.to_string_lossy()).expect("应该解得出来");

        assert_eq!(shot.w, THUMB_MAX_W, "宽图应被缩到 THUMB_MAX_W");
        assert_eq!(shot.h, 500 * THUMB_MAX_W / 1000, "高按同比例缩");
        assert_eq!(&shot.bgra[..4], &[200u8, 100, 40, 255], "解码不该改变颜色");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 给图片**文件**补预览：落到 `thumbs/`，且**不存原件**（否则跟 `images/` 重复占盘）。
    #[test]
    fn file_thumb_lands_on_disk_and_never_keeps_a_second_original() {
        let dir = std::env::temp_dir().join("curseen_test_filethumb");
        let _ = std::fs::create_dir_all(&dir);
        let thumbs = std::env::temp_dir().join("curseen_test_filethumb_thumbs");
        let _ = std::fs::create_dir_all(&thumbs);
        let (w, h) = (1600i32, 900i32);
        let bgra = std::iter::repeat_n([30u8, 90, 180, 255], (w * h) as usize)
            .flatten()
            .collect::<Vec<u8>>();
        let p = dir.join("pic.png");
        assert!(save_png(&Shot { w, h, bgra }, &p).is_ok());

        let _com = ComGuard::new();
        let factory: IWICImagingFactory = unsafe {
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                .expect("WIC 工厂")
        };
        let cap = capture_file_thumb_bg(Some(&factory), &p, &thumbs).expect("应产出预览图");

        // ★ 这一路是"引用原文件"，所以**绝不该**再多存一份全尺寸原件。
        assert_eq!(cap.full, None, "文件条目多了 Image 那份原件就白占一份磁盘");
        assert_eq!(cap.w, THUMB_MAX_W, "预览宽不应超过缩略图上限");
        // 落盘的文件能被读回来，且尺寸与内存里的那份一致。
        let back = shot_from_png(&from_app_relative(&cap.rel)).expect("应能读回预览图");
        assert_eq!((back.w, back.h), (cap.w, cap.h));

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&thumbs);
    }

    /// 白名单的唯一职责：挡住明显不是图片的文件，别让 worker 白跑一次 IO。
    /// 能不能真解出来不是它的活——那是 WIC 的事（缺解码器时静默失败）。
    #[test]
    fn image_ext_gate_is_for_skipping_the_obvious_only() {
        for p in [
            "a.png",
            "A.JPG",
            "res/a.jpeg",
            "x.bmp",
            "y.gif",
            "z.webp",
            "n.tif",
            "q.tiff",
            "i.ico",
            "k.heic",
            "m.jfif",
            "v.avif",
        ] {
            assert!(is_image_ext(p), "{p} 应判为图片");
        }
        for p in [
            "setup.exe",
            "pack.zip",
            "movie.mp4",
            "noext",
            "a.png.exe",
            "a.exe.png.txt",
            "C:\\Users\\me\\Desktop\\报告.docx",
        ] {
            assert!(!is_image_ext(p), "{p} 不该判为图片");
        }
    }

    #[test]
    fn thumbnail_scales_down_and_preserves_small() {
        let shot = Shot {
            w: 1920,
            h: 1080,
            bgra: vec![0u8; (1920 * 1080 * 4) as usize],
        };
        let t = thumbnail_of(&shot, 720);
        assert!(t.w <= 720);
        assert!(t.w > 0 && t.h > 0);
        // 小图不放大
        let small = Shot {
            w: 100,
            h: 100,
            bgra: vec![0u8; 100 * 100 * 4],
        };
        let t2 = thumbnail_of(&small, 720);
        assert_eq!(t2.w, 100);
    }

    #[test]
    fn eviction_keeps_favorites() {
        let mut mgr = ClipboardMgr::new();
        mgr.cfg.max_entries = 3;
        for i in 0..10 {
            mgr.push(
                Kind::Text,
                format!("t{i}"),
                vec![],
                None,
                0,
                0,
                None,
                None,
                None,
            );
        }
        // 全部非收藏：应只留 3 条
        assert_eq!(mgr.entries.len(), 3);
        // 收藏最旧的一条后，再插入应保留它
        mgr.entries[2].fav = true;
        let before = mgr.entries[2].text.clone();
        mgr.push(
            Kind::Text,
            "new".into(),
            vec![],
            None,
            0,
            0,
            None,
            None,
            None,
        );
        assert_eq!(mgr.entries.len(), 3);
        assert!(mgr.entries.iter().any(|e| e.text == before));
    }

    #[test]
    fn dedup_floats_and_updates_ts() {
        let mut mgr = ClipboardMgr::new();
        mgr.cfg.max_entries = 10;
        mgr.push(
            Kind::Text,
            "dup".into(),
            vec![],
            None,
            0,
            0,
            None,
            None,
            None,
        );
        let first_id = mgr.entries[0].id;
        mgr.push(
            Kind::Text,
            "dup".into(),
            vec![],
            None,
            0,
            0,
            None,
            None,
            None,
        );
        // 不新增，仍 1 条，且位置在最前、id 不变
        assert_eq!(mgr.entries.len(), 1);
        assert_eq!(mgr.entries[0].id, first_id);
    }

    #[test]
    fn once_lock_is_send_friendly() {
        // 静态入口可被多次调用而不 panic（幂等）。
        ensure_started();
    }

    #[test]
    fn time_format_matches_known_timestamps() {
        // 2026-09-24 00:00:00 UTC = 1_790_208_000（2026-01-01 = 1_767_225_600 + 266 天）。
        assert_eq!(fmt_time_hm(1_790_208_000), "09-24 00:00");
        assert_eq!(
            fmt_time_hm(1_790_208_000 + 12 * 3600 + 34 * 60),
            "09-24 12:34"
        );
        // 纪元起点
        assert_eq!(fmt_time_hm(0), "01-01 00:00");
    }

    #[test]
    fn human_bytes_buckets() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(20 * 1024), "20 KB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn panel_state_parses_and_falls_back() {
        assert_eq!(
            parse_panel_state(r#"{"x":100,"y":-5,"pinned":true}"#),
            (Some((100, -5)), true)
        );
        assert_eq!(parse_panel_state("garbage"), (None, false));
        assert_eq!(parse_panel_state("{}"), (None, false));
    }

    /// 本文件**生产代码**文本（`#[cfg(test)]` 之前那一段）——结构性门禁用。
    /// 同款先例见 `ui::pages::other::tests::production_src`。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("clipboard_history.rs");
        // ★★ 必须钉到**测试模块**那一个 `#[cfg(test)]`：文件中间还有别的 `#[cfg(test)]`
        //    （`fmt_time_hm` 只有测试在用）。按第一次出现切会把大半生产代码切掉，门禁
        //    变成"只检查开头 565 行"——表面全绿，实际一条都没卡住。
        let cut = SRC
            .find("mod tests {")
            .and_then(|i| SRC[..i].rfind("#[cfg(test)]"))
            .unwrap_or(SRC.len());
        &SRC[..cut]
    }

    #[test]
    fn text16_never_carries_the_nul_terminator() {
        // ★ DrawTextW 拿切片长度当字数：`wide()` 的结尾 NUL 会被当字符画出来（豆腐块），
        //   顶栏四个按钮 + 垃圾桶右侧那个长方形就是它（2026-09-24 用户报）。
        assert_eq!(wide("✕"), vec![0x2715, 0]);
        assert_eq!(text16("✕"), vec![0x2715]);
        assert_eq!(text16(""), Vec::<u16>::new());
        // 非 BMP 字形（📌 🗑 🔍）是代理对：长度必须正好 2，多一位就多一个方块。
        assert_eq!(text16("\u{1F4CC}").len(), 2);
        assert_eq!(text16("\u{1F5D1}").len(), 2);
    }

    #[test]
    fn tabs_follow_the_user_order() {
        let labels: Vec<&str> = TABS.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, vec!["全部", "文本", "链接", "图片", "文件", "★"]);
        assert!(TABS[0].1 == TabKind::All, "第一个页签必须是「全部」");
        assert!(TABS[5].1 == TabKind::Fav, "最后一个页签必须是「★收藏」");
    }

    #[test]
    fn content_line_is_the_second_and_last_line() {
        let t = entry(Kind::Text, "第一行\n第二行", &[], 0, 0, 1);
        assert_eq!(content_line(&t), "第一行");
        let l = entry(Kind::Link, "https://x.com/a", &[], 0, 0, 2);
        assert_eq!(content_line(&l), "https://x.com/a");
        let one = entry(Kind::File, "", &[r"C:\dir\a.txt"], 0, 0, 3);
        assert_eq!(content_line(&one), "a.txt");
        let many = entry(Kind::File, "", &[r"C:\dir\a.txt", r"D:\b\bb.doc"], 0, 0, 4);
        assert_eq!(content_line(&many), "a.txt 等 2 项");
        let img = entry(Kind::Image, "", &[], 1920, 1080, 5);
        assert_eq!(content_line(&img), "图片");
        // ★ 用户点名：第三行（"N 字符"）不要。
        assert!(!content_line(&t).contains("字符"));
    }

    #[test]
    fn meta_line_labels_kind_without_repeating_line_two() {
        let t = entry(Kind::Text, "hello", &[], 0, 0, 1);
        assert!(meta_line(&t).ends_with(" · 5 B"), "{}", meta_line(&t));
        let f = entry(Kind::File, "", &[r"C:\a\b.txt"], 0, 0, 2);
        assert!(meta_line(&f).ends_with(" · 文件"), "{}", meta_line(&f));
        let img = entry(Kind::Image, "", &[], 800, 600, 3);
        assert!(
            meta_line(&img).ends_with(" · 800×600"),
            "{}",
            meta_line(&img)
        );
        // ★ 两行不许说同一件事：尺寸归第一行，第二行只报类型。
        assert_ne!(meta_line(&img), content_line(&img));
    }

    #[test]
    fn file_name_of_splits_on_both_separators() {
        assert_eq!(file_name_of(r"C:\a\b\file.txt"), "file.txt");
        assert_eq!(file_name_of("/mnt/x/file.txt"), "file.txt");
        assert_eq!(file_name_of("plain.txt"), "plain.txt");
        // 中文路径：按字节找分隔符，绝不能切在多字节字符中间。
        assert_eq!(file_name_of("C:\\目录\\文件.txt"), "文件.txt");
    }

    #[test]
    fn scroll_thumb_maps_scroll_onto_the_track() {
        let t = scroll_track();
        let track_h = t.bottom - t.top;
        // 不满一屏 → 铺满整条（视觉上"没什么可滚的"）
        assert_eq!(scroll_thumb(5, 11, 0), (t.top, track_h));
        // 滚到顶 / 滚到底：滑块贴住轨道两端
        let (y0, h0) = scroll_thumb(100, 11, 0);
        let (y1, h1) = scroll_thumb(100, 11, 89);
        assert_eq!(y0, t.top);
        assert_eq!(y1 + h1, t.bottom);
        assert!(h0 >= 16 && h0 < track_h, "滑块要有最小高度且不撑满：h={h0}");
        // 越界夹取：不许滑出轨道
        let (y2, h2) = scroll_thumb(100, 11, 9999);
        assert!(y2 + h2 <= t.bottom);
    }

    #[test]
    fn layout_shares_one_gap_and_rows_fit_the_panel() {
        // 顶栏 → 页签 → 列表首尾相接，间隔全是同一个 GAP。
        assert_eq!(TAB_Y, TOP_B + GAP);
        assert_eq!(LIST_TOP, TAB_Y + TAB_H + GAP);
        assert_eq!(ROW_H, ITEM_H - GAP);
        let rows = visible_rows();
        assert!(rows >= 8, "一屏至少 8 行，实际 {rows}");
        assert!(LIST_TOP + rows * ITEM_H <= PANEL_H - GAP);
        // 列表给滚动条让出那一列
        assert!(list_right() < PANEL_W - GAP);
        assert_eq!(scroll_track().right, PANEL_W - GAP);
    }

    #[test]
    fn two_line_card_and_no_tofu_glyphs_are_locked_in() {
        let p = production_src();
        // 旧版 `summary()` 返回 ("…", format!("{} 字符", n))，被画成第三行。
        // 只钉这个**拼串**——"字符"两个字在注释里到处都有，整词卡会误伤自己的注释。
        assert!(
            !p.contains("{} 字符"),
            "记录卡片只有两行：不得再拼「N 字符」当第三行"
        );
        assert!(!p.contains("fn summary("), "三行摘要函数必须移除");
        assert!(!p.contains("kind_letter"), "T/F/I/L 字母块图标必须移除");
        assert!(
            !p.contains("&mut wide("),
            "DrawTextW 一律走 text16()：wide() 的结尾 NUL 会被画成豆腐块"
        );
    }

    /// 把 [`draw_kind_icon`] 画进**内存位图**再读回像素——真像素判据，不是"看代码像"。
    /// 返回 `ICON_W × ICON_W` 的布尔矩阵（`true` = 这一格有墨）。
    fn icon_ink(kind: Kind, folder: bool) -> Vec<Vec<bool>> {
        unsafe {
            // ★ 拿屏幕 DC 当基准：`CreateCompatibleDC(None)` 出来的是 1bpp 单色 DC，
            //   位图也跟着变单色，读回的像素没法当判据。
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, ICON_W, ICON_W);
            let _ = ReleaseDC(None, screen);
            let old_bmp = SelectObject(dc, HGDIOBJ(bmp.0));
            // 白底：墨色画纯黑，判据就是"非白即墨"。
            let white = CreateSolidBrush(RGB(255, 255, 255));
            let whole = RECT {
                left: 0,
                top: 0,
                right: ICON_W,
                bottom: ICON_W,
            };
            FillRect(dc, &whole, white);
            let _ = DeleteObject(HGDIOBJ(white.0));
            draw_kind_icon(dc, 0, 0, kind, folder, RGB(0, 0, 0), RGB(255, 255, 255));

            // 读回 32bpp；`biHeight` 取负 = 自上而下（省掉一次倒序翻转）。
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: ICON_W,
                    biHeight: -ICON_W,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u32; (ICON_W * ICON_W) as usize];
            let n = GetDIBits(
                dc,
                bmp,
                0,
                ICON_W as u32,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi,
                DIB_RGB_COLORS,
            );
            SelectObject(dc, old_bmp);
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(dc);
            assert_eq!(n, ICON_W, "GetDIBits 应读回 {ICON_W} 行");
            // ★ 32bpp DIB 每像素是 B,G,R,**保留字节**（保留字节恒为 0），所以白点是
            //   `0x00FF_FFFF` 而不是 `0xFFFF_FFFF`——按四字节整体比会把整幅图都判成有墨。
            (0..ICON_W)
                .map(|y| {
                    (0..ICON_W)
                        .map(|x| buf[(y * ICON_W + x) as usize] & 0x00FF_FFFF != 0x00FF_FFFF)
                        .collect()
                })
                .collect()
        }
    }

    #[test]
    fn link_icon_is_a_globe_not_a_chain() {
        // ★ 用户第 4 点："链接类图标太丑陋"。旧版是"两个环 + 中间一横"的链环，24px 下
        //   两个小环挤成一团；换成地球仪（外圈 + 赤道 + 子午线）后，判据就是：
        //   ① 正中那一横没了（链环的心脏就是那一横）；② 上下极点有墨（链环到不了）。
        let g = icon_ink(Kind::Link, false);
        let row_ink = |y: usize| g[y].iter().filter(|&&b| b).count();
        // ★ 判据取"这一行里**最长的一段连续墨**"：地球仪在中行要穿过 6 条边（外圈左右
        //    + 赤道左右 + 子午线左右），总墨数天然就有 8 格上下，但每一段都只有 1~2px。
        //   旧链环恰恰相反——它用 `fill_rect(x+4,y+11,x+20,y+13)` 铺了一条**不断开**的
        //   横杠。所以看最长连续段，而不是总格数。
        let longest_run = |y: usize| {
            let (mut best, mut cur) = (0usize, 0usize);
            for &b in &g[y] {
                if b {
                    cur += 1;
                    best = best.max(cur);
                } else {
                    cur = 0;
                }
            }
            best
        };
        // ★ 只卡**正中那一行**（y=12）：赤道椭圆的上/下缘（y=11 / y=13）本来就是两条
        //   近水平的弧，出现长连续墨是地球仪该有的样子；而 y=12 是赤道的左右极点，
        //   只剩几个点——旧链环正是在这一行铺了一条不断开的横杠。
        assert!(
            longest_run(12) <= 3,
            "链接图标正中不该还有一整条横杠：最长连续 {} 格",
            longest_run(12)
        );
        // ② 子午线上下贯通（y+3 .. y+21）：旧链环的墨只落在 7..18，顶底两行是空的。
        assert!(row_ink(4) > 0, "地球仪顶极要有墨（旧链环到不了 y=4）");
        assert!(row_ink(20) > 0, "地球仪底极要有墨（旧链环到不了 y=20）");
        // ③ 左右极点（赤道两端与外圈相切）
        assert!(g[12][3] || g[12][4], "赤道左端要有墨");
        assert!(g[12][19] || g[12][20], "赤道右端要有墨");
    }

    #[test]
    fn top_btn_at_hits_the_three_buttons_and_only_them() {
        // 自右往左：0=关闭 1=收起 2=置顶
        let y = TOP_Y + TOP_H / 2;
        assert_eq!(top_btn_at(PANEL_W - GAP - 1, y), Some(0));
        assert_eq!(top_btn_at(PANEL_W - GAP - BTN_W - 1, y), Some(1));
        assert_eq!(top_btn_at(PANEL_W - GAP - BTN_W * 2 - 1, y), Some(2));
        // 三颗按钮左侧（搜索框 / 计数区）与上下越界 → 都不算按钮
        assert_eq!(top_btn_at(PANEL_W - GAP - BTN_W * 3 - 1, y), None);
        assert_eq!(top_btn_at(PANEL_W - GAP - 1, TOP_Y - 1), None);
        assert_eq!(top_btn_at(PANEL_W - GAP - 1, TOP_B + 1), None);
    }

    #[test]
    fn count_text_shows_total_and_filtered() {
        // 没筛选（命中 == 总数）→ 只报总数
        assert_eq!(count_text(128, 128), "128 条");
        assert_eq!(count_text(0, 0), "0 条");
        // ★ 用户第 7 点：正在筛选时也要看得到总数，所以是"命中 / 总数"。
        assert_eq!(count_text(128, 3), "3 / 128");
        assert_eq!(count_text(128, 0), "0 / 128");
    }

    #[test]
    fn long_counts_are_compacted_so_they_stay_short() {
        // ★ 用户第 2 点："假如以后有 123454 条记录，能装得下吗"。
        // 计数区宽度是固定的 → 数字本身必须有上界：过万收「万」，百万往上封顶。
        assert_eq!(compact_num(9_999), "9999");
        assert_eq!(compact_num(10_000), "1万");
        assert_eq!(compact_num(123_454), "12万");
        assert_eq!(compact_num(999_999), "99万");
        assert_eq!(compact_num(1_000_000), "99万+");
        assert_eq!(compact_num(usize::MAX), "99万+");
        // 组合起来也一样有界
        assert_eq!(count_text(123_454, 123_454), "12万 条");
        assert_eq!(count_text(123_454, 137), "137 / 12万");
        assert_eq!(count_text(usize::MAX, 7), "7 / 99万+");
    }

    /// 用 `DT_CALCRECT` **实量**一段文本在指定字号下的像素宽度（真度量，不是估的）。
    fn text_w(px: i32, s: &str) -> i32 {
        unsafe {
            // 同 [`icon_ink`]：必须以屏幕 DC 为基准，否则拿到 1bpp 单色 DC。
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let _ = ReleaseDC(None, screen);
            let f = make_ui_font(px);
            let old = SelectObject(dc, HGDIOBJ(f.0));
            let mut buf = text16(s);
            let mut r = RECT::default();
            let _ = DrawTextW(dc, &mut buf, &mut r, DT_CALCRECT | DT_SINGLELINE);
            SelectObject(dc, old);
            let _ = DeleteObject(HGDIOBJ(f.0));
            let _ = DeleteDC(dc);
            r.right - r.left
        }
    }

    /// ★ 告知条**借用**计数那条槽位（见 [`warn_bytes_text`]），所以它同样受那条宽度的约束。
    /// 溢出会把字画到三颗按钮上去，而这里并没有 `DT_END_ELLIPSIS` 之外的兜底。
    #[test]
    fn warn_text_fits_the_count_slot_it_borrows() {
        let slot = count_rect().right - count_rect().left;
        for bytes in [
            IMG_DIR_WARN_BYTES,
            IMG_DIR_WARN_BYTES + 512 * 1024 * 1024,
            10 * 1024 * 1024 * 1024,
            99 * 1024 * 1024 * 1024,
        ] {
            let s = warn_bytes_text(bytes);
            let w = text_w(14, &s);
            assert!(
                w <= slot,
                "告知条「{s}」在 14px 下宽 {w}px，超出借用来的计数区宽度 {slot}px"
            );
        }
    }

    #[test]
    fn warn_bytes_text_speaks_in_the_users_own_unit() {
        // 用户口径是"到 5G 的时候就提示"，所以满 GB 就说 GB。整数即可——告知看的是量级，
        // 而每位小数都要多占约 7px（见上面那条宽度门禁）。
        assert_eq!(warn_bytes_text(IMG_DIR_WARN_BYTES), "已占 5 GB");
        assert_eq!(warn_bytes_text(10 * 1024 * 1024 * 1024), "已占 10 GB");
        // 没到 GB 就退回 MB（告知线之下本来也不会显示，这里是纯函数自己的完备性）。
        assert_eq!(warn_bytes_text(700 * 1024 * 1024), "已占 700 MB");
    }

    /// ★ 源文件失效告知同样借用计数槽位，宽度必须实量放下（同上条门禁的理由）。
    #[test]
    fn notice_text_fits_the_count_slot() {
        let slot = count_rect().right - count_rect().left;
        let s = notice_text(&TopNotice::SrcMissing);
        let w = text_w(14, &s);
        assert!(
            w <= slot,
            "告知条「{s}」在 14px 下宽 {w}px，超出借用来的计数区宽度 {slot}px"
        );
    }

    /// ★ 判"全部失效"才标行：一批里还有一条能落，粘贴照常（只写存活的）。
    #[test]
    fn files_all_missing_requires_every_path_gone() {
        // 全是不存在的路径 → 失效
        let gone = std::env::temp_dir()
            .join(format!("curseen_no_such_{}", std::process::id()))
            .to_string_lossy()
            .to_string();
        assert!(files_all_missing(std::slice::from_ref(&gone)));
        assert!(files_all_missing(&[gone.clone(), gone]));
        // 空列表不算失效（没有引用就没有"失效"可言）
        assert!(!files_all_missing(&[]));
        // 一条还在 → 不算失效
        let dir = std::env::temp_dir().join(format!("curseen_miss_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let alive = dir.join("a.txt");
        let _ = std::fs::write(&alive, b"x");
        assert!(!files_all_missing(&[
            alive.to_string_lossy().to_string(),
            format!("{}.gone", alive.to_string_lossy())
        ]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ★ `missing` 是运行态：**绝不落盘**。U 盘插回来它就得自己翻假，
    ///   序列化把它钉死等于把暂时状态写成永久判决。
    #[test]
    fn missing_flag_is_runtime_only_never_persisted() {
        let mut e = entry(Kind::File, "", &["C:\\\\no\\\\such\\\\a.txt"], 0, 0, 1);
        e.missing = true;
        let json = serde_json::to_string(&e).expect("应可序列化");
        assert!(!json.contains("missing"), "missing 不该进 JSON：{json}");
        let back: ClipEntry = serde_json::from_str(&json).expect("应可反序列化");
        assert!(!back.missing);
    }

    /// ★ 捕获重试状态机：只有「有格式却没读到」（别的监听程序握着剪贴板）才重试，
    ///   至多 [`CAP_RETRY_MAX`] 次后放弃；成功或剪贴板上没内容一律归零不再定时。
    #[test]
    fn capture_retry_only_for_unread_wanted_content() {
        assert_eq!(retry_decision(true, true, 0), (0, false));
        assert_eq!(retry_decision(true, false, 5), (0, false));
        assert_eq!(retry_decision(false, false, 0), (0, false));
        assert_eq!(retry_decision(false, true, 0), (1, true));
        assert_eq!(retry_decision(false, true, 1), (2, true));
        assert_eq!(retry_decision(false, true, CAP_RETRY_MAX), (0, false));
        assert_eq!(retry_decision(false, true, 9), (0, false));
    }

    /// ★ 源程序 opt-out 判据：任一标记格式在剪贴板上即请求不收录；注册失败留下的
    ///   0 号 id 必须被跳过（0 落在标准格式域，拿去问会误命中）。
    #[test]
    fn optout_marker_skips_capture_and_zero_ids_are_ignored() {
        let hit = |f: u32| f == 7;
        assert!(optout_requested(&[7, 0], &hit));
        assert!(optout_requested(&[0, 7], &hit));
        assert!(!optout_requested(&[0, 0], &hit), "全 0（注册失败）不得命中");
        assert!(!optout_requested(&[3, 4], &hit), "标记不在场不得命中");
        assert!(
            !optout_requested(&[0, 0], &|f: u32| f == 0),
            "0 号 id 即便\"存在\"也不算"
        );
    }

    /// ★ 源文件失效时第一行直接说原因，第二行只换色不加字（行高布局一概不动）。
    #[test]
    fn meta_line_says_why_when_source_is_gone() {
        let mut f = entry(Kind::File, "", &["C:\\no\\such.txt"], 0, 0, 1);
        assert!(meta_line(&f).ends_with(" · 文件"), "{}", meta_line(&f));
        f.missing = true;
        assert!(meta_line(&f).contains("源文件已不在"), "{}", meta_line(&f));
    }

    /// ★ 钉住 `clear` 的保留判据不再从文件名倒推条目 id。
    ///
    /// 缩略图与原件都按**内容哈希**命名（十六进制），`file_stem().parse::<u64>()` 对含
    /// `a–f` 的名字恒失败 ⇒ 旧实现据此判"不保留"，把收藏条目的图也一并删了。这个错
    /// 安静得可怕：注释写着"收藏原样保留"，代码做的是相反的事，只有真去清空才会发现。
    #[test]
    fn clear_keeps_files_by_name_not_by_guessing_the_id() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/platform/clipboard_history.rs"
        ))
        .expect("源码应可读");
        // `texts/` 旧版从未被清理，新增的 `images/` 会重蹈同一处泄漏。
        let clear_body = src
            .split("fn clear(&mut self)")
            .nth(1)
            .and_then(|s| s.split("\n    fn ").next())
            .expect("应找得到 `clear` 的函数体");
        // ★ 只判**代码**：上面那段注释里引述了这个历史判据，`contains` 粗判会被自己
        //   的注释误伤。本条要锁的是"倒推 id 的代码不得回来"，不是"不许提这段历史"。
        let clear_code: String = clear_body
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !clear_code.contains("parse::<u64>()"),
            "clear 不该再从文件名倒推条目 id：哈希名含 a–f 时该 parse 恒失败，\
             会把收藏条目的图也删掉"
        );
        // 两个目录必须一起理：`texts/` 旧版从未被清理，新增的 `images/` 会重蹈同一处泄漏。
        assert!(
            clear_body.contains("self.images_dir"),
            "清空必须一并理 images/"
        );
        assert!(
            clear_body.contains("self.texts_dir"),
            "清空必须一并理 texts/"
        );
    }

    #[test]
    fn count_fits_its_slot_even_when_the_number_gets_long() {
        // ★ 用户第 2 点的硬判据：**真去量**每条计数在 14px 字号下的像素宽度，
        // 必须塞得进"搜索框右边界 → 三颗按钮"那一条，否则又会去挤按钮边框。
        let slot = count_rect().right - count_rect().left;
        for (total, shown) in [
            (0usize, 0usize),
            (500, 500),
            (2_000, 2_000),
            (2_000, 1_377),
            (9_999, 9_998),
            (123_454, 123_454),
            (123_454, 12_345),
            (999_999, 999_998),
            (usize::MAX, usize::MAX),
            (usize::MAX, 1),
        ] {
            let s = count_text(total, shown);
            // 实测参考（14px 字号，计数区 94px）：最宽的是「9998 / 9999」= 78px，
            // 余量 16px；「12万 条」= 48px、「99万 / 99万」= 74px。
            let w = text_w(14, &s);
            assert!(
                w <= slot,
                "计数「{s}」宽 {w}px 超出计数区 {slot}px（会挤到按钮上）"
            );
        }
    }

    /// 排版常量关系护栏：搜索框固定宽、计数区占满剩余、余量下限等——
    /// 断言对象多为 const 间关系，clippy 的 assertions_on_constants 在此属误报，整测豁免。
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn search_box_is_fixed_and_the_count_owns_the_rest() {
        // ★★ 用户第 2 点定案的排版：搜索框**固定** `EDIT_W`，剩下整条全归计数。
        assert_eq!(edit_right() - edit_left(), EDIT_W);
        assert_eq!(count_rect().left, edit_right());
        assert_eq!(count_rect().right, PANEL_W - GAP - BTN_W * 3);
        // 计数区得有足够余量放得下最长的计数（具体数值由上面的实测判据兜底）。
        assert!(count_rect().right - count_rect().left >= 60, "计数区太窄了");
        // 搜索框也不能被挤到没法输入。
        assert!(EDIT_W >= 80, "搜索框还得留得下输入：{EDIT_W}");
        // ★ 计数**水平居中**：源码里必须是 DT_CENTER（旧版 DT_RIGHT 就是被用户
        //   点名"贴着按钮边框"的元凶）。
        let p = production_src();
        assert!(
            p.contains("DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS"),
            "计数必须水平居中（DT_CENTER）"
        );
        // ★ 用户第 3 点：顶栏内部**零内边距**——按钮与搜索框都跟顶栏同高，于是"顶栏
        //   上下那一圈"只剩外面那个 GAP，跟条目之间的间隔一模一样。
        assert_eq!(SEARCH_BOX_H, TOP_H);
        assert_eq!(TOP_Y, GAP);
    }

    #[test]
    fn search_text_is_pushed_to_the_vertical_center() {
        // ★ 用户第 3 点："搜索输入框文本不是垂直居中（Windows 固有缺陷）"。
        //   判据是格式化矩形的上下留白：顶/底各 `(28 - line_h)/2`，整行再 **下移 1px**
        //   补偿 ascent>descent（`EDIT_VCENTER_NUDGE`）。
        let w = EDIT_W;
        let line_h = 21; // Segoe UI 16px 的 tmHeight
        let r = edit_format_rect(w, line_h);
        let top_pad = r.top;
        let bottom_pad = SEARCH_BOX_H - r.bottom;
        assert_eq!(r.bottom - r.top, line_h, "格式化矩形正好一行高");
        assert!(
            (top_pad - bottom_pad).abs() <= 2,
            "上下留白必须差不多：top={top_pad} bottom={bottom_pad}"
        );
        assert_eq!(top_pad, (SEARCH_BOX_H - line_h) / 2 + EDIT_VCENTER_NUDGE);
        // 左右内边距（不然光标顶着框边）
        assert_eq!(r.left, EDIT_PAD_X);
        assert_eq!(r.right, w - EDIT_PAD_X);
        // ★★ `EM_SETRECT` 只对**多行** EDIT 生效（单行控件直接忽略它），所以搜索框
        //   必须建的是多行 EDIT，且不能带 `ES_WANTRETURN`（回车要留给粘贴）。
        let p = production_src();
        assert!(
            p.contains("ES_MULTILINE as u32 | ES_AUTOHSCROLL as u32"),
            "搜索框必须是多行 EDIT，否则 EM_SETRECT 无效"
        );
        // ★ 只卡**样式表达式**里那一项：`ES_WANTRETURN` 这串字在注释里也有（就是上面
        //   那段解释），整词匹配会变成假红。
        assert!(
            !p.contains("| ES_WANTRETURN as u32"),
            "回车不能被 EDIT 吃掉（ES_WANTRETURN 会让多行 EDIT 吞掉回车）"
        );
        assert!(p.contains("EM_SETRECT"), "必须发 EM_SETRECT 摁住文本行");
    }

    #[test]
    fn panel_has_a_one_pixel_border() {
        // ★ 用户第 5 点："是否需要更好的边框线？或者来个 1px 纯黑边框线？"
        //   定案：1px，但用**深灰** `C_BORDER` 而不是纯黑——面板底是 240 的浅灰，纯黑
        //   太跳，也跟内部那条 `C_LINE` 抢戏；深灰比内线更深一档，层级是"外框 > 内线"。
        let lum = |c: (u8, u8, u8)| 0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32;
        assert!(
            lum(C_BORDER) < lum(C_LINE) - 30.0,
            "外框必须比内部分隔线更深：{C_BORDER:?} vs {C_LINE:?}"
        );
        assert!(lum(C_BORDER) > 60.0, "外框不用纯黑：{C_BORDER:?}");
        // 画在最前面（列表段会收裁剪区，收完再画就只剩列表那一块）。
        let p = production_src();
        assert!(
            p.contains("FrameRect(hdc, &bg, self.border_brush)"),
            "必须有外框"
        );
    }

    #[test]
    fn light_theme_is_locked_in() {
        // ★ 用户第 5 点："背景要浅灰色，文字和标题栏图标颜色等要偏暗色"。
        // 判据用**亮度**而不是具体色号——以后微调色号不至于把门禁撞倒。
        let lum = |c: (u8, u8, u8)| 0.299 * c.0 as f32 + 0.587 * c.1 as f32 + 0.114 * c.2 as f32;
        assert!(lum(C_BG) > 200.0, "面板底必须是浅灰：{C_BG:?}");
        assert!(lum(C_TEXT) < 90.0, "正文必须偏暗：{C_TEXT:?}");
        assert!(lum(C_TEXT_DIM) < 160.0, "次要说明也偏暗：{C_TEXT_DIM:?}");
        assert!(lum(C_ICON) < 140.0, "顶栏图标偏暗：{C_ICON:?}");
        // 四档必须拉得开：旧版 hover 与 bg 只差 13/255，用户直接报"没有 hover 变色"。
        assert!((lum(C_BG) - lum(C_ROW)).abs() > 3.0, "底色与行底太接近");
        assert!((lum(C_ROW) - lum(C_HOVER)).abs() > 8.0, "悬停看不出来");
        assert!((lum(C_HOVER) - lum(C_SEL)).abs() > 8.0, "选中看不出来");
        // 悬停底纹现算（不写死色号）：既要看得出来，又不能压成一坨黑。
        // 旧版 hover 与 bg 只差 13/255，用户直接报"没有 hover 变色"。
        let drop = lum(C_BG) - lum(mix(C_BG, C_TEXT, BTN_HOVER_MIX));
        assert!(drop > 12.0 && drop < 60.0, "悬停底纹压得不对：drop={drop}");
    }

    #[test]
    fn flicker_guards_are_locked_in() {
        // ★ 用户第 1 点："鼠标在界面移动经常闪烁"。根因是三件事叠在一起，逐条钉死。
        let p = production_src();
        // ① 吞掉 WM_ERASEBKGND："先擦白再重画"中间那几毫秒，屏幕上就是一闪。
        assert!(p.contains("WM_ERASEBKGND =>"), "必须吞掉 WM_ERASEBKGND");
        // ② 双缓冲：内存里拼完一帧再整块贴上去，不在窗口 DC 上逐趟画。
        assert!(p.contains("CreateCompatibleDC(Some(hdc))"), "必须有内存 DC");
        assert!(p.contains("BitBlt("), "必须一次性 BitBlt 上屏");
        // ③ 不再带 CS_HREDRAW|CS_VREDRAW（尺寸一变就白付一次全客户区重画）。
        assert!(
            !p.contains("style: CS_HREDRAW"),
            "CS_HREDRAW|CS_VREDRAW 会强制整客户区重画，收起/展开时会闪"
        );
        // ④ hover 只脏列表、按钮只脏顶栏，别把整窗（含原生 EDIT）一起拖下水。
        assert!(p.contains("fn invalidate_list("), "hover 变化只重画列表");
        assert!(p.contains("fn invalidate_top("), "按钮悬停只重画顶栏");
    }

    #[test]
    fn panel_is_narrower_now() {
        // ★ 用户第 6 点："面板宽度太宽，改小一点"（520 → 430）；
        //   第 4 点："再少一点点，300 差不多"（430 → **300**）。
        // 两侧都是常量 → 放 `const` 块里，编译期就钉死（顺便避开 clippy 的
        // `assertions_on_constants`：运行期再判毫无意义）。
        const {
            assert!(PANEL_W <= 320, "面板宽度应已收窄（旧版 520 → 430 → 300）");
        }
        // 高度按"10 行 + 四边 GAP"倒推，底部不留半行的空档。
        assert_eq!(PANEL_H, LIST_TOP + GAP + 10 * ITEM_H);
        assert_eq!(visible_rows(), 10);
    }

    /// 置顶按钮那颗**向上箭头**：必须抗锯齿、必须朝上、四角必须干净。
    ///
    /// 这条锁的是用户"向上的箭头全是锯齿"这条反馈——换成图钉字形（📌）被明确否掉，
    /// 换成 GDI `Polygon` 又回到零抗锯齿，所以只能用超采样掩码；掩码一旦被改坏
    /// （比如三角头写反变成朝下、或退化成二值硬边），这里立刻红。
    #[test]
    fn up_arrow_mask_is_antialiased_and_points_up() {
        let n = ACT_W; // 与真实按钮同尺寸，判据才有意义
        let m = arrow_up_mask(n);
        assert_eq!(m.len(), (n * n) as usize);
        // ① 抗锯齿：必须有相当数量的中间灰阶（"没有锯齿"的可测判据）
        let aa = m.iter().filter(|&&v| v > 16 && v < 239).count();
        assert!(aa > 12, "边缘应有抗锯齿灰阶，实际只有 {aa} 个像素");
        // ② 实心：三角头内部与杆心都接近全实
        let cx = n / 2;
        assert!(
            m[(n / 3 * n + cx) as usize] > 200,
            "三角头内部应接近实心（用户要的是箭头，不是空心框）"
        );
        assert!(m[(n * 3 / 4 * n + cx) as usize] > 200, "杆心应接近实心");
        // ③ 四角必须干净（箭头不该糊满整格）
        for (x, y) in [(0, 0), (n - 1, 0), (0, n - 1), (n - 1, n - 1)] {
            assert_eq!(m[(y * n + x) as usize], 0, "({x},{y}) 应为空");
        }
        // ④ 朝上：上部的覆盖宽度必须大于下部
        let width = |row: i32| (0..n).filter(|&x| m[(row * n + x) as usize] > 128).count();
        let (top, bottom) = (width(n / 4), width(n * 4 / 5));
        assert!(
            top > bottom,
            "上部({top})应宽于下部({bottom})——箭头必须朝上"
        );
    }

    /// 箭头**不铺满按钮格**：2026-09-29 用户口径"向上箭头特别好，就是太大了"。
    ///
    /// 锁的是尺寸关系本身——图标必须明显小于按钮格（四边留余量），但又不能小到
    /// 看不清（16 已经是下限：再小三角头只剩三四个像素，抗锯齿反而糊成一团）。
    #[test]
    fn up_arrow_is_smaller_than_its_button_cell() {
        // 常量判据放 const 块里（clippy: assertions_on_constants），编译期就红。
        const {
            assert!(ARROW_PX < ACT_W, "箭头必须小于按钮格，否则又顶到格边");
            assert!(ARROW_PX >= 14, "箭头小于 14 就认不出形状了");
            // 两侧留白各 (ACT_W - ARROW_PX)/2 px，太小等于没缩。
            assert!(ACT_W - ARROW_PX >= 4, "缩进太小，看不出来");
        }
    }
}
