//! 内容区布局组件。
//!
//! 七页同时挂在 `Element::stack()` 上，用 `visible_signal` 按当前导航项切换。之所以
//! 不是"只构建当前页"：控件的选中态、展开态都挂在信号上，重建整棵子树会把这些状态
//! （以及 `PanelModel` 里各页共享的信号绑定）重新初始化，切页就会看到展开状态复位。

use windui::prelude::*;

use super::model::PanelModel;
use super::nav;
use super::pages;
use crate::core::events::UiMsg;
use crate::ui::temp_usage::TempUsageState;
use crate::update::state::UpdateState;

/// 创建内容区。
///
/// 两页需要"外部世界"：关于页要显示更新进度、要点按钮起后台线程；设置页的「工作文件夹」卡
/// 要显示后台统计出来的占用。这些分别由 [`UpdateState`] / [`TempUsageState`]（信号）与
/// [`Sender<UiMsg>`]（发意图）承担——都是**可拥有的值**，因此闭包可以直接捕获，本页也就
/// 不必认识 `Runtime`（依赖倒置，见 [`crate::ui::PanelHost`]）。
pub fn build_content(
    current: Signal<usize>,
    m: &PanelModel,
    update: UpdateState,
    host_store: crate::ui::store_state::StoreState,
    temp_usage: TempUsageState,
    sender: Option<Sender<UiMsg>>,
) -> Element {
    Element::stack()
        .child(
            pages::trail::build_trail_page(m).visible_signal(current.map(|c| *c == nav::NAV_TRAIL)),
        )
        .child(
            pages::key_display::build_key_display_page(m)
                .visible_signal(current.map(|c| *c == nav::NAV_KEY)),
        )
        .child(
            pages::screen::build_screen_page(m)
                .visible_signal(current.map(|c| *c == nav::NAV_SCREEN)),
        )
        .child(
            // 工具页：便签 / 剪贴板 / 计时器——不对应任何硬件的那一类功能。
            pages::tool::build_tool_page(m).visible_signal(current.map(|c| *c == nav::NAV_TOOL)),
        )
        .child(
            // 设置页的「工作文件夹」占用统计（后台线程）由这里注入；m69 起「高级」卡
            // 还摆着商店/更新共用的下载源下拉（只取 `StoreState::source`）。
            pages::global::build_global_page(m, host_store.clone(), temp_usage, sender.clone())
                .visible_signal(current.map(|c| *c == nav::NAV_GLOBAL)),
        )
        .child(
            // 关于页要更新按钮（检查/下载），它的外部依赖从这里注入；来源灰字与商店卡
            // 读同一个信号（`StoreState::source`），不新增第二份来源状态。
            pages::about::build_about_page(update, sender.clone(), host_store.source)
                .visible_signal(current.map(|c| *c == nav::NAV_ABOUT)),
        )
        .child(
            // 脚本页（原 Lua 页）：**官方资源下载是固定第一张卡**（m69），其下是
            // `assets/lua` 每脚本一张卡。商店卡的清单/安装状态与发意图的通道从这里注入。
            pages::lua::build_lua_page(m, host_store, sender.clone())
                .visible_signal(current.map(|c| *c == nav::NAV_LUA)),
        )
        .weight(1.0)
}
