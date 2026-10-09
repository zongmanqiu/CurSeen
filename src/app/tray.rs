//! 托盘菜单。
//!
//! 覆盖层本身无边框、透明、鼠标穿透、且不进任务栏（`tool_window`），关掉控制面板后
//! 进程就没有任何可见入口了——叠加单实例后更是死局（再双击 exe 只会被转发给首实例）。
//! 托盘是这个程序"还活着"的唯一凭据，也是唯一能重新唤回面板的地方。
//!
//! 回调签名是 `FnMut(&mut TrayCtx)`，**只能登记意图**（经 `Sender<UiMsg>` 发到 UI 线程），
//! 不能直接调 OS 窗口 API。唯独「退出」走 `ctx.quit()`——那是 TrayCtx 自己支持的意图。

use windui::prelude::{Sender, Tray, TrayHandle, TrayMenuItem};

use crate::app::UiMsg;
use crate::platform::icon;

/// 创建托盘图标与右键菜单。
pub fn create_tray(ui_tx: Sender<UiMsg>) -> Tray {
    // ★ 按当前 DPI 取档（100%→16 … 200%→32）：过去恒给 32，高 DPI 下被系统
    // 放大出锯齿；现在五档齐备，每一档都是 SVG 直接光栅化出来的。
    let (n, rgba) = icon::tray_icon(false);
    let tray = Tray::new()
        // ★ 版本号编译期从 `Cargo.toml` 拼接（`concat!` + `CARGO_PKG_VERSION`），
        //   不手写字面量——改版本永不漂移；标语与 README 主标题 / 关于页副标题同源
        //   （2026-10-08 定稿，同步替换掉旧的"键鼠痕迹呈现工具"口径）。
        .tooltip(concat!(
            "CurSeen ",
            env!("CARGO_PKG_VERSION"),
            " - 键鼠所至 · 屏幕所书 · 不止所见"
        ))
        .icon_rgba(n, n, rgba)
        // 左键单击：面板是唯一的交互入口，直接打开它。
        .on_left_click({
            let tx = ui_tx.clone();
            move |_ctx| {
                let _ = tx.send(UiMsg::OpenPanel);
            }
        })
        // 双击：与单击同一意图。平台上双击也会先派发一次单击，而两者都幂等
        //（`single("panel")` 让第二次请求退化为"把窗口拉到前台"）。
        .on_double_click({
            let tx = ui_tx.clone();
            move |_ctx| {
                let _ = tx.send(UiMsg::OpenPanel);
            }
        });

    tray.menu(vec![
        TrayMenuItem::item("打开控制面板", {
            let tx = ui_tx.clone();
            move |_ctx| {
                let _ = tx.send(UiMsg::OpenPanel);
            }
        }),
        TrayMenuItem::separator(),
        TrayMenuItem::item("退出", |ctx| ctx.quit()),
    ])
}

/// 托盘图标随录制态切换（2026-09-27 用户口径：开录后圆点变红，一眼认出"正在录"）。
///
/// `TrayHandle::new()` 连的是 windui 的**线程局部意图队列**，与 `create_tray` 交给
/// `App` 的那条是同一个——所以在任何 UI 线程代码里直接构造一个句柄发意图即可，
/// 不必把创建时的句柄一路穿进 `Runtime`。真正落地在事件分发之后
/// （windui `apply_tray_ops`，CurSeen 补丁 088）。
///
/// ★ 调用点在 `runtime::record` 的开录成功 / 停止处，都是**状态沿**（不是逐帧），
/// 重复调用幂等无害。
pub fn set_tray_recording(on: bool) {
    // ★ 名字是"红色那张"，用途（录制期间）由本函数表达；档位同 `create_tray`。
    let (n, rgba) = icon::tray_icon(on);
    TrayHandle::new().set_icon_rgba(n, n, rgba);
}
