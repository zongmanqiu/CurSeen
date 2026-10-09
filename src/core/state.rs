//! 应用运行态。不持久化，与 Config 分开：Config 是"用户设置"，State 是"当前正在发生什么"。
//!
//! 线程约定：State 只在 UI 线程访问。捕获线程只通过 channel 通信。

use super::config::Config;
use super::tick::FrameClock;

#[derive(Debug, Clone, Default)]
pub struct ModuleSwitches {
    pub trail: bool,
    pub key_display: bool,
    pub drawing: bool,
}

/// 已注册快捷键（冲突检测用）。
#[derive(Debug, Clone, Default)]
pub struct HotkeyRegistry {
    pub registered: Vec<(String, u32)>, // (hotkey_str, id)
}

#[derive(Debug, Clone, Default)]
pub struct CaptureState {
    pub capturing: bool,
    pub target: Option<String>,
}

/// 覆盖层运行标志。
#[derive(Debug, Clone)]
pub struct OverlayRuntimeFlags {
    pub click_through: bool,
    pub drawing_mode: bool,
    pub opacity: f32,
}

impl Default for OverlayRuntimeFlags {
    fn default() -> Self {
        Self {
            click_through: false,
            drawing_mode: false,
            opacity: 1.0,
        }
    }
}

/// 更新检查状态。
#[derive(Debug, Clone, Default)]
pub struct UpdateState {
    pub checking: bool,
    pub last_check: Option<u64>,
}

/// 应用运行态。
pub struct AppState {
    pub config: Config,
    pub modules: ModuleSwitches,
    pub hotkeys: HotkeyRegistry,
    pub capture: CaptureState,
    pub overlay: OverlayRuntimeFlags,
    pub update: UpdateState,
    pub clock: FrameClock,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            modules: ModuleSwitches {
                // ★ 开关架构：各模块只听自己功能段的开关（无功能总开关）。
                trail: config.trail_module_active(),
                key_display: config.key_display.enabled,
                drawing: config.drawing.enabled,
            },
            config,
            hotkeys: HotkeyRegistry::default(),
            capture: CaptureState::default(),
            overlay: OverlayRuntimeFlags::default(),
            update: UpdateState::default(),
            clock: FrameClock::new(),
        }
    }
}
