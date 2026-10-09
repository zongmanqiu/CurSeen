//! 验证冒号调用修复：脚本用 `ext:json_decode(...)` / `ext:http(...)`（冒号）应正确工作。
//!
//! 修复前（`add_function`）：冒号把 `self` 当第一实参 → `ext:json_decode` 收到 userdata →
//!   "error converting Lua userdata to String" → run 报错。
//! 修复后（`add_method`）：`self` 被 `_this` 吃掉 → 正常返回表 / 响应。
//!
//! 脚本返回字符串（非空），便于直接读 `call_run_ctx` 的返回值判据，且不弹阻塞框。
//! 跑法：`cargo run --example repro_colon`
//!   - 打印 `Ok("json_x=42 http_status=0")` → 修复生效
//!   - 打印 `Err(...error converting Lua userdata...)` → 仍坏

use std::collections::HashMap;
use std::time::Duration;

fn main() {
    let tmp = std::env::temp_dir().join("curseen_repro_colon2");
    std::fs::create_dir_all(&tmp).unwrap();
    let script = r#"
function run(ctx, params)
  local d = ext:json_decode('{"x":42}')
  local r = ext:http("POST", "https://127.0.0.1:1/", {["Content-Type"]="application/json"}, '{"a":1}')
  return "json_x=" .. tostring(d and d.x) .. " http_status=" .. tostring(r.status)
end
"#;
    std::fs::write(tmp.join("c.lua"), script).unwrap();
    let h = std::thread::spawn(move || {
        let eng = curseen_ui::scripting::engine::ScriptingEngine::new(&tmp).unwrap();
        eng.load_script("c").unwrap();
        let r = eng.call_run_ctx("image", &[], &HashMap::new());
        println!("[repro] call_run_ctx returned: {r:?}");
    });
    std::thread::sleep(Duration::from_secs(2));
    println!("[repro] done");
    let _ = h.join();
}
