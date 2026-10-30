//! m10 端到量具：**生产脚本 + 外置引擎文件**跑通 OCR（宿主本体不变）。
//!
//! 要回答的问题只有一个，而且必须是读数不是推理：
//! 「一个 Lua 脚本 + 一堆外置文件，能不能真的让 CurSeen 获得更强的离线 OCR？」
//!
//! 于是这里跑的是**产品自己那条路**（`scripting::ext_runner::run_ext_sync` →
//! `CurSeen-Scripts/Packs/OCR/scripts/2.ocr_text.lua` → `ext:tmpdir` / `img:save` / `ext:proc` /
//! `parse_lines`），不是在 example 里复刻一份逻辑——复刻出来的样本永远不会红，不算验证。
//!
//! 三臂（同一张图、同一进程内）：
//! · `外置` —— 脚本目录里有 `ocr_tool/CurSeenOcr.exe`（官方构建输出）→ 走 `ext:proc`；
//! · `坏工具` —— 把那个 exe 换成 `where.exe`（同名、能起、但不是 OCR 工具）→ 脚本必须**如实报
//!   外置跑坏了**（m13 起系统 OCR 已拆除，这里**不许**再出现任何"改用系统"的字样，也不许粉饰）；
//! · `没装` —— 删掉整个 `ocr_tool/` 目录 → 脚本要**点名缺哪几个文件**。这一臂钉的是热键那条路：
//!   菜单会被 `requires` 挡掉，热键不挡，靠的就是这段兜底要说人话。
//!
//! 另外两格读数：`ocr_in.png-result.txt` 是否存在（外置引擎真的起过进程的**副作用**证据）、
//! 以及输出里有没有"锟斤拷"式乱码（证明 `ansi = true` 那条码页声明真的生效）。
//!
//! 跑法（需本机有 `.ex/ocr-rapid` 的 CLI 与 v4 模型，见 `.plan/m09`）：
//! `cargo run --release --example ext_proc_ocr`

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value as Json;

use curseen_ui::core::backdrop::shot_to_bitmap;
use curseen_ui::platform::text_raster::render_text;
use curseen_ui::scripting::api_ext::{ExtApi, ExtCtx};
use curseen_ui::scripting::ext_runner;

/// 样本：中英数混排，正好覆盖"国标码解错就成了锟斤拷"的那段字节。
const WANT: &str = "文字识别测试 CurSeen 2.0.5";
/// 官方 OCR 工具的构建输出（`tools/ocr/build.ps1`；不进仓库也不进产物）。
const TOOL_SRC: &str = "../.temp/curseen-tools/ocr";
/// 脚本要的五个文件：exe + 3 个模型 + 1 个字典。缺一个都跑不起来。
const TOOL_FILES: &[&str] = &[
    "CurSeenOcr.exe",
    "ch_PP-OCRv4_det_mobile.onnx",
    "ch_ppocr_mobile_v2.0_cls_mobile.onnx",
    "ch_PP-OCRv4_rec_mobile.onnx",
    "ppocr_keys_v1.txt",
];

fn pack_dir() -> PathBuf {
    std::env::temp_dir().join(format!("curseen_ext_proc_pack_{}", std::process::id()))
}

/// 外置引擎写回的副作用文件（在 `ext:tmpdir()` 里，与输入 PNG 同名加后缀）。
fn side_file() -> PathBuf {
    curseen_ui::scripting::api_ext::tmpdir_path().join("ocr_in.png-result.txt")
}

/// 搭一个"用户自己装的脚本包"：真脚本 + `ocr/` 子目录里的真工具文件。
fn build_pack(pack: &Path) -> Option<()> {
    let src = PathBuf::from(TOOL_SRC);
    if !src.is_dir() {
        eprintln!("找不到 {TOOL_SRC}，先跑 tools/ocr/build.ps1");
        return None;
    }
    let _ = std::fs::remove_dir_all(pack);
    std::fs::create_dir_all(pack.join("ocr_tool")).ok()?;
    // ★ 用**仓库里的那份脚本**，不在这里重写一遍逻辑
    std::fs::copy(
        Path::new("../CurSeen-Scripts/Packs/OCR/scripts/2.ocr_text.lua"),
        pack.join("2.ocr_text.lua"),
    )
    .ok()?;
    for f in TOOL_FILES {
        std::fs::copy(src.join(f), pack.join("ocr_tool").join(f)).ok()?;
    }
    Some(())
}

/// ★ m14 才有的那一格：**用脚本侧的 `ext:read_file` 把外置工具写在脚本目录之外的产物读回来**。
///
/// 在这一格之前，那条链是断的——RapidOCR 除了 stdout 还会写一份 `<图片>-result.txt`，
/// 而它落在临时目录里，`ctx.fs`（限脚本目录、拒绝对路径）拿不到，脚本只能指望 stdout。
/// 现在走的是产品对脚本开放的那只口子，读到的字节就是盘上那份，不是宿主复刻的逻辑。
fn read_back_via_api(path: &Path) -> String {
    let lua = mlua::Lua::new();
    lua.globals()
        .set("ext", lua.create_userdata(ExtApi).unwrap())
        .unwrap();
    lua.globals().set("p", path.display().to_string()).unwrap();
    lua.load(
        r#"
        local d, err = ext:read_file(p)
        if d == nil then return "READ-ERR:" .. tostring(err) end
        return string.format("%d字节", #d)
        "#,
    )
    .eval::<String>()
    .unwrap_or_else(|e| format!("LUA-ERR:{e}"))
}

/// 组一个脚本参数表（量具用它把 `copy` 关掉，免得动用户的剪贴板）。
fn param_map(pairs: &[(&str, Json)]) -> HashMap<String, Json> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

/// 跑一次生产脚本（同一张图），返回脚本文本。`id` = 宿主登记里的脚本 id。
fn run_script(
    pack: &Path,
    id: &str,
    img: curseen_ui::core::image::RgbaBitmap,
) -> Result<String, String> {
    let t0 = std::time::Instant::now();
    let r = ext_runner::run_ext_sync(
        pack,
        id,
        // 量具**不**动用户剪贴板：出厂脚本默认会把识别结果复制出去，这里显式关掉。
        // （`ext:clip_set` 那一格的读数在 `api_ext::tests::clip_roundtrip…` 那条 opt-in 测试里。）
        &param_map(&[("copy", Json::Bool(false))]),
        Some(ExtCtx {
            image: img,
            hwnd: 0,
        }),
    );
    let ms = t0.elapsed().as_millis();
    match r {
        // 外置那臂的墙钟**含起进程 + 载模型**：这才是用户按一次热键感到的延迟。
        Ok(text) => Ok(format!("{text}\n[本次 {ms} ms]")),
        Err(e) => Err(format!("{e}（{ms} ms）")),
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// 文本里是否出现了"码页解错"的典型残骸（替换字符或锟斤拷）。
fn garbled(s: &str) -> bool {
    s.contains('\u{FFFD}') || s.contains("锟")
}

fn main() {
    let Some(shot) = render_text(WANT, 1200, 3.0) else {
        eprintln!("排版样本失败");
        std::process::exit(1);
    };
    let Some(img) = shot_to_bitmap(&shot) else {
        eprintln!("位图转换失败");
        std::process::exit(1);
    };

    // —— 只读那一臂：对着**真实安装目录**验"外置引擎是否真的生效" ——
    // `CURSEEN_OCR_PACK=<脚本根目录>` 时只跑这一臂：不搭临时包、不改脚本副本、不删任何东西
    // （那三臂是破坏性的，会把你装好的 `ocr/` 删掉）。用来回答"产品现在到底走的哪条"。
    if let Ok(real) = std::env::var("CURSEEN_OCR_PACK") {
        let dir = PathBuf::from(&real);
        let out = run_script(&dir, "scripts/OCR/2.ocr_text", img.clone());
        let side = side_file().is_file();
        println!("== 真实安装目录 {real} ==");
        match &out {
            Ok(t) => {
                println!("{}", t.lines().take(8).collect::<Vec<_>>().join("\n"));
                println!("   CLI 副作用文件（`ocr_in.png-result.txt`）存在：{side}");
            }
            Err(e) => println!("   失败：{e}"),
        }
        let text = out.unwrap_or_default();
        // m14 新读的一格：工具写在脚本目录外的产物，脚本侧现在取回来了。
        let back = read_back_via_api(&side_file());
        println!("   ext:read_file 读回 CLI 的产物：{back}");
        let mut bad: Vec<String> = Vec::new();
        if back.starts_with("READ-ERR") || back.starts_with("LUA-ERR") || back == "0字节" {
            bad.push(format!("读不回外置工具的产物 ⇒ 回程还没通：{back}"));
        }
        if !norm(&text).contains(&norm(WANT)) {
            bad.push("没命中全文".to_string());
        }
        if text.contains("系统自带 OCR") || text.contains("识别失败") {
            bad.push("输出说它走的是系统那一侧 ⇒ 外置没生效".to_string());
        }
        if !side {
            bad.push("没有 CLI 的副作用文件 ⇒ 外置进程没起".to_string());
        }
        if garbled(&text) || text.contains("鏂囧") {
            bad.push("输出里有码页解错的残骸".to_string());
        }
        // —— 长行那一臂（m29，2026-10-28 用户报）——
        // 一行 60+ 字符的英文命令。旧参数 `-a 1` 会让**角度分类器**把这种扁长的行误判成
        // 180°，rec 拿着倒过来的图出 "o -s-- --oi d-a--" 这类乱码；脚本改成 `-a 0` 后
        // 逐字正确。样本用宿主自己的排版器画（与产品同一套字体），不依赖外部图片。
        // —— 密集长行那一臂（m29，2026-10-28 用户报）——
        // 夹具 `tests/fixtures/ocr_dense_latin.png`：三行 60 字符左右的小字号英文（760×132），
        // 形状照着用户那张终端截图。**这条必须能红**：撤掉脚本里的 `-a 0`，CLI 的**角度
        // 分类器**会把这种扁长的行判成 180°，rec 拿着倒过来的图出 "o -s-- --oi d-a--" 一类乱码。
        // ★ 为什么不用 `render_text` 现场画：宿主排版器画出来的同样内容**不会**触发误判
        //   （cls 对它分类正确）——试过，撤掉 `-a 0` 也照样绿。夹具必须落在判据管的那个分支里。
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ocr_dense_latin.png");
        if let Some(shot) = curseen_ui::platform::screenshot::load_png(&fixture) {
            if let Some(img2) = shot_to_bitmap(&shot) {
                match run_script(&dir, "scripts/OCR/2.ocr_text", img2) {
                    Ok(t) => {
                        let flat = norm(&t);
                        println!(
                            "   密集长行样本：{}",
                            t.lines().take(3).collect::<Vec<_>>().join(" / ")
                        );
                        for want in [
                            "cargo bloat --release -p rupix-common --lib --crates -n 30",
                            "cargo bloat --release -p rupix-pro --lib --crates -n 30",
                        ] {
                            if !flat.contains(&norm(want)) {
                                println!(
                                    "!! 密集长行没逐字命中（角度分类器关了吗？脚本里应有 `-a 0`）"
                                );
                                std::process::exit(1);
                            }
                        }
                    }
                    Err(e) => {
                        println!("!! 密集长行样本失败：{e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        if bad.is_empty() {
            println!("判定：产品的这一次识别**走的是外置引擎**（不是系统 OCR 蒙的）。");
        } else {
            for b in &bad {
                println!("!! {b}");
            }
            std::process::exit(1);
        }
        return;
    }

    let pack = pack_dir();
    if build_pack(&pack).is_none() {
        return;
    }
    let _ = std::fs::remove_file(side_file());

    // —— 臂 1：外置引擎 ——
    let arm_ext = run_script(&pack, "2.ocr_text", img.clone());
    let side_after_ext = side_file().is_file();
    println!("== 臂 1 外置引擎（脚本目录里有 ocr\\CurSeenOcr.exe）==");
    match &arm_ext {
        Ok(t) => {
            println!("{t}");
            println!("   副作用文件（CLI 自己写的 -result.txt）存在：{side_after_ext}");
        }
        Err(e) => println!("   失败：{e}"),
    }

    // —— 臂 1b：同一张图、同一个工具，只把脚本里那句 `ansi = false` 翻成 `true` ——
    // 这一臂是给"码页开关真的在通路上"钉的红证：CLI 吐的是 UTF-8，按国标码解必须出"鏂囧瓧"。
    // 开关如果是死的（参数没传下去／解码头尾接错），这一臂就和臂 1 一模一样，当场就该红。
    let script = pack.join("2.ocr_text.lua");
    let src_lua = std::fs::read_to_string(&script).expect("读脚本副本");
    let patched = src_lua.replace("ansi = false", "ansi = true");
    assert_ne!(
        patched, src_lua,
        "脚本副本里没有 `ansi = false` 这一句，这一臂什么都没测"
    );
    std::fs::write(&script, &patched).expect("写脚本副本");
    let arm_ansi = run_script(&pack, "2.ocr_text", img.clone());
    std::fs::write(&script, &src_lua).expect("还原脚本副本");
    println!("\n== 臂 1b 同一工具按国标码解（开关必须是活的）==");
    match &arm_ansi {
        Ok(t) => println!("{}", t.lines().next().unwrap_or("")),
        Err(e) => println!("   失败：{e}"),
    }

    // —— 臂 2：同名但是个假工具 → 必须如实报外置跑坏了（没有退路）——
    let exe = pack.join("ocr_tool").join("CurSeenOcr.exe");
    let sys_where = PathBuf::from(std::env::var("WINDIR").unwrap_or_default())
        .join("System32")
        .join("where.exe");
    if std::fs::copy(&sys_where, &exe).is_err() {
        eprintln!("换假工具失败，跳过臂 2/3");
        return;
    }
    let arm_bad = run_script(&pack, "2.ocr_text", img.clone());
    println!("\n== 臂 2 同名假工具（外置那步必须红）==");
    match &arm_bad {
        Ok(t) => println!("{}", t.lines().take(6).collect::<Vec<_>>().join("\n")),
        Err(e) => println!("   脚本返回失败：{e}"),
    }

    // —— 臂 3：整个 `ocr/` 都不在 → 该点名缺的文件（热键那条路的兜底）——
    let _ = std::fs::remove_dir_all(pack.join("ocr_tool"));
    let arm_sys = run_script(&pack, "2.ocr_text", img.clone());
    println!("\n== 臂 3 没装外置引擎（该点名缺的文件，不许提系统引擎）==");
    match &arm_sys {
        Ok(t) => println!("{}", t.lines().take(6).collect::<Vec<_>>().join("\n")),
        Err(e) => println!("   失败：{e}"),
    }

    // —— 判词 ——
    let mut fails: Vec<String> = Vec::new();
    let ext_text = arm_ext.unwrap_or_default();
    if norm(&ext_text).contains(&norm(WANT)) {
        println!("\n外置：命中样本全文");
    } else {
        fails.push(format!(
            "外置引擎没给全文，实际：{:?}",
            ext_text.replace('\n', " / ")
        ));
    }
    if !side_after_ext {
        fails.push("外置那臂没写下 CLI 的副作用文件 ⇒ 外置进程可能根本没起".to_string());
    }
    if garbled(&ext_text) {
        fails.push("输出里有码页解错的残骸 ⇒ 默认的 UTF-8 读法被改坏了".to_string());
    }
    let bad_text = arm_bad.unwrap_or_default();
    if !(bad_text.contains("外置引擎") || bad_text.contains("外部引擎")) {
        fails.push(format!(
            "假工具那臂应当报出外置失败，实际：{:?}",
            bad_text.replace('\n', " / ")
        ));
    }
    if bad_text.contains("系统自带 OCR") || bad_text.contains("已改用系统") {
        fails.push(format!(
            "系统 OCR 已拆除（m13），假工具那臂不许还有退路：{:?}",
            bad_text.replace('\n', " / ")
        ));
    }
    let sys_text = arm_sys.unwrap_or_default();
    // 没装那一臂：既不许偷偷跑一次注定失败的识别，也不许提系统引擎——要的是"缺哪几个文件"。
    if !sys_text.contains("./ocr_tool/CurSeenOcr.exe") {
        fails.push(format!(
            "没装引擎那臂该点名缺的文件，实际：{:?}",
            sys_text.replace('\n', " / ")
        ));
    }
    if sys_text.contains("系统自带 OCR") || sys_text.contains("ext:ocr") {
        fails.push("没装引擎那臂不许指向已拆除的系统 OCR".to_string());
    }

    println!("\n== 汇总 ==");
    if fails.is_empty() {
        println!("三臂都符合预期：脚本 + 外置文件 ⇒ OCR 能力，本体没动。");
    } else {
        for f in &fails {
            println!("!! {f}");
        }
        std::process::exit(1);
    }
    let _ = std::fs::remove_dir_all(&pack);
}
