//! 面板对账收敛判据：`PanelModel::apply_to` 没动控件就必须报「无改动」。
//!
//! 为什么单独立一个文件：`Runtime::sync_panel` 每帧（以及覆盖层的 250ms 定时器）
//! 都会跑一次 `apply_to`，它返回 true 就 bump `config_version`，各模块随之整段重放
//! ——`modules::trail::controller::sync_settings` 里还含悬浮标图片解码。所以
//! 「用户没动控件却一直返回 true」不是小毛病：面板打开期间它会按帧频反复重放。
//!
//! `ui::model` 里的 `model_no_spurious_change` 只钉住 `Config::default()`；真机上跑
//! 的是用户自己的 `build/config.json`（出厂默认之外的取值组合才逼得出量程错位），
//! 本文件把同一条承诺钉在它上面。文件不存在（全新克隆还没有产物目录）则跳过。

use curseen_ui::core::config::Config;
use curseen_ui::ui::model::PanelModel;
use serde_json::Value;

/// 运行期配置（便携版：配置落在程序目录）。
/// 与 `config_pipeline_e2e.rs` 同一份——那条是绘制链路，这条是对账收敛。
const SHIPPED_CONFIG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "\\..\\build\\config.json");

/// 收集两份 JSON 里取值不同的叶子路径（够定位到字段，不做数组逐项对齐以外的智能匹配）。
fn diff_paths(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut keys: Vec<&str> = x.keys().map(|s| s.as_str()).collect();
            for k in y.keys() {
                if !x.contains_key(k.as_str()) {
                    keys.push(k.as_str());
                }
            }
            keys.sort_unstable();
            for k in keys {
                let p = if path.is_empty() {
                    k.to_string()
                } else {
                    format!("{path}.{k}")
                };
                match (x.get(k), y.get(k)) {
                    (Some(l), Some(r)) => diff_paths(l, r, &p, out),
                    (l, r) => out.push(format!("{p}: {l:?} → {r:?}")),
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (l, r)) in x.iter().zip(y.iter()).enumerate() {
                diff_paths(l, r, &format!("{path}[{i}]"), out);
            }
        }
        (x, y) => {
            if x != y {
                out.push(format!("{path}: {x} → {y}"));
            }
        }
    }
}

/// 判据：建好面板 → 对账一次 → 再对账一次，两次都必须是「无改动」。
fn assert_converges(cfg: &Config, tag: &str) {
    let m = PanelModel::from_config(cfg);
    let mut back = cfg.clone();
    let first = m.apply_to(&mut back);
    let (ja, jb) = (
        serde_json::to_value(cfg).unwrap_or(Value::Null),
        serde_json::to_value(&back).unwrap_or(Value::Null),
    );
    let mut d1 = Vec::new();
    diff_paths(&ja, &jb, "", &mut d1);
    assert!(
        !first,
        "{tag}：面板刚建好、没动任何控件，apply_to 就报了改动 ⇒ 每帧都会 bump \
         config_version（差异 {} 处：{:?}）",
        d1.len(),
        &d1[..d1.len().min(12)]
    );

    // 第二次同样必须静默：防「第一次改过去、第二次又改回来」的振荡。
    let mut again = back.clone();
    let second = m.apply_to(&mut again);
    let jc = serde_json::to_value(&again).unwrap_or(Value::Null);
    let mut d2 = Vec::new();
    diff_paths(&jb, &jc, "", &mut d2);
    assert!(
        !second,
        "{tag}：第二次对账仍报改动 ⇒ 值在两个状态间来回跳（差异 {} 处：{:?}）",
        d2.len(),
        &d2[..d2.len().min(12)]
    );
}

#[test]
fn panel_converges_on_factory_defaults() {
    assert_converges(&Config::default(), "出厂默认配置");
}

#[test]
fn panel_converges_on_shipped_config() {
    let Ok(text) = std::fs::read_to_string(SHIPPED_CONFIG) else {
        return; // 产物目录不存在：全新克隆，跳过
    };
    let Ok(cfg) = serde_json::from_str::<Config>(&text) else {
        return; // 配置解析不了不是本判据要管的事（加载侧另有门禁）
    };
    assert_converges(&cfg, "用户真实配置");
}
