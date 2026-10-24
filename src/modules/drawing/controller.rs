//! 书写控制器：模式、工具、状态机、工具面板命中路由。

use crate::core::backdrop::{Backdrop, DIM_ALPHA, DIM_COLOR};
use crate::core::color::Argb;
use crate::core::draw::DrawList;
use crate::core::geometry::{Point, Rect};
use crate::core::text_metrics;
use crate::core::tick::Tick;
use crate::platform::display::ScreenRect;

use super::dash;
use super::eraser::Eraser;
use super::history::History;
use super::notify::Notify;
use super::pens::{Pen, PenStyle};
use super::settings::{DrawingHotkeys, DrawingSettings};
use super::shapes::{self, ShapeTool};
use super::stroke::{LiveStroke, StrokeEntry, StrokePoints};
use super::toolbar::{self, ToolbarAction};

/// 书写模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawMode {
    Off,
    Screen,
    Whiteboard,
    /// ★★ 截图**编辑画布**（`Ctrl` + 截图快捷键 → 框选 → 落在这一档）。
    ///
    /// 与 [`DrawMode::Screen`] 是**同一件事**（冻结帧当画布 + 笔迹 + 工具栏），差别只有两处：
    /// - 底图**由调用方给**（就是刚才框选用的那张冻结帧），不再自己抓一帧 —— 抓一帧会
    ///   晚于用户框选的那一瞬间，画面已经变了，笔迹就会"画在旧位置上"；
    /// - 多一个[选区](Self::region)：选区**外压暗**、工具栏**贴着选区**放。
    ///
    /// 用户口径（2026-09-23）：*"按住ctrl键停顿时，我希望可以对截图做简单处理，就像屏幕
    /// 标注一样。换言之，截图和屏幕标注在本质上是相同的函数。"* —— 这一档就是那句话的
    /// 代码形态：它不是"另一个标注"，而是同一个标注**多带了一个选区**。
    ShotEdit,
}

impl DrawMode {
    /// 这一档是不是"截图编辑画布"。
    pub fn is_shot_edit(self) -> bool {
        matches!(self, DrawMode::ShotEdit)
    }
}

/// 工具类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Pen,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Eraser,
    /// 序号贴纸：点一下盖一个带编号的圈。
    Stamp,
    /// 马赛克：拖一个矩形，把冻结帧里那块内容打码。
    Mosaic,
}

/// 形状预览。
pub struct PreviewShape {
    pub start: Point,
    pub current: Point,
    pub tool: ShapeTool,
}

/// 文字工具的字号 = 画笔宽度 × 4（对齐 Python 1.0.0 `font.setPixelSize(width * 4)`）。
fn text_size(pen: &Pen) -> f32 {
    (pen.width as f32 * 4.0).max(12.0)
}

/// 该模式在冻结帧上盖什么纱。
///
/// - 白板：**不透明白幕**（`α=255`）——视觉上是纯白板，与 Python 1.0.0 完全一致；
///   `paint_into` 会因此跳过底图 blit，每帧成本与"硬编码一条白 fill"的旧实现相同。
/// - 透明注释：**不铺纱也铺不上**（底图是 `Backdrop::None`，压根没抓屏）。
fn veil_for(mode: DrawMode) -> Option<Argb> {
    match mode {
        DrawMode::Whiteboard => Some(Argb::new(255, 255, 255, 255)),
        _ => None,
    }
}

/// 按线型把一条折线推入绘制表。
///
/// 实线走零拷贝的 [`DrawList::draw_polyline_ref`]——它直接把点追加进点池；线型（虚线/点线/
/// 点划线）才做切段，而切段必然要新分配点数组。把这条分支单列，是为了不让最常见的实线
/// 也付一次整条折线的拷贝：标注时每帧都要重画正在画的这一笔，那条折线可以长达数千点。
///
/// 切段算法本身在 [`dash`] 里，纯几何、无渲染依赖。
fn push_polyline(list: &mut DrawList, points: &[Point], pen: &Pen) {
    let width = pen.width as f32;
    if pen.style == PenStyle::Solid {
        list.draw_polyline_ref(points, width, pen.color);
        return;
    }
    for seg in dash::segments(points, pen.style, width) {
        list.draw_polyline(seg, width, pen.color);
    }
}

/// 书写控制器。
pub struct DrawingController {
    settings: DrawingSettings,
    mode: DrawMode,
    tool: Tool,
    active_pen: usize,
    history: History,
    live: Option<LiveStroke>,
    preview: Option<PreviewShape>,
    eraser: Eraser,
    /// 工具栏悬停气泡的文案表（工具名 + 快捷键）。快捷键是**可改的配置项**，所以随
    /// [`Self::set_settings`] 重建一次，绘制路径只读（不在每帧拼字符串）。
    hints: toolbar::ToolbarHints,
    notify: Notify,
    /// 虚拟桌面矩形（世界坐标原点可能是负的）。工具栏定位与白板铺底都要它。
    screen: ScreenRect,
    /// 系统 DPI。工具栏是按逻辑像素设计的，绘制前要乘它换算成世界单位。
    ui_scale: f32,
    /// 0.5 秒的切换去抖。None 表示从未切换过（允许首次切换）。
    last_toggle: Option<f64>,
    /// 文字工具待落笔的位置。有值 = 正等用户输入（此刻画一个插入符）。
    pending_text: Option<Point>,
    /// 本轮待输入是否已经通知过 UI 层开输入窗（避免每帧重复开）。
    text_prompted: bool,
    /// 序号贴纸的下一个编号（每次退出/清空回到 1）。
    stamp_next: u32,
    /// 这次手势是否已被工具栏吃掉（点按钮而非画一笔）。
    toolbar_consumed: bool,
    /// 左键是否按着（用来分辨"路过工具栏"与"正在画"）。
    pressed: bool,
    /// 鼠标悬停的按钮。
    hover: Option<ToolbarAction>,
    /// ★ 按住已有文字拖动改位置（仅文字工具；见 `begin_stroke` 的 Tool::Text 分支）。
    text_drag: Option<TextDrag>,
    /// 上一次点中的文字条目与时刻：0.45s 内同一处第二次按下 = 双击 → 再编辑。
    last_text_click: Option<(usize, f64)>,
    /// ★ 马赛克画笔：上一个落码点。按住拖动时按步长逐点打码（对齐主流截图工具
    ///    的"马赛克笔"交互：走过即生效，而不是拖一个矩形松手才出现）。
    mosaic_last: Option<Point>,
    /// ★ 待打开的"再编辑"请求（边沿触发，UI 层取走后开预填输入窗）。
    pending_edit: Option<TextEditRequest>,
    text_edit_prompted: bool,
    /// ★ 叠层底图（进入标注时抓一帧整虚拟桌面，退出时释放）。
    ///
    /// 与截图**共用同一个函数**（[`Backdrop::capture`] → `platform::screenshot::grab`）：
    /// 用户的要求是"截图与屏幕画板调用同一个函数"，所以画板也是**冻结屏幕后书写**，
    /// 而不是像 Python 1.0.0 那样铺一层 1-α 的隐形底。
    backdrop: Backdrop,
    /// ★ 截图编辑态下的**选区**（世界物理像素）。它有两个消费者，且必须是同一份：
    /// ① 选区外压暗（画在这块之外的背景才暗）；② 工具栏的落点锚点。
    /// 两者若各存一份，出问题时表现是"工具条跟在旧位置、压暗还停在上一次框的地方"。
    ///
    /// 其余模式恒 `None` —— 那时工具栏贴屏幕底边居中（历史行为）。
    region: Option<Rect>,
    /// ★ 交给 `Runtime` 去办的**跨模块动作**（工具栏按钮 / `Esc` 按下的那一刻置位）。
    ///
    /// 为什么不让控制器自己处理：这些动作的语义**跨越了模块边界** —— 合成产物、写剪贴板、
    /// 弹原生「另存为」、释放那张整屏冻结帧，全在 `Runtime`（或更下层）手里，而
    /// `modules` 不得依赖 `runtime`（分层纪律）。所以模块只**报意图**（边沿触发，与
    /// `take_text_request` / `take_pin_request` 同款），由 `Runtime` 每帧取走执行。
    request: Option<DrawRequest>,
}

/// 「画笔」交给 `Runtime` 去办的跨模块动作（见 [`DrawingController::request`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawRequest {
    /// 截图编辑态**确认**：裁剪 + 合成笔迹 → 剪贴板（+ 默认位置留档）。
    ///
    /// 按的是工具栏那颗「保存」：除了默认位置的留档，还要**再弹一次原生对话框**
    /// 让用户挑落点。（`Enter` 在编辑态已彻底不认 —— 2026-09-23 用户两轮口径。）
    ShotEditConfirm,
    /// 截图编辑态**打勾确认**（工具栏那颗 ✓，★ 2026-10-03 用户口径"底部是不是要有
    /// 打勾选项？表示OK了？"）：与 [`Self::ShotEditConfirm`] 的产物**完全一样**
    /// （裁剪 + 合成笔迹 → 剪贴板 + 默认位置留档 + 入环），**唯一差别是不弹原生框**。
    ///
    /// 为什么不合并成一个带 `save_as: bool` 的变体：见 `apply_action` 里那颗按钮的注释
    /// —— 带布尔开关的变体迟早长成第二个布尔。
    ShotEditDone,
    /// 截图编辑态**取消**：整次截图作废（与 `Esc` / 右键同义），不留任何痕迹。
    ShotEditCancel,
    /// 屏幕标注 / 白板的工具栏「另存为」：把当前画面（底图 + 墨迹）写到用户挑的落点，
    /// **并结束这次标注**（用户口径 2026-09-23：保存按钮直接就是另存为，全部统一）。
    AnnotateSaveAs,
    /// 屏幕标注 / 白板**退出**（`Esc` / 工具栏「退出」/ 再按一次功能快捷键）：
    /// 把当前画面写到**默认位置**（`temp/annotation`），然后退出 —— 用户口径 2026-09-23：
    /// *"平常屏幕标注后退出，就保存在默认位置。以后我们在设置里会有定时清理的功能。"*
    AnnotateExit,
}

/// 一次文字拖动：目标条目下标 + 抓取点相对锚点的偏移 + 是否真的发生了位移。
struct TextDrag {
    idx: usize,
    dx: f32,
    dy: f32,
    moved: bool,
}

/// 一次"再编辑"请求：目标条目、当前位置与现有文字（预填输入窗）。
#[derive(Debug, Clone)]
pub struct TextEditRequest {
    pub idx: usize,
    pub anchor: Point,
    pub text: String,
}

impl DrawingController {
    pub fn new(settings: DrawingSettings) -> Self {
        let hints = toolbar::hints(&settings.hotkeys);
        Self {
            settings,
            mode: DrawMode::Off,
            tool: Tool::Pen,
            active_pen: 0,
            history: History::new(),
            live: None,
            preview: None,
            eraser: Eraser::new(),
            hints,
            notify: Notify::new(),
            screen: ScreenRect::FALLBACK,
            ui_scale: 1.0,
            last_toggle: None,
            pending_text: None,
            text_prompted: false,
            stamp_next: 1,
            toolbar_consumed: false,
            pressed: false,
            hover: None,
            text_drag: None,
            last_text_click: None,
            mosaic_last: None,
            pending_edit: None,
            text_edit_prompted: false,
            backdrop: Backdrop::None,
            region: None,
            request: None,
        }
    }

    /// 注入虚拟桌面矩形（每帧由 Runtime 调用）。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
    }

    /// 注入系统 DPI 缩放。工具栏的尺寸按逻辑像素写，要乘它才是世界单位。
    pub fn set_ui_scale(&mut self, scale: f32) {
        if scale.is_finite() && scale > 0.0 {
            self.ui_scale = scale;
        }
    }

    /// ★ 注入截图编辑态的选区（每帧由 `Runtime` 推一次，与 [`Self::set_screen`] 同款）。
    ///
    /// **所有者是 `Runtime` 的编辑会话，不是这里** —— 控制器只是镜像一份来画压暗与摆工具栏。
    /// 用户在编辑态里拖动选区（缩放/平移）时，选区在 `Runtime` 那边逐帧变，这一条就是把
    /// 变化**当帧**带过来的唯一通路；少了它，压暗与工具条会停在"刚进编辑态时"那个位置。
    ///
    /// 每帧无条件写、不做"变了才更新"的比较：`Option<Rect>` 的比较比一次赋值还贵，
    /// 而调用点在每帧只跑一次。
    pub fn set_region(&mut self, region: Option<Rect>) {
        self.region = region;
    }

    /// 这一落点是否落在工具栏的**某个按钮**上。
    ///
    /// 与 [`Self::begin_stroke`] 里"① 工具栏优先"那条走的是**同一个** [`toolbar::hit`] 与
    /// 同一个 `self.region` 锚点。单独开这个只读查询，是因为截图编辑态的"拖选区"也要按
    /// 落点分流（见 `Runtime::edit_drag`）：它若不先问这一句，工具栏与选区边框带**重叠**的
    /// 那一小块就会被当成拖框吃掉 —— 表现为"某些位置上的按钮点不动"。
    /// 重叠不是臆想：选区几乎铺满屏幕时工具条会退回屏幕底边，而选区底边很可能正好压在那里。
    pub fn toolbar_hit(&self, p: Point) -> bool {
        toolbar::hit(
            p,
            self.screen,
            self.ui_scale,
            self.region,
            self.mode.is_shot_edit(),
        )
        .is_some()
    }

    pub fn toggle(&mut self, mode: DrawMode, now: f64) {
        if let Some(last) = self.last_toggle {
            if now - last < 0.5 {
                return;
            }
        }
        self.last_toggle = Some(now);
        match self.mode {
            DrawMode::Off => self.start(mode),
            // ★ 关掉功能 = "退出" 的另一个手势，走**同一条**出口（默认位置留档再退出）。
            //   只 `stop()` 的话，用户按 Alt+G 关掉画面就等于把刚画的东西扔了 ——
            //   而他按的明明是"关掉"，不是"丢弃"。
            //
            //   编辑态走不到这一支（`DrawingModule::handle_key` 里那条 toggle 对编辑态一律
            //   无效，见那里的注释），但仍拆开写：万一哪天放开，"取消整次截图"与"留档"是
            //   两件完全不同的事，不能混成一条。
            m if m.is_shot_edit() => self.request(DrawRequest::ShotEditCancel),
            _ => self.request(DrawRequest::AnnotateExit),
        }
    }

    fn start(&mut self, mode: DrawMode) {
        // ★★ 屏幕标注 = **透明幕布**（2026-09-29 用户口径：*"只在屏幕上增加一层透明
        //   幕布…只要屏幕可以不冻结，依然可以看到下面的画面"*）：**不抓屏、不冻结**，
        //   叠层自身透明，底下的桌面照常活着（看得见任务管理器里的实时占用）。
        //
        //   省掉的是两笔硬开销：整张虚拟桌面的冻结帧（1080p 约 8 MB、4K 约 33 MB）与
        //   **每帧铺满整屏的一次位图光栅** —— 这正是"开启标注后内存飙到 50 MB + 书写
        //   卡顿"的大头。产物背景改成**保存那一刻**现抓桌面（见
        //   `runtime::snap_flow::compose_annotation`）。
        //
        //   白板**仍然抓一帧**：它的"纸"是内容的一部分，铺满白幕后那张帧看不见，但
        //   撤销 / 压暗等逻辑仍以它为底；且白板要的就是"看不见桌面"。
        let bd = match mode {
            DrawMode::Screen => Backdrop::None,
            _ => Backdrop::capture(
                crate::platform::screenshot::grab,
                self.screen,
                veil_for(mode),
            ),
        };
        // ★ 快捷键那条路永远走 `Screen` / `Whiteboard`（`ShotEdit` 只能由截图的框选流程
        //   进来，见 `start_with_backdrop`），但仍要显式拒绝 —— 否则 `toggle(ShotEdit)`
        //   会先写下 `mode` 再从 `_ => return` 溜走，留下"模式已是 ShotEdit、底图却是
        //   `None`"的半截状态。
        if mode.is_shot_edit() {
            return;
        }
        self.begin_with(mode, bd, None);
    }

    /// ★★ 进入**截图编辑画布**：底图**由调用方给**（就是用户框选时那一张冻结帧），
    /// 并且带上选区。
    ///
    /// 为什么不自己抓一帧（`Backdrop::capture`）：那会抓到"框选结束之后"的画面。用户
    /// 在框选时看到的、以及即将被裁进图片的，是**开始截图那一刻**的冻结帧；两者一错位，
    /// 笔迹就会落在与图片对不上的位置上（用户看到的是"我画的圈没圈住那个按钮"）。
    /// 这也是"截图与屏幕标注本质上是同一个函数"的最直白体现：同一份底图，只是换了入口。
    ///
    /// `region` 是世界物理像素的选区：它同时决定**选区外压暗**与**工具栏的落点**。
    pub fn start_with_backdrop(&mut self, backdrop: Backdrop, region: Rect) {
        self.begin_with(DrawMode::ShotEdit, backdrop, Some(region));
    }

    /// 进入某模式的共同部分（底图已经准备好）。
    fn begin_with(&mut self, mode: DrawMode, backdrop: Backdrop, region: Option<Rect>) {
        self.mode = mode;
        self.region = region;
        self.backdrop = backdrop;
        self.request = None;
        self.notify.show(match mode {
            DrawMode::Screen => "屏幕标注模式",
            DrawMode::Whiteboard => "白板模式",
            // ★ 把两个出口写在浮字里：这是全新的交互（框完不停、还能画），用户第一次
            //   遇到时最需要知道的正是"怎么结束"。20 个字以内，2 秒后自己淡掉。
            //   （编辑态按 Enter 毫无反应 —— 出口只有工具栏「保存」与 Esc。）
            DrawMode::ShotEdit => "截图编辑：拖边框调选区，工具栏「保存」完成 / Esc 取消",
            DrawMode::Off => return,
        });
        self.tool = Tool::Pen;
        self.reset_gesture();
        self.stamp_next = 1;
    }

    /// 退出绘图（**会报**「已退出绘图」浮字）。
    pub fn stop(&mut self) {
        self.stop_inner(true);
    }

    /// 退出绘图且**不报**浮字 —— 退出那一刻中央要写的是别的话（"已保存到 …"）。
    ///
    /// 两条浮字（本控制器的 `notify` 与 `Runtime` 的 `action_notify`）都是屏幕中央的同一条
    /// 横幅：同时显示只会叠成一团糊字，所以必须由调用方挑一句。
    pub fn stop_quiet(&mut self) {
        self.stop_inner(false);
    }

    fn stop_inner(&mut self, notice: bool) {
        // ★ 截图编辑态**不报**「已退出绘图」：那条浮字与截图的反馈（白闪）都在屏幕中央，
        //   两条一起出现会叠成一团糊字；而且编辑态的两个出口自带语义（确认 = 白闪 + 进剪贴板，
        //   取消 = 画面回到原样），不需要再补一句"退出绘图"。
        let was_shot_edit = self.mode.is_shot_edit();
        self.mode = DrawMode::Off;
        self.region = None;
        self.request = None;
        // ★ 释放冻结帧：整屏位图是 8 MB（1080p）/ 33 MB（4K）级别的常驻内存，
        //   不允许靠"下次进入覆盖掉"回收。
        //
        //   ⚠️ 所以**产物必须在 `stop()` 之前合成**：底图一释放，"底图 + 墨迹"就再也
        //   拼不出来了（`Runtime` 那边因此是"先合成、再 stop"的顺序，见
        //   `Runtime::handle_draw_request`）。
        self.backdrop = Backdrop::None;
        if notice && !was_shot_edit {
            self.notify.show("已退出绘图");
        }
        self.reset_gesture();
        self.history.clear();
        self.stamp_next = 1;
    }

    /// 当前选区（截图编辑态才有；世界物理像素）。
    pub fn region(&self) -> Option<Rect> {
        self.region
    }

    /// 取走跨模块动作的意图（边沿触发），见 [`DrawRequest`]。
    pub fn take_request(&mut self) -> Option<DrawRequest> {
        self.request.take()
    }

    /// 报一个跨模块动作的意图（幂等：先到的那条算数）。
    ///
    /// ★ 只**记意图**，一律不动状态：真正的退出要合成产物、落剪贴板、释放那张整屏冻结帧，
    ///   全是 `Runtime` 的事。若在这里就地 `stop()`，工具栏那颗按钮的 `MouseDown` 就会在
    ///   同一个手势里把控制器清掉，而调用方还握着一个"刚收到退出请求"—— 收尾跑在一张
    ///   已经被拆掉的画布上（底图与笔迹都没了）。
    ///
    /// 幂等：已经有请求时不覆盖 —— 先到的那条算数（用户连点两次「保存」不该变成两次导出）。
    pub fn request(&mut self, what: DrawRequest) {
        if self.request.is_none() {
            self.request = Some(what);
        }
    }

    /// 当前叠层底图（诊断 / 测试）。
    pub fn backdrop(&self) -> &Backdrop {
        &self.backdrop
    }

    /// 把手势相关的临时状态清干净。
    ///
    /// 必须在 `stop()` 里也调：退出是**在 mousedown 里发生的**（点"退出"按钮），而那个
    /// 手势的 mouseup 到不了这里——模块看到 `is_drawing() == false` 就不再转发 up 事件，
    /// 不主动清就会永远停在"按着"和"这一下被吃掉了"的状态上。
    fn reset_gesture(&mut self) {
        self.live = None;
        self.preview = None;
        self.eraser.cancel();
        self.pending_text = None;
        self.text_prompted = false;
        self.toolbar_consumed = false;
        self.pressed = false;
        self.hover = None;
        self.text_drag = None;
        self.last_text_click = None;
        self.pending_edit = None;
        self.text_edit_prompted = false;
    }

    pub fn is_drawing(&self) -> bool {
        self.mode != DrawMode::Off
    }

    pub fn mode(&self) -> DrawMode {
        self.mode
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    pub fn set_active_pen(&mut self, index: usize) {
        if index < self.settings.pens.len() {
            self.active_pen = index;
        }
    }

    /// 用配置里的最新设置替换控制器持有的画笔表（★ 修复：画笔改了不生效）。
    ///
    /// 控制器构造时从 `Config` 拷了一份 [`DrawingSettings`]，落笔宽/色/线型读的都是这份
    /// **快照**——面板改配置后不同步进来，下一次落笔仍用旧参数。`DrawingModule` 在
    /// 配置变化时调本方法（见 [`super::DrawingModule::sync_settings`]）。
    ///
    /// 当前选中的画笔下标按新表长度钳回 0（UI 固定三支笔，这只是防御）。
    pub fn set_settings(&mut self, s: DrawingSettings) {
        if self.active_pen >= s.pens.len() {
            self.active_pen = 0;
        }
        self.settings = s;
        // ★ 快捷键变了，悬停气泡的文案必须跟着变（否则用户改了键，气泡还在报旧的）。
        self.hints = toolbar::hints(&self.settings.hotkeys);
    }

    /// 控制器当前持有的设置快照（供 `DrawingModule` 做变化比对，避免每帧重建）。
    pub fn settings(&self) -> &DrawingSettings {
        &self.settings
    }
    pub fn active_pen(&self) -> &Pen {
        &self.settings.pens[self.active_pen]
    }

    /// 当前画笔序号（0 基）。面板要显示"画笔 N"高亮、测试要断言切换到第几支。
    pub fn active_pen_index(&self) -> usize {
        self.active_pen
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    // ─── 工具面板 ───

    /// 当前工具对应的按钮（用于高亮）。
    pub fn active_action(&self) -> ToolbarAction {
        match self.tool {
            Tool::Pen => ToolbarAction::Pen(self.active_pen),
            Tool::Line => ToolbarAction::Line,
            Tool::Arrow => ToolbarAction::Arrow,
            Tool::Rect => ToolbarAction::Rect,
            Tool::Ellipse => ToolbarAction::Ellipse,
            Tool::Text => ToolbarAction::Text,
            Tool::Stamp => ToolbarAction::Stamp,
            Tool::Eraser => ToolbarAction::Eraser,
            Tool::Mosaic => ToolbarAction::Mosaic,
        }
    }

    /// 鼠标此刻是否落在工具栏上（测试与悬停用）。
    pub fn toolbar_hover(&self) -> Option<ToolbarAction> {
        self.hover
    }

    /// 执行一个面板动作。
    fn apply_action(&mut self, action: ToolbarAction) {
        match action {
            ToolbarAction::Pen(i) => {
                self.set_active_pen(i);
                self.tool = Tool::Pen;
            }
            ToolbarAction::Line => self.tool = Tool::Line,
            ToolbarAction::Arrow => self.tool = Tool::Arrow,
            ToolbarAction::Rect => self.tool = Tool::Rect,
            ToolbarAction::Ellipse => self.tool = Tool::Ellipse,
            ToolbarAction::Text => self.tool = Tool::Text,
            ToolbarAction::Stamp => self.tool = Tool::Stamp,
            ToolbarAction::Eraser => self.tool = Tool::Eraser,
            ToolbarAction::Mosaic => self.tool = Tool::Mosaic,
            ToolbarAction::Undo => {
                self.history.undo();
            }
            ToolbarAction::Redo => {
                self.redo();
            }
            ToolbarAction::Save => {
                // ★★ 「保存」= **另存为**（2026-09-23 用户口径统一）：*"我们的屏幕标注，保存
                //   按钮也要用另存为功能最好。而平常屏幕标注后退出，就保存在默认位置。…
                //   这样子，保存按钮直接就是另存为，全部统一。"*
                //
                //   两个模式的区别只有"产物是什么"：
                //     · 截图编辑态 = 冻结帧 **裁到选区** + 笔迹（确认那次截图，附带挑落点）；
                //     · 标注 / 白板 = 整屏冻结帧 + 笔迹（**不裁**，那就是全部内容）。
                //   共同点是都**结束这次会话**：原生保存对话框是模态的、自带消息泵，而我们的
                //   覆盖层正铺满整屏且此刻整屏收鼠标（`on_nchit` 回 `Client`）——不先收掉
                //   覆盖层，对话框既看不见也点不着。
                let what = if self.mode.is_shot_edit() {
                    DrawRequest::ShotEditConfirm
                } else {
                    DrawRequest::AnnotateSaveAs
                };
                self.request(what);
            }
            ToolbarAction::Clear => self.clear(),
            ToolbarAction::Done => {
                // ★ 打勾 = **确认这次截图**，落默认位置、**不弹框**。
                //   与「保存」分成两个变体而不是共用一个带 `save_as: bool` 的变体：
                //   `Runtime` 侧是 `match` 分发，两个出口各自读起来就是两句话；
                //   而"带布尔开关的变体"迟早会长成第三个布尔（要不要闪、要不要贴图…），
                //   那时每个分支都要重新判断那堆开关的组合。
                self.request(DrawRequest::ShotEditDone);
            }
            ToolbarAction::Exit => {
                // ★ 截图编辑态下「退出」= **取消整次截图**（与 Esc / 右键同义）：把
                //   选区、笔迹、那张冻结帧一起丢掉。只 `stop()` 是不够的 —— 那会留下
                //   "选区还在屏上、却已经画不动了"的半死状态（编辑会话在 `Runtime` 手里）。
                // ★ 标注 / 白板下「退出」= 默认位置留档后退出（用户口径）：与 `Esc` 同一个
                //   出口，同样只**报意图** —— 合成与落盘在 `Runtime`（它才拿得到
                //   `translate_offset` 与 `platform::screenshot`）。
                let what = if self.mode.is_shot_edit() {
                    DrawRequest::ShotEditCancel
                } else {
                    DrawRequest::AnnotateExit
                };
                self.request(what);
            }
        }
    }

    // ─── 笔触 ───

    pub fn begin_stroke(&mut self, p: Point, now_s: f64) {
        // 不在标注模式就什么都不画。调用方（`DrawingModule::tick`）本来就会先判
        // `is_drawing()`，但"退出"恰恰是在**按下那一刻**发生的：点工具栏的「退出」按钮
        // 会把模式置为 Off，而这一趟手势的 mouseup 再也不会被转发过来。把这个守卫放在
        // 状态机自己身上，而不是指望每个调用方都记得——否则退出后随手一按就会留下一笔。
        if !self.is_drawing() {
            return;
        }
        self.pressed = true;
        self.toolbar_consumed = false;
        self.hover = None;

        // ① 工具栏优先：命中即执行动作并吞掉这次手势。
        //    必须在按下那一刻判——用"按下点命中"而不是"松手点"，因为用户可能按下后拖出去。
        if let Some(action) = toolbar::hit(
            p,
            self.screen,
            self.ui_scale,
            self.region,
            self.mode.is_shot_edit(),
        ) {
            self.apply_action(action);
            self.toolbar_consumed = true;
            return;
        }

        match self.tool {
            Tool::Pen => {
                self.live = Some(LiveStroke::begin(p));
            }
            Tool::Eraser => {
                let width = self.active_pen().width;
                self.eraser.begin(p, width);
            }
            Tool::Stamp => {
                self.add_stamp(p);
                // 贴纸是"点一下"的动作，不是拖拽：把这一趟手势整个吃掉，
                // 否则松手时会再走一遍 end_stroke 的形状分支。
                self.toolbar_consumed = true;
            }
            // ★ 马赛克是**画笔**（2026-09-28 二次返工）：按下就开始打码、拖动走过即
            //    生效——对齐主流截图工具的"马赛克笔"。旧版"拖矩形松手才出"被用户
            //    连续两轮报"没有效果"：手感和直觉都不对。
            Tool::Mosaic => {
                self.mosaic_dab(p);
            }
            Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse => {
                self.preview = Some(PreviewShape {
                    start: p,
                    current: p,
                    tool: match self.tool {
                        Tool::Line => ShapeTool::Line,
                        Tool::Arrow => ShapeTool::Arrow,
                        Tool::Rect => ShapeTool::Rect,
                        Tool::Ellipse => ShapeTool::Ellipse,
                        _ => return,
                    },
                });
            }
            Tool::Text => {
                // ★ 命中已有文字 → 拖动改位置 / 双击再编辑（此前文字落笔后完全
                //    不可交互，用户报告）。未命中 → 原行为：记下锚点开新文字。
                if let Some(idx) = self.text_hit(p) {
                    // 双击（0.45s 内第二次点中同一处）→ 开预填输入窗改文字。
                    if self
                        .last_text_click
                        .is_some_and(|(i, t)| i == idx && now_s - t < 0.45)
                    {
                        self.pending_edit = self.text_edit_request(idx);
                        self.text_edit_prompted = false;
                        self.last_text_click = None;
                        self.text_drag = None;
                        // 编辑请求吞掉整趟手势：松手不该有其它副作用。
                        self.toolbar_consumed = true;
                        return;
                    }
                    // 单击/按住 → 准备拖动：记录抓取点相对锦点的偏移，
                    // 移动在 `extend_stroke` 里跟手，松手在 `end_stroke` 收尾。
                    let (dx, dy) = match &self.history.strokes()[idx].points {
                        StrokePoints::Text { anchor, .. } => (p.x - anchor.x, p.y - anchor.y),
                        _ => (0.0, 0.0),
                    };
                    self.text_drag = Some(TextDrag {
                        idx,
                        dx,
                        dy,
                        moved: false,
                    });
                    self.last_text_click = Some((idx, now_s));
                    // 不吞手势：这一趟的 move/up 还要继续喂给拖动状态机。
                    return;
                }
                // 只记下锚点，真正开输入窗是 UI 层的事（这里够不到窗口）。
                self.pending_text = Some(p);
                self.text_prompted = false;
            }
        }
    }

    pub fn extend_stroke(&mut self, p: Point) {
        if self.toolbar_consumed {
            return;
        }
        if !self.pressed {
            // 没按着 → 这一趟只是路过，用来更新工具栏悬停
            self.hover = toolbar::hit(
                p,
                self.screen,
                self.ui_scale,
                self.region,
                self.mode.is_shot_edit(),
            );
            return;
        }
        match self.tool {
            Tool::Pen => {
                if let Some(ref mut live) = self.live {
                    live.extend(p);
                }
            }
            Tool::Eraser => {
                self.eraser.extend(p);
            }
            Tool::Text => {
                // ★ 拖动已有文字：锦点 = 指针位置 - 抓取偏移，bbox 随位移平移。
                //    位移超过 2px 才算真拖动——否则单击（无位移）要留给双击检测。
                let Some(drag) = self.text_drag.as_mut() else {
                    return;
                };
                let Some(entry) = self.history.entry_mut(drag.idx) else {
                    return;
                };
                let StrokePoints::Text { anchor, .. } = &mut entry.points else {
                    return;
                };
                let nx = p.x - drag.dx;
                let ny = p.y - drag.dy;
                let (ddx, ddy) = (nx - anchor.x, ny - anchor.y);
                if ddx.abs() > 2.0 || ddy.abs() > 2.0 {
                    drag.moved = true;
                }
                *anchor = Point { x: nx, y: ny };
                entry.bbox.x += ddx;
                entry.bbox.y += ddy;
            }
            Tool::Mosaic => {
                // 拖动打码：与上一落码点拉开一个步长才落下一码（既保证连续覆盖，
                // 又不至于每个像素都抓一次屏/算一次块平均）。
                let step = self.mosaic_diameter() * 0.3;
                let need = match self.mosaic_last {
                    Some(last) => {
                        let dx = p.x - last.x;
                        let dy = p.y - last.y;
                        dx * dx + dy * dy >= step * step
                    }
                    None => true,
                };
                if need {
                    self.mosaic_dab(p);
                }
            }
            Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse => {
                if let Some(ref mut preview) = self.preview {
                    // ★ Shift 约束（2026-09-22）：按住 Shift 拖拽 → 正方形/正圆、
                    //   水平/垂直直线与箭头。实时查询（keyboard::shift_held），
                    //   拖拽中途按放 Shift 也能即时切换约束。
                    preview.current = if crate::platform::input::keyboard::shift_held() {
                        shapes::constrain_shift(preview.start, p, preview.tool)
                    } else {
                        p
                    };
                }
            }
            _ => {}
        }
    }

    pub fn end_stroke(&mut self) {
        let consumed = std::mem::take(&mut self.toolbar_consumed);
        self.pressed = false;
        if consumed {
            return;
        }
        match self.tool {
            Tool::Pen => {
                if let Some(live) = self.live.take() {
                    let bbox = live.bbox();
                    let points = live.finish();
                    let pen = self.active_pen().clone();
                    self.push_entry(points, bbox, &pen, pen.width as f32, pen.style);
                }
            }
            Tool::Eraser => {
                // ★ 这里以前是 TODO：算出了待删下标却没人用，于是"擦了等于没擦"。
                //   判定统一交给 `History::erase_by_rect`（与 Python 1.0.0 同一语义：
                //   包围盒相交的整条笔迹一律删除）。
                if let Some(rect) = self.eraser.end() {
                    self.history.erase_by_rect(&rect);
                }
            }
            Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse => {
                if let Some(preview) = self.preview.take() {
                    let points = shapes::make_shape(
                        preview.start,
                        preview.current,
                        preview.tool,
                        self.active_pen().width as f32,
                    );
                    let bbox = shapes::shape_bbox(&points);
                    let pen = self.active_pen().clone();
                    self.push_entry(points, bbox, &pen, pen.width as f32, pen.style);
                }
            }
            Tool::Mosaic => {
                // 每一码在落下时就已提交（画笔式），松手只收尾状态。
                self.mosaic_last = None;
            }
            // 文字与贴纸都不在松手时提交（前者等用户敲完，后者在按下时就落了）。
            // ★ 文字拖动在此收尾：moved 已由 extend 更新，位置已实时写进条目。
            Tool::Text => {
                self.text_drag = None;
            }
            Tool::Stamp => {}
        }
    }

    fn push_entry(
        &mut self,
        points: StrokePoints,
        bbox: Rect,
        pen: &Pen,
        width: f32,
        style: PenStyle,
    ) {
        self.history.add(StrokeEntry {
            points,
            color: pen.color,
            width,
            style,
            filled: false,
            bbox,
        });
    }

    /// 盖一个序号贴纸。
    fn add_stamp(&mut self, center: Point) {
        let pen = self.active_pen().clone();
        let radius = shapes::stamp_radius(pen.width);
        let label = self.stamp_next;
        self.stamp_next = self.stamp_next.saturating_add(1);
        let points = StrokePoints::Stamp {
            center,
            radius,
            label,
        };
        let bbox = shapes::shape_bbox(&points);
        // 实心圆不跟笔宽走（宽度仅作为条目数据保留）：圆径由 stamp_radius(width) 定。
        let width = (pen.width as f32 * 0.15).clamp(1.5, 6.0);
        self.push_entry(points, bbox, &pen, width, PenStyle::Solid);
    }

    // ─── 马赛克 ───

    /// 马赛克画笔的直径（世界像素）：随当前笔宽走、钳在 32..=160。笔越粗码越宽，
    /// 与画笔"一支笔一个手感"的心智一致。
    fn mosaic_diameter(&self) -> f32 {
        (self.active_pen().width as f32 * 8.0).clamp(32.0, 160.0)
    }

    /// 在 `center` 落一码：以指针为中心的方形区域即刻打码并提交（画笔式，按下即生效）。
    /// 拖动时由 `extend_stroke` 按步长连续调用。
    fn mosaic_dab(&mut self, center: Point) {
        let d = self.mosaic_diameter();
        let h = d / 2.0;
        self.add_mosaic(Rect {
            x: center.x - h,
            y: center.y - h,
            w: d,
            h: d,
        });
        self.mosaic_last = Some(center);
    }

    /// 提交一块马赛克：从底图里取出 `rect` 那块像素，按块平均打码后贴回原位。
    ///
    /// ★★ 底图与它的**屏幕覆盖范围必须成对取**（2026-09-28 三返工，"马赛克没效果"
    /// 的真根因）：冻结帧（[`Backdrop::Frozen`]）覆盖的是**整张虚拟桌面**，而现场抓的
    /// 帧（[`Backdrop::None`]）只覆盖落码那一小块。旧代码把"落码矩形"统一当底图原点
    /// 传给取样 —— 冻结帧路径于是整体错位，码出来的永远是**屏幕左上角**那块的内容。
    /// 屏幕标注恰恰走冻结帧，所以用户两轮都看到"马赛克没有任何效果"。
    ///
    /// **透明标注**模式抓不到冻结帧时**现场抓取**（`CAPTUREBLT` 连覆盖层已有墨迹一起
    /// 抓，所见即所码）。白板是纯色幕，整块幕色平均还是幕色 —— 没有可打码的内容，
    /// 直接不落码。取样彻底失败就放弃这一笔，**绝不**贴一块黑补丁。
    fn add_mosaic(&mut self, rect: Rect) {
        // 最小尺寸：拖了个空气就什么都不做（与形状工具的"点一下不落形状"同口径）。
        if rect.w < 2.0 || rect.h < 2.0 {
            return;
        }
        let s = self.screen;
        let cover = crate::platform::display::ScreenRect {
            x: s.x,
            y: s.y,
            w: s.w,
            h: s.h,
        };
        // 底图 + 它的屏幕覆盖范围，**成对**产出。
        let pair: (
            std::rc::Rc<crate::core::image::RgbaBitmap>,
            crate::platform::display::ScreenRect,
        ) = match &self.backdrop {
            Backdrop::Frozen(b) => (b.clone(), cover),
            // 纯色幕（白板）：幕色打码还是幕色，落了也看不见 —— 不落码。
            Backdrop::Veiled(_, v) if v.a() == 255 => return,
            // 半透幕（稿纸，预留未接线）：仍以冻结帧为源，颜色会偏；开放时再补合成。
            Backdrop::Veiled(b, _) => (b.clone(), cover),
            Backdrop::None => {
                // 现场抓落码那一小块（世界矩形钳进虚拟桌面），抓到的帧就只覆盖这块。
                let x0 = (rect.x.round() as i32).max(s.x);
                let y0 = (rect.y.round() as i32).max(s.y);
                let x1 = ((rect.x + rect.w).round() as i32).min(s.x + s.w);
                let y1 = ((rect.y + rect.h).round() as i32).min(s.y + s.h);
                if x1 <= x0 || y1 <= y0 {
                    return;
                }
                let region = crate::platform::display::ScreenRect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                };
                let Some(shot) = crate::platform::screenshot::grab(region).ok() else {
                    return;
                };
                let Some(bmp) = crate::core::backdrop::shot_to_bitmap_owned(shot) else {
                    return;
                };
                (std::rc::Rc::new(bmp), region)
            }
        };
        self.push_mosaic(&pair.0, pair.1, rect);
    }

    /// 由"底图 + 它的屏幕覆盖范围 + 落码矩形"算补丁并入库。
    fn push_mosaic(
        &mut self,
        bmp: &crate::core::image::RgbaBitmap,
        cover: crate::platform::display::ScreenRect,
        rect: Rect,
    ) {
        let block = mosaic_block(&rect);
        let (patch, dest) = match crate::core::image::mosaic_patch(bmp, rect, cover, block) {
            Some(p) => p,
            None => return,
        };
        let pen = self.active_pen().clone();
        self.history.add(StrokeEntry {
            points: StrokePoints::Mosaic {
                rect: dest,
                px: std::rc::Rc::new(patch),
            },
            color: pen.color,
            width: pen.width as f32,
            style: pen.style,
            filled: false,
            bbox: dest,
        });
    }

    // ─── 文字工具 ───

    /// 是否有待输入的文字（UI 层据此决定是否开输入窗）。
    pub fn has_pending_text(&self) -> bool {
        self.pending_text.is_some()
    }

    /// 取一次"该开输入窗了"的请求。**边沿触发**：同一处只返回一次，避免每帧重复开窗。
    pub fn take_text_request(&mut self) -> Option<Point> {
        if self.pending_text.is_some() && !self.text_prompted {
            self.text_prompted = true;
            return self.pending_text;
        }
        None
    }

    /// 落笔一段文字。空串视为取消。返回是否真的落了。
    pub fn commit_text(&mut self, text: &str) -> bool {
        self.text_prompted = false;
        let Some(anchor) = self.pending_text.take() else {
            return false;
        };
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let pen = self.active_pen().clone();
        let points = StrokePoints::Text {
            anchor,
            text: text.to_string(),
            size_px: text_size(&pen),
        };
        let bbox = shapes::shape_bbox(&points);
        self.push_entry(points, bbox, &pen, pen.width as f32, pen.style);
        true
    }

    /// 放弃这次文字输入。
    pub fn cancel_text(&mut self) {
        self.pending_text = None;
        self.text_prompted = false;
    }

    /// 命中一处已有文字条目（从最上层往下找，包围格外扩 6px 便于抓取）。
    /// 只认 `StrokePoints::Text`：其它笔迹不参与文字拖动/再编辑。
    fn text_hit(&self, p: Point) -> Option<usize> {
        self.history
            .strokes()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, e)| {
                matches!(e.points, StrokePoints::Text { .. })
                    && e.bbox.inset(-6.0, -6.0).contains(p)
            })
            .map(|(i, _)| i)
    }

    /// 悬停点是否落在可拖动/可双击的文字上（UI 层据此换光标）。
    ///
    /// 只在"标注中 + 文字工具"下为真——其它工具下文字不参与交互，
    /// 光标提示反而撒谎（2026-09-26 用户需求：悬停可拖文本要有指针变化）。
    pub fn text_hover_at(&self, p: Point) -> bool {
        self.is_drawing() && self.tool == Tool::Text && self.text_hit(p).is_some()
    }

    /// 构造一次"再编辑"请求（取条目当前锚点与文字用于预填）。
    fn text_edit_request(&self, idx: usize) -> Option<TextEditRequest> {
        let entry = self.history.strokes().get(idx)?;
        let StrokePoints::Text { anchor, text, .. } = &entry.points else {
            return None;
        };
        Some(TextEditRequest {
            idx,
            anchor: *anchor,
            text: text.clone(),
        })
    }

    /// 取一次"该开再编辑窗了"的请求。**边沿触发**，与 [`Self::take_text_request`] 同一套约定。
    pub fn take_text_edit_request(&mut self) -> Option<TextEditRequest> {
        if self.pending_edit.is_some() && !self.text_edit_prompted {
            self.text_edit_prompted = true;
            return self.pending_edit.clone();
        }
        None
    }

    /// 保存对已有文字的再编辑。空串 = 取消（保留原文）。返回是否真的改了。
    pub fn commit_text_edit(&mut self, idx: usize, text: &str) -> bool {
        self.text_edit_prompted = false;
        self.pending_edit = None;
        let text = text.trim();
        if text.is_empty() {
            return false;
        }
        let Some(entry) = self.history.entry_mut(idx) else {
            return false;
        };
        let StrokePoints::Text { text: old, .. } = &mut entry.points else {
            return false;
        };
        if old == text {
            return false;
        }
        *old = text.to_string();
        // 宽度随文字变化：锦点（基线左端）不动，包围盒按新文字重算。
        entry.bbox = shapes::shape_bbox(&entry.points);
        true
    }

    /// 放弃这次再编辑（关窗/取消，原文不动）。
    pub fn cancel_text_edit(&mut self) {
        self.pending_edit = None;
        self.text_edit_prompted = false;
    }

    // ─── 撤销 / 重做 / 清空 ───

    pub fn undo(&mut self) -> Option<Rect> {
        self.history.undo()
    }

    pub fn redo(&mut self) -> Option<Rect> {
        self.history.redo()
    }

    /// 清空全部笔迹但留在书写模式里（对齐 Python `DrawingController.clear` → `clear_all`）。
    /// 与 [`stop`](Self::stop) 的区别：那个会退出书写模式并把穿透交还系统。
    pub fn clear(&mut self) {
        self.history.clear();
        self.live = None;
        self.preview = None;
        self.eraser.cancel();
        self.pending_text = None;
        self.text_prompted = false;
        self.stamp_next = 1;
        self.text_drag = None;
        self.last_text_click = None;
        self.pending_edit = None;
        self.text_edit_prompted = false;
    }

    /// ★ 内存回收（由 [`DrawingModule::release_memory`] 调用）：把书写的**独占资源**放掉。
    ///
    /// 与 [`clear`](Self::clear) 的区别：`clear` 是用户语义上的"清屏"（留在书写模式、
    /// 仍能继续画，且保留缓冲容量以便马上再画）；本方法是"这个功能被关掉了"——
    /// 要的是**把内存还给系统**，所以额外做两件事：
    ///
    /// 1. `shrink_to_fit`：撤销栈 [`history`] 每次落笔会 push 一串点，`Vec::clear` 只把
    ///    长度归零、**容量留着**（理由是"马上又要用"）。功能已关就没什么"马上"，把
    ///    容量也退掉。
    /// 2. 丢掉冻结帧 [`backdrop`]：它是整张虚拟桌面的 RGBA 副本，是最占地方的一块
    ///    （1080p 约 8 MB、4K 约 33 MB），而退出书写模式后它再也不会被用到。
    pub(crate) fn release_memory(&mut self) {
        self.clear();
        self.history.shrink_to_fit();
        self.backdrop = crate::core::backdrop::Backdrop::None;
        self.reset_gesture();
    }

    pub fn tick(&mut self, tick: &Tick) {
        // ★ 用 `raw_ms`（真实帧间隔）而不是 `dt_ms`：`dt_ms` 经指数滑动平均后
        //   **clamp(0.5, 50)**（见 `core::tick`），帧间隔一大（park 态唤醒的延迟）
        //   真实流逝的时间就被截断成 50ms。而浮字的等待时长恰恰是 `next_change_ms`
        //   报出去的那个"截止时刻"——平台会一直睡到那一刻。两者不匹配的后果是：
        //   每醒一次只推进 50ms，却又要再睡 ~1.9s，2 秒的浮字要 **几十秒**才淡完
        //   （`rec_countdown` 用 `raw_ms` 是同一条理由，见 `runtime::frame`）。
        let dt = tick.raw_ms / 1000.0;
        self.notify.tick(dt);
    }

    /// 距离画面下一次变化还有多少毫秒（提示浮字淡出）。
    ///
    /// `None` = 静止。帧门据此决定"要不要排下一帧"——标注模式空闲时必须报 `None`，
    /// 否则画完最后一笔就一直在空转烧 CPU。
    ///
    /// ★ 曾经这里还有三条"保存导出状态机"的分支（排一帧无工具栏的画面 → 等 150ms 让 GPU
    ///   合成 → 抓屏）。2026-09-23 起导出改成**离屏合成**（`Runtime::compose_annotation`），
    ///   屏面一个像素都不用动，那套状态机连同 `hide_chrome` / `save_stage` 一起删掉了。
    pub fn next_change_ms(&self) -> Option<u32> {
        let remain = self.notify.remain_ms()?;
        Some(remain.max(1))
    }

    // ─── 绘制 ───

    pub fn draw(&self, list: &mut DrawList) {
        let screen = self.screen;

        if self.is_drawing() {
            self.draw_content(list, screen);
        }

        // 提示浮字。**退出之后也要画**：Python 1.0.0 的 `paintEvent` 在 `_mode == "off"`
        // 分支里照样画 notify，所以「已退出绘图」那 2 秒是看得见的；只在标注态画会让
        // 这句提示永远不出现（用户以为退出没生效）。
        self.notify.draw(list, screen);

        // 工具面板压在所有内容之上，只属于标注态——退出后它必须消失，
        // 否则一个可点的工具栏会留在屏幕上（而那时鼠标已经穿透，点它也无效）。
        //
        // ★ 落点锚点 = 当前选区（只有截图编辑态有）。传的是**同一个** `self.region`
        //   给 `hit`（见 `begin_stroke` / `extend` 里的调用）：两处取值一旦不同步，
        //   表现就是"按钮画在这里、点在那里"，而这种错只在编辑态复现，极难猜。
        if self.is_drawing() {
            toolbar::draw(
                list,
                screen,
                self.ui_scale,
                self.region,
                self.mode.is_shot_edit(),
                self.active_action(),
                self.hover,
                &self.settings.pens,
                Some(&self.hints),
            );
        }
    }

    /// 标注内容本体（底图 + 压暗 + 笔迹 + 预览 + 插入符）。
    /// 与提示浮字/工具面板分开，是因为后两者在退出后还有各自不同的处置。
    fn draw_content(&self, list: &mut DrawList, screen: ScreenRect) {
        let full = Rect {
            x: screen.x as f32,
            y: screen.y as f32,
            w: screen.w as f32,
            h: screen.h as f32,
        };

        // 0+1. 叠层底图（★ 与截图**同一个函数**）。
        //
        //   截图区域选择 = `Frozen`（冻结帧 + 选区暗化）；
        //   透明注释     = `None`（**透明幕布**：不冻结，实时桌面可见）；
        //   白板         = `Veiled(.., 白 α=255)`（视觉上仍是纯白板）；
        //   截图编辑     = `Frozen`（**框选时那一张**，由 `start_with_backdrop` 递进来）。
        //
        //   旧实现在这里是两段硬编码：screen 模式铺 `Argb(1,0,0,0)` 当"隐形命中底"
        //   （分层窗按像素 α 做命中测试，全透明像素上的点击会漏给下层窗口），白板模式
        //   铺纯白。现在这两件事都由 `Backdrop` 的 α 语义承担。
        match self.mode {
            DrawMode::Screen | DrawMode::Whiteboard | DrawMode::ShotEdit => {
                self.backdrop.paint_into(list, screen);
                if !self.backdrop.is_opaque() {
                    // 兜底：抓屏失败（`Backdrop::None`）时仍要保证整屏有一个不透明像素，
                    // 否则鼠标点击会漏到底下的窗口。语义与 Python 1.0.0 一致。
                    let fill = match self.mode {
                        DrawMode::Whiteboard => Argb::new(0xFF, 0xFF, 0xFF, 0xFF),
                        _ => Argb::new(1, 0, 0, 0),
                    };
                    list.fill_rect(full, fill);
                }
                // ★ 截图编辑态：**选区外压暗**。
                //
                //   用户口径（2026-09-23）：*"屏幕其它地方的颜色仍然偏暗，就像在确定选区的
                //   时候一样。"* —— 这条不只是美观：它是"我还在截图态里"的唯一全局信号，
                //   与区域选择窗的压暗是**同一个观感**（同一个 α、同一个色相，见
                //   `layer::region` 的 `DIM_ALPHA`；两处都从 [`crate::core::geometry`]
                //   的 `outside_bands` 切边带，四块零重叠——重叠会在压暗层上混两次色，
                //   屏幕上是一个肉眼可见的深色十字）。
                //
                //   画在**笔迹之前**：压暗是"背景的亮度差"。超出选区的笔迹因此仍然看得见
                //   （导出时会被裁掉，但编辑时看得见比突然消失更好理解）。
                if let Some(sel) = self.region {
                    let dim = Argb::new(DIM_ALPHA, DIM_COLOR.0, DIM_COLOR.1, DIM_COLOR.2);
                    for band in crate::core::geometry::outside_bands(full, sel) {
                        if band.w > 0.0 && band.h > 0.0 {
                            list.fill_rect(band, dim);
                        }
                    }
                }
            }
            DrawMode::Off => {}
        }

        self.draw_strokes(list);
    }

    /// 屏上笔迹的**全部**绘制（墨 + 装饰）。
    fn draw_strokes(&self, list: &mut DrawList) {
        self.draw_ink(list);

        // 橡皮预览
        self.eraser
            .draw_preview(list, self.mode == DrawMode::Whiteboard);

        // 文字插入符（等输入时给个落点提示；刻意不做闪烁——闪烁要一直出帧）
        self.draw_text_caret(list);

        // ★ 再编辑高亮：双击进入编辑后给目标条目描一圈同色细框，
        //    让用户明确知道改的是哪一条（关窗后消失）。
        if let Some(req) = &self.pending_edit {
            if let Some(entry) = self.history.strokes().get(req.idx) {
                list.stroke_rect(entry.bbox.inset(-8.0, -8.0), 1.5, entry.color);
            }
        }
    }

    /// ★★ 把笔迹那几层画进指令表 —— **导出**要的正是这几层，且只要这几层。
    ///
    /// 与 [`Self::draw_strokes`] 的差别是刻意去掉三样：橡皮预览、文字插入符、再编辑高亮。
    /// 它们都是"屏上此刻的装饰"（光标处的一圈虚线、一个竖杠、一个选中框），不属于用户
    /// 画上去的墨。虽然它们在用户按确认的那一刻通常都不存在，但"通常"不是不变量 ——
    /// 一条判据应当由代码结构保证，而不是靠"那时候恰好没有"。
    ///
    /// 底图与压暗也不在这里：底图由导出方自己铺（它手上就有那张冻结帧），压暗是**界面**
    /// 效果，绝不该进产物。
    pub fn draw_annotations_into(&self, list: &mut DrawList) {
        self.draw_ink(list);
    }

    /// ★★ 导出画面的全部内容：**底图 + 墨迹**（`Runtime` 的离屏合成只调它）。
    ///
    /// 与屏上的 [`Self::draw_content`] 差三样，每一件都有理由：
    /// - **不给压暗**：选区外压暗是"我还在截图态里"的界面信号，不是图的内容；
    /// - **不给橡皮预览 / 文字插入符 / 再编辑框**（`draw_annotations_into` 本就不含它们）；
    /// - **不给工具栏**：它画在 [`Self::draw`] 里，从来不属于"画面"。
    ///
    /// ★ 这条路取代了 2026-09-23 之前的"隐藏工具栏 → 等 150ms → 抓屏 → PNG"：那条抓的是
    /// **屏幕**（会把此刻盖在上面的别的窗口一起拍进去），而且它依赖的 `hide_chrome` 从某次
    /// 重构起就**只写不读** ⇒ 工具栏其实一直被拍进产物。离屏合成与屏上**同源**（同一份底图、
    /// 同一份笔迹指令表），与"屏上此刻有什么"完全无关，也不需要等待帧。
    pub fn compose_into(&self, list: &mut DrawList) {
        let screen = self.screen;
        self.backdrop.paint_into(list, screen);
        // 白板模式下"纸"是内容的一部分：抓屏失败（`Backdrop::None`）时也要铺白，否则产物
        // 会是一张**透明**底只有墨迹的图。透明注释 / 截图编辑没有"纸"，抓不到屏幕时产物
        // 就只有墨迹（透明底）—— 那是唯一诚实的结局。
        if matches!(self.backdrop, Backdrop::None) && self.mode == DrawMode::Whiteboard {
            list.fill_rect(
                Rect {
                    x: screen.x as f32,
                    y: screen.y as f32,
                    w: screen.w as f32,
                    h: screen.h as f32,
                },
                Argb::new(0xFF, 0xFF, 0xFF, 0xFF),
            );
        }
        self.draw_annotations_into(list);
    }

    /// 产物需不需要**现抓一张桌面当背景**。
    ///
    /// 透明幕布（屏幕标注）在画的时候手里**没有**底图 —— 屏上看到的是活着的桌面，不是
    /// 我们持有的像素。要出图就只能在这一刻抓一张（用户口径：*"我们保存图片的时候，可能
    /// 就要换个方法了，例如以保存时的桌面为背景"*）。白板有"纸"、截图编辑有框选那张帧，
    /// 都不需要。
    pub fn needs_live_backdrop(&self) -> bool {
        self.mode == DrawMode::Screen && matches!(self.backdrop, Backdrop::None)
    }

    /// 换掉底图（只给"出图前补一张现抓的桌面"用 —— 见 [`Self::needs_live_backdrop`]）。
    pub fn set_backdrop(&mut self, backdrop: Backdrop) {
        self.backdrop = backdrop;
    }

    /// 这一趟到底画过东西没有。
    ///
    /// 退出留档（[`DrawRequest::AnnotateExit`]）拿它当判据：**什么都没画就不落文件** ——
    /// 否则每次手滑按一下 `Alt+G` 再 `Esc`，磁盘上都会多一张"和桌面一模一样"的图。
    pub fn has_ink(&self) -> bool {
        !self.history.strokes().is_empty()
    }

    /// 用户真正画上去的墨：历史笔迹 + 正在画的这一笔 + 形状预览。
    fn draw_ink(&self, list: &mut DrawList) {
        for entry in self.history.strokes() {
            self.draw_stroke_entry(list, entry);
        }
        if let Some(ref live) = self.live {
            push_polyline(list, live.points(), self.active_pen());
        }
        if let Some(ref preview) = self.preview {
            let points = shapes::make_shape(
                preview.start,
                preview.current,
                preview.tool,
                self.active_pen().width as f32,
            );
            self.draw_stroke(list, &points, self.active_pen());
        }
    }

    fn draw_text_caret(&self, list: &mut DrawList) {
        let Some(anchor) = self.pending_text else {
            return;
        };
        let pen = self.active_pen();
        let size = text_size(pen);
        let h = text_metrics::line_height(size);
        // ★ 落点标记画在锚点的**右下方**：`DrawCmd::Text` 的 `(x, y)` 是排版盒**左上角**
        //   （见 `core::draw::DrawCmd::Text` 与 `runtime::render` 的翻译），锚点就是盒顶。
        //   此前这画在锚点上方（按"锚点是基线"），与真正落墨的位置差整整一行。
        list.draw_line(anchor, Point::new(anchor.x, anchor.y + h), 2.0, pen.color);
        list.draw_line(
            anchor,
            Point::new(anchor.x + size, anchor.y),
            2.0,
            pen.color,
        );
    }

    fn draw_stroke(&self, list: &mut DrawList, points: &StrokePoints, pen: &Pen) {
        match points {
            StrokePoints::Polyline { points } => {
                push_polyline(list, points, pen);
            }
            StrokePoints::Rect { rect } => {
                list.stroke_rect(*rect, pen.width as f32, pen.color);
            }
            StrokePoints::Ellipse { rect } => {
                list.stroke_ellipse(*rect, pen.width as f32, pen.color);
            }
            StrokePoints::Arrow { outline } => {
                // ★ 一条闭合多边形、一次填充（一体成型）。
                //   此前是"描一条杆 + 填一个三角"两个图元：半透明下交界处被混两次，
                //   那道缝明显更深（实测最暗 64，而单次叠加应为 127）。见
                //   `shapes::make_arrow` 的几何说明与 `examples/shape_probe.rs` 的判据。
                list.fill_polygon(outline, pen.color);
            }
            StrokePoints::Text {
                anchor,
                text,
                size_px,
            } => {
                // ★ 多行（2026-09-26）：按 `\n` 拆行、逐行发一条 `DrawCmd::Text`。
                //   不整串交给渲染层：`DrawCmd::Text` 的矩形高按单次 `measure_text`
                //   取值，多行串的高与逐行排版不一定一致（渲染层 draw_text 的换行
                //   行距与 `text_metrics::line_height` 估高也不同源），逐行拆才能保证
                //   墨迹落进 `shape_bbox` 算出的包围盒——橡皮/再编辑高亮都靠它。
                let lh = crate::core::text_metrics::line_height(*size_px);
                for (i, line) in text.split('\n').enumerate() {
                    let y = anchor.y + i as f32 * lh;
                    list.text(anchor.x, y, line, *size_px, pen.color, None);
                }
            }
            StrokePoints::Stamp {
                center,
                radius,
                label,
            } => {
                // ★ 实心圆 + 反色居中数字（用户需求；此前是空心圆 + 从圆心起排的
                //    文字——文字整体偏右、还容易出圈）。
                //
                //    ★★ 居中交给渲染层（`DrawCmd::TextCentered`）：模块层没有字体，
                //    只能用 `text_metrics` 估算宽高，而这里的估算误差直接可见——实测
                //    数字墨迹偏圆心 **+13px**（用户报告"数字没有在圆的中心"）。
                //    正确做法是按**实测**排版宽度 + 实测 ascent 反推，见
                //    `runtime::render::draw_text_centered`。
                //    字号：单字时取圆径的 1.15 倍，多位数按宽度预算收收，保证不出圈。
                list.fill_circle(*center, *radius, pen.color);
                let label_str = label.to_string();
                let size = shapes::stamp_label_size(*radius, label_str.chars().count());
                list.text_centered(*center, &label_str, size, pen.color.inverse_solid(), None);
            }
            StrokePoints::Mosaic { rect, px } => {
                // 打码补丁 = 一张不透明小位图贴回原位。像素在提交时就定死了，
                // 所以预览 / 合成 / 离屏导出三条路看到的是**同一块**马赛克。
                list.image(px.clone(), *rect, 1.0);
            }
        }
    }

    fn draw_stroke_entry(&self, list: &mut DrawList, entry: &StrokeEntry) {
        self.draw_stroke(
            list,
            &entry.points,
            &Pen {
                color: entry.color,
                width: entry.width as u16,
                style: entry.style,
            },
        );
    }

    pub fn hotkeys(&self) -> &DrawingHotkeys {
        &self.settings.hotkeys
    }
}

/// 马赛克块边长（世界像素）：随选区短边走，小框小格、大框大格，钳在 6..=28。
/// 再大就认不出原内容轮廓，再小就起不到"看不清"的作用 —— 12px 档是常见截图工具
/// 的默认手感，这里用短边 / 12 的比例让同一参数覆盖指甲盖到半屏的选区。
fn mosaic_block(rect: &Rect) -> f32 {
    (rect.w.min(rect.h) / 12.0).round().clamp(6.0, 28.0)
}

/// 把一张画面（底图 + 墨迹）编码为 PNG 落盘到**默认位置**（`temp/annotation/`）。
/// 返回文件路径（失败 None）。
///
/// ★ 调用方是 `Runtime`（退出留档那条路）：合成要用 `runtime::render::translate_offset`
/// 做离屏光栅，而 `modules` 不得依赖 `runtime` —— 所以控制器只负责"把图编码到默认位置"
/// 这件纯本地的事，`pub(crate)` 供同 crate 的 `Runtime` 调用。
///
/// 位图是**非预乘 RGBA8、行优先**——与 `png` crate 的 Rgba8 直通编码一致。
/// 落点是用户看不到的内部回溯目录，编码走 `Fastest`（与 `temp/screenshot/` 同口径）。
pub(crate) fn save_annotation_png(
    bitmap: &crate::core::image::RgbaBitmap,
) -> Option<std::path::PathBuf> {
    use crate::core::paths::annotations_dir;
    let dir = annotations_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("标注_{}.png", timestamp_name()));
    crate::platform::screenshot::save_png_rgba_fast(bitmap, &path).ok()?;
    Some(path)
}

/// 文件名时间戳：本地时刻的 YYYYMMDD_HHMMSS（无 chrono/time 依赖，手工 UTC+8）。
fn timestamp_name() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        + 8 * 3600; // 部署时区 UTC+8
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Howard Hinnant 的 civil_from_days：天数 → (年, 月, 日)
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", y, m, d, h, mi, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> ScreenRect {
        ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        }
    }

    fn ctrl() -> DrawingController {
        let mut c = DrawingController::new(DrawingSettings::default());
        c.set_screen(screen());
        c
    }

    fn p(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    /// 一条绘制指令的粗略包围盒。只用于"某块区域里还留着东西吗"这类判定。
    ///
    /// 文字按"字号见方"估，位图按目标矩形算——两者都不需要精确，只要不把空白判成有内容。
    fn cmd_bbox(list: &DrawList, cmd: &crate::core::draw::DrawCmd) -> Option<Rect> {
        use crate::core::draw::{DrawCmd, Shape};
        let shape = match cmd {
            DrawCmd::Fill { shape, .. } | DrawCmd::Stroke { shape, .. } => shape,
            DrawCmd::Text { x, y, size, .. } => {
                // `(x, y)` 是排版盒左上角（见 `core::draw::DrawCmd::Text`）。
                return Some(Rect {
                    x: *x,
                    y: *y,
                    w: *size,
                    h: *size,
                });
            }
            DrawCmd::TextCentered { center, size, .. } => {
                return Some(Rect {
                    x: center.x - size,
                    y: center.y - size,
                    w: *size * 2.0,
                    h: *size * 2.0,
                });
            }
            DrawCmd::Image { dest, .. } => return Some(*dest),
            _ => return None,
        };
        Some(match shape {
            Shape::Rect { rect, .. } => *rect,
            Shape::Circle { center, radius } => Rect {
                x: center.x - radius,
                y: center.y - radius,
                w: radius * 2.0,
                h: radius * 2.0,
            },
            Shape::Ellipse {
                center,
                radius_x,
                radius_y,
            } => Rect {
                x: center.x - radius_x,
                y: center.y - radius_y,
                w: radius_x * 2.0,
                h: radius_y * 2.0,
            },
            Shape::Line { a, b } => Rect {
                x: a.x.min(b.x),
                y: a.y.min(b.y),
                w: (a.x - b.x).abs(),
                h: (a.y - b.y).abs(),
            },
            Shape::Polyline { points, .. } => {
                let pts = list.range(*points);
                if pts.is_empty() {
                    return None;
                }
                let mut xs = (f32::INFINITY, f32::NEG_INFINITY);
                let mut ys = (f32::INFINITY, f32::NEG_INFINITY);
                for pt in pts {
                    xs.0 = xs.0.min(pt.x);
                    xs.1 = xs.1.max(pt.x);
                    ys.0 = ys.0.min(pt.y);
                    ys.1 = ys.1.max(pt.y);
                }
                Rect {
                    x: xs.0,
                    y: ys.0,
                    w: xs.1 - xs.0,
                    h: ys.1 - ys.0,
                }
            }
        })
    }

    fn rects_overlap(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }

    #[test]
    fn toggle_drawing() {
        let mut c = ctrl();
        assert!(!c.is_drawing());
        c.toggle(DrawMode::Screen, 0.0);
        assert!(c.is_drawing());
        assert_eq!(c.mode(), DrawMode::Screen);
    }

    #[test]
    fn stop_clears_history() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.begin_stroke(p(10.0, 10.0), 0.0);
        c.extend_stroke(p(20.0, 20.0));
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 1);
        c.stop();
        assert_eq!(c.history().stroke_count(), 0);
    }

    /// 0.5s 去抖：窗口内的第二次 `toggle` 整个吃掉；越过窗口的那次才**报退出意图**。
    ///
    /// ★ 2026-09-23 起 `toggle` 不再就地 `stop()`（为什么见 `request` 的注释），所以判据
    ///   从"模式变成 Off"改成"报出了 `AnnotateExit`"，再由本测试自己 `stop()` 收尾。
    #[test]
    fn debounce() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        assert!(c.is_drawing());
        c.toggle(DrawMode::Screen, 0.1);
        assert!(c.is_drawing(), "去抖窗内的第二次不该有任何动作");
        assert!(c.take_request().is_none(), "被去抖吃掉的那次连意图都不该报");
        c.toggle(DrawMode::Screen, 1.0);
        assert_eq!(c.take_request(), Some(DrawRequest::AnnotateExit));
        assert!(c.is_drawing(), "报意图不等于已经停 —— 合成留档还没做");
        c.stop();
        assert!(!c.is_drawing());
    }

    /// ★ 这条是橡皮那个 TODO 的回归锁：擦之前必须真的把笔迹从 history 里去掉。
    #[test]
    fn eraser_actually_removes_strokes() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        // 画两笔，彼此隔开
        for (x0, x1) in [(0.0f32, 40.0f32), (500.0, 560.0)] {
            c.begin_stroke(p(x0, 300.0), 0.0);
            c.extend_stroke(p(x1, 320.0));
            c.end_stroke();
        }
        assert_eq!(c.history().stroke_count(), 2);

        // 用橡皮在那两笔附近扫一下（y=310 落在它们的包围盒里）
        c.set_tool(Tool::Eraser);
        c.begin_stroke(p(0.0, 305.0), 0.0);
        c.extend_stroke(p(40.0, 315.0));
        c.end_stroke();
        assert_eq!(
            c.history().stroke_count(),
            1,
            "第一笔应被擦掉，另一笔（x=500 起）不应受影响"
        );

        // 再扫另一笔附近
        c.begin_stroke(p(500.0, 305.0), 0.0);
        c.extend_stroke(p(560.0, 315.0));
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 0, "第二笔也应被擦掉");
    }

    #[test]
    fn eraser_drag_leaves_no_residual_preview() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Eraser);
        c.begin_stroke(p(100.0, 100.0), 0.0);
        c.extend_stroke(p(200.0, 200.0));
        c.end_stroke();
        assert!(!c.eraser.is_active(), "松手后橡皮不该还在拖动中");

        // 松手后拖动那块地方必须干净。这里按**区域**判定而不是"数 Stroke 指令条数"：
        // 工具面板画在屏幕底部，它自己的图标就是一堆 Stroke，全局计数只会把图标算进来。
        let mut list = DrawList::new();
        c.draw(&mut list);
        let drag = Rect {
            x: 100.0,
            y: 100.0,
            w: 100.0,
            h: 100.0,
        };
        let strays: Vec<&crate::core::draw::DrawCmd> = list
            .cmds()
            .iter()
            .filter(|cmd| {
                // 隐形命中底（alpha=1、铺满虚拟桌面的 Fill）不算残留：它是命中测试
                // 基底，肉眼不可见，也不是这一笔的痕迹（见 draw_content 的第 0 层）。
                if let crate::core::draw::DrawCmd::Fill {
                    paint: crate::core::draw::Paint::Solid(c, _),
                    ..
                } = cmd
                {
                    if c.a() == 1 {
                        return false;
                    }
                }
                // 冻结帧底图（铺满整屏的 Image）同理：它是叠层底板（`Backdrop`），
                // 不是橡皮留下的预览，与"松手后这块地方干不干净"无关。
                if let crate::core::draw::DrawCmd::Image { dest, .. } = cmd {
                    let s = screen();
                    if dest.x <= s.x as f32
                        && dest.y <= s.y as f32
                        && dest.w >= s.w as f32
                        && dest.h >= s.h as f32
                    {
                        return false;
                    }
                }
                cmd_bbox(&list, cmd).is_some_and(|b| rects_overlap(b, drag))
            })
            .collect();
        assert!(
            strays.is_empty(),
            "松手后拖动区域里不该还有指令残留，实测 {} 条: {:?}",
            strays.len(),
            strays
        );
    }

    #[test]
    fn clicking_the_toolbar_switches_tool_and_does_not_draw() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        let cells = toolbar::layout(screen(), 1.0, None, false).cells;
        let (_, rect) = cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Rect)
            .expect("应有矩形按钮");

        c.begin_stroke(rect.center(), 0.0);
        c.extend_stroke(p(rect.center().x + 30.0, rect.center().y + 30.0));
        c.end_stroke();

        assert_eq!(c.tool(), Tool::Rect, "点按钮应切换工具");
        assert_eq!(c.history().stroke_count(), 0, "点按钮不该画出一笔");
    }

    #[test]
    fn toolbar_pen_button_selects_that_pen() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        let cells = toolbar::layout(screen(), 1.0, None, false).cells;
        let (_, rect) = cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Pen(2))
            .expect("应有第 3 支笔的按钮");
        c.begin_stroke(rect.center(), 0.0);
        c.end_stroke();
        assert_eq!(c.active_pen_index(), 2);
        assert_eq!(c.tool(), Tool::Pen);
    }

    #[test]
    fn toolbar_undo_and_clear_work_from_the_bar() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.begin_stroke(p(10.0, 10.0), 0.0);
        c.extend_stroke(p(50.0, 50.0));
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 1);

        let cells = toolbar::layout(screen(), 1.0, None, false).cells;
        let undo = cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Undo)
            .unwrap()
            .1;
        c.begin_stroke(undo.center(), 0.0);
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 0, "撤销按钮应生效");
    }

    /// ★ 马赛克是**画笔**（2026-09-28 二次返工钉行为）：按下瞬间就落第一码，拖开一个
    /// 步长再落下一码；步长内不重复落；松手后新手势能继续。旧版"拖矩形松手才出"被
    /// 用户连续两轮报"没有效果"——交互形态本身就不对。
    #[test]
    fn mosaic_paints_as_a_brush() {
        let mut c = ctrl();
        // 合成一张与 screen 同大的不透明底图（Frozen 模式不走真抓屏，测试可复现）。
        let (w, h) = (1920_u32, 1080_u32);
        let bmp = std::rc::Rc::new(
            crate::core::image::RgbaBitmap::new(w, h, vec![255_u8; (w * h * 4) as usize])
                .expect("位图"),
        );
        c.start_with_backdrop(
            Backdrop::Frozen(bmp),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
            },
        );
        c.tool = Tool::Mosaic;

        // ① 按下瞬间即落第一码（不等松手）。
        c.begin_stroke(p(300.0, 500.0), 1.0);
        assert_eq!(c.history().stroke_count(), 1, "按下就应落第一码");
        let (cx, cy, side) = match &c.history().strokes()[0].points {
            StrokePoints::Mosaic { rect, .. } => {
                (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0, rect.w)
            }
            other => panic!("第一码应是 Mosaic 条目，实际 {other:?}"),
        };
        assert_eq!((cx, cy), (300.0, 500.0), "第一码应以落笔点为中心");
        assert_eq!(side, c.mosaic_diameter(), "码径 = 画笔直径");

        // ② 拖开一个步长 → 第二码；步长内继续拖 → 不重复落。
        let step = c.mosaic_diameter() * 0.3;
        c.extend_stroke(p(300.0 + step + 4.0, 500.0));
        assert_eq!(c.history().stroke_count(), 2, "拖开一个步长应落第二码");
        c.extend_stroke(p(300.0 + step + 6.0, 500.0));
        assert_eq!(c.history().stroke_count(), 2, "步长内不应重复落码");

        // ③ 松手收尾后，新手势能继续落码。
        c.end_stroke();
        c.begin_stroke(p(900.0, 500.0), 2.0);
        assert_eq!(c.history().stroke_count(), 3, "新手势应继续落码");
    }

    /// ★★ 冻结帧路径必须**按落码位置**取样（2026-09-28 三返工钉死）。
    ///
    /// 底图是"红通道 = x 方向渐变"的整屏帧：x=1600 处 ≈ 212，屏幕左上角 ≈ 0。
    /// 旧代码把落码矩形当底图原点传给取样，冻结帧路径永远取到屏幕左上角 ——
    /// 而屏幕标注恰恰走冻结帧，这就是用户两轮报"马赛克没有任何效果"的真根因。
    #[test]
    fn mosaic_samples_the_frozen_frame_at_the_dab_not_the_screen_corner() {
        let (w, h) = (1920_u32, 1080_u32);
        let mut rgba = vec![0_u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = (x * 255 / w) as u8;
                rgba[i + 3] = 255;
            }
        }
        let bmp = std::rc::Rc::new(crate::core::image::RgbaBitmap::new(w, h, rgba).expect("位图"));
        let mut c = ctrl();
        c.start_with_backdrop(
            Backdrop::Frozen(bmp),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 1920.0,
                h: 1080.0,
            },
        );
        c.tool = Tool::Mosaic;
        c.begin_stroke(p(1600.0, 500.0), 1.0);
        assert_eq!(c.history().stroke_count(), 1, "冻结帧路径应落码");
        let StrokePoints::Mosaic { px, .. } = &c.history().strokes()[0].points else {
            panic!("应是马赛克条目");
        };
        let mid = ((px.h as usize / 2) * px.w as usize + px.w as usize / 2) * 4;
        assert!(
            px.rgba[mid] > 180,
            "补丁应取落笔处（红≈212）的像素，实际 {}（≈0 说明取到了屏幕左上角）",
            px.rgba[mid]
        );
    }

    /// ★ 块网格锚在底图原点：相邻两码重叠处的颜色必须逐字节一致 —— 否则接缝处
    /// 同一块背景被拆成两半，肉眼就是"碎格没对齐"。
    #[test]
    fn mosaic_dabs_share_one_block_grid() {
        let (w, h) = (960_u32, 540_u32);
        let mut rgba = vec![0_u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                rgba[i] = ((x / 6) % 251) as u8;
                rgba[i + 1] = ((y / 6) % 241) as u8;
                rgba[i + 2] = ((x / 6 + y / 6) % 233) as u8;
                rgba[i + 3] = 255;
            }
        }
        let bmp = std::rc::Rc::new(crate::core::image::RgbaBitmap::new(w, h, rgba).expect("位图"));
        let mut c = ctrl();
        c.start_with_backdrop(
            Backdrop::Frozen(bmp),
            Rect {
                x: 0.0,
                y: 0.0,
                w: 960.0,
                h: 540.0,
            },
        );
        c.tool = Tool::Mosaic;
        // 两码错开 3px（非块宽 6 的整数倍）—— 旧实现各自起排块时必然接缝错位。
        c.begin_stroke(p(100.0, 100.0), 1.0);
        c.mosaic_dab(p(103.0, 100.0));
        assert_eq!(c.history().stroke_count(), 2);
        let rect_of = |e: &StrokeEntry| match &e.points {
            StrokePoints::Mosaic { rect, .. } => *rect,
            other => panic!("应是马赛克条目，实际 {other:?}"),
        };
        let px_of = |e: &StrokeEntry| match &e.points {
            StrokePoints::Mosaic { px, .. } => px.clone(),
            other => panic!("应是马赛克条目，实际 {other:?}"),
        };
        let (r1, r2) = (
            rect_of(&c.history().strokes()[0]),
            rect_of(&c.history().strokes()[1]),
        );
        let (p1, p2) = (
            px_of(&c.history().strokes()[0]),
            px_of(&c.history().strokes()[1]),
        );
        // 两码重叠区里取世界点 (110, 100)
        let at = |r: Rect, px: &crate::core::image::RgbaBitmap, wx: f32, wy: f32| {
            let lx = (wx - r.x) as usize;
            let ly = (wy - r.y) as usize;
            (ly * px.w as usize + lx) * 4
        };
        let i1 = at(r1, &p1, 110.0, 100.0);
        let i2 = at(r2, &p2, 110.0, 100.0);
        assert_eq!(
            p1.rgba[i1..i1 + 4],
            p2.rgba[i2..i2 + 4],
            "重叠处颜色必须一致（块网格未锚定底图原点）"
        );
    }

    #[test]
    fn exit_button_leaves_drawing_mode_and_resets_the_gesture() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        let cells = toolbar::layout(screen(), 1.0, None, false).cells;
        let exit = cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Exit)
            .unwrap()
            .1;
        c.begin_stroke(exit.center(), 0.0);
        // ★ 2026-09-23 起「退出」按钮只**报意图**：合成产物 + 落盘 + 释放冻结帧都是
        //   `Runtime` 的事（见 `DrawRequest`）。这里能保证、也必须保证的是**这一趟手势
        //   当场自洽** —— 点完退出就再也收不到 mouseup 了，若手势还挂着，松手那一下就会
        //   补画出一笔。
        assert_eq!(c.take_request(), Some(DrawRequest::AnnotateExit));
        c.end_stroke();
        assert_eq!(
            c.history().stroke_count(),
            0,
            "点「退出」那一下不该画出东西（这一趟手势必须已被吃掉）"
        );
        // 模拟 `Runtime` 的收尾，再验一次"停了之后按下不落笔"。
        c.stop_quiet();
        assert!(!c.is_drawing());
        c.begin_stroke(p(300.0, 300.0), 0.0);
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 0, "退出后的按下不该画出东西");
    }

    /// ★ 需求 3 的回归锁：工具栏「保存」= **另存为**，两个模式都只**报意图**。
    ///
    /// 用户口径（2026-09-23）：*"保存时没有提示。……可以让保存按键变成'另存为'功能吗？
    /// ……我们的屏幕标注，保存按钮也要用另存为功能最好。而平常屏幕标注后退出，就保存在默认位置。
    /// ……这样子，保存按钮直接就是另存为，全部统一。"*
    ///
    /// 两档的差别只有"产物是什么"（编辑态 = 冻结帧裁到选区 + 笔迹；标注 = 整屏 + 笔迹），
    /// 所以这里只锁"报的是哪一条意图"—— 合成与弹框全在 `Runtime`（它才拿得到光栅器）。
    #[test]
    fn save_button_requests_save_as_in_both_modes() {
        // ★ `shot_edit` 必须跟着要模拟的那一档走：编辑态的末组多一颗「完成」，
        //   它后面的格子（这里是「保存」右边的「退出」）会整体右移 —— 用标注态的布局
        //   去编辑态找按钮，点下去落在别的按钮的格子上，`take_request` 自然是 `None`。
        //   （这条测试当初就是这么红的：它同时暴露了"闭包共用的布局"这个隐患。）
        let save_at = |region: Option<Rect>, shot_edit: bool| {
            toolbar::layout(screen(), 1.0, region, shot_edit)
                .cells
                .iter()
                .find(|(a, _)| *a == ToolbarAction::Save)
                .expect("工具栏该有「保存」")
                .1
                .center()
        };

        // ① 标注 / 白板：整屏 + 墨迹 → 另存为。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.begin_stroke(save_at(None, false), 0.0);
        assert_eq!(c.take_request(), Some(DrawRequest::AnnotateSaveAs));

        // ② 截图编辑态：这一下同时**结束**这次截图（附带挑落点），所以是 ShotEditConfirm。
        let region = Rect {
            x: 100.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let mut e = ctrl();
        e.start_with_backdrop(Backdrop::None, region);
        e.begin_stroke(save_at(Some(region), true), 0.0);
        assert_eq!(
            e.take_request(),
            Some(DrawRequest::ShotEditConfirm),
            "编辑态的「保存」= 确认（出图）+ 另存为，走的仍是同一条出口"
        );
        assert!(
            e.mode().is_shot_edit(),
            "只报意图：编辑器不该在按下的那一瞬就自毁（合成还没做）"
        );
    }

    /// ★ 任务 2 的回归锁：编辑态的打勾「完成」报的是 `ShotEditDone`，**不是**
    /// `ShotEditConfirm`（2026-10-03 用户口径"底部是不是要有打勾选项？表示OK了？"）。
    ///
    /// 为什么这两个必须分开：`ShotEditConfirm` 在 `Runtime` 侧会 `stash_save_as` 弹原生
    /// 框，而打勾的语义是"就这张、别再问我"——共用一个变体就等于把打勾做成"另存为"，
    /// 用户点的明明是 OK 却弹了框。
    #[test]
    fn the_done_tick_reports_confirm_without_the_save_dialog() {
        let region = Rect {
            x: 100.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let done_at = toolbar::layout(screen(), 1.0, Some(region), true)
            .cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Done)
            .expect("编辑态工具栏该有「完成」")
            .1
            .center();

        let mut e = ctrl();
        e.start_with_backdrop(Backdrop::None, region);
        e.begin_stroke(done_at, 0.0);
        assert_eq!(
            e.take_request(),
            Some(DrawRequest::ShotEditDone),
            "打勾「完成」= 确认出图、落默认位置、不弹框"
        );
        // 与「保存」一样只报意图：合成在 `Runtime`，按下的那一瞬编辑器不能自毁。
        assert!(
            e.mode().is_shot_edit(),
            "只报意图：编辑器不该在按下的那一瞬就自毁（合成还没做）"
        );
    }

    /// 标注 / 白板态**取不到**打勾按钮 —— 与上一条成对，钉住"可见性判据真的接到了
    /// `apply_action` 这条路上"（只测布局不测点击，判据接断了测试照样绿）。
    #[test]
    fn the_done_tick_is_not_reachable_in_annotation_mode() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        // 标注态的布局里根本没有它；用编辑态那颗的坐标点下去，也只能落在别的按钮上。
        let region = Rect {
            x: 100.0,
            y: 100.0,
            w: 800.0,
            h: 600.0,
        };
        let done_at = toolbar::layout(screen(), 1.0, Some(region), true)
            .cells
            .iter()
            .find(|(a, _)| *a == ToolbarAction::Done)
            .expect("编辑态工具栏该有「完成」")
            .1
            .center();
        c.begin_stroke(done_at, 0.0);
        assert_ne!(
            c.take_request(),
            Some(DrawRequest::ShotEditDone),
            "标注态不该能报出打勾「完成」"
        );
    }

    #[test]
    fn hovering_over_the_toolbar_is_reported() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        let cells = toolbar::layout(screen(), 1.0, None, false).cells;
        let (want, rect) = cells[5];
        c.extend_stroke(rect.center());
        assert_eq!(c.toolbar_hover(), Some(want));
        c.extend_stroke(p(400.0, 400.0));
        assert_eq!(c.toolbar_hover(), None, "离开工具栏应清掉悬停");
    }

    #[test]
    fn stamp_lands_on_press_with_increasing_labels() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Stamp);
        c.begin_stroke(p(200.0, 200.0), 0.0);
        c.end_stroke();
        c.begin_stroke(p(300.0, 200.0), 0.0);
        c.end_stroke();
        assert_eq!(c.history().stroke_count(), 2, "每次点击盖一个");
        let labels: Vec<u32> = c
            .history()
            .strokes()
            .iter()
            .map(|e| match e.points {
                StrokePoints::Stamp { label, .. } => label,
                _ => 0,
            })
            .collect();
        assert_eq!(labels, vec![1, 2]);
    }

    #[test]
    fn stamp_numbers_restart_after_clear() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Stamp);
        c.begin_stroke(p(200.0, 200.0), 0.0);
        c.end_stroke();
        c.clear();
        c.begin_stroke(p(300.0, 200.0), 0.0);
        c.end_stroke();
        match c.history().strokes()[0].points {
            StrokePoints::Stamp { label, .. } => assert_eq!(label, 1, "清空后编号应回到 1"),
            _ => panic!("应当是贴纸"),
        }
    }

    #[test]
    fn text_request_fires_once_and_commits_at_the_anchor() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(400.0, 300.0), 0.0);
        c.end_stroke();

        assert!(c.has_pending_text());
        assert_eq!(c.take_text_request(), Some(p(400.0, 300.0)));
        assert_eq!(c.take_text_request(), None, "同一处只该请求一次");
        assert!(c.has_pending_text(), "还没落笔，插入符应继续显示");

        assert!(c.commit_text("步骤一"));
        assert!(!c.has_pending_text());
        assert_eq!(c.history().stroke_count(), 1);
        match &c.history().strokes()[0].points {
            StrokePoints::Text { anchor, text, .. } => {
                assert_eq!(*anchor, p(400.0, 300.0));
                assert_eq!(text, "步骤一");
            }
            _ => panic!("应当是文字笔迹"),
        }
    }

    #[test]
    fn multiline_text_keeps_newlines_and_bbox_covers_all_lines() {
        // ★ 输入框支持 Enter 换行（2026-09-26）：commit 保留内部 `\n`
        //   （只 trim 首尾），包围盒高盖住全部行——否则橡皮/双击命中会漏。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(100.0, 100.0), 0.0);
        assert!(c.commit_text("第一行\n第二行\n\n第四行"));
        assert_eq!(c.history().stroke_count(), 1);
        match &c.history().strokes()[0].points {
            StrokePoints::Text { text, size_px, .. } => {
                assert_eq!(text, "第一行\n第二行\n\n第四行", "内部换行不许被吃掉");
                let bbox = c.history().strokes()[0].bbox;
                let lines = 4;
                assert!(
                    (bbox.h - lines as f32 * crate::core::text_metrics::line_height(*size_px))
                        .abs()
                        < 1.0,
                    "包围盒高应盖住 4 行，得到 {}",
                    bbox.h
                );
            }
            _ => panic!("应当是文字笔迹"),
        }
    }

    #[test]
    fn text_hover_only_over_text_entries_with_the_text_tool() {
        // ★ 光标提示（2026-09-26 用户需求）：悬停已有文字 → 真（换移动光标）；
        //   空处 / 其它工具 → 假（不许撒谎）。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(400.0, 300.0), 0.0);
        assert!(c.commit_text("标注"));
        let bbox = c.history().strokes()[0].bbox;
        let inside = p(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
        let outside = p(bbox.x - 50.0, bbox.y - 50.0);
        assert!(c.text_hover_at(inside), "文字上悬停应提示可拖");
        assert!(!c.text_hover_at(outside), "文字外不该提示");
        c.set_tool(Tool::Pen);
        assert!(
            !c.text_hover_at(inside),
            "非文字工具下文字不参与交互，光标提示不许残留"
        );
    }

    #[test]
    fn empty_text_is_a_cancel() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(1.0, 1.0), 0.0);
        assert!(!c.commit_text("   "));
        assert_eq!(c.history().stroke_count(), 0);
        assert!(!c.has_pending_text());
    }

    #[test]
    fn text_size_follows_the_active_pen() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_active_pen(1); // 宽笔（默认 30）
        c.set_tool(Tool::Text);
        c.begin_stroke(p(10.0, 10.0), 0.0);
        c.commit_text("A");
        match c.history().strokes()[0].points {
            StrokePoints::Text { size_px, .. } => assert_eq!(size_px, 120.0),
            _ => panic!("应当是文字笔迹"),
        }
    }

    #[test]
    fn idle_reports_no_frames_but_the_toast_asks_for_one() {
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        // 刚进模式：提示浮字还在淡出 → 要排帧
        assert!(c.next_change_ms().is_some());
        // 跑完 2 秒 → 必须报静止（否则标注时一直空转）
        // ★ `raw_ms` 必须一起给：`DrawingController::tick` 用**真实帧间隔**递减浮字
        //   剩余时间（`dt_ms` 被 clamp 到 50ms，拿它推进会把这 2 秒拖成几十秒）。
        //   只设 `dt_ms` 的话这里推进量恒为 0，循环跑完浮字还挂着。
        let t = Tick {
            raw_ms: 100.0,
            smooth_ms: 100.0,
            dt_ms: 100.0,
            elapsed_ms: 0.0,
        };
        for _ in 0..40 {
            c.tick(&t);
        }
        assert!(c.next_change_ms().is_none(), "提示消失后应静止");
    }

    /// 按指定线型造一个控制器，并把第 3 支笔（默认就是 dashed 那支）设为当前笔。
    fn ctrl_with_style(style: PenStyle) -> DrawingController {
        let mut s = DrawingSettings::default();
        s.pens[2].style = style;
        let mut c = DrawingController::new(s);
        c.set_screen(screen());
        c.set_active_pen(2);
        c
    }

    /// 数出"落在笔迹区域内"的开放折线指令条数。
    ///
    /// 不能直接数全表：工具栏的「撤销」弧线与「清空」桶身也是折线，提示浮字、白板底
    /// 也各有指令。按区域筛掉工具栏（它在屏幕最底部），只留笔迹那一片。
    fn stroke_polylines(c: &DrawingController) -> usize {
        const MARK: Rect = Rect {
            x: 100.0,
            y: 200.0,
            w: 800.0,
            h: 200.0,
        };
        let mut list = DrawList::new();
        c.draw(&mut list);
        list.cmds()
            .iter()
            .filter(|cmd| {
                matches!(
                    cmd,
                    crate::core::draw::DrawCmd::Stroke {
                        shape: crate::core::draw::Shape::Polyline { closed: false, .. },
                        ..
                    }
                ) && cmd_bbox(&list, cmd).is_some_and(|b| rects_overlap(b, MARK))
            })
            .count()
    }

    /// 面板的「画笔N 线型」下拉必须真的改变屏上画出来的东西。
    ///
    /// 这条锁的是一处实打实的回归：`PenStyle` 从 config 一路传到 `StrokeEntry.style`
    /// 又被塞回一个 `Pen` 里，但 `draw_stroke` **从不读 `pen.style`**——用户在面板里把
    /// 线型改成「虚线」，画出来还是实线，而且这种"设置无效"极难被察觉（笔迹确实出现了）。
    #[test]
    fn pen_style_reaches_the_draw_commands() {
        let mut solid = ctrl_with_style(PenStyle::Solid);
        let mut dashed = ctrl_with_style(PenStyle::Dashed);
        for c in [&mut solid, &mut dashed] {
            c.toggle(DrawMode::Screen, 0.0);
            c.begin_stroke(p(100.0, 300.0), 0.0);
            c.extend_stroke(p(400.0, 300.0));
            c.extend_stroke(p(700.0, 300.0));
            c.end_stroke();
            assert_eq!(c.history().stroke_count(), 1);
        }

        assert_eq!(stroke_polylines(&solid), 1, "实线只该出一条折线");
        let n = stroke_polylines(&dashed);
        assert!(
            n > 10,
            "600px 长、4px 宽（16 划 / 8 空）的虚线应切成 20 段以上，实际 {n} 段"
        );
    }

    #[test]
    fn screen_annotation_is_a_transparent_curtain_not_a_frozen_frame() {
        // ★ 2026-09-29 用户口径：屏幕标注**不冻结**屏幕 —— 底下桌面照常活着，
        //   才能看到任务管理器里的实时内存。冻结帧（+每帧铺满整屏的光栅）正是
        //   "标注时飙到 50 MB、书写卡顿"的大头。回归锁：`start(Screen)` 后底图
        //   必须是 `Backdrop::None`；白板不受影响，仍是冻结帧 + 白幕。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        assert!(
            matches!(c.backdrop(), Backdrop::None),
            "屏幕标注必须是透明幕布（不抓屏），而不是冻结帧"
        );
        // 白板照旧：冻结帧 + 不透明白幕。（时间戳错开 1s，绕开 toggle 的 0.5s 防抖。）
        c.stop();
        c.toggle(DrawMode::Whiteboard, 1.0);
        assert!(
            matches!(c.backdrop(), Backdrop::Veiled(_, v) if v.a() == 255),
            "白板仍应是白幕（用户没要求改它）"
        );
    }

    #[test]
    fn whiteboard_fills_the_whole_virtual_desktop() {
        let mut c = DrawingController::new(DrawingSettings::default());
        c.set_screen(ScreenRect {
            x: -1920,
            y: -100,
            w: 3840,
            h: 1180,
        });
        c.toggle(DrawMode::Whiteboard, 0.0);
        let mut list = DrawList::new();
        c.draw(&mut list);
        let fill = list
            .cmds()
            .iter()
            .find_map(|cmd| match cmd {
                crate::core::draw::DrawCmd::Fill {
                    shape: crate::core::draw::Shape::Rect { rect, .. },
                    ..
                } => Some(*rect),
                _ => None,
            })
            .expect("白板应铺一块底色");
        assert_eq!(fill.x, -1920.0);
        assert_eq!(fill.y, -100.0);
        assert_eq!(fill.w, 3840.0);
        assert_eq!(fill.h, 1180.0);
    }

    #[test]
    fn text_entry_can_be_dragged_to_a_new_position() {
        // ★ 文字落笔后要能拖动改位置（用户报告：完全动不了）。拖动 = 按住已有文字 →
        //   移动 → 松手：锚点随抓取偏移平移，bbox 同步平移，条目数不变。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(100.0, 200.0), 0.0);
        assert!(c.take_text_request().is_some());
        assert!(c.commit_text("你好"));
        assert_eq!(c.history().stroke_count(), 1);
        let bbox_before = c.history().strokes()[0].bbox;

        // 按住文字（锚点 (100,200)，抓取点 (110,195)）横拖 100px。
        c.begin_stroke(p(110.0, 195.0), 1.0);
        c.extend_stroke(p(210.0, 195.0));
        c.end_stroke();

        assert_eq!(c.history().stroke_count(), 1, "拖动不该新增条目");
        let entry = &c.history().strokes()[0];
        let StrokePoints::Text { anchor, .. } = &entry.points else {
            panic!("应是文字条目");
        };
        assert!(
            (anchor.x - 200.0).abs() < 0.01 && (anchor.y - 200.0).abs() < 0.01,
            "锚点应平移到 (200,200)，得到 ({},{})",
            anchor.x,
            anchor.y
        );
        assert!(
            (entry.bbox.x - (bbox_before.x + 100.0)).abs() < 0.01,
            "bbox 应随位移平移"
        );
    }

    #[test]
    fn double_click_reopens_text_edit_and_commit_updates_entry() {
        // ★ 双击已有文字 → 再编辑请求（预填旧文字）→ 提交后条目更新、bbox 重算；
        //   空串提交 = 取消，原文保留。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Text);
        c.begin_stroke(p(100.0, 200.0), 0.0);
        c.commit_text("旧文字");

        // 第一次单击（按住原地 → 松手，无位移）：不进编辑，只记点击。
        c.begin_stroke(p(105.0, 198.0), 10.0);
        c.extend_stroke(p(105.0, 198.0));
        c.end_stroke();
        assert!(c.take_text_edit_request().is_none(), "单击不应触发再编辑");

        // 0.3s 后第二次按下同一处 → 双击 → 再编辑请求。
        c.begin_stroke(p(105.0, 198.0), 10.3);
        let req = c.take_text_edit_request().expect("双击应产生再编辑请求");
        assert_eq!(req.text, "旧文字");

        assert!(c.commit_text_edit(req.idx, "新文字！"));
        let entry = &c.history().strokes()[req.idx];
        let StrokePoints::Text { text, .. } = &entry.points else {
            panic!("应是文字条目");
        };
        assert_eq!(text, "新文字！");
        // bbox 按新文字重算：宽度应变。
        let w_old = crate::core::text_metrics::estimate_width("旧文字", entry_size(entry));
        let w_new = crate::core::text_metrics::estimate_width("新文字！", entry_size(entry));
        assert!((entry.bbox.w - w_new).abs() < 0.5);
        assert!((entry.bbox.w - w_old).abs() > 0.5);

        // 空串 = 取消：原文保留。
        assert!(!c.commit_text_edit(req.idx, "   "));
        let entry = &c.history().strokes()[req.idx];
        let StrokePoints::Text { text, .. } = &entry.points else {
            panic!("应是文字条目");
        };
        assert_eq!(text, "新文字！");
    }

    fn entry_size(e: &StrokeEntry) -> f32 {
        match &e.points {
            StrokePoints::Text { size_px, .. } => *size_px,
            _ => panic!("应是文字条目"),
        }
    }

    #[test]
    fn stamp_draws_solid_circle_with_inverse_centered_digit() {
        // ★ 序号贴纸 = 实心圆（Fill Circle）+ 反色数字，且数字左缘在圆心左侧
        //   （水平居中）；不再有空心圆描边。
        let mut c = ctrl();
        c.toggle(DrawMode::Screen, 0.0);
        c.set_tool(Tool::Stamp);
        c.begin_stroke(p(300.0, 300.0), 0.0);
        assert_eq!(c.history().stroke_count(), 1);

        let mut list = DrawList::new();
        c.draw(&mut list);
        let center = Point::new(300.0, 300.0);
        let mut has_fill_circle_at_stamp = false;
        let mut has_stroke_circle_at_stamp = false;
        let mut digit = None;
        for cmd in list.cmds() {
            match cmd {
                crate::core::draw::DrawCmd::Fill {
                    shape: crate::core::draw::Shape::Circle { center: cc, .. },
                    ..
                } if *cc == center => has_fill_circle_at_stamp = true,
                crate::core::draw::DrawCmd::Stroke {
                    shape: crate::core::draw::Shape::Circle { center: cc, .. },
                    ..
                } if *cc == center => has_stroke_circle_at_stamp = true,
                crate::core::draw::DrawCmd::TextCentered {
                    center: tc, color, ..
                } if *tc == center => {
                    digit = Some(*color);
                }
                _ => {}
            }
        }
        assert!(has_fill_circle_at_stamp, "序号贴纸应有实心圆填充");
        assert!(!has_stroke_circle_at_stamp, "不应再有空心圆描边");
        assert_eq!(
            digit,
            Some(Argb::new(255, 255, 255, 255)),
            "黑笔的数字应为反色（白）"
        );
    }
}
