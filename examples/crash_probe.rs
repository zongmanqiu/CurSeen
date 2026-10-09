//! 崩溃兜底探针：**故意**触发一次访问冲突，验证 `platform::crash` 真的落盘。
//!
//! 为什么需要它：崩溃兜底这类"只在异常路径上跑"的代码，正常测试覆盖不到 —— 而它恰恰
//! 是最不能出错的代码（它自己错了就什么都不剩）。唯一的验法是**用一个独立进程真的崩一次**，
//! 再检查产物。这与 `examples/render_probe.rs` 的思路一致：绕开链路，直接驱动被验的对象。
//!
//! 跑法：
//! ```text
//! CURSEEN_CRASH_NO_DIALOG=1 cargo run --release --example crash_probe
//! ```
//! 期望：进程以访问冲突（`0xC0000005`）退出，且 `<exe 同级>/log/` 下多出一个
//! `curseen-crash-YYYYMMDD-HHMMSS.log`，内含异常码、exe 指纹、栈回溯与当天日志尾部。
//!
//! ★ 它会真的崩掉本进程 —— **不要**放进 `cargo test`。

fn main() {
    curseen_ui::core::logger::init_logging();
    curseen_ui::platform::crash::install();
    log::info!("crash_probe：即将触发一次访问冲突（这是预期的，不是故障）");

    // 给异步日志线程时间落盘。真实场景下程序已跑很久、日志文件早就在了；这里只是让探针
    // 的对照条件更接近真实（不然会看到"日志文件还不存在"这种只属于探针的假象）。
    std::thread::sleep(std::time::Duration::from_millis(400));

    // 对照：先看一眼 `std::backtrace` 在这个 exe 上能给出什么（它正是崩溃报告里用的那个）。
    println!(
        "[crash_probe] 正常上下文 std::backtrace：\n{}",
        std::backtrace::Backtrace::force_capture()
    );

    println!("[crash_probe] 触发空指针写……");
    // SAFETY: 这一行**就是要**制造一次访问冲突，用来验证 SEH 兜底真的接管了它。
    // 空指针写必然抛 0xC0000005，绝不会有"其实写成功了"的风险。
    unsafe {
        // `dangling_mut::<u8>()` 就是地址 1（与 `1usize as *mut u8` 同一地址，clippy 现行写法）。
        std::ptr::write_volatile(std::ptr::dangling_mut::<u8>(), 0xAB);
    }
    println!("[crash_probe] 不该到达这里 —— 说明兜底没生效");
}
