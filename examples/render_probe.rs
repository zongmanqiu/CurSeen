//! 标注模式的**离屏**渲染验收探针。
//!
//! # 为什么需要它（而不是截屏）
//!
//! CurSeen 启动即提权（`platform::elevate::relaunch_elevated`，对齐 1.0.0：只有提权进程的
//! 低级钩子才收得到"提权窗口前台"时的输入）。后果是**非提权的探针注入不进任何输入**——
//! UIPI（User Interface Privilege Isolation）会静默丢弃跨完整性级别的合成键鼠，实测
//! `SendMessage` 返回 `err=5 ERROR_ACCESS_DENIED`。于是「启动 exe → 注入 Alt+G → 拖一笔 →
//! 截图比对」这条路在非提权环境下**永远走不通**，而它失败的样子和"功能坏了"一模一样。
//!
//! 本探针绕开整条窗口/输入链路：直接驱动真实的 [`DrawingController`] 跑一遍完整标注会话，
//! 产出真实的 `DrawList`，再经**生产用的同一个** [`translate_offset`] 渲进一块 RGBA 缓冲，
//! 最后合成到一张合成的"桌面"上存 PNG。像素级判据在进程内直接算，不依赖任何外部工具。
//!
//! 换句话说：它验的是"算法层到像素"这一段（也就是标注功能自己的全部代码），
//! 不验"Windows 把窗口画到屏幕上"这一段（那是 windui 的职责，另有面板探针覆盖）。
//!
//! # 跑法
//!
//! ```text
//! cargo run --example render_probe
//! ```
//!
//! 产出写到 `temp/render_probe/`：两张 PNG（屏幕标注 / 白板）+ 一份文字判据报告（stdout）。
//! 判据不通过时进程退出码为 1。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use curseen_ui::core::draw::{DrawCmd, DrawList};
use curseen_ui::core::geometry::{Point, Rect};
use curseen_ui::modules::drawing::controller::{DrawMode, DrawingController, Tool};
use curseen_ui::modules::drawing::settings::DrawingSettings;
use curseen_ui::modules::drawing::toolbar::{self, ToolbarAction};
use curseen_ui::platform::display::ScreenRect;
use curseen_ui::runtime::render::translate_offset;

use windui::render::{DibTarget, RenderTarget};
use windui::text::DWriteEngine;

/// 探针画布 = 1080p。与 `ScreenRect` 同一坐标系（世界坐标 = 虚拟桌面物理像素）。
const W: u32 = 1920;
const H: u32 = 1080;

/// `translate_offset` 的计时轮数。
const TIMING_ROUNDS: u32 = 40;

// ── 合成桌面用的颜色（刻意远离工具栏的浅色，免得污染"工具栏在哪"的扫描）──
const DESK_TOP: (u8, u8, u8) = (66, 78, 96);
const DESK_BOTTOM: (u8, u8, u8) = (80, 92, 110);
const PAGE: (u8, u8, u8) = (200, 208, 218);
const PAGE_TITLE: (u8, u8, u8) = (150, 158, 170);
const PAGE_BAR: (u8, u8, u8) = (176, 184, 194);

/// 文档"页面"在合成桌面上的位置：标注内容大多落在它上面，黑笔才有对比度。
const PAGE_RECT: Rect = Rect {
    x: 80.0,
    y: 60.0,
    w: 1760.0,
    h: 900.0,
};

/// 判据区域（世界坐标）。刻意互不重叠，且只放一种颜色的墨。
const BLACK_WAVE: Rect = Rect {
    x: 160.0,
    y: 150.0,
    w: 600.0,
    h: 100.0,
};
const GREEN_WAVE: Rect = Rect {
    x: 160.0,
    y: 400.0,
    w: 600.0,
    h: 100.0,
};
const RED_BAR: Rect = Rect {
    x: 280.0,
    y: 300.0,
    w: 640.0,
    h: 45.0,
};
/// 工具栏所在的那条底带。扫工具栏几何时只看这里——屏幕中间的提示浮字是纯白，
/// 混进扫描会把"工具栏外框"拉成整个屏幕。
const BAR_BAND: Rect = Rect {
    x: 0.0,
    y: 950.0,
    w: 1920.0,
    h: 130.0,
};

/// 探针是否全部通过（任一判据失败即置 false，最后决定退出码）。
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
}

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("temp")
        .join("render_probe");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("建输出目录失败 {}: {e}", out_dir.display());
        std::process::exit(2);
    }
    println!("输出目录：{}", out_dir.display());

    // ── 桌面 / 白板两种模式各跑一遍 ──
    let screen = ScreenRect {
        x: 0,
        y: 0,
        w: W as i32,
        h: H as i32,
    };

    let mut report = Report::new();

    for (mode, name, whiteboard) in [
        (DrawMode::Screen, "annotation_screen.png", false),
        (DrawMode::Whiteboard, "annotation_whiteboard.png", true),
    ] {
        // 1) 驱动真实控制器跑完一整场标注，拿到真实 DrawList。
        let list = build_scene(mode, screen);
        let path = out_dir.join(name);
        println!("\n══════ {name}（{mode:?}）══════");
        print_drawlist_stats(&list);

        // 2) 经生产翻译层渲进 RGBA 缓冲。
        let mut ann = vec![0_u8; (W * H * 4) as usize];
        let mut engine = DWriteEngine::new();
        render_once(&list, &mut ann, &mut engine);

        // 3) 合成到合成桌面上（分层窗口在系统眼里就是把预乘 RGBA 叠到屏幕上，
        //    白板模式没有"下面的东西"，直接铺白）。
        let mut frame = if whiteboard {
            opaque(W, H, (255, 255, 255))
        } else {
            make_desktop(W, H)
        };
        composite_premultiplied(&mut frame, &ann);

        // 4) 存 PNG。
        if let Err(e) = write_png(&path, W, H, &frame) {
            eprintln!("写 PNG 失败 {}: {e}", path.display());
            std::process::exit(2);
        }
        println!("已写出 {}", path.display());

        // 5) 判据。
        //    工具栏几何在**白板**帧上验：屏幕标注帧的底图是真实桌面的冻结帧，底带里全是
        //    真实像素（浏览器/任务栏多为近白），"近白 = 工具栏"会把整条底带判成工具栏
        //    （实测量出 1920×118 的整条带）。白板帧底色是纯白，用 `is_bar_paint` 排除纯白
        //    即可唯一框出工具栏。几何与模式无关，验一次就够。
        if whiteboard {
            verify_geometry(&mut report, &frame);
        }
        verify_ink(&mut report, &frame, whiteboard);
        verify_dashes(&mut report, &frame);
        verify_shapes_are_outlines(&mut report, &frame, whiteboard);

        // 6) 性能。
        report_perf(&list, &mut engine, &mut ann);
    }

    // 退出标注后工具栏必须消失（否则一块点不动的工具栏永远留在屏上）。
    verify_toolbar_leaves_after_exit(&mut report);

    // 截图编辑态工具栏（★ 2026-10-03 任务 2：多一颗打勾「完成」）。单测只验"有指令"，
    // 这里验**真有像素** —— 图标几何写错（坐标算反、画到格外）时指令数照样正常。
    verify_shot_edit_toolbar_pixels(&mut report, screen, &out_dir);

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

// ─────────────────────────── 场景 ───────────────────────────

fn build_scene(mode: DrawMode, screen: ScreenRect) -> DrawList {
    let mut c = DrawingController::new(DrawingSettings::default());
    c.set_screen(screen);
    c.toggle(mode, 0.0);

    // ① 黑细笔（4px 实线）：一条正弦波，用来和下面的绿虚线做"段数"对照。
    //    中线取 200 —— 必须落在 `BLACK_WAVE` 里，否则判据区域里一个像素都没有。
    c.set_active_pen(0);
    c.set_tool(Tool::Pen);
    stroke(&mut c, &wave(200.0));

    // ② 红粗笔（30px，alpha=0x40）：像荧光笔那样划一道重点。
    c.set_active_pen(1);
    stroke(
        &mut c,
        &[
            Point::new(280.0, 322.0),
            Point::new(500.0, 318.0),
            Point::new(700.0, 326.0),
            Point::new(920.0, 320.0),
        ],
    );

    // ③ 绿细笔（4px，**虚线**）：同形状的波，段数必须远多于 ①。
    c.set_active_pen(2);
    stroke(&mut c, &wave(450.0));

    // ④ 直线 / 箭头 / 矩形 / 椭圆：四个形状工具各来一次。
    c.set_active_pen(0);
    for (tool, from, to) in [
        (
            Tool::Line,
            Point::new(1000.0, 140.0),
            Point::new(1180.0, 250.0),
        ),
        (
            Tool::Arrow,
            Point::new(1000.0, 320.0),
            Point::new(1180.0, 430.0),
        ),
        (
            Tool::Rect,
            Point::new(1250.0, 140.0),
            Point::new(1450.0, 260.0),
        ),
        (
            Tool::Ellipse,
            Point::new(1250.0, 320.0),
            Point::new(1450.0, 440.0),
        ),
    ] {
        c.set_tool(tool);
        stroke(&mut c, &[from, to]);
    }

    // ⑤ 序号贴纸：两下点击，编号应是 1、2。
    c.set_tool(Tool::Stamp);
    for center in [Point::new(1040.0, 620.0), Point::new(1160.0, 620.0)] {
        c.begin_stroke(center, 0.0);
        c.end_stroke();
    }

    // ⑥ 文字：字号 = 笔宽 × 4。细笔 → 16px，粗笔 → 120px。
    //
    // 时序照抄真实链路：MouseDown 记锚点 → MouseUp（模块会转发）→ 用户在真窗口里敲完回车
    // 才 commit。这里少了 up 就会停在 pressed 上，后面的悬停判定跟着失效。
    c.set_active_pen(0);
    c.set_tool(Tool::Text);
    c.begin_stroke(Point::new(200.0, 640.0), 0.0);
    c.end_stroke();
    assert!(c.commit_text("重点！"), "文字应落笔");

    c.set_active_pen(1);
    c.begin_stroke(Point::new(200.0, 800.0), 0.0);
    c.end_stroke();
    assert!(c.commit_text("标注"), "文字应落笔");

    // ⑦ 工具栏的高亮 + 悬停两种态都要出现：当前工具 = 画笔1，鼠标停在「箭头」上。
    c.set_active_pen(0);
    c.set_tool(Tool::Pen);
    let bar = toolbar::layout(screen, 1.0, None, false).bar;
    let arrow = toolbar::layout(screen, 1.0, None, false)
        .cells
        .iter()
        .find(|(a, _)| *a == ToolbarAction::Arrow)
        .map(|(_, r)| *r)
        .expect("工具栏应有箭头按钮");
    // 不按下地划过 → 只更新悬停，不落笔。
    c.extend_stroke(arrow.center());
    assert_eq!(c.toolbar_hover(), Some(ToolbarAction::Arrow));
    assert!(
        arrow.center().y > bar.y,
        "箭头按钮应落在工具栏外框内（悬停点不该跑到别处）"
    );

    let mut list = DrawList::new();
    c.draw(&mut list);
    list
}

/// 一条正弦波，`y` 是中线。
fn wave(y: f32) -> Vec<Point> {
    let (x0, x1) = (160.0_f32, 760.0_f32);
    let n = 120;
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            let x = x0 + (x1 - x0) * t;
            Point::new(x, y + 28.0 * (t * std::f32::consts::TAU * 1.6).sin())
        })
        .collect()
}

/// 把一串点当成一次完整的按下—拖动—抬起。
fn stroke(c: &mut DrawingController, pts: &[Point]) {
    let Some((first, rest)) = pts.split_first() else {
        return;
    };
    c.begin_stroke(*first, 0.0);
    for p in rest {
        c.extend_stroke(*p);
    }
    c.end_stroke();
}

// ─────────────────────── 渲染 / 合成 / 存盘 ───────────────────────

fn render_once(list: &DrawList, ann: &mut [u8], engine: &mut DWriteEngine) {
    let mut target = DibTarget::new(ann, W, H);
    let mut canvas = target.make_canvas(engine, 1.0);
    // scale=1 / dx=dy=0：探针缓冲就是整块虚拟桌面，世界坐标与画布坐标 1:1。
    translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
}

/// 一张没被标注覆盖的"桌面"。颜色刻意做得离工具栏的浅色很远，这样"工具栏在哪"的
/// 像素扫描不会有假阳性。
fn make_desktop(w: u32, h: u32) -> Vec<u8> {
    let mut buf = vec![0_u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let p = Point::new(x as f32, y as f32);
            let t = y as f32 / h as f32;
            let (mut r, mut g, mut b) = (
                lerp_u8(DESK_TOP.0, DESK_BOTTOM.0, t),
                lerp_u8(DESK_TOP.1, DESK_BOTTOM.1, t),
                lerp_u8(DESK_TOP.2, DESK_BOTTOM.2, t),
            );
            if PAGE_RECT.contains(p) {
                let (pr, pg, pb) = if y < PAGE_RECT.y as u32 + 36 {
                    PAGE_TITLE
                } else if (y as f32 - PAGE_RECT.y - 60.0) % 44.0 < 10.0
                    && x > PAGE_RECT.x as u32 + 40
                    && (x as f32) < PAGE_RECT.right() - 40.0
                {
                    PAGE_BAR
                } else {
                    PAGE
                };
                r = pr;
                g = pg;
                b = pb;
            }
            let i = ((y * w + x) * 4) as usize;
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = 255;
        }
    }
    buf
}

fn opaque(w: u32, h: u32, c: (u8, u8, u8)) -> Vec<u8> {
    let mut buf = vec![0_u8; (w * h * 4) as usize];
    for px in buf.chunks_exact_mut(4) {
        px[0] = c.0;
        px[1] = c.1;
        px[2] = c.2;
        px[3] = 255;
    }
    buf
}

/// 把**预乘** RGBA 的标注层以 source-over 叠到不透明的底图上。
///
/// 与 `UpdateLayeredWindow` 在系统侧做的事同义：分层窗口交给系统的就是预乘 RGBA，
/// 系统按同一个公式合到屏幕上。这里自己算一遍，存出来的 PNG 才和用户实际看到的画面一致。
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
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

// ─────────────────────────── 像素工具 ───────────────────────────

fn px(buf: &[u8], w: u32, x: u32, y: u32) -> (u8, u8, u8) {
    let i = ((y * w + x) * 4) as usize;
    (buf[i], buf[i + 1], buf[i + 2])
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// 工具栏的底色 / 边框色，**不含**纯白。
///
/// 白板底色恰为 `(255,255,255)`，而工具栏底色 `(246,248,250)`、边框 `(226,230,234)`——
/// 用"亮度落在 225..252 且不是纯白"就能把两者干净分开（`ui::tokens` 的那几个值都在这段里）。
fn is_bar_paint(p: (u8, u8, u8)) -> bool {
    let lum = (p.0 as u32 + p.1 as u32 + p.2 as u32) / 3;
    // ★ 上界 255 而不是 252：工具栏底板 `BAR_BG` 是 `0xF7FFFFFF` ⇒ 合成到白底后
    //   实际是 **(254,254,254) / lum=254**，比原上界高 2，被判据当成"不是底板"⇒
    //   「工具栏几何」在 2026-10-03 之前就一直红着。**判据比被测物更严**就是假红。
    //   下界 225 是要排掉合成桌面（66,78,96 ⇒ lum 80）与提示浮字深底（2B2F35 ⇒ lum 47）。
    (225..=255).contains(&lum)
}

fn is_black(p: (u8, u8, u8)) -> bool {
    p.0 <= 45 && p.1 <= 45 && p.2 <= 45
}

fn is_green(p: (u8, u8, u8)) -> bool {
    p.1 as i32 - p.0 as i32 >= 60 && p.1 as i32 - p.2 as i32 >= 60
}

fn is_red(p: (u8, u8, u8)) -> bool {
    p.0 as i32 - p.1 as i32 >= 40 && p.0 as i32 - p.2 as i32 >= 25
}

/// 在 `region` 内选出满足 `pred` 的像素的外接矩形。
/// 找工具栏上一条"只有底板色"的竖缝，返回它的 x。
///
/// 取第一组与第二组之间的组间缝隙（`layout` 把组边界画成分隔线，但线只有 1px，
/// 落在**线旁边**那一列才是纯底板）。找不到就退回"第一颗按钮的左边缘 + 1px"。
fn toolbar_gap_x(bar: Rect) -> f32 {
    // `GAP` / `GROUP_GAP` 与 `toolbar` 同源，改动会同步（这里只是量位置，不重算布局）。
    const BTN: f32 = 34.0;
    const GAP: f32 = 4.0;
    const GROUP_GAP: f32 = 14.0;
    const PAD: f32 = 8.0;
    // 第一组 3 颗：pad + 3*btn + 2*gap；再走半个组间缝隙。
    bar.x + PAD + 3.0 * BTN + 2.0 * GAP + GROUP_GAP / 2.0
}

/// 返回 (墨的 y 重心, 墨的纵向跨度 / 格子高)。
///
/// 用途：验**形状**而不只是"有没有墨"。重心与跨度对"图形整体挪位置 / 压扁"很敏感，
/// 而单纯的墨像素计数对这两种改动完全无感（第一版判据的假绿就是坐标被挪到格子边缘）。
fn ink_centroid_y(frame: &[u8], r: Rect) -> (f32, f32) {
    let (x0, x1) = ((r.x).max(0.0) as u32, (r.right()).min(W as f32) as u32);
    let (y0, y1) = ((r.y).max(0.0) as u32, (r.bottom()).min(H as f32) as u32);
    // 底板色：四角最亮那个（与 `ink_pixels_in` 同一口径）。
    let inset = r.w / 2.0 - 2.0;
    let corners = [
        Point::new(r.x + 2.0, r.y + 2.0),
        Point::new(r.x + inset, r.y + 2.0),
        Point::new(r.x + 2.0, r.y + inset),
        Point::new(r.x + inset, r.y + inset),
    ];
    let px = |p: Point| -> [u8; 3] {
        let i = (((p.y as u32).min(H - 1) * W + (p.x as u32).min(W - 1)) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2]]
    };
    let bg = corners
        .iter()
        .map(|p| px(*p))
        .max_by_key(|c| c[0] as u32 + c[1] as u32 + c[2] as u32)
        .unwrap_or([255, 255, 255]);

    let mut sum = 0f32;
    let mut wsum = 0f32;
    let mut lo = f32::MAX;
    let mut hi = f32::MIN;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * W + x) * 4) as usize;
            let d = (frame[i] as i32 - bg[0] as i32).abs()
                + (frame[i + 1] as i32 - bg[1] as i32).abs()
                + (frame[i + 2] as i32 - bg[2] as i32).abs();
            if d > 90 {
                let fy = y as f32;
                sum += fy;
                wsum += 1.0;
                lo = lo.min(fy);
                hi = hi.max(fy);
            }
        }
    }
    if wsum == 0.0 {
        return (r.y + r.h / 2.0, 0.0);
    }
    ((sum / wsum), (hi - lo + 1.0) / r.h)
}

/// 从 `(x, y_start)` 往下数连续多少个像素仍是工具栏底板色。
///
/// ★ 只往下数、不往上找段：白板场景里工具栏**上方**紧挨着浅色的页面内容，两者在探针列
///   上是连着的（实测框成 h=121 而真值 52）。往下数则只覆盖工具栏自身，
///   "画高了 / 画低了 / 没画"都会让连续长度不足 40px 而变红。
fn bar_run_down_from(frame: &[u8], x_col: f32, y_start: f32, max_len: u32) -> u32 {
    let x = (x_col.max(0.0) as u32).min(W - 1);
    let y0 = (y_start.max(0.0) as u32).min(H);
    let end = (y0 + max_len).min(H);
    let mut n = 0u32;
    for y in y0..end {
        let i = ((y * W + x) * 4) as usize;
        if !is_bar_paint((frame[i], frame[i + 1], frame[i + 2])) {
            break;
        }
        n += 1;
    }
    n
}

fn count_in(buf: &[u8], region: Rect, pred: impl Fn((u8, u8, u8)) -> bool) -> u32 {
    let (rx, ry, rw, rh) = (
        region.x.max(0.0) as u32,
        region.y.max(0.0) as u32,
        region.w as u32,
        region.h as u32,
    );
    let mut n = 0;
    for y in ry..(ry + rh).min(H) {
        for x in rx..(rx + rw).min(W) {
            if pred(px(buf, W, x, y)) {
                n += 1;
            }
        }
    }
    n
}

/// 沿 x 逐列看"这一列里有没有目标像素"，再数连续段的个数。
///
/// 这是"虚线到底有没有断"的判据：实线横向铺过去只有 1 段，虚线会有 N 段。按列而不是按行，
/// 是因为波形在 y 方向上是起伏的——按行扫会漏掉谷底那些列。
fn runs_along_x(buf: &[u8], region: Rect, pred: impl Fn((u8, u8, u8)) -> bool + Copy) -> u32 {
    let (rx, rw) = (region.x.max(0.0) as u32, region.w as u32);
    let (ry, rh) = (region.y.max(0.0) as u32, region.h as u32);
    let mut runs = 0;
    let mut in_run = false;
    for x in rx..(rx + rw).min(W) {
        let hit = (ry..(ry + rh).min(H)).any(|y| pred(px(buf, W, x, y)));
        if hit && !in_run {
            runs += 1;
        }
        in_run = hit;
    }
    runs
}

// ─────────────────────────── 判据 ───────────────────────────

fn print_drawlist_stats(list: &DrawList) {
    let (mut fills, mut strokes, mut texts, mut images, mut other) = (0, 0, 0, 0, 0);
    for cmd in list.cmds() {
        match cmd {
            DrawCmd::Fill { .. } => fills += 1,
            DrawCmd::Stroke { .. } => strokes += 1,
            DrawCmd::Text { .. } => texts += 1,
            DrawCmd::Image { .. } => images += 1,
            _ => other += 1,
        }
    }
    println!(
        "DrawList: {} 条指令（填充 {fills} / 描边 {strokes} / 文字 {texts} / 位图 {images} / 其他 {other}），顶点池 {}",
        list.cmds().len(),
        list.points_len()
    );
}

/// 工具栏外框必须落在 `toolbar::layout` 算出来的位置上。
///
/// 只扫屏幕底带（`BAR_BAND`）：屏幕正中的提示浮字是纯白，混进来会把外框拉成整屏。
fn verify_geometry(report: &mut Report, frame: &[u8]) {
    let want = toolbar::layout(
        ScreenRect {
            x: 0,
            y: 0,
            w: W as i32,
            h: H as i32,
        },
        1.0,
        // 第三参 = 截图编辑态的选区锚点（`Some` 时工具条跟随选框）。本探针验的是**普通
        // 标注态**（工具条钉在屏幕底边），所以传 `None` —— 与 `draw` / `hit` 必须同值。
        None,
        // 第四参 = 是不是截图编辑态（编辑态多一颗打勾「完成」，见 `ToolbarAction::Done`）。
        // 本探针画的是普通标注态 ⇒ `false`。
        false,
    )
    .bar
    .inset(-1.0, -1.0); // 底板比外框大 1px（先铺边框色再铺内层，见 toolbar::draw）
                        // ★ 在白板帧上验：屏幕标注帧的底图是**真实桌面的冻结帧**，底带里全是真实像素
                        //   （浏览器/任务栏多为近白），"近白 = 工具栏"在那种底上会把整条底带判成工具栏。
                        //   白板底色是纯白，用 `is_bar_paint`（排除纯白）就能唯一地框出工具栏。
                        // ★★★ 这条判据 2026-10-03 之前**一直红着**，是既有假红，不是回归。两个独立成因：
                        //   ① `is_bar_paint` 上界 252 而底板合成后是 lum=254 ⇒ 底板压根没被认出来；
                        //   ② 即便认出来，"在整条底带上框所有浅色像素"这种整体包围盒做法（原 `bbox_in`，
                        //      因本条判据改用逐列下行而成为死代码，2026-10-10 删除），会把白板场景里
                        //      **页面上方的浅色内容**一起框进去（实测框成 y 959 / h 121，真值 999 / 52）。
                        //
                        //   修法：**只验工具栏自己的上边界**——从预测位置 `want.y` 往下数连续底板色像素，
                        //   要能连续盖住整个按钮区（≥ 40px）。上方内容连着不要紧（往下数遇不到它），
                        //   而"工具栏画高了 / 画低了"这类真回归会让这段连续长度立刻不足。
    let probe_x = toolbar_gap_x(want);
    let run = bar_run_down_from(frame, probe_x, want.y, want.h as u32 + 8);
    let got = Rect {
        x: want.x,
        y: want.y,
        w: want.w,
        h: run as f32,
    };
    let covered = run >= 40;
    // `want` 是 `layout` 直接算出来的（底板再外扩 1px），所以宽度/水平位置天然一致，
    // 真正要验的是"预测的那个位置**真的有工具栏底板**"。
    report.check(
        covered,
        "工具栏几何",
        format!(
            "预测 ({:.0},{:.0} {}×{})；探针列 x={:.0} 从 y={:.0} 往下连续底板 {run}px（需 ≥40，按钮区高 {:.0}px）",
            want.x, want.y, want.w, want.h, probe_x, want.y, want.h
        ),
    );
    let _ = got;

    // 当前工具按钮的选中底色（ACTIVE_BG 偏蓝：b 明显大于 r）。
    let active = count_in(frame, BAR_BAND, |p| {
        p.2 as i32 - p.0 as i32 >= 12 && p.2 >= 240 && p.0 >= 225
    });
    report.check(
        active > 300,
        "当前工具高亮",
        format!("选中底色约 {active} px"),
    );
}

/// 三种笔的颜色都要真的落在屏幕上。
fn verify_ink(report: &mut Report, frame: &[u8], whiteboard: bool) {
    let black = count_in(frame, BLACK_WAVE, is_black);
    let green = count_in(frame, GREEN_WAVE, is_green);
    let red = count_in(frame, RED_BAR, is_red);
    let tag = if whiteboard { "白板" } else { "屏幕" };
    report.check(
        black > 500,
        &format!("{tag}·黑笔着墨"),
        format!("{} px", black),
    );
    report.check(
        green > 500,
        &format!("{tag}·绿笔着墨"),
        format!("{} px", green),
    );
    report.check(
        red > 2000,
        &format!("{tag}·红荧光笔着墨"),
        format!("{} px（30px 笔宽 × 640px 长，25% 不透明度）", red),
    );
}

/// **虚线必须是断的**：同形状的一条实线（黑）与一条虚线（绿）做段数对照。
///
/// 这条锁的是"面板里选了线型但画出来还是实线"那类回归——笔迹确实出现了，
/// 所以只验"有没有墨"是发现不了的，必须验"墨是不是断成了多段"。
fn verify_dashes(report: &mut Report, frame: &[u8]) {
    let solid_runs = runs_along_x(frame, BLACK_WAVE, is_black);
    let dash_runs = runs_along_x(frame, GREEN_WAVE, is_green);
    // 下界必须写死成 1：`<= 3` 在"一根线都没画出来"（0 段）时也会通过。
    report.check(
        (1..=3).contains(&solid_runs),
        "实线连续",
        format!("黑实线扫出 {solid_runs} 段（应为 1；波形剧烈处允许少量断裂）"),
    );
    report.check(
        dash_runs >= 15,
        "虚线断开",
        format!("绿虚线扫出 {dash_runs} 段（4px 笔宽 = 16px 划 / 8px 空，600px 上应≈25 段）"),
    );
}

/// 矩形与椭圆必须是**描边**：边上要有墨、内部要是空的。
///
/// 两个方向都要验。只验"内部是空的"在"图元压根没画出来"时同样通过——那就成了摆设。
/// 这是从探针图上看出来的问题：翻译层对 `Shape::Rect` 走 `fill_round_rect`、对
/// `Shape::Ellipse` 走 `fill_ellipse`，两者都会把整个图元填实。表现是"点矩形工具拖出来
/// 一个实心色块"，与工具栏上的空心方框图标自相矛盾。
///
/// ★ **「内部是空的」只在白板帧上验**（`whiteboard` 参数）。屏幕帧的底图是**真实桌面的
/// 冻结帧**（`DrawingController::toggle` → `Backdrop::capture` → `screenshot::grab`，产品
/// 行为，不是探针的选择），而这条判据用 `is_black`（三通道 ≤45）——真实桌面上任何深色
/// 像素（深色主题的编辑器、终端、视频）都会被算成墨。实测：前台开着深色窗口时报
/// 16% 着墨，而探针自己的合成桌面**不可能**产生这种像素（配色下界 DESK_TOP(66,78,96)，
/// 全部 >45），所以那是环境噪声不是回归。白板帧底色纯白，判据唯一确定（实测 0/10944）。
/// 边框着墨两种帧都验：它在深色底图上只会偏高（更宽松），而真正要拦的"图元压根没画出来"
/// 是 0，照样拦得住。
fn verify_shapes_are_outlines(report: &mut Report, frame: &[u8], whiteboard: bool) {
    for (label, outer) in [
        (
            "矩形是空心描边",
            Rect {
                x: 1250.0,
                y: 140.0,
                w: 200.0,
                h: 120.0,
            },
        ),
        (
            "椭圆是空心描边",
            Rect {
                x: 1250.0,
                y: 320.0,
                w: 200.0,
                h: 120.0,
            },
        ),
    ] {
        let inner = outer.inset(24.0, 24.0);
        let edge = count_in(frame, outer, is_black) - count_in(frame, inner, is_black);
        let ink = count_in(frame, inner, is_black);
        let area = (inner.w * inner.h) as u32;
        let ratio = ink as f32 / area as f32;
        report.check(
            edge > 1000,
            &format!("{label}·边框着墨"),
            format!("边框带上 {edge} px 墨（4px 笔宽 × 周长 640px，应≈2500）"),
        );
        if whiteboard {
            report.check(
                ratio < 0.05,
                label,
                format!(
                    "内部 {ink}/{area} px 着墨（{:.0}%）；填实的话应接近 100%",
                    ratio * 100.0
                ),
            );
        }
    }
}

/// 退出标注后工具栏必须消失。
fn verify_toolbar_leaves_after_exit(report: &mut Report) {
    let screen = ScreenRect {
        x: 0,
        y: 0,
        w: W as i32,
        h: H as i32,
    };
    let mut c = DrawingController::new(DrawingSettings::default());
    c.set_screen(screen);
    c.toggle(DrawMode::Screen, 0.0);
    c.stop();
    let mut list = DrawList::new();
    c.draw(&mut list);
    // 退出后应只剩「已退出绘图」那一帧提示：9 条文字指令 + **1 条**它的深色底框
    // （`Notify::draw` 的 `fill_round_rect`）。工具栏那几十条指令必须一条不剩。
    //
    // ★ 期望值从 0 改成"恰好 1 且必须是圆角矩形"：底框是 2026-09 为"白字在亮底上看不见"
    //   补上的，这条判据当时没跟着更新，于是长期误报。写死 1 仍然拦得住"工具栏没清干净"
    //   （那会是 40+ 条），同时用形状把"这 1 条是提示底框"钉住。
    let leftovers: Vec<&DrawCmd> = list
        .cmds()
        .iter()
        .filter(|cmd| matches!(cmd, DrawCmd::Stroke { .. } | DrawCmd::Fill { .. }))
        .collect();
    let only_the_toast_box = leftovers.len() == 1
        && matches!(
            leftovers[0],
            DrawCmd::Fill {
                shape: curseen_ui::core::draw::Shape::Rect { radius, .. },
                ..
            } if *radius > 0.0
        );
    report.check(
        only_the_toast_box,
        "退出后工具栏消失",
        format!(
            "退出后剩 {} 条填充/描边指令（应为 1 条提示浮字底框；工具栏会产出 40+ 条）",
            leftovers.len()
        ),
    );
}

/// 截图编辑态工具栏的**逐按钮像素**验收（★ 2026-10-03 任务 2）。
///
/// 为什么单测不够：单测只验"这颗按钮产出了指令"，而图标几何写错时**指令数照样正常**
/// —— 坐标算反、画到格外、折线方向反了，单测一律绿。这里把整条工具栏渲进 RGBA 缓冲，
/// 逐按钮检查"图标那块地方有没有真的着墨"。
///
/// 三项判据：
/// ① 打勾「完成」**有像素**（它是本次新增的那颗，没画出来就是空按钮）；
/// ② 打勾**不在**标注态的按钮表里（可见性判据接到了渲染层，而不只是数据层）；
/// ③ 编辑态**每一颗**按钮都有像素（顺带把新布局整体验一遍：某格坐标算错会全露出来）。
fn verify_shot_edit_toolbar_pixels(report: &mut Report, screen: ScreenRect, out_dir: &Path) {
    // 选区放在屏幕中间偏上，工具条会落在它下方（`layout` 的锚点规则）。
    let sel = Rect {
        x: 400.0,
        y: 160.0,
        w: 1000.0,
        h: 500.0,
    };

    // ★★ 每一档都要用**它自己那个锚点**算布局：编辑态有选区（工具条跟随选区，落在
    //   选区下方 y≈685），标注态没有（工具条钉屏幕底边 y≈999）。拿同一个锚点去算两档，
    //   标注态那份就会取样到空气里 —— 而判据报出来是"16 颗按钮全无像素"，
    //   看着像渲染坏了，其实是**量错了地方**。
    //   这正是"落点必须与 draw 同一取值"那条铁律的像素版。
    let mut shots: Vec<(&str, Option<Rect>, Vec<(ToolbarAction, Rect)>)> = Vec::new();
    for (label, shot_edit, anchor) in [("编辑态", true, Some(sel)), ("标注态", false, None)] {
        let cells = toolbar::layout(screen, 1.0, anchor, shot_edit).cells;
        shots.push((label, anchor, cells));
    }

    for (idx, (label, anchor, cells)) in shots.iter().enumerate() {
        let mut c = DrawingController::new(DrawingSettings::default());
        c.set_screen(screen);
        // 只画工具栏：编辑态与标注态都靠 `mode` 决定按钮表，而 `ShotEdit` 需要一张
        // 冻结帧当底图 —— 探针里给 `Backdrop::None` 即可（这一段只验工具栏的像素）。
        if anchor.is_some() {
            c.start_with_backdrop(curseen_ui::core::backdrop::Backdrop::None, sel);
        } else {
            c.toggle(DrawMode::Screen, 0.0);
        }
        let mut list = DrawList::new();
        c.draw(&mut list);

        let mut ann = vec![0_u8; (W * H * 4) as usize];
        let mut engine = DWriteEngine::new();
        render_once(&list, &mut ann, &mut engine);
        // 合成到白底上：工具栏本身是浅色，透明缓冲上分不清"没画"与"画成透明"。
        let mut frame = opaque(W, H, (255, 255, 255));
        composite_premultiplied(&mut frame, &ann);

        let path = out_dir.join(format!("toolbar_{label}.png"));
        if let Err(e) = write_png(&path, W, H, &frame) {
            eprintln!("写 PNG 失败 {}: {e}", path.display());
            std::process::exit(2);
        }

        // 逐按钮数"图标区里着墨的像素"：与该格**中心 1/3 框**比较，统计明显不同于
        // 底板色的像素数。图标是深灰（ICON_FG）压在浅色底板上，差值很大。
        let mut blank: Vec<&str> = Vec::new();
        for (a, r) in cells.iter() {
            let ink = ink_pixels_in(&frame, *r);
            if ink < 12 {
                blank.push(match a {
                    ToolbarAction::Pen(i) => match i {
                        0 => "画笔1",
                        1 => "画笔2",
                        _ => "画笔3",
                    },
                    ToolbarAction::Line => "直线",
                    ToolbarAction::Arrow => "箭头",
                    ToolbarAction::Rect => "矩形",
                    ToolbarAction::Ellipse => "椭圆",
                    ToolbarAction::Text => "文字",
                    ToolbarAction::Stamp => "序号",
                    ToolbarAction::Mosaic => "马赛克",
                    ToolbarAction::Eraser => "橡皮擦",
                    ToolbarAction::Undo => "撤销",
                    ToolbarAction::Redo => "重做",
                    ToolbarAction::Clear => "清空",
                    ToolbarAction::Save => "保存",
                    ToolbarAction::Done => "完成",
                    ToolbarAction::Exit => "退出",
                });
            }
        }
        report.check(
            blank.is_empty(),
            &format!("{label}工具栏每颗按钮都有像素"),
            format!(
                "{label}共 {} 颗按钮，无像素的：{:?}（图 {}）",
                cells.len(),
                blank,
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
        );

        // 可见性：打勾只在编辑态。
        let has_done = cells.iter().any(|(a, _)| *a == ToolbarAction::Done);
        report.check(
            has_done == (*label == "编辑态"),
            &format!("{label}的打勾「完成」可见性"),
            format!("{label}含打勾 = {has_done}（应 = {}）", *label == "编辑态"),
        );

        // ★★ 打勾的**形状**：光"数得出像素"不够 —— 把坐标写反、画到格子底边，
        //   像素数照样正常（第一版判据就这么假绿过一次：故意把两段折线都挪到
        //   `c.y + h*0.98`，23 项全过）。所以再加一条"重心必须在格子中上部"：
        //   打勾的墨分布是**左下短划 + 右上长挑**，重心偏左上；真画错了位置，
        //   重心会跟着跑掉。
        if let Some((_, dr)) = cells.iter().find(|(a, _)| *a == ToolbarAction::Done) {
            let (cy, spread) = ink_centroid_y(&frame, *dr);
            let rel = (cy - dr.y) / dr.h;
            report.check(
                (0.15..0.75).contains(&rel) && spread >= 0.35,
                &format!("{label}打勾的形状"),
                format!(
                    "墨重心在格子高度 {}%（应 15%~75%）、纵向跨度占比 {}%（应 ≥35%）——                     坐标写反时重心会贴到边缘、跨度也会塌掉；括号里是乘 100 的值",
                    (rel * 100.0).round(),
                    (spread * 100.0).round()
                ),
            );
        }

        if idx == 0 {
            println!("已写出 {}", path.display());
        }
    }
}

/// 数一个按钮格里"明显着墨"的像素数（相对该格的**底板色**）。
///
/// ★ 底板色必须取**四角**而不是中心：图标（线、折线、点）几乎都横穿格子中心，
///   取中心当背景等于拿墨色当底色，差值全归零 ⇒ 明明画了却数出 0 个像素。
///   这个坑真实发生过：第一版判据把整条工具栏报成"全无像素"，而图上图标清清楚楚。
///
/// 取中心 1/3 而不是整格：格子边缘有圆角底色与 hover 底色，边缘像素会误算成图标。
fn ink_pixels_in(frame: &[u8], r: Rect) -> usize {
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    // ★ 取**整格**而不是中心 1/3：空心形状（矩形 / 椭圆）的墨只在**外缘**，
    //   中心 1/3 恰好是空的 —— 判据会把画得完全正确的矩形报成"无像素"。
    //   圆角底色不会误算：底板色是浅色、图标是深灰，差值阈值 90 远大于两者之差。
    let (x0, x1) = ((r.x).max(0.0) as u32, (r.right()).min(W as f32) as u32);
    let (y0, y1) = ((r.y).max(0.0) as u32, (r.bottom()).min(H as f32) as u32);
    let _ = (cx, cy);

    // 底板色 = 四角（内缩 2px，避开圆角）里**最亮**的那个：图标是深色，底板是浅色，
    // 取最亮的一角就一定落在底板上。
    let inset = r.w / 2.0 - 2.0;
    let corners = [
        Point::new(r.x + 2.0, r.y + 2.0),
        Point::new(r.x + inset, r.y + 2.0),
        Point::new(r.x + 2.0, r.y + inset),
        Point::new(r.x + inset, r.y + inset),
    ];
    let px = |p: Point| -> [u8; 3] {
        let i = (((p.y as u32).min(H - 1) * W + (p.x as u32).min(W - 1)) * 4) as usize;
        [frame[i], frame[i + 1], frame[i + 2]]
    };
    let bg = corners
        .iter()
        .map(|p| px(*p))
        .max_by_key(|c| c[0] as u32 + c[1] as u32 + c[2] as u32)
        .unwrap_or([255, 255, 255]);

    let mut n = 0usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * W + x) * 4) as usize;
            let d = (frame[i] as i32 - bg[0] as i32).abs()
                + (frame[i + 1] as i32 - bg[1] as i32).abs()
                + (frame[i + 2] as i32 - bg[2] as i32).abs();
            if d > 90 {
                n += 1;
            }
        }
    }
    n
}

fn report_perf(list: &DrawList, engine: &mut DWriteEngine, ann: &mut [u8]) {
    // 预热一轮（字体度量缓存、tiny-skia 内部状态）。
    render_once(list, ann, engine);

    let mut total = Duration::ZERO;
    for _ in 0..TIMING_ROUNDS {
        let mut target = DibTarget::new(ann, W, H);
        let mut canvas = target.make_canvas(engine, 1.0);
        let t0 = Instant::now();
        translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
        total += t0.elapsed();
    }
    let per = total.as_secs_f64() * 1000.0 / TIMING_ROUNDS as f64;
    println!(
        "渲染耗时：整屏 {W}×{H} 一帧 {per:.2} ms（{} 条指令，{TIMING_ROUNDS} 轮均值）",
        list.cmds().len()
    );
    println!(
        "  换算：即便按 60fps 连续重绘也只占 {:.1}% 的单核",
        per / 16.67 * 100.0
    );
}
