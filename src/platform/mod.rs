//! 平台相关模块。

pub mod ai_chat;
pub mod audio;
pub mod audio_capture;
pub mod capture_share;
pub mod clipboard;
pub mod clipboard_history;
pub mod clipboard_html;
pub mod crash;
pub mod display;
pub mod elevate;
pub mod file_dialog;
pub mod hide_cursor;
pub mod http;
pub mod icon;
pub mod input;
pub mod mp4_trim;
pub mod pip_window;
pub mod recorder;
pub mod screenshot;
pub mod scroll_capture;
pub mod sticky;
pub mod text_raster;
pub mod topmost;
pub mod win_util;
// WGC 采集层（DXGI 桌面复制的替代，见模块头"为什么换"）。DXGI 路径仍保留为回退。
pub mod window_probe;
pub mod working_set;
