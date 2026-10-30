//! 功能卡片组件。
//!
//! 标题行是「[开关] 名称 ── [下拉] [附加控件] ……… ▼」，正文在展开后显示。
//!
//! 开关是可选的：有对应功能开关的卡片（拖尾、击键回显…）传 `switch(信号)`；纯设置类
//! 卡片（快捷键、语言）不传——**不留假开关**。无开关的卡片仍占住开关那一格，保证各卡片
//! 标题左对齐。
//!
//! 折叠也是可选的：`.always_open()` 得到**常开卡**（无箭头、名称不可点、正文恒显示）——
//! 目前**没有调用方**（原本用于「点击效果」「按键声音」两张纯分组父卡，2026-09-22 起
//! 按用户口径改成可折叠了），保留为组件能力，理由同 [`FeatureCard::title_extra`]。

use std::path::Path;

use windui::core::EventCtx;
use windui::prelude::*;

use super::tokens;
use super::tokens::to_color;

/// 开关列的固定宽度（有复选框时是复选框，没有时是等宽空占位）。
const SWITCH_W: i32 = 24;

/// 复选框方框边长（= windui `inputs.rs` 的 `BOX_SIZE`，Normal 变体）。
///
/// 必须显式钉成元素高度：windui 的复选框元素高度是 `max(方框, 标签行高)`，标题行用的是
/// 空标签 `checkbox("", ..)`——行高照样算进去（实测 19），于是元素比方框高。方框在元素内
/// 居中走的是 `inputs.rs:163` 的 `(h - 18) / 2` 整数除法，19 会给成 0 而不是 0.5。
const SWITCH_BOX: i32 = 18;

/// 标题条高度。
///
/// **`col1` 必须显式撑到这条的高度**，否则复选框永远差 1px。纵向位置由两层整数除法叠加决定：
///
/// 1. `col1` 在条内居中：`(条高 - col1 交叉轴尺寸) / 2`；
/// 2. 复选框在 `col1` 内居中：`(col1 交叉轴尺寸 - 18) / 2`。
///
/// `col1` 交叉轴尺寸取的是子元素最大高 = 名称行高（实测 19，奇数），于是第 1 步得
/// `(36-19)/2 = 8.5 → 8`、第 2 步得 `(19-18)/2 = 0.5 → 0`，两次都向下截，方框比真正
/// 的中心高 1px（条高 36：理想上下各 9，实得 8 / 10，方框落在 rel 8..25）。
///
/// 让 `col1` 直接等于条高，两层除法就都落在 36 这个偶数上：`(36-18)/2 = 9` 精确，
/// 方框落在 rel 9..26，上下留白严格相等。名称文字的位置**不变**——它原本就在
/// `col1` 顶部偏移 0 处，即 8；`col1` 撑满后按 `(36-19)/2 = 8` 居中，仍是 8。
const HEADER_H: i32 = 36;

/// 数值滑条的原生测量宽（逻辑 px）：windui `Slider::measure` 固定返回 `120 + 值标签宽`。
/// 与 [`slider_row`] 搭配时不带值标签（数值只在输入框一处），恒为 120。
const SLIDER_NATIVE_W: i32 = 120;

/// [`slider_row`] 里滑条与数值输入框之间的间隔。
const SLIDER_INPUT_GAP: i32 = 8;

/// 数值输入框宽度（[`slider_row`] 与"控件组总宽"共用，单一来源）。
pub const INPUT_W: i32 = 72;

/// ★ 标准数值控件组的总宽 = 滑条 + 间隔 + 输入框 = 200。
///
/// 两个消费者**必须同源**：`slider_row` 的正文控件组（右缘 = 数值输入框的
/// 右缘）与 `build_title_row` 的标题栏下拉（宽度取此值 → 右缘恰好对齐输入框右缘，
/// 2026-09-25 "统一 UI" 第三定案）。改任何成员宽度都从这里整体重算，
/// 别在调用点写 200 的字面量。
pub const CONTROL_GROUP_W: i32 = SLIDER_NATIVE_W + SLIDER_INPUT_GAP + INPUT_W;

/// 标题下拉里**文本区**的可用宽度（逻辑 px）。
///
/// = `CONTROL_GROUP_W` − 左右内边距 24 − 右侧 chevron 18（= 158 ≈ 11 个汉字）。
/// 凡是要塞进下拉的显示名（效果名、声音名）都按这个预算截断，否则会在按钮里折成
/// 两行、文字溢出行高。内置效果名 4 字、脚本名带「（脚本）」后缀 8 字都放得下；
/// 再长的用户脚本名由 `ui::effects` 按同一预算截断，见 `MAX_DISPLAY_CHARS`。
pub const DROPDOWN_TEXT_W: i32 = CONTROL_GROUP_W - 24 - 18;

/// 参数行标签列的最小宽度（逻辑 px）。
///
/// 标签列宽取**该卡片的 `col1_width`**（见 [`option_row`]），但不得窄于这个值——
/// 否则「显示/隐藏控制面板」这类长标签会被挤到折行。
const OPTION_LABEL_MIN_W: i32 = 100;

/// 参数行固定高度（逻辑 px）。
///
/// 见 [`option_row`]：所有参数行强制等高，行间距因此与控件种类**无关**。取值须 ≥ 最高
/// 控件的固有高度——滑块 18 / 复选框 18 / 颜色选框 28 / 下拉框 30 / 文本框 30（均为
/// `font_size + 16`，`font_size = 14`）——并留一点上下余量；控件在行内由 `cross`
/// 垂直居中。整列疏密就由这一个常量决定，要调只改这里。
pub const ROW_H: i32 = 40;

/// 正文缩进标准（2026-09-25 "统一 UI" 定案，同日第四定案收敛为两档）：`build_title_row`
/// 的 `body_indent` 只允许两个值——
/// - `0`：**子卡**。子卡没有开关，标题栏也不再放假开关占位（见 `build_title_row`），
///   标题文字落在 `父卡缩进(24) + 16(内边距) = +40`——与主卡标题文字（开关 24 +
///   内边距 16）和主卡第一个正文子（缩进 24 + 内边距 16）**同一条对齐线**；
///   再缩就双倍、再放占位就错位，都不允许；
/// - [`INDENT_CARD`]：普通卡**和含子卡的父卡**。数值 = [`SWITCH_W`]，正文参数标签
///   因此与标题文字**左对齐**（标题文字从 SWITCH_W 之后开始，正文标签从缩进之后
///   开始，同一个 x）。
///
/// ★ 全项目只有一条文字对齐线 `卡左 + 40`（2026-09-25 用户拍板"子卡片第一个子与
/// 主卡片第一个子对齐"）：主卡标题文字、主卡正文标签、子卡标题文字、子卡正文标签
/// 全部落在它上面。标题栏下拉的对齐偏移**直接复用 body_indent**（见
/// `build_title_row`）：控件列永远同 x = `卡左 + 16 + col1_width + body_indent`，
/// 父卡 24 = 子卡所在的父内容缩进 24 → 主卡与子卡的下拉也同 x。
/// 页面要规划间隔，只动这一个变量，不再各自目测调间距。
pub const INDENT_CARD: i32 = SWITCH_W;

pub struct FeatureCard {
    name: String,
    enabled: Option<Signal<bool>>,
    expanded: Signal<bool>,
    /// 是否可折叠。`false` = 常开卡（不画折叠箭头、名称不可点、正文恒显示）。
    collapsible: bool,
    title_dropdown: Option<(Vec<String>, Signal<usize>)>,
    /// 标题下拉绑信号（用于「刷新脚本」这类运行期会变的选项表）。
    title_dropdown_signal: Option<(Signal<Vec<String>>, Signal<usize>)>,
    /// 标题栏尾部追加的自定义控件（在折叠箭头左边）。
    title_extra: Option<Element>,
    body: Element,
}

impl FeatureCard {
    pub fn new(name: &str, expanded: Signal<bool>) -> Self {
        Self {
            name: name.to_string(),
            enabled: None,
            expanded,
            collapsible: true,
            title_dropdown: None,
            title_dropdown_signal: None,
            title_extra: None,
            body: Element::col(),
        }
    }

    /// 给标题行加一个功能开关（绑到功能总开关信号）。
    pub fn switch(mut self, enabled: Signal<bool>) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// 常开卡：去掉折叠箭头、名称不可点、正文恒显示。
    ///
    /// 用于**只作分组**的父卡片（「点击效果」「点击声音」）：它们的标题栏既没有开关也
    /// 没有下拉，折叠起来不承载任何状态，只会让用户看不见里面**已经启用**的子功能
    /// （五键各自的开关都在子卡里）。所以索性不让折叠。
    ///
    /// 传入的 `expanded` 信号被忽略（对外表现为恒 `true`），调用方仍可照常
    /// `.visible_when(exp.get())` 包正文——永远为真。
    pub fn always_open(mut self) -> Self {
        self.collapsible = false;
        self
    }

    pub fn title_dropdown(mut self, options: Vec<String>, selected: Signal<usize>) -> Self {
        self.title_dropdown = Some((options, selected));
        self
    }

    /// 标题下拉，选项表绑信号（可选内容运行期变化，如扫描出的 Lua 效果表）。
    pub fn title_dropdown_signal(
        mut self,
        options: Signal<Vec<String>>,
        selected: Signal<usize>,
    ) -> Self {
        self.title_dropdown_signal = Some((options, selected));
        self
    }

    /// 标题栏尾部追加控件（自定义按钮等）。放在折叠箭头之前。
    /// 当前无调用方，保留为组件能力（备用）。
    pub fn title_extra(mut self, el: Element) -> Self {
        self.title_extra = Some(el);
        self
    }

    pub fn body(mut self, body: Element) -> Self {
        self.body = body;
        self
    }

    /// 构建卡片：返回 `(标题行, 正文行, 展开信号)`。
    ///
    /// `body_indent` = 正文左缩进（0 = 不缩进）。**缩进必须作为正文行内部的前导叶子，
    /// 且正文带权重**：`measure_linear` 把完整的可用宽传给非权重子节点
    /// （`.3rdParty/wind-ui-rust-0.16.1/src/core.rs:1096` 的 `child_spec(main_eff, main_avail, ..)`
    /// 用的是 `main_avail` 而非「剩余」），若正文不带权重，它会按满宽测量，行宽 =
    /// 缩进 + 满宽 → 整行右溢出。带权重后正文拿到 `exactly(可用宽 - 缩进)`，行宽精确
    /// 等于父容器宽，右边缘因此与父标题栏对齐、不超出窗口。
    pub fn build_title_row(
        self,
        col1_width: i32,
        body_indent: i32,
    ) -> (Element, Element, Signal<bool>) {
        let name = self.name.clone();
        let collapsible = self.collapsible;
        // 常开卡：对外恒为已展开。传进来的信号被忽略，调用方 `.visible_when(exp.get())`
        // 因此永远为真，无需改调用点。
        let expanded = if collapsible {
            self.expanded
        } else {
            signal(true)
        };

        let leading = match self.enabled {
            // 高度钉成方框边长：见 `SWITCH_BOX` 的注释——不钉就会因两次整数截断高 1px。
            // 宽度钉成 `SWITCH_W`：空标签复选框的 `measure` 返回 `BOX_SIZE + GAP + 0 = 26`，
            // 比占位 leaf 的 `SWITCH_W = 24` 宽 2px——不钉，带开关卡片的标题文字就会比
            // 无开关卡片右移 2px（见 Issue 1 修复）。布局引擎对显式 `.width()` 取
            // `Dimension::Exact`，精确报告 24，与占位对齐到同一 x。
            Some(sig) => Element::checkbox("", sig)
                .height(SWITCH_BOX)
                .width(SWITCH_W),
            // 占位：不显示假开关，但保持标题左边界与其它卡片一致。
            // ★ 例外：子卡（`body_indent == 0`）**不放占位**（2026-09-25 用户定案）
            //   ——子卡没有开关，24px 空白毫无意义；去掉后子卡标题文字恰好落在
            //   `父卡缩进 24 + 内边距 16 = +40` 这条全项目对齐线上（见 `INDENT_CARD`）。
            None if body_indent == 0 => Element::leaf().width(0),
            None => Element::leaf().width(SWITCH_W),
        };
        // 折叠/展开的点击区**只能**落在名称与箭头上，绝不能包住整行：
        // windui 的 `Clickable` 会消费 `Down`/`Up`（冒泡在它那里断掉），整行可点就等于
        // 把行内的复选框、下拉、按钮全部点不动——复选框点了不打勾、且没有任何报错。
        let toggle = move |_ctx: &mut EventCtx| expanded.set(!expanded.get());

        let name_label = Element::stack()
            .child(
                Element::label(&name)
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT)),
            )
            .align(Align::Center);
        // 常开卡的名称不可点：点了也没有状态可变，留着可点区只会让人以为能折叠。
        let name_hit = if collapsible {
            name_label.clickable().on_click(toggle)
        } else {
            name_label
        };

        // `cross(Align::Center)` 不能省：`Element::row()` 的交叉轴默认是 `Align::Start`。
        // `height(HEADER_H)` 也不能省：它同时定住复选框的位置，见 `HEADER_H` 的注释。
        // 宽度 `col1_width` 由调用方按本页最长名称算好。
        let col1 = Element::row()
            .child(leading)
            .child(name_hit)
            .cross(Align::Center)
            .width(col1_width)
            .height(HEADER_H);

        // 折叠箭头（常开卡不画——没有"折起来"这个状态，箭头就是误导）。
        let chevron_hit = if collapsible {
            // 对齐写在**箭头自己**身上，不能写在 stack 上：`arrange_frame` 只读子元素的
            // `align`（缺省 Start），容器上的 `align` 对子元素无效——写错地方箭头就落在
            // 28×28 命中区的左上角，看着「折叠箭头偏高」。
            let chevron = Element::label_signal(expanded.map(|e| {
                if *e {
                    "\u{25BC}".to_string()
                } else {
                    "\u{25B6}".to_string()
                }
            }))
            .fg(to_color(tokens::TEXT_SECONDARY))
            .align(Align::Center);
            Element::stack()
                .child(chevron)
                .width(28)
                .height(28)
                .clickable()
                .on_click(toggle)
        } else {
            // 零宽占位：保持标题右侧内边距与其它卡片一致（`row` 的 `padding_xy(16,0)`）。
            Element::leaf().width(0)
        };

        // 标题行**不做左缩进**：需要缩进的整块内容（如「点击效果」下的五键子卡）由父卡
        // 的 `body_indent` 统一缩进，子卡因此天然填满「父内容区」，右边缘与顶层卡片一致。
        let mut row = Element::row().child(col1);

        // ★ 控件列对齐标准（2026-09-25 "统一 UI" 定案）：标题栏下拉与正文参数控件
        //   **必须同 x**。正文控件列 = 卡左 + body_indent(缩进叶子) + 16(row 内边距)
        //   + col1_width(标签列)；标题下拉原本紧跟 col1，比正文控件列少一个
        //   body_indent —— 这正是"拖尾效果"卡里效果下拉与"不透明度"数值条错位 24px
        //   的根源。补同宽前导占位后，两条等式共用同一个偏移变量（`body_indent`，
        //   取值见 [`INDENT_CARD`]），任何卡片不再各自目测调间隔。
        //
        // ★ 标题栏下拉**右缘对齐**标准（2026-09-25 第三定案，推翻此前的"拉伸填满"
        //   误案）：下拉宽度 = [`CONTROL_GROUP_W`]（正文数值条 120 + 间隔 8 + 数值
        //   输入框 72 = 200，单一来源），左缘同正文控件列（上面的 body_indent 占位
        //   保证），右缘因此**恰好落在数值输入框的右缘**——"不透明度"这类
        //   滑条+输入框行的右缘就是标题下拉的右缘。不要改成 weight 拉伸：
        //   `flex_spacer` 本身也是 weight(1.0)，两者会把剩余空间对半分，且填满整行
        //   与正文控件的右缘毫无关系（2026-09-25 用户纠正过一次）。
        let has_title_controls = self.title_dropdown.is_some()
            || self.title_dropdown_signal.is_some()
            || self.title_extra.is_some();
        if has_title_controls {
            row = row.child(Element::leaf().width(body_indent));
        }
        if let Some((options, selected)) = self.title_dropdown {
            row = row.child(Element::dropdown(options, selected).width(CONTROL_GROUP_W));
        }
        if let Some((options, selected)) = self.title_dropdown_signal {
            row = row.child(Element::dropdown_signal(options, selected).width(CONTROL_GROUP_W));
        }
        if let Some(extra) = self.title_extra {
            row = row.child(extra).child(Element::leaf().width(8));
        }
        // ★ 弹性空隙**必须有**：它把标题色带撑到整卡宽度（权重子节点拿走全部剩余
        // 宽），同时把折叠箭头推到右缘。丢失的症状 = 标题条只剩"内容总和"宽，
        // 右边一截没有底色（2026-09-25 改写时弄丢过一次，用户立刻发现）。
        row = row
            .child(Element::flex_spacer())
            .child(chevron_hit)
            .cross(Align::Center)
            .height(HEADER_H)
            .padding_xy(16, 0)
            .margin_xy(0, tokens::PAGE_TOP_MARGIN)
            .bg(to_color(tokens::CARD_HEADER_BG))
            // ★ 容器级圆角（2026-09-25 统一）：标题条是独立色带（上下 margin 与正文
            //   隔开、正文无底色），四角直接圆即可——不存在"标题条+正文拼接处露角"
            //   的问题，也就无需 per-corner 能力。值走 `tokens::CORNER_RADIUS`（8），
            //   与 about 卡一致，同属容器级。
            .corner(tokens::CORNER_RADIUS);

        // 正文左缩进：见本方法文档——叶子在前、正文 `weight(1.0)` 在后，正文因此拿到
        // `exactly(父宽 - 缩进)`，右边缘与父容器（即顶层卡片标题栏）严格对齐。
        let body = if body_indent > 0 {
            Element::row()
                .child(Element::leaf().width(body_indent))
                .child(self.body.weight(1.0))
        } else {
            self.body
        };

        (row, body, expanded)
    }
}

/// 单个字符的估算宽度（逻辑 px）：ASCII 约 0.6em，其余（汉字/全角/省略号）按 1em。
///
/// [`text_width`] 与 `ui::sfx::fit`（按下拉文本预算截断显示名）**必须**共用它。
/// 各写一遍就会打架——实测踩过：省略号在一处按半角算、另一处按全角算，
/// 截断出的显示名比预算宽 5.6px，在下拉按钮里折行。
pub fn char_width(c: char, font_size: f32) -> f32 {
    if c.is_ascii() {
        font_size * 0.6
    } else {
        font_size
    }
}

/// 估算一段文本的渲染宽度（逻辑 px）。
///
/// 全项目**唯一**的宽度模型：给 [`calc_col1_width`]（算列宽）与 `ui::sfx`（截断显示名）
/// 共用，保证"算出来放得下"与"显示真的放得下"是同一个判断。
pub fn text_width(s: &str, font_size: f32) -> f32 {
    s.chars().map(|c| char_width(c, font_size)).sum()
}

/// 一段灰色说明文字（`pad_top` 控制与上一行的间距；左右内边距 16 与其它行对齐）。
///
/// ★ 字号就是面板唯一字号 [`tokens::FONT_MD`]："这是次要信息"靠
/// [`tokens::TEXT_SECONDARY`] 的灰色表达，**不靠降字号**（2026-09-28 字号统一——
/// 降 2px 看着像没排好，层级也没更清楚；"放不下"一律走截断，见 §六）。
///
/// 原先是 `pages::tool` 的私有函数，2026-09-28 快捷键页也要用它讲互斥规则，提到
/// 这里做**行组件正本**（本文件是全项目行组件的唯一来源）。
pub fn note(text: &str, pad_top: i32) -> Element {
    Element::label(text)
        .font_size(tokens::FONT_MD)
        .fg(to_color(tokens::TEXT_SECONDARY))
        .padding_xy(16, pad_top)
}

/// 标题列（开关 + 名称）的宽度：取本页最长名称，`font_size` 为标题字号。
pub fn calc_col1_width(names: &[&str], font_size: f32) -> i32 {
    let padding: i32 = 32;
    let max_name_w = names
        .iter()
        .map(|n| text_width(n, font_size) as i32)
        .max()
        .unwrap_or(0);
    SWITCH_W + max_name_w + padding
}

/// ★ 全项目**唯一**的标签列宽（2026-09-25 第六定案）。
///
/// 此前六页各自 `calc_col1_width(本页标签)`——标签集不同 → 各页 `col1_w` 不同 →
/// **标题栏下拉的 x 随页漂移**，用户在鼠标页与键盘页之间切换时看到控件列对不上。
/// 收敛为一个常量：所有页面同一条对齐线、同一个控件列 x。
///
/// 取值 = 最长标签 `"不按 Shift 印下档"`(≈147px) + `SWITCH_W`(24) + 内边距(32)
/// = 204，向上取 208 留余量（护栏测试 `col1_width_covers_the_longest_labels`
/// 钉死）。新页面出现更长的标签时改这一个常量即可；超长标签由行内
/// `OPTION_LABEL_MIN_W` 下限与固定行高兜底，不再各自扩列。
pub const COL1_W: i32 = 208;

/// 一行「参数名 + 控件」，参数名列宽 = `col1_width`，控件左对齐，行垂直居中。
///
/// # 为什么标签列宽必须是 `col1_width`
///
/// 标题行是 `row(…).padding_xy(16,0)`，列1 占 `col1_width`，所以**标题里的控件列
/// （下拉/开关）左边缘 = 卡片左边 + 16 + col1_width**。参数行同用 16px 内边距，
/// 于是「标签列宽 = `col1_width`」就让**参数值控件的左边缘与标题控件列严格同 x**——
/// 这就是"内容按本卡片的列对齐"的全部机制（一级卡与子卡同一套，子卡只是整块被父卡
/// 的 `body_indent` 平移，相对关系不变）。
///
/// 早先用固定 100：只有恰好 `col1_width == 100 + 16` 的页面看着是对齐的，
/// 别的页面（如 `col1_width = 131` 的全局页）就会偏 16px 以上。
///
/// 标签列宽不小于 [`OPTION_LABEL_MIN_W`]：页面标题名短（如 2 字）时 `col1_width` 会
/// 小于长标签所需，窄了会把标签挤折行。
///
/// **所有行等高（`ROW_H`，与控件种类无关）**：滑块 18 / 复选框 18 / 颜色选框 28 /
/// 下拉框 30 / 文本框 30，固有高度各不相同。行高钉死、纵向内边距归零，行间距完全由
/// 这唯一一个固定行高承担——否则行高会随本行控件的高度浮动，下拉框那几行就比滑块行
/// 高一截，整列行距参差（正是此前「透明度」行挤、下拉行松的不和谐来源）。
///
/// 控件在行内由 `cross(Align::Center)` 垂直居中，故不同高度的控件彼此行距仍然完全相等；
/// 想调整整列疏密，改 [`ROW_H`] 一处即可。
pub fn option_row(label: &str, control: Element, col1_width: i32) -> Element {
    Element::row()
        .child(
            Element::label(label)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY))
                .width(col1_width.max(OPTION_LABEL_MIN_W)),
        )
        .child(control)
        .child(Element::flex_spacer())
        .cross(Align::Center)
        .height(ROW_H)
        .padding_xy(16, 0)
}

/// [`option_row`] 的标签随信号联动版：行名本身就是状态（如「管理员权限（已启用）」），
/// 勾选切换时文字实时刷新，不再另配一行说明。版式与 [`option_row`] 完全一致。
pub fn option_row_signal(label: Signal<String>, control: Element, col1_width: i32) -> Element {
    Element::row()
        .child(
            Element::label_signal(label)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY))
                .width(col1_width.max(OPTION_LABEL_MIN_W)),
        )
        .child(control)
        .child(Element::flex_spacer())
        .cross(Align::Center)
        .height(ROW_H)
        .padding_xy(16, 0)
}

/// 「标签 + 下拉」一行（2026-09-25 自 tool / screen / capture 三页私有拷贝收拢为
/// **唯一正本**——样式只改这一处，四页自动同步）。
///
/// ★ 宽度标准：下拉取 [`CONTROL_GROUP_W`]，右缘与标题栏下拉 / 数值输入框右缘是
/// 同一条竖线。§37 起选项**不再预截断**：闭合框里超宽的当前项由 windui 的
/// `Dropdown::paint` 绘制时收前置 `…`（保尾部），弹出菜单则按内容**自适应变宽**
/// （菜单层 `level_width = max(内容宽, 触发框宽)`）——音频设备名这类长串在
/// 弹出层完整可辨，此前「选项预截断」会把弹出层一起截掉，正是为修这个。
pub fn choice_row(
    label: &str,
    options: Vec<String>,
    sig: Signal<usize>,
    col1_width: i32,
) -> Element {
    option_row(
        label,
        Element::dropdown(options, sig).width(CONTROL_GROUP_W),
        col1_width,
    )
}

/// 与 [`choice_row`] 同形，选项表绑信号（目录扫描出的声音/效果列表运行期可换）。
pub fn choice_row_signal(
    label: &str,
    options: Signal<Vec<String>>,
    sig: Signal<usize>,
    col1_width: i32,
) -> Element {
    option_row(
        label,
        Element::dropdown_signal(options, sig).width(CONTROL_GROUP_W),
        col1_width,
    )
}

/// 「标签 + 取色器」一行（2026-09-25 同日收拢）：取色器与标题栏下拉同宽
/// （右缘对齐标准）。需要透明度条的脚本参数（`EffectParamCell::Color`）仍走
/// `color_picker_opts`，但宽度同样取 [`CONTROL_GROUP_W`]。
pub fn color_row(label: &str, sig: Signal<Color>, col1_width: i32) -> Element {
    option_row(
        label,
        Element::color_picker(sig).width(CONTROL_GROUP_W),
        col1_width,
    )
}

/// 「标签 + 滑块 + 数值输入框」三件套（**全软件唯一正本**，2026-09-25 §35 定案：
/// 所有数值条一律这个样式——用户口径「所有数值条的都要改成这个样式，才能全软件统一」）。
///
/// 滑条好拖但难精确（时长量程 1–3600 秒时 1px ≈ 18s），右侧 72px 输入框补精度：
/// 点击全选（`autofocus_select_all`）、键入数字、实时生效；空串 / 非数字是打字
/// 中间态，不动值。滑块与输入框绑**两个信号**（f32 / String），写回侧按
/// "**谁与基准（配置现值）偏离谁赢**"对账——面板固定参数的对账统一在
/// `ui::model::slider_pair_sync`（脚本参数在 `ui::model::float_param_sync`）。
/// 滑块**不带值标签**：数值只在输入框显示一处，不重复。
pub fn slider_row(
    label: &str,
    sig: Signal<f32>,
    text_sig: Signal<String>,
    min: f32,
    max: f32,
    decimals: u8,
    col1_width: i32,
) -> Element {
    option_row(
        label,
        Element::row()
            .child(Element::slider(sig).slider_value_range(min, max, decimals))
            // 滑块与输入框之间的间隙（row 默认 spacing 0，控件会贴死）。
            // 三个成员宽度的和 = `CONTROL_GROUP_W`，与标题栏下拉的宽度同源——
            // 标题下拉的右缘因此恰好落在本行输入框的右缘。
            .child(Element::leaf().width(SLIDER_INPUT_GAP))
            .child(
                Element::text_input(text_sig, "")
                    .width(INPUT_W)
                    .autofocus_select_all(),
            )
            .cross(Align::Center),
        col1_width,
    )
}

/// 「浏览…」按钮与它右边文件名之间的间隔（逻辑 px）。
const BROWSE_GAP: i32 = 24;

/// 「文件名 / 路径」这一段的文本预算（逻辑 px）。
///
/// 内容区 = 面板 900 − 导航 66 = 834；一行扣掉左右内边距 32、标签列 ~200、
/// 「浏览…」按钮 ~66、间隔 24 ≈ 470 可用。取 500（宽度模型比真实渲染保守 ~10%，
/// 实际画出来更窄）：500px ≈ 60 个 ASCII 字符（0.6em ≈ 8.4px），出厂素材目录
/// 57 字符的完整路径（`…\build\assets\Sounds\click3.wav`）能整条显示不截断；
/// 再长由 [`ellipsize`] 收成前置 `…`（保尾部，见该函数）。
///
/// ★ 2026-09-22 起浏览类行**恒显示完整路径**（不再只给文件名或"出厂音效"这种标签），
/// 40–60 字符的绝对路径是常态，早先的 400px 会让它们普遍被截断。
pub const PATH_TEXT_W: i32 = 500;

/// 按像素预算截断字符串，超出的部分收成一个前置 `…`（省略号也占预算）——**保尾部**。
///
/// 为什么从尾部保：本函数唯一的消费者是浏览类行，显示的是**路径**——尾部（文件名、
/// 最后一级目录）才是关键信息，头部（盘符、用户目录）丢了照样能认出"选的是哪个"；
/// 反过来保头部会把 `click3.wav` 截成 `click3.…`，等于把用户最想看的部分藏起来
/// （2026-09-22 实测抓到的反例）。
///
/// 宽度模型复用 [`char_width`]——与下拉显示名的截断（`ui::sfx::fit`）同一套，否则会
/// 出现"算出来放得下、显示却溢出"。★ 逐**字符**累加：中文与 ASCII 混排时不会切在
/// UTF-8 码点中间（字节偏移切分是本项目明令禁止的，见长期约束）。
pub fn ellipsize(s: &str, max_px: f32, font_size: f32) -> String {
    if text_width(s, font_size) <= max_px {
        return s.to_string();
    }
    let budget = (max_px - char_width('…', font_size)).max(0.0);
    let mut kept: Vec<char> = Vec::new();
    let mut used = 0.0;
    for c in s.chars().rev() {
        let w = char_width(c, font_size);
        if used + w > budget {
            break;
        }
        used += w;
        kept.push(c);
    }
    kept.reverse();
    let mut out = String::with_capacity(kept.len() + 3);
    out.push('…');
    out.extend(kept);
    out
}

/// 一行「标签 + 浏览…按钮 + 文件名/路径」——**所有浏览类行的统一形状**。
///
/// ★ 2026-09-22 定：早先是"按钮顶在行首、文件名紧随其后"，与本卡其它参数行的
/// 「标签列 + 控件列」两列结构对不齐。现在与其它参数行完全同构：标签占 `col1_width`
/// （左），按钮落在控件列（与滑块/下拉同一 x），文件名再往右一格间隔。
///
/// `display` 把信号里的值转成显示文本。★ 2026-09-22 定的规则：**要么空，要么给出
/// 选定对象的完整路径**——浏览按钮后面挂的是"我选了什么"，只给文件名等于让用户自己
/// 猜它在哪个目录；给不出真实路径时才退化成原值。现成实现：[`path_display`]（短名补
/// 素材目录）、`ui::sfx::sound_dir_display` / `ui::sfx::sound_file_display`。
///
/// 文本按 [`PATH_TEXT_W`] 截断（超宽收成 `…`），不撑破行。
///
/// `act` 在按钮点击后的**阻塞槽**里执行（`defer_blocking`：文件对话框是模态的，绝不能
/// 在 UI 线程里直接开），挑到东西后自行 `sig.set(...)`。
pub fn browse_row(
    label: &str,
    sig: Signal<String>,
    col1_width: i32,
    display: impl Fn(&str) -> String + 'static,
    act: impl Fn(Signal<String>) + 'static,
) -> Element {
    // 两个闭包都进 `Rc`：`on_click` 收的是 `FnMut`（一次点击一次的闭包会被反复调用），
    // 信号映射也是 `Fn`，都不能把所有权移走——每次用克隆一个句柄。
    let act = std::rc::Rc::new(act);
    let display = std::rc::Rc::new(display);
    option_row(
        label,
        Element::row()
            .child(Element::button("浏览…").on_click(move |ctx| {
                let a = act.clone();
                let s = sig;
                ctx.defer_blocking(move || a(s));
            }))
            .child(Element::leaf().width(BROWSE_GAP))
            .child(
                Element::label_signal({
                    let d = display.clone();
                    sig.map(move |s| ellipsize(&(*d)(s), PATH_TEXT_W as f32, tokens::FONT_MD))
                })
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT)),
            )
            .cross(Align::Center),
        col1_width,
    )
}

/// 浏览类行的默认显示规则：**空就是空，非空恒给完整路径**。
///
/// 配置里存的是**短名**（浏览导入时文件被复制进素材目录，见各调用点），而用户挑的是
/// "哪个文件"——只回显 `curseen.gif` 等于让他自己猜目录，所以这里补成
/// `<asset_dir>/<name>`。已经是绝对路径的（复制失败时的退化值）原样给回。
pub fn path_display(value: &str, asset_dir: &Path) -> String {
    let v = value.trim();
    if v.is_empty() {
        return String::new();
    }
    let p = Path::new(v);
    if p.is_absolute() {
        return v.to_string();
    }
    asset_dir.join(p).to_string_lossy().into_owned()
}

pub fn divider() -> Element {
    Element::leaf().height(1).bg(to_color(tokens::BORDER))
}

/// 卡片正文里**一段的段名**（2026-10-07 用户口径：同构的两组行并进一张卡，用
/// 「横线 + 段名」标出每段是什么）。
///
/// 段名 = **暗红 + 加粗**（2026-10-07 用户口径：主色近黑与行标签的灰差不够显眼，
/// 段名得一眼看出"它是下面这一组的名字"）。色值取 `tokens::TEXT_ACCENT`，字重 600——
/// 这是界面上**第一处**非中性文字色与**第一处**非默认字重，别处不要跟着用
/// （一多就不叫强调了，等于把整屏文字都降级）。
///
/// 与行标签同一字号，但**不带标签列宽**：它就是一行的说明文字，不需要参与
/// 控件列的对齐（`option_row` 的 `col1_w` 是给"标签 + 控件"用的）。段名与它下面的行
/// 共用同一条左缩进（由所属卡片的 `body_indent` 给），调用方在它上面配 `divider()`。
///
/// ★ 骨架与 [`option_row`] **逐项同款**（`row` + `height(ROW_H)` + `cross(Center)` +
/// `padding_xy(16, 0)`）：段名必须占**一整行的高度**、文字垂直居中，否则它到下一行的
/// 中心距会短于行距（文字只有 ~20px 高），整列节奏在段名处塌一下 —— 2026-10-07
/// 用户就是先看到这个"行间隔不一样"。
pub fn group_label(text: &str) -> Element {
    Element::row()
        .child(
            Element::label(text)
                .font_size(tokens::FONT_MD)
                .font_weight(600)
                .fg(to_color(tokens::TEXT_ACCENT)),
        )
        .cross(Align::Center)
        .height(ROW_H)
        .padding_xy(16, 0)
}

/// 一行「标签 + 快捷键捕获框」。
///
/// `col1_w` 透传给 [`option_row`]，让框左边缘与标题栏的控件列同 x。`cap` 是面板共享的
/// 捕获态信号，点框进入捕获态时置成 `Some(field)`。屏幕书写页与录制页各有一组快捷键，
/// 放这里共用一份实现（捕获逻辑在 `Runtime::frame()`，两页走的同一条通路）。
///
/// ★ §36（2026-09-25）：框宽 = [`CONTROL_GROUP_W`]——快捷键框是**输入类控件**（与下拉 /
/// 数值输入框同层级），宽度必须与它们同一条右缘竖线（此前写死 150，右缘比下拉短
/// 50px，正是"快捷键设定没对齐"的来源）。
/// 快捷键捕获框的**视觉正本**（2026-09-29 自 `hotkey_row` 抽出）：框宽/高/配色/圆角/
/// 字号只有这一份——固定字段（屏幕标注等）与脚本热键（`hotkey_row_script`）共用，
/// 改一个地方两套框一起变。
///
/// `display` 由调用方算好（捕获中「按下按键…」vs 当前值/「（空）」）；`enter` 是
/// 点击进入捕获态的动作（清空现值 + 置捕获目标）；`blur` 是失焦动作（★ 2026-10-02
/// 用户口径：点了框又点别处 = 放弃捕获，框里直接空着，不再挂着「按下按键…」等人按键）。
fn hotkey_box_el(
    display: Signal<String>,
    enter: impl Fn(&mut EventCtx) + 'static,
    blur: impl Fn() + 'static,
) -> Element {
    Element::stack()
        .child(
            Element::label_signal(display)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT))
                // ★ 框内两轴居中（2026-09-25）：`align` 在 stack（`Layout::Frame`）里是
                //   **子元素专属**语义——`arrange_frame` 按"子元素自己的 align"摆放，
                //   缺省 `Align::Start` 即左上角。这层对齐与下面 stack 自己那个是两回事，
                //   各管一层，缺一不可（此前漏的就是本行，文字才顶在框左上角）。
                //   label 收缩包装为文本尺寸，居中摆放 = 文本视觉居中，任何文案长度都对。
                .align(Align::Center),
        )
        .width(CONTROL_GROUP_W)
        .height(30)
        .bg(to_color(tokens::CARD_HEADER_BG))
        .border(to_color(tokens::BORDER), 1)
        // ★ 控件级圆角（2026-09-25 统一）：快捷键框与输入框/下拉是同一层级的"输入类
        //   控件"，后者由 windui 主题取 corner_md=6；此前本框无 corner（直角），同层
        //   一圆一方。值必须走 `tokens::CORNER_CONTROL`（对齐主题 6），不要写字面量。
        .corner(tokens::CORNER_CONTROL)
        // 这层 `align` 管的是"框在 option_row 行内的位置"：`arrange_linear` 里子元素
        // 自己的 align 会**覆盖**容器的 cross（本行 = 显式 Center，行高大于 30 时框
        // 垂直居中）。它**不是**文字对齐——别再往这里找"文字居中"，那在子元素上。
        .align(Align::Center)
        .clickable()
        .on_click(enter)
        // ★ 失焦即退出捕获态（windui 补丁 094）：点框外的任何地方（别的控件或空白）
        //   焦点离开本框，`blur` 在下一帧 paint 时触发。0.30 起回调带 `&mut EventCtx`。
        .on_blur(move |_ctx| blur())
}

pub fn hotkey_row(
    label: &str,
    sig: Signal<String>,
    field: crate::ui::model::HotkeyField,
    cap: Signal<Option<crate::ui::model::HotkeyField>>,
    col1_w: i32,
) -> Element {
    // 显示文案随 `cap` 变化：捕获中显示「按下按键…」，否则显示当前值（空则「（空）」）。
    // 每次 `sig` 变动都伴随一次 `cap` 变动（点击清空、捕获完成），故在 `cap.map` 里顺手读
    // `sig` 即可，无需单独订阅 `sig`（避免重复渲染）。
    //
    // ★「（空）」= 这把键没配（空值即停用），不是一个动作文案。以前写的是「留空停用」，
    //   四个字读起来像"我按了这个东西"，还容易与框里已配好的键混；用户口径 2026-09-27
    //   改成「（空）」，带括号一眼就是占位符。
    let display = cap.map(move |c| {
        if *c == Some(field) {
            "按下按键…".to_string()
        } else {
            let v = sig.get();
            if v.trim().is_empty() {
                "（空）".to_string()
            } else {
                v
            }
        }
    });
    let box_el = hotkey_box_el(
        display,
        move |_ctx: &mut EventCtx| {
            // 点击即清空已有快捷键 + 进入捕获态。`cap` 置 `Some(field)` 后，`Runtime` 在
            // 下一帧拦截到的按键会经 `PanelModel::hotkey_signal` 写回 `sig` 并清零 `cap`。
            sig.set(String::new());
            cap.set(Some(field));
        },
        move || {
            // ★ 失焦 = 放弃捕获，值保持点击时已清空的现状（显示「（空）」）。
            //   ★★ 必须判「捕获目标仍是本字段」：点另一个捕获框时，本框的 blur 在
            //     paint 时才触发，可能晚于新框 on_click 置上的新捕获态——无脑清会把
            //     新框的捕获态一并打掉。
            if cap.get() == Some(field) {
                cap.set(None);
            }
        },
    );
    option_row(label, box_el, col1_w)
}

/// 脚本热键捕获框（2026-09-29，`type = "hotkey"` 参数）：与 [`hotkey_row`] 共用同一
/// 视觉正本 [`hotkey_box_el`]，交互一致（点击清空 + 进入捕获态）。差别只在捕获目标：
/// 固定字段是编译期枚举（`HotkeyField`），脚本参数是运行期随单元重建的信号——
/// 捕获态存 [`ScriptHotkeyCap`]（带目标 sig），回写走 `Runtime::try_capture_hotkey`
/// 的 script 分支。
pub fn hotkey_row_script(
    label: &str,
    sig: Signal<String>,
    target: crate::ui::model::ScriptHotkeyCap,
    cap: Signal<Option<crate::ui::model::ScriptHotkeyCap>>,
    col1_w: i32,
) -> Element {
    // display / on_click / on_blur 三个闭包各持一份 target（非 Copy；判等按 sig 槽位，等价）。
    let shown_target = target.clone();
    let display = cap.map(move |c| {
        if c.as_ref() == Some(&shown_target) {
            "按下按键…".to_string()
        } else {
            let v = sig.get();
            if v.trim().is_empty() {
                "（空）".to_string()
            } else {
                v
            }
        }
    });
    let blur_target = target.clone();
    let box_el = hotkey_box_el(
        display,
        move |_ctx: &mut EventCtx| {
            sig.set(String::new());
            cap.set(Some(target.clone()));
        },
        move || {
            // ★ 失焦 = 放弃捕获（判据同 `hotkey_row`：目标仍是本框才清）。
            if cap.get().as_ref() == Some(&blur_target) {
                cap.set(None);
            }
        },
    );
    option_row(label, box_el, col1_w)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 对齐标准不变式（2026-09-25 "统一 UI" 定案）：
    /// `INDENT_CARD == SWITCH_W`——普通卡正文参数标签与标题文字左对齐全靠这条等式；
    /// 全项目文字对齐线 = 卡左 + SWITCH_W + 16，父卡与普通卡同缩进（子卡 0 缩进
    /// 且无占位，标题文字落在父卡缩进 + 16，同一条线）。
    ///
    /// 改这个常量 = 改全项目的对齐基准，必须三思。
    #[test]
    fn col1_width_covers_the_longest_labels() {
        // 全项目已知最长的行标签（新增更长标签时应改 COL1_W 并把新标签加进来）：
        let longest = [
            "不按 Shift 印下档", // 虚拟键盘（vk_config）
            "按 Shift 印上档",   // 虚拟键盘
            "贴图数量上限",      // capture::capture_labels
            "从哪个设备来",      // capture
        ];
        assert!(
            calc_col1_width(&longest, tokens::FONT_MD) <= COL1_W,
            "COL1_W 必须容得下全项目最长的行标签，否则标签会被挤折行"
        );
    }

    /// 快捷键集合的行名（「模块 > 行标签」）出现在**设置页**，标签列就是 `COL1_W`，按
    /// **渲染字号**（`option_row` / [`hotkey_row`] 用的 `FONT_MD`，也是面板唯一字号）量。2026-09-27 起末段改为逐字复用各页原文（见
    /// `ui::pages::global::HOTKEY_ENTRIES`），同日这三行又省掉「快捷键」三字，最长项从
    /// 「截图与贴图 > 截图并贴图快捷键」（13 汉字 = 207.2px，只余 0.8px）落到「截图与
    /// 贴图 > 截图后贴图」（10 汉字 = 165.2px）。★ `COL1_W` 是全项目唯一对齐常量，收缩
    /// 它会挪动所有页的控件列 x、必须全局验收，所以余量变大也先留着不动。
    #[test]
    fn hotkey_collection_labels_fit_the_column_at_render_font() {
        let labels = ["截图与贴图 > 截图后贴图", "截图与贴图 > 截图"];
        for label in labels {
            let w = text_width(label, tokens::FONT_MD);
            assert!(
                w <= COL1_W as f32,
                "「{label}」按渲染字号 {} 折 {:.1}px，超出 COL1_W={}——标签要被挤",
                tokens::FONT_MD,
                w,
                COL1_W
            );
        }
    }

    #[test]
    fn alignment_indents_are_tied_to_the_switch_width() {
        assert_eq!(
            INDENT_CARD, SWITCH_W,
            "正文标签与标题文字的对齐依赖 INDENT_CARD == SWITCH_W 这条等式"
        );
    }

    /// 短文本原样返回（连空串也是）。
    #[test]
    fn ellipsize_leaves_short_text_alone() {
        let f = tokens::FONT_MD;
        assert_eq!(ellipsize("", 100.0, f), "");
        assert_eq!(ellipsize("click3.wav", 400.0, f), "click3.wav");
    }

    /// 超长的按像素预算截断：**保尾部**——前置 `…`，且以原串的尾巴收尾（文件名必须
    /// 留着），结果放得进预算（含省略号）。
    #[test]
    fn ellipsize_keeps_the_tail_within_the_pixel_budget() {
        let f = tokens::FONT_MD;
        let long = r"C:\Users\someone\Documents\机械键盘音效包\click5.wav";
        let out = ellipsize(long, 200.0, f);
        assert!(out.starts_with('…'), "截断以省略号开头：{out}");
        assert!(out.ends_with("click5.wav"), "文件名必须留住：{out}");
        assert!(out.len() < long.len(), "确实被截短了：{out}");
        // ★ 判据不是"少几个字符"，而是"渲染宽度不超预算"——宽度模型同一套才作数。
        assert!(
            text_width(&out, f) <= 200.0,
            "截断后仍超宽：{} > 200",
            text_width(&out, f)
        );
    }

    /// ★ 逐字符切分：中文不会被切在码点中间（字节偏移切分是本项目明令禁止的）。
    #[test]
    fn ellipsize_never_splits_a_character() {
        let f = tokens::FONT_MD; // 汉字按 1em 计
        let s = "一二三四五六七八九十";
        let out = ellipsize(s, 4.0 * f, f); // 预算 = 3 个汉字 + 1 个省略号（汉字/省略号都按 1em）
        assert!(out.starts_with('…'));
        let body = out.trim_start_matches('…');
        assert_eq!(body.chars().count(), 3, "应正好留最后 3 个完整汉字：{out}");
        assert!(body.chars().all(|c| s.contains(c)), "不该出现半个字的乱码");
        assert!(s.ends_with(body), "保的是尾部：{out}");
    }

    /// 浏览行的显示：空就是空（"要么空、要么给出选定对象"）。
    #[test]
    fn path_display_of_nothing_is_nothing() {
        let dir = std::path::Path::new(r"D:\素材");
        assert_eq!(path_display("", dir), "");
        assert_eq!(path_display("   ", dir), "");
    }

    /// 短名补成素材目录里的完整路径；已经绝对的（复制失败时的退化值）原样给回。
    #[test]
    fn path_display_always_yields_a_full_path() {
        let dir = std::path::Path::new(r"D:\素材");
        let out = path_display("curseen.gif", dir);
        assert!(out.ends_with("curseen.gif"), "{out}");
        assert!(out.starts_with("D:\\素材"), "必须补上素材目录：{out}");
        assert_eq!(
            path_display(r"E:\别处\a.png", dir),
            r"E:\别处\a.png",
            "绝对路径不该被再拼一次"
        );
    }
}
