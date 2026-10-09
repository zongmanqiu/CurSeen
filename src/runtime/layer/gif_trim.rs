//! GIF 裁剪窗：**预览 + 时间轴 + 划选删除**。
//!
//! # 交互（一句话）
//!
//! **在时间轴上按住左键横向划过，划到哪就删到哪**；两个把手可以再微调；「保存」落盘。
//!
//! 这一个手势同时覆盖三种需求，不需要三套 UI：
//!
//! | 想干的事 | 手势 |
//! |---|---|
//! | 掐掉开头 | 从最左端往右划 |
//! | 去掉结尾 | 从最右端往左划 |
//! | 抠掉中间某段 | 在中间划 |
//!
//! # 为什么是自绘而不是 `Element` 拼装
//!
//! 与 `layer::region`（截图选框）同一个理由，再加一条更要命的：windui 的窗口内容**只在
//! 开窗时构建一次**，而这里的预览图、把手位置、摘要文字全都要跟着输入实时变。用 `Signal`
//! 去绑不是不能做，但"图片跟着拖动换"恰好是 `Element` 拼装最别扭的地方。自绘则一切都在
//! `paint()` 里现取现画。
//!
//! # 坐标与单位
//!
//! windui 里 `Rect` / `Point` 是 **i32**，而 `Canvas` 的绘制参数是 **f32**，事件落点也是
//! 整数。本模块内部一律用 f32 算（[`Box2`]），只在调用 `Canvas` 时经 [`irect`] 收成整数 ——
//! 免得同一份布局代码里到处夹 `as`。
//!
//! # 内存
//!
//! 只持有一张解码画布 + 一张预览图（见 `core::gif_trim` 的模块头）。预览图**只在帧号
//! 真的变了才重建**：鼠标横划一像素就重建一次，拖动会明显发涩。
//!
//! # 两个入口，一个窗口
//!
//! | 入口 | 打开方式 | 初始状态 |
//! |---|---|---|
//! | 控件条「裁剪」/ 刚录完 | [`open`] | 直接载入刚录的那份 GIF |
//! | 裁剪快捷键（`gif.trim_hotkey`） | [`open_blank`] | **空窗**，等用户按「打开…」挑文件 |
//!
//! 空窗不是"没做完"的状态，而是**裁剪器的正常入口**：用户想裁的往往不是刚录的那一段
//! （可能是上周的、别人发来的）。所以空窗有自己的灰轨道，四个按钮里只有「打开…」
//! 可用；提示文字一律不加（2026-09-30 用户定稿："标题栏有了就行"）。
//!
//! # 保存 = 另存为
//!
//! 「保存」弹原生另存为对话框（见 [`crate::platform::file_dialog::save_gif`]），结果写到
//! 用户挑的落点 —— **原片绝不被改动**（这条由 `core::gif_trim::rewrite_to` 兜底）。
//! 停止录制那一刻落盘的那份文件因此可以一直留着当底稿。
//!
//! # 窗口外壳：系统标题栏 + 可缩放
//!
//! 留着系统标题栏（`WS_OVERLAPPEDWINDOW`，即**不**设 `frameless`），因此自带最小化 / 最大化 /
//! 关闭与拖动，用户可以把它放大到看清画面（2026-09-30 用户报"裁剪窗没法放大"）。布局按
//! **当前客户区**算（见 [`layout`]），拉大时预览吃满剩余高度。
//!
//! # 这个窗**不进置顶带**（与控件条/选框正相反）
//!
//! 开窗后的聚焦走 [`win_util::focus_window_by_title`](crate::platform::win_util::focus_window_by_title)
//! 而不是 `activate_window_by_title`：后者会 `SetWindowPos(HWND_TOPMOST)` 把窗口搬进置顶带，
//! 于是它压在**所有**普通窗口之上 —— 包括本窗自己要弹的原生「打开 / 另存为」对话框。
//! 症状就是用户 2026-09-20 报的"点「打开…」时选框全被遮挡、根本没法挑文件"。
//!
//! 附带一个更隐蔽的坏处：置顶时仍点得到本窗，而 `ctx.defer_blocking` 的闭包是在**窗口过程
//! 内部**执行的、里面又会开一个带消息泵的模态对话框 ⇒ 每次点击都会在上一层模态循环里再套
//! 一层对话框。实测连点三次 = 三层看不见的对话框（日志里三条连续"用户在「打开」里取消"）。
//!
//! # 两道防线挡"对话框套对话框"
//!
//! | 防线 | 在哪 | 挡住什么 |
//! |---|---|---|
//! | `hwndOwner` = 本窗 | [`crate::platform::file_dialog`] 的 `owner` 参数 | 物理上：对话框模态于我们，点不到本窗 |
//! | [`TrimCore::dialog_open`] | [`TrimCanvas::pick_file`] / [`TrimCanvas::save`] 开头 | 逻辑上：同一时刻只允许一个对话框在飞 |
//!
//! 只留一道都不够：属主对系统只是**提示**，而模态循环里的事件次序不是我们能保证的。
//!
//! # 状态为什么在 `Rc<RefCell<_>>` 里
//!
//! 两个按钮（「打开…」「保存」）都要弹**原生模态对话框**，而它必须经 `ctx.defer_blocking`
//! 延后执行（对话框自带消息泵，从窗口过程里直接调就是重入，见 `platform::file_dialog`
//! 模块头）。那个闭包是 `FnOnce() + 'static` —— **拿不到 `&mut self`**。所以全部可变状态
//! 收进 [`TrimCore`] 并由 `Rc<RefCell<_>>` 持有，闭包与画布共享同一份；闭包改完之后靠
//! `win_util::invalidate_by_title` 把窗口按重画一次。这也顺带让 `TrimCore` 能脱离窗口单测。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use windui::core::{EventCtx, Widget};
use windui::event::{Event, Key, MouseButton, PointerKind};
use windui::geometry::{Color, Rect};
use windui::prelude::*;
use windui::render::image::{Fit, Image as WImage};
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::spec::Align;
use windui::text::{TextEngine, TextStyle};

use crate::core::gif_trim::{self, FrameReader};
use crate::runtime::Runtime;

/// 窗口名（标题栏里 `-` **右边**那半）。
///
/// 标题栏全文走 [`window_title`]：**文档名 - 软件名**，与记事本（「文档 - 记事本」）同一套
/// 口径（2026-09-30 用户口径）。★ 标题是动态的，而 `FindWindowW` 只认**完整标题**——所以
/// 本模块自己记一份当前标题（[`cur_title`]），按标题关窗 / 聚焦 / 重画都用它，不写本常量。
pub const TITLE: &str = "CurSeen 裁剪 GIF";

/// 标题栏全文：**文档名 - 软件名**（`file_name` 为空时按记事本的"无标题"惯例）。
fn window_title(file_name: &str) -> String {
    if file_name.is_empty() {
        format!("未打开文件 - {TITLE}")
    } else {
        format!("{file_name} - {TITLE}")
    }
}

thread_local! {
    /// 当前标题栏文本（见 [`TITLE`] 的注释：`FindWindowW` 只认完整标题）。
    static CUR_TITLE: RefCell<String> = RefCell::new(window_title(""));
}

/// 当前标题（按标题关窗 / 聚焦 / 重画都用它）。
fn cur_title() -> String {
    CUR_TITLE.with(|c| c.borrow().clone())
}

/// 记下新标题。**必须在 UI 线程调用**（工作线程读不到这份 thread_local）。
fn set_cur_title(file_name: &str) {
    let t = window_title(file_name);
    CUR_TITLE.with(|c| *c.borrow_mut() = t);
}

/// 直接写下标题栏全文（`paint` 用它把 `CUR_TITLE` 校准成活窗的标题）。
fn set_cur_title_raw(title: String) {
    CUR_TITLE.with(|c| {
        let mut g = c.borrow_mut();
        if *g != title {
            *g = title;
        }
    });
}

// ─────────────────────────── 布局常量（逻辑像素） ───────────────────────────

/// 初始客户区尺寸（逻辑 dp）。只是"第一次多大"——用户可以拉大，见模块头。
const WIN_W: i32 = 760;
const WIN_H: i32 = 520;
/// 最小客户区（交给 `.min_size()`）：再小两簇按钮会撞上、预览也不够看清画面。
///
/// 高度下限按"每块都按最小高度排一遍再补齐"算出来（含新增的刻度行），
/// 由 `preview_grows_with_the_window_and_layout_stays_inside` 钉住。
const MIN_W: i32 = 620;
const MIN_H: i32 = 350;
const PAD: f32 = 16.0;
/// 预览区高度**下限**（[`layout`] 里按剩余高度算，再小就取这个值）。
const PREVIEW_MIN_H: f32 = 140.0;
const SUMMARY_H: f32 = 20.0;
const GAP: f32 = 10.0;
/// 轨道（那条时间轴）的可点/可拖区域高度。视觉上的轨道比它细得多，见 [`TRACK_H`]。
const TRACK_HIT_H: f32 = 30.0;
const TRACK_H: f32 = 10.0;
/// 轨道下面的**时间刻度行**（"第 1 帧" / 总帧数）。
///
/// ★ 它必须**独立成一行**：刻度原本画在轨道命中框的末尾，而按钮顶边只比轨道底边低 6px，
///   16px 高的字直接压到按钮上（2026-09-30 用户报"数字快接触到按钮了"）。
const TICK_H: f32 = 16.0;
/// 刻度是轨道的注脚，贴得紧一点；它**与按钮**之间才用整 [`GAP`]。
const TICK_GAP: f32 = 2.0;
/// 把手宽度与命中半径。
const HANDLE_W: f32 = 4.0;
const HANDLE_HIT: f32 = 8.0;
const BUTTON_W: f32 = 104.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_GAP: f32 = 10.0;
const BUTTON_N: usize = 4;

/// 按钮文案，下标与 [`Layout::buttons`] 一一对应。
///
/// ★ 第 0 个（「打开…」）**左对齐**，其余三个右对齐 —— 它是**文件级**动作，另外三个是
///   **编辑级**动作。混在一排里会让人以为"打开"和"保存"是同一层级的对称操作；Windows
///   自己也习惯把"浏览 / 打开"放在左边（见 [`layout`]）。
/// ★ 不设「取消」：系统标题栏的 ✕ 就是取消（2026-09-30 用户定稿）。
const BUTTON_LABELS: [&str; BUTTON_N] = ["打开…", "删除此段", "撤销", "保存"];

const ACCENT: (u8, u8, u8) = (76, 139, 245);
const DANGER: (u8, u8, u8) = (226, 68, 60);
/// 已经确认删掉的区间：比待删的浅一档，避免与"待删"混淆。
const DANGER_DONE: (u8, u8, u8) = (206, 138, 134);
const TEXT: (u8, u8, u8) = (45, 52, 54);
const TEXT_DIM: (u8, u8, u8) = (99, 110, 114);
const BORDER: (u8, u8, u8) = (226, 230, 234);
const PANEL: (u8, u8, u8) = (255, 255, 255);
const PREVIEW_BG: (u8, u8, u8) = (24, 28, 32);
/// 深底预览区上的提示文字。深底上不能再用 [`TEXT_DIM`]——那是给白底配的，压到深底上
/// 对比度只剩 3:1，等于没写。
const PREVIEW_HINT: (u8, u8, u8) = (146, 156, 166);

fn color(c: (u8, u8, u8), a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}

/// 本模块内部的矩形（浮点）。见模块头的单位说明。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Box2 {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Box2 {
    const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// 浮点矩形 → windui 要的整数矩形（四舍五入）。
///
/// 取整只发生在**绘制前的最后一刻**：布局全程用浮点，避免"每次取整累积半像素误差"
/// 导致相邻两块之间出现一条缝。
fn irect(b: Box2) -> Rect {
    Rect::new(
        b.x.round() as i32,
        b.y.round() as i32,
        b.w.round() as i32,
        b.h.round() as i32,
    )
}

// ─────────────────────────── 纯几何（门禁钉在这里） ───────────────────────────

/// 一个窗口里各块的逻辑矩形。布局是**纯函数**，所以能脱离窗口直接测。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    preview: Box2,
    summary: Box2,
    track: Box2,
    /// 轨道下方的刻度行（帧号），见 [`TICK_H`]。
    ticks: Box2,
    buttons: [Box2; BUTTON_N],
}

/// 窗口当前尺寸 → 各块矩形。**随窗口缩放重算**，所以预览会跟着变大（见模块头）。
///
/// 预览吃满"扣掉摘要/轨道/按钮之后的全部剩余高度"，下限 [`PREVIEW_MIN_H`]；
/// 宽度同理跟客户区走。（★ 顶部不留页眉：提示行删掉后留白也一并删——2026-09-30 用户
/// 定稿，报错改画在摘要行，见 [`TrimCanvas::paint`]。）
fn layout(w: f32, h: f32) -> Layout {
    let content_w = (w - 2.0 * PAD).max(40.0);
    let fixed = PAD + GAP * 3.0 + SUMMARY_H + TRACK_HIT_H + TICK_GAP + TICK_H + BUTTON_H + PAD;
    let preview_h = (h - fixed).max(PREVIEW_MIN_H);
    let preview = Box2::new(PAD, PAD, content_w, preview_h);
    let summary = Box2::new(PAD, preview.y + preview.h + GAP, content_w, SUMMARY_H);
    let track = Box2::new(PAD, summary.y + summary.h + GAP, content_w, TRACK_HIT_H);
    let ticks = Box2::new(PAD, track.y + track.h + TICK_GAP, content_w, TICK_H);
    // 按钮两簇：[0]「打开…」贴左，[1..]「删除此段/撤销/保存」右对齐。
    // 两簇之间那段空白是**刻意留的**——它把"换一份文件"与"改这一段"在视觉上分成两组，
    // 这也正是 Windows 文件对话框的惯例（浏览/打开在左，确定/取消在右）。
    // ★ 按钮顶边至少离刻度行一整个 [`GAP`]（旧版只留 6px，刻度文字压在按钮上）。
    //   高度不够就往下压（宁可挤，也不让按钮出界）。
    let by = (h - PAD - BUTTON_H).max(ticks.y + ticks.h + GAP);
    let mut buttons = [Box2::new(0.0, 0.0, 0.0, 0.0); BUTTON_N];
    buttons[0] = Box2::new(PAD, by, BUTTON_W, BUTTON_H);
    let mut x = w - PAD - BUTTON_W;
    for i in (1..BUTTON_N).rev() {
        buttons[i] = Box2::new(x, by, BUTTON_W, BUTTON_H);
        x -= BUTTON_W + BUTTON_GAP;
    }
    Layout {
        preview,
        summary,
        track,
        ticks,
        buttons,
    }
}

/// **帧边界** → 轨道 x。边界取值 `0..=total`：`0` 在最左、`total` 在最右。
///
/// ★ 用"边界"而不是"帧号"是关键：删掉最后一段时右把手要能停在**最右端**（= 边界 total），
///   按帧号 `total-1` 算的话它永远差一格，用户会觉得"拖不到头"。
fn bound_to_x(k: usize, total: usize, track: Box2) -> f32 {
    if total == 0 {
        return track.x;
    }
    track.x + track.w * (k.min(total) as f32 / total as f32)
}

/// 轨道 x → 帧边界（[`bound_to_x`] 的逆）。
fn x_to_bound(x: f32, total: usize, track: Box2) -> usize {
    if total == 0 {
        return 0;
    }
    let t = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
    (t * total as f32).round() as usize
}

/// 把 `src`（宽高比）等比塞进 `(bx, by, bw, bh)`。
fn fit_box(src_w: f32, src_h: f32, bx: f32, by: f32, bw: f32, bh: f32) -> Box2 {
    if src_w <= 0.0 || src_h <= 0.0 || bw <= 0.0 || bh <= 0.0 {
        return Box2::new(bx, by, bw.max(1.0), bh.max(1.0));
    }
    let s = (bw / src_w).min(bh / src_h);
    let (w, h) = (src_w * s, src_h * s);
    Box2::new(bx + (bw - w) / 2.0, by + (bh - h) / 2.0, w, h)
}

/// 落点在哪个按钮上。
fn button_at(x: f32, y: f32, lay: &Layout) -> Option<usize> {
    lay.buttons.iter().position(|b| b.contains(x, y))
}

/// 点在轨道区域内吗（含把手向外扩出的那点余量）。
fn in_track(x: f32, y: f32, lay: &Layout) -> bool {
    x >= lay.track.x - HANDLE_HIT
        && x <= lay.track.x + lay.track.w + HANDLE_HIT
        && y >= lay.track.y
        && y <= lay.track.y + lay.track.h
}

/// 落点抓到的是哪个把手（都没抓到就是"在轨道上划选"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    /// 待删区间的左边界。
    A,
    /// 待删区间的右边界。
    B,
    /// 从落点拉出一段新的选择（划选手势）。`from` 是按下那一刻的帧边界。
    Sweep { from: usize },
}

fn grab_at(x: f32, y: f32, lay: &Layout, a_x: f32, b_x: f32, total: usize) -> Option<Grab> {
    if !in_track(x, y, lay) {
        return None;
    }
    // 两个把手重合时（还没划过）只会抓到 B —— "从最左往右划"是最常见的手势。
    if (x - b_x).abs() <= HANDLE_HIT + HANDLE_W {
        return Some(Grab::B);
    }
    if (x - a_x).abs() <= HANDLE_HIT + HANDLE_W {
        return Some(Grab::A);
    }
    Some(Grab::Sweep {
        from: x_to_bound(x, total, lay.track),
    })
}

/// 厘秒 → `1.2s`。
fn secs(cs: u32) -> String {
    format!("{:.1}s", cs as f32 / 100.0)
}

// ─────────────────── 状态（与窗口解耦，可脱离窗口单测） ───────────────────

/// 裁剪器的**全部可变状态**：文件 + 选择 + 预览。
///
/// ★ 它单独成结构体、并被 `Rc<RefCell<_>>` 持有，唯一的原因是「打开…」「保存」要弹原生
///   模态对话框，而那个调用必须放进 `ctx.defer_blocking` 的 `FnOnce() + 'static` 闭包里
///   （见模块头）—— 闭包**拿不到 `&mut self`**，所以状态必须能共享。顺带的好处：这个
///   结构体不碰窗口、不碰 Canvas，除了几何之外的逻辑都能直接单测。
struct TrimCore {
    /// 当前打开的文件。`None` = 空窗（还没选文件）。
    path: Option<PathBuf>,
    /// 序列解码器。`None` = 空窗。
    reader: Option<FrameReader>,
    /// 总帧数（载入时走一遍解码得到，之后不变）。
    total: usize,
    /// 每帧时长（厘秒）。
    delays: Vec<u16>,

    /// 已确认删除的区间（可撤销）。
    removed: Vec<(usize, usize)>,
    /// 待删区间的两个**帧边界**（`a <= b <= total`）。`a == b` = 没选。
    a: usize,
    b: usize,

    /// 预览当前停在哪一帧。
    preview_k: usize,
    /// 预览图（只在帧号变化时重建）。
    preview: Option<WImage>,
    /// 预览图是按哪一帧建的（`None` = 还没有）。
    built_for: Option<usize>,
    /// 最近一次失败原因（画在标题栏右侧，不做弹窗——调参时不该被弹窗挡住）。
    status: Option<String>,
    /// **有一个原生对话框正开着**（重入守卫）。
    ///
    /// ★ 为什么非有不可：`ctx.defer_blocking` 的闭包是在**窗口过程内部**执行的，而那个
    ///   闭包会开一个带消息泵的模态对话框 —— 对话框泵消息时我们的窗口照样收事件。于是
    ///   "对话框开着再点一次「打开…」"会套出**第二层**对话框。实测（2026-09-20 日志）
    ///   连点三次得到三层对话框，全都躲在当时还置顶的裁剪窗后面，用户只看到"点了没反应"。
    ///
    /// 两道防线：`hwndOwner`（对话框模态于本窗，点不到窗口）+ 这条守卫（逻辑上也不允许
    /// 排队）。**不能只留前者**：属主只是"提示"级别的约束，而模态循环里的事件次序不是
    /// 我们能保证的。
    dialog_open: bool,
}

/// 窗口内容：共享状态 + 窗口自己的那点瞬态（手势、悬停、尺寸）。
struct TrimCanvas {
    core: Rc<RefCell<TrimCore>>,
    rt: Rc<RefCell<Runtime>>,
    /// **当前客户区尺寸**（`Cell` 是因为 `paint` 只给 `&self`——窗口被拉大时在这一刻
    /// 记下新尺寸，`on_event` 的命中判定与 [`Self::lay`] 才跟得上）。
    win: Cell<(f32, f32)>,
    /// 标题栏文本的信号源：换文件时写它，系统标题栏自动跟上（见 [`TITLE`]）。
    title: Signal<String>,
    /// 正抓着哪个把手（手势进行中）。
    grab: Option<Grab>,
    /// 鼠标悬在哪个按钮上。
    hover_btn: Option<usize>,
}

impl TrimCore {
    /// 空状态（还没选文件）。
    fn empty() -> Self {
        Self {
            path: None,
            reader: None,
            total: 0,
            delays: Vec::new(),
            removed: Vec::new(),
            a: 0,
            b: 0,
            preview_k: 0,
            preview: None,
            built_for: None,
            status: None,
            dialog_open: false,
        }
    }

    /// 打开一个文件并建立好初始状态。
    fn open(path: PathBuf) -> Result<Self, String> {
        let mut c = Self::empty();
        c.load(path)?;
        Ok(c)
    }

    /// 换一份文件。
    ///
    /// ★ 失败时**原样保留旧内容**，并把原因挂到 `status` 上（见 [`Self::try_load`] 的说明）。
    ///   用户在裁剪到一半时点「打开…」挑错了文件（或者挑了个坏的），不该把手上这份的
    ///   进度一起弄丢。
    fn load(&mut self, path: PathBuf) -> Result<(), String> {
        let r = self.try_load(path);
        // ★ **`status` 的写权只在这里**。界面上显示错误的通路只有这一条，把"挂原因"留给
        //   调用方就会出现"文档说挂了、代码里没挂"的两份真相 —— 真出问题时表现为
        //   "点「打开…」挑了个坏文件，窗口一动不动"，最难查的一类。成功则一并清掉旧原因。
        self.status = match &r {
            Ok(()) => None,
            Err(e) => Some(format!("打不开：{e}")),
        };
        r
    }

    /// [`Self::load`] 的正体：只做"解码 → 换字段"，**不碰 `status`**（那是外层的事）。
    /// 放掉解码器与预览图（关窗时调）。
    ///
    /// # MP4 版是两块，**这一侧是三块，而且更大**
    ///
    /// | 对象 | 1080p 下的量级 |
    /// |---|---|
    /// | `reader`（`FrameReader` 的合成画布 `canvas` + `opaque` 掩码） | ≈ 4.1 MB |
    /// | `reader` 的 `snapshot`（`Previous` 处置回滚用，又是两张） | ≈ 4.1 MB |
    /// | `preview`（一帧 RGBA + `WImage::from_rgba` 那份 premultiply） | ≈ 16 MB |
    ///
    /// 合计看一窗 ≈ 24 MB。它们是全项目最容易粗心错过的一块：`GIF` 既有
    /// `TrimCanvas` 又有 `TrimCore`，控件树 drop 到 **`FrameReader`** 这段路要跨
    /// `Rc<RefCell<TrimCore>>`，漏一个握着 `Rc` 的闭包就永远到不了。
    ///
    /// 与 MP4 同套路（详见 [`crate::runtime::layer::mp4_trim::TrimCore::release_memory`]）：
    /// 先显式置 `None` 让释放确定发生，再让 `request_trim_soon` 把页摘出工作集。
    pub fn release_memory(&mut self) {
        self.reader = None;
        self.preview = None;
        self.built_for = None;
    }

    fn try_load(&mut self, path: PathBuf) -> Result<(), String> {
        // 先把新文件解码到位、确认可用，**再**动自己的字段 —— 顺序反过来就会出现
        // "旧的丢了、新的没进来"的中间态。
        let mut reader = FrameReader::open(&path)?;
        let total = reader.frame_count()?;
        if total == 0 {
            return Err("这个 GIF 一帧都没有".to_string());
        }
        let delays = reader.delays().to_vec();

        self.path = Some(path);
        self.reader = Some(reader);
        self.total = total;
        self.delays = delays;
        self.removed.clear();
        self.a = 0;
        self.b = 0;
        self.preview = None;
        self.built_for = None;
        self.preview_k = 0;
        // 载入后立刻把首帧做成预览，窗口一打开就有画面。
        self.set_preview(0);
        Ok(())
    }

    /// 是否已载入文件（空窗判据）。
    fn is_loaded(&self) -> bool {
        self.reader.is_some()
    }

    /// 当前文件名（标题栏用；空窗时是空串）。
    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 当前"保留哪些帧"。**这是唯一的换算点**：界面只维护 removed + 待删段。
    fn keep_plan(&self) -> Vec<(usize, usize)> {
        let mut removed = self.removed.clone();
        if self.b > self.a {
            removed.push((self.a, self.b));
        }
        gif_trim::complement(self.total, &removed)
    }

    /// 摘要：`(保留帧数, 保留时长厘秒)`。
    fn plan_stats(&self) -> (usize, u32) {
        gif_trim::plan_summary(&self.delays, &self.keep_plan())
    }

    fn total_cs(&self) -> u32 {
        self.delays.iter().map(|&d| d as u32).sum()
    }

    /// 待删段的时长（厘秒）。
    fn pending_cs(&self) -> u32 {
        self.delays[self.a.min(self.delays.len())..self.b.min(self.delays.len())]
            .iter()
            .map(|&d| d as u32)
            .sum()
    }

    /// 让预览停在第 `k` 帧。帧号没变就什么都不做（拖动时不重建图像）。
    fn set_preview(&mut self, k: usize) {
        let k = k.min(self.total.saturating_sub(1));
        if self.built_for == Some(k) {
            return;
        }
        // 先把结果取出来，借用到此结束 —— 后面要写 `self.preview`，不能同时持有
        // `self.reader` 的可变借用。
        let got = match self.reader.as_mut() {
            Some(rd) => rd.frame_rgba_at(k),
            None => return,
        };
        match got {
            Ok((w, h, rgba)) => match WImage::from_rgba(w, h, &rgba) {
                Ok(img) => {
                    self.preview = Some(img);
                    self.built_for = Some(k);
                    self.preview_k = k;
                }
                Err(e) => self.status = Some(format!("预览图构建失败：{e}")),
            },
            Err(e) => self.status = Some(e),
        }
    }

    fn set_ab(&mut self, a: usize, b: usize) {
        let (mut a, mut b) = (a.min(self.total), b.min(self.total));
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        self.a = a;
        self.b = b;
    }

    /// 「删除此段」：把待删段落进 `removed`，然后把选择清空（避免"删了还在选"的歧义）。
    fn commit_delete(&mut self) {
        if self.b <= self.a {
            self.status = Some("先在时间轴上划出一段再删".to_string());
            return;
        }
        self.removed.push((self.a, self.b));
        self.removed = gif_trim::normalize(&self.removed);
        self.set_ab(0, 0);
        self.status = None;
    }

    fn undo(&mut self) {
        if self.removed.pop().is_none() {
            return;
        }
        self.removed = gif_trim::normalize(&self.removed);
        self.status = None;
    }

    /// 抢占"我要开一个原生对话框"的名额。已经有对话框开着 → `false`，调用方**必须**放弃。
    ///
    /// 用名字而不是直接读写 `dialog_open`，是为了让"先抢再开、开完必还"这件事只有一处
    /// 能写错（见 [`Self::end_dialog`]）。
    fn begin_dialog(&mut self) -> bool {
        if self.dialog_open {
            return false;
        }
        self.dialog_open = true;
        true
    }

    /// 还名额。**每个** `begin_dialog` 返回 `true` 的分支都必须走到这里 —— 包括用户取消、
    /// 包括出错。漏一次，之后「打开…」「保存」就**永久**没反应了（名额一直占着）。
    fn end_dialog(&mut self) {
        self.dialog_open = false;
    }
}

impl TrimCanvas {
    fn new(
        core: Rc<RefCell<TrimCore>>,
        rt: Rc<RefCell<Runtime>>,
        win: (f32, f32),
        title: Signal<String>,
    ) -> Self {
        Self {
            core,
            rt,
            win: Cell::new(win),
            title,
            grab: None,
            hover_btn: None,
        }
    }

    fn lay(&self) -> Layout {
        let (w, h) = self.win.get();
        layout(w, h)
    }

    /// 「打开…」：弹原生打开对话框，把选中的 GIF 载入**本窗**。
    ///
    /// 刻意不新开一个窗：用户在裁剪器里按"打开"，意思是"换一份来裁"，不是"再开一个
    /// 裁剪器"。
    ///
    /// ★ 三件事都在这里把关：
    ///   1. **重入守卫** —— 已经有一个对话框开着就整条丢弃（见 [`TrimCore::dialog_open`]）。
    ///      这是 2026-09-20 那个"点「打开…」毫无反应、日志里三条连续取消"的直接修复点。
    ///   2. **属主 = 本窗** —— 对话框模态于我们：它永远在我们之上，我们也点不到，从物理上
    ///      再堵一次重入。
    ///   3. **初始目录 = `temp/gif/`** —— 刚录的 GIF 都在这儿，这是用户最常挑的地方。
    ///      （目录是提示不是保证，用户上次改过目录时系统会记得那个。）
    fn pick_file(&mut self, ctx: &mut EventCtx) {
        let core = self.core.clone();
        if !core.borrow_mut().begin_dialog() {
            log::info!("裁剪：已有对话框开着，忽略这次「打开」");
            return;
        }
        // ★ 属主句柄必须在**这里**取：`native_window_handle()` 在窗口过程入口按"当前正在
        //   分发消息的窗口"写入，只有事件回调栈里读到的才是本窗（同 `layer::pin`）。而闭包
        //   跑在事件分发**完全返回之后**，那时它已经不保证是本窗了。
        let owner = windui::platform::native_window_handle();
        // 初始目录 = GIF 产物目录。**共用 `MODULE_GIF`** 而不是再写一个 "gif"：两处写死
        // 字面量时改一处就会出现"录到 A、打开默认在 B"，而且什么都不报。
        let start_dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_GIF);
        let title = self.title;
        ctx.defer_blocking(move || {
            let picked = crate::platform::file_dialog::open_gif(Some(&start_dir), owner);
            match &picked {
                None => log::info!("裁剪：用户在「打开」里取消"),
                Some(p) => {
                    // ★ 先把借用收进一个局部变量再 match：`match core.borrow_mut().load(..)`
                    //   会让 `RefMut` 活到整个 match 语句结束，Err 分支里再借一次就是 panic
                    //   （Rust 2021 的临时量生命周期，本项目已在 `pin_req` / `take_rec_toast`
                    //   踩过）。失败原因由 `load` 自己挂到 `status`（见其文档），这里只记日志。
                    let r = core.borrow_mut().load(p.clone());
                    match r {
                        Ok(()) => log::info!("裁剪窗载入 {}", p.display()),
                        Err(e) => log::warn!("裁剪窗载入失败：{e}"),
                    }
                    // 标题栏跟着文件走（"文档名 - 软件名"）。`Signal` 与 `CUR_TITLE` 都只在
                    // UI 线程动——本闭包正跑在 UI 线程（事件分发返回后）。
                    let name = core.borrow().file_name();
                    title.set(window_title(&name));
                    set_cur_title(&name);
                }
            }
            // 名额在这里**统一**归还：上面三条出口（取消 / 成功 / 失败）都汇到这一点，
            // 任何一条提前 return 都会让「打开…」「保存」从此永久失灵。
            core.borrow_mut().end_dialog();
            crate::platform::win_util::invalidate_by_title(&cur_title());
        });
    }

    /// 「保存」= **另存为**：弹原生对话框挑落点，结果写到那儿，原片只读。
    ///
    /// 为什么同步做、不养后台线程：正常几百毫秒内结束，而且是用户显式发起的编辑动作；
    /// 为它铺一条线程 + 完成消息的代价远大于收益（同一时刻只可能有一份裁剪在跑）。
    fn save(&mut self, ctx: &mut EventCtx) {
        // 先把要用的东西从共享状态里取出来，**立刻结束借用**：下面要 `defer_blocking`，
        // 而闭包执行时如果还端着 `RefCell` 的借用，窗口重画就会炸。
        let (path, keep) = {
            let st = self.core.borrow();
            let Some(p) = st.path.clone() else {
                drop(st);
                self.core.borrow_mut().status = Some("先打开一个 GIF 再保存".to_string());
                ctx.mark_dirty_all();
                return;
            };
            (p, st.keep_plan())
        };
        if keep.is_empty() {
            self.core.borrow_mut().status = Some("一帧都不剩，不能保存".to_string());
            ctx.mark_dirty_all();
            return;
        }

        // 预填名 `原名_裁剪`：落在**原片同目录**。用户按回车就是一个新文件，天然不会
        // 撞上原片（真撞上了 `rewrite_to` 也会拒掉）。
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "gif".to_string());
        let default_name = format!("{stem}_裁剪");
        let start_dir = path.parent().map(|p| p.to_path_buf());

        // 抢占对话框名额。放在**校验之后**：上面两条"没得保存"的出口根本不会弹对话框，
        // 不该占名额（更不该因为占了没还而把后面真要用的人挡住）。
        if !self.core.borrow_mut().begin_dialog() {
            log::info!("GIF 裁剪：已有对话框开着，忽略这次「保存」");
            return;
        }
        let owner = windui::platform::native_window_handle();
        let core = self.core.clone();
        let rt = self.rt.clone();
        ctx.defer_blocking(move || {
            let dst =
                crate::platform::file_dialog::save_gif(&default_name, start_dir.as_deref(), owner);
            match dst {
                None => log::info!("GIF 裁剪：用户在「另存为」里取消"),
                Some(dst) => match gif_trim::rewrite_to(&path, &keep, &dst) {
                    Ok(rep) => {
                        log::info!(
                            "GIF 裁剪完成：{} 帧 / {} → {} 字节 → {}",
                            rep.frames,
                            secs(rep.duration_cs),
                            rep.bytes,
                            dst.display()
                        );
                        rt.borrow_mut().show_notify(&format!(
                            "已裁剪：{} 帧 · {} → {}",
                            rep.frames,
                            secs(rep.duration_cs),
                            dst.file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default()
                        ));
                        crate::platform::win_util::post_close_by_title(&cur_title());
                    }
                    Err(e) => {
                        log::warn!("GIF 裁剪失败：{e}");
                        core.borrow_mut().status = Some(format!("保存失败：{e}"));
                        crate::platform::win_util::invalidate_by_title(&cur_title());
                    }
                },
            }
            // 与 `pick_file` 同一条纪律：三条出口都汇到这里才归还名额。
            core.borrow_mut().end_dialog();
        });
    }
}

impl Widget for TrimCanvas {
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
        let (bx, by, bw, bh) = (
            bounds.x as f32,
            bounds.y as f32,
            bounds.w as f32,
            bounds.h as f32,
        );
        canvas.clear_rect(bx, by, bw, bh);
        canvas.fill_rect(bx, by, bw, bh, &WPaint::fill(color(PANEL, 255)));
        // 记住这一刻的客户区尺寸：窗口被拉大/最大化后，命中判定与布局都按它算。
        self.win.set((bw, bh));
        // ★ 把 `CUR_TITLE` 校准成**这个活窗**的标题：`.single()` 让"重复按快捷键"只聚焦、
        //   不重建窗，而那条路上 `spawn` 仍会带着新文件名写一次 `CUR_TITLE`——若它就这么
        //   留下，之后所有按标题的关窗/聚焦/重画都会落空（静默失效）。活窗每帧校准一次，
        //   谁在画谁说了算。
        set_cur_title_raw(self.title.get());

        let lay = self.lay();
        let ts = TextStyle::new(13.0);
        let ts_small = TextStyle::new(12.0);

        // 状态一次性借出来（`paint` 只读）。`on_event` 那次可变借用到这里早就结束了 ——
        // 两次调用是分开的，不会撞上 `RefCell` 的"已借用"panic。
        let st = self.core.borrow();

        // ── 预览：深底 + 等比居中的当前帧 ──
        canvas.fill_rect(
            lay.preview.x,
            lay.preview.y,
            lay.preview.w,
            lay.preview.h,
            &WPaint::fill(color(PREVIEW_BG, 255)),
        );
        match &st.preview {
            Some(img) => {
                let r = fit_box(
                    img.width() as f32,
                    img.height() as f32,
                    lay.preview.x,
                    lay.preview.y,
                    lay.preview.w,
                    lay.preview.h,
                );
                canvas.draw_image(img, irect(r), Fit::Fill, 0.0, 1.0);
            }
            // 只有"载入了但这帧解不出来"才提示；空窗就是一块黑板——状态标题栏说了算
            // （2026-09-30 用户定稿：提示太多余）。
            None if st.is_loaded() => {
                canvas.draw_text(
                    "这一帧解不出来",
                    irect(lay.preview),
                    color(PREVIEW_HINT, 255),
                    Align::Center,
                    &ts,
                );
            }
            None => {}
        }

        // ── 摘要 / 报错**同一行**：页眉整行删掉后顶部不再留白（2026-09-30 用户定稿），
        //    有报错时红字顶替摘要画在这一行——报错本来就是当下最要紧的状态，不占独立空间。
        let (kept_frames, kept_cs) = st.plan_stats();
        let summary = if let Some(s) = &st.status {
            s.clone()
        } else if !st.is_loaded() {
            String::new()
        } else if st.b > st.a {
            format!(
                "共 {} 帧 · {}　|　待删 {} → {} 帧（{}）　|　删后保留 {} 帧 · {}",
                st.total,
                secs(st.total_cs()),
                st.a,
                st.b,
                secs(st.pending_cs()),
                kept_frames,
                secs(kept_cs),
            )
        } else {
            format!(
                "共 {} 帧 · {}　|　在时间轴上按住左键横向划过，划出要删掉的那一段",
                st.total,
                secs(st.total_cs())
            )
        };
        if !summary.is_empty() {
            let fg = if st.status.is_some() {
                DANGER
            } else {
                TEXT_DIM
            };
            canvas.draw_text(
                &summary,
                irect(lay.summary),
                color(fg, 255),
                Align::Start,
                &ts_small,
            );
        }

        // ── 轨道 ──
        // 空窗也照画这条灰底：它说明"这里将来是一条时间轴"，比留一片说不清的空白诚实。
        let track_y = lay.track.y + (lay.track.h - TRACK_H) / 2.0;
        canvas.fill_round_rect(
            lay.track.x,
            track_y,
            lay.track.w,
            TRACK_H,
            TRACK_H / 2.0,
            &WPaint::fill(color(BORDER, 255)),
        );
        if st.is_loaded() {
            // 已删区间（浅红）
            for &(ra, rb) in &st.removed {
                let x0 = bound_to_x(ra, st.total, lay.track);
                let x1 = bound_to_x(rb, st.total, lay.track);
                canvas.fill_round_rect(
                    x0,
                    track_y,
                    x1 - x0,
                    TRACK_H,
                    TRACK_H / 2.0,
                    &WPaint::fill(color(DANGER_DONE, 255)),
                );
            }
            // 待删区间（正红）
            if st.b > st.a {
                let x0 = bound_to_x(st.a, st.total, lay.track);
                let x1 = bound_to_x(st.b, st.total, lay.track);
                canvas.fill_round_rect(
                    x0,
                    track_y,
                    x1 - x0,
                    TRACK_H,
                    TRACK_H / 2.0,
                    &WPaint::fill(color(DANGER, 255)),
                );
            }
            // 预览播放头
            let px = bound_to_x(st.preview_k.min(st.total), st.total, lay.track);
            canvas.fill_rect(
                px - 0.5,
                lay.track.y,
                1.0,
                lay.track.h,
                &WPaint::fill(color(ACCENT, 160)),
            );
            // 两个把手
            for k in [st.a, st.b] {
                let x = bound_to_x(k, st.total, lay.track);
                canvas.fill_round_rect(
                    x - HANDLE_W / 2.0,
                    lay.track.y,
                    HANDLE_W,
                    lay.track.h,
                    HANDLE_W / 2.0,
                    &WPaint::fill(color(ACCENT, 255)),
                );
            }
            // 时间刻度：左端 0、右端总长。**画在自己那一行**（`lay.ticks`），不占用轨道
            // 命中框的末尾——否则 16px 高的字会压到按钮上（见 [`TICK_H`]）。
            canvas.draw_text(
                "0.0s",
                irect(Box2::new(lay.ticks.x, lay.ticks.y, 60.0, TICK_H)),
                color(TEXT_DIM, 255),
                Align::Start,
                &ts_small,
            );
            let end = secs(st.total_cs());
            canvas.draw_text(
                &end,
                irect(Box2::new(
                    lay.track.x + lay.track.w - 60.0,
                    lay.ticks.y,
                    60.0,
                    TICK_H,
                )),
                color(TEXT_DIM, 255),
                Align::End,
                &ts_small,
            );
        }

        // ── 按钮 ──
        for (i, label) in BUTTON_LABELS.iter().enumerate() {
            let r = lay.buttons[i];
            let enabled = match i {
                // 「打开…」任何状态都能点：它是"换一份文件"的入口，空窗时更是唯一出路。
                0 => true,
                1 => st.b > st.a,
                2 => !st.removed.is_empty(),
                // 空窗 / 一帧都不剩时"保存"必须点不动：点下去只会得到一句错误，不如直接置灰。
                3 => st.is_loaded() && kept_frames > 0,
                _ => true,
            };
            let hovered = self.hover_btn == Some(i);
            let (bg, fg) = match (enabled, hovered) {
                (false, _) => (color(BORDER, 120), color(TEXT_DIM, 120)),
                (true, true) => (color(ACCENT, 40), color(ACCENT, 255)),
                (true, false) => (color(BORDER, 160), color(TEXT, 255)),
            };
            canvas.fill_round_rect(r.x, r.y, r.w, r.h, 6.0, &WPaint::fill(bg));
            canvas.draw_text(label, irect(r), fg, Align::Center, &ts);
        }
    }

    fn on_event(&mut self, ctx: &mut EventCtx, ev: &Event) -> bool {
        let lay = self.lay();
        match ev {
            Event::Pointer(pe) => {
                let (x, y) = (pe.pos.x as f32, pe.pos.y as f32);
                match pe.kind {
                    PointerKind::Move => {
                        let hb = button_at(x, y, &lay);
                        if hb != self.hover_btn {
                            self.hover_btn = hb;
                            ctx.mark_dirty_all();
                        }
                        let Some(g) = self.grab else {
                            return false;
                        };
                        let mut st = self.core.borrow_mut();
                        if !st.is_loaded() {
                            return false;
                        }
                        let (a, b, total) = (st.a, st.b, st.total);
                        let k = x_to_bound(x, total, lay.track);
                        match g {
                            Grab::A => st.set_ab(k, b),
                            Grab::B => st.set_ab(a, k),
                            Grab::Sweep { from } => st.set_ab(from, k),
                        }
                        // 预览跟着**刚动的那一侧**走，用户才看得清"从这里开始不要"。
                        let follow = match g {
                            Grab::A => a,
                            _ => b.saturating_sub(1),
                        };
                        st.set_preview(follow);
                        // 显式放开：`mark_dirty_all` 可能同步触发重画，而重画要借同一份状态。
                        drop(st);
                        ctx.mark_dirty_all();
                        true
                    }
                    PointerKind::Down if pe.button == MouseButton::Left => {
                        if let Some(i) = button_at(x, y, &lay) {
                            self.grab = None;
                            match i {
                                // [0] 打开…（文件级动作，左簇）
                                0 => self.pick_file(ctx),
                                // [1..] 编辑级动作（右簇）
                                1 => self.core.borrow_mut().commit_delete(),
                                2 => self.core.borrow_mut().undo(),
                                3 => self.save(ctx),
                                _ => {}
                            }
                            ctx.mark_dirty_all();
                            return true;
                        }
                        if !in_track(x, y, &lay) {
                            return false;
                        }
                        // 空窗时轨道上的一切手势都无意义（`total == 0` 会让帧边界全是 0）。
                        let mut st = self.core.borrow_mut();
                        if !st.is_loaded() {
                            return false;
                        }
                        let a_x = bound_to_x(st.a, st.total, lay.track);
                        let b_x = bound_to_x(st.b, st.total, lay.track);
                        let g = grab_at(x, y, &lay, a_x, b_x, st.total);
                        self.grab = g;
                        if let Some(Grab::Sweep { from }) = g {
                            // 划选出新的一段：先把旧选择清掉，免得两段叠在一起看不出。
                            st.set_ab(from, from);
                            let top = st.total.saturating_sub(1);
                            st.set_preview(from.min(top));
                        }
                        drop(st);
                        ctx.capture();
                        ctx.mark_dirty_all();
                        true
                    }
                    PointerKind::Up if pe.button == MouseButton::Left => {
                        let was = self.grab.take().is_some();
                        if was {
                            ctx.release_capture();
                        }
                        ctx.mark_dirty_all();
                        was
                    }
                    _ => false,
                }
            }
            Event::Key(ke) if ke.pressed => {
                // Ctrl+O = 打开（Windows 惯例）。必须在下面那个 `match ke.key` **之前**判：
                // 它带修饰键，落不到任何单独的按键分支上。
                if ke.ctrl {
                    if let Key::Char(c) = ke.key {
                        if c.eq_ignore_ascii_case(&'o') {
                            self.pick_file(ctx);
                            return true;
                        }
                    }
                }
                match ke.key {
                    Key::Escape => {
                        ctx.request_close();
                        true
                    }
                    Key::Enter => {
                        self.save(ctx);
                        true
                    }
                    Key::Left | Key::Right => {
                        // 整段把手平移 1 帧（Shift = 10）：鼠标划不准时用它。
                        let step = if ke.shift { 10 } else { 1 };
                        let mut st = self.core.borrow_mut();
                        if !st.is_loaded() {
                            return false;
                        }
                        let (a, b, total) = (st.a, st.b, st.total);
                        if ke.key == Key::Left {
                            st.set_ab(a.saturating_sub(step), b.saturating_sub(step));
                        } else {
                            st.set_ab((a + step).min(total), (b + step).min(total));
                        }
                        let follow = st.b.saturating_sub(1);
                        st.set_preview(follow);
                        drop(st);
                        ctx.mark_dirty_all();
                        true
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }
}

/// 打开裁剪窗并**载入一份 GIF**（控件条「裁剪」/ 刚录完那条路）。
///
/// 文件解不开时返回 `false`，调用方据此给用户一句提示。
pub fn open(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, path: PathBuf) -> bool {
    let core = match TrimCore::open(path.clone()) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("打开裁剪窗失败：{e}");
            return false;
        }
    };
    spawn(ctx, rt, core);
    log::info!("GIF 裁剪窗已打开，载入 {}", path.display());
    true
}

/// 打开裁剪窗的**空状态**：一份文件都没载入，等用户按「打开…」挑。
///
/// 这是裁剪快捷键的入口。用户想裁的往往不是刚录的那一段（可能是上周的、别人发来的），
/// 所以空窗不是"没做完的状态"，而是裁剪器的正常入口。
pub fn open_blank(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) {
    spawn(ctx, rt, TrimCore::empty());
    log::info!("GIF 裁剪窗已打开（空状态，等用户选文件）");
}

/// 两个入口共用的开窗动作。
///
/// `.single("gif_trim")` 是"快捷键重复按不会开出第二个窗"的保证：已经开着时框架只把它
/// 聚焦、**连内容都不重建**（见 windui `App::single` 的文档与它自己的用例）。所以这里
/// 不需要自己查"窗开没开"。
fn spawn(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, core: TrimCore) {
    let win = (WIN_W as f32, WIN_H as f32);
    // 标题栏 = "文档名 - 软件名"（见 [`TITLE`]）。信号给 windui（它每帧拉取并
    // `SetWindowTextW`），`CUR_TITLE` 给我们自己的 `FindWindowW` —— 两份必须同时更新。
    let name = core.file_name();
    let title_sig = signal(window_title(&name));
    set_cur_title(&name);
    let core = Rc::new(RefCell::new(core));
    let canvas = TrimCanvas::new(core.clone(), rt.clone(), win, title_sig);
    let rt_close = rt.clone();
    let core_close = core.clone();
    // 居中：窗口尺寸是**逻辑 dp**，物理尺寸要乘 dpi（见 `layer` 模块头的单位表）。
    let dpi = crate::platform::display::dpi_scale();
    let vs = rt.borrow().screen;
    let x = vs.x + (vs.w - (WIN_W as f32 * dpi).round() as i32) / 2;
    let y = vs.y + (vs.h - (WIN_H as f32 * dpi).round() as i32) / 2;
    log::info!("GIF 裁剪窗 @ ({x},{y})");

    ctx.open_window(
        // ★ 留系统标题栏（不设 `frameless`）+ 可缩放：用户能最大化/拖大看画面，见模块头。
        //   `min_size` 是下限——再小按钮两簇会撞上，布局也会退化成"只剩一条缝"。
        Window::new(TITLE, WIN_W, WIN_H)
            .single("gif_trim")
            .title(title_sig)
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .position(x, y)
            .bg(color(PANEL, 255))
            .on_close_request(move |_ctx| {
                rt_close.borrow_mut().close_gif_trim();
                // ★ 解码器 + 画布 + 预览图，一窗 ≈ 24 MB（见 TrimCore::release_memory
                //   的分项表）。当场吐出来，再请 `working_set` 整一次工作集。
                core_close.borrow_mut().release_memory();
                crate::platform::working_set::request_trim_soon();
                true
            })
            .content(move || Element::leaf().widget(canvas).fill()),
    );
    // ★ 用 `focus_window_by_title` 而**不是** `activate_window_by_title`：后者会把这个窗
    //   搬进置顶带（`SetWindowPos(HWND_TOPMOST)`），而本窗要弹的原生文件对话框是**普通
    //   窗口** —— 裁剪窗一旦置顶，对话框就永远在它下面，用户看不见也点不到（2026-09-20
    //   用户报告"打开 gif 时全被遮挡"）。这里要的只是"叫到前台 + 给焦点"，z 序留在普通档。
    //
    //   ★ 别把这条当成"可选优化"：置顶时用户为了够到对话框会在裁剪窗上连点，而延后闭包
    //     跑在窗口过程里，每一次点击都会在上一层模态循环里再套一层对话框（实测三层）。
    let _ = crate::platform::win_util::focus_window_by_title(&cur_title());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 关窗必须把解码器 + 预览图（≈ 24 MB，见 `TrimCore::release_memory` 的分项表）
    /// 当场吐掉。回归锁：2026-10-01 用户报"关掉所有模块内存不回落"，本窗是最大单块。
    #[test]
    fn release_memory_clears_reader_and_preview() {
        let mut core = TrimCore::empty();
        core.reader = None;
        core.preview = Some(WImage::from_rgba(2, 2, &[0u8; 16]).expect("建图"));
        core.built_for = Some(0);
        core.release_memory();
        assert!(core.reader.is_none(), "解码器必须被清空");
        assert!(core.preview.is_none(), "预览图必须被清空");
        assert!(core.built_for.is_none(), "预览标记必须被清空");
    }

    fn track() -> Box2 {
        Box2::new(20.0, 100.0, 400.0, 30.0)
    }

    /// 帧边界 ↔ 轨道 x 必须是**严格互逆**的，否则拖到哪帧就说不准了。
    #[test]
    fn frame_boundary_and_track_x_are_inverse() {
        let t = track();
        let total = 180usize;
        for k in [0usize, 1, 59, 60, 90, 179, 180] {
            let x = bound_to_x(k, total, t);
            assert_eq!(x_to_bound(x, total, t), k, "边界 {k} 往返对不上");
        }
        // 两端必须落在轨道的两端：右把手要能真的拖到最右（边界 = total）。
        assert_eq!(bound_to_x(0, total, t), t.x);
        assert!((bound_to_x(total, total, t) - (t.x + t.w)).abs() < 1e-3);
        // 越界要被夹住，不能算出轨道外的 x。
        assert_eq!(bound_to_x(999, total, t), t.x + t.w);
        assert_eq!(bound_to_x(0, 0, t), t.x);
    }

    /// 划选手势落在空白处（没碰到把手）时才算划选；反着划也要得到同一段。
    #[test]
    fn sweep_selects_the_range_between_the_two_points() {
        let lay = layout(720.0, 440.0);
        let t = lay.track;
        let total = 100usize;
        let mid = t.x + t.w * 0.25;
        let grab = grab_at(mid, t.y + t.h / 2.0, &lay, t.x, t.x, total);
        assert!(
            matches!(grab, Some(Grab::Sweep { .. })),
            "空白处按下应是划选"
        );
        if let Some(Grab::Sweep { from }) = grab {
            let a = from;
            let b = x_to_bound(mid + t.w * 0.25, total, t);
            assert!(a < b);
            assert_eq!(a, x_to_bound(mid, total, t));
        }
    }

    /// 把手优先于划选：落在把手上必须抓把手，否则"想微调却把整段重置了"。
    #[test]
    fn handle_takes_priority_over_sweeping() {
        let lay = layout(720.0, 440.0);
        let t = lay.track;
        let total = 100usize;
        let (a, b) = (30usize, 70usize);
        let a_x = bound_to_x(a, total, t);
        let b_x = bound_to_x(b, total, t);
        let y = t.y + t.h / 2.0;
        assert_eq!(grab_at(a_x + 2.0, y, &lay, a_x, b_x, total), Some(Grab::A));
        assert_eq!(grab_at(b_x - 2.0, y, &lay, a_x, b_x, total), Some(Grab::B));
        // 两把手都在最左（还没划过）时抓到的是 B —— "从最左往右划"是最常见手势。
        assert_eq!(grab_at(t.x, y, &lay, t.x, t.x, total), Some(Grab::B));
        // 轨道之外不抓。
        assert_eq!(grab_at(t.x, t.y - 40.0, &lay, a_x, b_x, total), None);
    }

    /// 预览等比塞进方框：长边贴边、居中、不裁切。
    #[test]
    fn preview_fits_inside_the_box_keeping_aspect() {
        let r = fit_box(800.0, 600.0, 0.0, 0.0, 400.0, 400.0);
        assert!((r.w - 400.0).abs() < 0.5 && (r.h - 300.0).abs() < 0.5);
        assert!((r.y - 50.0).abs() < 0.5, "竖直方向要居中");
        let r = fit_box(600.0, 800.0, 0.0, 0.0, 400.0, 400.0);
        assert!((r.h - 400.0).abs() < 0.5 && (r.w - 300.0).abs() < 0.5);
        // 退化输入不能算出 0 宽高（那会让 draw_image 拿到空矩形）。
        let r = fit_box(0.0, 0.0, 0.0, 0.0, 100.0, 50.0);
        assert!(r.w > 0.0 && r.h > 0.0);
    }

    /// 五个按钮不得互相重叠，也不得跑出窗口；[0] 在左、[1..] 在右，两簇不能贴到一块。
    #[test]
    fn buttons_are_laid_out_inside_the_window() {
        let lay = layout(WIN_W as f32, WIN_H as f32);
        assert_eq!(lay.buttons.len(), BUTTON_N);
        for b in lay.buttons {
            assert!(b.x >= 0.0 && b.x + b.w <= WIN_W as f32, "按钮出界：{b:?}");
        }
        for i in 1..BUTTON_N {
            assert!(
                lay.buttons[i].x >= lay.buttons[i - 1].x + lay.buttons[i - 1].w,
                "按钮重叠"
            );
        }
        // 「打开…」贴左，「取消」贴右 —— 这是"文件级动作在左、编辑级动作在右"的几何表达。
        assert!((lay.buttons[0].x - PAD).abs() < 1e-3, "「打开…」没有贴左边");
        let last = lay.buttons[BUTTON_N - 1];
        assert!(
            (last.x + last.w - (WIN_W as f32 - PAD)).abs() < 1e-3,
            "「取消」没有贴右边"
        );
        // 两簇之间必须留出空白（否则"打开"看着像排在"删除此段"前面）。
        let gap = lay.buttons[1].x - (lay.buttons[0].x + lay.buttons[0].w);
        assert!(gap >= BUTTON_GAP * 2.0, "左右两簇几乎贴在一起了，gap={gap}");

        assert!(lay.preview.y + lay.preview.h <= lay.summary.y);
        assert!(lay.summary.y + lay.summary.h <= lay.track.y);
        assert!(lay.track.y + lay.track.h <= lay.ticks.y, "刻度压在轨道上");
        assert!(lay.ticks.y + lay.ticks.h <= lay.buttons[0].y);
        assert!(lay.buttons[0].y + lay.buttons[0].h <= WIN_H as f32);
    }

    /// 刻度数字**不得贴着按钮**：它曾经画在轨道命中框的末尾，而按钮只比轨道低 6px，
    /// 16px 高的字直接压到按钮上（2026-09-30 用户报"数字快接触到按钮了"）。
    #[test]
    fn tick_labels_keep_a_full_gap_from_the_buttons() {
        for (w, h) in [
            (MIN_W as f32, MIN_H as f32),
            (WIN_W as f32, WIN_H as f32),
            (1400.0, 900.0),
        ] {
            let lay = layout(w, h);
            let gap = lay.buttons[0].y - (lay.ticks.y + lay.ticks.h);
            assert!(gap >= GAP - 0.01, "{w}×{h} 下刻度离按钮只有 {gap}px");
        }
    }

    /// 标题栏：**文档名 - 软件名**（记事本口径），任务栏/Alt-Tab 里先看到自己的文件。
    #[test]
    fn window_title_is_document_name_dash_app_name() {
        assert_eq!(window_title(""), format!("未打开文件 - {TITLE}"));
        assert_eq!(
            window_title("CurSeen_2026-09-30.gif"),
            format!("CurSeen_2026-09-30.gif - {TITLE}")
        );
        assert!(TITLE.starts_with("CurSeen"));
    }

    /// 窗口拉大时预览跟着变大（"能放大才看得清"），且**任何尺寸下**各块都不越界。
    #[test]
    fn preview_grows_with_the_window_and_layout_stays_inside() {
        let small = layout(MIN_W as f32, MIN_H as f32);
        let big = layout(1400.0, 900.0);
        assert!(
            big.preview.h > small.preview.h + 100.0,
            "窗口拉高了预览没变大：{} → {}",
            small.preview.h,
            big.preview.h
        );
        assert!(small.preview.h >= PREVIEW_MIN_H, "最小时预览低于下限");
        for (lay, w, h) in [(small, MIN_W as f32, MIN_H as f32), (big, 1400.0, 900.0)] {
            assert!(lay.preview.y + lay.preview.h <= lay.summary.y);
            assert!(lay.summary.y + lay.summary.h <= lay.track.y);
            assert!(lay.track.y + lay.track.h <= lay.ticks.y);
            assert!(lay.ticks.y + lay.ticks.h <= lay.buttons[0].y);
            assert!(lay.buttons[0].y + lay.buttons[0].h <= h, "按钮出底边：{h}");
            assert!(
                lay.preview.x + lay.preview.w <= w - PAD + 0.01,
                "内容出右边：{w}"
            );
            // 两簇按钮在最小宽度下也不能贴在一起（这正是 MIN_W 的取值依据）。
            let gap = lay.buttons[1].x - (lay.buttons[0].x + lay.buttons[0].w);
            assert!(gap >= BUTTON_GAP * 2.0, "最小宽度下两簇贴上了，gap={gap}");
        }
    }

    /// 摘要里的"删后保留"必须与 `complement` 给的真值一致 —— 这是界面上唯一会误导用户的数字。
    #[test]
    fn summary_numbers_match_the_keep_plan() {
        let delays: Vec<u16> = vec![5; 100];
        let total = 100usize;
        let keep = gif_trim::complement(total, &[(0, 30)]);
        assert_eq!(gif_trim::plan_summary(&delays, &keep), (70, 350));
        let keep = gif_trim::complement(total, &[(30, 100)]);
        assert_eq!(gif_trim::plan_summary(&delays, &keep), (30, 150));
        let keep = gif_trim::complement(total, &[(10, 20), (60, 70)]);
        assert_eq!(gif_trim::plan_summary(&delays, &keep), (80, 400));
    }

    // ─────────────────── 离屏渲染（绕开鼠标注入与 UIPI） ───────────────────

    use crate::core::config::Config;
    use windui::render::{DibTarget, RenderTarget};
    use windui::text::DWriteEngine;

    /// 造一段 8×4 的三帧测试 GIF（黑 → 上半白 → 全白）。全局色表的 0 号是纯黑。
    fn write_test_gif(path: &PathBuf) {
        write_test_gif_n(path, 3);
    }

    /// 造 `n` 帧（三帧那个形状的推广：0 帧全黑、奇数帧上半白、偶数帧全白）。
    fn write_test_gif_n(path: &PathBuf, n: usize) {
        use gif::Frame;
        let (w, h) = (8u16, 4u16);
        let cells = (w * h) as usize;
        let pal: Vec<u8> = vec![0, 0, 0, 255, 255, 255];
        let file = std::fs::File::create(path).expect("建文件");
        let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w, h, &pal).expect("编码器");
        for k in 0..n {
            let mut buf = vec![0u8; cells];
            if k % 2 == 1 {
                for v in buf.iter_mut().take(cells / 2) {
                    *v = 1;
                }
            } else if k > 0 {
                buf.fill(1);
            }
            // ★ 与我们的录制产物同构：全局色表 + Keep + 首帧整幅。
            let f = Frame {
                width: w,
                height: h,
                buffer: buf.into(),
                delay: 5,
                dispose: gif::DisposalMethod::Keep,
                ..Default::default()
            };
            enc.write_frame(&f).expect("写帧");
        }
    }

    /// 建一个**已载入文件**的画布。状态在 `c.core` 里，测试直接借它。
    fn mk_canvas(tag: &str) -> (TrimCanvas, PathBuf) {
        let dir = std::env::temp_dir().join(format!("curseen_trimui_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let p = dir.join("t.gif");
        write_test_gif(&p);
        let rt = Rc::new(RefCell::new(Runtime::new(Config::default())));
        let core = Rc::new(RefCell::new(TrimCore::open(p.clone()).expect("开裁剪核心")));
        let c = TrimCanvas::new(
            core,
            rt,
            (WIN_W as f32, WIN_H as f32),
            signal(window_title("clip.gif")),
        );
        (c, p)
    }

    /// 建一个**空窗**画布（裁剪快捷键那条路：还没选文件）。
    fn mk_blank() -> TrimCanvas {
        let rt = Rc::new(RefCell::new(Runtime::new(Config::default())));
        TrimCanvas::new(
            Rc::new(RefCell::new(TrimCore::empty())),
            rt,
            (WIN_W as f32, WIN_H as f32),
            signal(window_title("")),
        )
    }

    /// 建一个**载入"带透明背景 GIF"**的画布（第三方贴纸的形态：透明下标 0 + 中间一块实色）。
    fn mk_transparent_canvas(tag: &str) -> (TrimCanvas, PathBuf) {
        use gif::Frame;
        let dir = std::env::temp_dir().join(format!("curseen_trimui_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let p = dir.join("alpha.gif");
        let (w, h) = (8u16, 4u16);
        let pal: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 0, 0];
        let mut buf = vec![0u8; (w * h) as usize];
        for y in 1..3usize {
            for x in 2..6usize {
                buf[y * w as usize + x] = 1; // 1 号色 = 白
            }
        }
        {
            let file = std::fs::File::create(&p).expect("建文件");
            let mut enc =
                gif::Encoder::new(std::io::BufWriter::new(file), w, h, &pal).expect("编码器");
            let f = Frame {
                width: w,
                height: h,
                buffer: buf.into(),
                delay: 5,
                dispose: gif::DisposalMethod::Keep,
                transparent: Some(0),
                ..Default::default()
            };
            enc.write_frame(&f).expect("写帧");
        }
        let rt = Rc::new(RefCell::new(Runtime::new(Config::default())));
        let core = Rc::new(RefCell::new(TrimCore::open(p.clone()).expect("开裁剪核心")));
        let c = TrimCanvas::new(
            core,
            rt,
            (WIN_W as f32, WIN_H as f32),
            signal(window_title("clip.gif")),
        );
        (c, p)
    }

    /// **透明像素在预览里必须透出深底**，不能变成一块实黑。
    ///
    /// 为什么值得一条像素门禁：预览是用户判断"我要存的这张图对不对"的**唯一**依据。预览把
    /// 透明画成 0 号色（多半是黑）、产物却是透明的 —— 用户会以为自己存的是一张黑底图，
    /// 而"预览与产物不一致"是最伤信任的一类问题（比两个都错还难查：看图的人会先怀疑眼睛）。
    #[test]
    fn preview_shows_transparent_pixels_as_the_backdrop() {
        let (c, path) = mk_transparent_canvas("alpha");
        let f = render(&c);
        let lay = c.lay();

        // 8×4 等比塞进预览框：高贴满 ⇒ 缩放 = 预览高 / 4，水平居中。
        let scale = lay.preview.h / 4.0;
        let x0 = lay.preview.x + (lay.preview.w - scale * 8.0) / 2.0;
        let y0 = lay.preview.y;

        // 图内左上角（原图 (0,0) 附近）是透明区。
        let tx = (x0 + scale * 0.5) as i32;
        let ty = (y0 + scale * 0.5) as i32;
        assert_eq!(
            rgb(&f, tx, ty),
            PREVIEW_BG,
            "透明像素该透出预览深底（实得 {:?} ≈ 被画成了 0 号色）",
            rgb(&f, tx, ty)
        );
        // 中间那块仍然是实色（白）—— 说明不是整幅都被"透掉"了。
        let cx = (x0 + scale * 4.0) as i32;
        let cy = (y0 + scale * 2.0) as i32;
        assert!(
            lum(rgb(&f, cx, cy)) > 200,
            "那块白色该还在，实得 {:?}",
            rgb(&f, cx, cy)
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    }

    /// 同一时刻**只允许一个原生对话框在飞**。
    ///
    /// ★ 守的是 2026-09-20 那场真事故：`ctx.defer_blocking` 的闭包是在**窗口过程内部**
    ///   执行的，而里面开的模态对话框会继续给我们的窗口泵消息 ⇒ 对话框开着时再点一次
    ///   「打开…」就在上一层模态循环里**套出第二层**对话框。用户看到的是"点了没反应"
    ///   （那时窗口还置顶，对话框全躲在它后面），日志里是三条连续"用户在「打开」里取消"。
    ///
    /// ★ 判据里那个"还名额"的一半比抢的一半更要紧：`begin_dialog` 漏还一次，「打开…」
    ///   与「保存」就**永久**失灵了，而且不报任何错。
    #[test]
    fn only_one_native_dialog_can_be_in_flight() {
        let mut c = TrimCore::empty();
        assert!(!c.dialog_open, "刚开窗时不该有对话框在飞");
        assert!(c.begin_dialog(), "第一次必须能抢到名额");

        // 第二次必须被拒 —— 这一行就是"套层"的总开关。
        assert!(!c.begin_dialog(), "已有对话框在飞时不能再抢到名额");
        assert!(c.dialog_open, "被拒的那次不该把状态改坏");

        // 用户取消 → 还名额 → 又能开。少了这一步，两个按钮从此永久失灵。
        c.end_dialog();
        assert!(!c.dialog_open);
        assert!(c.begin_dialog(), "归还之后必须还能再抢一次");

        // ★ 「换文件」（`load`）不许碰这个名额：它是**对话框状态**，不是文件状态。
        //   混在一起的话，"裁到一半换了份文件"就会白占掉一个名额。
        c.end_dialog();
        let dir = std::env::temp_dir().join(format!("curseen_trimui_guard_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("建目录");
        let p = dir.join("t.gif");
        write_test_gif(&p);
        c.load(p).expect("载入");
        assert!(!c.dialog_open, "载入文件不该动对话框名额");
        assert!(c.begin_dialog(), "载入之后名额该还是空的");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本文件的**生产代码**（`#[cfg(test)]` 之前那一段）的文本。
    ///
    /// `include_str!` 会把测试自己也包进来，所以必须切一刀 —— 否则下面那些"数出现次数"
    /// 的断言会被断言自己的文本影响（加一条断言就把计数改掉，这种门禁比没有还糟）。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("gif_trim.rs");
        let cut = SRC.find("#[cfg(test)]").expect("本文件有测试模块");
        &SRC[..cut]
    }

    /// 结构门禁：**按标题找窗的调用一律不能用 [`TITLE`] 常量**。
    ///
    /// 标题栏现在是动态的（"文档名 - 软件名"），而 `FindWindowW` 只认**完整标题**：任何一处
    /// 写成 `invalidate_by_title(TITLE)` 都会**静默**找不到窗口（不报错、只是不重画 / 不关窗 /
    /// 不聚焦），这类失效在日志里什么痕迹都没有。所以钉在源码结构上。
    #[test]
    fn by_title_calls_use_the_live_title_not_the_constant() {
        let p = production_src();
        for f in [
            "invalidate_by_title",
            "post_close_by_title",
            "focus_window_by_title",
        ] {
            assert!(
                !p.contains(&format!("{f}(TITLE)")),
                "{f}(TITLE) 用的是常量标题，窗口标题是动态的 → 必然找不到窗口"
            );
        }
        assert!(
            p.contains(".title(title_sig)"),
            "建窗时必须把标题信号绑给 windui，否则标题永远停在初始值"
        );
        // 取 `set_cur_title(&` 这种**调用形式**：函数定义那行也叫 `set_cur_title(`，
        // 光数名字会把定义本身算进去。
        assert_eq!(
            p.matches("set_cur_title(&").count(),
            2,
            "两处要同步 `CUR_TITLE`：建窗时（spawn）与载入新文件后（pick_file）"
        );
        assert!(
            p.contains("set_cur_title_raw(self.title.get())"),
            "活窗必须在 paint 里校准 `CUR_TITLE`（`.single()` 那条路会把它写歪）"
        );
    }

    /// 结构门禁：把三件**无头环境里观察不到**的事钉在源码结构上。
    ///
    /// 为什么不写行为门禁：本窗到底在不在置顶带，只能靠真机比对 `GetWindow` 的 z 序链；
    /// "对话框套层"要往自己的窗口注入点击 —— 两样在无头环境都做不到。可这三条一旦破掉，
    /// 症状是"点「打开…」毫无反应"，正是最难从日志反推的那一类。
    #[test]
    fn the_window_stays_out_of_the_topmost_band_and_always_guards_its_dialogs() {
        let p = production_src();

        // ① 开窗后的聚焦**不走**抢置顶带的那个函数。
        //    判据取**调用形式**（带 `win_util::` 与左括号）而不是光秃秃的函数名：注释里
        //    会正当地提到它（"不要用 activate_window_by_title，理由…"）。
        assert!(
            p.contains("focus_window_by_title(&cur_title())"),
            "裁剪窗开窗后必须用「不抢置顶带」的聚焦函数（标题是动态的，见 `TITLE` 注释）"
        );
        assert!(
            !p.contains("win_util::activate_window_by_title("),
            "裁剪窗被抬进置顶带之后，它自己弹的文件对话框就永远压在下面 —— 用户看不见也点不到"
        );

        // ② 两个弹对话框的入口各抢一次名额，且各在同一条出口归还。
        assert_eq!(
            p.matches(".begin_dialog()").count(),
            2,
            "「打开…」与「保存」各要抢一次名额"
        );
        assert_eq!(
            p.matches(".end_dialog()").count(),
            2,
            "抢几次就得还几次；漏一次这两个按钮就永久失灵"
        );

        // ③ 「打开…」的初始目录必须与录完落盘的地方**共用同一个常量** ——
        //    各写一个 `"gif"` 字面量时，改一处就会出现"录到 A、打开默认在 B"，
        //    而且什么都不报。
        assert!(
            p.contains("module_temp_dir(crate::core::paths::MODULE_GIF)"),
            "「打开…」的初始目录必须指向 GIF 产物目录，且与落盘处共用同一个常量"
        );
    }

    /// 把裁剪画布**离屏渲成一张不透明位图**（dpi=1，与窗口逻辑坐标 1:1）。
    ///
    /// 为什么必须用像素说话："预览里到底有没有图""轨道上那段红到底画在哪""按钮到底有没有
    /// 底色"，几何上都看不出来 —— 而这三件恰恰是用户一眼就会看到的。这也完全绕开了
    /// 真机验收里最难受的一环（往窗口注入鼠标）。
    fn render(c: &TrimCanvas) -> Vec<u8> {
        const PW: u32 = WIN_W as u32;
        const PH: u32 = WIN_H as u32;
        let mut ann = vec![0u8; (PW * PH * 4) as usize];
        let mut engine = DWriteEngine::new();
        {
            let mut target = DibTarget::new(&mut ann, PW, PH);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            Widget::paint(
                c,
                Rect::new(0, 0, PW as i32, PH as i32),
                Rect::new(0, 0, PW as i32, PH as i32),
                false,
                true,
                &mut *canvas,
                &Style::default(),
            );
        }
        ann
    }

    fn rgb(buf: &[u8], x: i32, y: i32) -> (u8, u8, u8) {
        let i = ((y as u32 * WIN_W as u32 + x as u32) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    fn lum(p: (u8, u8, u8)) -> u32 {
        (p.0 as u32 * 299 + p.1 as u32 * 587 + p.2 as u32 * 114) / 1000
    }

    fn near(a: (u8, u8, u8), b: (u8, u8, u8), tol: i32) -> bool {
        (a.0 as i32 - b.0 as i32).abs() <= tol
            && (a.1 as i32 - b.1 as i32).abs() <= tol
            && (a.2 as i32 - b.2 as i32).abs() <= tol
    }

    /// 面板底色 + **预览真的把图画出来了** + 五个按钮都占位。
    #[test]
    fn offscreen_render_shows_panel_preview_and_buttons() {
        let (c, path) = mk_canvas("base");
        c.core.borrow_mut().set_preview(0);
        let f = render(&c);
        let lay = c.lay();

        // ① 左上角是纯白面板（标题栏区域）。
        assert_eq!(rgb(&f, 3, 3), PANEL, "标题栏底色应是面板白");

        // ② 预览区：8×4 的图按高度塞进 688×232 → 图宽 464、左右各留 ~112px 深底；
        //    正中则是第 0 帧的内容（纯黑）。两处一起断言 = "铺了深底 **且** 画了图"。
        let py = (lay.preview.y + lay.preview.h / 2.0) as i32;
        assert_eq!(
            rgb(&f, (lay.preview.x + 3.0) as i32, py),
            PREVIEW_BG,
            "预览区左侧应是深底（图没铺满整框）"
        );
        let cx = (lay.preview.x + lay.preview.w / 2.0) as i32;
        assert!(
            lum(rgb(&f, cx, py)) < 40,
            "预览正中应是第 0 帧的画面（近黑），实得 {:?}",
            rgb(&f, cx, py)
        );

        // ③ 五个按钮都得有底色（不能是面板白，否则等于没画）。
        assert_eq!(lay.buttons.len(), BUTTON_N);
        for (i, b) in lay.buttons.iter().enumerate() {
            let p = rgb(&f, (b.x + b.w / 2.0) as i32, (b.y + b.h / 2.0) as i32);
            assert!(
                !near(p, PANEL, 2),
                "第 {i} 个按钮没有底色（说明没画），采样 {p:?}"
            );
        }

        let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    }

    /// **空窗**（裁剪快捷键那条路）也得是一张"看起来正常"的界面：
    /// 深底预览 + 一句提示 + 轨道灰底 + 五个按钮都在，且只有「保存/删除/撤销」是灰的。
    ///
    /// 空窗是新加的入口，用户很可能第一眼看到的就是它 —— 所以它和"有文件"一样要有像素门禁。
    #[test]
    fn offscreen_render_shows_a_clean_blank_state() {
        let c = mk_blank();
        let f = render(&c);
        let lay = c.lay();

        // ① 预览区：整块深底，**一个字的墨都没有**——空窗不加任何提示（2026-09-30 用户
        //    定稿："提示太多余，标题栏有了就行"。状态由 `未打开文件 - CurSeen 裁剪 GIF`
        //    这个系统标题栏承担）。
        let mut hint_px = 0;
        for y in (lay.preview.y as i32)..((lay.preview.y + lay.preview.h) as i32) {
            for x in (lay.preview.x as i32)..((lay.preview.x + lay.preview.w) as i32) {
                if lum(rgb(&f, x, y)) > 90 {
                    hint_px += 1;
                }
            }
        }
        assert_eq!(
            hint_px, 0,
            "空窗预览区不该有任何提示文字，实得 {hint_px} px"
        );

        // ② 空窗不画把手/播放头/刻度 —— 没有帧就没有"哪一段"可言。
        let ty = (lay.track.y + (lay.track.h - TRACK_H) / 2.0 + TRACK_H / 2.0) as i32;
        for k in 0..=8 {
            let x = (lay.track.x + lay.track.w * k as f32 / 8.0) as i32;
            let p = rgb(&f, x.min(WIN_W - 2), ty);
            assert!(
                !near(p, ACCENT, 30) && !near(p, DANGER, 30),
                "空窗的轨道上不该有把手/红段：x={x} 采到 {p:?}"
            );
        }

        // ③ 「打开…」必须是**可用**的（高亮底色），否则用户在这个界面上无路可走。
        let b0 = lay.buttons[0];
        let p0 = rgb(&f, (b0.x + b0.w / 2.0) as i32, (b0.y + b0.h / 2.0) as i32);
        assert!(!near(p0, PANEL, 2), "「打开…」没有底色");
        assert!(
            !near(p0, BORDER, 6),
            "「打开…」是灰的（当前不可用）—— 空窗里这是唯一出路，必须可点"
        );
    }

    /// **端到端**：开任意 GIF → 划掉一段 → 「保存」→ 用**独立解码器**重开产物。
    ///
    /// 这条补的是"三块各自测过、拼起来没用过"的缝：`TrimCore`（选择 → `keep_plan`）与
    /// `gif_trim::rewrite_to`（落盘）之间的契约只有两句话——"keep 是帧区间"与"原片只读"，
    /// 而用户真正拿到的东西正是这两者相乘。
    ///
    /// 为什么不去跑原生对话框：「另存为」对话框只是**问用户要一个落点**，不改变任何语义
    /// （`rewrite_to` 的入参本来就是一个 `dst: PathBuf`）。无头环境里点不了它，但可以
    /// 直接接上它下游的那个接口。落点特意用**含空格的非 ASCII 名**，那是路径处理最容易
    /// 出错的一类输入。
    #[test]
    fn open_trim_save_as_then_reopen_the_product() {
        let (c, src) = mk_canvas("e2e"); // mk_canvas 的素材是 3 帧
        let before = std::fs::read(&src).expect("读原片");

        // 划掉第 1 帧（边界区间 1..2）并确认删除。
        let (keep, kept_frames) = {
            let mut st = c.core.borrow_mut();
            st.set_ab(1, 2);
            st.commit_delete();
            (st.keep_plan(), st.plan_stats().0)
        };
        assert_eq!(
            keep,
            vec![(0, 1), (2, 3)],
            "保留区间应是 '第 0 帧' + '第 2 帧'"
        );
        assert_eq!(kept_frames, 2, "3 帧删 1 帧该剩 2 帧");

        let dst = src.parent().expect("父目录").join("裁剪后 新.gif");
        let rep = gif_trim::rewrite_to(&src, &keep, &dst).expect("另存为");
        assert_eq!(rep.frames, 2, "产物该只剩 2 帧");
        assert_eq!(rep.dst, dst, "落点要说真话（用户可能在对话框里改过名）");

        // 产物能重新打开、帧数对得上 —— 用一个**独立**的 FrameReader 读回来，
        // 不信任写入侧报的数（"写进去的"和"读得出来的"是两件事）。
        let mut rd = FrameReader::open(&dst).expect("重开产物");
        assert_eq!(rd.frame_count().expect("帧数"), 2);

        // 原片**一个字节都没动**：这是"另存为"这条需求真正要保住的东西。
        assert_eq!(
            std::fs::read(&src).expect("再读原片"),
            before,
            "原片被改写了 —— 另存为的硬约束破了"
        );

        let _ = std::fs::remove_dir_all(src.parent().expect("父目录"));
    }

    /// 在**同一个窗口**里换文件：内容要整套换掉（帧数、选择、预览），
    /// 而不是把新文件叠在旧的进度上。
    #[test]
    fn loading_a_second_file_resets_the_edit_state() {
        let (c, path) = mk_canvas("swap");
        // 先在旧文件上划一段并确认删除。
        {
            let mut st = c.core.borrow_mut();
            st.set_ab(0, 1);
            st.commit_delete();
            assert_eq!(st.removed.len(), 1);
        }
        // 换一份更大的文件（8 帧）。
        let dir = path.parent().expect("父目录");
        let big = dir.join("big.gif");
        write_test_gif_n(&big, 8);
        c.core.borrow_mut().load(big.clone()).expect("换文件");

        // ★ `Ref` 实现了 `Drop` ⇒ NLL **不能**把它缩短到"最后一次使用"处，借用一直活到
        //   `st` 的作用域结束。所以下面必须显式分块，否则后一次 `borrow_mut` 会撞上
        //   "already borrowed"（与 `MutexGuard` 要显式 drop 是同一件事）。
        {
            let st = c.core.borrow();
            assert_eq!(st.total, 8, "总帧数要跟着换");
            assert!(st.removed.is_empty(), "旧文件上的删除记录必须清掉");
            assert_eq!((st.a, st.b), (0, 0), "旧选择必须清掉");
            assert_eq!(st.preview_k, 0, "预览回到第一帧");
            assert!(st.preview.is_some(), "换完文件该立刻有预览");
            assert_eq!(st.file_name(), "big.gif");
        }

        // 载入一个坏文件：**旧内容必须原样留着**，只挂个原因。
        let bad = dir.join("bad.gif");
        std::fs::write(&bad, b"not a gif").expect("写坏文件");
        let e = c.core.borrow_mut().load(bad);
        assert!(e.is_err(), "坏文件该报错");
        {
            let st = c.core.borrow();
            assert_eq!(st.total, 8, "载入失败不该把手上这份弄丢");
            assert_eq!(st.file_name(), "big.gif");
            assert!(st.status.is_some(), "失败原因要能显示出来");
        }

        let _ = std::fs::remove_dir_all(dir);
    }

    /// **待删区间必须精确画在轨道上对应的那一段** —— 这是"划到哪删到哪"唯一的视觉凭证。
    #[test]
    fn offscreen_render_paints_the_pending_segment_where_it_was_selected() {
        let (c, path) = mk_canvas("track");
        let total = c.core.borrow().total;
        assert_eq!(total, 3, "测试素材是 3 帧");
        let lay = c.lay();
        let ty = (lay.track.y + (lay.track.h - TRACK_H) / 2.0 + TRACK_H / 2.0) as i32;

        // 没选之前：轨道中段应是中性灰，不该有红。
        let f0 = render(&c);
        let mid_before = rgb(&f0, (lay.track.x + lay.track.w / 2.0) as i32, ty);
        assert!(
            !near(mid_before, DANGER, 40),
            "还没划选就出现了红：{mid_before:?}"
        );

        // 划选第 1..2 帧（中间那一格）。
        c.core.borrow_mut().set_ab(1, 2);
        let f1 = render(&c);
        let x1 = bound_to_x(1, total, lay.track);
        let x2 = bound_to_x(2, total, lay.track);
        let mid = ((x1 + x2) / 2.0) as i32;
        let sel = rgb(&f1, mid, ty);
        assert!(
            near(sel, DANGER, 30),
            "选中段中点应是正红 {DANGER:?}，实得 {sel:?}"
        );
        // 选中段之外仍是灰（不能整条都红）。
        let outside = rgb(&f1, (lay.track.x + 4.0) as i32, ty);
        assert!(
            !near(outside, DANGER, 30),
            "未选中的左端也被染红了：{outside:?}"
        );
        // 端点位置也必须对得上：x1/x2 之间是红，紧邻 x1 左侧仍是灰。
        let left_of = rgb(&f1, (x1 - 6.0) as i32, ty);
        assert!(!near(left_of, DANGER, 30), "红色越过了左边界：{left_of:?}");

        let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    }

    /// 已确认删除的区间要**换一种颜色**画，否则用户分不清"待删"和"已删"。
    #[test]
    fn offscreen_render_distinguishes_committed_from_pending_removal() {
        let (c, path) = mk_canvas("done");
        let lay = c.lay();
        let ty = (lay.track.y + (lay.track.h - TRACK_H) / 2.0 + TRACK_H / 2.0) as i32;
        let total = {
            let mut st = c.core.borrow_mut();
            st.set_ab(0, 1);
            st.commit_delete();
            st.total
        };
        assert_eq!(c.core.borrow().removed, vec![(0, 1)]);
        let f = render(&c);
        let x0 = bound_to_x(0, total, lay.track);
        let x1 = bound_to_x(1, total, lay.track);
        let p = rgb(&f, ((x0 + x1) / 2.0) as i32, ty);
        assert!(
            near(p, DANGER_DONE, 30),
            "已删区间应是浅红 {DANGER_DONE:?}，实得 {p:?}"
        );
        assert!(
            !near(p, DANGER, 12),
            "已删与待删的颜色必须能区分开（现在两者一样）"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    }

    /// 保存计划把已删段与待删段**合起来**算 —— 两处都要出现在 `keep_plan` 里。
    #[test]
    fn keep_plan_merges_committed_and_pending_removals() {
        let (c, path) = mk_canvas("plan");
        // 3 帧：删掉第 0 帧（已确认）+ 第 2 帧（待删）→ 只剩第 1 帧。
        {
            let mut st = c.core.borrow_mut();
            st.set_ab(0, 1);
            st.commit_delete();
            st.set_ab(2, 3);
        }
        assert_eq!(
            c.core.borrow().keep_plan(),
            vec![(1, 2)],
            "已删段与待删段都要进保留计划"
        );
        assert_eq!(
            c.core.borrow().plan_stats(),
            (1, 5),
            "只剩 1 帧 / 0.05 秒 → 5 厘秒"
        );
        assert_eq!(c.core.borrow().pending_cs(), 5);

        // 全删光：保留计划为空，保存必须被拒（界面上"保存"也据此置灰）。
        c.core.borrow_mut().set_ab(0, 3);
        assert!(c.core.borrow().keep_plan().is_empty());
        assert_eq!(c.core.borrow().plan_stats().0, 0);
        let _ = std::fs::remove_dir_all(path.parent().expect("父目录"));
    }

    /// 把裁剪窗**离屏渲成一张 PNG** 供人眼评审（不进回归）。
    ///
    /// ```text
    /// cargo test --release --lib dump_trim_window_png -- --ignored --nocapture
    /// ```
    ///
    /// 产出 `temp/gif_ui/window.png` 并在 stdout 打印路径。★ 只落图、**不做断言** ——
    /// 断言归上面那三条离屏门禁。它存在的理由很实际：真机截图这条路被 UIPI 断了
    /// （见 `examples/render_probe.rs` 头部说明），而"这界面长什么样"恰好是评审
    /// 第一眼要看的东西 —— 那就把窗口画出来存成图，别让评审者去装环境。
    #[test]
    #[ignore = "只产出 PNG 供人看，不进回归"]
    fn dump_trim_window_png() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("temp")
            .join("gif_ui");
        std::fs::create_dir_all(&dir).expect("建目录");
        let src = dir.join("scenic.gif");

        // 造一段 240×140、12 帧、10cs/帧的"风景"GIF：上半深底、下半中灰，
        // 一条彩色横带每帧右移 —— 预览里能一眼看出"哪一帧、画面在动"。
        {
            let (w, h) = (240u16, 140u16);
            let pal: Vec<u8> = vec![
                40, 44, 52, // 0 深底
                90, 96, 108, // 1 中灰
                230, 80, 70, // 2 红
                70, 180, 120, // 3 绿
                240, 200, 70, // 4 黄
                60, 120, 230, // 5 蓝
            ];
            let file = std::fs::File::create(&src).expect("建文件");
            let mut enc =
                gif::Encoder::new(std::io::BufWriter::new(file), w, h, &pal).expect("编码器");
            let (wi, hi) = (w as usize, h as usize);
            for k in 0..12usize {
                let mut buf = vec![1u8; wi * hi];
                for y in 0..hi / 2 {
                    for x in 0..wi {
                        buf[y * wi + x] = 0;
                    }
                }
                let bx = 8 + k * 16;
                for y in hi / 2 + 10..hi / 2 + 40 {
                    for x in bx..(bx + 48).min(wi) {
                        buf[y * wi + x] = (2 + k % 4) as u8;
                    }
                }
                let f = gif::Frame {
                    width: w,
                    height: h,
                    buffer: buf.into(),
                    delay: 10,
                    dispose: gif::DisposalMethod::Keep,
                    ..Default::default()
                };
                enc.write_frame(&f).expect("写帧");
            }
        }

        let rt = Rc::new(RefCell::new(Runtime::new(Config::default())));
        let core = Rc::new(RefCell::new(TrimCore::open(src).expect("开裁剪核心")));
        let c = TrimCanvas::new(
            core,
            rt.clone(),
            (WIN_W as f32, WIN_H as f32),
            signal(window_title("clip.gif")),
        );
        // 摆一个"已删段 + 待删段"的混合状态：PNG 里要能同时看到浅红（已删）与正红（待删），
        // 以及摘要行随之变化的数字。
        {
            let mut st = c.core.borrow_mut();
            st.set_preview(4);
            st.set_ab(1, 3);
            st.commit_delete();
            st.set_ab(7, 9);
        }
        dump(&c, &dir.join("window.png"));

        // 空窗（裁剪快捷键那条路）也存一张：它是新入口，评审第一眼要看的就是它。
        let blank = TrimCanvas::new(
            Rc::new(RefCell::new(TrimCore::empty())),
            rt.clone(),
            (WIN_W as f32, WIN_H as f32),
            signal(window_title("")),
        );
        dump(&blank, &dir.join("window_blank.png"));

        // 第三张：**带透明背景的第三方 GIF**。它跟"我们的录制产物"是两种输入，
        // 预览里"透出来的那块"正是评审要确认的东西（见 `preview_shows_transparent_pixels_...`）。
        let (alpha, alpha_path) = mk_transparent_canvas("dump");
        dump(&alpha, &dir.join("window_alpha.png"));
        let _ = std::fs::remove_dir_all(alpha_path.parent().expect("父目录"));
    }

    /// 把画布渲成位图写进 PNG（只给人看，不做断言）。
    fn dump(c: &TrimCanvas, out: &PathBuf) {
        let rgba = render(c);
        let file = std::fs::File::create(out).expect("建 PNG");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), WIN_W as u32, WIN_H as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("PNG 头");
        wr.write_image_data(&rgba).expect("PNG 数据");
        wr.finish().expect("PNG 收尾");
        println!("裁剪窗 PNG: {}", out.display());
    }
}
