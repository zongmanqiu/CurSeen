//! 剪贴板历史模块。
//!
//! 本模块只把「剪贴板历史管理器」接进 CurSeen 的出帧循环：每帧读配置镜像（启用 / 容量），
//! 仅在变化时经 `platform::clipboard_history` 推送；并匹配呼出快捷键（非长按重复）切换面板。
//! 真正的窗口与存储都在 `platform::clipboard_history`（单后台原生 Win32 线程），本模块
//! 不占 windui 每帧画布、绝不阻塞——`draw` 因此为空实现。
//!
//! # 分层
//!
//! 本模块在 `modules` 层，只依赖 `core`（配置 / 事件 / 快捷键匹配）与 `platform::clipboard_history`
//! （平台层在 `modules` 之下，方向合法）。`enabled` 恒为真（与便签同构：快捷键空串即停用呼出）。

use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::InputEvent;
use crate::core::hotkeys;
use crate::core::tick::Tick;
use crate::modules::Module;
use crate::platform::clipboard_history::{ensure_started, set_config, toggle_panel, ClipCfg};

/// 剪贴板历史模块。
pub struct ClipboardModule {
    last_enabled: bool,
    last_max: usize,
}

impl ClipboardModule {
    pub fn new(config: &Config) -> Self {
        ensure_started();
        set_config(ClipCfg {
            enabled: config.clipboard.enabled,
            max_entries: config.clipboard.max_entries,
            panel: config.clipboard.panel,
        });
        Self {
            last_enabled: config.clipboard.enabled,
            last_max: config.clipboard.max_entries,
        }
    }
}

impl Module for ClipboardModule {
    /// 常驻工具：快捷键空串即不呼出；`enabled` 是功能总开关（由管理器据此决定是否捕获）。
    fn enabled(&self, _config: &Config) -> bool {
        true
    }

    fn tick(&mut self, _tick: &Tick, config: &Config, input: &Option<InputEvent>) {
        let en = config.clipboard.enabled;
        let mx = config.clipboard.max_entries;
        if en != self.last_enabled || mx != self.last_max {
            self.last_enabled = en;
            self.last_max = mx;
            set_config(ClipCfg {
                enabled: en,
                max_entries: mx,
                panel: config.clipboard.panel,
            });
        }

        // 呼出快捷键：按下（非长按重复）主键命中即切换面板（再按或 Esc 隐藏）。
        if en {
            let hotkey = config.clipboard.hotkey.clone();
            if !hotkey.is_empty() {
                if let Some(InputEvent::KeyDown {
                    key,
                    mods,
                    repeated,
                    ..
                }) = input
                {
                    if !*repeated && hotkeys::matches(&hotkey, *key, *mods) {
                        log::info!("剪贴板: 快捷键命中 ({hotkey})");
                        toggle_panel();
                    }
                }
            }
        }
    }

    /// 面板画在原生窗上，不进覆盖层 DrawList。
    fn draw(&self, _draw: &mut DrawList, _config: &Config) {}
}
