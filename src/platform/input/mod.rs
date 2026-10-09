//! 全局输入捕获层。对外只有三个能力：启动、停止、启停开关。

pub mod hit;
pub mod hook;
pub mod inject;
pub mod keyboard;
pub mod mouse;
pub mod vk;

pub use hook::InputHook;
