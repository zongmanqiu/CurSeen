//! 截图与贴图状态机（区域选择 + 贴图环 + 贴图窗记账）。
//!
//! # 流程（三个快捷键共用一条底层链）
//!
//! ```text
//! 快捷键（截图 / 截图并贴图）
//!   → Runtime::begin_snap(mode)     整张虚拟桌面 GDI 冻结帧一次抓取
//!   → overlay 通道回调开"区域选择窗"（layer::region：冻结帧 + 暗化遮罩 + 框选）
//!   → 确认（拖拽松手 / 回车）→ Runtime::confirm_snap(rect)
//!       → 裁剪 → 剪贴板（+ `temp/screenshot` 临时留档）→ 白闪反馈
//!       → ShotPin 模式追加：入贴图环 + 请求开贴图窗（**带原位**，贴在截图那一块上）
//!   → 取消（Esc / 单击 / 关窗）→ Runtime::cancel_snap()
//!
//! 快捷键（贴图）
//!   → Runtime::pin_next()           剪贴板有新内容 = 倒数第一；否则沿环回溯
//!   → overlay 通道回调开"贴图窗"（layer::pin：图片 1:1，带 2px 分层描边）
//!
//! 落位由**环条目自带的原位**决定（见 [`RingItem`]）：凡是通过本功能截下来的图（纯截图
//! `Shot` 与截图并贴图 `ShotPin` 都算）都记着"原来在屏幕哪一块"，什么时候贴都落回原处；
//! 别人复制到剪贴板进来的内容没有原位 → 屏幕居中。
//!
//! 贴图窗的关闭 / 另存为都**只针对选中的那一张**：左键点中即选中（并置顶），
//! `Esc` 关掉它、`Ctrl+S` 把它另存为 PNG，选中态见 [`SnapMgr::selected_pin`]。
//! ```
//!
//! # 贴图环（"最多支持 pin 最近的 10 个内容"）
//!
//! Windows 没有可编程的剪贴板历史读取 API（Win+V 是 shell 功能），因此用**本地环**
//! 逼近"倒数第一、第二、第三"的语义：
//!
//! - 每次贴出的内容入环（`Rc` 共享位图，不复制像素）；环容量 [`PIN_RING_CAP`]，超出淘汰最旧。
//! - 剪贴板内容由 `clipboard::sequence_number()` 轮询补收（见 `Runtime::poll_clipboard`），
//!   所以**两次按键之间复制的内容也会进环**——否则"最近 10 个"名不副实。
//! - 游标是**距环尾的偏移**，不是绝对下标：淘汰头部不影响语义。
//!
//! 图像条目判同只比对尺寸 + 像素（`Rc::ptr_eq` 先短路，同一份位图不会走逐字节比较）。

use std::collections::VecDeque;
use std::rc::Rc;

use crate::core::geometry::{Rect, ScreenRect};
use crate::core::image::RgbaBitmap;

/// 贴图环容量（用户要求：最多 pin 最近的 10 个内容）。
pub const PIN_RING_CAP: usize = 10;

/// 同时存活的贴图窗上限。一张 4K 贴图是 33 MB 常驻，不限量迟早吃满内存；
/// 超出时关掉最早的那一个（贴图是"看过就走"的东西，不是文档）。
pub const PIN_WINDOW_CAP: usize = 10;

/// 截图模式：纯截图（进剪贴板）或截图并贴图（进剪贴板 + 钉到桌面）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapMode {
    /// 截图：确认后进剪贴板。
    Shot,
    /// 截图并贴图：确认后进剪贴板 + 钉到桌面。
    ShotPin,
    /// ★ GIF 选框：确认后**只取那块矩形**，不进剪贴板、不留档、不白闪 —— 它不是截图，
    ///   是要"录哪一块"。矩形交给 `Runtime::gif_sel`，等用户在边框外的按钮上点「开始」。
    ///
    /// 复用截图那条选区链的代价是一次整屏抓帧（0.1–0.3s）与一张冻结帧位图，换来的是
    /// 完全一致的框选手感（压暗 + 尺寸标识），且用户看得见自己要录的是什么。
    Gif,
    /// ★ 选区录屏：复用同一套截图选框框一块区域，确认后**直接录制那一块**（MP4）——
    /// 不是截屏、不是贴图。与 [`SnapMode::Gif`] 同一条选区链，差别只在"确认后干什么"：
    /// GIF 留下控件条等「开始」，这里是立刻进倒计时开始录。矩形交录制器裁剪。
    Mp4Region,
    /// ★★ 长截图（滚动截图）：复用同一套选区链框一块区域，确认后**起采集线程**——
    /// 用户滚动目标页面，工作线程逐帧抓屏喂拼接器（`core::scroll_stitch`），到"到底 /
    /// 再按快捷键 / 画布满"时把拼接画布整体交付（剪贴板 + 留档）。
    ///
    /// 与 GIF / `Mp4Region` 的差别同样只在"确认后干什么"：这里既不弹控件条也不进倒计时，
    /// 而是直接开始滚动采集。确认那一刻**不走截图出口**（不裁帧、不进剪贴板）——
    /// 那些是"截图"的语义，这一刻要的只是"要滚哪一块"。
    Scroll,
    /// ★★ **宽截图**（2026-10-01 用户需求）：与 [`SnapMode::Scroll`] 完全同一条链
    /// （同一把两段式交互、同一个拼接器、同一套就绪态拖拽/控件条），唯一差别是
    /// **滚动方向**：用户**向右**滚，拼接器把每帧转置后当"向下滚"喂（见
    /// `core::scroll_stitch::transpose_bgra`）。方向在框选那一刻由按下的键决定，
    /// 绑进就绪态走完全程。
    ScrollWide,
    /// ★★ 截图**编辑态**（`Ctrl` + 截图快捷键）：框好之后**不停顿确认**，而是像 GIF 那样
    ///   把选区留下继续调（缩放 / 整体移动），并且可以在截图上标注，最后才落剪贴板。
    ///
    /// 用户口径（2026-09-23）：*"截图时，如果按住Ctrl，选区选择后应该停顿，就像gif选区
    /// 一样，允许选区缩放，也允许选区整体移动…并且，按住ctrl键停顿时，我希望可以对截图做
    /// 简单处理，就像屏幕标注一样。换言之，截图和屏幕标注在本质上是相同的函数。"*
    ///
    /// 这条模式与 [`SnapMode::Gif`] 的差别只在"暂停之后干什么"：GIF 暂停是为了等
    /// 「开始录制」，这里暂停是为了调框 + 标注，然后**继续走截图那条出口**（剪贴板 +
    /// 留档 + 白闪）。所以 `pin` 要把"最终是纯截图还是截图并贴图"带过来 —— 否则两把键
    /// 的 Ctrl 变体（旧出厂 `Ctrl+Alt+S` 与 `Ctrl+Alt+Q`，2026-09-26 起出厂留空、由用户
    /// 自配）到了确认那一步就没法分岔了。
    ShotEdit {
        /// 确认后是否还要钉到桌面（`hotkey_pin` 那一路带 `true`）。
        pin: bool,
    },
}

impl SnapMode {
    /// 这条模式是不是"暂停 + 继续编辑"（而不是框完就走）。
    pub fn is_edit(self) -> bool {
        matches!(self, SnapMode::ShotEdit { .. })
    }
}

/// 待框选的冻结帧。整张**虚拟桌面**（不是单显示器）——跨屏框选要它。
pub struct SnapFrame {
    /// 冻结帧（非预乘 RGBA，α 恒 255）。`Rc` 共享给区域窗 / 贴图环，不复制像素。
    pub frame: Rc<RgbaBitmap>,
    pub mode: SnapMode,
}

/// 贴图内容：图片，或"文本转成图片"的结果。
///
/// 文本在入环前就已经被栅格化成图片（`platform::text_raster`），所以这里只有一种形态
/// ——与用户要求"剪贴板文字转 jpg/png 图片"一致：贴到桌面上的就是图。
#[derive(Clone)]
pub enum PinContent {
    Image(Rc<RgbaBitmap>),
}

impl PinContent {
    /// 弱判同：同一份位图（`Rc::ptr_eq`）直接命中；否则比尺寸 + 逐字节。
    /// 主要用途是挡住"同一张剪贴板图重复入环"。
    fn same_content(&self, other: &PinContent) -> bool {
        match (self, other) {
            (PinContent::Image(a), PinContent::Image(b)) => {
                Rc::ptr_eq(a, b) || (a.w == b.w && a.h == b.h && a.rgba == b.rgba)
            }
        }
    }

    /// 贴图窗标题后缀（日志 / 按标题找窗用）。
    pub fn describe(&self) -> String {
        match self {
            PinContent::Image(b) => format!("图片 {}×{}", b.w, b.h),
        }
    }

    pub fn bitmap(&self) -> &Rc<RgbaBitmap> {
        match self {
            PinContent::Image(b) => b,
        }
    }
}

/// 一次开贴图窗的请求（边沿触发，UI 线程取走后开窗）。
#[derive(Clone)]
pub struct PinRequest {
    pub content: PinContent,
    pub seq: u32,
    /// ★ **原位**：产生这张图的屏幕矩形（冻结帧坐标系，物理像素）。
    ///
    /// `Some` = 这是我们自己截下来的（"截图并贴图"）——贴图必须**落在截图时那一块上**，
    /// 位置与尺寸都一模一样（用户口径："位置应该是截图的位置，位置要一样的"）。
    /// `None` = 内容来自剪贴板（别的程序复制的图、或剪贴板文字栅格化的图）——没有"原位"
    /// 可言，居中显示。
    pub origin: Option<Rect>,
}

/// 一个存活中的贴图窗（序号 + 它所显示的内容）。
///
/// ★ 为什么要带内容：贴图窗是独立窗口，`Runtime` 只按序号记账；而"选中某张 → Ctrl+S
/// 另存为"需要拿到**那一张**的位图。贴图环会按 `pin_ring_cap` 淘汰旧条目，窗口却还在，
/// 所以不能从环里反查——内容必须与窗口记账放在一起（`Rc` 共享像素，不复制）。
struct OpenPin {
    seq: u32,
    content: PinContent,
    /// 「锁定图片」：锁定后 Esc / 双击关不掉，只有右键解除或「关闭」才消失；
    /// 超上限淘汰也会跳过锁定项（保持用户口径"无法让图片消失"）。
    locked: bool,
}

/// 贴图环里的一个条目：内容 + **原位**。
///
/// ★ 原位必须跟着条目走，不能只挂在 `PinRequest` 上。用户口径（2026-09-17）：
/// *"只要是通过我们的功能截图的，就要记录信息。这样贴图的时候才方便。"*
/// ——纯截图（`SnapMode::Shot`）**不立刻贴图**，它先进环；用户可能几秒、几十秒之后才按
/// 贴图键，中间还会复制别的东西。原位若只活在"当次请求"里，那一刻早就没了，于是贴图
/// 落在屏幕中间（用户报的正是这个）。存进环条目 → 什么时候贴都带着原位。
struct RingItem {
    content: PinContent,
    /// 产生这张图的屏幕矩形（冻结帧坐标系，物理像素）。`None` = 来自外部剪贴板。
    origin: Option<Rect>,
}

/// 截图与贴图的全部瞬态状态。挂在 `Runtime` 上，由快捷键分派与覆盖层回调驱动。
pub struct SnapMgr {
    /// 待框选的冻结帧。`Some` = 区域选择窗开着。
    pub pending: Option<SnapFrame>,
    /// 边沿请求：开区域选择窗（overlay 通道回调取走）。
    open_requested: bool,
    /// 边沿请求：开贴图窗。
    pin_req: Option<PinRequest>,
    /// 贴图环（新条目在**尾部**）。每个条目自带原位，见 [`RingItem`]。
    ring: VecDeque<RingItem>,
    /// 环容量。
    cap: usize,
    /// 游标：**距环尾的偏移**（0 = 最新）。`None` = 下次按贴图键从最新的开始。
    cursor: Option<usize>,
    /// 贴图窗序号（标题去重）。
    seq: u32,
    /// 存活中的贴图窗（新在尾）。用户手关的窗由 `note_pin_closed` 摘除——`post_close_by_title`
    /// 找不到就静默失败，所以万一有陈旧项，也只会让"关最早那个"变成空操作，不会误关别的窗。
    open_pins: VecDeque<OpenPin>,
    /// ★ 当前**选中**的贴图窗序号（最后一次左键点中的那张）。
    ///
    /// 用户口径（2026-09-17）：`Esc` 只关**我选中的那一张**，不再全关；`Ctrl+S` 也是针对
    /// 选中的那一张另存为。选中态在下列时点变化：
    /// - 左键点中某张贴图 → 选中它（贴图控件里调 [`Self::select_pin`]）；
    /// - 点中贴图以外的任何地方 → 取消选中（`Runtime::note_pointer_down` 判定）；
    /// - 该窗被关闭（Esc / 双击 / 右键 / 超上限淘汰）→ 自动清空。
    selected: Option<u32>,
}

impl Default for SnapMgr {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapMgr {
    pub fn new() -> Self {
        Self::with_cap(PIN_RING_CAP)
    }

    pub fn with_cap(cap: usize) -> Self {
        Self {
            pending: None,
            open_requested: false,
            pin_req: None,
            ring: VecDeque::new(),
            cap: cap.max(1),
            cursor: None,
            seq: 0,
            open_pins: VecDeque::new(),
            selected: None,
        }
    }

    /// 改环容量（面板可调；`0` 会被钳到 `1`）。缩容时立即淘汰最旧的。
    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
        self.trim();
    }

    pub fn cap(&self) -> usize {
        self.cap
    }

    /// 开始一次区域选择。已在进行中则忽略（返回 false）。
    pub fn begin(&mut self, mode: SnapMode, frame: Rc<RgbaBitmap>) -> bool {
        if self.pending.is_some() {
            return false;
        }
        self.pending = Some(SnapFrame { frame, mode });
        self.open_requested = true;
        true
    }

    /// 取"该开区域选择窗了"的请求（边沿触发；冻结帧留在 pending 里供窗体读取）。
    pub fn take_open_request(&mut self) -> bool {
        std::mem::take(&mut self.open_requested)
    }

    /// 冻结帧位图（`Rc` 克隆，不复制像素）。
    pub fn frame_bitmap(&self) -> Option<Rc<RgbaBitmap>> {
        self.pending.as_ref().map(|f| f.frame.clone())
    }

    /// 当前待框选会话的**模式**（`None` = 没有会话）。
    ///
    /// 给 `Runtime::confirm_snap` 用：`ShotEdit` 那条路**不能**走 [`Self::confirm`]
    /// —— 那会把冻结帧裁掉、`pending` 也一起消费掉，而编辑阶段恰恰要整张帧当画布。
    pub fn pending_mode(&self) -> Option<SnapMode> {
        self.pending.as_ref().map(|f| f.mode)
    }

    /// 取出待框选的**整张**冻结帧（不裁剪）并结束这次框选会话。
    ///
    /// 与 [`Self::confirm`] 的差别只有"裁不裁"：编辑态要的是整张虚拟桌面当画布，
    /// 最终裁剪推迟到用户按确认那一刻（那样他调框时整张画面都在，放大镜也才有内容可取）。
    pub fn take_pending_frame(&mut self) -> Option<(Rc<RgbaBitmap>, SnapMode)> {
        self.open_requested = false;
        let f = self.pending.take()?;
        Some((f.frame, f.mode))
    }

    /// **一次区域选择会话是否正在进行中**（截图 / GIF 的区域选择窗开着的那段时间）。
    ///
    /// 判据就是"冻结帧还挂着没有"：它在按下快捷键时置上、在 [`confirm`](Self::confirm) /
    /// [`cancel`](Self::cancel) 被消费 —— 也就是说它的生命周期**恰好**覆盖"用户正在框屏"
    /// 这件事，不需要另造一个标志位（多一个可失同步的状态）。
    ///
    /// ★ 谁是它的消费者：光标放大镜的**所有权**。区域选择窗用它自己那份冻结帧驱动放大镜
    ///   （`layer::region::SnapRegionCanvas::track`），而主覆盖层在 GIF 选区**调整**阶段用
    ///   实时抓屏驱动**同一个** `RegionMagnifier`。两边都能出帧时（用户开着拖尾/标注，
    ///   主覆盖层就不会 park），必须有个人说了算，否则主覆盖层会把区域窗刚出的面板打掉。
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// 取消区域选择（Esc / 单击 / 关窗）。
    pub fn cancel(&mut self) {
        self.pending = None;
        self.open_requested = false;
    }

    /// 确认区域选择：裁出子帧。`rect` 为**冻结帧坐标系**（物理像素），`screen` 是虚拟桌面
    /// 矩形（用来把 `rect` 换回世界坐标去比对窗口外框）。
    /// 返回裁剪结果交由 Runtime 做剪贴板 / 留档 / 贴图。
    pub fn confirm(
        &mut self,
        rect: Rect,
        screen: ScreenRect,
    ) -> Option<(Rc<RgbaBitmap>, SnapMode)> {
        let frame = self.pending.take()?;
        let (x, y, w, h) = (rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32);
        // ★ 与 `shot_edit::compose` 同一件事：这一刀正好贴在某个窗口的外框上时，把最外圈
        //   1px 的半透明边框反解成不透明本色（否则边缘混 60% 桌面色）。
        //   只对**截图**做 —— GIF 那条路取的是"要录哪一块"的矩形，不是像素产物。
        let on_frame = matches!(frame.mode, SnapMode::Shot | SnapMode::ShotPin)
            && crate::platform::window_probe::rect_is_window_frame(ScreenRect::new(
                x + screen.x,
                y + screen.y,
                w,
                h,
            ));
        let cropped = if on_frame {
            frame.frame.crop_frame_opaque(
                x,
                y,
                w,
                h,
                crate::platform::window_probe::FRAME_BORDER_ALPHA,
            )
        } else {
            frame.frame.crop(x, y, w, h)
        }?;
        Some((Rc::new(cropped), frame.mode))
    }

    /// 把内容压入贴图环（新条目 = 倒数第一，游标归位）。
    ///
    /// ★ 与旧实现不同：这里**同时**写入 `pin_req`。"截图并贴图"的确认发生在区域窗里，
    /// 而开窗要等一条 UI 消息（通道回调）或 250ms 对账（定时器回调）——两条路都取
    /// `take_pin_request`，所以不会漏。
    ///
    /// `origin` 见 [`PinRequest::origin`]：截图来的给原位（贴图落在原处），剪贴板来的给 `None`。
    pub fn pin_push(&mut self, content: PinContent, origin: Option<Rect>) -> PinRequest {
        // 先组请求再入环：`PinContent::clone` 只复制 `Rc`（不复制像素），比入环后
        // `ring.back().expect(..)` 取回来更省心——那条路要靠"容量下界 ≥1"的不变量兜底。
        let req = PinRequest {
            content: content.clone(),
            seq: self.next_seq(),
            origin,
        };
        self.push_ring(content, origin);
        self.cursor = Some(0);
        self.pin_req = Some(req.clone());
        req
    }

    /// 纯截图（`SnapMode::Shot`）入环：**带原位，但不请求开窗**。
    ///
    /// ★ 这是用户口径 2 的正解（2026-09-17）：*"我们单纯截图，而后 pin 图，却出现在屏幕
    /// 中间。说明我们单纯截图的图片，位置信息没有被记录。这是错误的。只要是通过我们的功能
    /// 截图的，就要记录信息。"*
    ///
    /// 与 [`Self::pin_push`] 的差别只有两点：不写 `pin_req`（用户还没按贴图键，不该自己弹窗）、
    /// 游标置 `None`（"下次按贴图键 = 倒数第一 = 这张截图"，与外部复制进来的语义一致）。
    ///
    /// 与环尾同内容时**只刷新原位、不重复入环**——同一张图在环里出现两次会让"沿环回溯"
    /// 看到连续两张一样的图。判同用 `same_content`（`Rc::ptr_eq` 先短路）。
    pub fn note_shot(&mut self, content: PinContent, origin: Rect) {
        match self.ring.back_mut() {
            Some(back) if back.content.same_content(&content) => back.origin = Some(origin),
            _ => self.push_ring(content, Some(origin)),
        }
        self.cursor = None;
    }

    /// 剪贴板轮询收到的**外部复制**（两次按键之间的复制）。
    ///
    /// 与 `pin_push` 的差别只在游标：置 `None` 表示"下次按贴图键从最新的开始"。
    /// 若与环尾相同则什么都不做（同一个内容被反复复制不重复入环）。
    pub fn note_external(&mut self, content: PinContent) -> bool {
        if self
            .ring
            .back()
            .is_some_and(|last| last.content.same_content(&content))
        {
            return false;
        }
        self.push_ring(content, None);
        self.cursor = None;
        true
    }

    /// 贴图快捷键：剪贴板有新内容 → 入环贴出（倒数第一）；否则沿环回溯一格。
    /// 环空且剪贴板也空 → `None`（调用方提示"没有可贴的内容"）。
    ///
    /// 落位由条目自带的 [`RingItem::origin`] 决定：我们自己截的（无论纯截图还是截图并贴图）
    /// 落回原处；外部复制来的居中。
    pub fn pin_next(&mut self, clip: Option<PinContent>) -> Option<PinRequest> {
        if let Some(c) = clip {
            if !self
                .ring
                .back()
                .is_some_and(|last| last.content.same_content(&c))
            {
                // 走到这里说明它是**外部**复制来的内容（我们自己的截图由 `Runtime` 按
                // 剪贴板序列号挡掉，见 `Runtime::pin_next`），所以没有原位 → 居中。
                self.push_ring(c, None);
                self.cursor = None; // 新内容 = 倒数第一
            }
        }
        let idx = self.step()?;
        // 先把要的东西取出来（`Rc` 克隆 + `Option<Rect>` 都是 Copy 语义上的便宜操作），
        // 再 `next_seq()`——否则 `ring.get` 的不可变借用会和 `next_seq` 的 `&mut self` 打架。
        let (content, origin) = {
            let item = self.ring.get(idx)?;
            (item.content.clone(), item.origin)
        };
        let req = PinRequest {
            content,
            seq: self.next_seq(),
            // ★ 原位**从环条目上取**，不是恒 `None`：纯截图进的环带着"我原来在哪"，
            //   于是"先截图、过一会儿再按贴图键"也贴回原处（用户口径 2）。
            //   只有外部剪贴板进来的条目才是 `None` → 屏幕居中。
            origin,
        };
        // ★ 快捷键路径：请求入队（overlay 通道回调 / 250ms 对账经 take_pin_request 开窗）。
        self.pin_req = Some(req.clone());
        Some(req)
    }

    /// 取"该开贴图窗了"的请求（边沿触发）。
    pub fn take_pin_request(&mut self) -> Option<PinRequest> {
        self.pin_req.take()
    }

    /// 登记一个刚打开的贴图窗；超出 [`PIN_WINDOW_CAP`] 时返回**该关掉**的那个序号。
    ///
    /// `content` 与窗口一起记账：选中后 `Ctrl+S` 另存为要拿的就是它（见 [`OpenPin`]）。
    ///
    /// ★ 超上限淘汰**跳过锁定项**（用户口径：锁定了就不该消失），只淘汰最旧的**未锁定**
    ///   项。若所有贴图都锁定且已超上限，返回 `None`（不强关被锁的图——上限在这时是软上限）。
    pub fn note_pin_opened(&mut self, seq: u32, content: PinContent) -> Option<u32> {
        self.open_pins.push_back(OpenPin {
            seq,
            content,
            locked: false,
        });
        if self.open_pins.len() > PIN_WINDOW_CAP {
            // 只从**已存在的旧项**里挑最旧的未锁定项淘汰（新推入的这次是我们要保留的，
            // 且它默认未锁定——若连旧项一起搜会把刚加入的尾巴自己淘汰掉，那就永远留不下新图）。
            let old_n = self.open_pins.len() - 1;
            if let Some(pos) = self.open_pins.iter().take(old_n).position(|p| !p.locked) {
                let oldest = self.open_pins.remove(pos)?;
                // 被淘汰的恰好是选中项 → 选中态跟着一起清（否则 Esc / Ctrl+S 会指向一个
                // 已经不存在的窗口，`FindWindowW` 找不到、表现为"按了没反应"）。
                if self.selected == Some(oldest.seq) {
                    self.selected = None;
                }
                return Some(oldest.seq);
            }
        }
        None
    }

    /// 设置某张贴图的锁定态（右键「锁定图片」）。
    pub fn set_pin_locked(&mut self, seq: u32, locked: bool) {
        if let Some(p) = self.open_pins.iter_mut().find(|p| p.seq == seq) {
            p.locked = locked;
        }
    }

    /// 某张贴图是否锁定（Esc / 双击 / 淘汰前都要问它）。
    pub fn pin_locked(&self, seq: u32) -> bool {
        self.open_pins
            .iter()
            .find(|p| p.seq == seq)
            .map(|p| p.locked)
            .unwrap_or(false)
    }

    /// 登记一个已关闭的贴图窗（用户 Esc / 双击 / 右键关掉的）。
    pub fn note_pin_closed(&mut self, seq: u32) {
        self.open_pins.retain(|p| p.seq != seq);
        if self.selected == Some(seq) {
            self.selected = None;
        }
    }

    /// 存活中的贴图窗数量。
    pub fn open_pin_count(&self) -> usize {
        self.open_pins.len()
    }

    /// 某个贴图窗还有没有活着（`selected` 之外的路径也可能用到）。
    pub fn has_pin(&self, seq: u32) -> bool {
        self.open_pins.iter().any(|p| p.seq == seq)
    }

    /// 取某个贴图窗正在显示的内容（`Ctrl+S` 另存为用）。
    pub fn pin_content(&self, seq: u32) -> Option<PinContent> {
        self.open_pins
            .iter()
            .find(|p| p.seq == seq)
            .map(|p| p.content.clone())
    }

    // ── 选中态（用户口径：Esc / Ctrl+S 都只作用于"我选中的那一张"）──

    /// 选中某张贴图（左键点中时调用）。返回上一个选中项，供调用方把它重绘掉。
    pub fn select_pin(&mut self, seq: u32) -> Option<u32> {
        let old = self.selected;
        self.selected = Some(seq);
        old.filter(|s| *s != seq)
    }

    /// 当前选中的贴图序号。
    pub fn selected_pin(&self) -> Option<u32> {
        self.selected
    }

    /// 取消选中（点了贴图以外的任何地方）。
    pub fn clear_selection(&mut self) -> Option<u32> {
        self.selected.take()
    }

    /// 取走并关闭**选中的那一张**（Esc 的出口）。
    ///
    /// 没选中任何一张时返回 `None` —— 这正是用户要的："我选择了哪张图，Esc 就只取消哪张图"；
    /// 一张都没选时 Esc 不该顺手把所有贴图清空。
    ///
    /// ★ 锁定豁免：选中的那张被「锁定图片」锁定时，返回 `None` 且**保留选中态**
    ///   —— 这样用户按 Esc 图不会消失，还能再右键解锁（清除选中态反而会让用户失去目标）。
    pub fn take_selected(&mut self) -> Option<u32> {
        let seq = self.selected?;
        if self.pin_locked(seq) {
            return None;
        }
        self.selected = None;
        if self.has_pin(seq) {
            Some(seq)
        } else {
            None
        }
    }

    /// 环条目数（诊断 / 测试）。
    pub fn ring_len(&self) -> usize {
        self.ring.len()
    }

    /// 环里的第 `d` 个"倒数"条目（0 = 最新）；越界返回 `None`。诊断用。
    pub fn ring_back(&self, d: usize) -> Option<&PinContent> {
        let len = self.ring.len();
        if d >= len {
            return None;
        }
        self.ring.get(len - 1 - d).map(|it| &it.content)
    }

    /// 环里第 `d` 个"倒数"条目的**原位**（0 = 最新）。诊断 / 测试用。
    pub fn ring_back_origin(&self, d: usize) -> Option<Option<Rect>> {
        let len = self.ring.len();
        if d >= len {
            return None;
        }
        self.ring.get(len - 1 - d).map(|it| it.origin)
    }

    // ── 内部 ──

    fn next_seq(&mut self) -> u32 {
        self.seq = self.seq.wrapping_add(1);
        self.seq
    }

    fn push_ring(&mut self, c: PinContent, origin: Option<Rect>) {
        self.ring.push_back(RingItem { content: c, origin });
        self.trim();
    }

    fn trim(&mut self) {
        while self.ring.len() > self.cap {
            self.ring.pop_front();
        }
    }

    /// 沿环向前走一格（距尾偏移 +1，到头绕回最新）。
    fn step(&mut self) -> Option<usize> {
        let len = self.ring.len();
        if len == 0 {
            return None;
        }
        let d = match self.cursor {
            None => 0,
            Some(d) => {
                if d + 1 >= len {
                    0
                } else {
                    d + 1
                }
            }
        };
        self.cursor = Some(d);
        Some(len - 1 - d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img_stub(w: u32, h: u32, tag: u8) -> PinContent {
        let rgba = vec![tag; (w * h * 4) as usize];
        PinContent::Image(Rc::new(RgbaBitmap::new(w, h, rgba).expect("位图构建失败")))
    }

    /// 测试便利：`pin_push` 要原位参数，而绝大多数用例关心的是环语义（没有原位）。
    fn push(s: &mut SnapMgr, c: PinContent) -> PinRequest {
        s.pin_push(c, None)
    }

    /// ★★ 锁定豁免的两条事实（Esc 关贴图，用户 2026-10-03 报"没反应"就是撞在这里）。
    ///
    /// ① 锁定时 `take_selected` 返回 `None`（关不掉）—— 这是对的，锁定就该关不掉。
    /// ② 但**选中态必须保留**：清掉它用户就失去了目标，连右键解除锁定都找不到那张图。
    ///
    /// 于是"关不掉"这件事必须由**上层**给反馈（`Runtime::notify_pin_locked_cannot_close`
    /// → 一句「该图片已锁定，右键可解除锁定」）。本测试钉住 ①②，让上层那个反馈有据可依：
    /// 上层靠 `selected_pin_is_locked()` 区分"没选中"与"选中但锁定"，它必须为真。
    #[test]
    fn a_locked_pin_survives_esc_and_stays_reachable() {
        let mut s = SnapMgr::new();
        // ★ 贴图"真的开在屏幕上"要走两步：`pin_push` 只登记**请求**（环 + 待开窗），
        //   窗口真建起来时才 `note_pin_opened` 进 `open_pins`。锁定态挂在 `open_pins`
        //   上，所以测试必须走完这两步 —— 只 push 的话 `pin_locked` 恒为 false。
        let r = push(&mut s, img_stub(2, 2, 1));
        s.note_pin_opened(r.seq, r.content.clone());
        // 注意 select_pin 返回的是「上一个选中项」，首次选中恒为 None，不是错误。
        s.select_pin(r.seq);

        // 未锁定：Esc 能关掉，且选中态随之清空。
        assert_eq!(s.take_selected(), Some(r.seq), "未锁定时 Esc 应当关得掉");
        assert_eq!(s.selected_pin(), None, "关掉后不该还留着选中态");
        s.note_pin_closed(r.seq);

        // 锁定：Esc 关不掉，但选中态必须留着。
        let r2 = push(&mut s, img_stub(2, 2, 2));
        s.note_pin_opened(r2.seq, r2.content.clone());
        s.select_pin(r2.seq);
        s.set_pin_locked(r2.seq, true);
        assert!(
            s.pin_locked(r2.seq),
            "锁定态应生效——上层就靠它判'是不是因为锁住了才关不掉'"
        );

        assert_eq!(s.take_selected(), None, "锁定时不该关得掉（豁免成立）");
        assert_eq!(
            s.selected_pin(),
            Some(r2.seq),
            "★ 锁着时选中态必须保留：清掉用户就找不到该右键哪张图解除锁定"
        );
    }

    /// ★ 用户语义：连续按贴图快捷键 → 倒数第一、倒数第二、倒数第三……到头再循环。
    #[test]
    fn pin_walks_history_backwards_then_wraps() {
        let mut s = SnapMgr::new();
        push(&mut s, img_stub(2, 2, 1));
        push(&mut s, img_stub(2, 2, 2));
        let r3 = push(&mut s, img_stub(2, 2, 3));
        assert_eq!(r3.content.bitmap().rgba[0], 3);
        // 第二按 → 倒数第二
        let r = s.pin_next(None).expect("环非空应可回溯");
        assert_eq!(r.content.bitmap().rgba[0], 2);
        // 第三按 → 倒数第三
        let r = s.pin_next(None).expect("环非空应可回溯");
        assert_eq!(r.content.bitmap().rgba[0], 1);
        // 第四按 → 绕回最新（倒数第一）
        let r = s.pin_next(None).expect("环非空应可回溯");
        assert_eq!(r.content.bitmap().rgba[0], 3);
    }

    /// 剪贴板的新内容就是"倒数第一"。
    #[test]
    fn pin_clipboard_new_content_becomes_latest() {
        let mut s = SnapMgr::new();
        push(&mut s, img_stub(2, 2, 1));
        let _ = s.take_pin_request();
        let r = s.pin_next(Some(img_stub(2, 2, 9))).expect("应有贴图请求");
        assert_eq!(r.content.bitmap().rgba[0], 9);
        assert_eq!(s.ring_len(), 2, "新内容入环");
        // 再按 → 回溯到前一份
        let r2 = s.pin_next(None).expect("环非空应可回溯");
        assert_eq!(r2.content.bitmap().rgba[0], 1);
    }

    /// 剪贴板内容与环尾相同（贴图后没复制过新东西）→ 不重复入环，直接回溯。
    #[test]
    fn pin_clipboard_same_as_tail_walks_instead_of_duplicating() {
        let mut s = SnapMgr::new();
        push(&mut s, img_stub(2, 2, 7));
        let _ = s.take_pin_request();
        let r = s.pin_next(Some(img_stub(2, 2, 7))).expect("应有贴图请求");
        assert_eq!(r.content.bitmap().rgba[0], 7);
        assert_eq!(s.ring_len(), 1, "不应重复入环");
    }

    /// ★ 容量：连按 12 次只保留最近 10 个，顺序符合"倒数第 N"。
    #[test]
    fn ring_keeps_only_the_ten_most_recent() {
        let mut s = SnapMgr::new();
        assert_eq!(s.cap(), PIN_RING_CAP);
        for i in 1..=12_u8 {
            push(&mut s, img_stub(2, 2, i));
        }
        assert_eq!(s.ring_len(), PIN_RING_CAP);
        assert_eq!(s.ring_back(0).expect("有最新").bitmap().rgba[0], 12);
        assert_eq!(s.ring_back(9).expect("有第 10 个").bitmap().rgba[0], 3);
        assert!(s.ring_back(10).is_none(), "第 11 个已被淘汰");
        // 回溯：`pin_push` 已经把"倒数第一"贴出去了（游标停在最新），所以接下来的每一按
        // 依次是倒数第二、第三……走满 10 步恰好把环内 10 个内容各贴一遍，第 11 按回到起点。
        let mut seen = Vec::new();
        for _ in 0..PIN_RING_CAP {
            seen.push(s.pin_next(None).expect("应可回溯").content.bitmap().rgba[0]);
        }
        assert_eq!(seen.len(), PIN_RING_CAP);
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        assert_eq!(
            sorted,
            (3..=12).collect::<Vec<u8>>(),
            "10 按应恰好覆盖环内 10 个不同内容（3..=12），不多不少"
        );
        assert_eq!(seen[0], 11, "游标停在倒数第一，故首按是倒数第二");
        let wrap = s.pin_next(None).expect("应绕回");
        assert_eq!(wrap.content.bitmap().rgba[0], seen[0], "走满一圈回到起点");
    }

    /// 缩容时立刻淘汰最旧的。
    #[test]
    fn shrinking_cap_drops_the_oldest_immediately() {
        let mut s = SnapMgr::new();
        for i in 1..=6_u8 {
            push(&mut s, img_stub(2, 2, i));
        }
        s.set_cap(3);
        assert_eq!(s.ring_len(), 3);
        assert_eq!(s.ring_back(0).expect("最新").bitmap().rgba[0], 6);
        assert_eq!(s.ring_back(2).expect("最旧").bitmap().rgba[0], 4);
        s.set_cap(0); // 非法值钳到 1
        assert_eq!(s.cap(), 1);
        assert_eq!(s.ring_len(), 1);
    }

    /// 轮询收到两次按键之间的复制 → 进环，且下次按贴图键贴的是它（不是倒数第二）。
    #[test]
    fn polled_clipboard_change_enters_the_ring_and_becomes_latest() {
        let mut s = SnapMgr::new();
        let _ = s.pin_next(Some(img_stub(2, 2, 1))).expect("首贴");
        // 用户在两按之间复制了另一张图
        assert!(s.note_external(img_stub(2, 2, 2)));
        let r = s.pin_next(None).expect("应可回溯");
        assert_eq!(
            r.content.bitmap().rgba[0],
            2,
            "轮询收到的新内容应是倒数第一"
        );
        // 同一份内容再复制一次不重复入环
        assert!(!s.note_external(img_stub(2, 2, 2)));
        assert_eq!(s.ring_len(), 2);
    }

    /// 贴图窗记账：超过上限时返回"该关掉"的最早那个。
    #[test]
    fn pin_window_cap_reports_the_oldest_to_close() {
        let mut s = SnapMgr::new();
        for i in 1..=PIN_WINDOW_CAP as u32 {
            assert!(s.note_pin_opened(i, img_stub(2, 2, i as u8)).is_none());
        }
        assert_eq!(s.open_pin_count(), PIN_WINDOW_CAP);
        assert_eq!(
            s.note_pin_opened(99, img_stub(2, 2, 99)),
            Some(1),
            "超出上限应关掉最早那个"
        );
        assert_eq!(s.open_pin_count(), PIN_WINDOW_CAP);
        // 用户手关掉一个之后不再重复关
        s.note_pin_closed(2);
        assert_eq!(s.open_pin_count(), PIN_WINDOW_CAP - 1);
        assert!(!s.has_pin(2), "关掉的窗不该还在记账里");
        assert!(s.has_pin(3));
    }

    /// ★ 选中态：左键点中哪张就选哪张；Esc 只取走那一张，其余原封不动。
    ///
    /// 用户报的 bug 是"Esc 把所有贴图都取消了"。这条直接钉住"只关选中的那一张"，
    /// 以及"一张都没选时 Esc 不能顺手全清"。
    #[test]
    fn esc_closes_only_the_selected_pin() {
        let mut s = SnapMgr::new();
        for i in 1..=3 {
            s.note_pin_opened(i, img_stub(2, 2, i as u8));
        }
        // 一张都没选 → 没有可关的
        assert_eq!(s.take_selected(), None);
        assert_eq!(s.open_pin_count(), 3, "没选中时 Esc 不得清空贴图");
        // 选中 2 号
        assert_eq!(s.select_pin(2), None, "首次选中没有上一个");
        assert_eq!(s.selected_pin(), Some(2));
        assert_eq!(s.take_selected(), Some(2));
        assert_eq!(
            s.open_pin_count(),
            3,
            "取走选中项不改存活记账（关窗由 UI 侧落地）"
        );
        assert_eq!(s.selected_pin(), None, "关掉后选中态必须清空");
        // 再选另一个 → 返回上一个选中项，供调用方重绘
        assert_eq!(s.select_pin(3), None);
        assert_eq!(s.select_pin(1), Some(3), "换选中要报出旧的那张");
    }

    /// ★ 锁定豁免（Esc）：锁定的贴图 `take_selected` 返回 `None` 且**保留选中态**
    ///   —— 用户按 Esc 图不消失，还能右键解锁。锁定不挡「关闭」（走 `note_pin_closed`）。
    #[test]
    fn locked_pin_resists_esc_and_keeps_selection() {
        let mut s = SnapMgr::new();
        s.note_pin_opened(1, img_stub(2, 2, 1));
        s.note_pin_opened(2, img_stub(2, 2, 2));
        assert!(!s.pin_locked(1), "默认未锁定");
        s.set_pin_locked(1, true);
        assert!(s.pin_locked(1));
        s.select_pin(1);
        assert_eq!(
            s.take_selected(),
            None,
            "锁定的选中的图，Esc 取不走（不关闭）"
        );
        assert_eq!(s.selected_pin(), Some(1), "锁定项被 Esc 时选中态必须保留");
        assert!(s.has_pin(1), "锁定的图必须还在记账里");
        // 解锁后再 Esc → 正常关
        s.set_pin_locked(1, false);
        assert_eq!(s.take_selected(), Some(1), "解锁后 Esc 正常关闭");
        // 「关闭」不受锁定限制：直接被记录为关闭
        s.set_pin_locked(2, true);
        s.note_pin_closed(2);
        assert!(!s.has_pin(2), "右键「关闭」直接摘除，锁定不挡关闭");
    }

    /// ★ 锁定豁免（超上限淘汰）：只淘汰最旧的**未锁定**项；全锁定则不淘汰（上限成软上限）。
    #[test]
    fn locked_pin_is_exempt_from_cap_eviction() {
        let mut s = SnapMgr::new();
        // 锁住 1 号（最旧），把其余铺到刚好到上限。
        s.note_pin_opened(1, img_stub(2, 2, 1));
        s.set_pin_locked(1, true);
        for i in 2..=PIN_WINDOW_CAP as u32 {
            assert!(s.note_pin_opened(i, img_stub(2, 2, i as u8)).is_none());
        }
        assert!(s.has_pin(1), "锁定项在铺满前不会被动");
        // 超上限：应淘汰最旧的未锁定（2 号），而不是锁定的 1 号。
        assert_eq!(
            s.note_pin_opened(99, img_stub(2, 2, 99)),
            Some(2),
            "超上限应淘汰最旧的未锁定项"
        );
        assert!(s.has_pin(1), "锁定的最旧项被豁免");
        assert_eq!(s.open_pin_count(), PIN_WINDOW_CAP);
        // 把现存的所有项（含刚挤进来的 99）都锁上 → 硬挤下一张时没有任何可淘汰项。
        s.set_pin_locked(99, true);
        for k in 3..=(PIN_WINDOW_CAP as u32) {
            s.set_pin_locked(k, true);
        }
        assert_eq!(
            s.note_pin_opened(200, img_stub(2, 2, 200)),
            None,
            "全锁定时硬挤也不淘汰任何一张"
        );
    }

    /// 选中项被关掉 / 被超上限淘汰时，选中态必须自动清空——否则 Esc 与 Ctrl+S 会指向
    /// 一个已经不存在的窗口（`FindWindowW` 找不到，表现为"按了没反应"）。
    #[test]
    fn selection_clears_when_the_selected_pin_disappears() {
        let mut s = SnapMgr::new();
        s.note_pin_opened(1, img_stub(2, 2, 1));
        s.note_pin_opened(2, img_stub(2, 2, 2));
        s.select_pin(2);
        s.note_pin_closed(2);
        assert_eq!(s.selected_pin(), None, "关窗要带走选中态");
        // 超上限淘汰头部（1 号）时，选中 1 号也要被清
        s.select_pin(1);
        for i in 3..=(PIN_WINDOW_CAP as u32 + 2) {
            s.note_pin_opened(i, img_stub(2, 2, i as u8));
        }
        assert!(!s.has_pin(1), "1 号早该被淘汰");
        assert_eq!(s.selected_pin(), None, "被淘汰的选中项要带走选中态");
    }

    /// 点贴图以外的地方 → 取消选中（Ctrl+S 是全局键，不选中时绝不能拦住用户的 Ctrl+S）。
    #[test]
    fn clicking_elsewhere_clears_the_selection() {
        let mut s = SnapMgr::new();
        s.note_pin_opened(1, img_stub(2, 2, 1));
        s.select_pin(1);
        assert_eq!(s.clear_selection(), Some(1));
        assert_eq!(s.selected_pin(), None);
        assert_eq!(s.clear_selection(), None, "幂等：没选中时清不出东西");
    }

    /// 每个贴图窗都能拿回自己的内容（`Ctrl+S` 另存为的数据来源）。
    #[test]
    fn pin_content_is_reachable_by_seq() {
        let mut s = SnapMgr::new();
        s.note_pin_opened(7, img_stub(2, 2, 42));
        assert_eq!(s.pin_content(7).expect("7 号应有内容").bitmap().rgba[0], 42);
        s.note_pin_closed(7);
        assert!(s.pin_content(7).is_none(), "关掉的窗没有内容可取");
    }

    /// 测试用的虚拟桌面（与测试帧 100×80 同尺寸，原点在 0）。
    fn vs() -> ScreenRect {
        ScreenRect::new(0, 0, 100, 80)
    }

    #[test]
    fn snap_begin_is_reentrant_safe() {
        let mut s = SnapMgr::new();
        let mk = || Rc::new(RgbaBitmap::new(4, 3, vec![0; 4 * 3 * 4]).expect("位图"));
        assert!(s.begin(SnapMode::Shot, mk()));
        assert!(!s.begin(SnapMode::ShotPin, mk()), "进行中不得重入");
        assert!(s.take_open_request(), "首次应请求开窗");
        assert!(!s.take_open_request(), "开窗请求边沿触发");
    }

    /// [`SnapMgr::is_pending`] 是**光标放大镜所有权**的判据（区域选择窗 vs 主覆盖层），
    /// 所以它必须严格覆盖"一次区域选择会话"的边界：置上于 `begin`、落下于 `confirm` /
    /// `cancel`。这条不是在测一个 getter —— 它钉住的是"会话从哪到哪"这件事本身。
    #[test]
    fn pending_marks_exactly_one_region_session() {
        let mut s = SnapMgr::new();
        let mk = || Rc::new(RgbaBitmap::new(4, 3, vec![0; 4 * 3 * 4]).expect("位图"));
        assert!(!s.is_pending(), "没开始就没有会话");
        assert!(s.begin(SnapMode::Shot, mk()));
        assert!(
            s.is_pending(),
            "begin 之后会话开始（放大镜此时归区域选择窗）"
        );
        s.cancel();
        assert!(!s.is_pending(), "cancel 之后会话结束（放大镜交还主覆盖层）");
        assert!(s.begin(SnapMode::Shot, mk()));
        assert!(s.is_pending(), "再次开始会话");
        s.confirm(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 2.0,
                h: 2.0,
            },
            vs(),
        )
        .expect("应能裁剪");
        assert!(!s.is_pending(), "confirm 之后会话结束");
    }

    #[test]
    fn snap_confirm_crops_and_consumes_pending() {
        let mut s = SnapMgr::new();
        let mut rgba = vec![0_u8; 100 * 80 * 4];
        for (i, b) in rgba.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let frame = Rc::new(RgbaBitmap::new(100, 80, rgba).expect("位图"));
        assert!(s.begin(SnapMode::ShotPin, frame));
        let (cropped, mode) = s
            .confirm(
                Rect {
                    x: 10.0,
                    y: 20.0,
                    w: 30.0,
                    h: 40.0,
                },
                vs(),
            )
            .expect("应能裁剪");
        assert_eq!((cropped.w, cropped.h), (30, 40));
        assert_eq!(mode, SnapMode::ShotPin);
        assert!(s.pending.is_none(), "确认后待框选帧应被消费");
        // 二次确认无帧可裁
        assert!(s
            .confirm(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 5.0,
                    h: 5.0
                },
                vs()
            )
            .is_none());
    }

    #[test]
    fn snap_confirm_clamps_out_of_bounds_region() {
        let mut s = SnapMgr::new();
        let frame = Rc::new(RgbaBitmap::new(100, 80, vec![0; 100 * 80 * 4]).expect("位图"));
        assert!(s.begin(SnapMode::Shot, frame));
        let (cropped, _) = s
            .confirm(
                Rect {
                    x: -10.0,
                    y: -10.0,
                    w: 200.0,
                    h: 200.0,
                },
                vs(),
            )
            .expect("钳回帧内应能裁剪");
        assert_eq!((cropped.w, cropped.h), (100, 80));
    }

    /// ★★ 编辑态取帧：**拿整张、不裁剪**，而且会话照样结束。
    ///
    /// 这条判据容易被"反正都返回一张 `Rc<RgbaBitmap>`"蒙过去 —— 所以必须断言
    /// **尺寸等于整张冻结帧**（100×80），并且 `pending` 真的被消费掉（否则区域选择
    /// 会话会一直挂着，`layer().region` 恒真、Esc 目标永远指向不存在的区域窗）。
    #[test]
    fn shot_edit_takes_the_whole_frame_and_ends_the_session() {
        let mut s = SnapMgr::new();
        let frame = Rc::new(RgbaBitmap::new(100, 80, vec![7; 100 * 80 * 4]).expect("位图"));
        assert!(s.begin(SnapMode::ShotEdit { pin: true }, frame));
        assert_eq!(
            s.pending_mode(),
            Some(SnapMode::ShotEdit { pin: true }),
            "模式要原样带着 pin 走 —— 否则「截图并贴图」的 Ctrl 变体到确认那一步就退化成纯截图了"
        );

        let (kept, mode) = s.take_pending_frame().expect("应能取到整张帧");
        assert_eq!(
            (kept.w, kept.h),
            (100, 80),
            "取的是**整张**冻结帧，不是裁出来的块"
        );
        assert_eq!(kept.rgba[0], 7, "像素原样带出");
        assert_eq!(mode, SnapMode::ShotEdit { pin: true });
        assert!(!s.is_pending(), "取帧即结束框选会话");
        assert!(s.take_pending_frame().is_none(), "二次取帧无货");
    }

    /// 模式判据本身（`is_edit`）逐模式核对：容易把 `Gif` 也算进编辑态。
    #[test]
    fn only_shot_edit_is_an_edit_session() {
        assert!(SnapMode::ShotEdit { pin: false }.is_edit());
        assert!(SnapMode::ShotEdit { pin: true }.is_edit());
        assert!(!SnapMode::Shot.is_edit());
        assert!(!SnapMode::ShotPin.is_edit());
        assert!(
            !SnapMode::Gif.is_edit(),
            "GIF 选框不是编辑态：它等的是「开始录制」"
        );
        assert!(
            !SnapMode::Mp4Region.is_edit(),
            "选区录屏不是编辑态：确认后立刻进倒计时"
        );
    }

    /// "截图并贴图"的确认必须**同时**入队开窗请求（否则贴图窗永远不出现——
    /// 旧实现只调 `pin_push` 而它不写 `pin_req`，这条路径是死的）。
    #[test]
    fn shot_pin_enqueues_a_window_request() {
        let mut s = SnapMgr::new();
        let _ = s.take_pin_request();
        push(&mut s, img_stub(2, 2, 5));
        let req = s.take_pin_request().expect("按下贴图后应有开窗请求");
        assert_eq!(req.content.bitmap().rgba[0], 5);
        assert!(s.take_pin_request().is_none(), "边沿触发");
    }

    /// ★ 原位随请求走：截图来的贴图带着"我原来在哪"（贴回原处），剪贴板来的没有（居中）。
    #[test]
    fn pin_request_carries_the_origin_only_for_screenshots() {
        let mut s = SnapMgr::new();
        let rect = Rect {
            x: 100.0,
            y: 200.0,
            w: 300.0,
            h: 400.0,
        };
        let req = s.pin_push(img_stub(2, 2, 1), Some(rect));
        assert_eq!(req.origin, Some(rect), "截图贴图必须带原位");
        // 环回溯：原位跟着**条目**走，所以回溯也带着它
        let back = s.pin_next(None).expect("环非空");
        assert_eq!(back.origin, Some(rect), "环回溯要带出条目自己的原位");
        // 外部剪贴板进来的条目没有原位
        let mut s = SnapMgr::new();
        s.note_external(img_stub(2, 2, 9));
        let ext = s.pin_next(None).expect("环非空");
        assert_eq!(ext.origin, None, "外部复制的内容没有原位（居中）");
    }

    /// ★ 用户口径 2 的正面断言：**纯截图之后才按贴图键**，也必须贴回截图那一块。
    ///
    /// 用户报的是"截图并贴图 → 在原位；先单纯截图、过一会儿再贴 → 跑到屏幕中间"。
    /// 根因是纯截图那条路只写剪贴板、**没有记录原位**。这条测试盯住"记录下来了"这件事。
    #[test]
    fn pure_screenshot_remembers_its_origin_for_a_later_pin() {
        let mut s = SnapMgr::new();
        let rect = Rect {
            x: 640.0,
            y: 360.0,
            w: 320.0,
            h: 200.0,
        };
        // 纯截图：只入环，不该自己弹窗
        s.note_shot(img_stub(2, 2, 4), rect);
        assert!(
            s.take_pin_request().is_none(),
            "纯截图不该请求开贴图窗（用户还没按贴图键）"
        );
        assert_eq!(s.ring_len(), 1);
        // 过了一会儿按贴图键（剪贴板里就是我们自己写的那张，调用方已按序列号跳过读取）
        let req = s.pin_next(None).expect("按贴图键应贴出刚才那张截图");
        assert_eq!(req.origin, Some(rect), "纯截图的原位必须被记住并带出来");
        assert_eq!(req.content.bitmap().rgba[0], 4);
    }

    /// 同一张截图重复记录（例如重新确认）只刷新原位，不在环里出现两次——
    /// 否则"沿环回溯"会连续看到两张一模一样的图。
    #[test]
    fn recording_the_same_screenshot_twice_only_refreshes_the_origin() {
        let mut s = SnapMgr::new();
        let first = Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        let second = Rect {
            x: 50.0,
            y: 60.0,
            w: 30.0,
            h: 40.0,
        };
        let img = img_stub(2, 2, 8);
        s.note_shot(img.clone(), first);
        s.note_shot(img, second);
        assert_eq!(s.ring_len(), 1, "同一张图不该重复入环");
        assert_eq!(
            s.ring_back_origin(0),
            Some(Some(second)),
            "原位取最新的那次"
        );
    }
}
