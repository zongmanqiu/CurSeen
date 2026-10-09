//! 标注工具面板：屏幕底部一条可点的工具栏。
//!
//! # 为什么它不是一个窗口
//!
//! 面板是**标注层的一部分**：画进 `DrawList`（最后画，压在所有笔迹之上），点击由
//! `DrawingController` 在 `begin_stroke` **之前**做命中测试接住。这样：
//!
//!   · 零窗口成本——不新建 HWND、不动穿透样式、不碰焦点与 z 序；
//!   · 与笔迹天然同层——进入标注才出现，退出标注自动消失，不需要显隐管理；
//!   · 复用既有输入通路——鼠标事件本来就来自低级钩子，命中测试就地做掉。
//!
//! 代价只有一个：点按钮与"拖拽画一笔"共用同一个左键。区分办法是看这一下的**位移与时长**
//! （见 `DrawingController::begin_stroke`）：按下即命中就执行动作并吞掉这次手势，否则才是笔触。
//!
//! # 图标
//!
//! CurSeen 用 CPU 软渲染、没有 icon 字体，所以图标全部用图元自绘（线/圆/多边形/文字）。
//! 好处是零依赖、随主题色走；坏处是加图标要写几何——但工具栏一共就这几个，划算。
//!
//! # 坐标
//!
//! 世界坐标 = **虚拟桌面绝对物理像素**（副屏在左时 x 为负），所以工具栏的位置必须由
//! 整块 `ScreenRect` 推出来，不能按 (0,0) 起算。尺寸则乘 `scale`（= 系统 DPI）换算成
//! 物理像素，让工具栏在任何缩放下看起来一样大。

use crate::core::color::Argb;
use crate::core::draw::DrawList;
use crate::core::geometry::{Point, Rect};
use crate::platform::display::ScreenRect;

use super::pens::Pen;
use super::shapes;

// ── 尺寸（逻辑像素，绘制前统一乘 scale）──
const BTN: f32 = 34.0;
const GAP: f32 = 4.0;
const GROUP_GAP: f32 = 14.0;
const PAD: f32 = 8.0;
/// 底边距：不贴着屏幕边，一是好点，二是不撞任务栏。
const BOTTOM_MARGIN: f32 = 30.0;
/// 跟随选区时，工具条与选区边缘之间留的空隙（逻辑像素）。
const ANCHOR_GAP: f32 = 12.0;
/// 跟随选区时，工具条允许贴到屏幕边多近（逻辑像素）。太贴边会撞任务栏、也不好点。
const EDGE_KEEP: f32 = 8.0;
/// 图标半边长。
const ICON_H: f32 = 8.5;
const CORNER: f32 = 10.0;
const CORNER_BTN: f32 = 7.0;

// ── 配色（对齐 `ui/tokens.rs`，覆盖层里没有主题系统，只能写死一份）──
const BAR_BG: Argb = Argb(0xF7FF_FFFF);
const BAR_BORDER: Argb = Argb(0xFFE2_E6EA);
const ICON_FG: Argb = Argb(0xFF2D_3236);
const ACTIVE_BG: Argb = Argb(0xFFE8_F0FE);
const HOVER_BG: Argb = Argb(0xFFED_F0F3);
/// 橡皮**本体**那一截的浅灰（套壳用 `ICON_FG` 的深色，两截分明才认得出是橡皮）。
const ERASER_RUBBER: Argb = Argb(0xFFD5_DAE1);
/// 悬停气泡：深底白字 —— 浅色工具条上唯一压得住的组合。
const TIP_BG: Argb = Argb(0xF2_2B2F35);
const TIP_FG: Argb = Argb(0xFF_FFFFFF);
/// 悬停气泡的字号（逻辑像素）与左右内边距。
const TIP_FONT: f32 = 13.0;
const TIP_PAD_X: f32 = 8.0;
/// 气泡与按钮之间留的缝。
const TIP_GAP: f32 = 6.0;

/// 悬停气泡要显示的**工具名 + 快捷键**。
///
/// 快捷键是**用户可改的配置项**，不能写死在工具栏里；但也不能每帧现拼字符串（绘制
/// 热路径，标注态下每秒 60 次）。所以由 `DrawingController` 在设置变化时重建一次，
/// `draw` 只读——两处都不为对方付代价。
#[derive(Clone, Default)]
pub struct ToolbarHints {
    /// 与 [`Layout::cells`] 同序的 `(动作, 工具名, 快捷键)`；快捷键空串 = 只显示名字。
    items: Vec<(ToolbarAction, &'static str, String)>,
}

impl ToolbarHints {
    pub fn get(&self, a: ToolbarAction) -> Option<(&'static str, &str)> {
        self.items
            .iter()
            .find(|(x, _, _)| *x == a)
            .map(|(_, name, key)| (*name, key.as_str()))
    }
}

/// [`DrawingAction`] → 工具栏按钮（没有对应按钮的返回 `None`）。
///
/// ★ 与 [`GROUPS`] 是"快捷键序 / 按钮序"同源的另一半：正本顺序在
///   `core::hotkeys::DRAW_TOOL_ORDER`，本函数补上"动作 → 按钮"这一跳，两边一合就能测出
///   "按钮序 == 快捷键序"（`button_order_matches_the_hotkey_order`），不必再手抄一份顺序。
pub fn tool_action(a: crate::core::hotkeys::DrawingAction) -> Option<ToolbarAction> {
    use crate::core::hotkeys::DrawingAction as A;
    Some(match a {
        A::Pen1 => ToolbarAction::Pen(0),
        A::Pen2 => ToolbarAction::Pen(1),
        A::Pen3 => ToolbarAction::Pen(2),
        A::Line => ToolbarAction::Line,
        A::Arrow => ToolbarAction::Arrow,
        A::Rect => ToolbarAction::Rect,
        A::Ellipse => ToolbarAction::Ellipse,
        A::Text => ToolbarAction::Text,
        A::Stamp => ToolbarAction::Stamp,
        A::Mosaic => ToolbarAction::Mosaic,
        A::Eraser => ToolbarAction::Eraser,
        // 两把开关是"进这层的方式"，不是工具；工具栏上也没有对应按钮。
        A::ToggleScreen | A::ToggleWhiteboard => return None,
    })
}

/// 按当前快捷键配置生成提示表（**不在绘制路径里调**，见 [`ToolbarHints`]）。
pub fn hints(hk: &crate::core::config::DrawingHotkeys) -> ToolbarHints {
    let k = |s: &str| s.trim().to_string();
    let items = [
        (ToolbarAction::Pen(0), "画笔 1", k(&hk.draw_hk_pen1)),
        (ToolbarAction::Pen(1), "画笔 2", k(&hk.draw_hk_pen2)),
        (ToolbarAction::Pen(2), "画笔 3", k(&hk.draw_hk_pen3)),
        (ToolbarAction::Line, "直线", k(&hk.draw_hk_line)),
        (ToolbarAction::Arrow, "箭头", k(&hk.draw_hk_arrow)),
        (ToolbarAction::Rect, "矩形", k(&hk.draw_hk_rect)),
        (ToolbarAction::Ellipse, "椭圆", k(&hk.draw_hk_ellipse)),
        (ToolbarAction::Text, "文字", k(&hk.draw_hk_text)),
        // ★ 序号**有**可配键位（`draw_hk_stamp`，2026-10-01 起）：工具栏悬停气泡要跟着
        //   配的键显示，配空就是不显示键名（与其余各工具同款口径）。原来这里写死空串，
        //   等于气泡永远不报这把键——用户配了键也看不见。
        (ToolbarAction::Stamp, "序号", k(&hk.draw_hk_stamp)),
        (ToolbarAction::Mosaic, "马赛克", k(&hk.draw_hk_mosaic)),
        (ToolbarAction::Eraser, "橡皮擦", k(&hk.draw_hk_eraser)),
        // 清空 / 撤销 / 重做 / 保存 / 退出：快捷键不在 `DrawingHotkeys` 配置里
        // （是固定键 / 无快捷键），这里**不猜**，只显示名字——宁可少写，也不显示错的。
        (ToolbarAction::Undo, "撤销", String::new()),
        (ToolbarAction::Redo, "重做", String::new()),
        (ToolbarAction::Clear, "清空", String::new()),
        (ToolbarAction::Save, "保存", String::new()),
        // 「完成」无快捷键（同上），只在截图编辑态出现（见 `ToolbarAction::Done`）。
        (ToolbarAction::Done, "完成", String::new()),
        (ToolbarAction::Exit, "退出", String::new()),
    ];
    ToolbarHints {
        items: items.to_vec(),
    }
}

/// 面板上的一个动作。**只描述"点了什么"**，怎么改状态是控制器的事。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarAction {
    /// 切到第 n 支画笔。
    Pen(usize),
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Stamp,
    Eraser,
    /// 马赛克：拖一个矩形把那块画面打码。
    Mosaic,
    Undo,
    Redo,
    Clear,
    Save,
    /// ★ 打勾「完成」：截图编辑态**确认这次截图**，落默认位置、**不弹框**。
    ///
    /// 为什么要有它（2026-10-03 用户口径："截图，如果进入编辑状态。底部是不是要有
    /// 打勾选项？表示OK了？现在只有退出选项。"）：编辑态末尾本来只有两颗 ——
    /// 「保存」是**另存为**（弹原生框），「退出」是**取消整次截图**（丢弃）。用户画完
    /// 想直接说"就这张"，两条都得挑一个不想要的语义，等于没有"OK"。
    ///
    /// ★ 只在**截图编辑态**出现（[`Self::visible_in`]）：屏幕标注 / 白板的「退出」
    ///   本身就是"留档到默认位置后退出"（用户 2026-09-23 口径），那一档已经有 OK 语义了，
    ///   再加一颗同功能的按钮只会让人以为两者有区别。
    Done,
    Exit,
}

/// 分组。组间画一条细竖线，组内按钮紧挨。
const GROUPS: [&[ToolbarAction]; 4] = [
    &[
        ToolbarAction::Pen(0),
        ToolbarAction::Pen(1),
        ToolbarAction::Pen(2),
    ],
    &[
        ToolbarAction::Line,
        ToolbarAction::Arrow,
        ToolbarAction::Rect,
        ToolbarAction::Ellipse,
    ],
    // ★ 组序 = [`crate::core::hotkeys::DRAW_TOOL_ORDER`]（唯一顺序来源）：快捷键列表、
    //   屏幕页「快捷键」子卡、设置页集合表都照这张表排（2026-10-01 用户口径）。
    //   橡皮擦排在马赛克**右边**——按 4 键切橡皮与工具栏上第四格是同一个位置。
    &[
        ToolbarAction::Text,
        ToolbarAction::Stamp,
        ToolbarAction::Mosaic,
        ToolbarAction::Eraser,
    ],
    &[
        ToolbarAction::Undo,
        ToolbarAction::Redo,
        ToolbarAction::Clear,
        ToolbarAction::Save,
        // ★ 挨着「保存」：「完成」与「保存」是同一件事的两种落点（默认位置 / 用户挑的
        //   位置），挨着放才看得出是这一族；「退出」是取消，单独隔开一点。
        ToolbarAction::Done,
        ToolbarAction::Exit,
    ],
];

impl ToolbarAction {
    /// 这一格在当前模式下**要不要出现**。
    ///
    /// 目前只有「完成」是有模式的（只在截图编辑态，见 [`ToolbarAction::Done`]）。
    /// 收成"每个动作自己答"而不是在 `layout` 里 `if action == Done`，是为了往后
    /// 再出现第二个模式相关按钮时不必回头改这段 —— 而且**判据跟着动作走**，
    /// 读 `layout` 的人不必知道有哪几个动作是特殊的。
    pub fn visible_in(self, shot_edit: bool) -> bool {
        match self {
            ToolbarAction::Done => shot_edit,
            _ => true,
        }
    }
}

/// 按钮总数（给数组定长用）。= 分组表里的动作总数，隐藏格不计入（见 [`Layout::cells`]）。
const BUTTONS: usize = 17;

/// 一次布局的结果。
pub struct Layout {
    /// 整条工具栏的外框。
    pub bar: Rect,
    /// 每个按钮的动作与矩形，**只含当前模式下可见的那些**。
    ///
    /// ★ 为什么是 `Vec` 而不是定长数组：隐藏的按钮不能占位 —— 占位会让"组间分隔线"
    ///   的下标算术（`cells[acc-1]` / `cells[acc]`）指向一根不存在的格子，线会画歪。
    ///   顺序恒定，末尾不追加空位。
    pub cells: Vec<(ToolbarAction, Rect)>,
    /// 各组**过滤后**的长度，与 [`Self::cells`] 同序（给组间分隔线定位用）。
    ///
    /// ★ 为什么不写成"分组表下标"再让绘制侧回头查 `GROPS`：隐藏格之后两者的下标不再
    ///   一一对应，拿未过滤的组长算边界就会把线画到按钮中间。直接把过滤后的长度交出去。
    pub groups: Vec<usize>,
}

/// 计算工具栏布局。
///
/// 每帧算一次（十几个矩形，可忽略），因而不持有状态——`hit` 与 `draw` 各自算各自的，
/// 不会出现"布局与绘制不同步"这类需要同步的隐患。
///
/// # `shot_edit`：哪些按钮出现
///
/// 截图编辑态与屏幕标注态共用同一条工具栏，但**不是同一组按钮**（「完成」只在编辑态有，
/// 见 [`ToolbarAction::Done`]）。判据在 [`ToolbarAction::visible_in`]，这里只负责照它
/// 过滤后排布 —— 隐藏的按钮不占宽、不占位。
///
/// # `anchor`：跟随选区（★ 2026-09-23）
///
/// 截图编辑态里工具栏**不能**再钉在屏幕底边居中：那块地方往往在选区之外，用户刚画完的
/// 那一笔在画面另一头，眼睛要来回跑。用户口径是"应该跟随截图区域，显示在附近"。
///
/// 传 `Some(选区世界矩形)` 时的规矩：
/// - **水平**与选区**中心对齐**（不是左右边对齐）：选区很窄时工具条会伸出去，对齐中心
///   至少保证"它就在选区正下方"一眼看得出来；
/// - **垂直**优先放在选区**下方**（不与选区重叠），下方放不下就放**上方**，
///   上下都放不下（选区几乎占满屏幕）才退回屏幕底边；
/// - 无论怎么放都钳进屏幕，且与屏幕边保留 [`EDGE_KEEP`]。
pub fn layout(screen: ScreenRect, scale: f32, anchor: Option<Rect>, shot_edit: bool) -> Layout {
    let s = scale.max(0.01);
    let btn = BTN * s;
    let gap = GAP * s;
    let pad = PAD * s;

    // ★ 先按模式过滤再排布：宽度与格子都从同一份可见表算，不会出现"底板按 17 格算、
    //   实际只画 16 格"这种左右对不上的错位。
    let groups: Vec<Vec<ToolbarAction>> = GROUPS
        .iter()
        .map(|g| {
            g.iter()
                .copied()
                .filter(|a| a.visible_in(shot_edit))
                .collect()
        })
        .collect();

    let mut total = pad * 2.0;
    for (gi, g) in groups.iter().enumerate() {
        if gi > 0 {
            total += GROUP_GAP * s;
        }
        total += g.len() as f32 * btn + g.len().saturating_sub(1) as f32 * gap;
    }

    let h = btn + pad * 2.0;
    let (x0, y0) = bar_origin(screen, s, total, h, anchor);

    let mut cells: Vec<(ToolbarAction, Rect)> = Vec::with_capacity(BUTTONS);
    let mut cx = x0 + pad;
    for (gi, g) in groups.iter().enumerate() {
        if gi > 0 {
            cx += GROUP_GAP * s;
        }
        for a in g.iter() {
            cells.push((*a, Rect::new(cx, y0 + pad, btn, btn)));
            cx += btn + gap;
        }
        // 组内最后一个按钮后面不多留 gap（组间距由 GROUP_GAP 负责）
        cx -= gap;
    }

    Layout {
        bar: Rect::new(x0, y0, total, h),
        groups: groups.iter().map(|g| g.len()).collect(),
        cells,
    }
}

/// 工具栏左上角落点。
///
/// 拆出来单独一个函数，是因为"放在哪"与"里面怎么排"是两件独立的事：前者只跟屏幕与
/// 锚点有关，后者只跟按钮表有关。混在一个函数里的时候，锚点那几条分支会埋在
/// 一百多行按钮排版中间，改一处得先把另一处读完。
fn bar_origin(screen: ScreenRect, s: f32, total: f32, h: f32, anchor: Option<Rect>) -> (f32, f32) {
    let (sx, sy) = (screen.x as f32, screen.y as f32);
    let (sw, sh) = (screen.w as f32, screen.h as f32);
    let bottom = sy + sh - BOTTOM_MARGIN * s - h;

    let centered = |cx: f32| clamp_bar_x(screen, s, total, cx - total / 2.0);
    let Some(a) = anchor else {
        return (centered(sx + sw / 2.0), bottom);
    };

    let keep = EDGE_KEEP * s;
    let fits = |y: f32| y >= sy + keep && y + h <= sy + sh - keep;
    let below = a.bottom() + ANCHOR_GAP * s;
    let above = a.y - ANCHOR_GAP * s - h;
    let y = if fits(below) {
        below
    } else if fits(above) {
        above
    } else {
        // 上下都放不下（选区几乎铺满屏幕）：退回原来的屏幕底边位置，至少是个能点的落点。
        bottom
    };
    (centered(a.center().x), y)
}

/// 把工具条水平钳进屏幕（`ScreenRect` 原点可为负，所以两边都要按屏幕坐标算）。
fn clamp_bar_x(screen: ScreenRect, s: f32, total: f32, x: f32) -> f32 {
    let lo = screen.x as f32 + PAD * s;
    let hi = (screen.x as f32 + screen.w as f32 - total - PAD * s).max(lo);
    x.clamp(lo, hi)
}

/// 命中测试。`p` 是世界坐标（与鼠标事件同一坐标系）。`anchor` 必须与 [`draw`] 同一取值，
/// 否则"画在这里、点在那里"——两者都各自算一次布局，判据不一致时只在锚点态下复现。
pub fn hit(
    p: Point,
    screen: ScreenRect,
    scale: f32,
    anchor: Option<Rect>,
    shot_edit: bool,
) -> Option<ToolbarAction> {
    let l = layout(screen, scale, anchor, shot_edit);
    l.cells.iter().find(|(_, r)| r.contains(p)).map(|(a, _)| *a)
}

/// 绘制整条工具栏。`active` 高亮当前工具，`hover` 高亮鼠标所在按钮。
///
/// `anchor` 见 [`layout`]：截图编辑态传选区矩形让工具条跟随它，其余场合传 `None`
/// （屏幕底边居中）。**必须与 [`hit`] 传同一个值**。
#[allow(clippy::too_many_arguments)]
pub fn draw(
    list: &mut DrawList,
    screen: ScreenRect,
    scale: f32,
    anchor: Option<Rect>,
    shot_edit: bool,
    active: ToolbarAction,
    hover: Option<ToolbarAction>,
    pens: &[Pen],
    hints: Option<&ToolbarHints>,
) {
    let s = scale.max(0.01);
    let l = layout(screen, s, anchor, shot_edit);

    // 底板：先铺一层边框色的大圆角矩形，再铺内层浅色，得到 1px 描边（DrawList 没有
    // 「圆角描边」这个图元）。
    list.fill_round_rect(l.bar.inset(-s, -s), (CORNER + 1.0) * s, BAR_BORDER);
    list.fill_round_rect(l.bar, CORNER * s, BAR_BG);

    // 组间分隔线：组边界下标 = 各组**过滤后**长度的前缀和（最后一组的末尾不算边界）。
    // ★ 用 `l.groups` 而不是 `GROPS`：有按钮被模式隐藏时两者下标不再一一对应，
    //   拿未过滤的组长算边界会把线画到按钮中间。
    let mut acc = 0usize;
    for g in l.groups.iter() {
        acc += g;
        if acc >= l.cells.len() {
            continue;
        }
        let left = l.cells[acc - 1].1;
        let right = l.cells[acc].1;
        let x = (left.right() + right.x) / 2.0;
        list.draw_line(
            Point::new(x, l.bar.y + 9.0 * s),
            Point::new(x, l.bar.bottom() - 9.0 * s),
            1.0 * s,
            BAR_BORDER,
        );
    }

    // 按钮底色：先 hover、后 active（active 覆盖 hover）
    if let Some(h) = hover {
        if h != active {
            if let Some((_, r)) = l.cells.iter().find(|(a, _)| *a == h) {
                list.fill_round_rect(*r, CORNER_BTN * s, HOVER_BG);
            }
        }
    }
    if let Some((_, r)) = l.cells.iter().find(|(a, _)| *a == active) {
        list.fill_round_rect(*r, CORNER_BTN * s, ACTIVE_BG);
    }

    for (action, r) in l.cells.iter() {
        let fg = if *action == active {
            // 选中态图标用强调蓝更清楚，但强调蓝在浅底上对比足够，直接用主题色的深一档
            Argb(0xFF1F_5FA5)
        } else {
            ICON_FG
        };
        draw_icon(list, *action, *r, s, fg, pens);
    }

    // ── 悬停气泡：工具名 + 快捷键（2026-09-29 用户建议）──
    //
    // 图标是纯几何自绘的，17px 下"这颗到底是哪把工具"全靠认形状；悬停给一行字，
    // 顺带把快捷键也告诉用户 —— 工具栏本来就没有文字标签，这是唯一补信息的地方。
    if let (Some(hv), Some(hints)) = (hover, hints) {
        if let Some((name, key)) = hints.get(hv) {
            if let Some((_, r)) = l.cells.iter().find(|(a, _)| *a == hv) {
                draw_tip(list, &l.bar, *r, s, name, key);
            }
        }
    }
}

/// 悬停气泡：贴在按钮**正上方**。
///
/// 为什么往上冒：工具条默认贴着屏幕底边（`BOTTOM_MARGIN` 只有 30 逻辑像素），往下
/// 会出屏；往上则只在"工具条被推到屏幕顶部"时才可能压到边，而那时它本来就已经在
/// 边缘了。水平方向钳进工具条范围内，两端的按钮不会冒到条外。
fn draw_tip(list: &mut DrawList, bar: &Rect, r: Rect, s: f32, name: &str, key: &str) {
    let text = if key.is_empty() {
        name.to_string()
    } else {
        format!("{name}  {key}")
    };
    let size = TIP_FONT * s;
    let w = crate::core::text_metrics::estimate_width(&text, size) + TIP_PAD_X * 2.0 * s;
    let h = crate::core::text_metrics::line_height(size) + 6.0 * s;
    let x = (r.center().x - w / 2.0).clamp(bar.x, (bar.right() - w).max(bar.x));
    let box_r = Rect::new(x, r.y - TIP_GAP * s - h, w, h);
    list.fill_round_rect(box_r, 5.0 * s, TIP_BG);
    list.text_centered(box_r.center(), &text, size, TIP_FG, None);
}

/// 画一个按钮的图标。
fn draw_icon(list: &mut DrawList, action: ToolbarAction, r: Rect, s: f32, fg: Argb, pens: &[Pen]) {
    let c = r.center();
    let h = ICON_H * s;
    let stroke = 1.6 * s;

    match action {
        ToolbarAction::Pen(i) => {
            // 图标直接画这支笔的**真实颜色与粗细**：点之前就知道这一支长什么样。
            let (color, width) = match pens.get(i) {
                Some(p) => (p.color, p.width as f32),
                None => (Argb(0xFF00_0000), 4.0),
            };
            let radius = (width * s * 0.18).clamp(2.0 * s, 6.0 * s);
            list.fill_circle(c, radius, color);
            // 细笔点太小会看不清，补一圈同色描边（不影响观感，只把直径撑到可见）
            if radius < 3.2 * s {
                list.stroke_circle(c, 3.2 * s, 1.0 * s, color);
            }
        }
        ToolbarAction::Line => {
            list.draw_line(
                Point::new(c.x - h, c.y + h),
                Point::new(c.x + h, c.y - h),
                stroke,
                fg,
            );
        }
        ToolbarAction::Arrow => {
            let (x0, y0) = (c.x - h, c.y + h);
            let (x1, y1) = (c.x + h, c.y - h);
            list.draw_line(Point::new(x0, y0), Point::new(x1, y1), stroke, fg);
            // 方向恒为 45° 右上：单位向量与其法向都是 1/√2，直接用常量，省一次开方。
            const D: f32 = std::f32::consts::FRAC_1_SQRT_2;
            let (ux, uy) = (D, -D);
            let (nx, ny) = (D, D);
            let (hl, hw) = (h * 0.85, h * 0.42);
            list.fill_polygon(
                &[
                    Point::new(x1, y1),
                    Point::new(x1 - ux * hl + nx * hw, y1 - uy * hl + ny * hw),
                    Point::new(x1 - ux * hl - nx * hw, y1 - uy * hl - ny * hw),
                ],
                fg,
            );
        }
        ToolbarAction::Rect => {
            list.stroke_rect(
                Rect::new(c.x - h, c.y - h * 0.8, h * 2.0, h * 1.6),
                stroke,
                fg,
            );
        }
        ToolbarAction::Ellipse => {
            list.stroke_ellipse(
                Rect::new(c.x - h, c.y - h * 0.8, h * 2.0, h * 1.6),
                stroke,
                fg,
            );
        }
        ToolbarAction::Text => {
            // 一个 "T"：顶横 + 中竖
            list.draw_line(
                Point::new(c.x - h, c.y - h * 0.8),
                Point::new(c.x + h, c.y - h * 0.8),
                stroke,
                fg,
            );
            list.draw_line(
                Point::new(c.x, c.y - h * 0.8),
                Point::new(c.x, c.y + h * 0.85),
                stroke,
                fg,
            );
        }
        ToolbarAction::Stamp => {
            // ★ 实心圆 + 反色居中数字：与新版序号贴纸的实际绘制严格一致
            //    （此前是空心圆 + 从圆心起排的"1"——文字偏右、看起来不在圆里）。
            //    居中与字号都**复用贴纸那两条规则**（`shapes::stamp_label_size` +
            //    `DrawCmd::TextCentered`），避免图标与实物各写一份而悄悄漂移。
            //    反色用底板浅色 —— 深图标上的浅字对比最大。
            let radius = h * 0.9;
            list.fill_circle(c, radius, fg);
            let size = shapes::stamp_label_size(radius, 1);
            list.text_centered(c, "1", size, BAR_BG, None);
        }
        ToolbarAction::Eraser => {
            // ★ 橡皮（2026-09-29 用户："图标很丑，根本不像正常的橡皮擦"）。
            //   旧版是 45° 的**实心八边形**，17px 里看着就是一个菱形块，认不出是什么。
            //   改成真实橡皮的侧面：约 -32° 斜置的圆角厚块，**下半（橡皮本体）浅灰 +
            //   上半（套壳）深色**，中间留一道底色缝——"双色分截"是橡皮最强的识别
            //   特征，比任何轮廓都管用（铅笔、记号笔都不会这么分截）。
            let ang = -32.0_f32.to_radians();
            let (ux, uy) = (ang.cos(), ang.sin()); // 长轴：左下 → 右上
            let (nx, ny) = (-ang.sin(), ang.cos()); // 法向（长轴左转 90°）
            let l = 1.95 * h;
            let wd = 1.02 * h;
            let b = 0.28 * wd; // 倒角（伪圆角）
            let (ax, ay) = (c.x - ux * l * 0.5, c.y - uy * l * 0.5);
            let pt = |du: f32, dn: f32| Point::new(ax + ux * du + nx * dn, ay + uy * du + ny * dn);
            let (hw, hb) = (wd * 0.5, wd * 0.5 - b);
            let cut = l * 0.54; // 分界：左下这一截 = 橡皮本体
                                // ① 整块打底（浅灰 = 橡皮本体）
            let corners = [
                pt(0.0, hb),
                pt(b, hw),
                pt(l - b, hw),
                pt(l, hb),
                pt(l, -hb),
                pt(l - b, -hw),
                pt(b, -hw),
                pt(0.0, -hb),
            ];
            list.fill_polygon(&corners, ERASER_RUBBER);
            // ② 套壳：右上那一截压深色
            list.fill_polygon(
                &[
                    pt(cut, hw),
                    pt(l - b, hw),
                    pt(l, hb),
                    pt(l, -hb),
                    pt(l - b, -hw),
                    pt(cut, -hw),
                ],
                fg,
            );
            // ③ 分界缝：两色直接相接在软渲染下会有毛边，留一道底色缝更干净，
            //    也更像真橡皮"套壳与橡皮之间那道槽"。
            list.draw_line(pt(cut, hw), pt(cut, -hw), 1.2 * s, BAR_BG);
            // ④ 外描边：浅灰本体在近白的工具条上边界发虚，描一道才立得住。
            let ring = [
                corners[0], corners[1], corners[2], corners[3], corners[4], corners[5], corners[6],
                corners[7], corners[0],
            ];
            list.draw_polyline_ref(&ring, 1.2 * s, fg);
        }
        ToolbarAction::Mosaic => {
            // 2×2 小方格（打码块的象形）。格子留一道缝，一眼读出"一块一块的"。
            // ★ 外包络 = 2g+gap，起点必须按**外包络**居中——旧版按 `off = g+gap` 的一半
            //   定起点，整体往右下偏了 gap/2（用户报"图标偏右下"）。
            let g = h * 0.62; // 单格边长
            let gap = h * 0.16;
            let total = 2.0 * g + gap;
            let x0 = c.x - total / 2.0;
            let y0 = c.y - total / 2.0;
            let off = g + gap;
            for (i, j) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                list.fill_rect(Rect::new(x0 + i * off, y0 + j * off, g, g), fg);
            }
        }
        ToolbarAction::Undo => {
            // 上半圆弧 + 左端朝下的箭头。弧用折线采样——DrawList 里"曲线"就是折线。
            let radius = h * 0.88;
            let cy = c.y + h * 0.22;
            let mut arc = Vec::with_capacity(13);
            for k in 0..=12 {
                let a = std::f32::consts::PI + (k as f32 / 12.0) * std::f32::consts::PI;
                arc.push(Point::new(c.x + radius * a.cos(), cy + radius * a.sin()));
            }
            list.draw_polyline(arc, stroke, fg);
            list.fill_polygon(
                &[
                    Point::new(c.x - radius, cy + h * 0.5),
                    Point::new(c.x - radius - h * 0.42, cy + h * 0.02),
                    Point::new(c.x - radius + h * 0.42, cy + h * 0.02),
                ],
                fg,
            );
        }
        ToolbarAction::Redo => {
            // 重做 = 撤销的镜像（2026-09-22）：弧的 x 取反 + 箭头朝右。
            let radius = h * 0.88;
            let cy = c.y + h * 0.22;
            let mut arc = Vec::with_capacity(13);
            for k in 0..=12 {
                let a = std::f32::consts::PI + (k as f32 / 12.0) * std::f32::consts::PI;
                arc.push(Point::new(c.x - radius * a.cos(), cy + radius * a.sin()));
            }
            list.draw_polyline(arc, stroke, fg);
            list.fill_polygon(
                &[
                    Point::new(c.x + radius, cy + h * 0.5),
                    Point::new(c.x + radius - h * 0.42, cy + h * 0.02),
                    Point::new(c.x + radius + h * 0.42, cy + h * 0.02),
                ],
                fg,
            );
        }
        ToolbarAction::Clear => {
            // 垃圾桶：盖 + 提手 + 桶身
            list.draw_line(
                Point::new(c.x - h * 0.9, c.y - h * 0.5),
                Point::new(c.x + h * 0.9, c.y - h * 0.5),
                stroke,
                fg,
            );
            list.stroke_rect(
                Rect::new(c.x - h * 0.32, c.y - h * 0.85, h * 0.64, h * 0.35),
                1.3 * s,
                fg,
            );
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.72, c.y - h * 0.3),
                    Point::new(c.x - h * 0.55, c.y + h * 0.9),
                    Point::new(c.x + h * 0.55, c.y + h * 0.9),
                    Point::new(c.x + h * 0.72, c.y - h * 0.3),
                ],
                stroke,
                fg,
            );
        }
        ToolbarAction::Save => {
            // 保存（导出 PNG）：竖线 + 下载箭头 + 底托盘（2026-09-22）。
            list.draw_line(
                Point::new(c.x, c.y - h * 0.8),
                Point::new(c.x, c.y + h * 0.3),
                stroke,
                fg,
            );
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.55, c.y - h * 0.1),
                    Point::new(c.x, c.y + h * 0.38),
                    Point::new(c.x + h * 0.55, c.y - h * 0.1),
                ],
                stroke,
                fg,
            );
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.75, c.y + h * 0.62),
                    Point::new(c.x - h * 0.75, c.y + h * 0.92),
                    Point::new(c.x + h * 0.75, c.y + h * 0.92),
                    Point::new(c.x + h * 0.75, c.y + h * 0.62),
                ],
                stroke,
                fg,
            );
        }
        ToolbarAction::Done => {
            // 打勾（2026-10-03 用户口径："底部是不是要有打勾选项？表示OK了？"）。
            // 两段折线：短划 + 长勾。刻意不画外框 —— 末组里「保存」「退出」都是带外框的
            // 器物形，打勾若也套个框就成了第四个"按钮容器"，反而看不出它是"确认"这一类。
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.95, c.y + h * 0.02),
                    Point::new(c.x - h * 0.32, c.y + h * 0.62),
                ],
                stroke,
                fg,
            );
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.32, c.y + h * 0.62),
                    Point::new(c.x + h * 0.98, c.y - h * 0.72),
                ],
                stroke,
                fg,
            );
        }
        ToolbarAction::Exit => {
            // 一个开口朝右的门 + 一支穿出去的箭头
            list.draw_polyline(
                vec![
                    Point::new(c.x - h * 0.2, c.y - h * 0.85),
                    Point::new(c.x - h * 0.9, c.y - h * 0.85),
                    Point::new(c.x - h * 0.9, c.y + h * 0.85),
                    Point::new(c.x - h * 0.2, c.y + h * 0.85),
                ],
                stroke,
                fg,
            );
            list.draw_line(
                Point::new(c.x - h * 0.15, c.y),
                Point::new(c.x + h * 0.95, c.y),
                stroke,
                fg,
            );
            list.fill_polygon(
                &[
                    Point::new(c.x + h, c.y),
                    Point::new(c.x + h * 0.42, c.y - h * 0.34),
                    Point::new(c.x + h * 0.42, c.y + h * 0.34),
                ],
                fg,
            );
        }
    }
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

    #[test]
    fn layout_covers_every_button_and_stays_on_screen() {
        let l = layout(screen(), 1.0, None, true);
        // 每个按钮都在底板内（不越界）
        for (a, r) in l.cells.iter() {
            assert!(
                r.x >= l.bar.x && r.right() <= l.bar.right() + 0.01,
                "{a:?} 越出底板：{r:?} / {:?}",
                l.bar
            );
            assert!(r.y >= l.bar.y && r.bottom() <= l.bar.bottom() + 0.01);
        }
        assert!(l.bar.right() <= 1920.0, "不得超出屏幕右边界");
        assert!(l.bar.bottom() <= 1080.0, "不得超出屏幕下边界");
    }

    #[test]
    fn layout_is_horizontal_centered() {
        let l = layout(screen(), 1.0, None, true);
        let center = l.bar.x + l.bar.w / 2.0;
        assert!((center - 960.0).abs() < 1.0, "应水平居中，实际 {center}");
    }

    #[test]
    fn layout_follows_the_virtual_desktop_origin() {
        // 副屏在左：虚拟桌面原点 x = -1920、宽 3840 → 中心是 0。
        let l = layout(
            ScreenRect {
                x: -1920,
                y: 0,
                w: 3840,
                h: 1080,
            },
            1.0,
            None,
            true,
        );
        let center = l.bar.x + l.bar.w / 2.0;
        assert!(
            (center - 0.0).abs() < 1.0,
            "应居中于虚拟桌面，实际 {center}"
        );
    }

    #[test]
    fn scale_grows_the_bar_proportionally() {
        let a = layout(screen(), 1.0, None, true);
        let b = layout(screen(), 1.5, None, true);
        assert!((b.bar.w / a.bar.w - 1.5).abs() < 0.01);
        assert!((b.bar.h / a.bar.h - 1.5).abs() < 0.01);
    }

    #[test]
    fn every_button_is_hit_testable_and_they_do_not_overlap() {
        let sc = screen();
        for (a, r) in layout(sc, 1.0, None, true).cells.iter() {
            let p = r.center();
            assert_eq!(
                hit(p, sc, 1.0, None, true),
                Some(*a),
                "{a:?} 的中心点应命中自己"
            );
        }
        // 两两不重叠
        let cells = layout(sc, 1.0, None, true).cells;
        for i in 0..cells.len() {
            for j in (i + 1)..cells.len() {
                assert!(
                    !cells[i].1.intersects(cells[j].1),
                    "{:?} 与 {:?} 重叠",
                    cells[i].0,
                    cells[j].0
                );
            }
        }
    }

    #[test]
    fn empty_space_misses() {
        let sc = screen();
        assert_eq!(hit(Point::new(5.0, 5.0), sc, 1.0, None, true), None);
        // 工具栏正上方一点（底边距内）不该命中
        let bar = layout(sc, 1.0, None, true).bar;
        assert_eq!(
            hit(Point::new(bar.x + 2.0, bar.y - 6.0), sc, 1.0, None, true),
            None
        );
    }

    #[test]
    fn all_actions_are_covered_exactly_once() {
        let cells = layout(screen(), 1.0, None, true).cells;
        let mut seen = Vec::new();
        for (a, _) in cells.iter() {
            assert!(!seen.contains(a), "{a:?} 出现了两次");
            seen.push(*a);
        }
        // 分组表里的动作总数必须等于 BUTTONS，否则数组尾巴上会留下 Pen(0) 占位
        let declared: usize = GROUPS.iter().map(|g| g.len()).sum();
        assert_eq!(declared, BUTTONS, "分组表长度与 BUTTONS 不一致");
    }

    /// ★ 工具栏按钮序（前三组，也就是**有快捷键的那批**）必须与
    /// `core::hotkeys::DRAW_TOOL_ORDER` 逐项同序（2026-10-01 用户口径："快捷键顺序要和
    /// 工具栏一样"）。
    ///
    /// 三处同序（工具栏 / 屏幕页「快捷键」子卡 / 设置页集合表）里的正本是 core 那张表：
    /// 按钮表这一头靠本测试钉住，另两头由 `ui::pages::global` 里
    /// `hotkey_collection_follows_the_ui_order_and_reuses_ui_text` 钉住。少了这一条，
    /// "橡皮擦挪到马赛克右边"这种改动只能靠肉眼在两处一致的手抄里对齐。
    #[test]
    fn button_order_matches_the_hotkey_order() {
        let order = crate::core::hotkeys::DRAW_TOOL_ORDER;
        let flat: Vec<ToolbarAction> = GROUPS[..3].iter().flat_map(|g| g.iter().copied()).collect();
        assert_eq!(
            flat.len(),
            order.len(),
            "工具栏工具按钮数与快捷键工具序不一致"
        );
        for (i, (got, want)) in flat.iter().zip(order.iter()).enumerate() {
            assert_eq!(
                *got,
                tool_action(*want).unwrap_or_else(|| panic!("{want:?} 在工具栏上没有对应按钮")),
                "第 {i} 个工具按钮/快捷键错位：{got:?} ≠ {want:?}"
            );
        }
    }

    #[test]
    fn draw_touches_every_command_leaf() {
        // 只验证"画得出来"而不 panic，且至少产出了指令（防"图标全空"这种假通过）
        let pens = super::super::pens::default_pens();
        for a in [
            ToolbarAction::Pen(0),
            ToolbarAction::Line,
            ToolbarAction::Arrow,
            ToolbarAction::Rect,
            ToolbarAction::Ellipse,
            ToolbarAction::Text,
            ToolbarAction::Stamp,
            ToolbarAction::Eraser,
            ToolbarAction::Undo,
            ToolbarAction::Redo,
            ToolbarAction::Clear,
            ToolbarAction::Save,
            // ★ 新按钮必须进这份清单：`every_action_draws_something` 的价值就在于
            //   "加了一个动作但忘了画图标"会红。漏了它就是白加一颗空按钮。
            ToolbarAction::Done,
            ToolbarAction::Exit,
        ] {
            let mut list = DrawList::new();
            draw(
                &mut list,
                screen(),
                1.0,
                None,
                true,
                a,
                Some(a),
                &pens,
                None,
            );
            assert!(list.cmds().len() > 3, "{a:?} 应当画出东西");
        }
    }

    /// 悬停气泡：**只在悬停时**出现，且带上工具名与快捷键。
    #[test]
    fn hover_tip_shows_name_and_hotkey_only_on_hover() {
        let pens = super::super::pens::default_pens();
        let hk = crate::core::config::DrawingHotkeys::default();
        let hints = hints(&hk);
        let eraser_key = hk.draw_hk_eraser.clone();

        // ① 没悬停 → 一个气泡都不能有
        let mut plain = DrawList::new();
        draw(
            &mut plain,
            screen(),
            1.0,
            None,
            true,
            ToolbarAction::Pen(0),
            None,
            &pens,
            Some(&hints),
        );
        // （`TextCentered` 不唯一——序号贴纸的图标里也有一个居中的 "1"，所以按
        //  "含不含工具名"认气泡，而不是"有没有居中文字"。）
        let tip_of = |l: &DrawList, name: &str| -> Option<String> {
            l.cmds().iter().find_map(|c| match c {
                crate::core::draw::DrawCmd::TextCentered { text, .. } if text.contains(name) => {
                    Some(text.clone())
                }
                _ => None,
            })
        };
        assert!(tip_of(&plain, "橡皮擦").is_none(), "没悬停时不该有气泡");

        // ② 悬停橡皮擦 → 文案里既有"橡皮擦"也有配置的快捷键
        let mut hov = DrawList::new();
        draw(
            &mut hov,
            screen(),
            1.0,
            None,
            true,
            ToolbarAction::Pen(0),
            Some(ToolbarAction::Eraser),
            &pens,
            Some(&hints),
        );
        let tip = tip_of(&hov, "橡皮擦").expect("悬停时应画出气泡");
        assert!(tip.contains("橡皮擦"), "气泡应含工具名，实际：{tip}");
        assert!(
            tip.contains(&eraser_key),
            "气泡应含快捷键 {eraser_key}，实际：{tip}"
        );

        // ③ 不给提示表（None）就不画气泡 —— 老调用点不受影响
        let mut nohints = DrawList::new();
        draw(
            &mut nohints,
            screen(),
            1.0,
            None,
            true,
            ToolbarAction::Pen(0),
            Some(ToolbarAction::Eraser),
            &pens,
            None,
        );
        assert!(
            tip_of(&nohints, "橡皮擦").is_none(),
            "没给提示表时不该有气泡"
        );
    }

    /// 选区（锚点）在屏幕**中间**：工具条落在选区**下方**、水平与选区中心对齐。
    #[test]
    fn anchored_bar_sits_below_the_selection() {
        let sc = screen();
        let sel = Rect::new(600.0, 300.0, 400.0, 250.0);
        let l = layout(sc, 1.0, Some(sel), true);
        assert!(
            l.bar.y >= sel.bottom(),
            "应落在选区下方（不压住刚截的那块），实际 y={} vs 选区底 {}",
            l.bar.y,
            sel.bottom()
        );
        assert!(
            (l.bar.y - (sel.bottom() + ANCHOR_GAP)).abs() < 0.01,
            "贴着选区底边留一个 ANCHOR_GAP"
        );
        let (bc, sc_c) = (l.bar.x + l.bar.w / 2.0, sel.x + sel.w / 2.0);
        assert!((bc - sc_c).abs() < 0.01, "水平应与选区中心对齐");
    }

    /// ★ 选区**贴着屏幕底部**时必须翻到**上方**——这是"跟随选区"最容易漏的一条：
    /// 只判断"放下方"的实现会把它推出屏幕外，用户看见的是一条**画在屏幕外面**的工具栏。
    #[test]
    fn anchored_bar_flips_above_when_there_is_no_room_below() {
        let sc = screen();
        let sel = Rect::new(200.0, 900.0, 300.0, 170.0); // 底边 1070，紧贴 1080
        let l = layout(sc, 1.0, Some(sel), true);
        assert!(
            l.bar.bottom() <= sel.y,
            "下方放不下应翻到选区上方，实际 bar={:?} 选区顶 {}",
            l.bar,
            sel.y
        );
        assert!(l.bar.bottom() <= 1080.0, "无论如何不得出屏");
        assert!((l.bar.bottom() - (sel.y - ANCHOR_GAP)).abs() < 0.01);
    }

    /// 选区几乎铺满整屏（上下都放不下）→ 退回屏幕底边的落点，绝不出屏。
    ///
    /// 这条是"兜底也要能点"：宁可压住选区一角，也不能把工具条画到屏幕外面去。
    #[test]
    fn anchored_bar_falls_back_inside_the_screen_when_the_selection_fills_it() {
        let sc = screen();
        let sel = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let l = layout(sc, 1.0, Some(sel), true);
        assert!(l.bar.x >= 0.0 && l.bar.right() <= 1920.0, "横不得出屏");
        assert!(l.bar.y >= 0.0 && l.bar.bottom() <= 1080.0, "纵不得出屏");
        // 与无锚点时的落点一致（同一个兜底分支）。
        assert!((l.bar.y - layout(sc, 1.0, None, true).bar.y).abs() < 0.01);
    }

    /// 选区在屏幕边角时，工具条**横向**也要被钳回来（不能半截在屏幕外）。
    #[test]
    fn anchored_bar_is_clamped_horizontally_at_the_screen_edge() {
        let sc = screen();
        let sel = Rect::new(0.0, 40.0, 60.0, 40.0); // 极窄、贴左
        let l = layout(sc, 1.0, Some(sel), true);
        assert!(l.bar.x >= 0.0, "左边不得出屏");
        assert!(l.bar.right() <= 1920.0);
        let sel = Rect::new(1900.0, 40.0, 20.0, 40.0); // 贴右
        let l = layout(sc, 1.0, Some(sel), true);
        assert!(l.bar.right() <= 1920.0, "右边不得出屏");
        assert!(l.bar.x >= 0.0);
    }

    /// `hit` 与 `draw` 必须用**同一个** anchor：传错值时"画在下面、点在底边"，
    /// 表现为工具条点不动。这条把两个入口钉在一起。
    #[test]
    fn hit_follows_the_same_anchor_as_draw() {
        let sc = screen();
        let sel = Rect::new(600.0, 300.0, 400.0, 250.0);
        let anchored = layout(sc, 1.0, Some(sel), true);
        // 落在锚点态下某个按钮的中心 → 必须命中那个按钮。
        let (a0, r0) = anchored.cells[0];
        assert_eq!(hit(r0.center(), sc, 1.0, Some(sel), true), Some(a0));
        // 同一个点在**无锚点**布局下不应命中（两套布局确实不同，说明 anchor 真被用上了）。
        assert_ne!(
            layout(sc, 1.0, Some(sel), true).bar,
            layout(sc, 1.0, None, true).bar,
            "锚点必须真的改变落点，否则这条判据没有分辨力"
        );
    }

    /// ★ 任务 2 的回归锁：打勾「完成」**只在截图编辑态**出现（2026-10-03 用户口径
    /// "底部是不是要有打勾选项？表示OK了？现在只有退出选项。"）。
    ///
    /// 钉住三件事，缺一件这个功能就还能悄悄坏掉：
    /// ① 编辑态有、标注态没有；
    /// ② 隐藏时**不留空位**（`cells` 里不能残留一颗零宽的 `Done`）——占位会让分隔线
    ///    的下标算术指向不存在的格子，把线画到按钮中间；
    /// ③ 隐藏时底板宽度要跟着缩（否则标注态会多出一截空白，右边看着像少了个按钮）。
    #[test]
    fn the_done_check_only_shows_up_in_shot_edit() {
        let sc = screen();
        let edit = layout(sc, 1.0, None, true);
        let annot = layout(sc, 1.0, None, false);

        // ① 两档的有无
        assert!(
            edit.cells.iter().any(|(a, _)| *a == ToolbarAction::Done),
            "截图编辑态必须有打勾「完成」"
        );
        assert!(
            !annot.cells.iter().any(|(a, _)| *a == ToolbarAction::Done),
            "标注 / 白板不该有「完成」——那一档的「退出」本身就是留档后退出（用户 2026-09-23 口径），\
             再加一颗同功能的只会让人以为两者有区别"
        );

        // ② 隐藏时零占位：两档的 cells 里都不能出现宽高为 0 的格子。
        for (mode, l) in [("编辑态", &edit), ("标注态", &annot)] {
            for (a, r) in l.cells.iter() {
                assert!(
                    r.w > 0.0 && r.h > 0.0,
                    "{mode} 留下了零占位格子 {a:?}（{r:?}）——分隔线的下标算术会指错"
                );
            }
        }

        // ③ 宽度差正好一颗按钮（隐藏的那颗不占宽）。
        let one_btn = BTN + GAP;
        assert!(
            (edit.bar.w - annot.bar.w - one_btn).abs() < 0.01,
            "两档宽度差应恰为一颗按钮（{one_btn}），实际差 {}",
            edit.bar.w - annot.bar.w
        );

        // ④ 两档的 `groups` 长度都必须与实际组数一致（分隔线靠它定位）。
        for (mode, l) in [("编辑态", &edit), ("标注态", &annot)] {
            assert_eq!(
                l.groups.len(),
                GROUPS.len(),
                "{mode} 的 groups 长度应等于分组表组数"
            );
            assert_eq!(
                l.groups.iter().sum::<usize>(),
                l.cells.len(),
                "{mode} 的各组长度之和必须等于实际排出的格子数"
            );
        }
    }

    /// 打勾「完成」与「保存」在编辑态**挨着**（同一族：都是"确认出图"，只差落点怎么定），
    /// 且都在「退出」（取消）之前。位置错了用户会以为按错按钮。
    #[test]
    fn done_sits_next_to_save_and_before_exit() {
        let cells = layout(screen(), 1.0, None, true).cells;
        let idx = |a: ToolbarAction| {
            cells
                .iter()
                .position(|(x, _)| *x == a)
                .unwrap_or_else(|| panic!("编辑态该有 {a:?}"))
        };
        let (save, done, exit) = (
            idx(ToolbarAction::Save),
            idx(ToolbarAction::Done),
            idx(ToolbarAction::Exit),
        );
        assert_eq!(done, save + 1, "「完成」应紧挨在「保存」右边");
        assert_eq!(exit, done + 1, "「退出」应在「完成」之后（取消排最后）");
    }

    /// ★ 打勾图标的**实际坐标**（2026-10-03）。
    ///
    /// 为什么坐标要单测钉住、而像素探针不够：`render_probe` 的形状判据（重心 + 跨度）
    /// 只在**整体**挪位或压扁时才变红 —— 2026-10-03 实测：故意把打勾的**第一段**
    /// （短划）挪到格子底边 `c.y + h*0.98`、第二段（长勾）保持不动，24 项判据**全过**
    /// （重心 55%、跨度 50%，都在阈值内）。而图形已经明显不对了。
    ///
    /// 坐标单测是精确判据：图形对就是这几个数，不靠任何阈值。
    #[test]
    fn the_done_tick_is_two_segments_going_up_to_the_right() {
        use crate::core::draw::{DrawCmd, Shape};
        let mut list = DrawList::new();
        let r = Rect::new(100.0, 200.0, 34.0, 34.0);
        draw_icon(
            &mut list,
            ToolbarAction::Done,
            r,
            1.0,
            ICON_FG,
            &super::super::pens::default_pens(),
        );
        // 取出那两条折线：短划两点 + 长勾两点。
        // 折线顶点存在 DrawList 的**共享顶点池**里（`Shape::Polyline { points: PointRange }`），
        // 取点要过 `DrawList::range`，不能直接 clone `points`。
        let polys: Vec<Vec<Point>> = list
            .cmds()
            .iter()
            .filter_map(|c| match c {
                DrawCmd::Stroke {
                    shape: Shape::Polyline { points, .. },
                    ..
                } => Some(list.range(*points).to_vec()),
                _ => None,
            })
            .collect();
        assert_eq!(
            polys.len(),
            2,
            "打勾应是两段折线（短划 + 长勾），实测 {}",
            polys.len()
        );

        let c = r.center();
        let h = ICON_H; // `draw_icon` 里的 h = ICON_H * scale（scale=1）
        let near = |p: Point, x: f32, y: f32| {
            (p.x - (c.x + x)).abs() < 0.01 && (p.y - (c.y + y)).abs() < 0.01
        };
        // ① 短划：从左下 (-0.95h, +0.02h) 到折点 (-0.32h, +0.62h) —— **向右下**。
        assert!(
            near(polys[0][0], -h * 0.95, h * 0.02) && near(polys[0][1], -h * 0.32, h * 0.62),
            "短划应从左下到折点，实测 {:?} → {:?}（格心 {c:?}，h={h}）",
            polys[0][0],
            polys[0][1]
        );
        // ② 长勾：从折点 (-0.32h, +0.62h) 到右上 (+0.98h, -0.72h) —— **向右上翘**。
        assert!(
            polys[1].len() == 2
                && near(polys[1][0], -h * 0.32, h * 0.62)
                && near(polys[1][1], h * 0.98, -h * 0.72),
            "长勾应从折点翘到右上，实测 {:?} → {:?}",
            polys[1].first(),
            polys[1].last()
        );
        // ③ 折点重合：两段必须接在同一点，否则那不是"勾"是两个孤立笔画。
        assert!(
            (polys[0][1].x - polys[1][0].x).abs() < 0.01
                && (polys[0][1].y - polys[1][0].y).abs() < 0.01,
            "两段折线必须接在同一个折点，实测 {:?} vs {:?}",
            polys[0][1],
            polys[1][0]
        );
    }
}
