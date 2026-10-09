//! `DrawCmd::TextInBox` 的像素级探针：**图例是否在键帽里居中 + 放不下会不会缩**。
//!
//! 为什么只能用像素说话：虚拟键盘的图例曾经在模块层自己凑坐标
//! （`x = 键帽左边 + (键宽 - estimate_width)/2`、`y = 键帽上边 + (键高 - 字号×1.25)/2`），
//! 而模块层没有字体 —— 估宽偏大 → 整体左偏（用户报 "Ctrl 靠左"），凑的盒高 ≠ 真实行高
//! → 垂直也不居中。这类偏差只有量墨迹才看得出：代码看着"对称"，像素上偏 2~3px。
//!
//! 本探针跑三组对照，全部走生产翻译层 `translate_offset` + `DWriteEngine`：
//!   1. `TextInBox` 的墨迹质心 vs 矩形中心（两轴，容差 ±1.5px）；
//!   2. 窄盒子里的长图例：自适应缩字号后必须**不溢出**（对照组用不缩放的
//!      `TextCentered` 证明"这段文字本来是放不下的"）；
//!   3. 旧算法（模块层凑坐标）的偏移量 —— 只报告，用来量化"修好了多少"。
//!
//! 跑法：`cargo run --release --example text_in_box_probe`（退出码 0 = 判据全过）。

use std::path::PathBuf;

use curseen_ui::core::color::Argb;
use curseen_ui::core::draw::DrawList;
use curseen_ui::core::geometry::{Point, Rect};
use curseen_ui::core::text_metrics::estimate_width;
use curseen_ui::runtime::render::translate_offset;

use windui::render::{DibTarget, RenderTarget};
use windui::text::DWriteEngine;

const W: u32 = 900;
const H: u32 = 420;
const BG: u8 = 240;

/// 质心与中心的允许偏差（像素）。字形本身左右不对称、抗锯齿量化都会带来 ~1px 抖动。
const TOL: f32 = 1.5;

/// 场景 1：宽盒子（键帽 240×120，字号 28）—— 只验居中。
const BOX_A: Rect = Rect {
    x: 60.0,
    y: 60.0,
    w: 240.0,
    h: 120.0,
};
/// 场景 2：窄盒子（键帽 40×120）—— 验"放不下就缩"。
const BOX_B: Rect = Rect {
    x: 60.0,
    y: 230.0,
    w: 40.0,
    h: 120.0,
};
/// 对照组（同一行、右侧）：同样的窄盒子，但不缩放（`TextCentered`）。
const BOX_C: Rect = Rect {
    x: 200.0,
    y: 230.0,
    w: 40.0,
    h: 120.0,
};
/// ★ 场景 2 的**扫描区**要大于盒子：文字一旦溢出，溢出部分落在盒外，
///   只扫盒子内部会把"溢出"量成"恰好等于盒宽"，判据就废了。
const SCAN_B: Rect = Rect {
    x: 20.0,
    y: 230.0,
    w: 140.0,
    h: 120.0,
};
const SCAN_C: Rect = Rect {
    x: 160.0,
    y: 230.0,
    w: 140.0,
    h: 120.0,
};
/// 场景 3：旧算法复刻（右侧大盒子），只报告偏移。
const BOX_OLD: Rect = Rect {
    x: 400.0,
    y: 60.0,
    w: 240.0,
    h: 120.0,
};

const FONT: f32 = 28.0;
/// 墨色（`Argb::new` 不是 const fn，故在 `main` 里构造）。
fn ink() -> Argb {
    Argb::new(0xFF, 0x00, 0x00, 0x00)
}

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
        println!("[{}] {label} —— {detail}", if ok { "PASS" } else { "FAIL" });
        if !ok {
            self.fails.push(label.to_string());
        }
    }
    fn note(&self, label: &str, detail: String) {
        println!("[INFO] {label} —— {detail}");
    }
}

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("temp/probe_out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("建输出目录失败：{e}");
        std::process::exit(2);
    }
    let mut report = Report::new();

    // ── 场景 1 + 2 + 对照 + 旧算法，一次画完（互不重叠，各占一块）──
    let mut list = DrawList::new();
    // 盒子的可见轮廓（浅灰描边），方便人眼看 PNG；不参与判据（判据只认黑墨）。
    for b in [BOX_A, BOX_B, BOX_C, BOX_OLD] {
        list.stroke_rect(b, 1.0, Argb::new(0xFF, 0xAA, 0xAA, 0xAA));
    }
    list.text_in_box(BOX_A, "Ctrl", FONT, ink(), None);
    list.text_in_box(BOX_B, "Home", FONT, ink(), None);
    // 对照：同一段字、同一个盒子宽度，但走"只居中不缩放"的 TextCentered。
    list.text_centered(
        Point::new(BOX_C.x + BOX_C.w / 2.0, BOX_C.y + BOX_C.h / 2.0),
        "Home",
        FONT,
        ink(),
        None,
    );
    // 旧算法复刻：模块层用 estimate_width 估宽、用 1.25em 凑盒高。
    let w = estimate_width("Ctrl", FONT);
    list.text(
        BOX_OLD.x + (BOX_OLD.w - w) / 2.0,
        BOX_OLD.y + (BOX_OLD.h - FONT * 1.25) / 2.0,
        "Ctrl",
        FONT,
        ink(),
        None,
    );

    let frame = rasterize(&list, &out_dir, "text_in_box.png");

    println!("══════ TextInBox：水平 + 垂直居中 ══════");
    let m = ink_metrics(&frame, BOX_A);
    let Some(m) = m else {
        eprintln!("场景 1 没有墨迹 —— 文字没画出来，判据无法成立");
        std::process::exit(1);
    };
    let cx = BOX_A.x + BOX_A.w / 2.0;
    let cy = BOX_A.y + BOX_A.h / 2.0;
    let dx = m.cx - cx;
    let dy = m.cy - cy;
    report.check(
        m.px > 50,
        "图例确实画出来了",
        format!("墨迹 {} px，外接盒 {}×{}", m.px, m.w, m.h),
    );
    report.check(
        dx.abs() <= TOL,
        "水平居中",
        format!("墨迹质心相对盒子中心 {dx:+.2}px（容差 ±{TOL}）"),
    );
    report.check(
        dy.abs() <= TOL,
        "垂直居中",
        format!("墨迹质心相对盒子中心 {dy:+.2}px（容差 ±{TOL}）"),
    );

    println!("\n══════ TextInBox：放不下就缩字号 ══════");
    let fit = ink_metrics(&frame, SCAN_B);
    let raw = ink_metrics(&frame, SCAN_C);
    let (Some(fit), Some(raw)) = (fit, raw) else {
        eprintln!("场景 2 没有墨迹 —— 文字没画出来，判据无法成立");
        std::process::exit(1);
    };
    report.check(
        raw.w as f32 > BOX_C.w,
        "对照组：不缩放时确实放不下",
        format!(
            "同字号下 'Home' 墨迹宽 {}px > 盒子宽 {:.0}px（这条让下一条判据有意义）",
            raw.w, BOX_C.w
        ),
    );
    // ★ 缩到 `MIN_FIT_SCALE`(55%) 就**不再缩**——那是刻意的下限：再小就读不出来了，
    //   宁可让它略微溢出。所以判据不是"绝对不溢出"，而是
    //   (a) 确实缩了（宽度明显小于原始），(b) 溢出被限制在 15% 以内。
    report.check(
        (fit.w as f32) <= raw.w as f32 * 0.7,
        "自适应确实缩了字号",
        format!(
            "墨迹宽 {}px vs 不缩放时的 {}px（缩到 {:.0}%）",
            fit.w,
            raw.w,
            fit.w as f32 / raw.w as f32 * 100.0
        ),
    );
    report.check(
        (fit.w as f32) <= BOX_B.w * 1.15,
        "溢出被限制在可控范围",
        format!(
            "墨迹宽 {}px vs 盒子宽 {:.0}px（触到 55% 下限后允许少量溢出）",
            fit.w, BOX_B.w
        ),
    );
    report.check(
        fit.px > 30 && fit.h > 0,
        "缩了但没缩没（仍可读）",
        format!(
            "墨迹 {} px，高 {}px（原始 {}px）",
            fit.px,
            fit.h,
            raw.w.max(1)
        ),
    );

    println!("\n══════ 旧算法（模块层凑坐标）偏移量 —— 仅报告 ══════");
    if let Some(old) = ink_metrics(&frame, BOX_OLD) {
        report.note(
            "旧算法偏移",
            format!(
                "质心相对盒子中心 ({:+.2}, {:+.2})px —— 与 TextInBox 的 ({dx:+.2}, {dy:+.2})px 对比",
                old.cx - (BOX_OLD.x + BOX_OLD.w / 2.0),
                old.cy - (BOX_OLD.y + BOX_OLD.h / 2.0)
            ),
        );
    }

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

fn rasterize(list: &DrawList, out_dir: &std::path::Path, name: &str) -> Vec<u8> {
    let mut ann = vec![0_u8; (W * H * 4) as usize];
    let mut engine = DWriteEngine::new();
    {
        let mut target = DibTarget::new(&mut ann, W, H);
        let mut canvas = target.make_canvas(&mut engine, 1.0);
        translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
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

// ─────────────────────────── 墨迹度量 ───────────────────────────

struct Ink {
    px: u32,
    w: u32,
    h: u32,
    cx: f32,
    cy: f32,
}

/// 一个矩形内的**黑色墨迹**：像素数、外接盒、按暗度加权的质心。
///
/// 只认"明显比底暗"的像素（阈值 128，底是 240），浅灰的盒子轮廓（170 左右）不会被算进去。
fn ink_metrics(frame: &[u8], r: Rect) -> Option<Ink> {
    let x0 = r.x as u32;
    let y0 = r.y as u32;
    let x1 = (r.x + r.w) as u32;
    let y1 = (r.y + r.h) as u32;
    let mut px = 0_u32;
    let (mut sx, mut sy, mut sw) = (0.0_f32, 0.0_f32, 0.0_f32);
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (u32::MAX, 0_u32, u32::MAX, 0_u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let l = lum(px_at(frame, W, x, y)) as f32;
            if l >= 128.0 {
                continue;
            }
            let k = 255.0 - l;
            px += 1;
            sx += (x as f32 + 0.5) * k;
            sy += (y as f32 + 0.5) * k;
            sw += k;
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    if px == 0 || sw == 0.0 {
        return None;
    }
    Some(Ink {
        px,
        w: max_x - min_x + 1,
        h: max_y - min_y + 1,
        cx: sx / sw,
        cy: sy / sw,
    })
}

fn px_at(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (buf[i], buf[i + 1], buf[i + 2])
}

fn lum(p: (u8, u8, u8)) -> u8 {
    ((p.0 as u32 * 299 + p.1 as u32 * 587 + p.2 as u32 * 114) / 1000).min(255) as u8
}

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
