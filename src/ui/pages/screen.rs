//! 屏幕页：**一切以屏幕画面为操作对象的功能**（2026-09-25 重排）。
//!
//! 分类判据是"操作 / 处理的东西是什么"：屏幕标注（画它）、画中画 / 全屏放大（放大它）、
//! 截图与贴图（抓一帧）、GIF录制（抓多帧）、视频录制（抓一段视频）——五者处理的都是
//! 屏幕画面，所以同居本页。原先「截图」在屏幕页而「GIF / 录屏」另开一个录制页，是按
//! 产物形态切的一刀，而三者共用框选与采集管线（`recorder::Session`），属于人为割裂。
//!
//! ★ 层级结构：「屏幕标注」是父卡片（开关 = `drawing.enabled`，本功能的唯一开关），
//! 下面挂**两张同级子卡**：「画笔」（三支笔的颜色/粗细/线型）与「快捷键」（13 个书写快捷键）。
//! 两张子卡都**整块缩进**（父卡 `body_indent = INDENT_CARD` 统一给，标题 + 正文一起
//! 24px），一眼可见它们同属「屏幕标注」之下；往后新增的功能各自成为与「屏幕标注」
//! 平级的顶层卡片即可（对齐鼠标页拖尾/悬浮标/波纹/声音的同级布局）。子卡结构是
//! §36 起的正本形态：`build_title_row(col1_w, 0)` 直接挂进父卡正文（与键盘页同款），
//! 右边缘与父容器严格对齐。
//!
//! 快捷键用**点击捕获框**录入：点一下框 → 清空原值并进入捕获态（显示「按下按键…」），
//! 再按任意非修饰键即完成录入，按键串写回对应信号（空串 = 停用该动作）。捕获发生在
//! `Runtime::frame()` 的全局键盘钩子层（windui 没有 per-element `on_key`），不是文本框。

use windui::prelude::*;

use crate::core::config::{
    FULLSCREEN_ZOOM_MAX, FULLSCREEN_ZOOM_MIN, PIP_PANEL_MAX, PIP_PANEL_MIN, PIP_ZOOM_MAX,
    PIP_ZOOM_MIN,
};
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{self, HotkeyField, PanelModel};
use crate::ui::pages::capture;

// 数值滑块行已收拢进 `card::slider_row`（2026-09-25，四页私有拷贝的唯一正本）：
// ★ `min`/`max` 必须与 `PanelModel` 里那次 `v2s/s2v` 换算**用同一对常量**（量程在
// `core::config` 里定一处）的约定不变——见该函数文档。

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

pub fn build_screen_page(m: &PanelModel) -> Element {
    // 展开态来自模型（落盘 + 各卡独立），id 见 `core::config::CARD_IDS`。
    let pen_expanded = m.expand("drawing.pen");
    let pen_sub_expanded = m.expand("drawing.pensub");
    let hotkey_expanded = m.expand("drawing.hotkey");

    // 列1宽度：全项目唯一常量（2026-09-25 第六定案），所有页面同一条对齐线。
    // （音频设备名只当下拉选项、不当行标签，不再参与列宽计算。）
    let col1_w = card::COL1_W;

    let styles: Vec<String> = model::PEN_STYLES_LABELS
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut content = Element::col();

    // ── 快捷键（子卡）：13 个书写快捷键（2026-10-01 加「序号」）。
    // 顺序 = `core::hotkeys::DRAW_TOOL_ORDER`（= 工具栏按钮序，橡皮擦在马赛克之后），
    // 设置页集合表照抄同一顺序（门禁 `screen_hotkey_rows_follow_the_tool_order` 钉住）。§36 起与「画笔」子卡一起回归正本结构：
    // build_title_row(col1_w, 0) 直接挂进父卡正文（父卡 body_indent 统一给缩进），
    // 不再手写 SUBCARD_INDENT 包装行（与键盘页子卡同一套机制）。
    let (hk_title, hk_body, hk_exp) = FeatureCard::new("快捷键", hotkey_expanded)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "屏幕标注",
                    m.hk_screen,
                    HotkeyField::Screen,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "白板",
                    m.hk_white,
                    HotkeyField::White,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "画笔1",
                    m.hk_pen1,
                    HotkeyField::Pen1,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "画笔2",
                    m.hk_pen2,
                    HotkeyField::Pen2,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "画笔3",
                    m.hk_pen3,
                    HotkeyField::Pen3,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "直线",
                    m.hk_line,
                    HotkeyField::Line,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "箭头",
                    m.hk_arrow,
                    HotkeyField::Arrow,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "矩形",
                    m.hk_rect,
                    HotkeyField::Rect,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "椭圆",
                    m.hk_ellipse,
                    HotkeyField::Ellipse,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "文字",
                    m.hk_text,
                    HotkeyField::Text,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "序号",
                    m.hk_stamp,
                    HotkeyField::Stamp,
                    m.hotkey_capture,
                    col1_w,
                ))
                // ★ 顺序 = `core::hotkeys::DRAW_TOOL_ORDER`（工具栏按钮序），橡皮擦排在
                //   马赛克**之后**（2026-10-01 用户口径）：这里、工具栏、设置页三处同序。
                .child(card::hotkey_row(
                    "马赛克",
                    m.hk_mosaic,
                    HotkeyField::Mosaic,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::hotkey_row(
                    "橡皮擦",
                    m.hk_eraser,
                    HotkeyField::Eraser,
                    m.hotkey_capture,
                    col1_w,
                )),
        )
        .build_title_row(col1_w, 0);

    // ── 画笔（子卡）：三支画笔的颜色/粗细/线型，与「快捷键」子卡同构 ──
    let mut pens = Element::col();
    for (i, (color, width, width_text, style)) in [
        (m.pen1_color, m.pen1_width, m.pen1_width_text, m.pen1_style),
        (m.pen2_color, m.pen2_width, m.pen2_width_text, m.pen2_style),
        (m.pen3_color, m.pen3_width, m.pen3_width_text, m.pen3_style),
    ]
    .into_iter()
    .enumerate()
    {
        if i > 0 {
            pens = pens.child(card::divider());
        }
        pens = pens
            .child(card::color_row(
                &format!("画笔{} 颜色", i + 1),
                color,
                col1_w,
            ))
            .child(card::slider_row(
                &format!("画笔{} 粗细", i + 1),
                width,
                width_text,
                1.0,
                500.0,
                0,
                col1_w,
            ))
            .child(card::choice_row(
                &format!("画笔{} 线型", i + 1),
                styles.clone(),
                style,
                col1_w,
            ));
    }
    let (pen_sub_title, pen_sub_body, pen_sub_exp) = FeatureCard::new("画笔", pen_sub_expanded)
        .body(pens)
        .build_title_row(col1_w, 0);

    // 父卡正文直接挂两张子卡（canonical，2026-09-25 §36）：整块缩进由父卡
    // `body_indent = INDENT_CARD` 统一给，子卡右边缘与父容器严格对齐——
    // 与键盘页「击键回显 / 虚拟键盘」的子卡机制逐字节同款。
    let (title, body, exp) = FeatureCard::new("屏幕标注", pen_expanded)
        .switch(m.enabled_draw)
        .body(
            Element::col()
                .child(pen_sub_title)
                .child(pen_sub_body.visible_when(move || pen_sub_exp.get()))
                .child(card::divider())
                .child(hk_title)
                .child(hk_body.visible_when(move || hk_exp.get())),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 画中画放大（独立原生模块）：父卪（开关在标题行）+ 两张子卪「位置 / 显示」。
    // 子卪结构与击键回显同型（方位下拉 + 偏移滑条 / 尺寸倍率色板），同一个名字
    // 在不同页里指同一类参数，用户只学一次。位置在前：方位与偏移决定
    // "在哪儿"，是用户最先要碰的；宽高/倍率/边框色都是"长什么样"。
    let pip_expanded = m.expand("other.magnifier.pip");
    let pip_hk_expanded = m.expand("other.magnifier.pip.hotkey");
    let pip_pos_expanded = m.expand("other.magnifier.pip.pos");
    let pip_display_expanded = m.expand("other.magnifier.pip.display");
    let (pip_title, pip_body, pip_exp) = FeatureCard::new("画中画放大", pip_expanded)
        .switch(m.pip_enabled)
        .body({
            let mut col = Element::col();
            // ── 快捷键（子卪，2026-09-26 用户口径：快捷键单独一张子卡，与「位置 / 显示」
            //    同型；行名也从「启停快捷键」简化为「快捷键」）。出厂留空，配置后按一次
            //    激活、再按退出。与全屏放大同款 hotkey_row。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("快捷键", pip_hk_expanded)
                .body(Element::col().child(card::hotkey_row(
                    "快捷键",
                    m.hk_pip_toggle,
                    HotkeyField::PipToggle,
                    m.hotkey_capture,
                    col1_w,
                )))
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 位置（子卪：方位下拉 + 两向偏秿，与击键回显的「位置」同型）──
            // 偏移是屏幕宽/高的百分比（0–50）：贴边时把面板从边上往屏内挪。
            // 只有两根而不是四根：方位已经决定了贴哪条边，四根里永远有两根
            // 是无效项；垂直偏移抬高即可让开任务栏（2026-09-21 用户报告）。
            // 鼠标跟随没有"贴边"可言，偏移不生效（面板跟着鼠标走）。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("位置", pip_pos_expanded)
                .body(
                    Element::col()
                        .child(choice_row(
                            "位置",
                            &["鼠标跟随", "左上", "右上", "左下", "右下"],
                            m.pip_anchor,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "水平偏移",
                            m.pip_offset_x,
                            m.pip_offset_x_text,
                            0.0,
                            50.0,
                            0,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "垂直偏移",
                            m.pip_offset_y,
                            m.pip_offset_y_text,
                            0.0,
                            50.0,
                            0,
                            col1_w,
                        )),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 显示（子卪：宽高 / 倍率 / 边框色——"长什么样"的事都在这里）──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("显示", pip_display_expanded)
                .body(
                    Element::col()
                        .child(card::slider_row(
                            "面板宽",
                            m.pip_panel_w,
                            m.pip_panel_w_text,
                            PIP_PANEL_MIN,
                            PIP_PANEL_MAX,
                            2,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "面板高",
                            m.pip_panel_h,
                            m.pip_panel_h_text,
                            PIP_PANEL_MIN,
                            PIP_PANEL_MAX,
                            2,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "放大倍率",
                            m.pip_zoom,
                            m.pip_zoom_text,
                            PIP_ZOOM_MIN as f32,
                            PIP_ZOOM_MAX as f32,
                            0,
                            col1_w,
                        ))
                        .child(card::color_row("边框色", m.pip_border_color, col1_w)),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(pip_title)
        .child(pip_body.visible_when(move || pip_exp.get()));

    // ── 全屏放大（与画中画同一套底层：面板 = 整个虚拟屏）──
    let fs_expanded = m.expand("other.magnifier.full");
    let (fs_title, fs_body, fs_exp) = FeatureCard::new("全屏放大", fs_expanded)
        .switch(m.fs_enabled)
        .body(
            Element::col()
                .child(card::hotkey_row(
                    "快捷键",
                    m.hk_fullscreen_zoom,
                    HotkeyField::FullscreenZoom,
                    m.hotkey_capture,
                    col1_w,
                ))
                .child(card::slider_row(
                    "放大倍率",
                    m.fs_zoom,
                    m.fs_zoom_text,
                    FULLSCREEN_ZOOM_MIN as f32,
                    FULLSCREEN_ZOOM_MAX as f32,
                    0,
                    col1_w,
                )),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(fs_title)
        .child(fs_body.visible_when(move || fs_exp.get()));

    // ── 抓取三卡：截图与贴图 / GIF录制 / 视频录制 ──
    // 三张卡的组装在 `capture`：按"操作对象是屏幕画面"归到本页（理由见本文件头注释）。
    content = content.child(capture::build_capture_section(m, col1_w));

    Element::scroll().child(content.padding(16))
}

#[cfg(test)]
mod tests {
    /// 本文件的**生产代码**文本（`#[cfg(test)]` 之前那一段）。
    /// 同款先例见 `runtime::layer::gif_trim::tests::production_src`——
    /// `include_str!` 会把测试自己也包进来，必须切一刀，否则"数出现次数"
    /// 的断言会被断言自己的文本影响。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("screen.rs");
        let cut = SRC.find("#[cfg(test)]").unwrap_or(SRC.len());
        &SRC[..cut]
    }

    /// 结构门禁：倍率调节是**内置快捷键**（Ctrl+Win+滚轮，见
    /// `platform::input::mouse` 的 WM_MOUSEWHEEL 注释），面板不得再出现任何
    /// 倍率快捷键配置行（2026-09-21 用户拍板"不需要暴露给用户"）。
    ///
    /// 为什么用结构门禁：面板行的渲染是无头环境观察不到的事；行一旦回潮，
    /// ★ 结构门禁：画中画卡**不得回潮**旧的「倍率+ / 倍率−」快捷键行。
    ///
    /// 为什么用结构门禁：面板行的渲染是无头环境观察不到的事；行一旦回潮，
    /// 用户就会在设置里看到一个改了也不生效的键位框（旧版 Ctrl+Alt+=/- 正是
    /// 这样一副"看着能配、实际无效"的死态）。
    /// ★ 2026-09-25「画中画放大」从工具页搬进本页，这条门禁跟着卡走。
    /// ★ 2026-09-26 起「启停快捷键」（`HotkeyField::PipToggle` / `hk_pip_toggle`）
    ///   是**合法**的独立快捷键（一键开关，与倍率无涉），不再属于禁列。
    #[test]
    fn zoom_hotkey_rows_are_gone_from_the_pip_card() {
        let p = production_src();
        assert!(
            !p.contains("倍率+"),
            "「倍率+」快捷键行必须移除：缩放已改为内置 Ctrl+Win+滚轮"
        );
        assert!(
            !p.contains("zoom_in_hotkey") && !p.contains("zoom_out_hotkey"),
            "倍率增减快捷键的配置字段必须保持删除状态"
        );
    }
}
