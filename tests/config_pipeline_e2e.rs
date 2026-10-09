//! 无头全链路验证：真实 build/config.json → 模块 → 工具栏点击路由 → 落笔 → 渲染。
//!
//! 背景：E2E 截图取证（BitBlt + 像素差分）会被同机的实时桌面活动污染（用户正在用机）。
//! 本测试把同样的操作路径收敛为进程内精确断言：
//!   1. 用户真实配置（Ctrl+Alt+G 进入、三支笔的颜色/粗细/线型）能被解析并生效；
//!   2. 工具栏「画笔3」按钮的几何中心与 E2E 点击坐标一致，且点击后 active_pen 切换；
//!   3. 画笔3（#00FF00FF dashed）落笔后，渲染出的 DrawList 里存在**不透明绿色**墨迹指令。

use curseen_ui::core::config::{Config, DrawingHotkeys};
use curseen_ui::core::draw::{DrawCmd, DrawList, Paint};
use curseen_ui::core::events::{InputEvent, KeyCode, Modifiers, MouseButton};
use curseen_ui::core::tick::Tick;
use curseen_ui::modules::drawing::controller::DrawRequest;
use curseen_ui::modules::drawing::toolbar::{self, ToolbarAction};
use curseen_ui::modules::drawing::DrawingModule;
use curseen_ui::modules::Module;
use curseen_ui::platform::display::ScreenRect;

const CONFIG_JSON: &str = r"E:\tmp\AI.Agent\CurSeen\build\config.json";

fn tick16() -> Tick {
    Tick {
        raw_ms: 16.0,
        smooth_ms: 16.0,
        dt_ms: 16.0,
        elapsed_ms: 16.0,
    }
}

fn key_down(key: KeyCode, mods: Modifiers) -> Option<InputEvent> {
    Some(InputEvent::KeyDown {
        key,
        mods,
        repeated: false,
        time_ms: 0,
    })
}

fn mouse_down(x: i32, y: i32) -> Option<InputEvent> {
    Some(InputEvent::MouseDown {
        button: MouseButton::Left,
        x,
        y,
        time_ms: 0,
    })
}

fn mouse_move(x: i32, y: i32) -> Option<InputEvent> {
    Some(InputEvent::MouseMove { x, y, time_ms: 0 })
}

fn mouse_up(x: i32, y: i32) -> Option<InputEvent> {
    Some(InputEvent::MouseUp {
        button: MouseButton::Left,
        x,
        y,
        time_ms: 0,
    })
}

/// 按配置里的屏幕标注快捷键（`drawing.hotkeys.draw_hk_screen`）构造一条按键事件。
///
/// ★ 2026-09-18 实测：本文件此前写死了 `Ctrl+Alt+G`，而**出厂默认就是 `Alt+G`**
///   （`core::config::default_hk_screen`）—— 那个 `Ctrl+Alt` 按法不存在于任何默认配置里，
///   于是这三条用例从写下那天起就一直红着，且没人跑 `--test` 就没发现。
///   2026-09-27 出厂口径又变：`draw_hk_screen` / `draw_hk_white` 出厂留空，
///   `real_config` 里显式补了 `Alt+G`（reason 见该函数）。
///   既然本文件叫 `real_config_pipeline`，就该**用配置里的快捷键**去驱动。
///   `core::hotkeys` 只提供正向比对（`matches`）、没有字符串→按键的解析，所以这里
///   按它自己的语法（`+` 分隔、末段是主键）反向切一次，并**当场自检** —— 构造错了
///   立刻红在自检上，而不是静默地"按了没反应"。
fn screen_hotkey_event(cfg: &Config) -> Option<InputEvent> {
    let spec = cfg.drawing.hotkeys.draw_hk_screen.clone();
    let mut parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    let main = parts.pop()?;
    let mut mods = Modifiers::default();
    for p in parts {
        match p.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "win" | "super" | "meta" => mods.win = true,
            other => panic!("不认识的快捷键修饰段 `{other}`（spec={spec}）"),
        }
    }
    let key = KeyCode::Char(main.chars().next()?);
    assert!(
        curseen_ui::core::hotkeys::matches(&spec, key, mods),
        "构造的按键与 core::hotkeys 的语法对不上：spec={spec}、key={key:?}、mods={mods:?}"
    );
    Some(InputEvent::KeyDown {
        key,
        mods,
        repeated: false,
        time_ms: 0,
    })
}

/// 用户真实配置。**只取"用户调过的那些数据"**（笔的颜色/粗细/线型 —— 本文件真正的被测对象），
/// 不取"用户此刻的开关状态与快捷键" —— 后者随日常使用随时变。
///
/// ★ 2026-09-18 实测：这两样各让本文件全红过一次，而两次都与被测对象无关 ——
///   ① 「屏幕书写」被关掉（`drawing.enabled = false`）→ 功能门控拦下，Ctrl+Alt+G 进不了标注；
///   ② 快捷键从出厂的 `Ctrl+Alt+G` 改成 `Alt+G` → 本文件写死的那种按法匹配不上。
/// 门禁不该被用户的日常设置左右，所以这里在**内存里**把开关与快捷键恢复成出厂值；
/// 真实配置文件一个字节都不改。快捷键字符串本身的解析/比对由 `core::hotkeys` 的单测覆盖，
/// 本文件不重复承担（它只负责"配置 → 模块 → 落笔 → 墨色"这条链路）。
fn real_config() -> Config {
    // ★ 2026-09-27 实测：`build/config.json` 是**运行期正本**，却不是版本库里的文件
    //   （便携版约定：用户第一次关面板才落盘）。它被误删/尚未落盘时，下面三条用例会
    //   以"读取失败"全红 —— 这种失败与被测对象（配置 → 模块 → 落笔 → 墨色）无关，
    //   只会让真正的回归信号被淹没。所以：**文件在就测真配置，不在就按工厂默认跑同一条
    //   链路**，两种输入下断言完全一致。
    let text = match std::fs::read_to_string(CONFIG_JSON) {
        Ok(t) => {
            eprintln!("[config_pipeline] 采用真实配置 {CONFIG_JSON}");
            t
        }
        Err(e) => {
            eprintln!("[config_pipeline] {CONFIG_JSON} 不存在（{e}）→ 退回工厂默认配置");
            serde_json::to_string(&Config::default()).expect("序列化工厂默认配置")
        }
    };
    let mut cfg: Config = serde_json::from_str(&text).expect("解析配置文件为 Config");
    cfg.drawing.enabled = true;
    // ★ 2026-09-27 补：这两把**入口键出厂已留空**（功能靠卡片总开关），而下面三条用例
    //   都要"先进标注再落笔"——照默认值跑会退化成"按了下没反应、测了个寂寞"。故显式配
    //   一把（`Alt+G`），让被测对象回到"配置 → 模块 → 落笔 → 墨色"本身。用 `Drawing-
    //   Hotkeys::default()` 覆盖其余字段（画笔 1..9 等仍是出厂单键）。
    cfg.drawing.hotkeys = DrawingHotkeys::default();
    cfg.drawing.hotkeys.draw_hk_screen = "Alt+G".into();
    cfg
}

/// 工具栏几何：E2E 点击的 (793,1025) 必须正是「画笔3」按钮中心。
///
/// ★ 第三参是**截图编辑态的选区**（`Option<Rect>`）：编辑态要求工具栏跟随选框（用户口径
///   "画笔工具栏不能总在屏幕中央下方，得跟着截图区域显示在附近"），非编辑态传 `None` =
///   沿用"贴屏幕上/下边缘"的老布局。本用例验的正是后者，所以给 `None`。
#[test]
fn toolbar_pen3_button_center_matches_the_e2e_click_point() {
    let l = toolbar::layout(
        ScreenRect {
            x: 0,
            y: 0,
            w: 1920,
            h: 1080,
        },
        1.0,
        None,
        false,
    );
    let (_, r) = l
        .cells
        .iter()
        .find(|(a, _)| *a == ToolbarAction::Pen(2))
        .expect("应有画笔3按钮");
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    assert_eq!(
        cx, 736.0,
        "画笔3中心 x 应为 736（16 按钮布局：新增马赛克按钮后条形整体左移 19px）"
    );
    assert_eq!(cy, 1025.0, "画笔3中心 y 应为 1025");
}

/// 全链路：真实配置 → 快捷键进入 → 点击工具栏画笔3 → 落笔 → 渲染出绿色墨迹。
#[test]
fn real_config_pipeline_produces_green_ink_for_pen3() {
    let cfg = real_config();
    // 功能开关由 `real_config` 统一置位（见其注释），这里不再单独断言。
    let mut m = DrawingModule::new(&cfg);
    m.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    });
    m.set_ui_scale(1.0);

    // ① Ctrl+Alt+G 进入标注（用户配置的 draw_hk_screen）
    m.tick(&tick16(), &cfg, &screen_hotkey_event(&cfg));
    assert!(m.controller().is_drawing(), "Ctrl+Alt+G 应进入标注模式");

    // ② 点击工具栏「画笔3」（16 按钮布局下的 pen3 中心：新增马赛克按钮后左移 19px）
    m.tick(&tick16(), &cfg, &mouse_down(736, 1025));
    m.tick(&tick16(), &cfg, &mouse_up(736, 1025));
    assert_eq!(
        m.controller().active_pen_index(),
        2,
        "点击工具栏画笔3应切到第 3 支笔"
    );

    // ③ 落一笔（E2E 同款路径 1150,700 → 1550,880）
    m.tick(&tick16(), &cfg, &mouse_down(1150, 700));
    for (x, y) in [
        (1230, 740),
        (1310, 780),
        (1390, 810),
        (1470, 850),
        (1550, 880),
    ] {
        m.tick(&tick16(), &cfg, &mouse_move(x, y));
    }
    m.tick(&tick16(), &cfg, &mouse_up(1550, 880));
    assert_eq!(
        m.controller().history().stroke_count(),
        1,
        "落笔应产生一条笔迹"
    );

    // ④ 渲染 → 必须存在不透明绿色（#00FF00FF）的描边指令
    let mut list = DrawList::new();
    m.controller().draw(&mut list);
    let green_strokes = list
        .cmds()
        .iter()
        .filter(|c| {
            matches!(
                c,
                DrawCmd::Stroke {
                    paint: Paint::Solid(col, _),
                    ..
                } if col.0 == 0xFF00FF00
            )
        })
        .count();
    assert!(
        green_strokes > 0,
        "画笔3 的墨迹应以 #00FF00FF 渲染，实际绿色描边指令 0 条（共 {} 条指令）",
        list.cmds().len()
    );
}

/// 画笔1（黑色 dotted，宽 333→clamp 100）同样要能渲染出黑色墨迹。
#[test]
fn real_config_pipeline_produces_black_ink_for_pen1() {
    let cfg = real_config();
    let mut m = DrawingModule::new(&cfg);
    m.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    });
    m.set_ui_scale(1.0);

    m.tick(&tick16(), &cfg, &screen_hotkey_event(&cfg));
    assert!(m.controller().is_drawing());

    // 直接落笔（默认就是画笔1，不点工具栏）
    m.tick(&tick16(), &cfg, &mouse_down(300, 300));
    for (x, y) in [(420, 380), (540, 450), (660, 520), (780, 580), (920, 640)] {
        m.tick(&tick16(), &cfg, &mouse_move(x, y));
    }
    m.tick(&tick16(), &cfg, &mouse_up(920, 640));
    assert_eq!(m.controller().history().stroke_count(), 1);

    let mut list = DrawList::new();
    m.controller().draw(&mut list);
    let black_strokes = list
        .cmds()
        .iter()
        .filter(|c| {
            matches!(
                c,
                DrawCmd::Stroke {
                    paint: Paint::Solid(col, _),
                    ..
                } if col.0 == 0xFF000000
            )
        })
        .count();
    assert!(black_strokes > 0, "画笔1 的墨迹应以 #000000FF 渲染");

    // 宽度 clamp：配置里写了 333，实际必须收敛到 100（防止一渲染就是 1/3 屏宽）
    let max_w = list
        .cmds()
        .iter()
        .filter_map(|c| match c {
            DrawCmd::Stroke { width, .. } => Some(*width),
            _ => None,
        })
        .fold(0.0_f32, |acc, w| acc.max(w));
    assert!(
        max_w <= 100.0 + f32::EPSILON,
        "配置宽 333 应被 clamp 到 100，实际最大渲染宽度 {max_w}"
    );
}

/// Esc 退出 + 再次进入（验收：可重复进入且功能正常）。
#[test]
fn esc_exits_and_reenter_works_with_real_config() {
    let cfg = real_config();
    let mut m = DrawingModule::new(&cfg);
    m.set_screen(ScreenRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    });
    m.set_ui_scale(1.0);

    m.tick(&tick16(), &cfg, &screen_hotkey_event(&cfg));
    assert!(m.controller().is_drawing());
    m.tick(
        &tick16(),
        &cfg,
        &key_down(KeyCode::Esc, Modifiers::default()),
    );
    // ★ 2026-09-23 起 Esc 只**报意图**（`DrawRequest::AnnotateExit`）—— 真正停、合成留档
    //   与释放冻结帧都在 `Runtime` 那一侧（那里才拿得到光栅器与 `platform::screenshot`）。
    //   所以这一层能断的是"意图报出来了"，顺手 `stop()` 代替 Runtime 收尾，
    //   好继续验下一条真正关心的事：退出之后还能再进来。
    assert_eq!(
        m.take_request(),
        Some(DrawRequest::AnnotateExit),
        "Esc 应报出退出标注的意图"
    );
    assert!(
        m.controller().is_drawing(),
        "报意图不等于已经停：产物还没合成"
    );
    m.stop();
    assert!(!m.controller().is_drawing(), "Esc 应退出标注");

    // 去抖 0.5s：跨过去再进
    let mut t = tick16();
    t.elapsed_ms = 2000.0;
    m.tick(&t, &cfg, &screen_hotkey_event(&cfg));
    assert!(m.controller().is_drawing(), "退出后应能再次进入");
}
