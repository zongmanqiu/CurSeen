//! 剪贴板历史模块。
//!
//! 本模块只把「剪贴板历史管理器」接进 CurSeen 的出帧循环：每帧读配置镜像（启用 / 容量），
//! 仅在变化时经 `platform::clipboard_history` 推送；并匹配呼出快捷键（非长按重复）切换面板。
//! 真正的窗口与存储都在 `platform::clipboard_history`（单后台原生 Win32 线程），本模块
//! 不占 windui 每帧画布、绝不阻塞——`draw` 因此为空实现。
//!
//! ★ 后台线程**按需启动**（2026-10-24）：只有 `clipboard.enabled` 为真（启动时或启用边沿）
//! 才拉起管理器。功能关着时没有线程、没有隐藏监听窗、也没有剪贴板监听链——否则日志里
//! 每次启动都有一条「剪贴板: 后台线程就绪」，用户看到的就是"我没开它却在跑"。
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
        // ★ 按需启动（2026-10-24）：功能关着时后台管理器线程、隐藏监听窗、系统剪贴板
        //   监听链一律不存在。此前无条件 `ensure_started()`，日志里每次启动都有
        //   「剪贴板: 后台线程就绪」，用户看到的就是"我没开它却在跑"。
        let last_enabled = config.clipboard.enabled;
        let last_max = config.clipboard.max_entries;
        if last_enabled {
            ensure_started();
            set_config(ClipCfg {
                enabled: last_enabled,
                max_entries: last_max,
                panel: config.clipboard.panel,
            });
        }
        Self {
            last_enabled,
            last_max,
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
            // 启用边沿才拉起管理器；关闭边沿仍要推一次配置（让它停止捕获），
            // 但 `set_config` 本身不会再隐式启动线程。
            if en {
                ensure_started();
            }
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

#[cfg(test)]
mod tests {
    /// ★ 关闭态**不得**启动后台管理器（2026-10-24 用户口径"没开的功能不该在跑"）。
    ///
    /// 真启动会建隐藏窗、挂系统剪贴板监听链、并留下一条活到进程退出的线程 —— 那部分
    /// 只能真机验。这里钉住调用结构：生产代码里每一处 `ensure_started()` 都必须被
    /// "本次配置启用"的判断罩住，不能出现在 `new` / `tick` 的无条件路径上。
    #[test]
    fn every_start_call_is_guarded_by_the_feature_flag() {
        let src = include_str!("mod.rs");
        let body = &src[..src.find("mod tests {").map_or(src.len(), |i| i)];
        let lines: Vec<&str> = body.lines().collect();
        let mut starts = 0;
        for (i, line) in lines.iter().enumerate() {
            if line.trim() != "ensure_started();" {
                continue;
            }
            starts += 1;
            let prev = lines[..i]
                .iter()
                .rev()
                .map(|l| l.trim())
                .find(|l| !l.is_empty());
            assert!(
                matches!(prev, Some("if last_enabled {" | "if en {")),
                "ensure_started() 必须只出现在启用判断里，实际上一行是 {prev:?}"
            );
        }
        assert_eq!(
            starts, 2,
            "启动调用应恰好两处（启动时启用 / 运行中启用边沿），结构变了要同步这条门禁"
        );
    }
}
