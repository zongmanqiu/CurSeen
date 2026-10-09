//! probe-group: fast
//! 便签「颜色里的不透明度到底有没有生效」的真机探针（`#[ignore]`，要真实桌面会话）。
//!
//! # 它要回答的问题
//!
//! 用户报：便签的**背景颜色 / 文字颜色 / 编辑区颜色**都可以设透明度，但设了跟没设一个样；
//! 只有单独那个「透明度」选项有效。
//!
//! 单测（`sticky.rs` 里 `layered_alpha_*` / `over_*` 那几条）只能证明**算式**对，证明不了
//! Windows 真把这两个数写进了两张窗的层里 —— `COLORREF` 丢字节、`LWA_ALPHA` 一窗一个值
//! 这类坑，光靠单测是看不见的。这条探针把便签真开出来，用
//! `GetLayeredWindowAttributes` 把**两张窗各自的 alpha 读回来**对账。
//!
//! 判据（两个场景）：
//! | 场景 | 总不透明度 | 背景色 alpha | 编辑区色 alpha | 期望：标题栏窗 | 期望：正文窗 |
//! |---|---|---|---|---|---|
//! | 出厂 | 100 | 235 | 235 | 235 | 235（两者相等） |
//! | 拉开 | 50 | 255 | 128 | 128 | 64（**必须不等**） |
//!
//! 第 2 行是这次修复的全部意义：**两张窗各持一个 alpha**。读回来还是相等的，就说明
//! "颜色里的透明度"依旧是摆设。
//!
//! 跑法：`cargo test --release --test note_alpha_probe -- --ignored --nocapture`

use std::time::{Duration, Instant};

use curseen_ui::platform::sticky::{layered_alpha, NoteCfg, StickyManager, BODY_CLASS, CLASS_NAME};
// ★ windows 0.62 里 `BOOL` 在 `windows::core`（`Win32::Foundation` 只有 HWND/RECT/LPARAM），
//   跟 `platform/window_probe.rs` 一致。
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetLayeredWindowAttributes, GetWindow, GetWindowRect, ShowWindow,
    GW_OWNER, SW_HIDE,
};

/// 枚举到的便签窗：句柄 + 类名。
fn collect_note_windows() -> Vec<(HWND, String)> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let out = &mut *(l.0 as *mut Vec<(HWND, String)>);
        let mut buf = [0u16; 64];
        let n = GetClassNameW(h, &mut buf);
        let name = String::from_utf16_lossy(&buf[..n as usize]);
        if name == CLASS_NAME || name == BODY_CLASS {
            out.push((h, name));
        }
        BOOL(1) // 继续枚举。
    }
    let mut found: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

/// 找一组"标题栏窗 + 它的正文窗"（正文窗的 owner 就是标题栏窗）。
fn find_pair() -> Option<(HWND, HWND)> {
    let all = collect_note_windows();
    for (body, cls) in &all {
        if cls != BODY_CLASS {
            continue;
        }
        let owner = unsafe { GetWindow(*body, GW_OWNER) }.unwrap_or(HWND(std::ptr::null_mut()));
        if !owner.0.is_null() && all.iter().any(|(f, c)| f.0 == owner.0 && c == CLASS_NAME) {
            return Some((owner, *body));
        }
    }
    None
}

/// 读回 Windows 真正在用的层 alpha（`LWA_ALPHA` 那个 0–255）。
fn layer_alpha_of(h: HWND) -> Option<u8> {
    let mut a: u8 = 0;
    unsafe {
        GetLayeredWindowAttributes(h, None, Some(&mut a), None)
            .ok()
            .map(|_| a)
    }
}

fn window_rect(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

/// 开一张便签 → 找到两张窗 → 立刻藏起来（别在用户屏幕上晃）→ 把 alpha 读回来。
///
/// 返回 `(标题栏窗 alpha, 正文窗 alpha, 标题栏窗矩形, 正文窗矩形)`。
fn measure(cfg: &NoteCfg) -> (u8, u8, RECT, RECT) {
    let mut mgr = StickyManager::new();
    mgr.ensure(cfg);
    mgr.spawn(cfg);
    let deadline = Instant::now() + Duration::from_secs(5);
    let pair = loop {
        if let Some(p) = find_pair() {
            break p;
        }
        if Instant::now() > deadline {
            panic!("5 秒内没建出便签窗（pump 线程没起来？桌面会话不可用？）");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let (frame, body) = pair;
    // 藏掉：探针只需要它们的**属性**，不需要真的显示给用户看。
    unsafe {
        let _ = ShowWindow(body, SW_HIDE);
        let _ = ShowWindow(frame, SW_HIDE);
    }
    let fa = layer_alpha_of(frame).expect("标题栏窗没挂上 LWA_ALPHA");
    let ba = layer_alpha_of(body).expect("正文窗没挂上 LWA_ALPHA");
    let fr = window_rect(frame);
    let br = window_rect(body);
    drop(mgr); // shutdown：销毁两张窗 + join pump 线程。
    (fa, ba, fr, br)
}

/// ★ 场景 1（出厂）：两个颜色的 alpha 一样 → 两张窗的层 alpha **相等**。
#[test]
#[ignore]
fn factory_note_gives_both_layers_the_same_alpha() {
    let cfg = NoteCfg::default();
    let want = layered_alpha(cfg.alpha, cfg.bg[3]);
    let (fa, ba, fr, br) = measure(&cfg);
    println!(
        "出厂：总={}% 背景色 a={} 编辑区 a={}",
        cfg.alpha, cfg.bg[3], cfg.edit[3]
    );
    println!("  读到：标题栏窗 alpha={fa}（期望 {want}）  正文窗 alpha={ba}（期望 {want}）");
    println!("  位置：标题栏窗 {fr:?}  正文窗 {br:?}");
    assert_eq!(fa, want, "标题栏窗 alpha = 总 × 背景色 alpha");
    assert_eq!(
        ba, want,
        "正文窗 alpha = 总 × 编辑区色 alpha（出厂两色同 alpha）"
    );
    // 正文窗必须整块落在标题栏窗里面（两窗不同步 = "正文停在原地"）。
    assert!(
        br.left >= fr.left && br.right <= fr.right,
        "正文窗横向必须在标题栏窗内"
    );
    assert!(
        br.top > fr.top && br.bottom <= fr.bottom,
        "正文窗必须在标题栏下方"
    );
}

/// ★★ 场景 2（这次修复的全部意义）：两个颜色的 alpha 拉开 → 两张窗的 alpha **必须不等**。
#[test]
#[ignore]
fn the_two_layers_really_carry_different_alpha() {
    // 只改 alpha 两个分量，RGB 沿用出厂色（`bg[3]`/`edit[3]` 是数组元素，
    // 没法用 `..Default::default()` 的写法，故保留逐项赋值）。
    let mut cfg = NoteCfg {
        alpha: 50, // 总不透明度 50%
        ..Default::default()
    };
    cfg.bg[3] = 255; // 标题栏：全实
    cfg.edit[3] = 128; // 正文：半透
    let want_frame = layered_alpha(cfg.alpha, cfg.bg[3]); // 128
    let want_body = layered_alpha(cfg.alpha, cfg.edit[3]); // 64
    assert_ne!(
        want_frame, want_body,
        "期望两侧就不该相等（否则这条用例没意义）"
    );

    let (fa, ba, _fr, _br) = measure(&cfg);
    println!("拉开：总={}% 背景色 a=255 编辑区 a=128", cfg.alpha);
    println!(
        "  读到：标题栏窗 alpha={fa}（期望 {want_frame}）  正文窗 alpha={ba}（期望 {want_body}）"
    );
    assert_eq!(fa, want_frame, "标题栏窗必须是『总 × 背景色 alpha』");
    assert_eq!(ba, want_body, "正文窗必须是『总 × 编辑区色 alpha』");
    assert_ne!(
        fa, ba,
        "★ 两张窗必须各自独立 —— 相等就说明颜色的 alpha 依旧是摆设"
    );
}
