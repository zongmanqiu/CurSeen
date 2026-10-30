//! probe-group: slow
//! 四包颜色 alpha 动态审计：每个颜色参数在同一多通道场景下比较 alpha 255 vs 128。
//!
//! 背景：宿主 `parse_schema_table` 曾用 `get::<bool>().unwrap_or(true)` 读 alpha，
//! mlua 会把缺失键 Nil 转成 false，导致所有没写 alpha 的颜色选择器都没有透明度。
//! 修复后颜色默认带 alpha；本探针验证**四个包**的脚本确实把 alpha 乘进了输出。
//!
//! 跑法：`python build/probe.py run color_alpha_audit`

#![cfg(windows)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value as Json};

use curseen_ui::core::draw::DrawList;
use curseen_ui::scripting::ScriptingEngine;

const FIXED_RNG: &str = r#"
local s = 12345
rng = function(a, b)
  s = (s * 48271) % 2147483647
  return a + (b - a) * (s / 2147483647)
end
"#;

fn specs_for(eng: &ScriptingEngine) -> Vec<(String, Json, String, bool)> {
    let table: mlua::Table = eng.lua().load("return schema()").eval().expect("schema()");
    let mut out = Vec::new();
    for item in table.sequence_values::<mlua::Table>() {
        let item = item.expect("schema 项");
        let key: String = item.get("key").unwrap_or_default();
        let ty: String = item.get("type").unwrap_or_else(|_| "float".into());
        let default = match item.get::<mlua::Value>("default") {
            Ok(mlua::Value::String(s)) => Json::String(s.to_string_lossy()),
            _ => Json::String("#FFFFFF".into()),
        };
        let alpha = match item.get::<mlua::Value>("alpha") {
            Ok(mlua::Value::Boolean(b)) => b,
            _ => true,
        };
        if ty == "color" && !key.is_empty() {
            out.push((key, default, ty, alpha));
        }
    }
    out
}

fn defaults_for(eng: &ScriptingEngine) -> HashMap<String, Json> {
    let table: mlua::Table = eng.lua().load("return schema()").eval().expect("schema()");
    let mut out = HashMap::new();
    for item in table.sequence_values::<mlua::Table>() {
        let item = item.expect("schema 项");
        let key: String = item.get("key").unwrap_or_default();
        let ty: String = item.get("type").unwrap_or_else(|_| "float".into());
        let v = match item.get::<mlua::Value>("default") {
            Ok(mlua::Value::Boolean(b)) => json!(b),
            Ok(mlua::Value::Integer(i)) => json!(i),
            Ok(mlua::Value::Number(n)) => json!(n),
            Ok(mlua::Value::String(s)) => json!(s.to_string_lossy()),
            _ => match ty.as_str() {
                "bool" => json!(false),
                "color" => json!("#FFFFFF"),
                "choice" | "str" => json!(""),
                _ => json!(0.0),
            },
        };
        if !key.is_empty() {
            out.insert(key, v);
        }
    }
    out
}

fn scenario(root: &Path, pack: &str, stem: &str, params: &HashMap<String, Json>) -> String {
    let id = format!("scripts/{pack}/{stem}");
    let eng = ScriptingEngine::new(root).expect("引擎初始化");
    eng.load_script(&id).unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
    eng.lua().load(FIXED_RNG).exec().expect("固定 RNG");
    eng.set_wall_ms(50_000.0).unwrap();
    eng.call_init().unwrap();

    let mut out = String::new();
    let mut t = 0.5f32;
    for i in 0..121 {
        let moving = i < 30 || (i >= 100);
        let x = if moving {
            400.0 + (((i / 4) % 8) as f32 - 4.0) * 50.0
        } else {
            420.0
        };
        let y = if moving { 300.0 + (((i / 32) % 2) as f32) * 60.0 - 30.0 } else { 300.0 };
        let down = (50..81).contains(&i);
        let hover = i >= 90;
        eng.update_env((x, y), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
            .unwrap();
        eng.set_wall_ms(50_000.0 + i as f64 * 16.0 + if i >= 90 { 1600.0 } else { 0.0 })
            .unwrap();
        eng.set_pointer_state(12.0, 1.0, 12.04, 0.1, down, "left")
            .unwrap();
        eng.lua().globals().set("cursor_hover", hover).unwrap();
        if moving {
            let _ = eng.call_on_move(x, y, t, params);
        }
        if i == 30 {
            let _ = eng.call_on_button("left", x, y, t, params);
        }
        if i == 33 {
            let _ = eng.call_on_mouse_up("left", x, y, params);
        }
        // 50–80 按住：给 Hold 通道留出完整进度；81 抬起。
        if i == 50 {
            let _ = eng.call_on_button("left", x, y, t, params);
        }
        if i == 81 {
            let _ = eng.call_on_mouse_up("left", x, y, params);
        }
        // 右键/中键的 ripple 颜色也要真的被使用；放在长按之后，避免干扰按住态。
        if i == 84 {
            let _ = eng.call_on_button("right", x, y, t, params);
        }
        if i == 85 {
            let _ = eng.call_on_mouse_up("right", x, y, params);
        }
        if i == 86 {
            let _ = eng.call_on_button("middle", x, y, t, params);
        }
        if i == 87 {
            let _ = eng.call_on_mouse_up("middle", x, y, params);
        }
        if i == 40 || i == 44 {
            let _ = eng.call_on_wheel(120, false, x, y, params);
        }
        eng.call_step(1.0 / 60.0, t, params).unwrap();
        if [5, 15, 25, 31, 34, 37, 42, 45, 46, 52, 60, 70, 80, 83, 85, 86, 87, 88, 92, 96, 102, 110, 120]
            .contains(&i)
        {
            let mut list = DrawList::new();
            curseen_ui::scripting::api_draw::set_draw_target(&mut list);
            let _ = eng.call_draw(params);
            curseen_ui::scripting::api_draw::clear_draw_target();
            out.push_str(&format!(
                "@{}:{:?}|{:?}\n",
                i,
                list.ordered().collect::<Vec<_>>(),
                list.points()
            ));
        }
        t += 1.0 / 60.0;
    }
    out
}

fn rgb_of(color: &str) -> String {
    if color.len() >= 7 && color.starts_with('#') {
        color[..7].to_string()
    } else {
        "#FFFFFF".to_string()
    }
}

#[test]
#[ignore = "动态颜色 alpha 审计：四包约 140 个颜色，跑真引擎，几秒；默认集合不跑"]
fn all_pack_colors_honor_alpha() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
    let mut checked = 0usize;
    let mut ignored = Vec::new();
    for pack in ["CursorFX", "CursorX", "MFCMouseEffect", "MouseEffects"] {
        let dir = root.join("scripts").join(pack);
        let mut stems: Vec<String> = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("{pack} 目录不可读：{e}"))
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().and_then(|x| x.to_str()) == Some("lua"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        stems.sort();
        for stem in stems {
            let id = format!("scripts/{pack}/{stem}");
            let eng = ScriptingEngine::new(&root).expect("引擎初始化");
            eng.load_script(&id).unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
            let colors = specs_for(&eng);
            if colors.is_empty() {
                continue;
            }
            let base = defaults_for(&eng);
            for (key, default, _ty, alpha) in colors {
                if !alpha {
                    continue;
                }
                let rgb = rgb_of(default.as_str().unwrap_or("#FFFFFF"));
                let mut p255 = base.clone();
                p255.insert(key.clone(), json!(format!("{rgb}FF")));
                let mut p128 = base.clone();
                p128.insert(key.clone(), json!(format!("{rgb}80")));
                checked += 1;
                if scenario(&root, pack, &stem, &p255) == scenario(&root, pack, &stem, &p128) {
                    ignored.push(format!("{pack}/{stem}.{key}"));
                }
            }
        }
    }
    println!("颜色 alpha 审计：{checked} 个");
    println!("忽略 alpha：{ignored:?}");
    assert!(ignored.is_empty(), "以下颜色参数的 alpha 不改变输出：{ignored:?}");
}
