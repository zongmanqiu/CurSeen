//! 控制面板窗口的内容。
//!
//! 面板是覆盖层主窗口的**子窗**（经 `EventCtx::open_window` 打开），不是第二个 `App`：
//! windui 的 `App::run()` 是阻塞的消息循环，起第二个 `App` 的语句永远执行不到。
//! 详见 `runtime::overlay::run_overlay`。
//!
//! 建树时用当前 `Config` 初始化一份 [`PanelModel`] 并交给 `Runtime` 持有；此后覆盖层
//! 每帧把模型 diff 回配置。窗口关闭时 `Runtime::close_panel` 负责最后一次落盘。

use windui::prelude::*;

use super::model::PanelModel;
use super::tokens::to_color;
use super::{build_content, build_nav, tokens, PanelHost};

/// 面板窗口标题。
///
/// ★ 2026-09-27 用户口径：标题栏只留程序名，不加版本号与功能描述（那些在「关于」页）。
pub const TITLE: &str = "CurSeen";

/// 面板初始尺寸（逻辑像素）。
///
/// ★ 2026-09-25 从 1000×660 收窄到 900×610：左侧导航收窄后内容区净宽基本不变，
/// 各页都是滚动容器，高度收一点翻页成本低、屏上占地小。
/// ★ 2026-09-27 再收到 820：**只减内容区**——导航 66 px 用户已认可（"已经很合适"），
/// 动它反而要重排标签字号。
/// ★ 2026-09-27 三收：用户定案**内容区 550px** → 客户区 = 导航 66 + 550 = 616。
/// 各页行宽由 `tokens::COL1_W`（208）+ 控件列决定（最宽一行约 544px），550 放得下；
/// 页面都是滚动容器，万一哪页放不下也只是出滚动条、不会溢出。
pub const WIDTH: i32 = 616;
/// ★ 2026-09-27 高度 610 → **450**（用户定案）：各页都是滚动容器，收高只增加
/// 翻页频率，不碰布局。
pub const HEIGHT: i32 = 450;

/// 构建面板控件树：左侧导航 + 右侧内容区。
///
/// [`PanelHost`] 是面板与宿主的**唯一接触面**（依赖倒置，见该 trait 的注释）：
/// 本函数不知道 `Runtime` 的存在。
pub fn build(host: &mut dyn PanelHost) -> Element {
    let model = PanelModel::from_config(&host.config_snapshot());
    // 面板每次打开都是一套全新的信号 → 把模型登记进运行时，并让更新类状态强制全量推一次。
    host.open_panel(model.clone());

    // ★ 主窗口状态记忆：页码信号来自模型（初始化自 `ui.panel_page`，切换时经
    // `apply_to` diff 回配置并防抖落盘）——重开面板/重启进程都回到最后所在页。
    let current = model.page;
    Element::row()
        .child(build_nav(current))
        .child(build_content(
            current,
            &model,
            host.update_state(),
            host.temp_usage_state(),
            host.ui_sender(),
        ))
        .bg(to_color(tokens::BG))
        .cross(Align::Stretch)
}
