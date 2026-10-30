//! UI 线程消息总线的**进程级**入口（m08 脚本模态窗）。
//!
//! # 为什么需要一个全局量
//!
//! [`UiMsg`] 的正规通路是 `Runtime::ui_tx`，但那条通道在 `runtime` 层，而**脚本层拿不到它**
//! （分层是 `core ← platform ← scripting ← … ← runtime`，`scripting` 不许向上借 `Runtime`）。
//! 脚本里的 `ext:result` / `ext:input` 又确实要说"请 UI 线程替我开一个窗"。
//!
//! # 为什么存的是"发送函数"而不是通道本身
//!
//! `app.channel(...)` 给的是 **windui 的 `Sender`**，而 `core` 禁依赖 `windui`
//! （`build/layering_scan.py` 第 2 条：可测性的根基）。所以这里只存一个
//! `dyn Fn(UiMsg) -> bool`：由 `runtime::overlay` 在起应用时把"往 windui 通道里塞一条消息"
//! 包成闭包交进来，闭体留在高层、`core` 只认识 [`UiMsg`]。形状与
//! `platform::input::hook` 的 `static HOOK_TX: OnceLock<Sender<..>>` 同一条思路——
//! 进程级一份、装一次、到处能问。
//!
//! # 装不上怎么办
//!
//! 单元测试里的裸 `Runtime` 没有通道（`runtime/mod.rs` 的 `ui_tx: None`），覆盖层起来之前
//! 也还没有通道。所以 [`dialog`] 返回 [`DialogOutcome::Unavailable`]，**调用方退回原生框**——
//! 这不是"以防万一"，而是本层唯一诚实的做法：`ext:result` 在无 UI 线程的环境里必须仍然跑得完
//! （`scripting` 的测试就是这种环境）。

use std::sync::mpsc::Receiver;
use std::sync::{Arc, OnceLock};
use std::thread::ThreadId;

use super::events::{ScriptDialogSpec, UiMsg};

/// 把一条消息送进 UI 线程；返回 `false` = 通道已断（应用正在退出）。
type Sink = dyn Fn(UiMsg) -> bool + Send + Sync;

/// 总线本体：发送闭包 + **UI 线程的身份**（后者是防死锁用的，见 [`dialog`]）。
struct Bus {
    sink: Arc<Sink>,
    ui_thread: ThreadId,
}

static BUS: OnceLock<Bus> = OnceLock::new();

/// 装上总线（重复调用无害，第一次生效）。由 `runtime::overlay` 在 UI 线程调用一次——
/// "哪个是 UI 线程"就是从这里的 `current()` 记下来的。
pub fn install(f: impl Fn(UiMsg) -> bool + Send + Sync + 'static) -> bool {
    let bus = Bus {
        sink: Arc::new(f),
        ui_thread: std::thread::current().id(),
    };
    BUS.set(bus).is_ok()
}

/// 一次宿主模态窗的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome {
    /// 宿主窗已经开过并关闭。`Some(文本)` = 输入窗点了「确定」；`None` = 取消 / 关窗 / 结果窗关闭。
    Closed(Option<String>),
    /// 总线还没装或通道已断：调用方**必须**退回原生框，不许静默丢弃。
    Unavailable,
}

/// 送一条 [`UiMsg::ScriptDialog`] 并**阻塞等用户**（调用方是脚本的工作线程，阻塞无害——
/// 与旧的 `text_result_box` 逐字同一条语义：弹框直到关窗才返回，UI 线程零等待）。
///
/// 三条出口都有值可回，不会挂死：
/// · 用户点按钮 / 关窗 → [`ScriptDialogSpec::reply`]；
/// · 发送失败 → [`DialogOutcome::Unavailable`]；
/// · 窗彻底销毁且没人回 → 通道里的 `Sender` 随 `Arc` 一起掉 → `recv()` 返回 `Err`
///   → 按取消收尾。
///
/// ★★ **在 UI 线程上绝不阻塞**：`draw` 模型（逐帧特效）的脚本就跑在 UI 线程里，而窗是
///   UI 线程下一次派发才开的——在那儿等回复等于**自己等自己**，整个软件会当场冻住。
///   所以这条路上只发消息、不等回答（`Closed(None)`），并把话说明写进日志：
///   要读用户输入请放 `run` 模型（`image` / `keysound`，那些跑在工作线程上）。
pub fn dialog(spec: Arc<ScriptDialogSpec>, rx: Receiver<Option<String>>) -> DialogOutcome {
    let Some(bus) = BUS.get() else {
        return DialogOutcome::Unavailable;
    };
    if !(bus.sink)(UiMsg::ScriptDialog(spec)) {
        log::warn!("脚本模态窗送不进 UI 通道（应用可能在退出），退回原生框");
        return DialogOutcome::Unavailable;
    }
    if std::thread::current().id() == bus.ui_thread {
        log::warn!(
            "在 UI 线程上请求了阻塞式弹窗：窗会开，但不等用户回答\
             （阻塞会自锁；逐帧脚本请改用 run 模型）"
        );
        return DialogOutcome::Closed(None);
    }
    DialogOutcome::Closed(rx.recv().unwrap_or(None))
}

/// 回写贴图的结果档。三档都有值，不会挂死。
#[derive(Debug)]
pub enum ImageOutcome {
    /// UI 线程已经处理完（`Err` 里是给人看的原因，如"那张贴图不在了"）。
    Applied(Result<(), String>),
    /// **在 UI 线程上发出的**（`draw` 逐帧脚本）：不能阻塞自己，所以只把动作排进去，
    /// 下一次派发时生效。调用方要如实这么说，不许报成"已经换好了"。
    Queued,
    /// 总线没装（无 UI 线程的环境）。
    Unavailable,
}

/// 送一条 [`UiMsg::ScriptImage`] 并等 UI 线程把它应用完。
///
/// ★ 与 [`dialog`] 同一前提、**不同处置**：模态窗在 UI 线程上"只开不等"是有意义的
///   （用户看得见窗，脚本拿不到答案）；回写贴图在 UI 线程上不能等（等的就是自己），
///   但动作本身**不需要**等谁 ⇒ 那条路送出去并如实报 [`ImageOutcome::Queued`]。
pub fn set_image(
    spec: std::sync::Arc<crate::core::events::ScriptImageSpec>,
    rx: std::sync::mpsc::Receiver<Result<(), String>>,
) -> ImageOutcome {
    let Some(bus) = BUS.get() else {
        return ImageOutcome::Unavailable;
    };
    let on_ui = std::thread::current().id() == bus.ui_thread;
    if !(bus.sink)(UiMsg::ScriptImage(spec)) {
        log::warn!("脚本回写送不进 UI 通道（应用可能在退出）");
        return ImageOutcome::Unavailable;
    }
    if on_ui {
        return ImageOutcome::Queued;
    }
    // 没人回复（窗半路销毁）时 `Sender` 随 `Arc` 一起掉 ⇒ 按失败收尾，不挂死。
    ImageOutcome::Applied(
        rx.recv()
            .unwrap_or_else(|_| Err("回写没有得到回应（那张贴图可能已经关了）".to_string())),
    )
}

/// 当前线程是不是 UI 线程（总线没装 = `false`：没有 UI 会被我冻住，比如单测与量具进程）。
///
/// ★ 给**原生模态对话框**用的：`platform::file_dialog` 那几扇窗走 `GetOpenFileNameW`，
///   它自带消息泵但**阻塞调用线程**——而 `draw` 模型的脚本就跑在画覆盖层的那条线程上，
///   在那儿开对话框 = 全屏冻住直到用户点掉。与 [`dialog`] 里那条防自锁分支同一个判据，
///   不要再抄第三份。
/// 送一条 [`UiMsg::Notice`]：屏幕中央那句短反馈（「正在运行：X」）。
///
/// ★ 语义与其它 `send` 一致：**不阻塞、不保证送达**。总线没装（单测 / 量具进程）返回
///   `false`，调用方**不许**把它当失败——它是反馈，不是业务结果；脚本该怎么跑还怎么跑。
pub fn notice(text: String) -> bool {
    match BUS.get() {
        Some(bus) => (bus.sink)(UiMsg::Notice(text)),
        None => false,
    }
}

pub fn on_ui_thread() -> bool {
    BUS.get()
        .map(|b| std::thread::current().id() == b.ui_thread)
        .unwrap_or(false)
}

/// 总线是否已经被装上（**不分线程**）。
///
/// 给测试用：`BUS` 是进程级 `OnceLock`，一个二进制里只有一份，谁先 `install` 谁占住。
/// 没有这只函数，测试只能"静默跳过"别人占用的情况——那是假绿的一种，必须自报。
pub fn installed() -> bool {
    BUS.get().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `result` / `input` 两种窗的字段形状（决定 UI 侧画只读正文还是画输入框）。
    #[test]
    fn spec_kinds_carry_the_right_fields() {
        let (r, _rx) = ScriptDialogSpec::result("图片体检", "尺寸：1920 × 1080", 0);
        assert!(!r.editable);
        assert_eq!(r.title, "图片体检");
        assert_eq!(r.body, "尺寸：1920 × 1080");
        assert!(r.initial.is_empty());

        let (i, _rx2) = ScriptDialogSpec::input("改名", "old.png", 7);
        assert!(i.editable);
        assert_eq!(i.initial, "old.png");
        assert!(i.body.is_empty());
        assert_eq!(i.owner_hwnd, 7);
    }

    /// 回复是**一次性**的：按钮先回、关窗再回不该送出第二个值。
    #[test]
    fn reply_is_once_only() {
        let (spec, rx) = ScriptDialogSpec::input("输入", "", 0);
        assert!(!spec.replied());
        spec.reply(Some("第一段".to_string()));
        assert!(spec.replied());
        spec.reply(Some("第二段".to_string()));
        assert_eq!(rx.recv().unwrap().as_deref(), Some("第一段"));
        // 通道里只剩这一个值：第二个值既没发出、也不该阻塞。
        drop(spec);
        assert!(rx.recv().is_err());
    }

    /// 没人回就把窗丢掉 ⇒ 接收端报错，调用方按取消收尾（**这是不挂死的保证**）。
    #[test]
    fn dropped_spec_unblocks_receiver() {
        let (spec, rx) = ScriptDialogSpec::result("标题", "正文", 0);
        drop(spec);
        assert!(rx.recv().is_err());
    }

    /// ★ 回写贴图那条消息的三档出口（m15）。这里不碰任何窗，只验**契约本身**：
    /// · 回复只算第一次（换图成功后紧接着的失败回投不许改口）；
    /// · 没人回就丢包 ⇒ 接收端报错，`set_image` 那边据此**不挂死**；
    /// · 总线没装（单测/量具进程）⇒ `Unavailable`，脚本拿到的是"没送到"而不是"成功了"。
    #[test]
    fn writeback_spec_replies_once_and_never_hangs() {
        let bmp = std::sync::Arc::new(
            crate::core::image::RgbaBitmap::new(2, 1, vec![9u8; 8]).expect("构造测试位图"),
        );
        let (spec, rx) = crate::core::events::ScriptImageSpec::new(0x1234, bmp.clone());
        assert_eq!(spec.hwnd, 0x1234);
        assert_eq!(spec.bitmap.w, 2);
        spec.reply(Ok(()));
        assert!(rx.recv().unwrap().is_ok(), "第一次回复必须算数");
        spec.reply(Err("迟到的失败".to_string())); // 不许改口，也不许 panic

        // 没人回 ⇒ 随 `Arc` 一起掉，接收端报错（调用方按失败收尾）。
        let (spec2, rx2) = crate::core::events::ScriptImageSpec::new(1, bmp.clone());
        drop(spec2);
        assert!(
            rx2.recv().is_err(),
            "丢包必须让接收端拿到 Err，而不是永远等"
        );

        // 总线没装时 `set_image` 走 Unavailable（本测试二进制里若已被别的测试装上，
        // 这条会走另一档 —— 所以只在没装的情况下断言，装了要自报，不许静默绿）。
        if crate::core::ui_bus::installed() {
            eprintln!("UI 总线已被其它测试占用，跳过 Unavailable 那一格读数");
            return;
        }
        let (spec3, rx3) = crate::core::events::ScriptImageSpec::new(7, bmp);
        assert!(matches!(
            crate::core::ui_bus::set_image(spec3, rx3),
            crate::core::ui_bus::ImageOutcome::Unavailable
        ));
    }
}
