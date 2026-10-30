pub mod backdrop;
pub mod color;
pub mod config;
pub mod damage;
pub mod draw;
pub mod error;
pub mod events;
pub mod exit_policy;
pub mod geometry;
pub mod gif;
/// GIF **编码**（录制用）：调色板量化 + 降采样 + 帧封装。与 [`gif`]（解码）互不相关。
pub mod gif_enc;
pub mod gif_trim;
/// GIF 选区的纯几何命中判定（8 手柄 + 边框移动带 + 内部穿透）。
pub mod gif_zone;
pub mod hotkeys;
pub mod image;
pub mod logger;
pub mod math;
pub mod paths;
/// 长截图拼接核心（纯函数：行哈希粗筛 + 纹理加权精配 + 粘性头部检测 + 帧闸门）。
pub mod scroll_stitch;
pub mod state;
/// `<程序目录>/temp` 的占用统计（面板设置页「工作文件夹」卡的数据来源）。
pub mod temp_usage;
pub mod text_metrics;
pub mod tick;
/// UI 线程消息总线的进程级入口（脚本模态窗走它，见模块头）。
pub mod ui_bus;
