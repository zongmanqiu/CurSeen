//! 内存归因探针：把"基座 / 覆盖层整屏缓冲 / AI 对话"三笔账逐段量出来。
//!
//! # 为什么要有它
//!
//! 用户在任务管理器「进程」页看到的数字是 **私有工作集**（`PrivateWorkingSetSize`），
//! 它**不含**共享 DLL 的代码页。所以要回答"16MB 是哪来的"，必须自己按段采样，
//! 而不是靠"系统 DLL 大概多少"这类估计。
//!
//! # 走生产代码
//!
//! 不复制一份近似实现：真实 `Runtime::new` → 真实 `run_overlay`（建覆盖层 + 开面板）
//! → 真实 park 闸门。只读内存计数器，不改任何行为。
//!
//! 跑法：
//! ```text
//! cargo run --release --example mem_probe -- idle     # 什么都不开
//! cargo run --release --example mem_probe -- active   # 开鼠标拖尾（覆盖层需整屏缓冲）
//! ```
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::channel;
use std::time::Duration;

use curseen_ui::core::config::Config;
use curseen_ui::core::events::InputEvent;
use curseen_ui::platform::display::virtual_screen;
use curseen_ui::runtime::overlay::run_overlay;
use curseen_ui::runtime::Runtime;
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2,
};
use windows::Win32::System::Threading::GetCurrentProcess;

/// 两个口径：任务管理器看 `pws`（私有工作集），`commit` 是私有提交（堆的实际占用）。
#[derive(Clone, Copy)]
struct Snap {
    pws: usize,
    commit: usize,
}

fn snap() -> Snap {
    unsafe {
        let mut m = PROCESS_MEMORY_COUNTERS_EX2 {
            // ★ `cb` 就是版本号：填 EX2 的大小，系统才会填 `PrivateWorkingSetSize`。
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
            ..Default::default()
        };
        // 形参类型是 EX（收窄版），EX2 的指针按它传即可——版本由上面的 `cb` 表达。
        let p = (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX2).cast::<PROCESS_MEMORY_COUNTERS>();
        if GetProcessMemoryInfo(GetCurrentProcess(), p, m.cb).is_err() {
            return Snap { pws: 0, commit: 0 };
        }
        Snap {
            pws: m.PrivateWorkingSetSize,
            commit: m.PrivateUsage,
        }
    }
}

const MB: f64 = 1024.0 * 1024.0;

fn show(tag: &str, s: Snap) {
    // 覆盖层是否休眠 —— 休眠 = 整屏后备缓冲（1080p ≈ 8 MB）已释放。这是"整屏缓冲
    // 到底在不在"的直接判据，比内存数字更能定位归因。
    let parked = windui::platform::overlay_is_parked();
    println!(
        "{tag:<28} 私有工作集 {:>6.2} MB | 私有提交 {:>6.2} MB | 整屏缓冲 {}",
        s.pws as f64 / MB,
        s.commit as f64 / MB,
        if parked {
            "已释放(park)"
        } else {
            "常驻(活跃)"
        }
    );
}

fn main() {
    let active = std::env::args().nth(1).as_deref() == Some("active");
    let scene = std::env::args().nth(1).unwrap_or_else(|| "idle".into());
    println!(
        "场景：{}",
        match scene.as_str() {
            "active" => "active（开鼠标拖尾 → 覆盖层需整屏缓冲）",
            "ai" => "ai（只开 AI 对话复合脚本 → 不该唤醒覆盖层）",
            _ => "idle（什么都不开 → 覆盖层应休眠）",
        }
    );

    let vs = virtual_screen().or_fallback();
    println!(
        "虚拟屏 {}x{} → 整屏缓冲理论值 = {:.2} MB（宽×高×4）",
        vs.w,
        vs.h,
        (vs.w as f64) * (vs.h as f64) * 4.0 / MB
    );

    show("M0 进程裸基线", snap());

    let mut config = Config::default();
    if active {
        config.trail.trail_enabled = true;
    }
    // `ai` 场景：只开 AI 对话（复合脚本）。它是"一次性/开窗"脚本（只有 schema + run），
    // 一个像素都不画在覆盖层上 —— 覆盖层不该为它保持唤醒（整屏缓冲 + 60fps 重绘）。
    if std::env::args().nth(1).as_deref() == Some("ai") {
        config
            .lua_ext
            .composite_active
            .push("scripts/ai_chat".into());
    }
    let mut runtime = Runtime::new(config);
    runtime.set_screen(vs);
    let runtime = Rc::new(RefCell::new(runtime));
    show("M1 Runtime::new 后", snap());

    let (_tx, rx): (_, std::sync::mpsc::Receiver<InputEvent>) = channel();
    // ★ 覆盖层必须跑在**主线程**（`Rc<RefCell<Runtime>>` 非 Send，`run_overlay` 里还有
    //   大量 UI 线程专有状态）；采样放子线程——它只读进程级计数器，不碰 Runtime。
    let plan: Vec<(u64, &'static str)> = vec![
        (2500, "M2 覆盖层+面板起窗 2.5s"),
        (1500, "M3 稳态（+4.0s）"),
    ];
    std::thread::spawn(move || {
        for (wait, tag) in &plan {
            std::thread::sleep(Duration::from_millis(*wait));
            show(tag, snap());
        }
        // 裁剪工作集：只摘页不释放内存。若数字大幅回落 → 之前是"已释放但页还挂着"；
        // 若几乎不动 → 那些内存**真的还被持有**（归因指向常驻缓冲，而非回收滞后）。
        unsafe {
            let _ = windows::Win32::System::ProcessStatus::EmptyWorkingSet(GetCurrentProcess());
        }
        std::thread::sleep(Duration::from_millis(300));
        show("M4 EmptyWorkingSet 后", snap());
        std::thread::sleep(Duration::from_millis(2000));
        show("M5 稳态（+6.3s）", snap());
        std::process::exit(0);
    });

    run_overlay(runtime, rx);

    // `run_overlay` 正常永不返回（`app.run()` 的消息循环），采样线程跑完就 exit。
    std::process::exit(0);
}
