//! Lua 脚本引擎模块：用户自定义**拖尾/悬浮标/点击效果特效**（`role = effect`）、
//! **贴图右键动作**（`role = image`）、**击键音效**（`role = keysound`）与脚本页散置
//! 扩展脚本共用的一套引擎 —— schema 注册表（[`registry`]）、只读文件访问（[`fs_api`]）、
//! 宿主级 VM 缓存（[`runtime`]）。

pub mod api_draw;
pub mod api_effect;
pub mod api_ext;
pub mod engine;
pub mod ext_runner;
pub mod fs_api;
pub mod registry;
pub mod registry_ext;
pub mod runtime;
pub mod store;
pub mod store_task;

pub use engine::ScriptingEngine;
pub use registry::{EffectSchema, LuaEffectRegistry, ParamDef, ParamType};
pub use registry_ext::ExtSchema;
