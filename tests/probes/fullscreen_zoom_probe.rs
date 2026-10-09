//! probe-group: fast
//! 全屏放大「真机探针」（`#[ignore]`，要真实桌面会话）。
//!
//! # 为什么还要为"复用画中画"的东西再写一份探针
//!
//! 全屏放大与画中画共用同一条链路，但**几何换了一个极端**：面板从"屏幕的一小块"
//! 变成"整个虚拟屏"。这会引出两个只有真机才答得了的问题：
//!
//! 1. **这么大一块 `UpdateLayeredWindow` 还能不能被 `WDA_EXCLUDEFROMCAPTURE` 排除？**
//!    不能的话，下一帧就会把自己抓进源图 → 又是画中画当初那个"无限套娃"。而且全屏
//!    时取样区**必然**落在面板内部（面板=整屏），所以这一次套娃会比画中画更彻底。
//! 2. **倍率有没有真的驱动采样？** 几何单测只证明算式，证明不了"抓屏 + GDI 呈现"
//!    这一整条在整屏尺寸下还活着。
//!
//! 判据与 `tests/pip_probe.rs` 同款：喂一块**纯品红**源图，再用生产那条路（`grab`，
//! 带 `CAPTUREBLT`）抓回来数品红占比 —— 0% 才是真的没把自己抓进来。

use curseen_ui::core::config::Config;
use curseen_ui::core::image::Shot;
use curseen_ui::modules::magnifier::fullscreen::FullscreenMagnifier;
use curseen_ui::platform::display::{virtual_screen, ScreenRect};
use curseen_ui::platform::pip_window::{PipLook, PipWindow, FULLSCREEN_WINDOW_TITLE};
use curseen_ui::platform::screenshot;

fn cfg_at(zoom: u32) -> Config {
    let mut cfg = Config::default();
    cfg.magnifier_fullscreen.enabled = true;
    cfg.magnifier_fullscreen.zoom = zoom;
    cfg
}

/// ★ 假说 1：整屏尺寸的 `present` 之后，这个窗**仍然**从抓屏里消失（不套娃）。
#[test]
#[ignore]
fn the_fullscreen_window_never_appears_in_its_own_source() {
    let vs = virtual_screen();
    let mut win = PipWindow::with_title(FULLSCREEN_WINDOW_TITLE);
    // 源图用纯品红；取样区只有面板的四分之一 → 实际放大 4×。
    let src = {
        let sw = vs.w / 4;
        let sh = vs.h / 4;
        let mut bgra = vec![0u8; (sw * sh * 4) as usize];
        for q in bgra.chunks_exact_mut(4) {
            q[0] = 0xFF; // B
            q[1] = 0x00; // G
            q[2] = 0xFF; // R
            q[3] = 0xFF;
        }
        Shot { w: sw, h: sh, bgra }
    };
    let look = PipLook {
        // 全屏时生产代码不画边框、也不画准星，这里保持一致。
        border_width: 0,
        ..PipLook::default()
    };
    win.present(&src, vs, &look).expect("全屏 present 应成功");
    assert!(
        win.is_excluded(),
        "WDA_EXCLUDEFROMCAPTURE 在整屏尺寸下也必须成功 —— 否则全屏放大必然套娃"
    );
    std::thread::sleep(std::time::Duration::from_millis(400));

    // 抓屏幕正中的一块（必在面板内），走生产同款 `grab`。
    let probe = ScreenRect::new(vs.x + vs.w / 2 - 200, vs.y + vs.h / 2 - 120, 400, 240);
    let got = screenshot::grab(probe).expect("抓屏应成功");
    let hit = magenta_ratio(&got.bgra);
    println!(
        "全屏窗已铺满 {vs:?}，生产链路抓回中心 {probe:?}：品红占比 = {hit:6.2}%  \
         （0 = 没把自己抓进来；接近 100 = 套娃）"
    );
    assert!(
        hit < 1.0,
        "全屏放大在自己的源图里看到了自己（{hit:.2}%）—— WDA 在整屏尺寸下失效了"
    );
}

/// ★ 假说 2：生产链路上，倍率确实驱动了采样（每一档都要成功出画）。
#[test]
#[ignore]
fn zoom_drives_the_sample_size_on_the_real_pipeline() {
    let mut m = FullscreenMagnifier::new();
    let mouse = (960.0, 540.0);
    let mut prev: Option<(i32, i32)> = None;
    for zoom in [1u32, 2, 3, 5] {
        let cfg = cfg_at(zoom);
        m.draw(&cfg, mouse);
        let cur = m
            .last_sample()
            .unwrap_or_else(|| panic!("zoom={zoom} 没出画（先确认上面的成功性前提）"));
        println!("全屏 zoom={zoom} → 取样区 {cur:?}");
        if let Some(p) = prev {
            assert!(
                cur.0 < p.0 && cur.1 < p.1,
                "倍率 {zoom} 没有把取样区缩小：上一档 {p:?} → 本档 {cur:?}"
            );
        }
        prev = Some(cur);
    }
}

/// 鼠标**完全静止**、只改倍率，也必须换帧（用户手停在滑条上的真实操作）。
#[test]
#[ignore]
fn changing_zoom_alone_refreshes_without_mouse_movement() {
    let mut m = FullscreenMagnifier::new();
    let mouse = (960.0, 540.0);
    m.draw(&cfg_at(2), mouse);
    let before = m.last_sample();
    m.draw(&cfg_at(8), mouse);
    let after = m.last_sample();
    assert!(
        before != after && after.is_some(),
        "鼠标静止时改倍率没有换帧：{before:?} → {after:?}"
    );
}

/// 连续多帧推进后窗必须真的在屏上（整屏尺寸下 `present` 有没有被系统拒掉）。
#[test]
#[ignore]
fn it_actually_shows_on_the_real_desktop() {
    let mut m = FullscreenMagnifier::new();
    let cfg = cfg_at(2);
    for i in 0..5 {
        m.draw(&cfg, (400.0 + i as f32 * 7.0, 300.0));
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
    assert!(m.has_frame(), "连续推进 5 帧仍没成功出画");
    assert!(m.is_shown(), "窗没有真的出现在屏上");
    println!("全屏放大已在屏上，取样区 = {:?}", m.last_sample());
    m.shutdown();
    assert!(!m.is_shown(), "shutdown 后必须收掉");
}

/// ★★★ 点击穿透必须**由系统裁决**是真的：全屏窗盖住整屏，若它吞掉鼠标消息，
/// "放大后鼠标无法点击"就成立。
///
/// 判据两条，都只看 Win32 的回答、不看我们自己写的样式常量：
/// 1. `GetWindowLongW(GWL_EXSTYLE)` 里带 `WS_EX_TRANSPARENT`；
/// 2. 对窗口正中做 `WM_NCHITTEST`，`DefWindowProc` 回 `HTTRANSPARENT`（-1）—— 系统就是
///    按这个结果把鼠标消息交给下层窗口的（-1 写死数值，避免依赖常量的类型）。
///
/// 为什么单靠第 1 条不够：样式只是"申请"，`WM_NCHITTEST` 才是"裁决"。分层窗在某些
/// 分支（如逐像素 alpha 命中测试）会绕过透明样式，必须实测。
#[test]
#[ignore]
fn the_fullscreen_window_is_click_through() {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GetWindowLongW, SendMessageW, WindowFromPoint, GWL_EXSTYLE, WM_NCHITTEST,
        WS_EX_TRANSPARENT,
    };

    let mut m = FullscreenMagnifier::new();
    let cfg = cfg_at(2);
    // 推进几帧把窗真正建出来、显示出来。
    for i in 0..3 {
        m.draw(&cfg, (500.0 + i as f32, 400.0));
        std::thread::sleep(std::time::Duration::from_millis(60));
    }
    assert!(m.is_shown(), "前置条件：全屏窗必须先真的出现在屏上");

    let title: Vec<u16> = FULLSCREEN_WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hwnd =
        unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title.as_ptr())) }.expect("按标题找不到全屏窗");

    let ex = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    assert_ne!(
        ex & WS_EX_TRANSPARENT.0,
        0,
        "全屏窗缺少 WS_EX_TRANSPARENT —— 它会把鼠标点击吞掉"
    );

    // WM_NCHITTEST 的 lparam 是**屏幕坐标**（低 16 位 x，高 16 位 y）；取屏幕正中，必在窗内。
    let vs = virtual_screen();
    let sx = vs.x + vs.w / 2;
    let sy = vs.y + vs.h / 2;
    let lp = LPARAM((((sy as i64) << 16) | (sx as i64 & 0xFFFF)) as isize);
    let hit = unsafe { SendMessageW(hwnd, WM_NCHITTEST, Some(WPARAM(0)), Some(lp)) };
    // 同一位置的 WindowFromPoint：穿透成立时它**不**应回我们的窗。
    let under = unsafe { WindowFromPoint(POINT { x: sx, y: sy }) };
    println!(
        "全屏窗 GWL_EXSTYLE=0x{ex:08X}（含 WS_EX_TRANSPARENT），\
         WM_NCHITTEST={}（-1=HTTRANSPARENT），WindowFromPoint={under:?}，self={hwnd:?}",
        hit.0
    );
    assert_eq!(
        hit.0, -1,
        "DefWindowProc 没回 HTTRANSPARENT —— 系统会把点击交给这个窗，穿透不成立"
    );
    assert!(
        under != hwnd,
        "WindowFromPoint 仍命中全屏窗 —— 点击会被它吞掉"
    );

    m.shutdown();
}

/// 数 BGRA 缓冲里"接近品红"的像素占比（%）。
fn magenta_ratio(px: &[u8]) -> f64 {
    if px.is_empty() {
        return 0.0;
    }
    let n = px.len() / 4;
    let mut hit = 0usize;
    for q in px.chunks_exact(4) {
        let (b, g, r) = (q[0], q[1], q[2]);
        if r > 180 && b > 180 && g < 60 {
            hit += 1;
        }
    }
    hit as f64 * 100.0 / n as f64
}
