//! 便签（Sticky Note）模块。
//!
//! 便签窗本身是**原生 Win32 顶层窗**（见 [`crate::platform::sticky`]），本模块只负责
//! 把它接进 CurSeen 的出帧循环：每帧读配置镜像 + 匹配呼出快捷键。原生窗跑在独立的
//! pump 线程上，`tick` 只发 `Sender<NoteCmd>` 命令，不占 windui 每帧画布、绝无阻塞；
//! `draw` 因此是空实现（便签不画进覆盖层）。
//!
//! # 分层
//!
//! 本模块在 `modules` 层，只依赖 `core`（配置 / 事件 / 快捷键匹配）与
//! `platform::sticky`（`StickyManager`）—— `platform` 在 `modules` 之下，方向合法。
//! 配置类型 `NoteSettings` 放 `core`，跨到 pump 线程时转成 `platform::sticky::NoteCfg`
//! 值拷贝，不把 `&mut Config` 送进别线程。

use crate::core::config::Config;
use crate::core::draw::DrawList;
use crate::core::events::InputEvent;
use crate::core::hotkeys;
use crate::core::tick::Tick;
use crate::modules::Module;
use crate::platform::sticky::{NoteCfg, StickyManager};

/// `NoteSettings` → pump 线程能用的 `NoteCfg` 值拷贝镜像。
///
/// `NoteCfg` 用 `bg`/`text`（平台层命名），配置用 `bg_color`/`text_color`（core 语义），
/// 在这里完成映射。透明度 / 字号是 [A,R,G,B] 与百分比之外的同名同单位，直接透传。
fn note_cfg_from(config: &Config) -> NoteCfg {
    let n = &config.sticky_note;
    NoteCfg {
        bg: n.bg_color,
        text: n.text_color,
        edit: n.edit_color,
        alpha: n.alpha,
        font_size: n.font_size,
    }
}

/// 便签模块：持有 `StickyManager`，每帧同步配置并匹配呼出快捷键。
pub struct StickyNoteModule {
    mgr: StickyManager,
    /// 上一帧功能开关是开的吗（"开→关"边沿，见 [`Self::tick`]）。
    noted_enabled: bool,
    /// 上一帧配置镜像，用于识别"配置变化 → 广播 apply"。
    last_cfg: NoteCfg,
}

/// "上一帧开着、这一帧关了" ⇒ 本帧要发关便签命令。
///
/// ★ 收成独立函数（而不是 inline 在 `tick` 里）就是为了能单测：真开窗 / 真销毁要起
///   pump 线程与消息泵，headless 里验不了，能验的只有"边沿只发一次"这个纯逻辑。
fn close_all_on_disable(was_enabled: bool, now_enabled: bool) -> bool {
    was_enabled && !now_enabled
}

impl StickyNoteModule {
    pub fn new(config: &Config) -> Self {
        let cfg = note_cfg_from(config);
        Self {
            mgr: StickyManager::new(),
            noted_enabled: true,
            last_cfg: cfg,
        }
    }

    /// 呼出一张新便签（快捷面板的入口；与快捷键路径同一底层调用）。
    pub fn spawn_new(&mut self) {
        let cfgv = self.last_cfg.clone();
        self.mgr.ensure(&cfgv);
        self.mgr.spawn(&cfgv);
    }
}

impl Module for StickyNoteModule {
    /// `Module::enabled` 恒真：便签是常驻工具，功能总开关是
    /// `config.sticky_note.enabled`（面板「便签」卡标题栏选框），由 `tick` 把关快捷键呼出。
    fn enabled(&self, _config: &Config) -> bool {
        true
    }

    fn tick(&mut self, _tick: &Tick, config: &Config, input: &Option<InputEvent>) {
        // 配置变化 → 广播给所有便签（颜色 / 透明度 / 字号）。
        let cfg = note_cfg_from(config);
        if cfg != self.last_cfg {
            self.last_cfg = cfg.clone();
            self.mgr.apply(&cfg);
        }

        // 呼出快捷键：按下（非长按重复）主键命中，开一张便签。
        // ★ 功能开关（面板「便签」卡标题栏选框）：关 = 快捷键不响应（颜色同步照常做，
        //   它只是把配置广播给已存在的窗，没有副作用）。
        if !config.sticky_note.enabled {
            // ★ 关开关 = **关掉已开的便签**（2026-10-03 用户口径"主窗口模块开关不能关闭
            //   正在进行中的任务？……其它模块也要检查"）。此前只挡快捷键 ⇒ 取消勾选后
            //   **屏幕上那张便签还留着**，面板与实际状态不一致。
            // 边沿触发：只在"开→关"那一次发命令（判据与 `last_cfg` 无关 ——
            //   配置变更多少次都不影响"关过一次"这件事只发一次）。
            if close_all_on_disable(self.noted_enabled, false) {
                self.mgr.close_all_notes();
            }
            self.noted_enabled = false;
            return;
        }
        self.noted_enabled = true;
        let hotkey = config.sticky_note.hotkey.clone();
        if hotkey.is_empty() {
            return;
        }
        if let Some(InputEvent::KeyDown {
            key,
            mods,
            repeated,
            ..
        }) = input
        {
            if !*repeated && hotkeys::matches(&hotkey, *key, *mods) {
                let cfgv = self.last_cfg.clone();
                self.mgr.ensure(&cfgv);
                self.mgr.spawn(&cfgv);
            }
        }
    }

    /// 便签画在各自的原生窗上，不进覆盖层 DrawList。
    fn draw(&self, _draw: &mut DrawList, _config: &Config) {}
}
#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 关开关要**关掉已开的便签**（2026-10-03 用户口径"其它模块也要检查"）。
    ///
    /// 这条只能验"边沿只发一次"这个纯逻辑：真开窗 / 真销毁要起 pump 线程与消息泵，
    /// headless 单测里验不了（那部分靠真机：勾上→按快捷键出便签→取消勾选→便签消失）。
    ///
    /// ★ 边沿判据单独收成一个函数（`close_all_on_disable`）而不是 inline 在 `tick` 里，
    ///   就是为了能这么单测 —— inline 的话只能靠"起线程看窗在不在"，成本高得多。
    #[test]
    fn the_close_all_edge_fires_once_per_disable() {
        // 开 → 关：发一次（true = 本次要发命令）。
        assert!(
            close_all_on_disable(true, false),
            "开→关那一帧必须发关便签命令"
        );
        // 关 → 关：不再发（否则每帧都往命令队列塞一条，pump 线程白忙）。
        assert!(
            !close_all_on_disable(false, false),
            "已经在关的状态不该反复发命令"
        );
        // 关 → 开 → 关：又发一次（用户重开勾选后又关掉，理应再收一次）。
        assert!(close_all_on_disable(true, false), "重开后再关要再发一次");
    }
}
