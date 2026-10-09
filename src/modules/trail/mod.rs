//! 鼠标特效模块（拖尾/波纹/悬浮标/复合脚本）。

pub mod clickfx;
pub mod companion;
pub mod controller;
pub mod effects;
pub mod fx_core;
pub mod hover_dot;
pub mod ripples;
pub mod sample;
pub mod settings;
pub mod sound;

pub use controller::TrailModule;
pub use settings::{EffectId, TrailSettings};
