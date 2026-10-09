//! 形状渲染的像素级探针：箭头「一体成型」与序号「圆内居中」。
//!
//! 存在理由：这两条判据都只能用像素说话。
//!
//!   · **半透明下箭头交界处是否二次叠加**——几何上"杆和头有没有重叠"看不出来，
//!     但像素上一叠加就必然更暗（α 合成两次）。这个探针直接量最暗值。
//!   · **数字是否在圆心**——`DrawCmd::Text` 的 y 到底被渲染层当成"基线"还是"排版盒顶"
//!     是这条判据的全部关键，只有量出来才算数。
//!
//! 走的是 `curseen-offscreen-render-probe` 那套：真实控制器 → 真实 `DrawList` →
//! 生产翻译层 `translate_offset` → `DibTarget` → 预乘合成 → 数像素。全程绕开窗口与输入。
//!
//! 跑法：`cargo run --release --example shape_probe`（退出码 0 = 全部判据通过）。

use std::path::PathBuf;

use curseen_ui::core::color::Argb;
use curseen_ui::core::draw::DrawList;
use curseen_ui::core::geometry::{Point, Rect};
use curseen_ui::modules::drawing::controller::{DrawMode, DrawingController, Tool};
use curseen_ui::modules::drawing::pens::{Pen, PenStyle};
use curseen_ui::modules::drawing::settings::DrawingSettings;
use curseen_ui::platform::display::ScreenRect;
use curseen_ui::runtime::render::translate_offset;

use windui::render::{DibTarget, RenderTarget};
use windui::text::DWriteEngine;

const W: u32 = 900;
const H: u32 = 700;

/// 合成桌面的底色。取浅灰：黑笔哪怕只有 25% α 也能拉开明显差距。
const BG: u8 = 240;

/// 探针是否全部通过。
struct Report {
    fails: Vec<String>,
    checks: usize,
}

impl Report {
    fn new() -> Self {
        Self {
            fails: Vec::new(),
            checks: 0,
        }
    }

    fn check(&mut self, ok: bool, label: &str, detail: String) {
        self.checks += 1;
        let mark = if ok { "PASS" } else { "FAIL" };
        println!("[{mark}] {label} —— {detail}");
        if !ok {
            self.fails.push(label.to_string());
        }
    }

    fn note(&self, label: &str, detail: String) {
        println!("[INFO] {label} —— {detail}");
    }
}

// ─────────────────────────── 场景几何（三块互不重叠） ───────────────────────────

/// 箭头：水平放置，便于"杆内部 / 三角形内部"各取一个采样点。
const ARROW_FROM: Point = Point { x: 160.0, y: 180.0 };
const ARROW_TO: Point = Point { x: 660.0, y: 180.0 };
/// 箭头扫描区（含整根杆与整个三角头，留足余量给抗锯齿）。
const ARROW_REGION: Rect = Rect {
    x: 100.0,
    y: 100.0,
    w: 620.0,
    h: 160.0,
};

/// 序号贴纸的圆心。
const STAMP_CENTER: Point = Point { x: 250.0, y: 420.0 };
/// 序号扫描区。
const STAMP_REGION: Rect = Rect {
    x: 190.0,
    y: 360.0,
    w: 120.0,
    h: 120.0,
};

/// 文字锚点（用来判定 `DrawCmd::Text` 的 y 语义）。
///
/// 位置刻意放在 x ≥ 545：`Notify` 的提示浮字（"白板模式"）画在屏幕中心，
/// 实测占 `x 376..524 / y 328..390`，扫描区与它必须完全错开。
const TEXT_ANCHOR: Point = Point { x: 560.0, y: 470.0 };
/// 字号 = 笔宽 × 4，故笔宽 15 → 60px。
const TEXT_PEN_WIDTH: u16 = 15;
const TEXT_SIZE: f32 = (TEXT_PEN_WIDTH as f32) * 4.0;
const TEXT_REGION: Rect = Rect {
    x: 545.0,
    y: 395.0,
    w: 155.0,
    h: 210.0,
};

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("temp")
        .join("shape_probe");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("建输出目录失败 {}: {e}", out_dir.display());
        std::process::exit(2);
    }
    println!("输出目录：{}", out_dir.display());

    let screen = ScreenRect {
        x: 0,
        y: 0,
        w: W as i32,
        h: H as i32,
    };
    let mut report = Report::new();

    // ── 场景 1：半透明箭头（α=128 ≈ 50%）──
    println!("\n══════ 箭头：半透明下的二次叠加 ══════");
    let arrow_pen = Pen {
        color: Argb::new(0x80, 0x00, 0x00, 0x00),
        width: 24,
        style: PenStyle::Solid,
    };
    let frame = render_scene(
        &out_dir,
        "arrow_alpha.png",
        &arrow_pen,
        |c| {
            c.set_tool(Tool::Arrow);
            stroke(c, &[ARROW_FROM, ARROW_TO]);
        },
        screen,
    );
    verify_arrow_single_blend(&mut report, &frame);

    // ── 场景 2：序号贴纸（实心圆 + 反色数字）──
    println!("\n══════ 序号：数字是否在圆心 ══════");
    let stamp_pen = Pen {
        color: Argb::new(0xFF, 0x00, 0x00, 0x00),
        width: 4,
        style: PenStyle::Solid,
    };
    let frame = render_scene(
        &out_dir,
        "stamp_center.png",
        &stamp_pen,
        |c| {
            c.set_tool(Tool::Stamp);
            c.begin_stroke(STAMP_CENTER, 0.0);
            c.end_stroke();
        },
        screen,
    );
    verify_stamp_centered(&mut report, &frame);

    // ── 场景 3：文字锚点语义（只报告，不作为判据）──
    println!("\n══════ 文字：`DrawCmd::Text` 的 y 是基线还是盒顶 ══════");
    let text_pen = Pen {
        color: Argb::new(0xFF, 0x00, 0x00, 0x00),
        width: TEXT_PEN_WIDTH,
        style: PenStyle::Solid,
    };
    let frame = render_scene(
        &out_dir,
        "text_anchor.png",
        &text_pen,
        |c| {
            c.set_tool(Tool::Text);
            c.begin_stroke(TEXT_ANCHOR, 0.0);
            c.end_stroke();
            assert!(c.commit_text("Hg"), "文字应落笔");
        },
        screen,
    );
    report_text_anchor(&mut report, &frame);

    println!("\n──────────────────────────────");
    if report.fails.is_empty() {
        println!("全部 {} 项判据通过", report.checks);
    } else {
        println!("{} / {} 项判据失败：", report.fails.len(), report.checks);
        for f in &report.fails {
            println!("  · {f}");
        }
        std::process::exit(1);
    }
}

// ─────────────────────────── 渲染 ───────────────────────────

/// 用给定的画笔与动作跑一遍真实控制器，渲出一张合成后的不透明画面。
///
/// 走**白板模式**：`Backdrop::Veiled(白 α=255)` 会在 `paint_into` 里跳过底图 blit，
/// 于是画面底色是确定的纯白——把"底图未知"这个变量从判据里拿掉。
fn render_scene(
    out_dir: &std::path::Path,
    name: &str,
    pen: &Pen,
    build: impl FnOnce(&mut DrawingController),
    screen: ScreenRect,
) -> Vec<u8> {
    let settings = DrawingSettings {
        hotkeys: DrawingSettings::default().hotkeys,
        pens: vec![pen.clone(), pen.clone()],
    };
    let mut c = DrawingController::new(settings);
    c.set_screen(screen);
    c.toggle(DrawMode::Whiteboard, 0.0);
    build(&mut c);

    let mut list = DrawList::new();
    c.draw(&mut list);

    let mut ann = vec![0_u8; (W * H * 4) as usize];
    let mut engine = DWriteEngine::new();
    {
        let mut target = DibTarget::new(&mut ann, W, H);
        let mut canvas = target.make_canvas(&mut engine, 1.0);
        translate_offset(&list, &mut *canvas, 1.0, 0.0, 0.0);
    }

    let mut frame = opaque(W, H, (BG, BG, BG));
    composite_premultiplied(&mut frame, &ann);
    let path = out_dir.join(name);
    if let Err(e) = write_png(&path, W, H, &frame) {
        eprintln!("写 PNG 失败 {}: {e}", path.display());
        std::process::exit(2);
    }
    println!("已写出 {}", path.display());
    frame
}

fn stroke(c: &mut DrawingController, pts: &[Point]) {
    c.begin_stroke(pts[0], 0.0);
    for p in &pts[1..] {
        c.extend_stroke(*p);
    }
    c.end_stroke();
}

// ─────────────────────────── 判据 1：箭头只混一次 ───────────────────────────

/// 半透明箭头**不允许**出现二次叠加：整根箭头（杆 + 三角 + 交界）只应是"底色混一次"。
///
/// 判据用"最暗像素"：设底色 `B`、γ=128/255，混一次得 `B × (1-γ)`，混两次得 `B × (1-γ)²`。
/// `B=255` 时分别是 127 与 63，两者相差一倍，中间没有任何可能混淆的地带，所以阈值
/// 取「不允许出现比混一次还暗 15 级以上的像素」这条硬边界即可。
///
/// 底色**从画面里量**而不是写死：白板模式的 `Backdrop::Veiled(白 α=255)` 会把合成底
/// 整个盖掉，真实的底是纯白而非探针合成时铺的灰。量出来才不会把"底变了"误判成"叠加了"。
fn verify_arrow_single_blend(report: &mut Report, frame: &[u8]) {
    // 左上角远离一切图元，取那里的亮度作底色。
    let bg = lum(px(frame, W, 2, 2)) as f32;
    let a = 0x80 as f32 / 255.0;
    let once = (bg * (1.0 - a)).round();
    let twice = (bg * (1.0 - a) * (1.0 - a)).round();

    let mut min_lum = 255_u8;
    let mut inked = 0_u32;
    let mut doubled = 0_u32;
    for y in ARROW_REGION.y as u32..(ARROW_REGION.y + ARROW_REGION.h) as u32 {
        for x in ARROW_REGION.x as u32..(ARROW_REGION.x + ARROW_REGION.w) as u32 {
            let p = px(frame, W, x, y);
            let l = lum(p);
            if l < (bg as u8).saturating_sub(20) {
                inked += 1;
                min_lum = min_lum.min(l);
                // "比混一次还暗 15 级以上" = 一定混了不止一次。
                if (l as f32) < once - 15.0 {
                    doubled += 1;
                }
            }
        }
    }

    report.note(
        "预期色阶",
        format!("底色 {bg:.0}：混一次 ≈ {once}，混两次 ≈ {twice}"),
    );
    report.check(
        inked > 2000,
        "箭头确实画出来了",
        format!("着墨 {inked} px，最暗 {min_lum}"),
    );
    report.check(
        doubled == 0,
        "箭头无二次叠加（一体成型）",
        format!(
            "比混一次更暗的像素 {doubled} px（最暗 {min_lum}，需 ≥ {:.0}）",
            once - 15.0
        ),
    );
}

// ─────────────────────────── 判据 2：数字在圆心 ───────────────────────────

/// 数字墨迹的几何中心必须落在圆心。
///
/// # 容差为什么是 ±1.5px 而不是 0
///
/// 残余误差有三个来源，都已量化，且**没有**一个是"没居中"：
///
///   · **像素量化 0.5px**——判据取的是"墨迹像素的外接盒中心"，整像素边界天然有半像素
///     的不确定度（一个跨 `[232, 268)` 的圆，其暗像素外接盒中心是 249.5 而几何中心是 250）。
///   · **大写高比值 ≤0.03 em**——渲染层垂直居中用的是"数字墨迹高 = 大写高"这条业界
///     通行的比值（`DIGIT_CAP_HEIGHT_EM`，windui 不给真实大写高）。各 Windows UI 字体
///     在 0.70–0.73 之间，取 0.72 后最坏偏差 0.02 em ≈ 0.4px @ 字号 20.7。
///   · **字形自身的不对称 ≤1px**——水平方向按**排版宽度**居中（不按墨迹居中），这样
///     "1"…"9" 的**列位置一致**；而 "1" 的墨迹在自己那一格里本来就偏左约 1px。
///
/// 修复前实测偏移 **+13.00px**（数字几乎掉出圆外），现在应落在 1.5px 内——
/// 对直径 36px 的徽标来说已是"看着就在中心"。
fn verify_stamp_centered(report: &mut Report, frame: &[u8]) {
    const TOL: f32 = 1.5;
    // ① 圆心的实测位置：整块区域里所有"暗像素"的包围盒中心。
    let mut min_x = u32::MAX;
    let mut min_y = u32::MAX;
    let mut max_x = 0_u32;
    let mut max_y = 0_u32;
    for y in STAMP_REGION.y as u32..(STAMP_REGION.y + STAMP_REGION.h) as u32 {
        for x in STAMP_REGION.x as u32..(STAMP_REGION.x + STAMP_REGION.w) as u32 {
            if lum(px(frame, W, x, y)) < 128 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if min_x == u32::MAX {
        report.check(
            false,
            "序号圆确实画出来了",
            "扫描区内一个暗像素都没有".into(),
        );
        return;
    }
    let cx = (min_x + max_x) as f32 / 2.0;
    let cy = (min_y + max_y) as f32 / 2.0;
    let r_x = (max_x - min_x) as f32 / 2.0;
    let r_y = (max_y - min_y) as f32 / 2.0;
    report.check(
        (r_x - r_y).abs() <= 1.0 && r_x > 8.0,
        "序号圆确实是圆",
        format!("外接盒 {r_x}×{r_y}（半径），圆心 ({cx:.1},{cy:.1})"),
    );

    // ② 数字墨迹：圆内**远离边缘**的高亮像素（3px 让开圆环与它的抗锯齿）。
    let inset = r_x.min(r_y) - 3.0;
    let mut t_min_x = u32::MAX;
    let mut t_min_y = u32::MAX;
    let mut t_max_x = 0_u32;
    let mut t_max_y = 0_u32;
    let mut text_px = 0_u32;
    for y in STAMP_REGION.y as u32..(STAMP_REGION.y + STAMP_REGION.h) as u32 {
        for x in STAMP_REGION.x as u32..(STAMP_REGION.x + STAMP_REGION.w) as u32 {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if (dx * dx + dy * dy).sqrt() > inset {
                continue;
            }
            if lum(px(frame, W, x, y)) > 170 {
                text_px += 1;
                t_min_x = t_min_x.min(x);
                t_min_y = t_min_y.min(y);
                t_max_x = t_max_x.max(x);
                t_max_y = t_max_y.max(y);
            }
        }
    }
    if text_px == 0 {
        report.check(false, "数字确实画出来了", "圆内没有高亮墨迹".into());
        return;
    }
    let tcx = (t_min_x + t_max_x) as f32 / 2.0;
    let tcy = (t_min_y + t_max_y) as f32 / 2.0;
    let (dx, dy) = (tcx - cx, tcy - cy);

    report.check(
        true,
        "数字确实画出来了",
        format!(
            "数字墨迹 {text_px} px，外接盒 x {}..{} y {}..{}",
            t_min_x, t_max_x, t_min_y, t_max_y
        ),
    );
    report.check(
        dx.abs() <= TOL && dy.abs() <= TOL,
        "数字在圆心",
        format!("数字墨迹中心相对圆心偏移 ({dx:+.2}, {dy:+.2})，容差 ±{TOL:.2}"),
    );
}

// ─────────────────────────── 判据 3（报告）：文字锚点语义 ───────────────────────────

/// 报告文字墨迹相对锚点的位置，用来判定 `DrawCmd::Text` 的 y 语义。
///
/// 两种约定的差别很大：`y = 排版盒顶` 时墨迹顶边 ≈ y；`y = 基线` 时墨迹顶边 ≈ y − 大写高。
/// 字号 60 时这一差别约 40px，量得出来。
fn report_text_anchor(report: &mut Report, frame: &[u8]) {
    let mut min_y = u32::MAX;
    let mut max_y = 0_u32;
    let mut min_x = u32::MAX;
    for y in TEXT_REGION.y as u32..(TEXT_REGION.y + TEXT_REGION.h) as u32 {
        for x in TEXT_REGION.x as u32..(TEXT_REGION.x + TEXT_REGION.w) as u32 {
            if lum(px(frame, W, x, y)) < 128 {
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                min_x = min_x.min(x);
            }
        }
    }
    if min_y == u32::MAX {
        report.note("文字锚点", "扫描区内没有墨迹（文字没画出来？）".into());
        return;
    }
    report.note(
        "文字锚点",
        format!(
            "锚点 ({:.0},{:.0})，字号 {TEXT_SIZE}；墨迹 x 从 {} 起，y ∈ {}..{}；\
             墨迹顶边相对锚点 = {:+.0}px（≈0 → y 是盒顶；≈-0.7em ≈ -42 → y 是基线）",
            TEXT_ANCHOR.x,
            TEXT_ANCHOR.y,
            min_x,
            min_y,
            max_y,
            min_y as f32 - TEXT_ANCHOR.y
        ),
    );
}

// ─────────────────────────── 小工具 ───────────────────────────

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (buf[i], buf[i + 1], buf[i + 2])
}

/// 亮度（Rec. 601 加权）。判据只需要单调性，不需要精确色度。
fn lum(p: (u8, u8, u8)) -> u8 {
    ((p.0 as u32 * 299 + p.1 as u32 * 587 + p.2 as u32 * 114) / 1000).min(255) as u8
}

/// 纯色不透明底。
fn opaque(w: u32, h: u32, c: (u8, u8, u8)) -> Vec<u8> {
    let mut buf = vec![0_u8; (w * h * 4) as usize];
    for p in buf.chunks_exact_mut(4) {
        p[0] = c.0;
        p[1] = c.1;
        p[2] = c.2;
        p[3] = 255;
    }
    buf
}

/// 把**预乘** RGBA 的标注层以 source-over 叠到不透明底上（与 `UpdateLayeredWindow` 同义）。
fn composite_premultiplied(dst: &mut [u8], src: &[u8]) {
    for (d, s) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
        let a = s[3] as f32 / 255.0;
        let inv = 1.0 - a;
        d[0] = (s[0] as f32 + d[0] as f32 * inv).round().clamp(0.0, 255.0) as u8;
        d[1] = (s[1] as f32 + d[1] as f32 * inv).round().clamp(0.0, 255.0) as u8;
        d[2] = (s[2] as f32 + d[2] as f32 * inv).round().clamp(0.0, 255.0) as u8;
        d[3] = 255;
    }
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc
        .write_header()
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    writer
        .write_image_data(rgba)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}
