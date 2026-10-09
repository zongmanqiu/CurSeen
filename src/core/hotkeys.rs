//! 快捷键**匹配**：把"给人看的快捷键串"与"捕获到的按键事件"对上。
//!
//! 纯数据转换，无 UI、无平台调用，所以放在 `core`：`modules` 要用它（书写快捷键在
//! `DrawingModule::tick` 里匹配），而 `modules` 不能依赖 `app`。`app::hotkeys` 在此之上
//! 只补一个"读配置拿面板快捷键"的包装。
//!
//! # 为什么不用 `RegisterHotKey`
//!
//! windui 提供了 `App::hotkey`（Windows 上是 `RegisterHotKey`），但那是**系统级独占**：
//! 组合一旦注册，前台程序就再也收不到这个键。Python 1.0.0 走的是 `pynput` 低级钩子，
//! 按键**照常透传**，只是额外触发一次动作。本项目已有自己的 `WH_KEYBOARD_LL` 钩子，
//! 于是在同一通路上做匹配，保持"按键不被吞掉"这一行为。
//!
//! # 归一化比较
//!
//! 配置里的快捷键串是给人看的（`"Ctrl+Shift+D"`、`"Alt+G"`、`"1"`），用户可能写成
//! `"Shift + Ctrl + D"`。统一走 [`normalize`]：按 `+` 拆、去空白、转大写、**排序**后拼接。
//! 排序是关键——`"Shift+Ctrl+D"` 与 `"Ctrl+Shift+D"` 必须等价，否则用户改了顺序快捷键就"坏"了。

use super::config::DrawingHotkeys;
use super::events::{KeyCode, Modifiers};

/// 归一化快捷键串：`"Shift + Ctrl + D"` → `"CTRL+D+SHIFT"`。
pub fn normalize(spec: &str) -> String {
    let mut parts: Vec<String> = spec
        .split('+')
        .map(|p| p.trim().to_ascii_uppercase())
        .filter(|p| !p.is_empty())
        .collect();
    parts.sort();
    parts.join("+")
}

/// 把当前按键 + 修饰键状态归一化成与 [`normalize`] 同构的串。
///
/// 主键 token 取不到时返回空串——调用方据此判定"这次按键不可能是快捷键"。
pub fn pressed(key: KeyCode, mods: Modifiers) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(5);
    if mods.ctrl {
        parts.push("CTRL".to_string());
    }
    if mods.alt {
        parts.push("ALT".to_string());
    }
    if mods.shift {
        parts.push("SHIFT".to_string());
    }
    if mods.win {
        parts.push("WIN".to_string());
    }
    let token = key_token(key, mods.shift);
    if token.is_empty() {
        return String::new();
    }
    parts.push(token);
    parts.sort();
    parts.join("+")
}

/// 把 [`pressed`] 产出的全大归一化串转成给人看的格式（`"ALT+G"` → `"Alt+G"`）。
///
/// 匹配本身大小写无关（`normalize` 两边都转大写），但配置里写的是 `"Alt+G"` 这种，捕获到的
/// 快捷键若原样存 `"ALT+G"` 会跟其它默认快捷键的显示风格不一致。修饰键统一排到主键前面
/// （`Ctrl`/`Alt`/`Shift`/`Win`），于是 `Ctrl+Shift+D` 不会因 `pressed` 的排序而显示成
/// `"Ctrl+D+Shift"`。
pub fn pretty_hotkey(norm: &str) -> String {
    let mut has_ctrl = false;
    let mut has_alt = false;
    let mut has_shift = false;
    let mut has_win = false;
    let mut key = String::new();
    for t in norm.split('+') {
        match t {
            "CTRL" => has_ctrl = true,
            "ALT" => has_alt = true,
            "SHIFT" => has_shift = true,
            "WIN" => has_win = true,
            other => key = other.to_string(),
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if has_ctrl {
        parts.push("Ctrl".into());
    }
    if has_alt {
        parts.push("Alt".into());
    }
    if has_shift {
        parts.push("Shift".into());
    }
    if has_win {
        parts.push("Win".into());
    }
    if !key.is_empty() {
        parts.push(key);
    }
    parts.join("+")
}

/// 该按键与修饰键状态是否命中快捷键串。
pub fn matches(spec: &str, key: KeyCode, mods: Modifiers) -> bool {
    if is_modifier_key(key) {
        // 修饰键本身不是主键。否则按住 Ctrl 的那一下会被当成"键 Ctrl"去比对，
        // 而配置形如 "Ctrl"（只写修饰键）的快捷键是误配置，不该被激活。
        return false;
    }
    let target = normalize(spec);
    if target.is_empty() {
        return false;
    }
    let hit = pressed(key, mods);
    !hit.is_empty() && hit == target
}

/// 「同一个快捷键再按住 Ctrl」的规格串；该快捷键本身不可能有 Ctrl 变体时返回**空串**。
///
/// # 为什么需要它（而不是直接判 `mods.ctrl`）
///
/// [`matches`] 是**精确**比对（`Ctrl+Alt+S` 与配置里的 `Alt+S` 不相等），所以"按住 Ctrl
/// 让同一个快捷键多出一种语义"这件事，光看 `mods.ctrl` 是不够的 —— 那一位已经让原规格
/// 匹配不上了，必须先**把规格补上 CTRL** 再比。截图快捷键的 Ctrl 变体（框完不停顿、
/// 进编辑态，见 `SnapMode::ShotEdit`）走的就是这条路。
///
/// 已经含 Ctrl 的规格原样**作废**（返回空串）：说明这把键本来就要按着 Ctrl
/// （例如用户配的 `Ctrl+Alt+P`），再补一个只会得到 `Ctrl+Ctrl+Alt+P` —— 永远匹配不上。
/// 返回空串而不是原规格，是为了让调用方写 `hk_matches(&with_ctrl(spec), key, mods)`
/// 时天然得到 `false`（[`matches`] 对空规格一律返回 `false`），不必再判一次空。
pub fn with_ctrl(spec: &str) -> String {
    let n = normalize(spec);
    if n.is_empty() || n.split('+').any(|p| p == "CTRL") {
        return String::new();
    }
    format!("Ctrl+{spec}")
}

/// 该规格是否允许充当**全局**热键（系统任何前台程序下都生效的那类：截图、录制、
/// 面板、脚本、标注 toggle……全表见 `Runtime::handle_action_key`）。
///
/// 规则（按本项目的钩子路线调整）：
/// **无任何修饰键的裸键，主键必须落在功能键白名单内**——`F1`..`F24` / `PrtSc` /
/// `Pause` / `ScrollLock`。原因：本项目的热键走自家 `WH_KEYBOARD_LL` 匹配且**不吞键**，
/// 全局语义意味着这把键在用户打字、聊天的每一次按下都会触发一次动作——配一把裸
/// `A` 当截图键，等于每打一个 a 截一次图。功能键（F 键区、打印屏等）日常输入不会按到，
/// 才有资格裸配。
///
/// 与部分同类工具的差异：它们排除 `F12`（`RegisterHotKey` 把 F12 留给系统调试器），
/// 本项目不走 `RegisterHotKey`，F12 照常可配。
///
/// 空规格返回 `true`——空 = 停用，不存在误触发。
///
/// ★ **只约束全局语义**。标注态内的工具切换键（画笔 `1`..`9`/`0`、橡皮等）不在此列：
/// 它们在 `DrawingModule::handle_key` 里匹配、且仅当标注开着才生效（`is_drawing()`
/// 门控），打字时根本不会走到，裸键正是设计意图（出厂默认就是 `1`..`9`）。所以
/// 这道判定的调用点必须在「全局消费方」（捕获校验 / 配置加载清洗），**不能**塞进
/// [`matches`]——那会把工具键一并砍掉。
pub fn global_safe(spec: &str) -> bool {
    let n = normalize(spec);
    if n.is_empty() {
        return true;
    }
    // 归一化后带任一修饰键即放行（修饰键排过序，逐段比即可）。
    if n.split('+')
        .any(|p| matches!(p, "CTRL" | "ALT" | "SHIFT" | "WIN"))
    {
        return true;
    }
    bare_token_whitelisted(&n)
}

/// 裸键主键 token 是否在全局白名单内（[`global_safe`] 的私有下半场）。
fn bare_token_whitelisted(token: &str) -> bool {
    if let Some(num) = token.strip_prefix('F') {
        return num.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n));
    }
    matches!(token, "PRTSC" | "PAUSE" | "SCROLLLOCK")
}

/// 修饰键（左右都算）。这些键按下时不触发快捷键匹配。
pub fn is_modifier_key(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::LShift
            | KeyCode::RShift
            | KeyCode::LCtrl
            | KeyCode::RCtrl
            | KeyCode::LAlt
            | KeyCode::RAlt
            | KeyCode::LWin
            | KeyCode::RWin
            | KeyCode::Menu
    )
}

/// 主键 → 归一化 token（全大写、无空白）。
///
/// 字母走 `KeyCode::Char`，但**移位符号要还原成基准键**：钩子把 `Shift+1` 映射成
/// `Char('!')`（见 `vk::from_vk`），而配置里写的是 `"1"`。不还原的话 `Ctrl+Shift+1`
/// 这类快捷键永远匹配不上。
///
/// 还原必须**带上 `shift` 判断**：`Char('!')` 只在 Shift 按下时才出现，若无条件折叠，
/// 就会把"按了 Shift 的 1"当成"按了 1"，让配置 `"1"` 被 `Shift+1` 误触发。
fn key_token(key: KeyCode, shift: bool) -> String {
    match key {
        KeyCode::Char(c) => match (shift, shifted_base(c)) {
            (true, Some(base)) => base.to_string(),
            _ => c.to_ascii_uppercase().to_string(),
        },
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::Esc => "ESC".to_string(),
        KeyCode::Tab => "TAB".to_string(),
        KeyCode::Space => "SPACE".to_string(),
        KeyCode::Backspace => "BACKSPACE".to_string(),
        KeyCode::Enter => "ENTER".to_string(),
        KeyCode::Delete => "DELETE".to_string(),
        KeyCode::Insert => "INSERT".to_string(),
        KeyCode::Home => "HOME".to_string(),
        KeyCode::End => "END".to_string(),
        KeyCode::PageUp => "PAGEUP".to_string(),
        KeyCode::PageDown => "PAGEDOWN".to_string(),
        KeyCode::Up => "UP".to_string(),
        KeyCode::Down => "DOWN".to_string(),
        KeyCode::Left => "LEFT".to_string(),
        KeyCode::Right => "RIGHT".to_string(),
        KeyCode::Pause => "PAUSE".to_string(),
        KeyCode::PrtSc => "PRTSC".to_string(),
        KeyCode::CapsLock => "CAPSLOCK".to_string(),
        KeyCode::NumLock => "NUMLOCK".to_string(),
        KeyCode::ScrollLock => "SCROLLLOCK".to_string(),
        KeyCode::Num(n) => format!("NUM{n}"),
        KeyCode::NumAdd => "NUMADD".to_string(),
        KeyCode::NumSub => "NUMSUB".to_string(),
        KeyCode::NumMul => "NUMMUL".to_string(),
        KeyCode::NumDiv => "NUMDIV".to_string(),
        KeyCode::NumDot => "NUMDOT".to_string(),
        KeyCode::NumEnter => "NUMENTER".to_string(),
        // 修饰键（由 `is_modifier_key` 提前挡掉）、媒体键与未映射键：无可用 token。
        _ => String::new(),
    }
}

/// 移位符号 → 基准键。覆盖 `vk::from_vk` 里所有"同一物理键、Shift 下变符号"的映射
/// （主键盘数字与 OEM 标点）。唯一的例外是 `+`：它在快捷键串里是分隔符，无法写成基准键。
fn shifted_base(c: char) -> Option<char> {
    Some(match c {
        ')' => '0',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ':' => ';',
        '_' => '-',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}

/// 书写快捷键表里的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingAction {
    ToggleScreen,
    ToggleWhiteboard,
    Pen1,
    Pen2,
    Pen3,
    Eraser,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    Mosaic,
    /// 序号贴纸（2026-10-01）：切到 `Tool::Stamp`，点一下盖一个带编号的圈。
    Stamp,
}

/// 屏幕标注「有快捷键的工具」的**唯一全序**（= 工具栏前三组扁平展开后的按钮序）。
///
/// ★ 三处界面都按这张表排：工具栏按钮序、屏幕页「快捷键」子卡的行序、设置页「快捷键」
///   集合表的行序。顺序只写一份、其余两处照抄，是 2026-10-01 用户口径（"快捷键顺序要和
///   工具栏一样"）落地的做法——三处各写一份迟早走偏，而走偏的表现是"按 4 键切到马赛克
///   而不是橡皮"这种最难查的错：每一处都单独看是"排好了"的。
///
/// 前两位（`ToggleScreen` / `ToggleWhiteboard`）是开关、不是工具，故不在此表内——
/// 它们永远排在工具前面（开关是进这层的方式，不该挤占工具的位置）。
pub const DRAW_TOOL_ORDER: [DrawingAction; 11] = [
    DrawingAction::Pen1,
    DrawingAction::Pen2,
    DrawingAction::Pen3,
    DrawingAction::Line,
    DrawingAction::Arrow,
    DrawingAction::Rect,
    DrawingAction::Ellipse,
    DrawingAction::Text,
    DrawingAction::Stamp,
    DrawingAction::Mosaic,
    DrawingAction::Eraser,
];

/// 在书写快捷键表里找命中项。返回 `None` 表示这次按键不是书写快捷键。
pub fn match_drawing(
    key: KeyCode,
    mods: Modifiers,
    hotkeys: &DrawingHotkeys,
) -> Option<DrawingAction> {
    // ★ 表的行序与 [`DRAW_TOOL_ORDER`] 一致（工具在前、两把开关在最前）——出厂值互不相
    //   同，顺序只决定"同一把键绑到两个动作时谁赢"，但保持同一份顺序省得两头查。
    let table: [(&str, DrawingAction); 13] = [
        (&hotkeys.draw_hk_screen, DrawingAction::ToggleScreen),
        (&hotkeys.draw_hk_white, DrawingAction::ToggleWhiteboard),
        (&hotkeys.draw_hk_pen1, DrawingAction::Pen1),
        (&hotkeys.draw_hk_pen2, DrawingAction::Pen2),
        (&hotkeys.draw_hk_pen3, DrawingAction::Pen3),
        (&hotkeys.draw_hk_line, DrawingAction::Line),
        (&hotkeys.draw_hk_arrow, DrawingAction::Arrow),
        (&hotkeys.draw_hk_rect, DrawingAction::Rect),
        (&hotkeys.draw_hk_ellipse, DrawingAction::Ellipse),
        (&hotkeys.draw_hk_text, DrawingAction::Text),
        (&hotkeys.draw_hk_stamp, DrawingAction::Stamp),
        (&hotkeys.draw_hk_mosaic, DrawingAction::Mosaic),
        (&hotkeys.draw_hk_eraser, DrawingAction::Eraser),
    ];
    table
        .into_iter()
        .find(|(spec, _)| matches(spec, key, mods))
        .map(|(_, action)| action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool, win: bool) -> Modifiers {
        Modifiers {
            ctrl,
            alt,
            shift,
            win,
        }
    }

    const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };

    /// 工具序表必须覆盖**每一个工具类** `DrawingAction` 恰好一次（两把开关有意不在内——
    /// 它们是进这层的方式，挤占工具位置会让"按 1/2/3 切笔"的口诀失效）。
    #[test]
    fn draw_tool_order_covers_every_tool_action_exactly_once() {
        const TOOLS: [DrawingAction; 11] = [
            DrawingAction::Pen1,
            DrawingAction::Pen2,
            DrawingAction::Pen3,
            DrawingAction::Line,
            DrawingAction::Arrow,
            DrawingAction::Rect,
            DrawingAction::Ellipse,
            DrawingAction::Text,
            DrawingAction::Stamp,
            DrawingAction::Mosaic,
            DrawingAction::Eraser,
        ];
        assert_eq!(
            super::DRAW_TOOL_ORDER.len(),
            TOOLS.len(),
            "工具序表长度与工具类动作数不一致"
        );
        for want in TOOLS {
            let hits = super::DRAW_TOOL_ORDER
                .iter()
                .filter(|a| **a == want)
                .count();
            assert_eq!(hits, 1, "{want:?} 在工具序表里出现 {hits} 次（应为 1）");
        }
    }

    #[test]
    fn normalize_ignores_order_space_and_case() {
        assert_eq!(normalize("Ctrl+Shift+D"), "CTRL+D+SHIFT");
        assert_eq!(normalize("shift + ctrl + d"), "CTRL+D+SHIFT");
        assert_eq!(normalize("Alt+G"), "ALT+G");
        assert_eq!(normalize("1"), "1");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn panel_hotkey_matches_default() {
        assert!(matches(
            "Ctrl+Shift+D",
            KeyCode::Char('d'),
            mods(true, false, true, false)
        ));
        // 顺序不同也要命中
        assert!(matches(
            "Shift+Ctrl+D",
            KeyCode::Char('D'),
            mods(true, false, true, false)
        ));
        // 少一个修饰键不命中
        assert!(!matches(
            "Ctrl+Shift+D",
            KeyCode::Char('d'),
            mods(true, false, false, false)
        ));
        // 多一个修饰键不命中
        assert!(!matches(
            "Ctrl+Shift+D",
            KeyCode::Char('d'),
            mods(true, true, true, false)
        ));
    }

    /// ★ `with_ctrl` 的三条语义：补一个 Ctrl、已含 Ctrl 则作废、空规格作废。
    ///
    /// 最后一条断言是**分辨力**所在：如果 `with_ctrl` 对"已含 Ctrl 的规格"原样返回，
    /// 那么一把 `Ctrl+Alt+P` 的键就会在"按住 Ctrl"时同时命中两条路（它自己 + 它的
    /// Ctrl 变体），谁先谁后取决于代码顺序 —— 那是靠巧合成立的实现。
    #[test]
    fn with_ctrl_builds_a_ctrl_variant_and_refuses_when_ctrl_is_already_there() {
        // 补出来之后必须能被"按住 Ctrl 的同一个主键"命中。
        assert_eq!(with_ctrl("Alt+S"), "Ctrl+Alt+S");
        assert!(matches(
            &with_ctrl("Alt+S"),
            KeyCode::Char('s'),
            mods(true, true, false, false)
        ));
        // 不按住 Ctrl 时，这个变体不该命中。
        assert!(!matches(
            &with_ctrl("Alt+S"),
            KeyCode::Char('s'),
            mods(false, true, false, false)
        ));
        // 原规格在按住 Ctrl 时也**不该**命中（精确比对）—— 两条路互不重叠。
        assert!(!matches(
            "Alt+S",
            KeyCode::Char('s'),
            mods(true, true, false, false)
        ));

        // 已含 Ctrl / 空串 → 没有 Ctrl 变体。
        assert_eq!(with_ctrl("Ctrl+Alt+P"), "");
        assert_eq!(with_ctrl(""), "");
        assert_eq!(with_ctrl("   "), "");
        assert!(
            !matches("", KeyCode::Char('p'), mods(true, true, false, false)),
            "空规格必须永不命中（这是调用方不必再判空的前提）"
        );
    }

    #[test]
    fn single_key_hotkey() {
        assert!(matches("1", KeyCode::Char('1'), NONE));
        assert!(!matches("1", KeyCode::Char('2'), NONE));
        // 配置空串永不命中（= 未启用）
        assert!(!matches("", KeyCode::Char('1'), NONE));
        assert!(!matches("   ", KeyCode::Char('1'), NONE));
    }

    #[test]
    fn shifted_symbol_maps_back_to_base_key() {
        // 钩子把 Shift+1 变成 Char('!')，配置里写的是 "1"。
        assert!(matches(
            "Ctrl+Shift+1",
            KeyCode::Char('!'),
            mods(true, false, true, false)
        ));
        // Shift+1 也只该命中 "Shift+1"，不该命中裸 "1"
        assert!(matches(
            "Shift+1",
            KeyCode::Char('!'),
            mods(false, false, true, false)
        ));
        // 不带 Shift 时 Char('!') 不该命中 "1"（那是另一个物理组合）
        assert!(!matches("1", KeyCode::Char('!'), NONE));
        // OEM 标点同理：Shift+= 是 Char('+')，基准键是 '='
        assert!(matches(
            "Ctrl+=",
            KeyCode::Char('='),
            mods(true, false, false, false)
        ));
        assert!(matches(
            "Ctrl+Shift+;",
            KeyCode::Char(':'),
            mods(true, false, true, false)
        ));
    }

    #[test]
    fn modifier_press_is_not_a_hotkey() {
        // 只写修饰键的"快捷键"是误配置；按住 Ctrl 的那一下不能把它激活。
        assert!(!matches(
            "Ctrl",
            KeyCode::LCtrl,
            mods(true, false, false, false)
        ));
        assert!(is_modifier_key(KeyCode::RWin));
        assert!(!is_modifier_key(KeyCode::Char('a')));
    }

    #[test]
    fn alt_letter_hotkeys() {
        assert!(matches(
            "Alt+G",
            KeyCode::Char('g'),
            mods(false, true, false, false)
        ));
        assert!(matches(
            "Alt+W",
            KeyCode::Char('W'),
            mods(false, true, false, false)
        ));
        assert!(!matches(
            "Alt+G",
            KeyCode::Char('w'),
            mods(false, true, false, false)
        ));
    }

    /// ★ 全局热键的裸键白名单（2026-10-01）：带修饰键放行；裸键只认 F1..F24 /
    ///   PrtSc / Pause / ScrollLock；空串 = 停用恒真。
    ///
    /// 分辨力集中在「裸字母 / 裸数字 / 裸 ESC 必须拒」——那正是"打字时每次按下都触发"
    /// 的事故形态。数字被拒也再次钉住了工具键走 [`global_safe`] 的**调用点约束**：
    /// `1`..`9` 是标注态工具键，判它必须经过 [`HotkeyField::allows_bare_key`] 那一层，
    /// 不能无脑把所有裸键塞进这里判。
    #[test]
    fn global_safe_rejects_bare_typing_keys_but_allows_fkeys_and_modifiers() {
        // 裸键白名单内：功能键区。
        assert!(global_safe("F2"));
        assert!(global_safe("F12"));
        assert!(global_safe("F24"));
        assert!(global_safe("PrtSc"));
        assert!(global_safe("Pause"));
        assert!(global_safe("ScrollLock"));
        // 裸键白名单外：日常输入会按到的键，一律拒。
        assert!(!global_safe("1"));
        assert!(!global_safe("A"));
        assert!(!global_safe("Esc"));
        assert!(!global_safe("Space"));
        assert!(!global_safe("Backspace"));
        // 大小写与空白走 normalize 同一口径。
        assert!(global_safe("f2"));
        assert!(!global_safe("  a  "));
        // 带任一修饰键即放行（主键是什么都无所谓）。
        assert!(global_safe("Ctrl+1"));
        assert!(global_safe("Alt+G"));
        assert!(global_safe("Shift+A"));
        assert!(global_safe("Win+Space"));
        // 空规格 = 停用，恒真。
        assert!(global_safe(""));
        assert!(global_safe("   "));
        // 只写修饰键的误配置：带修饰放行（匹配层本来也匹配不上，见 modifier_press 测试）。
        assert!(global_safe("Ctrl"));
    }

    #[test]
    fn unmapped_key_never_matches() {
        assert!(key_token(KeyCode::Unknown(65535), false).is_empty());
        assert!(!matches(
            "Ctrl+A",
            KeyCode::Unknown(65535),
            mods(true, false, false, false)
        ));
    }

    #[test]
    fn drawing_table_resolves_each_slot() {
        let hk = DrawingHotkeys {
            draw_hk_screen: "Alt+G".into(),
            draw_hk_white: "Alt+W".into(),
            draw_hk_pen1: "1".into(),
            draw_hk_pen2: "2".into(),
            draw_hk_pen3: "3".into(),
            draw_hk_eraser: "4".into(),
            draw_hk_line: "5".into(),
            draw_hk_arrow: "6".into(),
            draw_hk_rect: "7".into(),
            draw_hk_ellipse: "8".into(),
            draw_hk_text: "9".into(),
            draw_hk_mosaic: "0".into(),
            draw_hk_stamp: String::new(),
        };
        assert_eq!(
            match_drawing(KeyCode::Char('g'), mods(false, true, false, false), &hk),
            Some(DrawingAction::ToggleScreen)
        );
        assert_eq!(
            match_drawing(KeyCode::Char('w'), mods(false, true, false, false), &hk),
            Some(DrawingAction::ToggleWhiteboard)
        );
        assert_eq!(
            match_drawing(KeyCode::Char('4'), NONE, &hk),
            Some(DrawingAction::Eraser)
        );
        assert_eq!(
            match_drawing(KeyCode::Char('0'), NONE, &hk),
            Some(DrawingAction::Mosaic)
        );
        // 修饰键参与时不再是单键快捷键
        assert_eq!(
            match_drawing(KeyCode::Char('4'), mods(true, false, false, false), &hk),
            None
        );
    }

    /// ★ 「序号」快捷键（2026-10-01）：**出厂留空 = 停用**，填了才切到序号工具。
    ///
    /// 这条门的分辨力全在最后那个 `None`：若哪天 `match_drawing` 的表忘了补 `Stamp` 槽
    /// （槽数对不上会直接编译错，但"补了槽没补字段"是静默的），用户配完这把键会"点了没反应"。
    #[test]
    fn the_stamp_hotkey_is_empty_by_default_and_only_matches_when_set() {
        let mut hk = DrawingHotkeys {
            ..Default::default()
        };
        assert!(
            hk.draw_hk_stamp.is_empty(),
            "序号出厂必须是空串（用户自己在面板里配）"
        );
        assert_eq!(
            match_drawing(KeyCode::Char('n'), mods(false, true, false, false), &hk),
            None
        );
        hk.draw_hk_stamp = "Alt+N".into();
        assert_eq!(
            match_drawing(KeyCode::Char('n'), mods(false, true, false, false), &hk),
            Some(DrawingAction::Stamp)
        );
    }
}
