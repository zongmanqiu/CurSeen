//! 工具页（原「其它」页）：**不对应任何硬件**的那一类功能 —— 便签 / 剪贴板 / 计时器。
//!
//! # 为什么改名「工具」（2026-09-25 用户口径）
//!
//! 分类判据已定为"看这个功能**操作 / 处理的东西**是什么"（见 `ui::nav` 头注释）：便签
//! 操作的是我的文字、剪贴板操作的是复制的内容、计时器操作的是时间 —— 它们不属于鼠标 /
//! 键盘 / 屏幕任何一个硬件，但也不是"没想好"的杂项，而是一类正式成员。页名叫「其它」
//! 会让人以为里面装的是不重要的东西，名不副实，故改名「工具」。
//!
//! 页内顺序 = 用户口径：**便签 → 剪贴板 → 计时器**。
//!
//! # 「计时器」的开关架构（2026-09-25 起两道闸）
//!
//! 标题栏有**功能开关**（`timer.enabled`）；开关之外快捷键为空串仍是第二道闸——
//! 按快捷键开始，再按一下停止并清零。便签同此（`sticky_note.enabled` + 快捷键）。

use windui::prelude::*;

use crate::core::config::{
    CLIP_ENTRIES_LABELS, NOTE_ALPHA_MAX, NOTE_ALPHA_MIN, NOTE_FONT_MAX, NOTE_FONT_MIN,
    TIMER_DURATION_MAX, TIMER_DURATION_MIN, TIMER_FONT_MAX, TIMER_FONT_MIN, TIMER_OFFSET_MAX,
    TIMER_OFFSET_MIN,
};
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{HotkeyField, PanelModel};

// 数值滑块行 / 滑条+输入框三件套已收拢进 `card`（2026-09-25）：`card::slider_row`
// `card::slider_row` 现为「滑条 + 数值框」三件套唯一正本（2026-09-25 §35）——
// 三件套（详见 card.rs 两函数的文档）。本页直接调用，不再自带本地副本。

/// 一行「标签 + 下拉」（静态 `&str` 选项）。
fn choice_row(label: &str, options: &[&str], sig: Signal<usize>, col1_w: i32) -> Element {
    // 委托 `card` 正本：宽度（CONTROL_GROUP_W）与截断预算只在一处定义。
    card::choice_row(
        label,
        options.iter().map(|s| s.to_string()).collect(),
        sig,
        col1_w,
    )
}

// ★ `note`（一段灰色说明文字）已提到 `card::note` 做行组件正本——快捷键页也要用它。

pub fn build_tool_page(m: &PanelModel) -> Element {
    // 展开态来自模型（落盘 + 各卡独立）。★ 沿用时器的旧 expand id `global.timer`、
    // 便签的 `sticky`：id 与页无关（只管折叠态落盘），搬家不换 id —— 换了用户的
    // 折叠偏好就全丢。
    let timer_expanded = m.expand("global.timer");

    // 列1宽度：全项目唯一常量（2026-09-25 第六定案），所有页面同一条对齐线。
    let col1_w = card::COL1_W;

    let mut content = Element::col();

    // ── 便签（顶层卡，与「屏幕标注」平级）：多便签并存，置顶锁定收起编辑 ──
    let sticky_expanded = m.expand("sticky");

    let (title, body, exp) = FeatureCard::new("便签", sticky_expanded)
        .switch(m.sticky_enabled)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "快捷键",
                    m.hk_sticky,
                    HotkeyField::Sticky,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::color_row("背景颜色", m.note_bg_color, col1_w))
                .child(card::color_row("文字颜色", m.note_text_color, col1_w))
                .child(card::color_row("编辑区颜色", m.note_edit_color, col1_w))
                .child(card::slider_row(
                    "不透明度",
                    m.note_alpha,
                    m.note_alpha_text,
                    NOTE_ALPHA_MIN as f32,
                    NOTE_ALPHA_MAX as f32,
                    0,
                    col1_w,
                ))
                .child(card::slider_row(
                    "字号",
                    m.note_font_sz,
                    m.note_font_sz_text,
                    NOTE_FONT_MIN,
                    NOTE_FONT_MAX,
                    0,
                    col1_w,
                )),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    // ★ 首卡之前**不放** divider（2026-09-27 用户报"只有工具页顶部间隔大一点点"）：
    //   divider 是 1px `BORDER` 色线，摆在页面最顶 = 首卡标题条被压低 1px 还多一条
    //   横线，实测首行文字 y=55 vs 其它页 53~54。分隔线只属于"卡与卡之间"，
    //   本页卡间也没有用它的口径（其它页顶层卡间无 divider），整条删。
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 剪贴板历史（独立原生模块；开关 + 快捷键 + 容量 + 清空）──
    let clip_expanded = m.expand("clipboard");
    let (clip_title, clip_body, clip_exp) = FeatureCard::new("剪贴板", clip_expanded)
        .switch(m.clip_enabled)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "快捷键",
                    m.hk_clipboard,
                    HotkeyField::Clipboard,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(choice_row(
                    "历史容量",
                    &CLIP_ENTRIES_LABELS,
                    m.clip_max_entries,
                    col1_w,
                ))
                .child(card::option_row(
                    "清空历史",
                    Element::button("清空").on_click(move |_ctx| {
                        crate::platform::clipboard_history::request_clear_history();
                    }),
                    col1_w,
                )),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(clip_title)
        .child(clip_body.visible_when(move || clip_exp.get()));

    // ── 计时器 ──
    // 标题栏功能开关 + **模式下拉**（2026-09-25 自正文搬上来——它是功能级选择器，
    // 与击键回显 2026-09-22"模式上标题栏"的用户口径同款先例；番茄钟同日删除，
    // 只剩正计时 / 倒计时两档）+ 快捷键为空串的第二道闸。
    // 正文：位置（锚点 + 双向偏移，与画中画「位置」同形同量纲）+ 字号 + 时长（倒计时才显示）。
    // ★ 模式用一个**本地**信号副本给 `visible_when` 用：闭包要求 `'static`，而
    //   `Signal` 是 `Copy`，拷一份即可（不能借 `m`）。
    let timer_mode = m.timer_mode;
    let (title, body, exp) = FeatureCard::new("计时器", timer_expanded)
        .switch(m.timer_enabled)
        .title_dropdown(
            ["正计时", "倒计时"].iter().map(|s| s.to_string()).collect(),
            m.timer_mode,
        )
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "快捷键",
                    m.hk_timer,
                    HotkeyField::Timer,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::note(
                    "位置选非中央的锚点后，偏移把横幅从贴定的边往屏内挪\
                     （贴下边抬高即可让开任务栏）。快捷键留空即停用。",
                    6,
                ))
                // 锚点下标 = `global.timer.anchor` 的值一一对应（0 中央…6 右下），
                // 与 `runtime::timer::timer_center` 的 match 严格同序。
                .child(choice_row(
                    "位置",
                    &[
                        "中央",
                        "顶部居中",
                        "底部居中",
                        "左上",
                        "右上",
                        "左下",
                        "右下",
                    ],
                    m.timer_anchor,
                    col1_w,
                ))
                .child(card::slider_row(
                    "水平偏移",
                    m.timer_offset_x,
                    m.timer_offset_x_text,
                    TIMER_OFFSET_MIN as f32,
                    TIMER_OFFSET_MAX as f32,
                    0,
                    col1_w,
                ))
                .child(card::slider_row(
                    "垂直偏移",
                    m.timer_offset_y,
                    m.timer_offset_y_text,
                    TIMER_OFFSET_MIN as f32,
                    TIMER_OFFSET_MAX as f32,
                    0,
                    col1_w,
                ))
                .child(card::slider_row(
                    "字号",
                    m.timer_font,
                    m.timer_font_text,
                    TIMER_FONT_MIN as f32,
                    TIMER_FONT_MAX as f32,
                    0,
                    col1_w,
                ))
                // 时长只对倒计时有意义：正计时没有终点（番茄钟 2026-09-25 已删除）。
                // 滑条 + 数值输入框双入口（2026-09-25）：滑条好拖难精确，输入框补
                // 秒级精度，两侧在 `apply_to` 对账（见 `card::slider_row`）。
                .child(
                    card::slider_row(
                        "时长(秒)",
                        m.timer_duration,
                        m.timer_duration_text,
                        TIMER_DURATION_MIN as f32,
                        TIMER_DURATION_MAX as f32,
                        0,
                        col1_w,
                    )
                    .visible_when(move || timer_mode.get() == 1),
                ),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    Element::scroll().child(content.padding(16))
}

#[cfg(test)]
mod tests {
    /// 本文件的**生产代码**文本（`#[cfg(test)]` 之前那一段）。
    /// 同款先例见 `runtime::layer::gif_trim::tests::production_src`——
    /// `include_str!` 会把测试自己也包进来，必须切一刀，否则"数出现次数"
    /// 的断言会被断言自己的文本影响。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("tool.rs");
        let cut = SRC.find("#[cfg(test)]").unwrap_or(SRC.len());
        &SRC[..cut]
    }

    /// 结构门禁：本页（工具页）不该残留任何放大镜卡片的内容——「画中画放大」「全屏放大」
    /// 已按"操作对象是屏幕画面"整体搬去屏幕页（2026-09-25）。这张卡万一搬一半留了尾巴，
    /// 会出现"工具页里一张改了不生效的卡"（它的展开 id 还在、开关信号也还在，但屏幕页
    /// 那份才是真的），所以必须钉死。
    #[test]
    fn magnifier_cards_are_gone_from_the_tool_page() {
        let p = production_src();
        assert!(
            !p.contains("画中画放大") && !p.contains("全屏放大"),
            "放大镜两张卡已搬去屏幕页（ui/pages/screen.rs），工具页不得残留"
        );
        assert!(
            !p.contains("pip_enabled") && !p.contains("fs_enabled"),
            "放大镜开关信号引用必须一并搬走，别在工具页留死引用"
        );
    }
}
