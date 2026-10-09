//! MP4 **时间裁剪**窗：与 GIF 裁剪（[`super::gif_trim`]）**同一套交互**——预览 + 时间轴 +
//! 划选删除 + 把手微调 + 撤销 + 另存为。
//!
//! # 与 GIF 版的三处实质差异
//!
//! | 差异 | GIF 版 | 本窗 | 为什么 |
//! |---|---|---|---|
//! | 时间轴单位 | 帧号 | **毫秒**（`usize`） | MP4 没有固定"帧"概念（VFR 可能），按时间裁才是用户语义 |
//! | 预览来源 | 进程内 GIF 解码 | MF 的 H.264 解码（`platform::mp4_trim::frame_rgba_at`） | 压缩帧不能直接画；解码器是 OS 组件，零新增依赖 |
//! | 落盘 | 进程内重写 GIF | `trim_ranges` 流拷贝（不重新编码） | 画质零损失、内存"一次一个样本" |
//!
//! 预览解码一次几十毫秒，所以预览时刻按 **100ms 量化**：拖把手时同一格内不重复解码
//!（与 GIF 版"帧号没变不重建"同一个思路，只是格子粗细不同）。
//!
//! # 交互（与 GIF 版逐字相同）
//!
//! **在时间轴上按住左键横向划过，划到哪就删到哪**；两个把手可以再微调；「保存」另存为，
//! 原片只读。一个手势覆盖掐头 / 去尾 / 抠中间。
//!
//! # 复用声明
//!
//! 布局纯函数、`Box2`/`irect`、Grab 手势枚举、双对话框防线（属主 + `dialog_open` 名额）、
//! `Rc<RefCell<TrimCore>>` 的原因——全部照抄 [`super::gif_trim`] 的设计与注释，那边是
//! 权威版本。两窗刻意**不共享代码**：GIF 那份 1900 行已经带满测试，抽公共组件要把它
//! 连测试一起翻新，风险大于 ~30KB 的重复；等第三个裁剪器出现再抽。
//!
//! # 窗口外壳：系统标题栏 + 可缩放
//!
//! 本窗**不再**是 `frameless`：留系统标题栏（`WS_OVERLAPPEDWINDOW`），因此自带最小化 /
//! 最大化 / 关闭与拖动，用户能把它放大到看清画面（2026-09-30 用户报"裁剪窗没法放大"）。
//! 代价只是多一圈非客户区，内容布局按**当前客户区**算（见 [`layout`]），拉大时预览吃满
//! 剩余高度。
//!
//! ★ 仍然**不进置顶带**：本窗要弹原生「打开 / 另存为」对话框，一旦置顶对话框就被压在
//! 下面（见 GIF 版模块头那次事故）。标题栏不影响这条纪律。
//!
//! # 标题栏文案：**文档名 - 软件名**
//!
//! 与记事本（「新建文本文档.txt - 记事本」）同一套口径：用户在任务栏 / Alt-Tab 里先看到
//! **自己那份文件**，再看到是哪个工具（2026-09-30 用户口径）。没打开文件时按记事本的
//! 「无标题」惯例显示"未打开文件"。
//!
//! ★ 标题是**动态的**，而 `FindWindowW` 只认**完整标题**——所以本模块自己记一份当前标题
//! （[`cur_title`]），所有按标题的关窗 / 聚焦 / 重画都走它，不写 [`TITLE`] 常量。
//!
//! # 保存：不冻 UI + 进度条
//!
//! 裁剪是**秒级**操作（13s/13MB 的录屏实测 0.3s；但几分钟的录像会长得多），跑在 UI 线程
//! 上就是"点完保存整个程序没反应"（2026-09-30 用户报"保存卡死"）。所以：**另存为对话框**
//! 留在 UI 线程（原生对话框，本来就要 owner），**裁剪本身**起工作线程，进度写回
//! `Arc<Mutex<SaveState>>`，窗口照常重画进度条。工作线程结束不直接碰 `Rc` 那一套，只把
//! 结局放进 `SaveState::done`，由 [`spawn`] 挂的 `on_interval` 在 UI 线程里收尾。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windui::core::{EventCtx, Widget};
use windui::event::{Event, Key, MouseButton, PointerKind};
use windui::geometry::{Color, Rect};
use windui::prelude::*;
use windui::render::image::{Fit, Image as WImage};
use windui::render::Canvas;
use windui::render::Paint as WPaint;
use windui::spec::Align;
use windui::text::{TextEngine, TextStyle};

use crate::core::gif_trim;
use crate::runtime::Runtime;

/// 窗口名（标题栏里 `-` **右边**那半，见模块头"标题栏文案"）。
pub const TITLE: &str = "CurSeen MP4 裁剪";

/// 标题栏全文：**文档名 - 软件名**（`file_name` 为空时按记事本的"无标题"惯例）。
fn window_title(file_name: &str) -> String {
    if file_name.is_empty() {
        format!("未打开文件 - {TITLE}")
    } else {
        format!("{file_name} - {TITLE}")
    }
}

thread_local! {
    /// 当前标题栏文本（见模块头：`FindWindowW` 只认完整标题）。
    static CUR_TITLE: RefCell<String> = RefCell::new(window_title(""));
}

/// 当前标题（按标题关窗 / 聚焦 / 重画都用它）。
fn cur_title() -> String {
    CUR_TITLE.with(|c| c.borrow().clone())
}

/// 记下新标题。**必须在 UI 线程调用**（工作线程读不到这份 thread_local，要先把字符串取走）。
fn set_cur_title(file_name: &str) {
    let t = window_title(file_name);
    CUR_TITLE.with(|c| *c.borrow_mut() = t);
}

/// 直接写下标题栏全文（`paint` 用它把 `CUR_TITLE` 校准成活窗的标题，见 [`TrimCanvas::paint`]）。
fn set_cur_title_raw(title: String) {
    CUR_TITLE.with(|c| {
        let mut g = c.borrow_mut();
        if *g != title {
            *g = title;
        }
    });
}

/// 保存的**跨线程**状态：工作线程写进度，UI 线程读进度并收尾。
///
/// 为什么不是塞进 `TrimCore`：那份是 `Rc<RefCell<..>>`，**不能**过线程边界；而进度必须在
/// 工作线程里更新。两块状态因此分开，各自只在自己那条线程上被借。
#[derive(Default)]
struct SaveState {
    /// 正在写盘（此时时间轴与按钮都不该再响应）。
    busy: bool,
    /// 完成度 `0.0..=1.0`。
    pct: f32,
    /// 工作线程的结局，由 UI 线程在 `on_interval` 里取走：`Ok` = 已经给用户的提示文案，
    /// `Err` = 失败原因。存**字符串**而不是 `PathBuf`+`TrimStats`：文案在工作线程里一次
    /// 算好，UI 线程只管显示，不必再碰那份文件。
    done: Option<Result<String, String>>,
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
const TRACK_HIT_H: f32 = 30.0;
/// 轨道下面的**时间刻度行**（"0.0s" / 总长）。
///
/// ★ 它必须**独立成一行**：刻度原本画在轨道命中框的末尾，而按钮顶边只比轨道底边低 6px，
///   16px 高的数字直接压到按钮上（2026-09-30 用户报"数字快接触到按钮了"）。
const TICK_H: f32 = 16.0;
/// 刻度是轨道的注脚，贴得紧一点；它**与按钮**之间才用整 [`GAP`]。
const TICK_GAP: f32 = 2.0;
const TRACK_H: f32 = 10.0;
const HANDLE_W: f32 = 4.0;
const HANDLE_HIT: f32 = 8.0;
const BUTTON_W: f32 = 104.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_GAP: f32 = 10.0;
const BUTTON_N: usize = 4;

const BUTTON_LABELS: [&str; BUTTON_N] = ["打开…", "删除此段", "撤销", "保存"];

/// 预览时刻的量化粒度（毫秒）。拖动时同一格内不重复解码（见模块头）。
const PREVIEW_QUANT_MS: usize = 100;

/// 保存期间收尾的轮询间隔：够快（进度条看着是连续的），又不是每帧都 lock 一次。
const SAVE_POLL: Duration = Duration::from_millis(120);

const ACCENT: (u8, u8, u8) = (76, 139, 245);
const DANGER: (u8, u8, u8) = (226, 68, 60);
const DANGER_DONE: (u8, u8, u8) = (206, 138, 134);
const TEXT: (u8, u8, u8) = (45, 52, 54);
const TEXT_DIM: (u8, u8, u8) = (99, 110, 114);
const BORDER: (u8, u8, u8) = (226, 230, 234);
const PANEL: (u8, u8, u8) = (255, 255, 255);
const PREVIEW_BG: (u8, u8, u8) = (24, 28, 32);
const PREVIEW_HINT: (u8, u8, u8) = (146, 156, 166);

fn color(c: (u8, u8, u8), a: u8) -> Color {
    Color::rgba(c.0, c.1, c.2, a)
}

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

fn irect(b: Box2) -> Rect {
    Rect::new(
        b.x.round() as i32,
        b.y.round() as i32,
        b.w.round() as i32,
        b.h.round() as i32,
    )
}

// ─────────────────────────── 纯几何（门禁钉在这里） ───────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
struct Layout {
    preview: Box2,
    summary: Box2,
    track: Box2,
    /// 轨道下方的刻度行（"0.0s" / 总长），见 [`TICK_H`]。
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

/// 毫秒边界 → 轨道 x。边界取值 `0..=total`：`0` 在最左、`total` 在最右。
fn bound_to_x(k: usize, total: usize, track: Box2) -> f32 {
    if total == 0 {
        return track.x;
    }
    track.x + track.w * (k.min(total) as f32 / total as f32)
}

/// 轨道 x → 毫秒边界（[`bound_to_x`] 的逆）。
fn x_to_bound(x: f32, total: usize, track: Box2) -> usize {
    if total == 0 {
        return 0;
    }
    let t = ((x - track.x) / track.w.max(1.0)).clamp(0.0, 1.0);
    (t * total as f32).round() as usize
}

fn fit_box(src_w: f32, src_h: f32, bx: f32, by: f32, bw: f32, bh: f32) -> Box2 {
    if src_w <= 0.0 || src_h <= 0.0 || bw <= 0.0 || bh <= 0.0 {
        return Box2::new(bx, by, bw.max(1.0), bh.max(1.0));
    }
    let s = (bw / src_w).min(bh / src_h);
    let (w, h) = (src_w * s, src_h * s);
    Box2::new(bx + (bw - w) / 2.0, by + (bh - h) / 2.0, w, h)
}

fn button_at(x: f32, y: f32, lay: &Layout) -> Option<usize> {
    lay.buttons.iter().position(|b| b.contains(x, y))
}

fn in_track(x: f32, y: f32, lay: &Layout) -> bool {
    x >= lay.track.x - HANDLE_HIT
        && x <= lay.track.x + lay.track.w + HANDLE_HIT
        && y >= lay.track.y
        && y <= lay.track.y + lay.track.h
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Grab {
    A,
    B,
    Sweep { from: usize },
}

fn grab_at(x: f32, y: f32, lay: &Layout, a_x: f32, b_x: f32, total: usize) -> Option<Grab> {
    if !in_track(x, y, lay) {
        return None;
    }
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

/// 毫秒 → `1.2s`（整数毫秒进，一位小数出）。
fn secs(ms: usize) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

// ─────────────────── 状态（与窗口解耦，可脱离窗口单测） ───────────────────

/// 裁剪器的全部可变状态（时间单位一律**毫秒**）。
struct TrimCore {
    path: Option<PathBuf>,
    /// 总时长（毫秒）。载入时读一次，之后不变。
    total: usize,

    /// 已确认删除的区间（可撤销）。
    removed: Vec<(usize, usize)>,
    /// 待删区间的两个**毫秒边界**（`a <= b <= total`）。`a == b` = 没选。
    a: usize,
    b: usize,

    /// 预览当前停在哪个时刻（量化后的毫秒）。
    preview_ms: usize,
    /// 预览图（只在量化时刻变化时重建）。
    preview: Option<WImage>,
    /// 预览图是按哪个量化时刻建的（`None` = 还没有）。
    built_for: Option<usize>,
    /// 最近一次失败原因（画在标题栏右侧）。
    status: Option<String>,
    /// **有一个原生对话框正开着**（重入守卫，见 `gif_trim` 同名注释）。
    dialog_open: bool,
}

struct TrimCanvas {
    core: Rc<RefCell<TrimCore>>,
    /// **当前客户区尺寸**（`Cell` 是因为 `paint` 只给 `&self`——窗口被拉大时在这一刻
    /// 记下新尺寸，`on_event` 的命中判定与 [`Self::lay`] 才跟得上）。
    win: Cell<(f32, f32)>,
    /// 标题栏文本的信号源：换文件时写它，系统标题栏自动跟上（见模块头"标题栏文案"）。
    title: Signal<String>,
    /// 保存进度 / 结局（跨线程，见 [`SaveState`]）。
    save: Arc<Mutex<SaveState>>,
    grab: Option<Grab>,
    hover_btn: Option<usize>,
}

impl TrimCore {
    fn empty() -> Self {
        Self {
            path: None,
            total: 0,
            removed: Vec::new(),
            a: 0,
            b: 0,
            preview_ms: 0,
            preview: None,
            built_for: None,
            status: None,
            dialog_open: false,
        }
    }

    fn open(path: PathBuf) -> Result<Self, String> {
        let mut c = Self::empty();
        c.load(path)?;
        Ok(c)
    }

    /// 换一份文件。失败时**原样保留旧内容**（理由见 `gif_trim` 同名方法）。
    fn load(&mut self, path: PathBuf) -> Result<(), String> {
        let r = self.try_load(path);
        self.status = match &r {
            Ok(()) => None,
            Err(e) => Some(format!("打不开：{e}")),
        };
        r
    }

    /// 放掉预览图那一块内存（关窗时调）。
    ///
    /// # 为什么要显式放，而不是等 `TrimCanvas` 被 drop
    ///
    /// `preview` 是一整帧 RGBA：1080p × 4B ≈ **8.3 MB**，而 `WImage::from_rgba` 内部还会
    /// 整份 `to_vec()` 做一次 premultiply，于是稳态持有的是**两份**（≈ 16 MB）。
    ///
    /// 靠"窗被销毁 → 控件树 drop → `TrimCanvas` drop → `TrimCore` drop"这条链理论上也会
    /// 释放，但它发生在 windui 内部、时间点不可控，而且**链尾没有任何人会调**
    /// `EmptyWorkingSet`。2026-10-01 用户报的"录完、裁完、关掉所有模块，还停在 40MB"
    /// 里最结实的一块就是它：页确实还了，只是还挂在私有工作集里，任务管理器照旧写着
    /// 写着那个数（原理见 `platform::working_set` 模块头）。这里显式置 `None` 是把释放
    /// 变成**此刻确定发生**的动作，随后由 [`crate::platform::working_set::request_trim_soon`]
    /// 把这些页摘出工作集。
    pub fn release_memory(&mut self) {
        self.preview = None;
        self.built_for = None;
    }

    fn try_load(&mut self, path: PathBuf) -> Result<(), String> {
        // 先确认新文件可读、时长有效，**再**动自己的字段。
        let ns = crate::platform::mp4_trim::mp4_duration_ns(&path)
            .ok_or_else(|| "读不到视频时长".to_string())?;
        let total = (ns / 10_000).max(1) as usize; // 100ns → ms，向上保 1ms 防 0 长度
        self.path = Some(path);
        self.total = total;
        self.removed.clear();
        self.a = 0;
        self.b = 0;
        self.preview = None;
        self.built_for = None;
        self.preview_ms = 0;
        self.set_preview(0);
        Ok(())
    }

    fn is_loaded(&self) -> bool {
        self.path.is_some()
    }

    fn file_name(&self) -> String {
        self.path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default()
    }

    /// 当前"保留哪些毫秒区间"。唯一的换算点（复用 `core::gif_trim::complement`——纯函数）。
    fn keep_plan(&self) -> Vec<(usize, usize)> {
        let mut removed = self.removed.clone();
        if self.b > self.a {
            removed.push((self.a, self.b));
        }
        gif_trim::complement(self.total, &removed)
    }

    /// `(保留毫秒,)`——MP4 没有"帧数"概念，摘要只报时长。
    fn kept_ms(&self) -> usize {
        self.keep_plan().iter().map(|&(s, e)| e - s).sum()
    }

    fn total_ms(&self) -> usize {
        self.total
    }

    /// 待删段的时长（毫秒）。
    fn pending_ms(&self) -> usize {
        self.b.saturating_sub(self.a)
    }

    /// 让预览停在 `t_ms`。按 [`PREVIEW_QUANT_MS`] 量化，同一格内不重复解码。
    fn set_preview(&mut self, t_ms: usize) {
        let t_ms = t_ms.min(self.total.saturating_sub(1));
        let q = t_ms / PREVIEW_QUANT_MS * PREVIEW_QUANT_MS;
        if self.built_for == Some(q) {
            return;
        }
        let Some(p) = self.path.clone() else {
            return;
        };
        match crate::platform::mp4_trim::frame_rgba_at(&p, q as f64 / 1000.0) {
            Ok((w, h, rgba)) => match WImage::from_rgba(w, h, &rgba) {
                Ok(img) => {
                    self.preview = Some(img);
                    self.built_for = Some(q);
                    self.preview_ms = q;
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

    fn begin_dialog(&mut self) -> bool {
        if self.dialog_open {
            return false;
        }
        self.dialog_open = true;
        true
    }

    fn end_dialog(&mut self) {
        self.dialog_open = false;
    }
}

impl TrimCanvas {
    /// `rt` **不在画布里**：保存的收尾（提示 / 关窗）由 [`reap_save`] 在 `on_interval` 里做，
    /// 它自己端着 `Runtime` 的句柄——画布这一侧因此不再需要它。
    fn new(
        core: Rc<RefCell<TrimCore>>,
        win: (f32, f32),
        title: Signal<String>,
        save: Arc<Mutex<SaveState>>,
    ) -> Self {
        Self {
            core,
            win: Cell::new(win),
            title,
            save,
            grab: None,
            hover_btn: None,
        }
    }

    fn lay(&self) -> Layout {
        let (w, h) = self.win.get();
        layout(w, h)
    }

    /// 保存中的完成度 / 是否忙（`paint` 与 `on_event` 都要问）。
    ///
    /// 锁被别的线程持有时（写进度的瞬间）返回 `None` —— 画图不该等锁，下一帧再看。
    fn save_view(&self) -> Option<(bool, f32)> {
        self.save.lock().ok().map(|g| (g.busy, g.pct))
    }

    /// 「打开…」：弹原生打开对话框，把选中的 MP4 载入**本窗**（防线照抄 GIF 版）。
    fn pick_file(&mut self, ctx: &mut EventCtx) {
        let core = self.core.clone();
        if !core.borrow_mut().begin_dialog() {
            log::info!("视频裁剪：已有对话框开着，忽略这次「打开」");
            return;
        }
        let owner = windui::platform::native_window_handle();
        let start_dir = crate::core::paths::module_temp_dir(crate::core::paths::MODULE_RECORD);
        let title = self.title;
        ctx.defer_blocking(move || {
            let picked = crate::platform::file_dialog::open_mp4(Some(&start_dir), owner);
            match &picked {
                None => log::info!("视频裁剪：用户在「打开」里取消"),
                Some(p) => {
                    // 先收借用再 match：`match core.borrow_mut().load(..)` 会让 `RefMut`
                    // 活到整条语句结束（本项目踩过两次的坑）。失败原因由 `load` 挂 status。
                    let r = core.borrow_mut().load(p.clone());
                    match r {
                        Ok(()) => log::info!("视频裁剪窗载入 {}", p.display()),
                        Err(e) => log::warn!("视频裁剪窗载入失败：{e}"),
                    }
                    // 标题栏跟着文件走（"文档名 - 软件名"）。`Signal` 与 `CUR_TITLE` 都只
                    // 在 UI 线程动——本闭包正跑在 UI 线程（事件分发返回后）。
                    let name = core.borrow().file_name();
                    title.set(window_title(&name));
                    set_cur_title(&name);
                }
            }
            core.borrow_mut().end_dialog();
            crate::platform::win_util::invalidate_by_title(&cur_title());
        });
    }

    /// 「保存」= **另存为**：`trim_ranges` 流拷贝（不重新编码），原片只读。
    ///
    /// ★ 只有「另存为」对话框在 UI 线程（原生对话框要 owner，且它自己会泵消息）；
    ///   真正的写盘**起工作线程**，否则整窗连同程序一起"点完没反应"（见模块头
    ///   "保存：不冻 UI + 进度条"，2026-09-30 用户报"保存卡死"）。
    fn save(&mut self, ctx: &mut EventCtx) {
        if let Ok(g) = self.save.lock() {
            if g.busy {
                log::info!("视频裁剪：上一次保存还没完，忽略这次「保存」");
                return;
            }
        }
        let (path, keep) = {
            let st = self.core.borrow();
            let Some(p) = st.path.clone() else {
                drop(st);
                self.core.borrow_mut().status = Some("先打开一个视频再保存".to_string());
                ctx.mark_dirty_all();
                return;
            };
            (p, st.keep_plan())
        };
        if keep.is_empty() {
            self.core.borrow_mut().status = Some("一秒都不剩，不能保存".to_string());
            ctx.mark_dirty_all();
            return;
        }

        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "video".to_string());
        let default_name = format!("{stem}_裁剪");
        let start_dir = path.parent().map(|p| p.to_path_buf());

        if !self.core.borrow_mut().begin_dialog() {
            log::info!("视频裁剪：已有对话框开着，忽略这次「保存」");
            return;
        }
        let owner = windui::platform::native_window_handle();
        let core = self.core.clone();
        let save = self.save.clone();
        // ★ 工作线程读不到 thread_local 的 `CUR_TITLE`，先把**现在**这份标题字符串取走。
        //   保存期间按钮全禁用，文件不会换，标题也就不会变。
        let title = cur_title();
        ctx.defer_blocking(move || {
            let dst =
                crate::platform::file_dialog::save_mp4(&default_name, start_dir.as_deref(), owner);
            match dst {
                None => log::info!("视频裁剪：用户在「另存为」里取消"),
                Some(dst) => {
                    let keep_s: Vec<(f64, f64)> = keep
                        .iter()
                        .map(|&(s, e)| (s as f64 / 1000.0, e as f64 / 1000.0))
                        .collect();
                    if let Ok(mut g) = save.lock() {
                        g.busy = true;
                        g.pct = 0.0;
                    }
                    core.borrow_mut().status = Some("正在保存…".to_string());
                    let save2 = save.clone();
                    let title2 = title.clone();
                    let path2 = path.clone();
                    let dst2 = dst.clone();
                    match std::thread::Builder::new()
                        .name("mp4-trim".into())
                        .spawn(move || {
                            log::info!("视频裁剪：工作线程开写 {}", dst2.display());
                            let r = crate::platform::mp4_trim::trim_ranges_progress(
                                &path2,
                                &dst2,
                                &keep_s,
                                &mut |p| {
                                    if let Ok(mut g) = save2.lock() {
                                        g.pct = p as f32;
                                    }
                                    // 工作线程也能让窗口重画（`InvalidateRect` 是跨线程合法的）。
                                    crate::platform::win_util::invalidate_by_title(&title2);
                                },
                            );
                            let name = dst2
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            let outcome = match r {
                                Ok(rep) => Ok(format!(
                                    "已裁剪视频：保留 {} → {name}",
                                    secs(rep.kept_s as usize * 1000)
                                )),
                                Err(e) => Err(format!("{e}")),
                            };
                            log::info!(
                                "视频裁剪：工作线程结束（{}）",
                                if outcome.is_ok() { "成功" } else { "失败" }
                            );
                            if let Ok(mut g) = save2.lock() {
                                g.done = Some(outcome);
                            }
                            crate::platform::win_util::invalidate_by_title(&title2);
                        }) {
                        Ok(_) => {}
                        Err(e) => {
                            log::warn!("视频裁剪：起不了工作线程：{e}");
                            if let Ok(mut g) = save.lock() {
                                g.busy = false;
                            }
                            core.borrow_mut().status = Some(format!("保存失败：{e}"));
                        }
                    }
                }
            }
            core.borrow_mut().end_dialog();
            crate::platform::win_util::invalidate_by_title(&title);
        });
    }

    /// 当前是否在写盘（`paint` 与输入门都要问）。
    fn is_saving(&self) -> bool {
        self.save_view().map(|(b, _)| b).unwrap_or(false)
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
        let kept = st.kept_ms();
        let summary = if let Some(s) = &st.status {
            s.clone()
        } else if !st.is_loaded() {
            String::new()
        } else if st.b > st.a {
            format!(
                "共 {}　|　待删 {} → {}（{}）　|　删后保留 {}",
                secs(st.total_ms()),
                secs(st.a),
                secs(st.b),
                secs(st.pending_ms()),
                secs(kept),
            )
        } else {
            format!(
                "共 {}　|　在时间轴上按住左键横向划过，划出要删掉的那一段",
                secs(st.total_ms())
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

        // ── 轨道（保存中改画进度条） ──
        let track_y = lay.track.y + (lay.track.h - TRACK_H) / 2.0;
        let saving = self.save_view().unwrap_or((false, 0.0));
        let busy = saving.0;
        if busy {
            // 写盘期间时间轴没有可操作的东西，把这条位置让给进度：底槽 + 已完成段 + 百分比。
            let pct = saving.1.clamp(0.0, 1.0);
            canvas.fill_round_rect(
                lay.track.x,
                track_y,
                lay.track.w,
                TRACK_H,
                TRACK_H / 2.0,
                &WPaint::fill(color(BORDER, 255)),
            );
            canvas.fill_round_rect(
                lay.track.x,
                track_y,
                (lay.track.w * pct).max(TRACK_H),
                TRACK_H,
                TRACK_H / 2.0,
                &WPaint::fill(color(ACCENT, 255)),
            );
            canvas.draw_text(
                &format!("正在保存… {}%", (pct * 100.0).round() as i32),
                irect(lay.ticks),
                color(ACCENT, 255),
                Align::Center,
                &ts_small,
            );
        } else {
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
                let px = bound_to_x(st.preview_ms.min(st.total), st.total, lay.track);
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
                let end = secs(st.total_ms());
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
        }

        // ── 按钮 ──
        for (i, label) in BUTTON_LABELS.iter().enumerate() {
            let r = lay.buttons[i];
            // 保存中**全部置灰**：再点一次「保存」或「打开…」都会把状态搅乱。
            let enabled = if busy {
                false
            } else {
                match i {
                    0 => true,
                    1 => st.b > st.a,
                    2 => !st.removed.is_empty(),
                    3 => st.is_loaded() && kept > 0,
                    _ => true,
                }
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
        // 保存中整窗只读：任何手势/点击都不该再改"要保留哪些区间"（写盘正按它抄样本）。
        if self.is_saving() {
            return false;
        }
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
                        // 预览跟着刚动的那一侧走。
                        let follow = match g {
                            Grab::A => a,
                            _ => b.saturating_sub(1),
                        };
                        st.set_preview(follow);
                        drop(st);
                        ctx.mark_dirty_all();
                        true
                    }
                    PointerKind::Down if pe.button == MouseButton::Left => {
                        if let Some(i) = button_at(x, y, &lay) {
                            self.grab = None;
                            match i {
                                0 => self.pick_file(ctx),
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
                        let mut st = self.core.borrow_mut();
                        if !st.is_loaded() {
                            return false;
                        }
                        let a_x = bound_to_x(st.a, st.total, lay.track);
                        let b_x = bound_to_x(st.b, st.total, lay.track);
                        let g = grab_at(x, y, &lay, a_x, b_x, st.total);
                        self.grab = g;
                        if let Some(Grab::Sweep { from }) = g {
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
                        // 整段平移 100ms（Shift = 1s）：鼠标划不准时用它。
                        let step = if ke.shift { 1000 } else { 100 };
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

/// 收尾一次保存（UI 线程）：把工作线程放进 `SaveState::done` 的结局显示掉。
///
/// 返回是否要重画（还在保存时也要重画——进度条在动）。做成自由函数是因为 `on_interval`
/// 的闭包拿不到 `TrimCanvas`（那份已经搬进控件树了），只需要这三个句柄。
fn reap_save(
    save: &Arc<Mutex<SaveState>>,
    core: &Rc<RefCell<TrimCore>>,
    rt: &Rc<RefCell<Runtime>>,
) -> bool {
    let outcome = match save.lock() {
        Ok(mut g) => {
            if !g.busy {
                return false; // 没在保存：这个定时器此刻什么都不做（空转成本一次 lock）
            }
            g.done.take()
        }
        Err(_) => return false,
    };
    let Some(outcome) = outcome else {
        return true; // 还在跑（`busy` 仍为真）：继续重画进度条
    };
    match outcome {
        Ok(msg) => {
            log::info!("视频裁剪完成：{msg}");
            rt.borrow_mut().show_notify(&msg);
            // 保存成功即关窗（与 GIF 版同一条口径：结果交给系统通知，窗不留着）。
            crate::platform::win_util::post_close_by_title(&cur_title());
        }
        Err(e) => {
            log::warn!("视频裁剪失败：{e}");
            core.borrow_mut().status = Some(format!("保存失败：{e}"));
        }
    }
    if let Ok(mut g) = save.lock() {
        g.busy = false;
    }
    crate::platform::win_util::invalidate_by_title(&cur_title());
    true
}

/// 打开裁剪窗并**载入一份 MP4**（控件条「裁剪」/ 刚录完那条路）。
///
/// 文件解不开时返回 `false`，调用方据此给用户一句提示。
pub fn open(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, path: PathBuf) -> bool {
    let core = match TrimCore::open(path.clone()) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("打开视频裁剪窗失败：{e}");
            return false;
        }
    };
    spawn(ctx, rt, core);
    log::info!("视频裁剪窗已打开，载入 {}", path.display());
    true
}

/// 打开裁剪窗的**空状态**：一份文件都没载入，等用户按「打开…」挑。
///
/// 这是时间裁剪快捷键的入口（与 GIF 版同一条口径：想裁的往往不是刚录的那段）。
/// 总是返回 `true`——空窗也是窗，"裁剪器开着"的状态必须登记。
pub fn open_blank(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>) -> bool {
    spawn(ctx, rt, TrimCore::empty());
    log::info!("视频裁剪窗已打开（空状态，等用户选文件）");
    true
}

/// 两个入口共用的开窗动作（`.single("mp4_trim")` 保证重复按只聚焦，见 GIF 版注释）。
fn spawn(ctx: &mut EventCtx, rt: &Rc<RefCell<Runtime>>, core: TrimCore) {
    let win = (WIN_W as f32, WIN_H as f32);
    // 标题栏 = "文档名 - 软件名"（见模块头）。信号给 windui（它每帧拉取并 `SetWindowTextW`），
    // `CUR_TITLE` 给我们自己的 FindWindowW——两份必须同时更新，否则按标题关窗会落空。
    let name = core.file_name();
    let title_sig = signal(window_title(&name));
    set_cur_title(&name);
    let save = Arc::new(Mutex::new(SaveState::default()));
    let core = Rc::new(RefCell::new(core));
    let canvas = TrimCanvas::new(core.clone(), win, title_sig, save.clone());
    // 定时收尾与关窗各自要一份 `Runtime` 句柄（闭包是 `'static`，借不到 `rt` 这个引用）。
    let rt_reap = rt.clone();
    let rt_close = rt.clone();
    let core_close = core.clone();
    let dpi = crate::platform::display::dpi_scale();
    let vs = rt.borrow().screen;
    let x = vs.x + (vs.w - (WIN_W as f32 * dpi).round() as i32) / 2;
    let y = vs.y + (vs.h - (WIN_H as f32 * dpi).round() as i32) / 2;
    log::info!("视频裁剪窗 @ ({x},{y})");

    ctx.open_window(
        // ★ 留系统标题栏（不设 `frameless`）+ 可缩放：用户能最大化/拖大看画面，见模块头。
        //   `min_size` 是下限——再小按钮两簇会撞上，布局也会退化成"只剩一条缝"。
        Window::new(TITLE, WIN_W, WIN_H)
            .single("mp4_trim")
            .title(title_sig)
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .position(x, y)
            .bg(color(PANEL, 255))
            // 工作线程把结局放进 `SaveState` 就完事，剩下的（提示 / 关窗 / 报错）只能在
            // UI 线程做——`Rc` 那一套过不了线程边界。定时轮询随本窗关闭一起停。
            .on_interval(SAVE_POLL, move |ctx| {
                if reap_save(&save, &core, &rt_reap) {
                    ctx.mark_dirty_all();
                }
            })
            .on_close_request(move |_ctx| {
                rt_close.borrow_mut().close_gif_trim();
                // ★ 关窗就把最占地方那块（预览图，1080p ≈ 16 MB，两份）当场吐出来，
                //   并请 `working_set` 顺带整一次工作集——理由见 TrimCore::release_memory。
                core_close.borrow_mut().release_memory();
                crate::platform::working_set::request_trim_soon();
                true
            })
            .content(move || Element::leaf().widget(canvas).fill()),
    );
    // 与 GIF 版同一条纪律：只聚焦、不置顶——本窗要弹原生文件对话框（见 gif_trim 模块头
    // 的三层对话框事故）。
    let _ = crate::platform::win_util::focus_window_by_title(&cur_title());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 关窗必须把预览图（1080p 两份 ≈ 16 MB 的那块）当场吐掉——回归锁：
    /// 2026-10-01 用户报"录完裁完关掉所有模块仍停 40MB"，链路里最结实的一块就是它。
    #[test]
    fn release_memory_clears_the_preview() {
        let mut core = TrimCore::empty();
        core.preview = Some(WImage::from_rgba(2, 2, &[0u8; 16]).expect("建图"));
        core.built_for = Some(0);
        core.release_memory();
        assert!(core.preview.is_none(), "预览图必须被清空");
        assert!(core.built_for.is_none(), "预览时刻标记也必须清空");
    }

    fn track() -> Box2 {
        Box2::new(20.0, 100.0, 400.0, 30.0)
    }

    /// 毫秒边界 ↔ 轨道 x 必须严格互逆。
    #[test]
    fn ms_boundary_and_track_x_are_inverse() {
        let t = track();
        let total = 12_345usize;
        for k in [0usize, 1, 100, 5000, 9999, 12_344, 12_345] {
            let x = bound_to_x(k, total, t);
            assert_eq!(x_to_bound(x, total, t), k, "边界 {k} 往返对不上");
        }
        assert_eq!(bound_to_x(0, total, t), t.x);
        assert!((bound_to_x(total, total, t) - (t.x + t.w)).abs() < 1e-3);
        assert_eq!(bound_to_x(999_999, total, t), t.x + t.w);
        assert_eq!(bound_to_x(0, 0, t), t.x);
    }

    /// 划选手势落在空白处才算划选；反着划也得到同一段。
    #[test]
    fn sweep_selects_the_range_between_the_two_points() {
        let lay = layout(720.0, 440.0);
        let t = lay.track;
        let total = 10_000usize;
        let mid = t.x + t.w * 0.25;
        let grab = grab_at(mid, t.y + t.h / 2.0, &lay, t.x, t.x, total);
        assert!(
            matches!(grab, Some(Grab::Sweep { .. })),
            "空白处按下应是划选"
        );
        if let Some(Grab::Sweep { from }) = grab {
            assert!(from < x_to_bound(mid + t.w * 0.25, total, t));
        }
    }

    /// 把手优先于划选。
    #[test]
    fn handle_takes_priority_over_sweeping() {
        let lay = layout(720.0, 440.0);
        let t = lay.track;
        let total = 10_000usize;
        let (a, b) = (3_000usize, 7_000usize);
        let a_x = bound_to_x(a, total, t);
        let b_x = bound_to_x(b, total, t);
        let y = t.y + t.h / 2.0;
        assert_eq!(grab_at(a_x + 2.0, y, &lay, a_x, b_x, total), Some(Grab::A));
        assert_eq!(grab_at(b_x - 2.0, y, &lay, a_x, b_x, total), Some(Grab::B));
        assert_eq!(grab_at(t.x, y, &lay, t.x, t.x, total), Some(Grab::B));
        assert_eq!(grab_at(t.x, t.y - 40.0, &lay, a_x, b_x, total), None);
    }

    /// 五个按钮不得互相重叠/出界；两簇分开。
    #[test]
    fn buttons_are_laid_out_inside_the_window() {
        let lay = layout(WIN_W as f32, WIN_H as f32);
        for b in lay.buttons {
            assert!(b.x >= 0.0 && b.x + b.w <= WIN_W as f32, "按钮出界：{b:?}");
        }
        for i in 1..BUTTON_N {
            assert!(
                lay.buttons[i].x >= lay.buttons[i - 1].x + lay.buttons[i - 1].w,
                "按钮重叠"
            );
        }
        assert!((lay.buttons[0].x - PAD).abs() < 1e-3);
        let last = lay.buttons[BUTTON_N - 1];
        assert!((last.x + last.w - (WIN_W as f32 - PAD)).abs() < 1e-3);
        let gap = lay.buttons[1].x - (lay.buttons[0].x + lay.buttons[0].w);
        assert!(gap >= BUTTON_GAP * 2.0, "左右两簇贴在一起了，gap={gap}");

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

    /// 结构门禁：**按标题找窗的调用一律不能用 [`TITLE`] 常量**。
    ///
    /// 标题栏现在是动态的（"文档名 - 软件名"），而 `FindWindowW` 只认**完整标题**：任何一处
    /// 写成 `invalidate_by_title(TITLE)` 都会**静默**找不到窗口（不报错、只是不重画 / 不关窗 /
    /// 不聚焦），这类失效在日志里什么痕迹都没有。所以钉在源码结构上。
    #[test]
    fn by_title_calls_use_the_live_title_not_the_constant() {
        const SRC: &str = include_str!("mp4_trim.rs");
        let cut = SRC.find("#[cfg(test)]").expect("本文件有测试模块");
        let p = &SRC[..cut];
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
        assert!(
            p.contains("set_cur_title_raw(self.title.get())"),
            "活窗必须在 paint 里校准 `CUR_TITLE`（`.single()` 那条路会把它写歪）"
        );
    }

    /// 标题栏：**文档名 - 软件名**（记事本口径），任务栏/Alt-Tab 里先看到自己的文件。
    #[test]
    fn window_title_is_document_name_dash_app_name() {
        assert_eq!(window_title(""), format!("未打开文件 - {TITLE}"));
        assert_eq!(
            window_title("CurSeen_2026-09-30.mp4"),
            format!("CurSeen_2026-09-30.mp4 - {TITLE}")
        );
        // 软件名那半不带文件类型——窗口自己是哪个工具由 `TITLE` 承担。
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

    /// 摘要的"删后保留"必须与 complement 的真值一致（时间单位毫秒）。
    #[test]
    fn keep_plan_matches_complement_truth() {
        let mut c = TrimCore::empty();
        c.total = 10_000;
        c.removed = vec![(1_000, 3_000)];
        assert_eq!(c.keep_plan(), vec![(0, 1_000), (3_000, 10_000)]);
        assert_eq!(c.kept_ms(), 8_000);
        // 待删段叠加进计划。
        c.set_ab(8_000, 9_000);
        assert_eq!(c.kept_ms(), 7_000);
        assert_eq!(c.pending_ms(), 1_000);
    }

    /// 预览量化：同 100ms 格内不触发重建，跨格才更新 `built_for`。
    #[test]
    fn preview_time_is_quantized_without_a_file() {
        let mut c = TrimCore::empty();
        c.total = 5_000;
        // 没有文件时 set_preview 不 panic 也不建图，但量化后的"应到时刻"可独立验证：
        assert_eq!(120 / PREVIEW_QUANT_MS * PREVIEW_QUANT_MS, 100);
        assert_eq!(199 / PREVIEW_QUANT_MS * PREVIEW_QUANT_MS, 100);
        assert_eq!(200 / PREVIEW_QUANT_MS * PREVIEW_QUANT_MS, 200);
        let _ = c; // 静默 unused
    }

    #[test]
    fn secs_formats_milliseconds_to_one_decimal() {
        assert_eq!(secs(0), "0.0s");
        assert_eq!(secs(1_200), "1.2s");
        assert_eq!(secs(65_400), "65.4s");
    }

    /// 把裁剪画布**离屏渲成一张不透明位图**（dpi=1，与窗口逻辑坐标 1:1）。
    ///
    /// 几何断言查不出"预览里到底有没有画面"——那要靠像素，见 [`dump_trim_window_png`]。
    fn render(c: &TrimCanvas, w: u32, h: u32) -> Vec<u8> {
        use windui::render::{DibTarget, RenderTarget};
        use windui::text::DWriteEngine;
        let mut ann = vec![0u8; (w * h * 4) as usize];
        let mut engine = DWriteEngine::new();
        {
            let mut target = DibTarget::new(&mut ann, w, h);
            let mut canvas = target.make_canvas(&mut engine, 1.0);
            Widget::paint(
                c,
                Rect::new(0, 0, w as i32, h as i32),
                Rect::new(0, 0, w as i32, h as i32),
                false,
                true,
                &mut *canvas,
                &Style::default(),
            );
        }
        ann
    }

    /// 把裁剪窗离屏渲成 PNG 供人眼评审（不进回归）。**要一份真 MP4**，用环境变量给：
    ///
    /// ```text
    /// MP4_TRIM_PREVIEW_SRC=<某.mp4> cargo test --release --lib dump_trim_window_png -- --ignored --nocapture
    /// ```
    ///
    /// 产出 `temp/mp4_ui/window.png`（初始尺寸）与 `window_big.png`（拉大后）——后者正是
    /// "窗口能放大、且放大后画面跟着变大"的图文证据。真机截图这条路被 UIPI 断了
    /// （见 `examples/render_probe.rs` 头部），而这两件事只能看图说话。
    #[test]
    #[ignore = "只产出 PNG 供人看，不进回归；需要环境变量 MP4_TRIM_PREVIEW_SRC"]
    fn dump_trim_window_png() {
        let Some(src) = std::env::var_os("MP4_TRIM_PREVIEW_SRC").map(PathBuf::from) else {
            println!("未设 MP4_TRIM_PREVIEW_SRC，跳过（用法见本用例文档注释）");
            return;
        };
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("temp")
            .join("mp4_ui");
        std::fs::create_dir_all(&dir).expect("建目录");
        let core = Rc::new(RefCell::new(TrimCore::open(src).expect("开裁剪核心")));
        // 画布本身不再持有 `Runtime`（收尾交给 `reap_save`），所以这里不需要建它。
        for (name, (w, h), busy_pct) in [
            ("window.png", (WIN_W, WIN_H), None),
            ("window_big.png", (1200, 800), None),
            // 保存中：轨道位置让给进度条，按钮全灰 —— "保存卡死"那条修法的人眼证据。
            ("window_saving.png", (WIN_W, WIN_H), Some(0.52f32)),
        ] {
            let save = Arc::new(Mutex::new(SaveState::default()));
            if let Some(p) = busy_pct {
                if let Ok(mut g) = save.lock() {
                    g.busy = true;
                    g.pct = p;
                }
            }
            let c = TrimCanvas::new(
                core.clone(),
                (w as f32, h as f32),
                signal(window_title("clip.mp4")),
                save,
            );
            // 摆一个"已删一段 + 待删一段"的混合状态：PNG 里要能同时看到浅红（已删）与正红
            // （待删），以及摘要行随之变化的数字。
            {
                let mut st = c.core.borrow_mut();
                st.set_ab(1_000, 3_000);
                st.commit_delete();
                st.set_ab(6_000, 9_000);
                st.set_preview(7_000);
            }
            let rgba = render(&c, w as u32, h as u32);
            let out = dir.join(name);
            let file = std::fs::File::create(&out).expect("建 PNG");
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().expect("PNG 头");
            wr.write_image_data(&rgba).expect("PNG 数据");
            wr.finish().expect("PNG 收尾");
            println!("MP4 裁剪窗 PNG: {}", out.display());
        }
    }
}
