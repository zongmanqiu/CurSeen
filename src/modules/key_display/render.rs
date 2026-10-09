//! 按键弹窗绘制。逐词着色。逐行对照 overlay.py:152-245。
//!
//! 盒宽用"逐段 advance 累加"算，不整行一次量，保证与绘制严格自洽。

use super::keymap;
use super::layout::{PopupBox, KEY_PADDING};
use super::merge::{
    is_count_badge, is_shortcut_line, split_keeping_spaces, split_shortcut, split_x_suffix,
};
use super::settings::KeyDisplaySettings;
use crate::core::color::Argb;
use crate::core::draw::DrawList;

/// 绘制全部弹窗盒到 DrawList。
///
/// ★ 这里**不许**再推 `DrawList::clear_rect`。它曾在逐行绘制之后以「dirty rect」的
/// 名义追加一次整盒清除，而 `DrawCmd::Clear` 在渲染层（`render.rs` → `Canvas::clear_rect`）
/// 的语义是**把该矩形抹成透明**（`BlendMode::Clear`）——于是弹窗刚画完就被自己擦干净，
/// 屏上一个字都看不到。清屏由 `OverlayCanvas::paint` 每帧整窗负责，模块层不需要也不该做。
pub fn render_key_popups(
    boxes: &[PopupBox],
    settings: &KeyDisplaySettings,
    list: &mut DrawList,
    font_px: f32,
) {
    for b in boxes {
        if b.alpha <= 0.0 {
            continue;
        }
        let rect = b.rect;
        // 背景。★ 必须把 `b.alpha` 带下去：`fill_round_rect` 的内部 alpha **恒为 1.0**，
        // 用它画背景会让「不透明度」只作用于文字与边框、底色始终实心 —— 用户报的
        // 「不透明度无法调整背景颜色」就是这条（背景色自带 alpha=255，乘子又没传）。
        // 与 Python `overlay.py` 的 `bg.setAlpha(alpha)` 等价。
        list.fill_round_rect_alpha(rect, 6.0, settings.bg_color, b.alpha);
        // 边框已删（2026-09-25 晚用户口径：不要边框线）——原 1px #3C3C3C 描边整体移除。

        // 逐行绘制
        let text_x = rect.x + KEY_PADDING;
        // ★ 锚点是**墨迹盒顶边**，不是基线（见 `PopupBox::text_top`）。这里曾写
        //   `rect.y + KEY_PADDING + b.ascent`（照搬 Python 的 PyQt 基线语义），
        //   整行被多推下去一个 ascent —— 字号 16 时 12.8px，肉眼就是"文字偏下"。
        let mut line_y = rect.y + b.text_top;

        for line in &b.lines {
            if is_shortcut_line(line) {
                draw_shortcut(line, text_x, line_y, font_px, b.alpha, settings, list);
            } else {
                draw_plain(line, text_x, line_y, font_px, b.alpha, settings, list);
            }
            line_y += b.line_height;
        }
    }
}

/// 纯键绘制。逐 token，逐行对照 overlay.py:179-205。
fn draw_plain(
    line: &str,
    mut x: f32,
    y: f32,
    font_px: f32,
    alpha: f32,
    settings: &KeyDisplaySettings,
    list: &mut DrawList,
) {
    let tokens = split_keeping_spaces(line);
    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i];
        if tok == " " {
            draw_segment(tok, x, y, font_px, alpha, settings.font_color, list);
            x += advance(tok, font_px);
            i += 1;
        } else if is_count_badge(tok) {
            // "x{N}" 计数徽标（2026-09-22 统一格式）：单 token，整体 symbol_color。
            // 旧实现是 "x"+" "+"N" 三 token 拼合——键名（ini 自定义，如 "LMB"）与 "x"
            // 之间的空格依赖 token 推进宽度，实测丢过一次（用户报 "LMBx 5"）；
            // 徽标并成单 token 后，与键名的间距只由前面的空格 token 决定，不可能再丢。
            draw_segment(tok, x, y, font_px, alpha, settings.symbol_color, list);
            x += advance(tok, font_px);
            i += 1;
        } else {
            let ws = tok.trim();
            let color = if !ws.is_empty() {
                keymap::display_info(ws)
                    .map(|(_, f)| {
                        if f {
                            settings.fun_color
                        } else {
                            settings.font_color
                        }
                    })
                    .unwrap_or(settings.font_color)
            } else {
                settings.font_color
            };
            draw_segment(tok, x, y, font_px, alpha, color, list);
            x += advance(tok, font_px);
            i += 1;
        }
    }
}

/// 快捷键绘制。逐行对照 overlay.py:207-239。
fn draw_shortcut(
    line: &str,
    mut x: f32,
    y: f32,
    font_px: f32,
    alpha: f32,
    settings: &KeyDisplaySettings,
    list: &mut DrawList,
) {
    let parts = split_shortcut(line);
    for (idx, part) in parts.iter().enumerate() {
        if idx > 0 {
            // " + " 用 symbol_color
            draw_segment(" + ", x, y, font_px, alpha, settings.symbol_color, list);
            x += advance(" + ", font_px);
        }
        if idx == parts.len() - 1 {
            if let Some((key_part, count_part)) = split_x_suffix(part) {
                let color = if keymap::display_info(key_part)
                    .map(|(_, f)| f)
                    .unwrap_or(false)
                {
                    settings.fun_color
                } else {
                    settings.font_color
                };
                draw_segment(key_part, x, y, font_px, alpha, color, list);
                x += advance(key_part, font_px);
                draw_segment(
                    count_part,
                    x,
                    y,
                    font_px,
                    alpha,
                    settings.symbol_color,
                    list,
                );
                x += advance(count_part, font_px);
            } else {
                let color = if keymap::display_info(part).map(|(_, f)| f).unwrap_or(false) {
                    settings.fun_color
                } else {
                    settings.font_color
                };
                draw_segment(part, x, y, font_px, alpha, color, list);
                x += advance(part, font_px);
            }
        } else {
            let color = if keymap::display_info(part).map(|(_, f)| f).unwrap_or(false) {
                settings.fun_color
            } else {
                settings.font_color
            };
            draw_segment(part, x, y, font_px, alpha, color, list);
            x += advance(part, font_px);
        }
    }
}

/// 绘制一段文字。
fn draw_segment(
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    alpha: f32,
    color: Argb,
    list: &mut DrawList,
) {
    if text.is_empty() || alpha <= 0.0 {
        return;
    }
    list.text(x, y, text, size, color, Some(alpha));
}

/// 计算文字推进宽度。委托 [`super::layout::text_advance`] 的分类字宽表
/// （2026-09-22 起 ASCII 一律 0.5×字号的旧近似会让 M/W 等宽字形墨迹
/// 超出推进量、压过后续空格段——"LMBx 5" 的根因）。
fn advance(text: &str, size: f32) -> f32 {
    super::layout::text_advance(text, size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::draw::DrawCmd;
    use crate::core::geometry::Rect;
    use crate::modules::key_display::layout::KEY_LINE_SPACING;

    /// ★ 回归锁：按键弹窗的绘制指令里**不允许出现 `DrawCmd::Clear`**。
    ///
    /// 它曾经在逐行绘制之后以「dirty rect」的名义追加一次整盒清除；而 `Clear` 在渲染层
    /// 是「把该矩形抹成透明」，于是弹窗刚画完就被自己擦掉，屏上一个字都看不见。
    /// ★ 绘制级间距门禁（2026-09-22 计数格式统一）：`"LMB x5"` 的徽标段前必须保留
    /// 一个空格宽的推进——旧三 token 徽标曾在键名与 "x" 之间丢过空格（用户报
    /// "LMBx 5"）。本用例直接检查绘制指令的 x 坐标，间距错立刻红。
    #[test]
    fn count_badge_keeps_one_space_after_the_name() {
        let settings = KeyDisplaySettings::default();
        let mut list = DrawList::new();
        draw_plain("LMB x5", 0.0, 0.0, 16.0, 1.0, &settings, &mut list);
        let segs: Vec<(String, f32)> = list
            .cmds()
            .iter()
            .filter_map(|c| match c {
                DrawCmd::Text { text, x, .. } => Some((text.clone(), *x)),
                _ => None,
            })
            .collect();
        let (_, lmb_x) = segs.iter().find(|(t, _)| t == "LMB").expect("应有 LMB 段");
        let (_, badge_x) = segs
            .iter()
            .find(|(t, _)| t == "x5")
            .expect("应有 x5 徽标段");
        let gap = badge_x - (lmb_x + advance("LMB", 16.0));
        assert!(
            (gap - advance(" ", 16.0)).abs() < 0.01,
            "键名与徽标之间必须恰好一个空格宽"
        );
    }

    #[test]
    fn key_popup_draw_has_no_clear() {
        let settings = KeyDisplaySettings::default();
        let boxes = vec![PopupBox {
            rect: Rect::new(10.0, 10.0, 200.0, 40.0),
            lines: vec!["A".to_string()],
            item_index: 0,
            alpha: 1.0,
            line_height: 20.0,
            text_top: 9.0,
        }];
        let mut list = DrawList::new();
        render_key_popups(&boxes, &settings, &mut list, 16.0);
        assert!(!list.cmds().is_empty(), "弹窗应产出绘制指令");
        assert!(
            !list.cmds().iter().any(|c| matches!(c, DrawCmd::Clear(_))),
            "弹窗绘制指令里不允许有 Clear —— 它会把弹窗抹成透明"
        );
    }

    fn one_box(alpha: f32, font: f32) -> Vec<PopupBox> {
        vec![PopupBox {
            rect: Rect::new(10.0, 10.0, 200.0, 40.0),
            lines: vec!["A".to_string()],
            item_index: 0,
            alpha,
            line_height: font * 1.2 + KEY_LINE_SPACING,
            text_top: KEY_PADDING + KEY_LINE_SPACING / 2.0,
        }]
    }

    fn first_text_y(list: &DrawList) -> f32 {
        list.cmds()
            .iter()
            .find_map(|c| match c {
                DrawCmd::Text { y, .. } => Some(*y),
                _ => None,
            })
            .expect("纯键文本必须落成 DrawCmd::Text")
    }

    /// ★ 背景填充必须带上盒的不透明度。
    ///
    /// 回归锚点（用户报「不透明度无法调整背景颜色」）：背景曾用 `fill_round_rect` 画，
    /// 而那个便捷构造器的 `alpha` **恒为 1.0** —— 于是不透明度只作用到文字与边框，
    /// 底色始终实心。背景色自带 alpha=255，光看代码很难发现"乘子没传下去"。
    #[test]
    fn popup_background_carries_box_alpha() {
        let settings = KeyDisplaySettings::default();
        for alpha in [0.35f32, 1.0] {
            let mut list = DrawList::new();
            render_key_popups(&one_box(alpha, 16.0), &settings, &mut list, 16.0);
            let bg = list
                .cmds()
                .iter()
                .find_map(|c| match c {
                    DrawCmd::Fill { alpha, .. } => Some(*alpha),
                    _ => None,
                })
                .expect("弹窗必须画背景");
            assert!(
                (bg - alpha).abs() < 1e-6,
                "背景 alpha 应等于盒不透明度 {alpha}，实得 {bg}"
            );
        }

        // 对照组：不透明度 0 时整盒都不落笔（否则上面那条在"底色压根没画"时也可能通过）。
        let mut list = DrawList::new();
        render_key_popups(&one_box(0.0, 16.0), &settings, &mut list, 16.0);
        assert!(list.cmds().is_empty(), "不透明度 0 时不该产出任何绘制指令");
    }

    /// ★ 文字锚点 = **墨迹盒顶边**，不是基线（两倍/四倍字号都要成立）。
    ///
    /// 回归锚点（用户报「文本偏下、没有垂直居中」）：这里写的是
    /// `rect.y + KEY_PADDING + b.ascent`，ascent 来自 Python `overlay.py` 的
    /// `fm.ascent()`——那是 PyQt `drawText` 的**基线**语义，而本项目的 `DrawCmd::Text`
    /// 锚点是排版盒左上角。串了语义的净效果 = 整行下移一个 ascent（16 号字 12.8px）。
    #[test]
    fn popup_text_anchor_is_ink_box_top_not_baseline() {
        let settings = KeyDisplaySettings::default();
        for font in [8.0f32, 16.0, 32.0, 72.0] {
            let mut list = DrawList::new();
            render_key_popups(&one_box(1.0, font), &settings, &mut list, font);
            let y = first_text_y(&list);
            let want = 10.0 + KEY_PADDING + KEY_LINE_SPACING / 2.0;
            assert!(
                (y - want).abs() < 1e-6,
                "字号 {font}：文字顶边应为 {want}，实得 {y}"
            );
            let old = 10.0 + KEY_PADDING + font * 0.8;
            assert!(
                (y - old).abs() > 1.0,
                "字号 {font}：不得退回基线语义（那会得到 {old}）"
            );
        }
    }

    #[test]
    fn advance_ascii() {
        assert!((advance("ABC", 16.0) - 37.44).abs() < 0.01); // A0.78 + B0.78 + C0.78
    }

    #[test]
    fn advance_mixed() {
        assert!((advance("A\u{2190}", 16.0) - 28.48).abs() < 0.01); // A0.78 + ←1.0
    }

    #[test]
    fn advance_empty() {
        assert!((advance("", 16.0) - 0.0).abs() < 0.01);
    }

    #[test]
    fn draw_segment_empty() {
        let mut list = DrawList::new();
        draw_segment(
            "",
            0.0,
            0.0,
            16.0,
            1.0,
            Argb::new(255, 255, 255, 255),
            &mut list,
        );
        assert!(list.cmds().is_empty());
    }

    #[test]
    fn draw_segment_zero_alpha() {
        let mut list = DrawList::new();
        draw_segment(
            "A",
            0.0,
            0.0,
            16.0,
            0.0,
            Argb::new(255, 255, 255, 255),
            &mut list,
        );
        assert!(list.cmds().is_empty());
    }
}
