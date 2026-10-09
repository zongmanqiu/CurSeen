//! 按键弹窗的布局计算。纯函数、无状态、可单测。
//! 输入：条目列表 + 设置 + 覆盖层逻辑尺寸 + 文字度量器；输出：每个盒子的矩形与行内容。

use super::merge::KeyItem;
use super::settings::{KeyDisplaySettings, Placement};
use crate::core::geometry::{Rect, Size};

/// Python overlay.py:15-17 的三个常量。
pub const KEY_PADDING: f32 = 8.0;
pub const KEY_MARGIN: f32 = 6.0;
pub const KEY_LINE_SPACING: f32 = 2.0;

/// 文字度量。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    pub width: f32,
    pub ascent: f32,
    pub height: f32,
}

/// 度量器 trait。测试里用 FakeMetrics 实现。
pub trait TextMeasure {
    fn measure(&mut self, s: &str, size_pt: f32) -> TextMetrics;
}

/// 一个已排好版的弹窗盒。
#[derive(Clone, Debug)]
pub struct PopupBox {
    /// 盒左上角（逻辑 px）。
    pub rect: Rect,
    /// 换行后的每一行文本。
    pub lines: Vec<String>,
    /// 该盒对应的条目在输入列表里的下标（index 0 = 最新）。
    pub item_index: usize,
    /// 盒的不透明度。
    pub alpha: f32,
    /// 行高（px）—— 相邻两行的**推进量**。
    pub line_height: f32,
    /// 第一行墨迹盒**顶边**相对 `rect.y` 的偏移（px）。
    ///
    /// ★ 渲染层 `DrawCmd::Text` 的锚点是「排版盒左上角」，**不是基线**（见
    /// `core::draw::DrawCmd::Text` 的文档与 `runtime/render.rs` 的翻译分支）。
    /// 这里早年照搬 Python `overlay.py` 的 `y + padding + li*line_height + fm.ascent()`
    /// —— 那是 PyQt `drawText` 的**基线**语义 —— 于是整行文字被多推下去一个 ascent
    /// （字号 16 时 = 12.8px），用户看到的就是"文字偏下、没有垂直居中"。
    ///
    /// 取值 `KEY_PADDING + KEY_LINE_SPACING / 2`：行间距上下各分一半，
    /// 首行顶边与末行底边的留白因此严格相等（见 `text_block_is_vertically_centered`）。
    pub text_top: f32,
}

/// 计算全部盒子的位置与尺寸。
/// `items` 必须已经按"最新在最后"排好（controller.items 的顺序）。
pub fn layout(
    items: &[KeyItem],
    settings: &KeyDisplaySettings,
    screen: Size,
    metrics: &mut dyn TextMeasure,
) -> Vec<PopupBox> {
    let font_px = settings.font_size;
    let max_w = screen.w * settings.popup_width as f32 / 100.0;
    let inner_w = max_w - KEY_PADDING * 2.0;

    // 反转：index 0 = 最新
    let reversed: Vec<&KeyItem> = items.iter().rev().collect();

    let mut boxes = Vec::new();
    for (item_idx, item) in reversed.iter().enumerate() {
        let raw = item.display_text();
        let lines = word_wrap(&raw, inner_w, metrics, font_px);
        if lines.is_empty() {
            continue;
        }

        let m = metrics.measure(&lines[0], font_px);
        let line_height = m.height + KEY_LINE_SPACING;
        let max_lw = lines
            .iter()
            .map(|l| metrics.measure(l, font_px).width)
            .fold(0.0f32, f32::max);
        let box_w = (max_lw + KEY_PADDING * 2.0).min(max_w);
        let total_h = lines.len() as f32 * line_height + KEY_PADDING * 2.0;

        let x = compute_x(settings.placement, screen.w, box_w, settings);
        let alpha = settings.opacity * item.opacity;

        boxes.push(PopupBox {
            rect: Rect::new(x, 0.0, box_w, total_h),
            lines,
            item_index: item_idx,
            alpha,
            line_height,
            text_top: KEY_PADDING + KEY_LINE_SPACING / 2.0,
        });
    }

    // Y 堆叠。☆ bottom_offset 在收敛后由「垂直偏移」信号驱动（同名字段保留，
    // 语义 = 贴定的那条水平边往屏内挪的屏幕高度百分比，贴下边抬高即可让开任务栏）。
    let is_bottom = settings.placement.is_bottom();
    if is_bottom {
        let mut cy = screen.h - screen.h * settings.bottom_offset as f32 / 100.0;
        for b in &mut boxes {
            cy -= b.rect.h;
            b.rect.y = cy;
            cy -= KEY_MARGIN;
        }
    } else {
        let mut cy = screen.h * settings.top_offset as f32 / 100.0;
        for b in &mut boxes {
            b.rect.y = cy;
            cy += b.rect.h + KEY_MARGIN;
        }
    }

    boxes
}

/// 运行时逐字符真实推进宽缓存：`{(字符, 逻辑字号位模式) → 逻辑推进宽}`。
///
/// 由 runtime 渲染层用 windui/DirectWrite 的 `measure_text` **逐字符**测量后写入
/// （与实际绘制同源、含 DPI 换算）。[`char_advance`] **优先查缓存**——查到即真实
/// 排版推进宽，segment 边界的累计 x 与 windui 的实际渲染一致，键名墨迹不可能
/// 压过后续空格段；未命中回退下方分类表（宁宽勿窄），测试/离线场景确定性保持。
static MEASURED_ADVANCE: std::sync::RwLock<Option<std::collections::HashMap<(char, u32), f32>>> =
    std::sync::RwLock::new(None);

/// 渲染层写入一个字符的真实推进宽（**逻辑** px，物理宽已除以 DPI 缩放）。
///
/// ★ 容量封顶 [`MEASURED_ADVANCE_CAP`]：键 = (字符 × 字号)，字号档位理论无限
/// （虚拟键盘可自定义字号），不封顶就是理论缓涨。超限整体清空——缓存miss只是
/// 回退分类表，正确性不受影响（泄露审计 2026-09-30）。
pub fn set_measured_advance(c: char, font_px: f32, width: f32) {
    let mut guard = MEASURED_ADVANCE.write().unwrap_or_else(|e| e.into_inner());
    let m = guard.get_or_insert_with(std::collections::HashMap::new);
    if m.len() >= MEASURED_ADVANCE_CAP && !m.contains_key(&(c, font_px.to_bits())) {
        m.clear();
    }
    m.insert((c, font_px.to_bits()), width);
}

/// 缓存条目上限：字符 × 常用字号档位，实际远达不到；64K 是兜底不是常态。
const MEASURED_ADVANCE_CAP: usize = 64 * 1024;

/// 该字符在该字号下是否已有真实测量值。
pub fn has_measured_advance(c: char, font_px: f32) -> bool {
    let guard = MEASURED_ADVANCE.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .map(|m| m.contains_key(&(c, font_px.to_bits())))
        .unwrap_or(false)
}

fn measured_advance(c: char, font_px: f32) -> Option<f32> {
    let guard = MEASURED_ADVANCE.read().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .and_then(|m| m.get(&(c, font_px.to_bits())))
        .copied()
}

/// 字符推进宽度（px）。2026-09-22 起按 Segoe UI 16px 的实测墨宽分类校准
/// （**宁宽勿窄**：表值 ≥ 真实墨宽，否则键名墨迹压过后续空格段——
/// "LMB x5" 显示成 "LMBx5" 的事故根因）。此前所有 ASCII 一律 0.5×字号，
/// M/W 等宽字形（真实墨宽 ≈0.85-0.92×字号）严重超出推进量。
/// render::advance 与两处 FakeMetrics 都委托本表——三方一张表，不再漂移。
pub fn char_advance(c: char, font_px: f32) -> f32 {
    // ★ 真实测量优先（DirectWrite 同源）：缓存命中 = 该字符的推进宽与实际渲染
    //   完全一致；未命中（测试/离线/新字符首帧前）回退分类表——宁宽勿窄。
    if let Some(w) = measured_advance(c, font_px) {
        return w;
    }
    let cp = c as u32;
    if cp > 0x7F {
        return font_px; // CJK / 全角 / 其它非 ASCII：全宽
    }
    if c == ' ' {
        // 0.40：Segoe UI 实测 0.27，高估 48%（宁宽勿窄）——空格可见宽度不足时
        // 组合键加号后、键名与徽标之间的间隔会视觉消失（用户实测 " +ALT"）。
        return font_px * 0.40;
    }
    if c == 'l' {
        return font_px * 0.50;
    }
    if matches!(
        c,
        'i' | 'j' | 't' | 'f' | 'r' | 'I' | '.' | ',' | ':' | ';' | '!' | '|' | '\'' | '`'
    ) {
        return font_px * 0.42;
    }
    if c == 'M' || c == 'm' {
        return font_px * 0.88;
    }
    if c == 'W' || c == 'w' {
        return font_px * 0.92;
    }
    if c.is_ascii_uppercase() {
        return font_px * 0.78;
    }
    font_px * 0.58 // 小写其它 / 数字
}

/// 整段推进宽度（px）。
pub fn text_advance(text: &str, font_px: f32) -> f32 {
    text.chars().map(|c| char_advance(c, font_px)).sum()
}

fn word_wrap(text: &str, max_w: f32, metrics: &mut dyn TextMeasure, font_px: f32) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut cur_words = Vec::new();
    let mut cur_w = 0.0f32;
    let space_w = metrics.measure(" ", font_px).width;

    for &word in &words {
        let rw = metrics.measure(word, font_px).width;
        let sep = if cur_words.is_empty() { 0.0 } else { space_w };
        if cur_w + sep + rw <= max_w || cur_words.is_empty() {
            cur_words.push(word);
            cur_w += sep + rw;
        } else {
            lines.push(cur_words.join(" "));
            cur_words = vec![word];
            cur_w = rw;
        }
    }
    if !cur_words.is_empty() {
        lines.push(cur_words.join(" "));
    }
    lines
}

/// X 位置计算。☆ 2026-09-21 起偏移收敛为两根（水平/垂直，同画中画放大）：
/// 方位下拉已决定左右，旧的 left/right 两根里永远有一根是无效项。
/// 水平偏移 = 贴定的那条边往屏内挪的屏幕宽度百分比。
fn compute_x(
    placement: Placement,
    screen_w: f32,
    box_w: f32,
    settings: &KeyDisplaySettings,
) -> f32 {
    if placement.is_left() {
        screen_w * settings.left_offset as f32 / 100.0
    } else {
        screen_w - box_w - screen_w * settings.right_offset as f32 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeMetrics;

    impl TextMeasure for FakeMetrics {
        fn measure(&mut self, s: &str, size_pt: f32) -> TextMetrics {
            // 与 render::advance 同一张字宽表（2026-09-22 统一）。
            TextMetrics {
                width: text_advance(s, size_pt),
                ascent: size_pt * 0.8,
                height: size_pt * 1.2,
            }
        }
    }

    /// ★ 字宽表"宁宽勿窄"校准：Segoe UI 16px 的实测墨宽（px）——表值低于墨宽，
    ///   键名就会压过后面的空格段（"LMB x5" 显示成 "LMBx5" 的事故根因）。
    #[test]
    fn advance_never_underestimates_real_ink_width() {
        let cases = [
            ("LMB", 31.6),
            ("RMB", 31.0),
            ("C", 10.4),
            ("Wheel", 48.6),
            ("Space", 42.9),
            ("x5", 15.9),
            ("Win", 27.0),
        ];
        for (name, real) in cases {
            let adv = text_advance(name, 16.0);
            assert!(adv >= real, "advance({name})={adv} 低估真实墨宽 {real}");
        }
    }

    #[test]
    fn layout_single_item() {
        let items = vec![KeyItem::new("A", 1.0, true)];
        let settings = KeyDisplaySettings::default();
        let screen = Size {
            w: 1920.0,
            h: 1080.0,
        };
        let mut metrics = FakeMetrics;
        let boxes = layout(&items, &settings, screen, &mut metrics);
        assert_eq!(boxes.len(), 1);
        assert!(boxes[0].rect.w > 0.0);
        assert!(boxes[0].rect.h > 0.0);
    }

    #[test]
    fn layout_empty_items() {
        let items = vec![];
        let settings = KeyDisplaySettings::default();
        let screen = Size {
            w: 1920.0,
            h: 1080.0,
        };
        let mut metrics = FakeMetrics;
        let boxes = layout(&items, &settings, screen, &mut metrics);
        assert!(boxes.is_empty());
    }

    #[test]
    fn layout_multiple_items() {
        let items = vec![KeyItem::new("A", 1.0, true), KeyItem::new("B", 2.0, true)];
        let settings = KeyDisplaySettings::default();
        let screen = Size {
            w: 1920.0,
            h: 1080.0,
        };
        let mut metrics = FakeMetrics;
        let boxes = layout(&items, &settings, screen, &mut metrics);
        // 2 items, reversed → 2 boxes
        assert_eq!(boxes.len(), 2);
    }

    #[test]
    fn word_wrap_basic() {
        let mut metrics = FakeMetrics;
        let lines = word_wrap("A B C", 100.0, &mut metrics, 16.0);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn word_wrap_split() {
        let mut metrics = FakeMetrics;
        // Very narrow: only fit one word per line
        let lines = word_wrap("A B C", 10.0, &mut metrics, 16.0);
        assert!(lines.len() > 1);
    }

    // ─── 位置 / 尺寸的量纲门禁（用户口径：偏移与弹窗宽度都是**屏幕百分比**）───
    //
    // 这组用例把"百分比"从注释里的承诺变成机器判据：偏移量每变 1，落点就移动屏幕尺寸
    // 的 1%。此前 `layout` 的四个偏移与 `popup_width` **一条断言都没有** —— 它们看起来
    // 正确纯属"读代码看出来的"，一旦有人把 `/100.0` 改成 `/1000.0` 或把 `screen.h`
    // 换成 `box_w`，没有任何门禁会红。

    const SCREEN_W: f32 = 1920.0;
    const SCREEN_H: f32 = 1080.0;

    fn box_of(
        placement: Placement,
        top: u16,
        bottom: u16,
        left: u16,
        right: u16,
        width: u8,
    ) -> PopupBox {
        let items = vec![KeyItem::new("A", 1.0, true)];
        let s = KeyDisplaySettings {
            placement,
            top_offset: top,
            bottom_offset: bottom,
            left_offset: left,
            right_offset: right,
            popup_width: width,
            ..Default::default()
        };
        let screen = Size {
            w: SCREEN_W,
            h: SCREEN_H,
        };
        let mut metrics = FakeMetrics;
        let boxes = layout(&items, &s, screen, &mut metrics);
        assert_eq!(boxes.len(), 1);
        boxes.into_iter().next().expect("上面已断言非空")
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    /// 下偏移 = 屏幕高度的百分比：盒子**下边缘**距屏幕底 = `bottom_offset%` 屏高。
    #[test]
    fn bottom_offset_is_a_screen_percentage() {
        let b5 = box_of(Placement::BottomRight, 0, 5, 0, 0, 15);
        assert!(
            close(b5.rect.y + b5.rect.h, SCREEN_H * 0.95),
            "下偏移 5% 时盒底应在 y={}，实得 {}",
            SCREEN_H * 0.95,
            b5.rect.y + b5.rect.h
        );

        let b25 = box_of(Placement::BottomRight, 0, 25, 0, 0, 15);
        assert!(
            close(
                (b5.rect.y + b5.rect.h) - (b25.rect.y + b25.rect.h),
                SCREEN_H * 0.20
            ),
            "偏移 5% → 25% 的位移应恰为屏高的 20%"
        );
    }

    /// 上偏移 = 屏幕高度的百分比：盒子**上边缘**距屏幕顶 = `top_offset%` 屏高。
    #[test]
    fn top_offset_is_a_screen_percentage() {
        let b = box_of(Placement::TopLeft, 5, 0, 0, 0, 15);
        assert!(close(b.rect.y, SCREEN_H * 0.05), "实得 y={}", b.rect.y);

        let b2 = box_of(Placement::TopLeft, 40, 0, 0, 0, 15);
        assert!(close(b2.rect.y, SCREEN_H * 0.40), "实得 y={}", b2.rect.y);
    }

    /// 左 / 右偏移 = 屏幕**宽度**的百分比（不是高度、不是盒宽）。
    #[test]
    fn horizontal_offsets_are_screen_percentages() {
        let left = box_of(Placement::TopLeft, 0, 0, 12, 0, 15);
        assert!(
            close(left.rect.x, SCREEN_W * 0.12),
            "实得 x={}",
            left.rect.x
        );

        let right = box_of(Placement::TopRight, 0, 0, 0, 12, 15);
        assert!(
            close(right.rect.x + right.rect.w, SCREEN_W * 0.88),
            "右偏移 12% 时盒右缘应在 x={}，实得 {}",
            SCREEN_W * 0.88,
            right.rect.x + right.rect.w
        );
    }

    /// 只有**该方位**的那一半偏移生效：右下角的弹窗不该被上偏移/左偏移推走。
    ///
    /// 这条锁的是"四个滑块里有两个是死的"这一真实外观（右下角改上偏移毫无反应）——
    /// 它是**设计如此**（与虚拟键盘同源），用例把它钉成契约而不是缺陷。
    #[test]
    fn only_the_active_side_offsets_move_the_box() {
        let base = box_of(Placement::BottomRight, 0, 5, 0, 0, 15);
        let top_moved = box_of(Placement::BottomRight, 40, 5, 0, 0, 15);
        let left_moved = box_of(Placement::BottomRight, 0, 5, 40, 0, 15);
        assert!(close(base.rect.y, top_moved.rect.y), "右下角不吃上偏移");
        assert!(close(base.rect.x, left_moved.rect.x), "右下角不吃左偏移");

        // 生效的那两个必须真的动：位移量 = 屏幕尺寸的 40%。
        let bottom_moved = box_of(Placement::BottomRight, 0, 45, 0, 0, 15);
        assert!(close(base.rect.y - bottom_moved.rect.y, SCREEN_H * 0.40));
        let right_moved = box_of(Placement::BottomRight, 0, 5, 0, 40, 15);
        assert!(close(base.rect.x - right_moved.rect.x, SCREEN_W * 0.40));
    }

    /// `popup_width` = 屏幕**宽度**的百分比，且是盒宽的**上限**（内容窄时按内容收缩）。
    #[test]
    fn popup_width_is_a_screen_percentage_cap() {
        let s = |width: u8| {
            let items = vec![KeyItem::new(
                "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z",
                1.0,
                true,
            )];
            let st = KeyDisplaySettings {
                popup_width: width,
                ..Default::default()
            };
            let screen = Size {
                w: SCREEN_W,
                h: SCREEN_H,
            };
            let mut metrics = FakeMetrics;
            layout(&items, &st, screen, &mut metrics)
                .into_iter()
                .next()
                .expect("长文本必然产出盒子")
        };

        let narrow = s(10);
        let wide = s(30);
        let cap_narrow = SCREEN_W * 0.10;
        let cap_wide = SCREEN_W * 0.30;

        assert!(
            narrow.rect.w <= cap_narrow + 1e-3,
            "盒宽 {} 不得超过屏宽的 10% = {cap_narrow}",
            narrow.rect.w
        );
        assert!(
            wide.rect.w <= cap_wide + 1e-3,
            "盒宽 {} 不得超过屏宽的 30% = {cap_wide}",
            wide.rect.w
        );
        assert!(wide.rect.w > narrow.rect.w, "放宽上限必须真的变宽");
        assert!(
            narrow.lines.len() > wide.lines.len(),
            "卡得越窄行数越多：{} vs {}",
            narrow.lines.len(),
            wide.lines.len()
        );
        // 每一行的估算宽度都在可用文本宽之内（= 上限 - 两侧内边距）。
        for b in [&narrow, &wide] {
            let inner = b.rect.w - KEY_PADDING * 2.0;
            let mut metrics = FakeMetrics;
            for line in &b.lines {
                let w = metrics
                    .measure(line, KeyDisplaySettings::default().font_size)
                    .width;
                assert!(w <= inner + 1e-3, "行「{line}」宽 {w} 超出可用宽 {inner}");
            }
        }
    }

    /// 文本块在盒内**垂直居中**：首行顶边的留白 == 末行底边的留白。
    ///
    /// 判据用 `line_height - KEY_LINE_SPACING` 当墨迹高度 —— 这是 `layout` 对度量器的
    /// 假设（`line_height = m.height + KEY_LINE_SPACING`），`FakeMetrics` 满足它。
    /// 修复前 `text_top` 是 `KEY_PADDING + ascent`（ascent = 0.8 字号），字号 16 时
    /// 上留白 20.8px / 下留白 8px —— 差 12.8px，肉眼一眼可见的"文字偏下"。
    #[test]
    fn text_block_is_vertically_centered() {
        // 「A A A A」= 4×12.48 字宽 + 3×6.4 空格 = 69.1 ≤ 可用 80，再加第 5 个就要
        // 88 > 80（字宽表 2026-09-22 校准后：大写 0.78、空格 0.40）；
        // 故 `4*n` 个词稳定换出 `n` 行 —— 行数是这条用例的自变量，不能靠巧合。
        for lines in [1usize, 2, 5] {
            let text = vec!["A"; lines * 4].join(" ");
            let items = vec![KeyItem::new(&text, 1.0, true)];
            let st = KeyDisplaySettings {
                popup_width: 5, // 卡到最窄 → 强制换行成 lines 行
                ..Default::default()
            };
            let screen = Size {
                w: SCREEN_W,
                h: SCREEN_H,
            };
            let mut metrics = FakeMetrics;
            let b = layout(&items, &st, screen, &mut metrics)
                .into_iter()
                .next()
                .expect("有文本就有盒子");
            let n = b.lines.len() as f32;
            assert_eq!(n, lines as f32, "换行行数应符合预期（自变量）");

            let ink_h = b.line_height - KEY_LINE_SPACING;
            let gap_top = b.text_top;
            let gap_bottom = b.rect.h - (b.text_top + (n - 1.0) * b.line_height + ink_h);
            assert!(
                (gap_top - gap_bottom).abs() < 1e-3,
                "{n} 行时上下留白应相等，实得 上={gap_top} 下={gap_bottom}"
            );
        }
    }
}
