//! 文字尺寸估算。
//!
//! 算法层（`modules/*`）产出 `DrawCmd::Text` 时**拿不到字体**——真正的排版发生在渲染层的
//! `TextEngine` 里，而那一步在另一侧、也在另一个线程之外。但有三处必须要一个宽度：
//!
//!   · 提示浮字要水平居中；
//!   · 文字笔迹要算包围盒（橡皮按包围盒相交判定，包围盒偏小就会"擦不掉"）；
//!   · 工具面板里那个序号图标。
//!
//! 早先这里用的是 `text.len() as f32`——那是 **UTF-8 字节数**，一个汉字三字节，于是
//! 「屏幕标注模式」被算成 18 个字宽、宽出三倍，居中的字整体左偏。改成按字符类别估算：
//! 全角按 1.0 em、其余按 0.55 em。不求精确，只求量级对。
//!
//! 估算**宁可略大**：包围盒大了顶多多擦一点，小了会漏擦；居中的字略大则只是往左偏一点。

/// 是否占一个全角字宽（CJK、假名、谚文、全角标点）。
///
/// 区间取自 Unicode 的 East Asian Width = W/F 那几段，够本项目用；不追求完整实现。
fn is_wide(c: char) -> bool {
    matches!(c as u32,
        0x1100..=0x115F        // 谚文字母
        | 0x2E80..=0x303E      // 康熙部首 / CJK 部首 / CJK 符号标点
        | 0x3041..=0x33FF      // 平假名 / 片假名 / CJK 兼容
        | 0x3400..=0x4DBF      // CJK 扩展 A
        | 0x4E00..=0x9FFF      // CJK 基本区
        | 0xA000..=0xA4CF      // 彝文
        | 0xAC00..=0xD7A3      // 谚文音节
        | 0xF900..=0xFAFF      // CJK 兼容表意
        | 0xFE30..=0xFE4F      // CJK 兼容形式
        | 0xFF00..=0xFF60      // 全角形式
        | 0xFFE0..=0xFFE6      // 全角符号
    )
}

/// 估算一行文字的宽度（与 `size_px` 同单位）。
pub fn estimate_width(text: &str, size_px: f32) -> f32 {
    let mut em = 0.0f32;
    for c in text.chars() {
        em += if is_wide(c) { 1.0 } else { 0.55 };
    }
    em * size_px
}

/// 估算单行文字的高度（含行距）。
pub fn line_height(size_px: f32) -> f32 {
    size_px * 1.25
}

/// 让文字以 `center_y` 为垂直中心时的基线位置。
///
/// ⚠️ **只对"直接把基线交给渲染层"的场景有效**，而 `DrawCmd::Text` 的 `y` 不是基线——它是
/// **排版盒左上角**（见 `core::draw::DrawCmd::Text` 与 `runtime::render` 的翻译）。
/// 所以凡是"要让文字在某个盒子里垂直居中"的地方都**不要**用这个函数去凑 `Text::y`：
/// 模块层没有字体，盒高只能估，凑出来的偏差直接可见（序号贴纸曾因此偏圆心 13px）。
/// 正解是用 `DrawCmd::TextCentered`，把居中交给拿得到真实度量的渲染层。
///
/// 保留它是因为仍有消费者按"基线锚点"自行排版（如工具栏那些逐段累加 x 的图标）——
/// 那种场景下它就是对的。
pub fn baseline_for_center(center_y: f32, size_px: f32) -> f32 {
    center_y + size_px * 0.36
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_roughly_half_em() {
        let w = estimate_width("abcd", 100.0);
        assert!(
            (220.0..=220.0 + 1.0).contains(&w),
            "四个半角应约 2.2 em，得到 {w}"
        );
    }

    #[test]
    fn chinese_counts_one_em_each() {
        // 六个汉字 = 6 em。旧实现按字节算会得到 18 em（三倍宽），这条断言就是那个 bug 的锁。
        let w = estimate_width("屏幕标注模式", 28.0);
        assert!(
            (168.0 - 1.0..=168.0 + 1.0).contains(&w),
            "六个汉字应约 168px，得到 {w}"
        );
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_width("", 30.0), 0.0);
    }

    #[test]
    fn baseline_sits_below_center() {
        assert!(baseline_for_center(100.0, 20.0) > 100.0);
    }
}
