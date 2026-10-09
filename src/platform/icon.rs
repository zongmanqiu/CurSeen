//! 托盘图标资源（原始 RGBA8，**非预乘**）。
//!
//! ★ 换 LOGO 的唯一入口是矢量源 `assets/icons/icon.svg`（red 版 `icon_red.svg`，
//!   结构一致、只差圆点颜色：青 `#3df8f8` / `red`），然后跑一次：
//!       cargo run --example gen_icon --features icon-gen
//!   它从 SVG **逐档**光栅化出（本模块只吃其中的托盘那份）：
//!     · `assets/icons/tray_<N>.rgba` / `tray_<N>_red.rgba`
//!       —— N = 16/20/24/28/32 的非预乘 RGBA8 裸字节，托盘按 DPI 挑档；
//!     · `assets/icons/app.ico` —— 12 档容器（16/20/24/28/32/40/48/56/64/96/128/256），
//!       由 `build.rs` 经 `rc.exe` 编进 exe 资源（名字 `MAINICON`），
//!       窗口标题栏 / 任务栏 / Alt-Tab / 资源管理器里看到的都是它。
//!   每一档都是**从矢量直接渲染**的，没有"一张大图反复缩放"——这正是换 SVG 源的
//!   意义：档位不是位图时代的包袱，而是"按目标尺寸精确渲染"的输出。
//!
//! ★★ **脚本不改图**（2026-09-27 用户口径）：源图画的是什么颜色，图标就是什么颜色，
//!   唯一的处理是光栅化。曾有「黑转透明」自动变换被否决——源图黑底就是黑底，
//!   偷偷把背景换透明等于篡改设计稿；`logo_is_not_blank` 里钉着"黑底必须还在"。
//!
//! DPI：托盘小图标是 16 逻辑 px，物理 px = 16·dpi/96，即 100%/125%/150%/175%/200%
//! 分别要 16/20/24/28/32。过去只给 32 一档，高 DPI 下被系统放大出锯齿；
//! 现在五档齐备，启动时按 `GetDpiForSystem` 定档（运行中途改缩放是罕见操作，不追）。
//!
//! 窗口 / 任务栏 / 资源管理器里的图标**不在这里**：那是 exe 资源里的 `MAINICON`
//! （见 `build/app.rc`），windui 建窗口类时 `LoadIconW` 自动取它，各窗都继承。

/// 当前系统 DPI（96 = 100% 缩放）。
///
/// 非 DPI-aware 进程恒返回 96——正好就是保底值，所以取不到 / 取不准都落在
/// "100% 那档"，不会让托盘图标消失。
fn system_dpi() -> u32 {
    unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }
}

/// 托盘尺寸档（与 `examples/gen_icon.rs` 的 `TRAY_SIZES` 同源同序）。
const TRAY_SIZES: [u32; 5] = [16, 20, 24, 28, 32];

/// 按 DPI 挑托盘档位边长：16 逻辑 px 的物理尺寸向上取档，超出 32 顶格。
fn tray_size_for_dpi(dpi: u32) -> u32 {
    let want = (16 * dpi / 96).max(16);
    TRAY_SIZES
        .iter()
        .copied()
        .find(|&s| s >= want)
        .unwrap_or(32)
}

/// 指定档位与变体的裸像素。私有：外部一律走 [`tray_icon`]（它负责按 DPI 选档）。
fn tray_rgba_for_size(n: u32, red: bool) -> &'static [u8] {
    match (n, red) {
        (16, false) => include_bytes!("../../assets/icons/tray_16.rgba"),
        (20, false) => include_bytes!("../../assets/icons/tray_20.rgba"),
        (24, false) => include_bytes!("../../assets/icons/tray_24.rgba"),
        (28, false) => include_bytes!("../../assets/icons/tray_28.rgba"),
        (16, true) => include_bytes!("../../assets/icons/tray_16_red.rgba"),
        (20, true) => include_bytes!("../../assets/icons/tray_20_red.rgba"),
        (24, true) => include_bytes!("../../assets/icons/tray_24_red.rgba"),
        (28, true) => include_bytes!("../../assets/icons/tray_28_red.rgba"),
        (_, true) => include_bytes!("../../assets/icons/tray_32_red.rgba"),
        _ => include_bytes!("../../assets/icons/tray_32.rgba"),
    }
}

/// 当前系统 DPI 下的托盘图标：`(边长, 非预乘 RGBA8 像素)`。
///
/// `red` 选红色变体（源 `icon_red.svg`：录制圆点是红的）。视频 / GIF
/// **真正开录之后**才切到它（倒计时不算——那 3 秒还能取消），停止后切回，
/// 切换动作见 `app::tray::set_tray_recording`。
///
/// ★ red 版名字说的是"红色那张图"：用途（录制期间）由调用方表达，
/// 文件名只描述它是什么。
pub fn tray_icon(red: bool) -> (u32, &'static [u8]) {
    let n = tray_size_for_dpi(system_dpi());
    (n, tray_rgba_for_size(n, red))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 五档 × 两版都要真的存在且长度精确——gen_icon 产物与代码不同步会在这里炸。
    #[test]
    fn tray_buffers_have_expected_lengths() {
        for red in [false, true] {
            for n in TRAY_SIZES {
                let px = tray_rgba_for_size(n, red);
                assert_eq!(
                    px.len(),
                    (n * n * 4) as usize,
                    "tray_{n}{}.rgba 尺寸不符，请重跑 gen_icon",
                    if red { "_red" } else { "" }
                );
            }
        }
    }

    /// DPI → 档位映射（100%–200% 每一档都落在自己的物理尺寸上）。
    #[test]
    fn dpi_maps_to_expected_tray_size() {
        assert_eq!(tray_size_for_dpi(96), 16);
        assert_eq!(tray_size_for_dpi(120), 20);
        assert_eq!(tray_size_for_dpi(144), 24);
        assert_eq!(tray_size_for_dpi(168), 28);
        assert_eq!(tray_size_for_dpi(192), 32);
        // 越界两侧：比 100% 还小按 16 保底；比 200% 大顶格 32。
        assert_eq!(tray_size_for_dpi(0), 16);
        assert_eq!(tray_size_for_dpi(240), 32);
    }

    /// 图标不该是全透明或全黑：源图黑底上画着青色图形，至少要有不透明像素。
    #[test]
    fn logo_is_not_blank() {
        let px = tray_rgba_for_size(32, false);
        let opaque = px.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(opaque > 0, "图标全透明，生成脚本或源图有问题");
        // 源图青色 #3df8f8：光栅化后也必须是青色系（G/B 高、R 低）。
        let cyan = px
            .chunks_exact(4)
            .filter(|p| p[3] > 128 && p[0] < 128 && p[1] > 180 && p[2] > 180)
            .count();
        assert!(cyan > 0, "没找到 LOGO 的青色图形 (#3df8f8)");
        // ★ 用户口径 2026-09-27：源图是黑底，脚本不得偷偷"黑转透明"。
        let non_opaque = px.chunks_exact(4).filter(|p| p[3] < 255).count();
        assert_eq!(
            non_opaque, 0,
            "托盘图标出现了非不透明像素：脚本又在改图（黑转透明被用户否决）"
        );
        let black = px
            .chunks_exact(4)
            .filter(|p| p[3] == 255 && p[0] < 32 && p[1] < 32 && p[2] < 32)
            .count();
        assert!(black > 0, "黑底丢了：源图是黑底设计稿，背景必须原样保留");
    }

    /// red 版：真有"红圆点"——没有它，录制态切换就是白切。
    #[test]
    fn red_logo_carries_a_red_dot() {
        let px = tray_rgba_for_size(32, true);
        let red = px
            .chunks_exact(4)
            .filter(|p| p[3] > 200 && p[0] > 180 && p[1] < 80 && p[2] < 80)
            .count();
        assert!(red > 4, "red 版托盘图标里没找到红圆点（red px = {red}）");
        // 两版必须肉眼可辨（录屏态要一眼认出来）：逐像素比对，若完全相同就是白切。
        // ★ 不钉具体配色——2026-10-01 用户改版后 red 版不再含青色箭头，旧断言
        //   "必须保留青色笔画"随之作废；功能契约只有"两版不同"这一条。
        let plain = tray_rgba_for_size(32, false);
        let same = px
            .chunks_exact(4)
            .zip(plain.chunks_exact(4))
            .filter(|(a, b)| a == b)
            .count();
        assert!(
            same < px.len() / 4,
            "red 版与普通版逐像素相同（{} / {}），录制态切换看不出来",
            same,
            px.len() / 4
        );
    }
}
