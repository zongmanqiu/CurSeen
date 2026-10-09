//! 应用层：UI 意图消息与托盘菜单。
//!
//! [`UiMsg`] 的真身在 [`crate::core::events`]（消息契约属于中立地带），这里重导出是为了
//! 让既有 `crate::app::UiMsg` 的引用继续成立。

pub mod hotkeys;
pub mod tray;

pub use crate::core::events::UiMsg;
