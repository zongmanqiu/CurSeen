//! 键盘页。
//!
//! 三张**功能父卡**（2026-09-22 用户拍板顺序：击键音效提到虚拟键盘之前 —— 音效是
//! "按下去有什么反馈"、与击键回显同族，虚拟键盘是另一件东西，放最后）：
//!
//! ```text
//! [✓] 击键回显                     [✓] 击键音效  [脚本 ▾]      [✓] 虚拟键盘  [方案 ▾]
//!     ├ 位置   方位下拉 + 四向偏移     ├ 音效文件/音效目录 + 音量    ├ 位置   方位下拉 + 四向偏移
//!     ├ 显示   鼠标按键 + 五个数值         └ 脚本声明的参数行           ├ 交互   鼠标穿透
//!     └ 颜色   四个色块                                            ├ 显示   键帽尺寸 / 圆角 / …
//!                                                                 └ 颜色   方案声明的全部配色
//! ```
//!
//! # 命名规则
//!
//! **子卡名 = 一个用户能自己回答的问题**，彼此不重叠：位置（在哪儿）/ 显示（显示什么、
//! 多大、多久）/ 颜色（什么颜色）/ 交互（鼠标怎么对待它 —— 可不可点）。卡名一律不带"设置"
//! 后缀 —— 整页都是设置，后缀不承载信息。旧名「位置设置 / 外观设置 / 颜色设置 / 显示内容」
//! 已按此规则重构。两张父卡里同名的子卡**同形**（两个「位置」都是方位下拉 + 四向偏移），
//! 同一个名字在同一页里只学一次。
//!
//! # 「显示 / 颜色」的边界按**客观判据**（是不是颜色）划
//!
//! 虚拟键盘的子卡边界反转过三次，落点都是同一条：**按什么切**。
//!
//! 1. 按"主观刀口"切四张卡（外观 / 配色 …）→「圆角」被挪进颜色卡、「字号 / 不透明度」被
//!    拆到第三张卡，用户看着像 JSON 里的选项丢了一堆。
//! 2. 合成一张「方案配置」→ 边界说得通（都是所选方案 JSON 的属性），但**长度无上界**：
//!    实测 `ansi_104` / `ansi_60` 是 10 项、`ansi_104_dark` 是 21 项，方案作者每多声明一个
//!    命名样式就再长 ≥2 项，一张卡翻不到底。
//! 3. 现在切两张，判据客观：`ControlGroup::Look`（数值项 —— 键帽大小 / 空隙 / 圆角 /
//!    不透明度 / 高亮保持 / 字号 / 命名样式圆角）叫「显示」，`ControlGroup::Color`
//!    （兜底色 + 命名样式色）叫「颜色」。分组只由代码里的 `ControlGroup` 决定，
//!    不在页面里按标签猜，也不再有长度无上界的单卡。
//!
//! 两个「颜色」**同名不同源**：击键回显的是四个应用固定色（数量恒定、与任何"方案"无关），
//! 虚拟键盘的由方案 JSON 声明（几个命名样式就有几个色块）。同名是有意的 —— 用户要找
//! "颜色"时，两个功能下它都在同一个位置。
//!
//! # 开关架构（2.0）
//!
//! 每个功能的唯一开关在其**父卡标题行**上（→ `key_display.enabled` /
//! `virtual_keyboard.enabled`）；子卡是同一功能的参数面，不重复绑开关。

use windui::prelude::*;

use crate::modules::virtual_keyboard::vk_config::ControlGroup;
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{self, PanelModel, VkControlSignal};

use super::trail::build_param_cell;

// 数值滑块行已收拢进 `card::slider_row`（2026-09-25 §35 起为「滑条 + 数值框」
// 三件套唯一正本：全软件所有数值条同一样式）。

/// 生成**方案控件**的行，只出 `group` 这一组。
///
/// 每组各成一张子卡：`Look` →「显示」，`Color` →「颜色」。分组判据在 `ControlDef::new`：
/// 颜色（`is_color`）归 `Color`，其余数值项（键帽大小 / 空隙 / 圆角 / 不透明度 / 高亮保持 /
/// 字号）一律归 `Look`。分类**不按"外观 / 配色"这种主观刀口**切，也不再合并成一张长卡。
///
/// 卡内行序由 `controls_of` 的天然顺序给（末尾那次 `group_radii` 会把命名样式的圆角提到
/// 全局「圆角」后面，于是「显示」卡里是"先通用尺寸、后命名样式圆角"）。一卡一组之后，页面
/// 对行序**不再有任何承诺** —— `controls_of` 调整顺序只会让卡内行序跟着变，不会串到别的卡。
///
/// 为什么不是"只给当前方案建一组"：下拉切换方案时页面并不重建（面板只在打开时建一次
/// 元素树），只建一组的话切完方案还是旧方案的控件。多建几组 + `visible_when` 门控，
/// 换方案即刻生效，代价只是几个隐藏的信号。
fn control_rows(m: &PanelModel, group: ControlGroup, col1_w: i32) -> Element {
    // 闭包要 `move`，而 `m` 是借用 —— 只把"当前方案下标"这个 Copy 句柄带进去。
    let layout = m.vk_layout;
    let mut col = Element::col();
    for (i, sc) in m.vk_controls.iter().enumerate() {
        for it in &sc.items {
            if it.def.group != group {
                continue;
            }
            // ★ 2026-09-25 统一 UI：控件形状全部走 `card` 正本函数（slider_row 三件套
            //   / color_row 取色器 / checkbox），与鼠标页逐字节同款——本页不再自己拼控件。
            let row = match it.sig {
                VkControlSignal::Range(sig, text) => {
                    let (min, max, decimals) = match it.def.kind {
                        crate::modules::virtual_keyboard::vk_config::ControlKind::Range {
                            min,
                            max,
                            decimals,
                            ..
                        } => (min, max, decimals),
                        _ => continue,
                    };
                    // ★ 三件套（数值条 + 数值框）：与鼠标页脚本参数同款正本函数。
                    card::slider_row(&it.def.label, sig, text, min, max, decimals, col1_w)
                }
                VkControlSignal::Color(sig) => card::color_row(&it.def.label, sig, col1_w),
                VkControlSignal::Bool(sig) => {
                    card::option_row(&it.def.label, Element::checkbox("", sig), col1_w)
                }
            };
            col = col.child(row.visible_when(move || layout.get() == i));
        }
    }
    col
}

pub fn build_key_display_page(m: &PanelModel) -> Element {
    // 展开态来自模型（落盘 + 各卡独立），id 见 `core::config::CARD_IDS`。
    let kd_expanded = m.expand("key_display");
    let kd_pos_expanded = m.expand("key_display.pos");
    let kd_display_expanded = m.expand("key_display.display");
    let kd_color_expanded = m.expand("key_display.color");
    // 虚拟键盘：一张父卡（开关 + 方案下拉在标题行）+ 四张子卡
    // 「位置 / 窗口行为 / 显示 / 颜色」，各自独立展开。
    let vk_expanded = m.expand("virtual_keyboard");
    let vk_pos_expanded = m.expand("virtual_keyboard.pos");
    let vk_window_expanded = m.expand("virtual_keyboard.window");
    let vk_look_expanded = m.expand("virtual_keyboard.look");
    let vk_color_expanded = m.expand("virtual_keyboard.color");

    // 列宽按**本页全部卡片名 + 全部方案控件名**里最长的那个算：虚拟键盘「显示 / 颜色」
    // 两卡的行标签是方案声明出来的，长短全看 JSON，只按卡片名算会让这两卡的控件列差一截
    // 列1宽度：全项目唯一常量（2026-09-25 第六定案），所有页面同一条对齐线。
    let col1_w = card::COL1_W;

    let mut content = Element::col();

    // ── 击键回显（父卡：开关 + 模式下拉在标题行；下面三张子卡）──
    //
    // ★ 2026-09-22 用户口径：把「显示」子卡里的**模式**下拉搬到标题栏 —— 它是本功能的
    // 功能级选择器（显示哪些键），与「虚拟键盘」标题栏的"方案"下拉同层级、同位置；
    // 沉在两层子卡里要展开两次才能改，也不该跟尺寸类参数混在一张卡。
    // 方位下拉仍留在「位置」子卡（它是位置参数，本卡与「虚拟键盘」的两张「位置」子卡
    // 因此严格同形），且 `key_display.pos` 在 `default_card_expanded` 里，打开面板即展开。
    let (kd_title, kd_body, kd_exp) = FeatureCard::new("击键回显", kd_expanded)
        .switch(m.enabled_key)
        .title_dropdown(
            model::DISPLAY_MODES_LABELS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            m.display_mode,
        )
        .body({
            let mut col = Element::col();
            // ── 位置（子卡：方位下拉 + 四向偏移，与虚拟键盘的「位置」同形）──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("位置", kd_pos_expanded)
                .body(
                    Element::col()
                        .child(card::choice_row(
                            "位置",
                            model::PLACEMENTS_LABELS
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                            m.placement,
                            col1_w,
                        ))
                        // ☆ 与画中画放大同型：两根偏移（屏幕百分比 0–50）。
                        // 方位已决定贴哪条边，四根里有两根永远无效；
                        // 运行时把两根信号映射到方位选中的那两根字段上。
                        .child(card::slider_row(
                            "水平偏移",
                            m.offset_left,
                            m.offset_left_text,
                            0.0,
                            50.0,
                            0,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "垂直偏移",
                            m.offset_bottom,
                            m.offset_bottom_text,
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
            // ── 显示（子卡：弹窗尺寸 / 不透明度 / 淡出 / 合并 / 字号）──
            //
            // 由旧「显示内容」+ 旧「外观设置」合并而来（后者名叫"外观"却装着淡出延迟、
            // 合并间隔与字号，名不副实），合并后这张卡回答的是同一个问题 ——
            // "多大、多透明、留多久"。
            //
            // ★ 2026-09-22：原来的「模式」下拉已搬到**父卡标题栏**（与「虚拟键盘」的方案
            //   下拉同位置、同形式）。它是这份功能的**功能级选择器**（显示哪些键），
            //   跟"位置 / 显示 / 颜色"三张子卡不是一个层级；沉在子卡里既要点开两层才看得见，
            //   又让本卡的第一行（模式）与第二行（尺寸类参数）语义不同类。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("显示", kd_display_expanded)
                .body(
                    Element::col()
                        // ★ 旧「鼠标按键」开关已删（2026-09-25 用户口径）：显示模式
                        //   下拉的「排除鼠标」已覆盖这个意图，两处开关会互相打架。
                        .child(card::slider_row(
                            "弹窗宽度",
                            m.popup_width,
                            m.popup_width_text,
                            5.0,
                            50.0,
                            0,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "不透明度",
                            m.key_opacity,
                            m.key_opacity_text,
                            0.05,
                            1.0,
                            2,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "淡出延迟",
                            m.fade_delay,
                            m.fade_delay_text,
                            1.0,
                            30.0,
                            0,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "合并间隔",
                            m.merge_interval,
                            m.merge_interval_text,
                            0.5,
                            10.0,
                            1,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "字号",
                            m.font_size,
                            m.font_size_text,
                            8.0,
                            72.0,
                            0,
                            col1_w,
                        )),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 颜色（子卡：四个应用固定色）──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("颜色", kd_color_expanded)
                .body(
                    Element::col()
                        .child(card::color_row("字体色", m.color_font, col1_w))
                        .child(card::color_row("功能键色", m.color_fun, col1_w))
                        .child(card::color_row("符号色", m.color_symbol, col1_w))
                        .child(card::color_row("背景色", m.color_bg, col1_w)),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(kd_title)
        .child(kd_body.visible_when(move || kd_exp.get()));

    // ── 击键音效（模仿鼠标页「拖尾效果」的卡：开关 + 标题行脚本下拉）──
    // 脚本下拉列 `assets/lua` 里 `role="keysound"` 的脚本（面板每次打开重扫目录，
    // 丢新脚本即可选到）。★ 2026-09-25 改版：音源统一为脚本参数里的 `file` 浏览
    // 按钮，浏览即引用：程序目录内记便携相对路径，外部记绝对路径，无中转目录），
    // 原「音效文件夹」「音效文件」两行已删——统一音效挑一个音、分类音效五类各挑
    // 一个，都在各自脚本的参数区完成。
    let ksound_expanded = m.expand("key_sound");
    let ksound_catalog = m.ksound_catalog.clone();
    let ksound_script = m.ksound_script;
    let (ks_title, ks_body, ks_exp) = FeatureCard::new("击键音效", ksound_expanded)
        .switch(m.ksound_enabled)
        .title_dropdown_signal(ksound_catalog.names, ksound_script)
        .body(
            Element::col()
                .child(card::slider_row(
                    "音量",
                    m.ksound_volume,
                    m.ksound_volume_text,
                    0.0,
                    100.0,
                    0,
                    col1_w,
                ))
                // ★ 脚本参数段（2026-09-21 对齐拖尾机制）：下面的选项行来自**当前脚本
                //   schema() 的声明**——`file` 参数渲染成「标签 + 浏览按钮 + 文件名」，
                //   选不同脚本显示不同选项。改参数只存进该脚本自己的配置段，切换脚本、
                //   重启都不丢。
                .child({
                    // Signal 是 Copy：闭包捕副本，别在 move 闭包里通过 `m` 访问（引用会逃逸）。
                    let hk_cap = m.script_hotkey_cap;
                    Element::host_signal(m.ksound_params, move |c| {
                        build_param_cell(c, hk_cap, col1_w)
                    })
                }),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(ks_title)
        .child(ks_body.visible_when(move || ks_exp.get()));

    // ── 虚拟键盘（父卡：勾选框 +「虚拟键盘」+ 方案下拉都在标题行；下面四张子卡）──
    //
    // 标题行三者同排，与原布局一致。父卡本身可折叠，展开后是四张**没有勾选框**的子卡：
    // 位置 / 窗口行为 / 显示 / 颜色。整组缩进由父卡 `body_indent` 统一给
    // （`card::INDENT_CARD`），子卡缩进 0 —— 缩进全靠父卡，子卡因此填满父内容区，
    // 右边缘与父标题栏对齐（见 `card::FeatureCard::build_title_row`）。
    //
    // 「显示 / 颜色」两卡的每一行都是**方案声明出来的**（`*.json` 的 `controls` 块；
    // 没声明则由代码补齐一套预设，见 `vk_config::controls_of`）—— 与 Lua 脚本暴露参数
    // 输入是同一套思路：方案作者决定暴露哪些选项、叫什么、默认多少、范围多大。
    // 切方案时整批换掉（`vk_layout` 的下标就是开关），换套方案再换回来，各自的调色还在。
    let (vk_title, vk_body, vk_exp) = FeatureCard::new("虚拟键盘", vk_expanded)
        .switch(m.vk_enabled)
        // 下拉的选项 = **方案目录扫描结果**（`assets/VirtualKeyboards/*.json` 加出厂内嵌
        // 兜底）。用户往目录里丢一个方案，重开面板就能在这里看到并选中。
        .title_dropdown(
            m.vk_schemes.iter().map(|e| e.label.clone()).collect(),
            m.vk_layout,
        )
        .body({
            let mut col = Element::col();
            // ── 位置（子卡：四个方位的下拉 + 四向偏移）──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("位置", vk_pos_expanded)
                .body(
                    Element::col()
                        .child(card::choice_row(
                            "位置",
                            model::PLACEMENTS_LABELS
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                            m.vk_placement,
                            col1_w,
                        ))
                        // ☆ 与击键回显同型：两根偏移（屏幕百分比 0–50），
                        // 运行时映射到方位选中的那两根字段（另两根恒 0）。
                        .child(card::slider_row(
                            "水平偏移",
                            m.vk_offset_left,
                            m.vk_offset_left_text,
                            0.0,
                            50.0,
                            1,
                            col1_w,
                        ))
                        .child(card::slider_row(
                            "垂直偏移",
                            m.vk_offset_bottom,
                            m.vk_offset_bottom_text,
                            0.0,
                            50.0,
                            1,
                            col1_w,
                        )),
                )
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 交互（子卡：鼠标穿透 —— 虚拟键盘私有属性）──
            //
            // 「总在最上 / 全屏避让」已搬去全局页的「图层覆盖」卡（2026-09-27 改名，原叫
            // 「覆盖层」）：它们管的是整层覆盖层的 z 序（拖尾 / 击键回显 / 虚拟键盘在同一
            // 层），对所有功能生效。这张卡只剩鼠标穿透（键盘可不可点）——它是交互问题，
            // 不叫「窗口行为」，叫「交互」。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("交互", vk_window_expanded)
                .body(Element::col().child(card::option_row(
                    "鼠标穿透",
                    Element::checkbox("", m.vk_click_through),
                    col1_w,
                )))
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 显示（子卡：方案声明的**数值**项 —— 键帽尺寸 / 圆角 / 透明度 / 字号）──
            //
            // 与「击键回显 → 显示」同名：同一个问题（"显示成什么样"）在两处都叫这个名字。
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("显示", vk_look_expanded)
                .body(control_rows(m, ControlGroup::Look, col1_w))
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            // ── 颜色（子卡：方案声明的全部配色 —— 兜底色 + 命名样式色）──
            let (sub_title, sub_body, sub_exp) = FeatureCard::new("颜色", vk_color_expanded)
                .body(control_rows(m, ControlGroup::Color, col1_w))
                .build_title_row(col1_w, 0);
            col = col
                .child(sub_title)
                .child(sub_body.visible_when(move || sub_exp.get()));
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(vk_title)
        .child(vk_body.visible_when(move || vk_exp.get()));

    Element::scroll().child(content.padding(16))
}
