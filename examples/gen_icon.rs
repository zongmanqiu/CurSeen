//! SVG 图标源 → 全部位图产物。**换图标/换二维码的唯一入口**。
//!
//!     cargo run --example gen_icon --features icon-gen
//!
//! 源（人只写这些，全在 `assets/`）：
//!   · `icons/icon.svg`      —— 常规态 LOGO（黑底 + 青色图形）；
//!   · `icons/icon_red.svg`  —— 录制态（圆点红色），结构须与常规版一致；
//!   · `images/ALiPay.svg` / `images/WeChatPay.svg` —— 赞助二维码（真码矢量）。
//!
//! 产物（机器生成，不要手改）：
//!   · `icons/tray_<N>.rgba` / `icons/tray_<N>_red.rgba` —— 托盘托 five 档
//!     非**非预乘** RGBA8 裸像素（N = 16/20/24/28/32，覆盖 100%–200% 系统缩放），
//!     `platform::icon` 按 DPI 挑档后 `include_bytes!` 内嵌；
//!   · `icons/app.ico` —— 多尺寸容器（每档嵌 PNG），`build.rs` 经 `rc.exe` 编进 exe；
//!   · `images/ALiPay.png` / `images/WeChatPay.png` —— 二维码渲染成品
//!     （额外带 4 模块白边作静默区），`ui::pages::about` 原样内嵌。
//!
//! 为什么产物仍是位图：Windows 图标消费方（任务栏 / Alt-Tab / 资源管理器 / 托盘）
//! 由 explorer.exe 绘制且只认 ICO / HICON 位图，程序没运行时也要能显示——
//! SVG 只能在这里（构建期）光栅化，运行期不背任何 SVG 解码器。
//!
//! 依赖 `resvg`（optional，Apache-2.0 OR MIT）只在本 feature 下编译，
//! 不进发布 exe、不进默认构建。

use std::fs;
use std::path::PathBuf;

use resvg::tiny_skia::{self, Pixmap, Transform};
use resvg::usvg;

/// 仓库 `main/` 目录（examples 的 cwd 就是它，但用绝对化防呆）。
fn main_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

const ICON_DIR: &str = "assets/icons";
const IMG_DIR: &str = "assets/images";

/// 托盘图标尺寸档。`platform::icon::TRAY_SIZES` 必须与它同源同序。
const TRAY_SIZES: [u32; 5] = [16, 20, 24, 28, 32];

/// exe 图标容器要塞的尺寸档。100%→16/32，125%→20/40，150%→24/48，
/// 175%→28/56，200%→32/64；96/128/192 供资源管理器大图标视图；256 为最大档。
/// 少给一档不报错（系统就近缩放），但那一档会糊——给全就是了。
const ICO_SIZES: [u32; 12] = [16, 20, 24, 28, 32, 40, 48, 56, 64, 96, 128, 256];

/// 二维码四周额外留白（**源单位**）。二维码 SVG 模块铺满整个 viewBox
/// （实测四边静默区均为 0），QR 规范要求 ≥4 模块静默区；两张码模块都是
/// 10 单位宽，取 40 单位 = 4 模块。没有它，扫码器在贴近背景或小尺寸时
/// 识别率会明显下降。
const QR_QUIET_ZONE: f32 = 40.0;

fn main() {
    // ★ 产物路径一律基于 CARGO_MANIFEST_DIR 绝对化：`cargo run --example …` 的 cwd
    // 恰好是 `main/`，但换用 `--manifest-path` 调用时就会写错地方。
    let icon_dir = main_dir().join(ICON_DIR);
    let img_dir = main_dir().join(IMG_DIR);

    // ── 托盘 + exe 图标：icon.svg / icon_red.svg ──
    let normal = fs::read(icon_dir.join("icon.svg")).expect("读 icon.svg");
    let red = fs::read(icon_dir.join("icon_red.svg")).expect("读 icon_red.svg");

    for (tag, data) in [("", &normal), ("_red", &red)] {
        for n in TRAY_SIZES {
            let px = render_rgba(data, n, n, 0.0, None);
            let out = icon_dir.join(format!("tray_{n}{tag}.rgba"));
            fs::write(&out, &px).unwrap_or_else(|e| panic!("写 {} 失败: {e}", out.display()));
            println!("写出 {}（托盘 {n}×{n}，{} 字节）", out.display(), px.len());
        }
    }

    let sizes: Vec<(u32, Vec<u8>)> = ICO_SIZES
        .iter()
        .map(|&n| (n, encode_png(n, n, &render_rgba(&normal, n, n, 0.0, None))))
        .collect();
    let ico = encode_ico(&sizes);
    let ico_path = icon_dir.join("app.ico");
    fs::write(&ico_path, &ico).expect("写 app.ico");
    println!(
        "写出 {}（{} 字节，尺寸档 {ICO_SIZES:?}）",
        ico_path.display(),
        ico.len()
    );

    // ── 赞助二维码：SVG → PNG（带静默区）──
    for name in ["ALiPay.svg", "WeChatPay.svg"] {
        let data = fs::read(img_dir.join(name)).expect(name);
        let (w, h) = qr_output_size(&data);
        let px = render_rgba(&data, w, h, QR_QUIET_ZONE, Some([255, 255, 255]));
        let out = img_dir.join(name.replace(".svg", ".png"));
        let png = encode_png(w, h, &px);
        fs::write(&out, &png).unwrap_or_else(|e| panic!("写 {} 失败: {e}", out.display()));
        println!("写出 {}（{w}×{h}，{} 字节）", out.display(), png.len());
    }

    println!("提示：产物更新后要重新编译 exe（build.rs 走 rc.exe 编 ico；托盘 rgba 走 include_bytes!）。");
}

/// 把 SVG 光栅化到 w×h，返回**非预乘** RGBA8 扁平字节。
///
/// `pad`（源单位）为四周留白——二维码静默区用；`bg` 先铺底色
/// （二维码要白底，托盘图标透明底传 None）。源有无 viewBox 都行
/// （icon.svg 已标准化带 viewBox，按 1:1 缩放到目标尺寸）。
fn render_rgba(svg: &[u8], w: u32, h: u32, pad: f32, bg: Option<[u8; 3]>) -> Vec<u8> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default())
        .expect("SVG 解析失败（检查是否用了 filter/<image>/text 等不支持特性）");
    let size = tree.size();
    let scale = (w as f32 - 2.0 * pad) / size.width();
    let scale_h = (h as f32 - 2.0 * pad) / size.height();
    let s = scale.min(scale_h);

    let mut pixmap = Pixmap::new(w, h).expect("位图分配失败");
    match bg {
        Some([r, g, b]) => pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255)),
        None => pixmap.fill(tiny_skia::Color::TRANSPARENT),
    }
    let tf = Transform::from_scale(s, s).post_translate(pad, pad);
    resvg::render(&tree, tf, &mut pixmap.as_mut());

    // resvg 产出预乘 alpha 像素；托盘 CreateIconIndirect 与 PNG 条目都要非预乘。
    pixmap
        .data_mut()
        .chunks_exact_mut(4)
        .for_each(unpremultiply);
    pixmap.data().to_vec()
}

/// 预乘 → 非预乘（单像素，in place）。
fn unpremultiply(px: &mut [u8]) {
    let a = px[3] as u32;
    if a == 255 || a == 0 {
        return;
    }
    for c in &mut px[..3] {
        *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
    }
}

/// 二维码输出尺寸：viewBox + 两侧静默区，向上取整。
fn qr_output_size(svg: &[u8]) -> (u32, u32) {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).expect("SVG 解析失败");
    let w = tree.size().width() + 2.0 * QR_QUIET_ZONE;
    let h = tree.size().height() + 2.0 * QR_QUIET_ZONE;
    (w.ceil() as u32, h.ceil() as u32)
}

/// RGBA8 → PNG 字节（供 ICO 条目与二维码产物复用）。
fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("PNG 头写入失败");
        writer.write_image_data(rgba).expect("PNG 数据写入失败");
    }
    out
}

/// [(边长, PNG 字节)] → ICO 容器。每档直接嵌 PNG（Vista 起支持，比 BMP 条目小得多）。
fn encode_ico(sizes: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut entries = Vec::with_capacity(16 * sizes.len());
    let mut blobs = Vec::new();
    let mut offset = (6 + 16 * sizes.len()) as u32;
    for &(n, ref png) in sizes {
        entries.extend_from_slice(&[
            (n % 256) as u8, // 0 表示 256
            (n % 256) as u8,
            0, // 调色板色数（真彩填 0）
            0, // reserved
            1,
            0, // planes
            32,
            0, // bitCount
        ]);
        entries.extend_from_slice(&(png.len() as u32).to_le_bytes());
        entries.extend_from_slice(&offset.to_le_bytes());
        blobs.extend_from_slice(png);
        offset += png.len() as u32;
    }
    let mut ico = Vec::with_capacity(6 + entries.len() + blobs.len());
    ico.extend_from_slice(&0_u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1_u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    ico.extend_from_slice(&entries);
    ico.extend_from_slice(&blobs);
    ico
}
