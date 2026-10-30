//! CurSeen 2.0 UI 库

// ★ `#[implement]` 宏生成的代码引用绝对路径 `::windows_core::*`，
//   因此 crate 根必须有这个别名（windows 0.62 的 `windows::core` 就是它）。
extern crate windows_core;

pub mod app;
pub mod core;
pub mod modules;
pub mod platform;
pub mod runtime;
pub mod scripting;
pub mod ui;
pub mod update;

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::config::Config;
use crate::platform::display::virtual_screen;
use crate::platform::input::InputHook;
use crate::runtime::Runtime;

/// 单实例标识。
///
/// 必须与 [`runtime::overlay::run_overlay`] 里 `App::single_instance("curseen", ..)`
/// 用的 app_id **逐字一致**：windui 用 "这次是否已经仲裁过" 判断是否跳过二次仲裁，
/// id 不一致会让它在本进程内二次 `acquire` ——撞上自己刚持有的那把锁、把自己判成
/// 二次实例、再 forward 给自己，结果是窗口永远不出现（进程在跑，界面不出来）。
pub const APP_ID: &str = "curseen";

/// 启动整个应用（阻塞，直到退出）。
///
/// 装配范式（与 windui 的线程模型对齐）：
/// **覆盖层是唯一的 `App`**，控制面板是它的子窗、托盘挂在它上面。
/// 这里不能先跑覆盖层再跑面板——`App::run()` 是阻塞的消息循环，第二个 `App`
/// 的语句永远执行不到，表现就是"双击 exe 什么窗口都不出"。
/// 覆盖层装配见 `runtime::overlay::run_overlay`。
pub fn run() {
    // ★★ 单实例闸门：必须在**做任何有副作用的事之前**。
    //
    // windui 的 `App::run` 内部也会仲裁一次，但那已经在装配流程跑完之后——二次实例会
    // 先读配置、甚至把键鼠低级钩子装上（一个 `WH_MOUSE_LL`/`WH_KEYBOARD_LL` 线程），
    // 而它的全部使命只是"把 argv 交给首实例然后退出"。钩子装两次的窗口期不长，但配置
    // 加载（下面第一步，可能要写文件）被二次实例再跑一遍就是实打实的副作用了——把它挡在
    // 最前面，正是 `claim_instance` 存在的理由。
    //
    // 返回 `Handoff` 时 argv 已送达首实例（它会拉起控制面板），本进程立即返回：
    // 不读配置、不前移 AppData、不装钩子、不建窗口。
    if windui::claim_instance(APP_ID) == windui::InstanceRole::Handoff {
        log::info!("已有实例在运行，argv 已转交，本进程退出");
        return;
    }

    // ★ 只有首实例才做这项补恢复：二次实例在首实例仍隐藏指针时看到标记，
    //   若在这里恢复就会把正在使用的特效光标弄回来。
    crate::platform::hide_cursor::restore_after_crash();
    // ★ 配置落盘的「父目录 fsync」实现在这里注入（`core` 不得碰 Win32，见
    //   `core::config::DIR_SYNC` 的注释）。必须在**读配置之前**——`Config::load()`
    //   里的迁移会立刻写盘，那次写盘同样该是耐久的。
    crate::core::config::set_dir_sync(crate::platform::win_util::sync_dir);

    let mut config = Config::load().unwrap_or_default();
    // 剪贴板：旧 panel.json 的窗口记忆一次性并入 config.json（迁完删旧文件）。
    crate::platform::clipboard_history::migrate_legacy_panel_state(&mut config);
    let vs = virtual_screen().or_fallback();

    let mut runtime = Runtime::new(config);
    runtime.set_screen(vs);
    let runtime = Rc::new(RefCell::new(runtime));

    // 键鼠低级钩子（自带线程，线程内跑消息循环，事件经 std mpsc 送出）。
    // 用 `WH_KEYBOARD_LL` / `WH_MOUSE_LL` 而不是轮询 `GetCursorPos`：轮询拿不到按键、
    // 拿不到点击，而且 4ms 一跳会持续唤醒 CPU（实测空闲 ~0.7%）。钩子是事件驱动，
    // 不产生任何空闲开销。
    // ★ 鼠标钩子按需装卸（2026-10-24）：没有鼠标功能时它根本不该被系统调用。
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let hook = match InputHook::start(event_tx) {
        Ok(h) => {
            log::info!("键盘钩子已安装（鼠标钩子按需启用）");
            Some(h)
        }
        Err(e) => {
            // 装不上钩子不是致命错误：面板、托盘、配置都还能用，只是没有特效输入。
            log::error!("安装键盘钩子失败，特效将不响应输入: {e}");
            None
        }
    };

    runtime::overlay::run_overlay(runtime, event_rx);
    log::info!("覆盖层已退出，收尾中");
    // 双保险：即使 `run_overlay` 的收尾路径将来被改坏，入口也知道"退出必须还指针"。
    crate::platform::hide_cursor::ensure_shown();
    drop(hook);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单实例闸门：本进程首次仲裁必须判为首实例。
    ///
    /// 用的是**专用 app_id**，不与真实运行的那把锁（`APP_ID`）相干——测试进程与真实
    /// 进程若共用一个 id，会把"谁先起来"变成一种竞态。
    ///
    /// 这条断言的真实价值在于：它把 `claim_instance` 这条路径（具名 Mutex 创建 +
    /// GetLastError 判定）跑了一遍。它一旦退化成 panic 或恒返回 Handoff，二次启动就
    /// 再也拉不起面板——而那种失败在手工测试里极易被当成"exe 没反应"而漏过。
    #[test]
    fn claim_instance_grants_the_first_caller() {
        assert_eq!(
            windui::claim_instance("curseen_unit_test_claim"),
            windui::InstanceRole::First
        );
    }

    /// 两次仲裁必须同 id：不一致会让 windui 在自己进程内二次 acquire，把自己判成
    /// 二次实例 → forward 给自己 → 窗口永远不出现（详见 [`APP_ID`] 的注释）。
    ///
    /// 这里断言**字面值**，把"两处引用同一个常量"这个约定钉在测试里。
    #[test]
    fn instance_id_is_stable() {
        assert_eq!(APP_ID, "curseen");
        assert!(!APP_ID.is_empty(), "空的 app_id 会撞上 windui 的默认命名");
    }
}
