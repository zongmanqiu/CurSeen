//! 功能页面模块
//!
//! 七个页面，与 `ui::nav` 的七个分类一一对应：鼠标、键盘、屏幕、工具、脚本、设置、关于。
//!
//! ★ 例外：`capture` **不是一页**——它是屏幕页的三张抓取录制卡（原「录制」页，
//! 2026-09-25 按"操作对象是屏幕画面"并进屏幕页），只导出一个分区构造函数供
//! `screen.rs` 组装，因此不参与 `ui::content` 的页分发。

pub mod about;
pub mod capture;
pub mod global;
pub mod key_display;
pub mod lua;
pub mod screen;
pub mod store;
pub mod tool;
pub mod trail;
