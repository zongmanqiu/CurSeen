//! 开发期工具：把**真实生成**的更新脚本写到指定目录，用于手工/自动化验证更新流程。
//!
//! 更新脚本平时由 `installer::install` 按"当前 exe 所在目录"生成，指向不了任意沙箱目录；
//! 这个示例复用同一份 `bat_script`，保证沙箱里跑的和线上跑的是同一段脚本文本。
//!
//! 用法：
//!
//! ```text
//! cargo run --example gen_update_bat -- <程序目录> <zip 路径> <exe 文件名>
//! ```
//!
//! 输出：写到 `<程序目录>/curseen_update.bat` 的脚本路径。

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        eprintln!("用法: gen_update_bat <程序目录> <zip 路径> <exe 文件名>");
        std::process::exit(2);
    }
    let app = std::path::Path::new(args[0].as_str());
    let zip = std::path::Path::new(args[1].as_str());
    let script = curseen_ui::update::installer::bat_script(app, zip, &args[2]);
    let bat = app.join(curseen_ui::update::installer::BAT_NAME);
    if let Err(e) = std::fs::write(&bat, script.as_bytes()) {
        eprintln!("写更新脚本失败: {e}");
        std::process::exit(1);
    }
    println!("{}", bat.display());
}
