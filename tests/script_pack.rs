//! 脚本包的**端到端**测试：跑的是 `CurSeen-Scripts/Packs/` 里真实交付的那几个文件。
//!
//! 图片体检与 OCR 已迁到同级的 `CurSeen-Scripts` 仓库（2026-10-29），这里直接跑仓库源，
//! **不再复制第二份**；仓库缺失就直接失败（不许静默跳过一条资产判据）。
//!
//! 为什么放在 `tests/` 而不是 `src/` 的单测里：这些脚本是**资产**，它们的判据也属于资产
//! （"交付给用户的东西能不能用"），不是某个 Rust 模块的内部不变量。放这里跑的是
//! `scripting::ext_runner::run_ext_sync` —— 与右键「脚本」/ 热键触发**同一条路**
//! （`examples/ext_proc_ocr.rs` 用的也是它），不是复刻一份加载逻辑。
//!
//! 判据纪律（沿用 m20 的档）：
//! - 每个脚本至少一格**硬读数**（具体 HEX / 具体比值 / 单调性），"有输出"不算测；
//! - 合成样本要能区分"算对了"与"看起来像那么回事"——例如取色板的渐变样本必须让占比
//!   单调且和为 100，两边都错时会滑走；
//! - 不发真剪贴板：写剪贴板的脚本一律传 `copy=false`。

use std::collections::HashMap;

use curseen_ui::core::image::RgbaBitmap;
use curseen_ui::scripting::api_ext::ExtCtx;
use curseen_ui::scripting::ext_runner;
use serde_json::{json, Value};

fn builtin_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/lua")
}

/// 同级 `CurSeen-Scripts` 仓库的 Packs 目录（迁移后的源）。
fn repo_tools() -> std::path::PathBuf {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../CurSeen-Scripts/Packs");
    assert!(
        p.is_dir(),
        "图片体检/OCR 已迁到 CurSeen-Scripts，测试需要同级检出：{p:?}"
    );
    p
}

/// 跑一个图片体检脚本（id = `1.image_report`），固定传 `copy=false`。
fn run(id: &str, params: &[(&str, Value)], img: RgbaBitmap) -> String {
    let mut p: HashMap<String, Value> = params
        .iter()
        .cloned()
        .map(|(k, v)| (k.to_string(), v))
        .collect();
    p.insert("copy".to_string(), json!(false));
    ext_runner::run_ext_sync(
        &repo_tools().join("ImageReport").join("scripts"),
        id,
        &p,
        Some(ExtCtx {
            image: img,
            hwnd: 0,
        }),
    )
    .unwrap_or_else(|e| panic!("{id} 跑失败：{e}"))
}

/// 按像素函数造图。
fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaBitmap {
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            rgba.extend_from_slice(&f(x, y));
        }
    }
    RgbaBitmap::new(w, h, rgba).expect("位图")
}

/// 取"主色板"那几行（`1. #FF0000 — 25.0%`）。
///
/// ★ 只认**带序号的行**：合并成「图片体检」之后，同一份结果里还有对比度的
/// 前景/背景 HEX——按"% 号"或"# 号"盲扫会把它们一起收进来，份额之和永远不等于 100。
/// 判据必须认**形状**，不能认"出现过百分号"。
fn palette_rows(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let Some(idx) = it.next() else { continue };
        let idx = idx.trim_end_matches('.');
        if idx.parse::<u32>().is_err() {
            continue;
        }
        let Some(hex) = it.next() else { continue };
        if hex.len() != 7 || !hex.starts_with('#') {
            continue;
        }
        let mut pct = it.next();
        if pct == Some("—") {
            pct = it.next();
        }
        let Some(pct) = pct else { continue };
        if let Ok(v) = pct.trim_end_matches('%').parse::<f64>() {
            out.push((hex.to_uppercase(), v));
        }
    }
    out
}

fn hexes(text: &str) -> Vec<String> {
    palette_rows(text).into_iter().map(|(h, _)| h).collect()
}

fn shares(text: &str) -> Vec<f64> {
    palette_rows(text).into_iter().map(|(_, v)| v).collect()
}

/// ★ 取色板 / 渐变：64 级灰阶切成 8 档，每档约 1/8，且**灰度值必须铺开**
/// （第一档明显暗、最后一档明显亮）。这条防的是"切了 8 个盒子但盒子里全是同一片像素"
/// 那种形状对、结果错的情况。
#[test]
fn palette_splits_a_ramp_into_even_bands() {
    let img = image(64, 4, |x, _| {
        let v = (x * 4) as u8; // 0,4,…,252
        [v, v, v, 255]
    });
    let out = run("1.image_report", &[("colors", json!("8"))], img);
    let sh = shares(&out);
    assert_eq!(sh.len(), 8, "64 级灰该切成 8 档：{out}");
    for (i, v) in sh.iter().enumerate() {
        assert!(
            (v - 12.5).abs() < 1.5,
            "第 {i} 档占比 {v}% 不该偏离 12.5% 这么多"
        );
    }
    let mut grays: Vec<u32> = hexes(&out)
        .iter()
        .filter_map(|h| u32::from_str_radix(&h[1..3], 16).ok())
        .collect();
    grays.sort_unstable();
    assert_eq!(grays.len(), 8, "{out}");
    assert!(grays[0] < 40, "最暗那档不够暗：{grays:?}");
    assert!(grays[7] > 215, "最亮那档不够亮：{grays:?}");
}

/// 从「图片体检」的结果里取清晰度那一格的分数（`- 清晰度：123.4（拉普拉斯方差）→ …`）。
fn score_of(text: &str) -> f64 {
    for line in text.lines() {
        if let Some(rest) = line.trim_start().strip_prefix("- 清晰度：") {
            let num: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = num.parse::<f64>() {
                return v;
            }
        }
    }
    panic!("没读到清晰度那一行：{text}");
}

/// ★ 清晰度：棋盘（边缘极多）的拉普拉斯方差必须**远大于**线性渐变（二阶差近似 0）。
/// 只测"有分数"等于没测——这条防的是"算了个常数"或"把所有图都判成清晰"。
#[test]
fn sharpness_separates_a_checkerboard_from_a_ramp() {
    let sharp = image(32, 32, |x, y| {
        if (x + y) % 2 == 0 {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        }
    });
    let smooth = image(32, 32, |x, _| {
        let v = (x * 8) as u8;
        [v, v, v, 255]
    });
    let a = run("1.image_report", &[], sharp);
    let b = run("1.image_report", &[], smooth);
    let (sa, sb) = (score_of(&a), score_of(&b));
    assert!(sa > 10000.0, "棋盘应远大于 10000，实际 {sa}：{a}");
    assert!(sb < 100.0, "线性渐变的二阶差应接近 0，实际 {sb}：{b}");
    assert!(sa > sb * 100.0, "两者差得不够开：{sa} vs {sb}");
    assert!(
        a.contains("清晰") && b.contains("糊"),
        "档位判词也要分开：\n{a}\n---\n{b}"
    );
}

/// ★ 体检报告：1920×1080 必须认出 16:9（差 0.00%），纯色图的唯一色数 = 1。
/// PNG 体积那组已按用户口径删除（体检只针对贴图，不需要试编码）——这里**反向钉住**它。
#[test]
fn image_health_names_the_ratio_and_the_palette_size() {
    let img = image(1920, 1080, |_, _| [10, 20, 30, 255]);
    let out = run("1.image_report", &[], img);
    assert!(out.contains("1920 × 1080"), "{out}");
    assert!(out.contains("16:9"), "该认出 16:9：{out}");
    assert!(out.contains("横向"), "{out}");
    // ★ 直方图是**采样**出来的（2M 像素逐点读会把脚本卡死）：断言的是"采样范围内的账"
    //   与步长，而不是总像素——总像素那一行由上面的尺寸行负责。
    assert!(
        out.contains("不透明：129600"),
        "采样 129600 个点全不透明：{out}"
    );
    assert!(out.contains("步长 4"), "步长该按面积开方算出来：{out}");
    assert!(out.contains("唯一色：1"), "纯色图：{out}");
    assert!(
        !out.contains("PNG 体积") && !out.contains("压缩到"),
        "PNG 体积那组已删除，不该再出现在报告里：{out}"
    );
}

/// 1000×750 是 4:3（250 的约分），别被 16:10 抢走。
#[test]
fn image_health_prefers_4_3_for_1000x750() {
    let img = image(1000, 750, |_, _| [0, 0, 0, 255]);
    let out = run("1.image_report", &[], img);
    // ★ 必须断言**"≈"那半句**：「4:3」本来就出现在最简比那一段里，只查 contains("4:3")
    //   时把"常见比例"写死成 16:9 也照样绿（我第一版就是这样，红证判死）。
    assert!(out.contains("≈ 4:3"), "常见比例该点名 4:3：{out}");
    assert!(
        !out.contains("≈ 16:10"),
        "4:3 与 16:10 差 6.7%，不该点名 16:10：{out}"
    );
    assert!(!out.contains("≈ 16:9"), "4:3 不该被说成 16:9：{out}");
}

/// ★ OCR 只交付**一个**脚本，名字/菜单统一叫「OCR」、不带说明文字；
/// 用户明确说不需要的「结果怎么收拾」/`transform` 后处理必须**彻底不在**脚本里。
#[test]
fn ocr_is_one_script_named_ocr_without_cleanup_modes() {
    let tools = repo_tools();
    assert!(
        !builtin_root().join("scripts/5.ocr_post.lua").exists(),
        "OCR 是一体的：后处理不该再是一个独立脚本"
    );
    let src = std::fs::read_to_string(tools.join("OCR/scripts/2.ocr_text.lua"))
        .expect("读 OCR/2.ocr_text.lua");
    assert!(
        src.contains("name = \"OCR 文字识别\""),
        "脚本名应改为「OCR 文字识别」"
    );
    assert!(
        src.contains("menu = \"OCR 文字识别\""),
        "菜单项应改为「OCR 文字识别」"
    );
    assert!(
        !src.contains("description =") && !src.contains("认出贴图"),
        "按用户口径删掉「认出贴图…」那行说明"
    );
    assert!(
        src.contains("识别后复制文本"),
        "复制参数应改叫「识别后复制文本」"
    );
    assert!(
        src.contains("label = \"快捷键\""),
        "快捷键参数的标签应统一成「快捷键」"
    );
    assert!(
        src.contains("./ocr_tool/CurSeenOcr.exe"),
        "外置引擎的 requires 不该随合并丢掉"
    );
    assert!(
        !src.contains("function transform("),
        "用户说不需要后处理：transform 应已删除"
    );
    assert!(
        !src.contains("结果怎么收拾"),
        "用户说不需要后处理：「结果怎么收拾」不该再出现在脚本里"
    );
    // "一个菜单项"按**仓库里的 Packs 脚本**数：只有 2.ocr_text 声明「OCR」。
    let mut owners = Vec::new();
    let mut stack = vec![tools];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("lua") {
                let text = std::fs::read_to_string(&path).expect("读脚本");
                if text.contains("menu = \"OCR 文字识别\"") {
                    owners.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    assert_eq!(
        owners,
        vec!["2.ocr_text.lua".to_string()],
        "OCR 文字识别只该有一个菜单项"
    );
}

/// ★ 迁移判据：图片体检 / OCR / AI 写脚本都只在 CurSeen-Scripts 官方包里；
/// 内置只剩音效脚本；复合示例按用户口径删除（测试夹具另存 `tests/fixtures`）。
#[test]
fn migrated_scripts_leave_builtin_and_live_in_the_official_repo() {
    // 只看**源里的出厂资产**：`build/assets` 是本机安装目录，商店装好后是在
    // `scripts/<id>/` 的稳定包文件夹里出现（记录写在文件夹内的 `.curseen-pack.json`）。
    let builtin = builtin_root().join("scripts");
    for name in [
        "1.image_report.lua",
        "2.ocr_text.lua",
        "3.composite_demo.lua",
        "5.ocr_post.lua",
        "ai_chat.lua",
        "ai_chat_api.txt",
    ] {
        assert!(
            !builtin.join(name).exists(),
            "内置脚本里不该再有 {name}（已迁移或按口径删除）"
        );
    }
    let keysound = builtin_root().join("KeySound");
    assert!(
        keysound.join("class_sound.lua").is_file() && keysound.join("unified_sound.lua").is_file(),
        "键音脚本应内置在 assets/lua/KeySound/"
    );
    let tools = repo_tools();
    for rel in [
        "ImageReport/scripts/1.image_report.lua",
        "OCR/scripts/2.ocr_text.lua",
        "AI/scripts/ai_chat.lua",
        "AI/scripts/ai_chat_api.txt",
    ] {
        assert!(tools.join(rel).is_file(), "官方包里缺 {rel}");
    }
}

/// ★ 二维码包同样只带官方工具：脚本必须指向 `barcode_tool`，不允许自己拼系统命令。
#[test]
fn barcode_pack_uses_the_official_tool_only() {
    let src = repo_tools().join("Barcode/scripts/barcode.lua");
    let src = std::fs::read_to_string(&src).expect("读 Barcode/barcode.lua");
    assert!(
        src.contains("./barcode_tool/CurSeenBarcode.exe"),
        "二维码脚本必须调用官方随包工具"
    );
    assert!(
        src.contains("name = \"条码识别\"") && src.contains("menu = \"条码识别\""),
        "条码识别的显示名/菜单名应一致"
    );
    assert!(
        src.contains("ext:open_temp") && src.contains("ext:open_url"),
        "图片/网址打开走宿主接口，不自己起 shell"
    );
    assert!(!src.contains("os.time"), "脚本沙箱没有 os 库");
}

/// ★ 「裁成居中正方形」按用户口径删除：源目录里不许再出现这个脚本（6_deploy 不裁剪，
/// 所以这里连 `build/assets` 那份一起钉住——两份都在仓内可见）。
#[test]
fn removed_crop_square_script_stays_gone() {
    let dir = builtin_root().join("scripts");
    assert!(
        !dir.join("4.crop_square.lua").exists(),
        "「裁成居中正方形」已删除，不该回到出厂脚本里"
    );
    let built = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../build/assets/lua/scripts/4.crop_square.lua");
    assert!(!built.exists(), "build/assets 里也删干净：{built:?}");
}

/// 白底黑字：WCAG 对比度必须落在 **21.0**（两端亮度的理论上限），判词 AAA。
#[test]
fn contrast_white_on_black_is_21_and_aaa() {
    let img = image(40, 40, |x, y| {
        if (12..28).contains(&x) && (12..28).contains(&y) {
            [0, 0, 0, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let out = run("1.image_report", &[], img);
    assert!(out.contains("21.0"), "白底黑字必须是 21.0:1：{out}");
    assert!(out.contains("#000000") && out.contains("#FFFFFF"), "{out}");
    assert!(out.contains("AAA"), "{out}");
}

/// ★ 阈值格（本轮判据的红证靶）：`#767676` 在白底上是 **4.54**——刚好过 AA、够不着 AAA。
/// 把阈值写成 `>=` 还是 `>`、或者把 4.5 记成 5.0，这一格都会翻。
#[test]
fn contrast_gray_767676_is_aa_but_not_aaa() {
    let img = image(40, 40, |x, y| {
        if (12..28).contains(&x) && (12..28).contains(&y) {
            [0x76, 0x76, 0x76, 255]
        } else {
            [255, 255, 255, 255]
        }
    });
    let out = run("1.image_report", &[], img);
    assert!(out.contains("4.5"), "#767676 在白底上应约 4.54：{out}");
    assert!(out.contains("#767676"), "{out}");
    assert!(
        !out.contains("AAA（正文与大号字都过）"),
        "4.54 不该被判成 AAA：{out}"
    );
    // ★ 必须断言**具体那一档**：只写 `contains("AA")` 时，把阈值从 4.5 改成 5.0
    //   （判词会变成「只够 AA-large（…）」）这一格照样绿——我第一版就是这样，红证把它判死了。
    assert!(
        out.contains("AA（正文过；AAA 要 7.0）"),
        "4.54 应落在 AA 正文档：{out}"
    );
    assert!(
        !out.contains("只够 AA-large"),
        "4.54 不该掉进 AA-large 档：{out}"
    );
}

/// ★ 四角四种颜色（平票）⇒ 背景必须取**第一个角**（左上），而且**每次都一样**。
///
/// 旧写法用 `pairs` 遍历哈希表择优：Lua 5.4 给字符串哈希加了随机种子 ⇒ 同一张图两次跑
/// 可能选到不同的角，判词跟着变（m24 审查抓到的"随机猜"）。这条钉的是**确定性**：
/// 跑两遍输出必须逐字相同，且背景恰好是左上那个颜色。
#[test]
fn contrast_is_deterministic_when_all_four_corners_differ() {
    let make = || {
        image(40, 40, |x, y| {
            let (left, top) = (x < 5, y < 5);
            let (right, bottom) = (x >= 35, y >= 35);
            if (12..28).contains(&x) && (12..28).contains(&y) {
                [0, 0, 0, 255]
            } else if left && top {
                [255, 0, 0, 255] // 左上：红
            } else if right && top {
                [0, 255, 0, 255]
            } else if left && bottom {
                [0, 0, 255, 255]
            } else if right && bottom {
                [255, 255, 0, 255]
            } else {
                [255, 255, 255, 255]
            }
        })
    };
    let a = run("1.image_report", &[], make());
    let b = run("1.image_report", &[], make());
    assert_eq!(a, b, "同一张图两次跑必须逐字相同");
    assert!(
        a.contains("背景：#FF0000"),
        "平票该取第一个角（左上红）：{a}"
    );
}

/// 整张图都是背景色：不许硬编一个前景出来，必须如实说"没有可比较的前景色"。
#[test]
fn contrast_says_so_when_the_image_is_one_flat_color() {
    let img = image(20, 20, |_, _| [18, 52, 86, 255]);
    let out = run("1.image_report", &[], img);
    assert!(out.contains("没有可比较的前景色"), "{out}");
    assert!(out.contains("#123456"), "该点名背景色：{out}");
}

/// ★ 取色板：四象限各占 1/4 的已知色，取 8 色。
/// 判据两半：**那 4 个 HEX 必须都在**（中位切分把每个象限再切一刀，均值仍等于原色），
/// 且占比之和 = 100%、每个 ~12.5% —— 只断言"有文字输出"是测不出任何东西的。
#[test]
fn palette_finds_the_four_quadrant_colors() {
    let img = image(4, 4, |x, y| match (x < 2, y < 2) {
        (true, true) => [255, 0, 0, 255],
        (false, true) => [0, 255, 0, 255],
        (true, false) => [0, 0, 255, 255],
        (false, false) => [255, 255, 0, 255],
    });
    let out = run("1.image_report", &[("colors", json!("8"))], img);
    let got: Vec<String> = hexes(&out);
    for want in ["#FF0000", "#00FF00", "#0000FF", "#FFFF00"] {
        assert!(got.contains(&want.to_string()), "缺 {want}，实际：{out}");
    }
    assert!(
        got.iter().any(|h| h != "#FF0000"),
        "只给一个颜色说明切分没工作：{out}"
    );
    let sh: Vec<f64> = shares(&out);
    assert!(!sh.is_empty(), "没有占比行：{out}");
    let sum: f64 = sh.iter().sum();
    assert!((sum - 100.0).abs() < 0.5, "占比之和必须是 100%，实际 {sum}");
    println!("palette/quadrants:\n{out}");
}
