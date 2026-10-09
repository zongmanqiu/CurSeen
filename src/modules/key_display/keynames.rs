//! 击键回显的键名映射表（INI 覆盖层）：`assets/KeyDisplay/keynames.ini`。
//!
//! 用户改 ini 就能改每个键位在弹窗里显示的文字。本模块负责三件事：
//! ① **VK ↔ 规范键名**对照表——ini 的键名用规范 ID 而不是显示名，因为显示名有重名
//!    （主键盘 `0` 与小键盘 `0`、OEM `-` 与小键盘 `-`、"←" 同时是 Backspace 和方向键），
//!    按显示名配置会有歧义；
//! ② 一个 ~50 行的 ini 解析器（`键名 = 文本`，行首 `;`/`#` 注释，`[节]` 忽略；
//!    这个解析量不值得引第三方 ini 库——克制引库是项目公约）；
//! ③ **按键频率**的 mtime 热重载（改 ini 保存后按一次键即生效，无需重启；不占每帧开销）。
//!
//! 语义：
//! - ini 没写的键 → 内置默认名（`keymap` 的显示名）；
//! - 值留空 → 这一条覆盖作废（回到内置默认）；
//! - 未知键名 → 留在表里但永远查不到（键名区分大小写，拼错等于没写）；
//! - 文件不存在 / 解析失败 → 维持上一份表（出厂即内置默认，功能不受影响）。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::events::MouseButton;

/// 主键盘数字（VK 0x30-0x39）。
const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];
/// 字母（VK 0x41-0x5A）。
const LETTERS: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];
/// 小键盘数字（VK 0x60-0x69）。
const NUM_DIGITS: [&str; 10] = [
    "Num0", "Num1", "Num2", "Num3", "Num4", "Num5", "Num6", "Num7", "Num8", "Num9",
];
/// F 键（VK 0x70-0x7B）。
const F_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];

/// 鼠标事件 → (ini 规范 ID, 内置显示名)。
///
/// 与键盘同一条 ini（`MouseLeft = 左键` 等 9 条）；侧键按硬件名"侧键1/侧键2"
/// 而不是"后退/前进"——XButton1/2 在不同应用里绑定不同（浏览器是后退/前进，
/// 游戏里常是近战/道具），显示硬件名不撒谎。
pub const MOUSE_IDENTS: [(&str, &str); 9] = [
    ("MouseLeft", "左键"),
    ("MouseRight", "右键"),
    ("MouseMiddle", "中键"),
    ("MouseX1", "侧键1"),
    ("MouseX2", "侧键2"),
    ("WheelUp", "滚轮上"),
    ("WheelDown", "滚轮下"),
    ("WheelLeft", "滚轮左"),
    ("WheelRight", "滚轮右"),
];

/// 鼠标按键 → (规范 ID, 内置名)。
pub fn mouse_ident(button: MouseButton) -> (&'static str, &'static str) {
    match button {
        MouseButton::Left => MOUSE_IDENTS[0],
        MouseButton::Right => MOUSE_IDENTS[1],
        MouseButton::Middle => MOUSE_IDENTS[2],
        MouseButton::X1 => MOUSE_IDENTS[3],
        MouseButton::X2 => MOUSE_IDENTS[4],
    }
}

/// 滚轮 → (规范 ID, 内置名)。`horizontal` = 横向倾斜；`positive` = delta > 0
/// （纵向=上滚、横向=右滚，与 Windows 滚轮同向）。
pub fn wheel_ident(horizontal: bool, positive: bool) -> (&'static str, &'static str) {
    match (horizontal, positive) {
        (false, true) => MOUSE_IDENTS[5],
        (false, false) => MOUSE_IDENTS[6],
        (true, true) => MOUSE_IDENTS[8],
        (true, false) => MOUSE_IDENTS[7],
    }
}

/// VK → ini 规范键名。
///
/// **必须覆盖 `keymap::lookup_vk` 认识的每一个 VK**（`canonical_covers_keymap` 钉住）；
/// 出厂 ini 也按这张表逐项生成（`shipped_ini_covers_every_key` 钉住——两者一起保证
/// "按键对应定义"不缺项、不漂移）。
pub fn canonical_name(vk: u16) -> Option<&'static str> {
    match vk {
        0x08 => Some("Backspace"),
        0x09 => Some("Tab"),
        0x0D => Some("Enter"),
        0x10 => Some("Shift"),
        0x11 => Some("Ctrl"),
        0x12 => Some("Alt"),
        0x13 => Some("Pause"),
        0x14 => Some("CapsLock"),
        0x1B => Some("Esc"),
        0x20 => Some("Space"),
        0x21 => Some("PgUp"),
        0x22 => Some("PgDn"),
        0x23 => Some("End"),
        0x24 => Some("Home"),
        0x25 => Some("Left"),
        0x26 => Some("Up"),
        0x27 => Some("Right"),
        0x28 => Some("Down"),
        0x2C => Some("PrtSc"),
        0x2D => Some("Ins"),
        0x2E => Some("Del"),
        0x30..=0x39 => Some(DIGITS[(vk - 0x30) as usize]),
        0x41..=0x5A => Some(LETTERS[(vk - 0x41) as usize]),
        // 左右 Win 共用一个 VK 条目（keymap 0x5B|0x5C 同名）。
        0x5B | 0x5C => Some("Win"),
        0x5D => Some("Menu"),
        0x60..=0x69 => Some(NUM_DIGITS[(vk - 0x60) as usize]),
        0x6A => Some("NumMul"),
        0x6B => Some("NumAdd"),
        0x6C => Some("NumEnter"),
        0x6D => Some("NumSub"),
        0x6E => Some("NumDot"),
        0x6F => Some("NumDiv"),
        0x70..=0x7B => Some(F_KEYS[(vk - 0x70) as usize]),
        0x90 => Some("NumLock"),
        0x91 => Some("ScrollLock"),
        0xBA => Some("OemSemicolon"),
        0xBB => Some("OemPlus"),
        0xBC => Some("OemComma"),
        0xBD => Some("OemMinus"),
        0xBE => Some("OemPeriod"),
        0xBF => Some("OemSlash"),
        0xC0 => Some("OemTilde"),
        0xDB => Some("OemLBracket"),
        0xDC => Some("OemBackslash"),
        0xDD => Some("OemRBracket"),
        0xDE => Some("OemQuote"),
        _ => None,
    }
}

/// 修饰键**内置显示名** → VK。
///
/// 控制器按内置名管理修饰键状态（`has_any_mod` / 卡死清理），渲染前缀时经此
/// 查覆盖文本——内部身份稳定，显示可改（ini 里 `Ctrl = Ctrl` 改名只影响前缀）。
pub fn mod_vk(name: &str) -> Option<u16> {
    match name {
        "Shift" => Some(0x10),
        "Ctrl" => Some(0x11),
        "Alt" => Some(0x12),
        "Win" => Some(0x5B),
        "Menu" => Some(0x5D),
        _ => None,
    }
}

/// 解析 ini 文本 → {规范键名: 显示文本}。
///
/// - 行首 `;` / `#` 是注释；`[节]` 忽略（分组纯为可读性）；
/// - 按**第一个** `=` 切分（显示文字里可以再写 `=`）；
/// - 键名区分大小写；值留空 = 这条覆盖作废（不进表）。
pub fn parse(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        map.insert(key.to_string(), value.to_string());
    }
    map
}

/// 带热重载的键名覆盖表。挂在控制器上，**按键频率**检查文件签名（不占每帧开销）。
pub struct Keynames {
    path: PathBuf,
    overrides: HashMap<String, String>,
    /// (mtime 毫秒, 字节数)。两个一起当"文件变没变"的签名——mtime 只到毫秒，
    /// 测试与快速手改可能落在同一毫秒里，size 补上第二维（内容变了通常大小也变）。
    stamp: Option<(u128, u64)>,
}

impl Keynames {
    /// 出厂态：空表（全部走内置默认名）。path 由调用方注入
    /// （生产 = `paths::keynames_ini_path()`，测试 = 临时文件）。
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            overrides: HashMap::new(),
            stamp: None,
        }
    }

    /// 文件签名变了才重读。文件消失 → 清空覆盖（全部回内置默认）。
    pub fn refresh(&mut self) {
        let stamp = self.stamp();
        if stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        match std::fs::read_to_string(&self.path) {
            Ok(text) => self.overrides = parse(&text),
            Err(_) => self.overrides.clear(),
        }
    }

    fn stamp(&self) -> Option<(u128, u64)> {
        let m = std::fs::metadata(&self.path).ok()?;
        let t = m
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis();
        Some((t, m.len()))
    }

    /// 按规范 ID 直接查覆盖文本（鼠标事件不走 VK；键盘走 [`Self::text_for`]）。
    pub fn text_for_id(&self, id: &str) -> Option<&str> {
        self.overrides.get(id).map(|s| s.as_str())
    }

    /// vk 的覆盖文本；`None` = 用内置默认名。
    pub fn text_for(&self, vk: u16) -> Option<&str> {
        self.text_for_id(canonical_name(vk)?)
    }

    /// 修饰键前缀的覆盖文本；`None` = 用内置名。
    pub fn mod_text<'a>(&'a self, builtin: &'a str) -> &'a str {
        mod_vk(builtin)
            .and_then(|vk| self.text_for(vk))
            .unwrap_or(builtin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::key_display::keymap;

    fn temp_ini(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "curseen_keynames_test_{}_{}.ini",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn parser_skips_comments_sections_and_junk() {
        let text = "; 注释\n# 也是注释\n[Names]\n\n垃圾行没有等号\nSpace = 空格\n  Ctrl  =  Ctrl  \nBad\n= 无键名\nA = Ctrl=C 复制\n";
        let m = parse(text);
        assert_eq!(m.get("Space").map(String::as_str), Some("空格"));
        // 键名与值两侧空白都要 trim。
        assert_eq!(m.get("Ctrl").map(String::as_str), Some("Ctrl"));
        // 值里的 `=` 按**第一个** = 切分后原样保留。
        assert_eq!(m.get("A").map(String::as_str), Some("Ctrl=C 复制"));
        assert!(!m.contains_key("垃圾行没有等号"), "无等号行不该进表");
        assert_eq!(m.len(), 3, "注释/节头/空键名行全部忽略");
    }

    #[test]
    fn empty_value_is_not_an_override() {
        let m = parse("Space = \nA = A\n");
        assert!(!m.contains_key("Space"), "空值 = 回退内置默认，不该进表");
        assert!(m.contains_key("A"));
    }

    #[test]
    fn canonical_covers_keymap_and_ids_are_unique() {
        // ① keymap 认识的每个 VK 都必须有规范名——ini 才能做到"全键位可配"。
        let mut ids = Vec::new();
        for vk in 0x00..=0xFFu16 {
            if keymap::lookup_vk(vk).is_some() {
                let name = canonical_name(vk)
                    .unwrap_or_else(|| panic!("VK 0x{vk:02X} 没有规范键名，ini 无法覆盖它"));
                ids.push(name);
            } else {
                assert!(
                    canonical_name(vk).is_none(),
                    "VK 0x{vk:02X} 不在 keymap 里却有规范名 {}",
                    canonical_name(vk).unwrap()
                );
            }
        }
        // ② 真不变量：同一条规范 ID 允许对应多个 VK（Win = 0x5B|0x5C），
        //    但它们在内置 keymap 里的显示名必须一致——否则一条 ini 行改出两种结果。
        let mut by_id: HashMap<&str, &str> = HashMap::new();
        for vk in 0x00..=0xFFu16 {
            if let Some(info) = keymap::lookup_vk(vk) {
                let id = canonical_name(vk).unwrap();
                if let Some(prev) = by_id.insert(id, info.display) {
                    assert_eq!(
                        prev, info.display,
                        "规范名 {id} 覆盖的多个 VK 显示名不一致：{prev} vs {}",
                        info.display
                    )
                }
            }
        }
        // ③ 全量对账：keymap 里的每个内置显示名都恰好由一条规范 ID 承载
        //    （防"两个 ID 写进了 ini 却只有一半生效"的漂移）。
        assert_eq!(
            by_id.len(),
            100,
            "规范 ID 应为 100 条（101 个 VK，Win 合并两个）"
        );
    }

    #[test]
    fn mod_vk_matches_builtin_display_names() {
        for (name, vk) in [
            ("Shift", 0x10),
            ("Ctrl", 0x11),
            ("Alt", 0x12),
            ("Win", 0x5B),
            ("Menu", 0x5D),
        ] {
            assert_eq!(keymap::lookup_vk(vk).unwrap().display, name);
            assert_eq!(mod_vk(name), Some(vk));
        }
        assert_eq!(mod_vk("不存在的修饰键"), None);
    }

    #[test]
    fn refresh_picks_up_edits_and_deletions_without_restart() {
        let path = temp_ini("reload");
        std::fs::write(&path, "Space = 空格键\n").unwrap();
        let mut kn = Keynames::new(path.clone());
        kn.refresh();
        assert_eq!(kn.text_for(0x20), Some("空格键"), "首次加载应读到覆盖");
        assert_eq!(kn.text_for(0x41), None, "没写的键回退内置默认");

        // 热重载：改文件 → 按键频率的 refresh 立即看到新文本。
        std::fs::write(&path, "Space = 空格\nA = 阿尔法\n").unwrap();
        kn.refresh();
        assert_eq!(kn.text_for(0x20), Some("空格"));
        assert_eq!(kn.text_for(0x41), Some("阿尔法"));

        // 删文件 → 全部回内置默认（不能留下半份旧表）。
        std::fs::remove_file(&path).unwrap();
        kn.refresh();
        assert_eq!(kn.text_for(0x20), None);
        assert_eq!(kn.text_for(0x41), None);
    }

    #[test]
    fn mod_text_overrides_prefix_but_keeps_builtin_fallback() {
        let path = temp_ini("mod");
        std::fs::write(&path, "Ctrl = Ctrl\nShift = 上档\n").unwrap();
        let mut kn = Keynames::new(path.clone());
        kn.refresh();
        assert_eq!(kn.mod_text("Ctrl"), "Ctrl", "与内置同名 = 不变");
        assert_eq!(kn.mod_text("Shift"), "上档", "改名只影响前缀显示");
        assert_eq!(kn.mod_text("Alt"), "Alt", "没写的修饰键用内置名");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn shipped_ini_covers_every_key_with_nonempty_values() {
        // ★ "要做好按键对应定义"的硬门禁：出厂 ini 必须覆盖**全部**规范键名且值非空。
        //   判据从盘上真文件读（不是从编译期内嵌常量），ini 被人删行/删值会直接红。
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("KeyDisplay")
            .join("keynames.ini");
        let text = std::fs::read_to_string(&path)
            .expect("出厂 keynames.ini 缺失——它是键名自定义功能的唯一载体");
        let map = parse(&text);
        for vk in 0x00..=0xFFu16 {
            if keymap::lookup_vk(vk).is_some() {
                let id = canonical_name(vk).unwrap();
                let value = map
                    .get(id)
                    .unwrap_or_else(|| panic!("出厂 ini 缺少键名 {id}（VK 0x{vk:02X}）"));
                assert!(!value.trim().is_empty(), "出厂 ini 里 {id} 的值是空的");
            }
        }
        // ④ 鼠标 9 条也必须全量在册（2026-09-22 追加：鼠标键位同样可自定义）。
        for (id, builtin) in MOUSE_IDENTS {
            let value = map
                .get(id)
                .unwrap_or_else(|| panic!("出厂 ini 缺少鼠标键名 {id}（内置名 {builtin}）"));
            assert!(
                !value.trim().is_empty(),
                "出厂 ini 里鼠标键名 {id} 的值是空的"
            );
        }
    }

    #[test]
    fn old_ini_without_mouse_entries_falls_back_to_builtins() {
        // 兼容性：只有键盘条目的旧 ini（§12.9 首版）在新程序里照常加载——
        // 鼠标没有覆盖 → 回退内置名，不报错、不丢功能。
        let path = temp_ini("old");
        std::fs::write(&path, "Space = 空格\n").unwrap();
        let mut kn = Keynames::new(path);
        kn.refresh();
        assert_eq!(kn.text_for(0x20), Some("空格"));
        assert_eq!(kn.text_for_id("MouseLeft"), None, "旧 ini 没有鼠标条目");
        assert_eq!(
            kn.text_for_id("MouseLeft").unwrap_or(MOUSE_IDENTS[0].1),
            "左键",
            "缺省回退内置名"
        );
    }
}
