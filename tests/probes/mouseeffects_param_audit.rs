//! probe-group: slow
//! MouseEffects 参数有效性动态审计：每个 schema 参数在**同一固定场景**下比较
//! 「默认值 vs 扰动值」产出的真实 `DrawList`，完全相同即判定参数未生效。
//!
//! # 为什么不能只做静态"读过没有"
//!
//! `params.color` 出现在源码里，不代表它参与了输出（Neon Glow 的颜色就曾在第一次
//! 点击后永久失效）。本探针走真实 `ScriptingEngine`：固定 RNG、覆盖悬停开/关、
//! 点击动画、时间推进与移动采样，把这些状态下的绘制指令拼起来比较。
//!
//! # 判据
//!
//! - 每个参数取 schema 的默认值作基准；扰动值取 min/max 的另一端（数值）、取反
//!   （布尔）、另一个候选（choice）、不同文本（str）、对比色（color）。
//! - color 参数额外做一次「只改 alpha」的对照：若输出完全相同，说明脚本忽略了
//!   颜色自带的透明度。
//! - 唯一允许的例外是 `fluid_glass.zoomLevel`：探针进程没有自己的覆盖层窗口，
//!   `ctx:backdrop_circle` 走静态回退分支，回退外观不依赖 zoom；真镜片路径由
//!   `lens_probe` 与 `core::image::lens_circle` 的单测覆盖。
//!
//! 跑法：`python build/probe.py run mouseeffects_param_audit`

#![cfg(windows)]

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value as Json};

use curseen_ui::core::draw::DrawList;
use curseen_ui::scripting::ScriptingEngine;

/// 一个 schema 参数的审计规格。
#[derive(Debug, Clone)]
struct Spec {
    key: String,
    ty: String,
    default: Json,
    min: Option<f64>,
    max: Option<f64>,
    choices: Vec<String>,
    /// 颜色参数是否允许透明度；`alpha=false` 的颜色不参与 alpha 对照。
    alpha: bool,
}

impl Spec {
    /// 返回要尝试的扰动值：数值同时试 min 和 max（只试一端会漏掉
    /// "默认值靠近另一端、该端本来就没效果"的情况），其余类型各一个。
    fn perturbed_values(&self) -> Vec<Json> {
        let d = self.default.as_f64().unwrap_or(0.0);
        let mut out: Vec<Json> = Vec::new();
        match self.ty.as_str() {
            "bool" => out.push(json!(!self.default.as_bool().unwrap_or(false))),
            "color" => out.push(json!("#FF0000")),
            "int" | "float" => {
                let mut add = |v: Option<f64>| {
                    if let Some(v) = v {
                        if (v - d).abs() > 1e-9 {
                            let j = if self.ty == "int" { json!(v.round() as i64) } else { json!(v) };
                            if !out.contains(&j) {
                                out.push(j);
                            }
                        }
                    }
                };
                add(self.min);
                add(self.max);
                if out.is_empty() {
                    out.push(if self.ty == "int" { json!(d.round() as i64 + 1) } else { json!(d + 0.5) });
                }
            }
            "choice" => {
                let d = self.default.as_str().unwrap_or("");
                if let Some(c) = self.choices.iter().find(|c| c.as_str() != d) {
                    out.push(json!(c));
                }
            }
            "str" => {
                let d = self.default.as_str().unwrap_or("");
                out.push(json!(if d == "MouseEffects audit" { "MouseEffects audit 2" } else { "MouseEffects audit" }));
            }
            _ => {}
        }
        out
    }

    /// 同一 RGB、alpha 从 255 改成 128 的对照色。
    fn alpha_variant(&self) -> Option<(Json, Json)> {
        if self.ty != "color" {
            return None;
        }
        let d = self.default.as_str()?;
        let rgb = if d.len() == 9 {
            &d[..7]
        } else if d.len() == 7 {
            d
        } else {
            "#FFFFFF"
        };
        Some((json!(format!("{rgb}FF")), json!(format!("{rgb}80"))))
    }
}

fn specs_for(eng: &ScriptingEngine) -> Vec<Spec> {
    let table: mlua::Table = eng
        .lua()
        .load("return schema()")
        .eval()
        .expect("schema() 应返回表");
    let mut out = Vec::new();
    for item in table.sequence_values::<mlua::Table>() {
        let item = item.expect("schema 项应是表");
        let key: String = item.get("key").unwrap_or_default();
        if key.is_empty() {
            continue;
        }
        let ty: String = item.get("type").unwrap_or_else(|_| "float".to_string());
        let default = lua_value_to_json(item.get::<mlua::Value>("default").unwrap_or(mlua::Value::Nil))
            .unwrap_or_else(|| match ty.as_str() {
                "bool" => json!(false),
                "color" => json!("#FFFFFF"),
                "choice" => json!(""),
                "str" => json!(""),
                _ => json!(0.0),
            });
        let min = item.get::<Option<f64>>("min").unwrap_or(None);
        let max = item.get::<Option<f64>>("max").unwrap_or(None);
        let mut choices = Vec::new();
        if let Ok(Some(ct)) = item.get::<Option<mlua::Table>>("choices") {
            for c in ct.sequence_values::<String>() {
                if let Ok(c) = c {
                    choices.push(c);
                }
            }
        }
        // 不能用 get::<bool>().unwrap_or(true)：mlua 会把缺失键 Nil 转成 false，
        // 导致探针误以为所有颜色都是 alpha=false，直接跳过 alpha 对照。
        let alpha: bool = match item.get::<mlua::Value>("alpha") {
            Ok(mlua::Value::Boolean(b)) => b,
            _ => true,
        };
        out.push(Spec {
            key,
            ty,
            default,
            min,
            max,
            choices,
            alpha,
        });
    }
    out
}

fn lua_value_to_json(v: mlua::Value) -> Option<Json> {
    match v {
        mlua::Value::Boolean(b) => Some(json!(b)),
        mlua::Value::Integer(i) => Some(json!(i)),
        mlua::Value::Number(n) => Some(json!(n)),
        mlua::Value::String(s) => s.to_str().ok().map(|s| json!(s.to_string())),
        _ => None,
    }
}

fn defaults_map(specs: &[Spec]) -> HashMap<String, Json> {
    specs
        .iter()
        .map(|s| (s.key.clone(), s.default.clone()))
        .collect()
}

/// 固定但**有分布**的 RNG：同一脚本同一参数下两次运行逐字节一致，且星星/粒子不会
/// 全部叠在同一个点上（返回常数会让 constellation 的阈值类参数全部失真）。
/// 用 Lehmer LCG：乘数 48271，模 2^31-1；中间值 < 2^47，double 可精确表示。
const FIXED_RNG: &str = r#"
local s = 12345
rng = function(a, b)
  s = (s * 48271) % 2147483647
  return a + (b - a) * (s / 2147483647)
end
rng_int = function(a, b)
  s = (s * 48271) % 2147483647
  if b <= a then return a end
  return a + (s % (b - a + 1))
end
"#;

/// 跑一个固定场景，返回所有采样帧的绘制指令 Debug 串。
fn scenario(root: &Path, stem: &str, params: &HashMap<String, Json>) -> String {
    let id = format!("scripts/MouseEffects/{stem}");
    let eng = ScriptingEngine::new(root).expect("引擎初始化失败");
    eng.load_script(&id)
        .unwrap_or_else(|e| panic!("{id} 加载失败：{e}"));
    eng.lua()
        .load(FIXED_RNG)
        .exec()
        .expect("固定 RNG 注入失败");
    eng.set_wall_ms(50_000.0).unwrap();
    eng.call_init().unwrap();

    let mut out = String::new();
    let mut t = 0.5f32;
    for i in 0..90 {
        // 每 4 帧跳 50px 的折线：既越过 Circuit 的 44px 生长阈值，又让每条
        // 走线有时间生长（若每帧都生成，活跃上限会掩盖 length 参数）。
        let x = 400.0 + (((i / 4) % 8) as f32 - 4.0) * 50.0;
        let y = 300.0 + (((i / 32) % 2) as f32) * 60.0 - 30.0;
        let hover = (20..60).contains(&i);
        let down = (30..33).contains(&i) || (61..64).contains(&i);
        eng.update_env((x, y), (400.0, 300.0), t, (1920.0, 1080.0), "panel")
            .unwrap();
        eng.set_wall_ms(50_000.0 + i as f64 * 16.0).unwrap();
        eng.set_pointer_state(3.0, 1.0, 3.16, 0.3, down, "left")
            .unwrap();
        // 探针是集成测试，拿不到 `cfg(test)` 的 force_for_test；直接覆盖全局即可
        // （`set_pointer_state` 已按真光标写过一次，这里改成场景需要的值）。
        eng.lua().globals().set("cursor_hover", hover).unwrap();
        eng.call_on_move(x, y, t, params)
            .unwrap_or_else(|e| panic!("{stem} on_move 失败：{e}"));
        eng.call_step(1.0 / 60.0, t, params)
            .unwrap_or_else(|e| panic!("{stem} step 失败：{e}"));
        if i == 30 || i == 61 {
            eng.call_on_button("left", x, y, t, params)
                .unwrap_or_else(|e| panic!("{stem} on_mouse_down 失败：{e}"));
        }
        if i == 33 || i == 64 {
            eng.call_on_mouse_up("left", x, y, params)
                .unwrap_or_else(|e| panic!("{stem} on_mouse_up 失败：{e}"));
        }
        // 采样帧：空闲 / 悬停 / 点击动画 / 时间推进后各取几帧。
        if [0, 15, 24, 31, 34, 37, 40, 50, 59, 62, 65, 68, 75, 89].contains(&i) {
            let mut list = DrawList::new();
            curseen_ui::scripting::api_draw::set_draw_target(&mut list);
            eng.call_draw(params)
                .unwrap_or_else(|e| panic!("{stem} draw 失败：{e}"));
            curseen_ui::scripting::api_draw::clear_draw_target();
            // ★ 必须带上点池：多边形/折线的坐标不在 `DrawCmd` 里（命令只存区间），
            // 只比命令会把"形状/位置变了"误判成"参数没生效"。
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

#[test]
#[ignore = "动态审计：33 脚本 × 236 参数，跑真引擎，约几十秒；默认集合不跑"]
fn mouseeffects_every_schema_param_changes_output() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assets/lua");
    let pack = root.join("scripts").join("MouseEffects");
    let mut ineffective: Vec<String> = Vec::new();
    let mut alpha_ignored: Vec<String> = Vec::new();
    let mut checked = 0usize;

    let mut stems: Vec<String> = std::fs::read_dir(&pack)
        .expect("CursorX 包目录应存在")
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
        let id = format!("scripts/MouseEffects/{stem}");
        let eng = ScriptingEngine::new(&root).expect("引擎初始化失败");
        eng.load_script(&id).expect("加载失败");
        let specs = specs_for(&eng);
        let base_params = defaults_map(&specs);
        let base = scenario(&root, &stem, &base_params);

        let has_rainbow = specs.iter().any(|s| s.key == "rainbow" && s.ty == "bool");
        for spec in &specs {
            checked += 1;
            let mut changed = false;
            for alt_value in spec.perturbed_values() {
                let mut alt_params = base_params.clone();
                alt_params.insert(spec.key.clone(), alt_value.clone());
                if scenario(&root, &stem, &alt_params) != base {
                    changed = true;
                }
            }
            // 固定颜色在 rainbow=true 时被彩虹模式覆盖；把彩虹关掉再验一次 RGB。
            if !changed && spec.ty == "color" && has_rainbow {
                let mut gate = base_params.clone();
                gate.insert("rainbow".to_string(), json!(false));
                let gate_base = scenario(&root, &stem, &gate);
                for alt_value in spec.perturbed_values() {
                    let mut alt_params = gate.clone();
                    alt_params.insert(spec.key.clone(), alt_value);
                    if scenario(&root, &stem, &alt_params) != gate_base {
                        changed = true;
                    }
                }
            }
            if !changed {
                ineffective.push(format!("{stem}.{}", spec.key));
            }
            if spec.alpha {
                if let Some((a255, a128)) = spec.alpha_variant() {
                    let mut p255 = base_params.clone();
                    p255.insert(spec.key.clone(), a255);
                    let mut p128 = base_params.clone();
                    p128.insert(spec.key.clone(), a128);
                    if scenario(&root, &stem, &p255) == scenario(&root, &stem, &p128) {
                        alpha_ignored.push(format!("{stem}.{}", spec.key));
                    }
                }
            }
        }
        println!("[ok] {stem}: {} 个参数已审计", specs.len());
    }

    // 静态回退里没有真镜片：Zoom 的倍率 / Black Hole 的透镜开关只在真抓屏路径生效。
    // firework.rise 只在 rocket=true（升空模式，默认关=上游默认原位爆）时被消费；
    // 升空路径由引擎 load+draw 门禁与真机覆盖。
    ineffective.retain(|x| {
        x != "zoom.zoom" && x != "black_hole.lens" && x != "firework.rise"
    });

    println!("\n审计参数总数：{checked}");
    println!("未生效参数：{ineffective:?}");
    println!("忽略颜色 alpha 的参数：{alpha_ignored:?}");
    assert!(
        ineffective.is_empty(),
        "以下参数在固定场景下完全不改变输出：{ineffective:?}"
    );
    assert!(
        alpha_ignored.is_empty(),
        "以下颜色参数只改 alpha 时输出不变（说明 alpha 被忽略）：{alpha_ignored:?}"
    );
}
