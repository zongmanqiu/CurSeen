//! 设计令牌
//!
//! 定义颜色、字体大小、间距等设计常量
//!
//! **配色整体为浅色主题**，取值刻意与 windui 默认主题（`theme::Palette::default()`，
//! 浅色）对齐：按钮、下拉、滑块、复选框这些内置控件由 windui 自己按主题取色，本文件
//! 只管面板自身画的底色/文字/边框。两边各按各的色系来（曾经的深色面板 + 浅色控件），
//! 就会出现白底黑字的下拉贴在深灰面板上的错配。
//!
//! 换肤入口就在这一个文件：全仓只有 `ui/` 用色，且不出现字面色值（见 check_deps.ps1）。

/// 字号档位（**只有两档，不要随手加新值**；2026-09-28 用户口径：常用一种、最多三种，
/// 字号统一才方便、视觉上也不突兀；界面里"字放不下"是**长度**问题，不是字号问题）。
///
/// - [`FONT_MD`] 14：**界面里的一切文字**。侧边栏导航、卡片标题条、页内分区标题、
///   行名、选项名、数值、链接、卡内说明，以及屏上的录屏完成提示窗——**全都 14**，
///   不再有"标题比正文大"这一档（层级交给 [`TEXT_SECONDARY`] 灰色、卡片底色、缩进
///   和分隔线去表达）。它恰好等于 windui 的默认 `font_size = 14.0`
///   （`windui::style::TextStyle::default()`），所以"看着像没设字号"是对的——我们只是
///   显式写出来，不依赖默认值（默认值一改就会静默走样）。
/// - [`FONT_DISPLAY`] 30：只给关于页的 `CurSeen` 大标题（2026-09-27 用户口径"要大一点"）。
///
/// ★ **"放不下"一律走截断，不走降档**：windui 的文本不换行、超宽只会画出去，所以
/// 长文本的正确处置是按像素预算截（`ui::card::ellipsize`，与下拉显示名同一套宽度模型）。
/// 曾经为一处长路径留过 `FONT_SM`(12) 这一档——实测同一条 46 字符的目录按 12px 排仍要
/// 391px、照样溢出 372px 的可用宽，降档只是把门槛推远，却凭空多出一档字号；已废。
///
/// ★ **不提供"界面字号"用户设置**：面板是 DPI aware 的（windui 按 `dpi_scale()` 把
/// 逻辑坐标放大到物理像素），系统缩放 125%/150%/200% 时整个面板等比放大，再给一个
/// 手动字号开关是重复功能；而代价不小——列宽、截断预算、行高、侧边栏宽度全是按
/// 14 这一个值算出来的常量，要做缩放就得乘遍布局。
pub const FONT_MD: f32 = 14.0;
/// 关于页 LOGO 字号（2026-09-27 用户口径：`CurSeen` 标题要"大一点"）。
pub const FONT_DISPLAY: f32 = 30.0;

/// 间距
pub const SPACING_SM: f32 = 8.0;
pub const SPACING_MD: f32 = 16.0;
pub const SPACING_LG: f32 = 24.0;

/// 圆角——**层级化**（2026-09-25 统一拍板，全面板只此两级 + windui 主题自带一级）：
///
/// - [`CORNER_RADIUS`] = 8：**容器级**（卡片标题带、about 卡）。
/// - [`CORNER_CONTROL`] = 6：**控件级**，对齐 windui 主题 `metrics.corner_md`——输入框/
///   下拉/按钮这些内置控件由主题自动取 6，应用自绘的控件（快捷键框）必须同值，否则
///   并排的两个"输入类控件"一圆一方（快捷键框统一直角→圆角前的实况）。
/// - 窗口级圆角归系统（Win11 DWM）+ 显式 `DWMWCP_ROUND`（剪贴板面板）；覆盖层/贴图窗
///   **刻意方角**（windui frameless 分支 DONOTROUND，理由见其注释），不参与统一。
pub const CORNER_RADIUS: f32 = 8.0;
/// 控件级圆角：与 windui 默认主题 `Metrics::corner_md`（6.0）逐值一致。
pub const CORNER_CONTROL: f32 = 6.0;

// 颜色（RGB）
/// 主色调（对齐 windui 默认主题的 accent）
pub const ACCENT: [u8; 3] = [76, 139, 245]; // 蓝色

/// 文本颜色
pub const TEXT: [u8; 3] = [45, 52, 54]; // 近黑，对齐 windui palette.text
pub const TEXT_SECONDARY: [u8; 3] = [99, 110, 114]; // 灰，对齐 windui palette.text_muted
/// 强调文字色（暗红）：只给**段名**用（`card::group_label`，2026-10-07 用户口径——
/// 段名要一眼看出"它是下面这一组的名字"，主色近黑与行标签的灰差不够）。
/// ★ 取暗红而非纯红：白底上对比度够、又不刺眼；这是界面上**第一处**非中性文字色，
///   别处不要跟着用（一多就不叫强调了）。
pub const TEXT_ACCENT: [u8; 3] = [192, 57, 43];

/// 背景颜色
pub const BG: [u8; 3] = [255, 255, 255]; // 白色（面板与内容区底色）
pub const NAV_BG: [u8; 3] = [246, 248, 250]; // 导航背景：极浅灰，与白底拉开层次
pub const CARD_BG: [u8; 3] = [246, 248, 250]; // 卡片背景
pub const CARD_HEADER_BG: [u8; 3] = [241, 243, 245]; // 卡片标题条：比卡片略深一档

/// 边框颜色
pub const BORDER: [u8; 3] = [226, 230, 234]; // 浅灰边框

/// 对话框颜色
pub const DIALOG_BG: [u8; 3] = [255, 255, 255]; // 对话框背景
pub const DIALOG_HEADER_BG: [u8; 3] = [246, 248, 250]; // 对话框标题背景
pub const DIALOG_FOOTER_BG: [u8; 3] = [246, 248, 250]; // 对话框底部背景

/// 按钮颜色
pub const BUTTON_PRIMARY: [u8; 3] = [76, 139, 245]; // 主要按钮
pub const BUTTON_OUTLINE: [u8; 3] = [207, 212, 220]; // 轮廓按钮

/// 便签原生窗（Win32/GDI）配色。
///
/// 便签画在**独立原生窗**上（`platform::sticky` + `ui::note_ui`），不走 windui 控件，
/// 所以取色只能自己给；这套值与便签出厂底色 `Config::sticky_note.bg_color` 同源
/// （RGB 40,44,52 那一档深色），窗口与它的设置页看着才是同一个东西。
pub const NOTE_BG: [u8; 3] = [40, 44, 52];
pub const NOTE_PANEL_BG: [u8; 3] = [52, 57, 66];
pub const NOTE_ROW_BG: [u8; 3] = [60, 65, 75];
pub const NOTE_TEXT: [u8; 3] = [232, 236, 240];
pub const NOTE_TEXT_DIM: [u8; 3] = [168, 173, 182];
pub const NOTE_ACCENT: [u8; 3] = [76, 139, 245];
pub const NOTE_BUTTON_BG: [u8; 3] = [70, 76, 88];
pub const NOTE_BUTTON_HOVER: [u8; 3] = [86, 93, 106];
pub const NOTE_SEPARATOR: [u8; 3] = [96, 102, 114];
pub const NOTE_SELECTED_ROW: [u8; 3] = [58, 74, 104];

/// 将RGB颜色转换为Color
pub fn to_color(rgb: [u8; 3]) -> windui::geometry::Color {
    windui::geometry::Color::rgba(rgb[0], rgb[1], rgb[2], 255)
}

/// 每页**首个 UI 元素**距页面内容顶的统一额外间隔（px；页面滚动容器自带 padding 16，
/// 这里是 padding 之外的那一档）。
///
/// ★ 单一来源（2026-09-27 用户口径"每个标签页首个元素距顶的间隔都要相同"）：
/// 卡片页的标题条（`card::FeatureCard::build_title_row` 的 `margin_xy(0, 4)`）与
/// 关于页首卡（`about.rs` 软件信息卡的 `margin_edges.top`）都取这个值——此前关于页
/// 首卡是 16，比卡片页的 4 多 12px，真机截图实锤（色带 y=51 vs 卡 y=63）。
/// 等效"首元素距客户区顶 20px = scroll padding 16 + 本值 4"。
pub const PAGE_TOP_MARGIN: i32 = 4;
