//! CurSeen 2.0 主程序入口
//!
//! 单实例由 windui 的 `App::single_instance("curseen", ..)` 保证（内核命名 Mutex），
//! 见 `runtime::overlay::run_overlay`。这里**不做**任何自制的锁文件检测。

// 发布版不弹控制台窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    curseen_ui::core::logger::init_logging();
    curseen_ui::core::logger::install_panic_hook();
    // ★ 紧跟着挂一环"恢复系统指针"：`panic = "abort"` 下 `Drop` 不会跑，而激光笔这类
    //   特效换掉的是**系统级光标位图**，不恢复就会把"没有鼠标指针"留给用户。
    //   必须在上一行之后 —— 它用 `take_hook` 串在日志钩子前面。
    curseen_ui::platform::hide_cursor::install_panic_restore();
    // ★ 再装一层 SEH 兜底：panic hook 只覆盖 Rust 的 panic 通道，而访问冲突 / 栈溢出这类
    //   结构化异常**不经过** panic —— 而 `panic = "abort"` 下 Rust 的 `abort()` 走 `__fastfail`，
    //   又反过来绕过 SEH。两条通道各管一半，缺一条就还是"程序毫无征兆地没了"。
    //   见 `platform::crash` 的模块文档。
    curseen_ui::platform::crash::install();
    log::info!("CurSeen v{} 启动", env!("CARGO_PKG_VERSION"));
    // ★ 启动环境摘要（总纲 §10）：用户能给我们的只有一份日志，缺了版本 / 系统 /
    //   显示器 / 权限级别这四项，一半的「在我机器上好的」问题无从下手。
    curseen_ui::platform::win_util::log_env_banner();

    // 管理员权限启动 = **用户配置项**（2026-10-07 拍板）：设置页「系统」卡的勾选项
    // （`general.admin_launch`），勾了才提权，出厂不勾 = 普通权限（启动链路干净、
    // 无 UAC；代价是提权窗口前台时低级钩子收不到输入，特效在其上冻结——这是
    // 1.0.0 启动即提权的原始动机，现交由用户勾选自行权衡）。UAC 被拒不退出。
    if !curseen_ui::platform::elevate::is_elevated() {
        if curseen_ui::core::config::Config::read_admin_launch_flag() {
            log::info!("配置要求管理员权限启动，发起提权重启");
            if curseen_ui::platform::elevate::relaunch_elevated() {
                // ★ 不立刻退：先建隐形窗把消息泵跑 1.5s，让系统把"应用已启动"握手走完
                //   （秒退会让这个等待悬空缠上提权副本 ⇒ 首开剪贴板面板悬停一直转圈，
                //   2026-10-07 实测定案，见 elevate::settle_startup_handshake 文档）。
                log::info!("提权副本已发起，本进程走完启动握手后退出");
                curseen_ui::platform::elevate::settle_startup_handshake();
                log::info!("启动握手完成，退出");
                return;
            }
            log::warn!(
                "提权未成功（用户拒绝 UAC），继续以普通权限运行：覆盖提权窗口时钩子将收不到输入"
            );
        } else {
            log::info!("普通权限启动（「系统 > 管理员权限启动」未勾选）");
        }
    }

    curseen_ui::run();
}
