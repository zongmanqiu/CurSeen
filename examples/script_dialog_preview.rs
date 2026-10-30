//! 脚本文本窗的**真像素**预览：把生产内容树离屏画成 PNG，供人（和我自己）看一眼。
//!
//! 为什么要单独拍一张：`ui::script_dialog::content_tree` 是界面形状，单元测试只能证明
//! "文本会被切成几行、窗高怎么算"，**证明不了好不好看**。滚动条样式、按钮圆角、配色层级
//! 这类判断只能在真实像素上做——所以这里跑生产同一条路：
//!
//! 1. 用**生产执行器** `ext_runner::run_ext_sync` 跑出厂脚本 `scripts/1.image_info`，
//!    喂一张可复算的合成贴图，拿到真实的输出文本（不在 example 里复刻统计逻辑）；
//! 2. 把文本塞进**生产同一个** `content_tree`，用 windui 的 `Tree` + `DWriteEngine`
//!    离屏排版绘制；
//! 3. 两档并排拍：左＝信息窗（`ext:result` 那档），右＝输入窗（`ext:input` 那档）。
//!
//! 跑法：`cargo run --release --example script_dialog_preview`
//! 产物：`../temp/script_dialog_preview/contact.png`（退出码 0 = 两扇都画出了墨迹）。

use std::collections::HashMap;
use std::path::PathBuf;

use curseen_ui::core::image::RgbaBitmap;
use curseen_ui::scripting::api_ext::ExtCtx;
use curseen_ui::scripting::ext_runner::run_ext_sync;
use curseen_ui::ui::script_dialog::{content_tree, height_of, inert_click, DIALOG_W};

use tiny_skia::{Color, FilterQuality, Paint, Pixmap, PixmapMut, PixmapPaint, Rect, Transform};
use windui::core::Tree;
use windui::prelude::*;
use windui::render::SkiaCanvas;
use windui::text::DWriteEngine;

/// 合成贴图：1920×1080，红蓝渐变 + 顶部一条全透明带（专门用来验"透明像素不参与平均"）。
fn sample_image() -> RgbaBitmap {
    let (w, h) = (1920u32, 1080u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let transparent_band = y < h / 4;
            rgba[i] = (x % 256) as u8;
            rgba[i + 1] = (y % 256) as u8;
            rgba[i + 2] = 128;
            rgba[i + 3] = if transparent_band { 0 } else { 255 };
        }
    }
    RgbaBitmap { w, h, rgba }
}

/// 把一棵内容树离屏画进一张新 Pixmap（尺寸 = 窗宽 × 按内容算的窗高）。
///
/// ★ 画出来的是**客户区**：标题栏是系统画的，离屏拍不到。所以"标题栏写着脚本名"这件事
///   本预览证明不了，只能证明正文区域的形状（记事本式、无圆角无边框）与按钮不被裁。
fn render_window(text: &str, editable: bool) -> Pixmap {
    let h = height_of(text, editable);
    let root = content_tree(
        editable,
        signal(text.to_string()),
        inert_click(),
        inert_click(),
    );
    let mut tree = Tree::new();
    let id = root.build(&mut tree);
    tree.root = Some(id);
    let mut engine = DWriteEngine::new();
    tree.layout_root(Size::new(DIALOG_W, h), &mut engine);

    let mut pm = Pixmap::new(DIALOG_W as u32, h as u32).expect("建离屏缓冲");
    // 窗底色与生产一致（`tokens::BG` = 白），windui 的控件由主题取色。
    pm.fill(Color::from_rgba8(255, 255, 255, 255));
    {
        let mut canvas = SkiaCanvas::with_text(&mut pm, &mut engine, 1.0);
        tree.paint(&mut canvas);
    }
    pm
}

/// 贴到合成图上（位置 = 桌面里的落点），并画一条 1px 深描边把 frameless 窗的边缘显出来。
fn blit(desktop: &mut PixmapMut<'_>, win: &Pixmap, x: i32, y: i32) {
    let (w, h) = (win.width() as f32, win.height() as f32);
    let mut paste = PixmapPaint::default();
    // 1:1 贴图：用 Nearest，插值只会让 1px 的线与字发虚。
    paste.quality = FilterQuality::Nearest;
    desktop.draw_pixmap(x, y, win.as_ref(), &paste, Transform::identity(), None);

    let mut line = Paint::default();
    line.set_color_rgba8(70, 78, 80, 255);
    line.anti_alias = false;
    // 描边只画上下两条边：左右两条由两扇窗之间的间距承担，免得看起来粘连。
    let top = Rect::from_xywh(x as f32, y as f32, w, 1.0).expect("描边矩形");
    let bottom = Rect::from_xywh(x as f32, y as f32 + h - 1.0, w, 1.0).expect("底边矩形");
    desktop.fill_rect(top, &line, Transform::identity(), None);
    desktop.fill_rect(bottom, &line, Transform::identity(), None);
}

/// 墨迹判据：窗里必须**有非白像素**——画不出来的"功能存在"不算效果。
fn ink_pixels(pm: &Pixmap) -> usize {
    let mut n = 0usize;
    for px in pm.pixels().iter() {
        if !(px.red() > 250 && px.green() > 250 && px.blue() > 250) {
            n += 1;
        }
    }
    n
}

/// 底部 60px 这条带子里的非白像素——按钮就住在这里。
///
/// 判"按钮有没有被裁"不能看整窗墨迹（正文自己就很多），必须只看**尾部**：
/// 上一版的缺陷正是"正文照画、尾部被裁"。
fn bottom_band_ink(pm: &Pixmap) -> usize {
    let (w, h) = (pm.width(), pm.height());
    let top = h.saturating_sub(60);
    let mut n = 0usize;
    for y in top..h {
        for x in 0..w {
            let p = match pm.pixel(x, y) {
                Some(p) => p,
                None => continue,
            };
            if !(p.red() > 250 && p.green() > 250 && p.blue() > 250) {
                n += 1;
            }
        }
    }
    n
}

fn main() {
    let out_dir = PathBuf::from("../temp/script_dialog_preview");
    std::fs::create_dir_all(&out_dir).expect("建输出目录");

    // ① 生产执行器跑真脚本（面板脚本页那种"没有贴图上下文"的调用不在这儿，我们喂图）。
    let scripts = PathBuf::from("assets/lua");
    let text = run_ext_sync(
        &scripts,
        "scripts/1.image_info",
        &HashMap::new(),
        Some(ExtCtx {
            image: sample_image(),
            hwnd: 0,
        }),
    )
    .expect("出厂脚本 1.image_info 必须跑通");
    println!("脚本输出：\n{text}\n");

    // ② 三扇并排画：八行的信息窗、输入窗、以及一扇内容远超窗高上限的（OCR 结果那种）。
    let long_text = (1..=40)
        .map(|i| format!("第 {i} 行识别出的文字，用来把正文撑到超过窗高上限"))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    let info = render_window(&text, false);
    let input = render_window("sunset-4k.png", true);
    let tall = render_window(&long_text, false);
    for (name, pm) in [("信息窗", &info), ("输入窗", &input), ("超长窗", &tall)] {
        let ink = ink_pixels(pm);
        // ★ 底部按钮带必须有墨：这是"按钮没被遮挡"的像素级判据（单测那边用真实排版边界判）。
        assert!(
            bottom_band_ink(pm) > 300,
            "{name} 底部按钮带几乎没有墨迹——按钮被裁掉了"
        );
        println!("{name} {}x{} 非白像素 {ink}", pm.width(), pm.height());
    }

    // ③ 合成到一块"桌面"上，好看清无边框窗的边缘与阴影层级。
    let gap = 24i32;
    let pad = 32i32;
    let widest = info.height().max(input.height()).max(tall.height()) as i32;
    let dw = (DIALOG_W * 3 + gap * 2 + pad * 2) as u32;
    let dh = (widest + pad * 2) as u32;
    let mut desk = Pixmap::new(dw, dh).expect("建合成缓冲");
    desk.fill(Color::from_rgba8(236, 239, 242, 255));
    {
        let mut band = Paint::default();
        band.set_color_rgba8(222, 226, 230, 255);
        band.anti_alias = false;
        // 上半压深一档：窗的边缘在纯白底上根本看不出来。
        desk.as_mut().fill_rect(
            Rect::from_xywh(0.0, 0.0, dw as f32, dh as f32 * 0.28).expect("带子矩形"),
            &band,
            Transform::identity(),
            None,
        );
    }
    let mut desk_mut = desk.as_mut();
    blit(&mut desk_mut, &info, pad, pad);
    blit(&mut desk_mut, &input, pad * 2 + DIALOG_W + gap, pad);
    blit(&mut desk_mut, &tall, pad * 3 + DIALOG_W * 2 + gap * 2, pad);

    let png_path = out_dir.join("contact.png");
    std::fs::write(&png_path, desk.encode_png().expect("编码 PNG")).expect("写出 contact.png");
    println!("已写出：{}", png_path.display());

    println!("三扇窗都有墨迹、底部按钮带都露得出来 ✓");
}
