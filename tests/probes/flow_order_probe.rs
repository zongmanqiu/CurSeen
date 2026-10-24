//! probe-group: fast
//!
//! 运行期顺序 / 门控探针（2026-10-24 静态审查之后的动态复核）。
//!
//! 两件事，都是**真调生产函数**，不做源码文本断言：
//!
//! 1. `gif_confirm_does_not_touch_delivery_side_effects`
//!    `Runtime::confirm_snap(GIF)` 是否只交出矩形，而不碰截图出口（剪贴板 / 留档）。
//!    做法：先跑一次 `Shot` 当**阳性对照**（证明量具看得见截图出口真的发生），再跑
//!    `Gif`。两次都用 `GetClipboardSequenceNumber` 的变化证明剪贴板写入真的发生。
//!
//!    ⚠ 本探针**会改写剪贴板**（这正是被测行为）。因此：
//!    - 默认拒绝运行，必须设 `CURSEEN_PROBE_ALLOW_CLIPBOARD=1`；
//!    - 开跑前备份 `CF_UNICODETEXT` / `CF_DIB` 的原始字节，结束时（含 panic）恢复；
//!    - 只恢复这两种格式；富文本 / 文件列表等其它格式无法完整还原——所以**别在
//!      剪贴板里有重要内容时跑**。
//!
//!    ★ 为什么不像第一版那样"屏蔽写入"：实测 `OpenClipboard` 在**同一进程**内是
//!    可重入的（两个线程都能开），跨进程的纯无窗进程也未能形成互斥。既然挡不住，
//!    就诚实测量 + 备份恢复，而不是假装内容没被动过。
//!
//! 2. `script_card_gates_enable_the_mouse_hook`
//!    四类脚本宿主（拖尾 / 悬浮标 / 点击效果 / `scripts/` 复合脚本）任一启用时，
//!    `Runtime::mouse_input_needed()` 是否为真；`Runtime::new` 是否把期望值同步到
//!    `MOUSE_HOOK_WANTED`；再真的 `InputHook::start` / `set_mouse_hook_enabled(false)`
//!    走一遍系统钩子的装与卸。钩子只装 ~几十毫秒，不吞键、不注入。
//!
//! 运行（不要直接 `cargo test` 整个文件，按仓里的探针规矩来）：
//!
//! ```text
//! CURSEEN_PROBE_ALLOW_CLIPBOARD=1 python build/probe.py run flow_order_probe
//! ```
//!
//! 注意：本文件**不是门禁**（探针不在默认测试列表里）。GIF 那条自 m08 起是**硬断言**
//! （`archive_delta=0 / seq_delta=0`），但阳性对照臂与剪贴板备份门都保留——必须先证明
//! 同一条链在 `Shot` 下两件事都看得见，否则"GIF 没写"可能只是量具坏了。

#![cfg(windows)]

use std::path::Path;
use std::rc::Rc;
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use curseen_ui::core::config::Config;
use curseen_ui::core::events::InputEvent;
use curseen_ui::core::geometry::{Rect, ScreenRect};
use curseen_ui::core::image::RgbaBitmap;
use curseen_ui::core::paths::{module_temp_dir, MODULE_SCREENSHOT};
use curseen_ui::platform::clipboard::sequence_number;
use curseen_ui::platform::input::hook::mouse_hook_wanted;
use curseen_ui::platform::input::keyboard;
use curseen_ui::platform::input::InputHook;
use curseen_ui::runtime::snap::SnapMode;
use curseen_ui::runtime::Runtime;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

// 与 `platform::clipboard` 同一套本地钉住的格式号（为一个常量开整个 Ole feature 不值）。
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;

// ─── 日志捕获（机器判据：某条日志出现 = 对应分支真的执行过） ───

static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn logs() -> &'static Mutex<Vec<String>> {
    LOGS.get_or_init(|| Mutex::new(Vec::new()))
}

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            logs()
                .lock()
                .unwrap()
                .push(format!("{} {}", record.level(), record.args()));
        }
    }

    fn flush(&self) {}
}

static CAPTURE: Capture = Capture;

fn init_log() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Info);
    });
    logs().lock().unwrap().clear();
}

fn logs_contain(needle: &str) -> bool {
    logs().lock().unwrap().iter().any(|l| l.contains(needle))
}

fn wait_for_log(needle: &str, within: Duration) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < within {
        if logs_contain(needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    logs_contain(needle)
}

fn dump_logs() {
    for l in logs().lock().unwrap().iter() {
        println!("  [log] {l}");
    }
}

// ─── 剪贴板备份 / 恢复（只覆盖 TEXT + DIB；其它格式无法还原） ───

struct ClipBackup {
    items: Vec<(u32, Vec<u8>)>,
}

impl ClipBackup {
    fn take() -> Self {
        let mut items = Vec::new();
        unsafe {
            if OpenClipboard(None).is_err() {
                return Self { items };
            }
            for fmt in [CF_UNICODETEXT, CF_DIB] {
                if IsClipboardFormatAvailable(fmt).is_err() {
                    continue;
                }
                let Ok(h) = GetClipboardData(fmt) else {
                    continue;
                };
                let hg = HGLOBAL(h.0);
                let ptr = GlobalLock(hg);
                if ptr.is_null() {
                    continue;
                }
                let len = GlobalSize(hg);
                if len > 0 && len <= 128 * 1024 * 1024 {
                    let bytes = std::slice::from_raw_parts(ptr.cast::<u8>(), len).to_vec();
                    items.push((fmt, bytes));
                }
                let _ = GlobalUnlock(hg);
            }
            let _ = CloseClipboard();
        }
        Self { items }
    }

    fn restore(&self) {
        unsafe {
            if OpenClipboard(None).is_err() {
                println!("WARN: 恢复剪贴板失败（OpenClipboard），原内容未能写回");
                return;
            }
            let _ = EmptyClipboard();
            for (fmt, bytes) in &self.items {
                let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else {
                    continue;
                };
                let ptr = GlobalLock(hg);
                if ptr.is_null() {
                    let _ = GlobalFree(Some(hg));
                    continue;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len());
                let _ = GlobalUnlock(hg);
                if SetClipboardData(*fmt, Some(HANDLE(hg.0))).is_err() {
                    let _ = GlobalFree(Some(hg));
                }
            }
            let _ = CloseClipboard();
        }
    }
}

/// Drop 兜底：即使断言 panic，也把剪贴板写回。
struct ClipRestore(Option<ClipBackup>);

impl ClipRestore {
    fn new(b: ClipBackup) -> Self {
        Self(Some(b))
    }
}

impl Drop for ClipRestore {
    fn drop(&mut self) {
        if let Some(b) = self.0.take() {
            b.restore();
        }
    }
}

// ─── 夹具 ───

fn png_count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(Result::ok)
                .filter(|e| {
                    e.path()
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("png"))
                })
                .count()
        })
        .unwrap_or(0)
}

fn frame() -> Rc<RgbaBitmap> {
    Rc::new(RgbaBitmap::new(320, 240, vec![180u8; 320 * 240 * 4]).expect("测试位图尺寸自洽"))
}

fn runtime() -> Runtime {
    let mut cfg = Config::default();
    // 不让剪贴板历史后台线程参与本次测量（它会开自己的隐藏窗与监听链）。
    cfg.clipboard.enabled = false;
    let mut rt = Runtime::new(cfg);
    rt.set_screen(ScreenRect::new(0, 0, 320, 240));
    rt
}

fn wait_ctrl_released() {
    let t0 = Instant::now();
    while keyboard::ctrl_held() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !keyboard::ctrl_held(),
        "Ctrl 正被按住：截图模式会被判成编辑态，阳性对照会失真；松开后重跑"
    );
}

// ─── 1. GIF 确认路径的顺序 ───

#[test]
#[ignore]
fn gif_confirm_does_not_touch_delivery_side_effects() {
    if std::env::var("CURSEEN_PROBE_ALLOW_CLIPBOARD").ok().as_deref() != Some("1") {
        println!(
            "SKIP: 本探针会改写剪贴板（这正是被测行为）；\
             确认剪贴板里没有重要内容后，设 CURSEEN_PROBE_ALLOW_CLIPBOARD=1 重跑"
        );
        return;
    }
    init_log();
    wait_ctrl_released();
    let dir = module_temp_dir(MODULE_SCREENSHOT);
    let mut rt = runtime();
    let rect = Rect::new(10.0, 10.0, 100.0, 80.0);

    let _restore = ClipRestore::new(ClipBackup::take());

    // 阳性对照：Shot 必须真的写留档、真的改动剪贴板序列号。
    // 如果这一臂不动，后面的 "GIF 也动了" 只能说明量具坏了，不能说明缺陷存在。
    let before = png_count(&dir);
    let seq0 = sequence_number();
    assert!(rt.snap.begin(SnapMode::Shot, frame()), "Shot 会话没开起来");
    rt.confirm_snap(rect);
    let seq1 = sequence_number();
    let shot_archive = png_count(&dir).saturating_sub(before);
    let shot_seq = seq1.wrapping_sub(seq0);
    println!("CONTROL(Shot)  archive_delta={shot_archive} seq_delta={shot_seq}");
    assert_eq!(shot_archive, 1, "阳性对照失败：Shot 没有产生留档，量具不可信");
    assert!(shot_seq > 0, "阳性对照失败：Shot 没有改动剪贴板，量具不可信");

    // 被测臂：GIF 按设计**只交矩形**。
    // ★ 留档文件名是秒级时间戳（`timestamp()`），两臂在同一秒内跑完会**同名覆盖**，
    //   让 `png_count` 的差值假报 0。跨过一秒再测，计数才代表"这一次真的写了新文件"。
    std::thread::sleep(Duration::from_millis(1100));
    logs().lock().unwrap().clear();
    let before = png_count(&dir);
    let seq0 = sequence_number();
    assert!(rt.snap.begin(SnapMode::Gif, frame()), "GIF 会话没开起来");
    rt.confirm_snap(rect);
    let seq1 = sequence_number();
    let gif_archive = png_count(&dir).saturating_sub(before);
    let gif_seq = seq1.wrapping_sub(seq0);
    let gif_ready = logs_contain("GIF 选框就绪");
    println!("GIF             archive_delta={gif_archive} seq_delta={gif_seq} arm_gif={gif_ready}");
    println!(
        "GIF_VERDICT={}",
        if gif_archive == 0 && gif_seq == 0 {
            "CLEAN"
        } else {
            "DEFECT"
        }
    );
    if gif_archive != 0 || gif_seq != 0 {
        dump_logs();
    }
    // GIF 分支跑到 = 确认链确实走到了「只交矩形」那一步；否则零副作用可能只是没测量。
    assert!(gif_ready, "GIF 分支没跑到，测量无效");
    // m08 回归断言：GIF 确认只交矩形，截图出口的两件事都不能发生。
    // ★ seq_delta 是全进程剪贴板序列号的差值：共享桌面上若有别的进程在同一毫秒内写
    //   剪贴板，这里会假红——先看 dump 出的日志有没有「GIF 选框就绪」，再重跑一次。
    assert_eq!(gif_archive, 0, "GIF 确认留下了截图留档（旧顺序回归）");
    assert_eq!(gif_seq, 0, "GIF 确认写了剪贴板（旧顺序回归）");
}

// ─── 2. 脚本宿主 → 鼠标门 → 真钩子 ───

#[test]
#[ignore]
fn script_card_gates_enable_the_mouse_hook() {
    init_log();

    let mut cfg = Config::default();
    cfg.clipboard.enabled = false;

    // 全关：门必须是关的（m03 的"按需"起点）。
    let rt_off = Runtime::new(cfg.clone());
    println!(
        "ALL_OFF          mouse_input_needed={} hook_wanted={}",
        rt_off.mouse_input_needed(),
        mouse_hook_wanted()
    );
    assert!(!rt_off.mouse_input_needed(), "全关时鼠标门不应打开");
    assert!(!mouse_hook_wanted(), "全关时钩子期望值不应为 true");
    drop(rt_off);

    // 四类脚本宿主（脚本本身走各自卡片/启用列表，不是单独的钩子路径）。
    let mut rt = Runtime::new(cfg.clone());
    rt.config.trail.trail_enabled = true;
    println!(
        "MouseTrail       mouse_input_needed={}",
        rt.mouse_input_needed()
    );
    assert!(rt.mouse_input_needed(), "MouseTrail 卡片开启后鼠标门应打开");
    rt.config.trail.trail_enabled = false;

    rt.config.trail.ripple_enabled = true;
    rt.config
        .trail
        .ripple_bindings
        .get_mut("left")
        .expect("出厂种子里有 left")
        .enabled = true;
    println!(
        "ClickFx          mouse_input_needed={}",
        rt.mouse_input_needed()
    );
    assert!(rt.mouse_input_needed(), "ClickFx 绑定开启后鼠标门应打开");
    rt.config
        .trail
        .ripple_bindings
        .get_mut("left")
        .unwrap()
        .enabled = false;
    rt.config.trail.ripple_enabled = false;

    rt.config.trail.hover_enabled = true;
    rt.config.trail.hover_active = Some(vec!["HoverDot/solid".into()]);
    println!(
        "HoverDot         mouse_input_needed={}",
        rt.mouse_input_needed()
    );
    assert!(rt.mouse_input_needed(), "HoverDot 槽位开启后鼠标门应打开");
    rt.config.trail.hover_enabled = false;
    rt.config.trail.hover_active = Some(vec![]);

    rt.config.lua_ext.composite_active = vec!["scripts/composite_demo".into()];
    println!(
        "composite        mouse_input_needed={}",
        rt.mouse_input_needed()
    );
    assert!(rt.mouse_input_needed(), "复合脚本启用后鼠标门应打开");
    drop(rt);

    // 生产接线：`Runtime::new` 末尾的 sync_input_gate 应把期望值写进原子。
    let mut cfg_script = cfg.clone();
    cfg_script.lua_ext.composite_active = vec!["scripts/composite_demo".into()];
    let rt_script = Runtime::new(cfg_script);
    println!("Runtime::new     hook_wanted={}", mouse_hook_wanted());
    assert!(
        mouse_hook_wanted(),
        "只开复合脚本时，Runtime::new 应请求鼠标钩子"
    );

    // 真装 / 真卸（不是只看原子；日志由 hook 线程在系统调用成功后才打）。
    let (tx, _rx) = mpsc::channel::<InputEvent>();
    let mut hook = InputHook::start(tx).expect("启动输入钩子线程失败");
    let installed = wait_for_log("鼠标钩子已安装（按需）", Duration::from_secs(2));
    println!("HOOK_INSTALLED={installed}");
    assert!(installed, "只开脚本时鼠标钩子没有真的装上");
    // 关闭方向也走生产接线：新建一个"全关"的 Runtime，其末尾 sync_input_gate
    // 把期望值写成 false，钩子线程应随即卸下。
    let rt_off2 = Runtime::new(cfg.clone());
    assert!(!mouse_hook_wanted(), "全关 Runtime 应把期望值写回 false");
    let uninstalled = wait_for_log("鼠标钩子已卸载（无鼠标功能在用）", Duration::from_secs(2));
    println!("HOOK_UNINSTALLED={uninstalled}");
    assert!(uninstalled, "关门后鼠标钩子没有真的卸下");
    drop(rt_off2);
    hook.stop();
    drop(rt_script);
}
