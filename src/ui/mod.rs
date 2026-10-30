//! CurSeen 2.0 UI 模块。
//!
//! 侧边栏 + 内容区布局，每张功能卡片含「开关 / 选项 / 参数」三部分。
//!
//! 数据流：`panel::build` 用当前 `Config` 造一份 [`model::PanelModel`]（一堆
//! `Signal`）交给 `Runtime`；用户拨控件只改信号；覆盖层每帧把模型 diff 回 `Config`
//! 并由 `Runtime` 防抖落盘。UI 层不直接写配置文件。
//!
//! # 依赖倒置
//!
//! 本层对外的**唯一接触面**是 [`PanelHost`]：宿主（`runtime`）实现它并在建窗时注入，
//! 本层因此不反向依赖 `runtime`。

pub mod card;
pub mod content;
pub mod effects;
pub mod host;
pub mod model;
pub mod nav;
pub mod pages;
pub mod panel;
/// 脚本用的宿主模态窗（`ext:result` / `ext:input`）。
pub mod script_dialog;
pub mod sfx;
pub mod store_state;
pub mod temp_usage;
pub mod tokens;
pub mod update_window;

// 重新导出主要组件
pub use card::FeatureCard;
pub use content::build_content;
pub use host::PanelHost;
pub use model::PanelModel;
pub use nav::build_nav;
pub use panel::build as build_panel;
