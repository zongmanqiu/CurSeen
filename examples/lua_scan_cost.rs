//! 量一次「右键菜单要花多久」：`LuaExtRegistry::scan()` 对 `assets/lua` 全树扫一遍的墙钟。
//!
//! 为什么值得单独量：贴图右键每次都要重扫（**刻意的**——用户把脚本粘进目录不该要求重启，
//! 见 `registry_ext` 的模块头）。这个代价落在"右键到菜单弹出"那段延迟上，正好是用户那把尺子
//! 里的「切图快 / 打开快」。加了特效包之后脚本数到 160+，先量再决定要不要缓存。
//!
//! 跑法：`cargo run --release --example lua_scan_cost`

use std::time::Instant;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/lua");
    // 预热一次：第一次会带上文件系统缓存缺失（冷盘不是用户的常态）。
    let (files, menu, warm) = {
        let mut reg = curseen_ui::scripting::registry_ext::LuaExtRegistry::new(&dir);
        let t = Instant::now();
        reg.scan();
        (
            reg.list().len(),
            reg.menu_items().len(),
            t.elapsed().as_millis(),
        )
    };
    let mut best = u128::MAX;
    let mut total = 0u128;
    const N: u32 = 5;
    for _ in 0..N {
        let mut reg = curseen_ui::scripting::registry_ext::LuaExtRegistry::new(&dir);
        let t = Instant::now();
        reg.scan();
        let ms = t.elapsed().as_millis();
        best = best.min(ms);
        total += ms;
    }
    println!("脚本 {files} 个（其中声明 menu 的 {menu} 个）");
    println!(
        "首次扫描 {warm} ms；热态 5 次：最好 {best} ms，平均 {} ms",
        total / u128::from(N)
    );
}
