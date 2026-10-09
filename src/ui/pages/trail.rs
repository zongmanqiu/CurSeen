//! 鼠标特效页面。
//!
//! ★★ **命名（2026-09-28 用户口径）**：这个功能在界面上一律叫**「点击效果」**——
//! 它早先只有"波纹"一种形态，后来加进了喷泉、星芒、随机文字等，名字不能还钉在
//! 一种形态上。代码里沿用的 `ripple` / 裸"波纹"是**内部历史名**（配置字段名
//! `ripple_bindings`、`ripple_enabled`，卡 id `ripple`），改它们会让老 `config.json`
//! 失效，故只换用户看得见的文案，不换标识。
//!
//! 四张功能卡片（拖尾 / 悬浮标 / 点击效果 / 点击声音），每张都是「开关 + 选项 + 参数」。
//! ★ 点击效果 2026-09-25 晚定稿：**父卡 + 五键子卡**——父卡标题栏总开关；子卡标题栏 =
//!   键名 +「关闭/效果」下拉（首项「关闭」= 该键停用，先例：录屏音频来源下拉 0=关闭），
//!   正文 = 该键该效果的参数。★ 子卡**没有开关**（启停住进下拉首项），全项目其它
//!   子卡也不带开关，口径一致。
//! ★ 点击声音保持**平铺单卡**：每键只有一行「关闭/文件」下拉、没有子卡——它没有
//!   每键参数，不需要分组容器。
//! 所有控件绑 [`PanelModel`] 里的信号，由 `Runtime` 每帧 diff 回 `Config`——页面本身
//! 不碰配置，也不做持久化。

use windui::prelude::*;

use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{self, EffectParamCell, PanelModel};

/// 把一个动态参数单元渲染成「标签 + 控件」一行。
///
/// 控件形态由单元类型决定（与 `EffectParamCell` 变体一一对应）：
/// Float/Int → 滑块+输入框三件套（2026-09-25 起，脚本参数也有数值输入框了）、
/// Bool → 复选框、Color → 颜色选框、Choice → 下拉、Image → 路径框。
/// `host_signal` 在"选中特效变化、参数单元整批重建"时按此函数重新生成整段控件。
///
/// `col1_w` 由调用点用闭包捕获（见各 `host_signal`）：脚本参数的控件列也要落在**所在
/// 卡片**的控件列上，子卡（五键波纹）与一级卡一样。
///
/// `hk_cap` = 脚本热键捕获态信号（[`PanelModel::script_hotkey_cap`]）：`Hotkey`
/// 单元的捕获框点下后把目标写进去，`Runtime::try_capture_hotkey` 据此拦截按键回写。
pub(crate) fn build_param_cell(
    cell: EffectParamCell,
    hk_cap: Signal<Option<model::ScriptHotkeyCap>>,
    col1_w: i32,
) -> Element {
    match cell {
        EffectParamCell::Float {
            label,
            sig,
            text,
            min,
            max,
            decimals,
            ..
        } => card::slider_row(&label, sig, text, min, max, decimals, col1_w),
        EffectParamCell::Int {
            label,
            sig,
            text,
            min,
            max,
            ..
        } => card::slider_row(&label, sig, text, min, max, 0, col1_w),
        EffectParamCell::Bool { label, sig, .. } => {
            card::option_row(&label, Element::checkbox("", sig), col1_w)
        }
        EffectParamCell::Choice {
            label,
            sig,
            choices,
            labels,
            ..
        } => {
            // 界面显示中文（`labels`），落盘与喂给脚本的仍是 `choices` 里的稳定标识。
            let shown = if labels.len() == choices.len() && !labels.is_empty() {
                labels
            } else {
                choices
            };
            // 宽度/截断标准统一由 `card::choice_row` 正本给（右缘对齐 + 前置 …）。
            card::choice_row(&label, shown, sig, col1_w)
        }
        // 音效下拉：与 Choice 同控件，**候选项由宿主按「音效目录」扫出来**（脚本写不出
        // 目录列表）。界面显示 `stem.ext`，落盘的是短名或绝对路径（见 `ui::sfx`）。
        EffectParamCell::Sound {
            label,
            sig,
            choices,
            labels,
            ..
        } => {
            let shown = if labels.len() == choices.len() && !labels.is_empty() {
                labels
            } else {
                choices
            };
            // 显示名在 `ui::sfx` 构造处已截断；choice_row 内的二次截断幂等无害。
            card::choice_row(&label, shown, sig, col1_w)
        }
        // 「音效目录」：浏览文件夹（Vista+ 的 IFileOpenDialog），挑完同一脚本里的音效
        // 下拉整批重建。显示的是**完整路径**（`sound_dir_display`：没挑过给出厂目录的
        // 真实路径），超宽由 `browse_row` 收成 `…`。
        // 形状与其它浏览类行一致（标签 + 按钮 + 路径），见 `card::browse_row`。
        EffectParamCell::Folder { label, sig, .. } => card::browse_row(
            &label,
            sig,
            col1_w,
            crate::ui::sfx::sound_dir_display,
            move |s| {
                if let Some(dir) = crate::platform::file_dialog::open_folder() {
                    // ★ 落盘前便携化：目录在程序目录内就记相对路径（换台电脑仍有效）。
                    s.set(crate::core::paths::to_app_relative(&dir));
                }
            },
        ),
        EffectParamCell::Color {
            label, sig, alpha, ..
        } => card::option_row(
            &label,
            // ★ 透明度条由脚本声明决定：schema 里写了 `alpha = true` 才有（落盘 8 位
            //   `#RRGGBBAA`，脚本端 `hex_to_rgb` 第四个返回值拿到它）；否则颜色恒为 6 位
            //   `#RRGGBB`，透明度交给脚本自己的「不透明度」参数（见 `model::fmt_color`）。
            // 取色器与标题栏下拉同宽（右缘对齐标准，2026-09-25）。
            Element::color_picker_opts(sig, ColorPickerOpts::default().alpha(alpha))
                .width(card::CONTROL_GROUP_W),
            col1_w,
        ),
        EffectParamCell::Image { label, sig, .. } => {
            // 本期仅契约：显示路径输入框，真正的图片加载/绘制留待接入悬浮标时实现。
            // 宽度对齐快捷键框（2026-09-29 用户反馈：路径/文本框太窄放不下 URL）。
            card::option_row(
                &label,
                Element::text_input(sig, "图片路径").width(card::CONTROL_GROUP_W),
                col1_w,
            )
        }
        EffectParamCell::Str { label, sig, .. } => {
            // ★ 自由文本：可编辑文本框，值原样输入（Lua 扩展脚本的 api_key/模型名等）。
            // 宽度对齐快捷键框（CONTROL_GROUP_W）：API 地址这类 URL 一行放得下。
            card::option_row(
                &label,
                Element::text_input(sig, "输入文本…").width(card::CONTROL_GROUP_W),
                col1_w,
            )
        }
        EffectParamCell::File { label, sig, .. } => {
            // ★ 音频文件浏览（2026-09-25 二次改版：浏览即引用，不再复制中转）：
            //   选哪个文件就记哪个文件的路径——程序目录内的记**相对路径**（便携，
            //   换台电脑仍有效），外部文件记**绝对路径**。解析端
            //   `key_sound::stored_file_path`（绝对 → 便携相对 → 出厂 Sounds）
            //   与显示端 `sound_file_display` 是同一套，看到的路径就是会响的文件。
            card::browse_row(
                &label,
                sig,
                col1_w,
                crate::ui::sfx::sound_file_display,
                move |sig| {
                    if let Some(src) = crate::platform::file_dialog::open_audio() {
                        sig.set(crate::core::paths::to_app_relative(&src));
                    }
                },
            )
        }
        EffectParamCell::Hotkey { label, sig, .. } => {
            // ★ 快捷键捕获框（2026-09-29）：与屏幕标注快捷键同一套视觉与交互
            //   （card::hotkey_row_script ↔ hotkey_row 共用一个视觉正本）。捕获态
            //   目标 = 本单元的 sig（ScriptHotkeyCap），`Runtime::try_capture_hotkey`
            //   拦到按键写回；互斥（与固定字段、其它脚本热键）在模型侧统一做。
            //   ★ 派发语义只对 `scripts/` 独立脚本生效（Runtime::script_hotkeys
            //     只认该前缀）；特效脚本声明它只是存键位串，不会被当全局热键。
            card::hotkey_row_script(&label, sig, model::ScriptHotkeyCap { sig }, hk_cap, col1_w)
        }
    }
}

pub fn build_trail_page(m: &PanelModel) -> Element {
    // ★ 展开态一律从模型取：`m.expand(id)`（id 见 `core::config::CARD_IDS`）。
    // 它是**每张卡一个独立信号**且会落盘，所以既能"只展开点的那一张"，也能在下次
    // 打开面板时还原现场。这里别再自己 `signal(...)`——本地建的信号不落盘，一关就丢。
    let trail_expanded = m.expand("trail");
    let hover_expanded = m.expand("hover");
    // ★ 2026-09-22 用户口径：「点击效果」「点击声音」也要能折叠（此前是 `.always_open()`）。
    //   与其它父卡同款 —— id 进 `CARD_IDS`、信号会落盘、出厂默认展开
    //   （见 `core::config::default_card_expanded`），所以观感与"恒展开"时一致。
    let ripple_expanded = m.expand("ripple");
    let sound_expanded = m.expand("sound");

    // 把 `m` 上的信号拷成本地副本：Signal 是 Copy，但 `visible_when` / `on_click` 的闭包要求
    // `'static`，直接捕获 `m.xxx` 会试图带走 `m` 的借用 → 编译期"数据逃逸"。拷到本地后
    // 闭包只捕获这个 'static 副本。
    let hover_image = m.hover_image;
    // 图片导入后落在素材目录里（配置只记文件名），显示完整路径时要补上它。
    let hover_asset_dir = crate::core::paths::hoverdot_dir();
    let hover_catalog = m.hover_catalog.clone();
    let hover_type = m.hover_type;

    // 列1宽度：全项目唯一常量（2026-09-25 第六定案），所有页面同一条对齐线。
    let col1_w = card::COL1_W;
    // 脚本热键捕获态信号：Signal 是 Copy，各 host_signal 闭包各捕一份。
    let hk_cap = m.script_hotkey_cap;

    let mut content = Element::col();

    // ── 拖尾效果 ──
    // 效果下拉走信号表（`m.catalog.names`）：每次打开面板都重新扫一遍
    // `assets/MouseTrail/`，用户新放进目录的 Lua 效果下次开面板即可选到。
    //
    // 参数段不再写死：每段参数都来自脚本 `schema()`，由 `m.effect_params` 这个信号表驱动。
    // 选中特效变化（或开关切到别的脚本）时 `apply_to` 整批重建 `m.effect_params`，
    // `host_signal` 据此重建下面整段控件——每个脚本的"自己那套参数"就直接长成了主窗口 UI。
    let (title, body, exp) = FeatureCard::new("拖尾效果", trail_expanded)
        .switch(m.trail_enabled)
        .title_dropdown_signal(m.catalog.names, m.trail_effect)
        .body(Element::host_signal(m.effect_params, move |c| {
            build_param_cell(c, hk_cap, col1_w)
        }))
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 悬浮标 ──
    // ★ 每类型一个独立脚本（assets/HoverDot/<type>.lua）：下拉列出目录里的全部脚本，
    // 参数区由脚本 schema 驱动（与拖尾特效同一套 host_signal 机制）；png 类型的图片
    // 导入是专属行（选图 → 复制进本目录 → 配置只记文件名）。
    let (title, body, exp) = FeatureCard::new("悬浮标", hover_expanded)
        .switch(m.hover_enabled)
        .title_dropdown_signal(hover_catalog.names, m.hover_type)
        .body(
            Element::col()
                // 图片：浏览类行的统一形状（标签 + 浏览按钮 + 文件名），见 `card::browse_row`；
                // 只在「图片」类型下出现，与「缩放」「透明度」两条参数互不干扰。
                .child(
                    // 图片：与其余参数行同构——标签在左、浏览按钮落在控件列（与滑块/下拉
                    // 同一 x）、文件名在按钮右侧（`card::browse_row`：所有浏览类行同一形状）。
                    // 选图 → 复制进本目录 → 配置只记文件名；按原始宽高贴出、鼠标对准中心。
                    // ★ 显示**完整路径**（`card::path_display`：短名补成素材目录的绝对
                    //   路径）——只给文件名的话，用户不知道这张图到底落在哪儿。
                    card::browse_row(
                        "图片",
                        hover_image,
                        col1_w,
                        move |v| card::path_display(v, &hover_asset_dir),
                        move |sig| {
                            if let Some(src) = crate::platform::file_dialog::open_image() {
                                let fname = src
                                    .file_name()
                                    .map(|f| f.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                let dest = crate::core::paths::hoverdot_dir().join(&fname);
                                if let Some(parent) = dest.parent() {
                                    let _ = std::fs::create_dir_all(parent);
                                }
                                // 复制到素材目录，配置只记文件名（与素材一起搬家不失效）。
                                // 复制失败则退化为记绝对路径，controller 也能直接加载。
                                if std::fs::copy(&src, &dest).is_ok() {
                                    sig.set(fname);
                                } else {
                                    sig.set(src.to_string_lossy().into_owned());
                                }
                            }
                        },
                    )
                    .visible_when(move || hover_catalog.id_at(hover_type.get()) == "png"),
                )
                // 当前类型的参数（脚本 schema 驱动；换类型整批重建）。
                .child(Element::host_signal(m.hover_params, move |c| {
                    build_param_cell(c, hk_cap, col1_w)
                })),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 点击效果（父卡 + 五键子卡，2026-09-25 晚定稿）──
    //
    // 形态沿革：① 父卡+五键子卡（各带开关）→ ② 平铺单卡 + 标题栏按键选择器 →
    // ③ 平铺行 + 参数挂行下 → **④ 本版：回到①的骨架，但子卡标题栏的「开关」换成
    // 「关闭/效果」下拉**（首项「关闭」= 该键停用，与声音卡每键行同语义）。
    // 用户拍板：平铺行选完效果后参数直接出现，与其它键没有明显间隔——子卡的
    // 标题行就是天然的视觉分隔，参数归属一目了然。
    //
    // 父卡标题行 = 总开关（`trail.ripple_enabled`，AND 闸）；子卡 = 键名 +
    // 「关闭/效果」下拉（数据源 `assets/lua/ClickFx/`，丢新脚本重开面板即可选）
    // + 正文参数（脚本 schema 驱动，`host_signal` 换效果整批重建）。
    // ★ 子卡**没有开关**——启停就住在下拉首项「关闭」里；全项目其它子卡也不带
    //   开关，口径一致。
    // 键行短名（声音卡五行）；子卡标题用带"波纹"后缀的长名（见下）。
    let key_labels = ["左键", "右键", "中键", "侧键1", "侧键2"];
    let key_names = ["左键效果", "右键效果", "中键效果", "侧键1效果", "侧键2效果"];
    let (title, body, exp) = FeatureCard::new("点击效果", ripple_expanded)
        .switch(m.ripple_master)
        .body({
            let mut col = Element::col();
            for (i, name) in key_names.iter().enumerate() {
                let ks = &m.ripple[i];
                // 每张子卡各取自己的展开信号。⚠️ 别写 `[signal(false); 5]`：数组重复
                // 语法只求值一次，五张子卡会共用同一个句柄 → 点一个全部展开。
                let kexp = m.expand(&format!("ripple.{}", model::RIPPLE_KEYS[i]));
                let combo = ks.combo;
                let (sub_title, sub_body, sub_exp) = FeatureCard::new(name, kexp)
                    .title_dropdown(m.ripple_choices.clone(), ks.combo)
                    .body(Element::host_signal(ks.clickfx_params, move |c| {
                        build_param_cell(c, hk_cap, col1_w)
                    }))
                    // 子卡自身不缩进：缩进由父卡 `body_indent` 统一给（24px）。
                    // ★ 2026-09-25 第四定案：子卡标题栏不再放假开关占位，子卡标题
                    //   文字落在 父缩进24+内边距16 = +40，与主卡第一个正文子同一条
                    //   对齐线；下拉 x = 40+col1_w，与主卡标题栏下拉同 x。
                    .build_title_row(col1_w, 0);
                col = col
                    .child(sub_title)
                    // 折叠子卡连参数带下拉正文一起收起（下拉在标题栏，始终可见）。
                    .child(sub_body.visible_when(move || sub_exp.get() && combo.get() > 0));
            }
            col
        })
        // 整组缩进由父卡 body_indent 统一给（24px），五张子卡左缘/右缘各内收 24。
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 点击声音（单卡，2026-09-25 重构）──
    //
    // 标题行 = 开关（总闸 → `trail.sound_enabled`）；正文 = **共享音量**一行 +
    // 五行「按键 [关闭/文件…]」。每键差异只剩"响哪个文件"（音量五键共用，1.0 本就是
    // 全局音量）；下拉首项「关闭」= 该键停用。目录来自 `assets/Sounds/`——
    // 往目录里丢一个新文件（wav/mp3/…）下次开面板就能选到（与拖尾脚本同一套扩展方式）。
    //
    // 下拉显示名带扩展名（`stem.ext`），id 仍是 stem。列表里**只会有系统能播的格式**
    // （见 `platform::audio::SUPPORTED_EXT`），所以这里既不需要"可播性"标志，也不需要
    // "无解码器"提示行——能出现在下拉里的项，点下去就该有声音。
    let (title, body, exp) = FeatureCard::new("点击声音", sound_expanded)
        .switch(m.sound_master)
        .body({
            let mut col = Element::col().child(card::slider_row(
                "音量",
                m.sound_volume,
                m.sound_volume_text,
                0.0,
                100.0,
                0,
                col1_w,
            ));
            for (i, ks) in m.sound.iter().enumerate() {
                // 与其它下拉同走 `card::choice_row` 正本（构造处已截断，二次幂等）。
                col = col.child(card::choice_row(
                    key_labels[i],
                    m.sound_choices.clone(),
                    ks.combo,
                    col1_w,
                ));
            }
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    Element::scroll().child(content.padding(16))
}

#[cfg(test)]
mod tests {
    /// ★ 2026-09-25 晚定稿：波纹必须是「父卡 + 五键子卡」结构——子卡标题栏 =
    /// 键名 +「关闭/效果」下拉（**没有开关**），正文 = 参数。此前两个被否决的形态
    /// 都不许回潮：① 标题栏按键选择器（`ripple_key_sel`，看着像只能配一个键）；
    /// ② 平铺行 + 参数挂行下（参数与其它键没有视觉分隔）。
    ///
    /// 判据切在 `mod tests {` 前（`include_str!` 会把测试自己也包进来，搜
    /// `ripple_key_sel` 整词会命中本测试的注释——所以判据卡的是**页面代码**里
    /// 不该再出现的完整表达式）。
    #[test]
    fn ripple_keys_are_sub_cards_with_dropdown_titles_without_switches() {
        const SRC: &str = include_str!("trail.rs");
        let body = SRC.split("mod tests {").next().unwrap_or(SRC);
        // ① 标题栏按键选择器不许回潮。
        assert!(
            !body.contains("ripple_key_sel"),
            "标题栏「按键」选择器已删除，不许回潮"
        );
        // ② 五键子卡：expand id 用 `ripple.<键>` 拼、下拉绑各自的 combo。
        assert!(
            body.contains("m.expand(&format!(\"ripple.{}\", model::RIPPLE_KEYS[i]))"),
            "波纹子卡应按 ripple.<键> 取展开信号"
        );
        assert!(
            body.contains(".title_dropdown(m.ripple_choices.clone(), ks.combo)"),
            "子卡标题栏应是「关闭/效果」下拉（数据源 ripple_choices）"
        );
        // ③ 子卡**不带开关**：`.switch(` 全页只允许出现在四张一级卡上
        //    （拖尾 / 悬浮标 / 波纹总闸 / 声音总闸）。
        let switches = body.matches(".switch(").count();
        assert_eq!(
            switches, 4,
            "鼠标页只允许四处 .switch（拖尾/悬浮标/波纹总闸/声音总闸），子卡不带开关"
        );
        assert!(!body.contains(".switch(ks."), "子卡不得绑定每键开关信号");
        // ④ 关闭态连参数带正文一起收起（combo>0 参与子卡正文可见性）。
        assert!(
            body.contains("sub_exp.get() && combo.get() > 0"),
            "子卡正文应由「展开 且 效果≠关闭」共同门控"
        );
    }
}
