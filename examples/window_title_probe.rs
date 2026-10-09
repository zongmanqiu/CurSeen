//! 真机探针：**验证 `Window::title(Signal<String>)` 真的把标题推到了系统标题栏**。
//!
//! 为什么要单独验：裁剪窗的标题栏文案是"文档名 - 软件名"（动态），而这件事**单测证明不了**
//! ——`window_title()` 只是拼字符串，真正让系统标题栏改字的那一步在 windui 的
//! `take_window_title` 拉取式机制里（win32 侧 `SetWindowTextW`）。单元测试碰不到窗口过程。
//!
//! 跑法：`cargo run --release --example window_title_probe`
//! 输出走 **stderr**（无缓冲，重定向到文件也立刻可见）。
//!
//! 判据用系统的 `FindWindowW`（**只认完整标题**，正是我们按标题找窗用的同一个 API）：
//! 每改一次信号，下一拍必须能按**新标题**找到那个窗；最后一拍找不到就退 1。

use std::time::Duration;

use windui::prelude::*;

/// 按标题找顶层窗口（与 `win_util::find_by_title` 同一个 API 口径）。
fn find(title: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;

    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())).is_ok() }
}

fn main() {
    // 与裁剪窗同一套取值：初始"未打开文件"，载入后改成"文档名 - 软件名"。
    const APP: &str = "CurSeen 裁剪视频探针";
    let steps = [
        format!("未打开文件 - {APP}"),
        format!("clip.mp4 - {APP}"),
        format!("另一份片子.mp4 - {APP}"),
    ];
    let cur = signal(steps[0].clone());
    let step = std::cell::Cell::new(0usize);

    eprintln!("[probe] main 开始");
    let app = App::new("标题探针宿主", 320, 200)
        .content(Element::col().fill())
        .on_interval(Duration::from_millis(400), move |ctx| {
            let i = step.get();
            if i == 0 {
                // 第一拍：建子窗（与裁剪窗同一条路：`Window::new(占位).title(信号)`）。
                ctx.open_window(
                    Window::new("占位", 420, 200)
                        .title(cur)
                        .content(|| Element::label("标题探针")),
                );
                eprintln!("已建子窗，初始标题「{}」", steps[0]);
                step.set(1);
                return;
            }
            // 之后每拍：先按**上一拍设的**标题找窗，再把标题推成下一个。
            let expect = &steps[i - 1];
            let found = find(expect);
            eprintln!(
                "第 {i} 拍：按「{expect}」找窗 → {}",
                if found { "命中" } else { "落空" }
            );
            if i >= steps.len() {
                eprintln!(
                    "{}",
                    if found {
                        "结论：动态标题确实推到了系统标题栏（FindWindowW 能按新标题命中）"
                    } else {
                        "结论：动态标题没有生效"
                    }
                );
                std::process::exit(if found { 0 } else { 1 });
            }
            cur.set(steps[i].clone());
            step.set(i + 1);
        });
    eprintln!("[probe] 进消息循环");
    app.run();
    eprintln!("[probe] 消息循环结束");
}
