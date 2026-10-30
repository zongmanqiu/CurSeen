//! m21 的前置读数：**同一线程先按 MTA 初始化 COM，之后再要 STA 会拿到什么**。
//!
//! 背景：脚本跑在自己的工作线程上，`ext:open_file` / `ext:save_file` 会在这条线程上用
//! **STA**（`platform::file_dialog`）初始化 COM；而宿主已有的 WIC 解码链活在剪贴板历史的
//! worker 上、用的是 **MTA**（`clipboard_history.rs` 的 `ComGuard`）。
//! 若"读图"也在脚本线程上按 MTA 初始化，之后那条线程要开对话框时 `CoInitializeEx(STA)` 会失败——
//! `file_dialog.rs` 的注释说"已初始化过会返回 RPC_E_CHANGED_MODE，不是致命错误、对话框照样能开"，
//! 但**这句话没有读数**。这里只量 HRESULT 本身；"对话框照样能开"要真窗，本轮不做（见 m21 §5）。
//!
//! 跑法：`cargo run --example wic_decode_probe`（零 GUI：不开窗、不读盘、不动剪贴板）。

/// ★ 两次初始化必须**叠在一起**：先 `CoUninitialize` 再要另一种模式等于回到"干净线程"，
/// 那样两格都读出 S_OK，看着像"换模式无害"——第一版就是这么写的，读数全是假的。
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT};

fn pair(name: &str, first: COINIT, second: COINIT) {
    unsafe {
        let a = CoInitializeEx(None, first);
        let b = CoInitializeEx(None, second);
        println!("{name}: first=0x{:08X} second=0x{:08X}", a.0, b.0);
        if b.is_ok() {
            CoUninitialize();
        }
        if a.is_ok() {
            CoUninitialize();
        }
    }
}

fn main() {
    use windows::Win32::System::Com::COINIT_APARTMENTTHREADED;
    use windows::Win32::System::Com::COINIT_MULTITHREADED;

    // 干净线程：先要 STA（对话框那条路的正常姿势），再叠一个 MTA（读图那条路的正常姿势）。
    std::thread::spawn(|| {
        pair(
            "fresh-thread STA then MTA",
            COINIT_APARTMENTTHREADED,
            COINIT_MULTITHREADED,
        );
    })
    .join()
    .unwrap();

    // 反过来的顺序：先 MTA（"读图先在脚本线程起 MTA"），再要 STA（之后开对话框）。
    std::thread::spawn(|| {
        pair(
            "fresh-thread MTA then STA",
            COINIT_MULTITHREADED,
            COINIT_APARTMENTTHREADED,
        );
    })
    .join()
    .unwrap();

    println!(
        "\n读法：S_OK=0x0、S_FALSE=0x1（已按别的模式初始化过，引用计数不变）、\n\
         RPC_E_CHANGED_MODE=0x80010106（**这次调用没有生效**）。\n\
         第二种顺序里若 'then STA' 不是 0x0/0x1，说明「读图先在脚本线程起 MTA」会挡住那条线程\n\
         之后的对话框初始化 —— 那就必须把读图放到独立 worker 上（m21 §5.4 的兜底）。"
    );
}
