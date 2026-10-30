//! probe-group: fast
//! 桌面镜片（`ctx:backdrop_circle`）的真机判据：自身排除真的生效、镜片真的抓到了桌面。
//!
//! # 为什么需要真窗口
//!
//! 镜片的正确性只有一条硬判据：**抓屏里不能有自己的覆盖层**。这条只能用真窗口 + 真
//! `BitBlt` 验——纯函数测试只能证明圆形蒙版与反色数学，证明不了 `WDA_EXCLUDEFROMCAPTURE`
//! 在这个进程 / 这块屏上真的生效。
//!
//! 探针流程（每一步都有对照组，避免"跑了但没测"）：
//!
//! 1. 造一个 300×300 的**纯洋红**顶层窗，标题故意用 `CurSeen Overlay`；
//! 2. 先直接抓它中心的 80×80 —— 洋红占比必须 > 90%（证明夹具本身可见、抓屏抓得到它）；
//! 3. 调 `capture_exclusion::set_overlay_excluded(true)`，再抓同一块 —— 洋红必须 < 1%
//!    （证明排除生效）；
//! 4. 走真实 Lua `ctx:backdrop_circle`，断言返回 true、DrawList 出现一张圆形蒙版位图、
//!    位图里没有洋红、圆外 α=0。
//!
//! 跑法：`python build/probe.py run lens_probe -- --nocapture --test-threads=1`
//! （会短暂弹出一个洋红方块，结束自动销毁；不影响 CurSeen 主实例——按标题找到的窗口
//!   会先校验 PID 属于本进程。）

#![cfg(windows)]

use std::mem::size_of;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, UpdateWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowDisplayAffinity, RegisterClassExW,
    SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNOACTIVATE,
    WDA_EXCLUDEFROMCAPTURE, WINDOW_EX_STYLE, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    WS_VISIBLE,
};

use curseen_ui::core::draw::DrawList;
use curseen_ui::platform::capture_exclusion;
use curseen_ui::platform::display::{virtual_screen, ScreenRect};
use curseen_ui::platform::screenshot;
use curseen_ui::scripting::api_draw;
use curseen_ui::scripting::ScriptingEngine;

const SIDE: i32 = 300;
const CORE: i32 = 80;

unsafe extern "system" fn probe_wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

struct MagentaWindow {
    hwnd: HWND,
    brush: windows::Win32::Graphics::Gdi::HBRUSH,
}

impl MagentaWindow {
    fn spawn(rect: ScreenRect) -> Self {
        let brush = unsafe { CreateSolidBrush(COLORREF(0x00FF00FF)) };
        unsafe {
            let class: Vec<u16> = "CurSeenLensProbeCls\0".encode_utf16().collect();
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(probe_wndproc),
                hbrBackground: brush,
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassExW(&wc);
            let title: Vec<u16> = "CurSeen Overlay\0".encode_utf16().collect();
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE((WS_EX_TOPMOST | WS_EX_TOOLWINDOW).0),
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP | WS_VISIBLE,
                rect.x,
                rect.y,
                rect.w,
                rect.h,
                None,
                None,
                None,
                Some(std::ptr::null()),
            )
            .expect("CreateWindowExW 应成功");
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = UpdateWindow(hwnd);
            Self { hwnd, brush }
        }
    }

    fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let _ = UpdateWindow(self.hwnd);
        }
    }
}

impl Drop for MagentaWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
            let _ = DeleteObject(self.brush.into());
        }
    }
}

fn magenta_ratio(shot: &screenshot::Shot) -> f32 {
    let mut hit = 0usize;
    let mut total = 0usize;
    for px in shot.bgra.chunks_exact(4) {
        total += 1;
        if px[2] > 200 && px[1] < 60 && px[0] > 200 {
            hit += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        hit as f32 / total as f32
    }
}

#[test]
#[ignore = "真机：短暂弹出一个洋红顶层窗并抓屏；默认集合不跑"]
fn lens_excludes_overlay_and_returns_circle_bitmap() {
    let vs = virtual_screen();
    let win_rect = ScreenRect {
        x: vs.x + (vs.w - SIDE) / 2,
        y: vs.y + (vs.h - SIDE) / 2,
        w: SIDE,
        h: SIDE,
    };
    let core_rect = ScreenRect {
        x: win_rect.x + (SIDE - CORE) / 2,
        y: win_rect.y + (SIDE - CORE) / 2,
        w: CORE,
        h: CORE,
    };

    let win = MagentaWindow::spawn(win_rect);
    std::thread::sleep(std::time::Duration::from_millis(120));
    win.raise();
    std::thread::sleep(std::time::Duration::from_millis(80));

    // ① 对照组：排除之前，这块必须是洋红；否则后面的 0% 分不清是"排己生效"还是"夹具没显示"。
    let before = screenshot::grab(core_rect).expect("对照抓屏失败");
    let ratio_before = magenta_ratio(&before);
    assert!(
        ratio_before > 0.90,
        "对照组失败：洋红窗应占抓屏 >90%，实际 {:.2}%——夹具没显示或被别的窗口盖住",
        ratio_before * 100.0
    );

    // ② 排除后同块抓屏必须几乎无洋红。
    assert!(
        capture_exclusion::set_overlay_excluded(true),
        "set_overlay_excluded(true) 应成功（窗口标题与 PID 都匹配）"
    );
    let mut affinity: u32 = 0;
    let ok = unsafe { GetWindowDisplayAffinity(win.hwnd, &mut affinity) };
    assert!(ok.is_ok(), "GetWindowDisplayAffinity 失败");
    assert_eq!(
        affinity,
        WDA_EXCLUDEFROMCAPTURE.0,
        "窗口的捕获策略应已是排除"
    );

    win.raise();
    std::thread::sleep(std::time::Duration::from_millis(50));
    let after = screenshot::grab(core_rect).expect("排除后抓屏失败");
    let ratio_after = magenta_ratio(&after);
    assert!(
        ratio_after < 0.01,
        "排除后仍抓到洋红 {:.2}%——WDA_EXCLUDEFROMCAPTURE 没生效，镜片会套娃",
        ratio_after * 100.0
    );

    // ③ 真实 Lua 图元：返回 true，且产出一张圆外透明的位图。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
    let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
    let f: mlua::Function = eng
        .lua()
        .load(
            "return function(x, y, a) return ctx:backdrop_circle(x, y, 30, \"magnify\", 2.0, 255, 255, 255, a or 255) end",
        )
        .eval()
        .expect("Lua 片段应可编译");
    let cx = (win_rect.x + SIDE / 2) as f32;
    let cy = (win_rect.y + SIDE / 2) as f32;
    let mut list = DrawList::new();
    api_draw::set_draw_target(&mut list);
    let drawn: bool = f.call((cx, cy, 255.0)).expect("backdrop_circle 调用不应报错");
    // alpha 与其它颜色参数同口径（0–255）：128 应换算成 0.502 的图元乘子。
    let drawn_half: bool = f
        .call((cx, cy, 128.0))
        .expect("backdrop_circle(alpha=128) 调用不应报错");
    api_draw::clear_draw_target();
    assert!(drawn, "backdrop_circle 应返回 true");
    assert!(drawn_half, "backdrop_circle(alpha=128) 应返回 true");
    let alphas: Vec<f32> = list
        .ordered()
        .filter_map(|c| match c {
            curseen_ui::core::draw::DrawCmd::Image { alpha, .. } => Some(*alpha),
            _ => None,
        })
        .collect();
    assert_eq!(alphas.len(), 2, "两次采样应产出 2 张镜片位图");
    assert!((alphas[0] - 1.0).abs() < 0.001, "alpha=255 应完全不透明");
    assert!(
        (alphas[1] - 128.0 / 255.0).abs() < 0.01,
        "alpha=128 应换算成约 0.502，实际 {}",
        alphas[1]
    );
    let images = list.images();
    assert_eq!(images.len(), 2, "应产出 2 张镜片位图");
    let bmp = &images[0];
    assert_eq!((bmp.w, bmp.h), (60, 60), "半径 30 → 边长 60");
    let corner = &bmp.rgba[3];
    assert_eq!(*corner, 0, "圆外 α 必须是 0");

    // 镜片里不能有洋红（那说明抓到了自己的测试窗）。
    let mut magenta = 0usize;
    let mut opaque = 0usize;
    for px in bmp.rgba.chunks_exact(4) {
        if px[3] > 0 {
            opaque += 1;
            if px[0] > 200 && px[1] < 60 && px[2] > 200 {
                magenta += 1;
            }
        }
    }
    assert!(opaque > 1000, "镜片应大部分不透明，实际 {opaque} 像素");
    assert_eq!(magenta, 0, "镜片里出现洋红 = 抓到了自己");

    // 收尾：还原策略，窗口由 Drop 销毁。
    let _ = capture_exclusion::set_overlay_excluded(false);
}
