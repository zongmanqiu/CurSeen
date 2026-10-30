//! Lua 扩展脚本页面。
//!
//! `scripts/` 下一张脚本 = 一张**独立可折叠卡片**，标题左侧一律带开关（2026-09-27 用户
//! 口径：`scripts/` 下每个脚本都要能开关）。开关语义随 role 走，但控件形状一致：
//! - `role = image`（贴图右键「脚本」触发）：开关 = **菜单里这一条是否出现**
//!   （`config.lua_ext.image_active`）；手动动作，点了跑一次、不常驻；
//! - 其余一切（`role = composite` 复合脚本，或 `role = effect` / 未声明 role 的普通
//!   脚本——用户直接把特效脚本复制进来也属于这类）：开关 = **运行**
//!   （`config.lua_ext.composite_active`，开 = 常驻实例，多脚本可同时运行）。
//!
//! 两种卡片的正文一样：一句话说明 + `schema()` 驱动的参数输入区。
//!
//! ★ 2026-09-25（用户口径）：不再有「脚本」分组标题——卡片直接平铺。
//!
//! 有固定归宿的脚本**不住在这里**，由各自的专属页承接，避免重复：
//! - `MouseTrail/` `HoverDot/` `ClickFx/` 三个目录下的 `role = effect` → 鼠标页三个
//!   专属下拉（**归属由目录决定**）；
//! - `role = keysound` → 键盘页「击键音效」卡。
//!
//! ★ m69（2026-10-31 用户口径）：在线资源下载搬回本页，**固定是第一张卡**——获取内容
//!   与配置内容同页，先下载、后配置；展开态 id 仍叫 `global.store`（卡片搬家不换 id，
//!   见 `core::config::CARD_IDS`）。下载来的脚本仍由目录监听自动出卡。
//!
//! 除上述两类外，`scripts/` 下**任何脚本**都在这里出卡（`role` 只影响开关语义，
//! 不再决定"是否出现"）：`image` = 菜单可见性；其余（`composite`/`effect`/未声明）
//! = 常驻运行（companion 宿主对特效回调全套驱动，见 `modules::trail::companion`）。
//!
//! 页面不碰 `Runtime`、不碰配置：参数控件的值都在 `PanelModel` 的信号里，由 `apply_to`
//! 每帧 diff 回 `config.lua_ext.params[id]`，折叠态回 `config.ui.expanded["lua.<id>"]`。

use windui::prelude::*;

use crate::core::events::UiMsg;
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{LuaExtCard, PanelModel};
use crate::ui::pages::trail::build_param_cell;
use crate::ui::store_state::StoreState;
use crate::ui::tokens;
use crate::ui::tokens::to_color;

/// 单张脚本卡片：可折叠标题（`name`，标题左侧一个开关）+ 说明（**脚本写了才画**）+ 参数区。
///
/// ★ 开关语义随 role 走（2026-09-27 用户口径：**每个脚本都要有开关**）：
/// - `image`：开关 = **贴图右键「脚本」里这一条是否出现**，状态存
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
    let mut head = FeatureCard::new(&card.name, card.expanded).switch(card.toggle);
    // 包卡：标题栏下拉选子脚本；单脚本卡（selector = None）保持原样。
    if let Some(sel) = card.selector.clone() {
        head = head.title_dropdown_signal(sel.names, sel.selected);
    }
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
///
/// ★ m35：**没有刷新按钮**——宿主监听 `assets/lua`，放入 / 删除 / 解压出来的脚本会在
///   250 ms 内自动出现在这张表里（`platform::dir_watch` 只置标记，
///   `Runtime::refresh_assets_if_changed` 在 UI 对账里重扫）。卡片表是信号，
///   换表只重建列表、不重建面板，正在跑的实例与开关状态一概不动。
///
/// ★ m69：在线资源下载回到本页并**固定排第一**（先获取、后配置），展开态 id 沿用
///   `global.store`（卡片搬家不换 id，见 `core::config::CARD_IDS`）。
pub fn build_lua_page(m: &PanelModel, store: StoreState, sender: Option<Sender<UiMsg>>) -> Element {
    let col1_w = card::COL1_W;
    let hk_cap = m.script_hotkey_cap;

    // 顺序写死在**建树处**：商店卡在列表之前，不靠任何排序约定——用户口径是"永远是
    // 脚本页第一张卡"，代码形状直接就是那个顺序。
    let store_card =
        crate::ui::pages::store::build_store_card(store, sender, m.expand("global.store"), col1_w);

    let cards = m.lua_ext_cards;
    let list = Element::host_signal(cards, move |c: LuaExtCard| {
        // 包卡（`selector` 非 None）与 `scripts/` 外的特效/音效由别的页承接；
        // `host_signal` 每个元素都建一棵，跳过的给一片零尺寸叶子（不可见、不占位）。
        if c.is_script_card() {
            script_card(c, hk_cap, col1_w)
        } else {
            Element::leaf()
        }
    });

    let empty_hint = Element::label(
        "这里放通用扩展脚本（贴图右键「脚本」/ 复合特效 / 直接复制的特效脚本）。往 assets/lua/scripts 放入 .lua，或解压一个包到这里——面板开着时会**自动出现**。",
    )
    .font_size(tokens::FONT_MD)
    .fg(to_color(tokens::TEXT_SECONDARY))
    .visible_when(move || cards.get().iter().all(|c| !c.is_script_card()));

    Element::scroll().child(
        Element::col()
            .children(vec![store_card, list, empty_hint])
            .padding(16)
            .width_match(),
    )
}

#[cfg(test)]
mod tests {
    /// 本文件的**生产代码**文本（截至测试模块）——`include_str!` 会把测试自身也包进来，
    /// 不切的话断言会被断言自己的文本满足（同款先例见 `ui::pages::store`）。
    fn production_src() -> &'static str {
        let all = include_str!("lua.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    /// m69（用户口径）：官方资源下载卡挂在脚本页，且**永远是第一张卡**——
    /// 顺序在建树处写死（商店卡先于脚本列表）；卡建好却忘了挂同样会被抓住。
    #[test]
    fn store_card_is_the_first_card_on_the_script_page() {
        let p = production_src();
        let store_at = p
            .find("crate::ui::pages::store::build_store_card")
            .expect("脚本页缺少官方资源下载卡");
        let list_at = p
            .find("Element::host_signal(cards")
            .expect("找不到脚本列表");
        assert!(store_at < list_at, "官方资源下载必须是脚本页第一张卡");
        for needle in [
            "m.expand(\"global.store\")",
            ".children(vec![store_card, list, empty_hint])",
        ] {
            assert!(p.contains(needle), "脚本页资源下载卡缺少：{needle}");
        }
    }

    /// 脚本页只**挂**商店卡，不自己实现下载 UI：行/动作/详情窗仍只有 `pages/store.rs`
    /// 一份正本。有人在这里手搓第二套入口（或把来源下拉复制一份）时这条会红。
    #[test]
    fn lua_page_mounts_the_shared_store_card_instead_of_reimplementing_it() {
        let p = production_src();
        for gone in ["StoreAction", "StorePackRow", "Element::dropdown("] {
            assert!(!p.contains(gone), "脚本页不该自己实现下载 UI：{gone}");
        }
    }
}
