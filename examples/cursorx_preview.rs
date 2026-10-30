//! CursorX 扩展包的**离屏预览**：24 个脚本各驱动 90 帧，产出真实 `DrawList`，
//! 经生产翻译层 `translate_offset` 渲染后贴到合成桌面上，存成 PNG 联系表。
//!
//! 用途：不启动覆盖层、不注入输入，就能看每个脚本的构图 / 对比度 / 指针可读性。
//! `Difference Blend` / `Fluid Glass` 依赖真实桌面采样，在探针里会走静态回退分支
//! （覆盖层窗口不存在），所以这两张图看的是回退外观，不是真镜片。
//!
//! 跑法：`cargo run --release --example cursorx_preview`
//! 产物：`../temp/cursorx_preview/`（每个效果一张 + `contact.png` 联系表）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use curseen_ui::core::color::Argb;
use curseen_ui::core::draw::DrawList;
use curseen_ui::core::geometry::Rect;
use curseen_ui::runtime::render::translate_offset;
use curseen_ui::scripting::api_draw;
use curseen_ui::scripting::ScriptingEngine;

use windui::render::{DibTarget, RenderTarget};
use windui::text::DWriteEngine;

const TILE_W: u32 = 480;
const TILE_H: u32 = 270;
const COLS: u32 = 6;
const ROWS: u32 = 4;

const EFFECTS: [(&str, &str); 24] = [
    ("magnetic", "Magnetic"),
    ("particle_trail", "Particle Trail"),
    ("spotlight", "Spotlight"),
    ("morphing_blob", "Morphing Blob"),
    ("pixel_shatter", "Pixel Shatter"),
    ("elastic_ring", "Elastic Ring"),
    ("neon_glow", "Neon Glow"),
    ("text_orbiter", "Text Orbiter"),
    ("gravity_pull", "Gravity Pull"),
    ("constellation", "Constellation"),
    ("fire_trail", "Fire Trail"),
    ("crosshair_scope", "Crosshair Scope"),
    ("mirror_ghost", "Mirror Ghost"),
    ("rainbow_comet", "Rainbow Comet"),
    ("bubble_float", "Bubble Float"),
    ("ripple_wave", "Ripple Wave"),
    ("glitch_shift", "Glitch Shift"),
    ("wind_stream", "Wind Stream"),
    ("dna_helix", "DNA Helix"),
    ("torch_light", "Torch Light"),
    ("difference_blend", "Difference Blend"),
    ("ghost_trail", "Ghost Trail"),
    ("audio_pulse", "Audio Pulse"),
    ("fluid_glass", "Fluid Glass"),
];

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("temp")
        .join("cursorx_preview");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("建输出目录失败 {}: {e}", out_dir.display());
        std::process::exit(2);
    }

    let mut engine = DWriteEngine::new();
    let mut sheet = vec![0_u8; (COLS * TILE_W * ROWS * TILE_H * 4) as usize];
    fill_rect_rgba(
        &mut sheet,
        COLS * TILE_W,
        0,
        0,
        COLS * TILE_W,
        ROWS * TILE_H,
        (18, 20, 26),
    );

    for (i, (stem, label)) in EFFECTS.iter().enumerate() {
        let list = match run_effect(&root, stem) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[skip] {stem}: {e}");
                continue;
            }
        };
        let mut tile = desktop_tile();
        let mut layer = vec![0_u8; (TILE_W * TILE_H * 4) as usize];
        render(&list, &mut layer, &mut engine);
        composite_premultiplied(&mut tile, &layer);
        stamp_label(&mut tile, &mut engine, label);
        let p = out_dir.join(format!("{stem}.png"));
        if let Err(e) = write_png(&p, TILE_W, TILE_H, &tile) {
            eprintln!("写 {} 失败：{e}", p.display());
        }
        let col = i as u32 % COLS;
        let row = i as u32 / COLS;
        blit(
            &mut sheet,
            COLS * TILE_W,
            col * TILE_W,
            row * TILE_H,
            &tile,
            TILE_W,
            TILE_H,
        );
        println!("[ok] {label} ({} 条指令)", list.len_all());
    }

    let p = out_dir.join("contact.png");
    if let Err(e) = write_png(&p, COLS * TILE_W, ROWS * TILE_H, &sheet) {
        eprintln!("写 {} 失败：{e}", p.display());
        std::process::exit(1);
    }
    println!("联系表：{}", p.display());
}

fn run_effect(root: &Path, stem: &str) -> Result<DrawList, String> {
    let eng = ScriptingEngine::new(root).map_err(|e| format!("引擎初始化失败：{e}"))?;
    eng.load_script(&format!("scripts/CursorX/{stem}"))
        .map_err(|e| format!("加载失败：{e}"))?;
    eng.set_wall_ms(50_000.0).map_err(|e| e.to_string())?;
    eng.call_init().map_err(|e| format!("init 失败：{e}"))?;

    let params = HashMap::new();
    let cx = TILE_W as f32 / 2.0;
    let cy = TILE_H as f32 / 2.0;
    for i in 0..90 {
        let t = i as f32 / 60.0;
        let x = cx + (t * 2.2).sin() * 120.0 + (t * 0.9).cos() * 40.0;
        let y = cy + (t * 1.7).cos() * 70.0 + (t * 3.1).sin() * 25.0;
        let (vx, vy) = ((x - cx) * 0.02, (y - cy) * -0.02);
        eng.update_env((x, y), (cx, cy), t, (TILE_W as f32, TILE_H as f32), "panel")
            .map_err(|e| e.to_string())?;
        eng.set_pointer_state(
            vx,
            vy,
            (vx * vx + vy * vy).sqrt(),
            vy.atan2(vx),
            false,
            "left",
        )
        .map_err(|e| e.to_string())?;
        eng.call_on_move(x, y, t, &params)
            .map_err(|e| format!("on_move 失败：{e}"))?;
        eng.call_step(1.0 / 120.0, t, &params)
            .map_err(|e| format!("step 失败：{e}"))?;
        if i == 55 {
            eng.call_on_button("left", x, y, t, &params)
                .map_err(|e| format!("on_mouse_down 失败：{e}"))?;
        }
        if i == 58 {
            eng.call_on_mouse_up("left", x, y, &params)
                .map_err(|e| format!("on_mouse_up 失败：{e}"))?;
        }
    }

    let mut list = DrawList::new();
    api_draw::set_draw_target(&mut list);
    let r = eng.call_draw(&params);
    api_draw::clear_draw_target();
    r.map_err(|e| format!("draw 失败：{e}"))?;
    Ok(list)
}

fn render(list: &DrawList, layer: &mut [u8], engine: &mut DWriteEngine) {
    let mut target = DibTarget::new(layer, TILE_W, TILE_H);
    let mut canvas = target.make_canvas(engine, 1.0);
    translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
}

/// 合成桌面：上暗下浅 + 一块白卡片 + 一块深色卡片，方便同时检查深/浅背景对比度。
fn desktop_tile() -> Vec<u8> {
    let mut buf = vec![0_u8; (TILE_W * TILE_H * 4) as usize];
    for y in 0..TILE_H {
        for x in 0..TILE_W {
            let t = y as f32 / TILE_H as f32;
            let (r, g, b) = if x < TILE_W / 2 {
                (
                    (44.0 + 24.0 * t) as u8,
                    (54.0 + 28.0 * t) as u8,
                    (70.0 + 30.0 * t) as u8,
                )
            } else {
                (
                    (196.0 + 30.0 * t) as u8,
                    (204.0 + 26.0 * t) as u8,
                    (214.0 + 22.0 * t) as u8,
                )
            };
            let i = ((y * TILE_W + x) * 4) as usize;
            buf[i] = r;
            buf[i + 1] = g;
            buf[i + 2] = b;
            buf[i + 3] = 255;
        }
    }
    // 白卡片与深色卡片
    fill_rect_rgba(&mut buf, TILE_W, 24, 24, 180, 90, (248, 249, 252));
    fill_rect_rgba(&mut buf, TILE_W, 24, 150, 180, 70, (32, 36, 44));
    buf
}

fn stamp_label(tile: &mut [u8], engine: &mut DWriteEngine, text: &str) {
    let mut list = DrawList::new();
    let w = 12.0 + text.chars().count() as f32 * 8.0;
    list.fill_round_rect(
        Rect {
            x: 6.0,
            y: 6.0,
            w,
            h: 22.0,
        },
        4.0,
        Argb::new(170, 8, 10, 14),
    );
    list.text(12.0, 10.0, text, 13.0, Argb::new(255, 255, 255, 255), None);
    let mut layer = vec![0_u8; (TILE_W * TILE_H * 4) as usize];
    render(&list, &mut layer, engine);
    composite_premultiplied(tile, &layer);
}

fn fill_rect_rgba(buf: &mut [u8], stride_w: u32, x: u32, y: u32, w: u32, h: u32, c: (u8, u8, u8)) {
    for yy in y..(y + h).min(buf.len() as u32 / (stride_w * 4)) {
        for xx in x..(x + w).min(stride_w) {
            let i = ((yy * stride_w + xx) * 4) as usize;
            buf[i] = c.0;
            buf[i + 1] = c.1;
            buf[i + 2] = c.2;
            buf[i + 3] = 255;
        }
    }
}

fn blit(dst: &mut [u8], dst_w: u32, x: u32, y: u32, src: &[u8], sw: u32, sh: u32) {
    for row in 0..sh {
        let d = (((y + row) * dst_w + x) * 4) as usize;
        let s = (row * sw * 4) as usize;
        dst[d..d + (sw * 4) as usize].copy_from_slice(&src[s..s + (sw * 4) as usize]);
    }
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

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}
