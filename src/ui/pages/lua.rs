//! Lua 扩展脚本页面。
//!
//! `scripts/` 下一张脚本 = 一张**独立可折叠卡片**，标题左侧一律带开关（2026-09-27 用户
//! 口径：`scripts/` 下每个脚本都要能开关）。开关语义随 role 走，但控件形状一致：
//! - `role = image`（贴图右键「图片操作」触发）：开关 = **菜单里这一条是否出现**
//!   （`config.lua_ext.image_active`）；手动动作，点了跑一次、不常驻；
//! - 其余一切（`role = composite` 复合脚本，或 `role = effect` / 未声明 role 的普通
//!   脚本——用户直接把特效脚本复制进来也属于这类）：开关 = **运行**
//!   （`config.lua_ext.composite_active`，开 = 常驻实例，多脚本可同时运行）。
//!
//! 两种卡片的正文一样：一句话说明 + `schema()` 驱动的参数输入区。
//!
//! ★ 2026-09-25（用户口径）：不再有「图片操作」分组标题——卡片直接平铺。
//!
//! 有固定归宿的脚本**不住在这里**，由各自的专属页承接，避免重复：
//! - `MouseTrail/` `HoverDot/` `ClickFx/` 三个目录下的 `role = effect` → 鼠标页三个
//!   专属下拉（**归属由目录决定**）；
//! - `role = keysound` → 键盘页「击键音效」卡。
//!
//! 除上述两类外，`scripts/` 下**任何脚本**都在这里出卡（`role` 只影响开关语义，
//! 不再决定"是否出现"）：`image` = 菜单可见性；其余（`composite`/`effect`/未声明）
//! = 常驻运行（companion 宿主对特效回调全套驱动，见 `modules::trail::companion`）。
//!
//! 页面不碰 `Runtime`、不碰配置：参数控件的值都在 `PanelModel` 的信号里，由 `apply_to`
//! 每帧 diff 回 `config.lua_ext.params[id]`，折叠态回 `config.ui.expanded["lua.<id>"]`。

use windui::prelude::*;

use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{LuaExtCard, PanelModel};
use crate::ui::pages::trail::build_param_cell;
use crate::ui::tokens;
use crate::ui::tokens::to_color;

/// 单张脚本卡片：可折叠标题（`name`，标题左侧一个开关）+ 说明（**脚本写了才画**）+ 参数区。
///
/// ★ 开关语义随 role 走（2026-09-27 用户口径：**每个脚本都要有开关**）：
/// - `image`：开关 = **贴图右键「图片操作」里这一条是否出现**，状态存
///   `config.lua_ext.image_active`（`runtime::Runtime::lua_ext_menu_items` 按它过滤）；
/// - 其余一切（`composite`/`effect`/未声明）：开关 = **运行**（开 = 常驻实例，多脚本
///   可同时运行），状态存 `config.lua_ext.composite_active`。
///
/// 两者都是"勾了就有可见效果"，所以都画控件；判定与写回都只过
/// [`LuaExtSettings::role_is_active`] / [`LuaExtSettings::set_role_active`] 一个入口。
fn script_card(
    card: LuaExtCard,
    hk_cap: Signal<Option<crate::ui::model::ScriptHotkeyCap>>,
    col1_w: i32,
) -> Element {
    let params_sig = card.params;
    // ★ 说明行**没有就不画**（2026-09-30 定稿）：旧实现在脚本没写 `description` 时补一句
    //   「（无说明）」，等于把"作者没话说"变成 UI 上一条噪音。没有说明就不占位、不补任何文本。
    //   写成说明的只有**没有任何选项可设定**的脚本（image role 的「图片信息」/「平均颜色」）。
    let mut body_col = Element::col();
    if !card.description.is_empty() {
        body_col = body_col
            .child(
                Element::label(&card.description)
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
            )
            .child(Element::leaf().height(6));
    }
    // 每种 role 的开关都有副作用（运行 / 菜单可见性），一律画，见函数头。
    let head = FeatureCard::new(&card.name, card.expanded).switch(card.toggle);
    let (title, body, exp) = head
        .body(body_col.child(Element::host_signal(params_sig, move |c| {
            build_param_cell(c, hk_cap, col1_w)
        })))
        .build_title_row(col1_w, card::INDENT_CARD);
    Element::col()
        .child(title)
        .child(body.visible_when(move || exp.get()))
}

/// 创建 Lua 扩展页：`scripts/` 下**除 `keysound`（键盘页专属）外的全部脚本**各一张卡，
/// 平铺（无分组标题）。★ 2026-09-27 起 `role="effect"`（含用户直接复制进来的特效脚本）
/// 与未声明 role 的脚本都住这里——开关 = 常驻运行（落 `composite_active`，见
/// [`LuaExtSettings::role_list`] 的落点映射），不再按 role 白名单过滤。
pub fn build_lua_page(m: &PanelModel) -> Element {
    let cards = m.lua_ext_cards.clone();
    let col1_w = card::COL1_W;
    let hk_cap = m.script_hotkey_cap;

    let mut children: Vec<Element> = Vec::new();
    for c in cards {
        // ★ 只收 `scripts/` 目录（归属由目录决定）：MouseTrail/HoverDot/ClickFx 下的
        //   特效脚本由鼠标页三个专属下拉承接，这里再出卡 = 同一脚本双宿主重复跑。
        //   `keysound` 由键盘页承接，同样排除。判定走共享入口（与设置页「脚本开关」
        //   镜像同源，见 `LuaExtCard::is_script_card`）。
        if c.is_script_card() {
            children.push(script_card(c, hk_cap, col1_w));
        }
    }
    if children.is_empty() {
        children.push(
            Element::label(
                "这里放通用扩展脚本（贴图右键菜单 / 复合特效 / 直接复制的特效脚本）。往 assets/lua/scripts 放入 .lua 后重启即可。",
            )
            .font_size(tokens::FONT_MD)
            .fg(to_color(tokens::TEXT_SECONDARY)),
        );
    }

    Element::scroll().child(Element::col().children(children).padding(16))
}
