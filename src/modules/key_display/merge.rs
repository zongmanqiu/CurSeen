//! 按键条目与合并逻辑。逐字对照 controller.py:24-52。

use std::fmt;

use super::settings::MAX_ITEMS;
use std::collections::VecDeque;

/// 一组连续相同的按键（显示文本 + 重复次数）。
#[derive(Clone, Debug)]
pub struct KeyGroup {
    pub text: String,
    pub count: u32,
}

/// 一条按键弹窗记录，由多个 KeyGroup 组成。
#[derive(Clone, Debug)]
pub struct KeyItem {
    pub groups: Vec<KeyGroup>,
    pub timestamp: f64,
    pub opacity: f32,
    pub is_char: bool,
    pub is_fading: bool,
}

impl KeyItem {
    pub fn new(text: &str, timestamp: f64, is_char: bool) -> Self {
        Self {
            groups: vec![KeyGroup {
                text: text.to_string(),
                count: 1,
            }],
            timestamp,
            opacity: 1.0,
            is_char,
            is_fading: false,
        }
    }

    /// 显示文本。
    ///
    /// ★ 间隔模型（2026-09-22 用户拍板）：**每个元素自带头部一个尾随空格**——
    ///   组文本 `"{text} "`、计数徽标 `"x{N} "`；行首就是首组文本的开头（新弹窗
    ///   无需任何去空格特例），行尾留一个空格（不可见、无害）。旧模型空格归属
    ///   分散在 merge（续组前导）/display_text（徽标前导+尾随）两处，组间出现
    ///   双空格，且键名墨迹挤压下空格被吃（"LMBx 5" 事故）。字宽表（见
    ///   `layout::char_advance`）保证墨迹不再超出推进量，单规则间隔稳定。
    pub fn display_text(&self) -> String {
        let mut result = String::new();
        for g in &self.groups {
            result.push_str(&g.text);
            result.push(' ');
            if g.count > 1 {
                result.push_str(&format!("x{} ", g.count));
            }
        }
        result.trim_end().to_string()
    }

    /// 合并。返回值恒为 true（对齐 Python）。
    ///
    /// 对齐 controller.py:39-53，逐字：
    pub fn merge(&mut self, other: &KeyItem) -> bool {
        let other_text = other.groups[0].text.as_str();
        if let Some(last) = self.groups.last_mut() {
            if last.text == other_text {
                last.count += 1;
                self.timestamp = other.timestamp;
                return true;
            }
        }
        // 尾随模型：组文本不带前导空格——与下一个组之间的间隔由本组**尾部**的
        // 空格（display_text 拼接时统一补）承担，见 [`KeyItem::display_text`]。
        self.groups.push(KeyGroup {
            text: other_text.to_string(),
            count: 1,
        });
        self.timestamp = other.timestamp;
        if self.groups.len() > MAX_ITEMS * 6 {
            let cut = std::cmp::max(10, self.groups.len() / 4);
            self.groups.drain(0..cut);
        }
        true
    }
}

/// 带容量限制的条目队列。
pub struct KeyItemQueue {
    items: VecDeque<KeyItem>,
}

impl KeyItemQueue {
    pub fn new() -> Self {
        Self {
            items: VecDeque::new(),
        }
    }

    pub fn push_back(&mut self, item: KeyItem) {
        if self.items.len() >= MAX_ITEMS {
            self.items.pop_front();
        }
        self.items.push_back(item);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> impl Iterator<Item = &KeyItem> {
        self.items.iter()
    }

    pub fn items_mut(&mut self) -> impl Iterator<Item = &mut KeyItem> {
        self.items.iter_mut()
    }

    pub fn remove(&mut self, index: usize) -> Option<KeyItem> {
        self.items.remove(index)
    }
}

impl Default for KeyItemQueue {
    fn default() -> Self {
        Self::new()
    }
}

/// 按" + "分割快捷键文本。
///
/// `line.split(" + ")` 的等价实现，保留空格。
pub fn split_shortcut(line: &str) -> Vec<&str> {
    line.split(" + ").collect()
}

/// 按单个空格切分文本，保留空格 token。
///
/// 等价 Python 的 `re.split(r'( )', line)`。
/// `"A x3 "` → `["A", " ", "x3", ""]`
pub fn split_keeping_spaces(line: &str) -> Vec<&str> {
    // ★★ 全程用**字节偏移**，绝不用 `chars()` 的下标。
    //
    // 这里曾有一个致命 bug（2026-09-18 实测崩溃，日志 `[FATAL] panic: start byte index
    // 1 is not a char boundary; it is inside '←'`）：空格分支写的是 `&line[i..i + 1]`，
    // 而 `i` 是按 `line.chars()` 递增的**字符索引** —— 两个量纲混用。
    //
    // 只要**多字节字符后紧跟空格**就炸：方向键显示名 `←` 占 3 字节，`"← x2 "` 里那个
    // 空格的字符索引是 1、字节偏移是 3，于是 `&line[1..2]` 切在 `←` 内部 → panic。
    // 配合 `panic = "abort"` + GUI 子系统（无控制台），用户只看到"程序毫无征兆地没了"
    // —— 这正是"击键回显经常闪退"的真凶，一按方向键必崩。
    //
    // 字节遍历是安全的：分隔符是 ASCII 空格（`b' '`），非空格字节只用来推进游标、
    // 不会单独成切片，故 `start` / `i` 恒落在字符边界上。
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b' ' {
            tokens.push(&line[i..i + 1]);
            i += 1;
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b' ' {
                i += 1;
            }
            tokens.push(&line[start..i]);
        }
    }
    // 末尾追加空串
    if !line.is_empty() && line.ends_with(' ') {
        tokens.push("");
    }
    tokens
}

/// 手写扫描：从 `"Ctrl x3"` 中拆出 `("Ctrl", " x3")`。
///
/// 从右往左找最后一个 `" x"`，要求其后是 `x` + 非空纯数字（新徽标格式，
/// 见 [`KeyItem::display_text`]）。
pub fn split_x_suffix(part: &str) -> Option<(&str, &str)> {
    if let Some(p) = part.rfind(" x") {
        if p > 0 {
            // p 指向空格，'x' 在 p+1；徽标 = "x" + 非空纯数字。
            let suffix = &part[p + 1..];
            if suffix.len() >= 2
                && suffix.starts_with('x')
                && suffix[1..].chars().all(|c| c.is_ascii_digit())
            {
                return Some((&part[..p], &part[p..]));
            }
        }
    }
    None
}

/// 是否为计数徽标 token：`x` 紧跟纯数字（"x3"、"x12"）。
pub fn is_count_badge(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() >= 2 && b[0] == b'x' && b[1..].iter().all(|c| c.is_ascii_digit())
}

/// 检查一行是否是快捷键（5 个硬编码前缀）。
pub fn is_shortcut_line(line: &str) -> bool {
    line.starts_with("Ctrl +")
        || line.starts_with("Alt +")
        || line.starts_with("Shift +")
        || line.starts_with("Win +")
        || line.starts_with("Menu +")
}

impl fmt::Display for KeyItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_text())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_item() {
        let item = KeyItem::new("A", 1.0, true);
        assert_eq!(item.groups.len(), 1);
        assert_eq!(item.groups[0].text, "A");
        assert_eq!(item.groups[0].count, 1);
        assert_eq!(item.opacity, 1.0);
        assert!(!item.is_fading);
    }

    #[test]
    fn display_text_plain() {
        let item = KeyItem::new("A", 1.0, true);
        assert_eq!(item.display_text(), "A");
    }

    #[test]
    fn display_text_with_count() {
        let mut item = KeyItem::new("A", 1.0, true);
        item.merge(&KeyItem::new("A", 2.0, true));
        item.merge(&KeyItem::new("A", 3.0, true));
        assert_eq!(item.display_text(), "A x3");
    }

    /// ★ 计数徽标是 `x{N}` 单 token（2026-09-22 统一格式）：渲染端按 token 分组着色，
    ///   徽标拆成多个 token 曾让键名与 "x" 之间的空格依赖推进宽度（实测丢过）。
    #[test]
    fn count_badge_is_single_token() {
        assert!(is_count_badge("x3"));
        assert!(is_count_badge("x12"));
        assert!(!is_count_badge("x"), "裸 x 不是徽标");
        assert!(!is_count_badge("x3a"), "非纯数字不算");
        assert!(!is_count_badge("LMB"), "键名不是徽标");
    }

    #[test]
    fn merge_same_text() {
        let mut item = KeyItem::new("A", 1.0, true);
        assert!(item.merge(&KeyItem::new("A", 2.0, true)));
        assert_eq!(item.groups.len(), 1);
        assert_eq!(item.groups[0].count, 2);
    }

    #[test]
    fn merge_different_text() {
        let mut item = KeyItem::new("A", 1.0, true);
        assert!(item.merge(&KeyItem::new("B", 2.0, true)));
        assert_eq!(item.groups.len(), 2);
        assert_eq!(item.groups[0].text, "A");
        // 尾随模型：组文本不带前导空格——间隔由 display_text 统一补。
        assert_eq!(item.groups[1].text, "B");
    }

    #[test]
    fn merge_update_timestamp() {
        let mut item = KeyItem::new("A", 1.0, true);
        item.merge(&KeyItem::new("A", 5.0, true));
        assert_eq!(item.timestamp, 5.0);
    }

    #[test]
    fn merge_trimspace() {
        let mut item = KeyItem::new("A", 1.0, true);
        // group 0 text is "A"; other text is "A" → trim_start == text
        assert!(item.merge(&KeyItem::new("A", 2.0, true)));
        assert_eq!(item.groups[0].count, 2);
    }

    #[test]
    fn split_keeping_spaces_basic() {
        // display_text（trim 后）的行形态：词 + 空格 + 徽标，共 3 token。
        let tokens = split_keeping_spaces("A x3");
        assert_eq!(tokens, vec!["A", " ", "x3"]);
    }

    #[test]
    fn split_keeping_spaces_no_trailing() {
        let tokens = split_keeping_spaces("A x3");
        assert_eq!(tokens, vec!["A", " ", "x3"]);
    }

    /// ★★ 宽字符（多字节）键名 + 空格 —— 这条用例的**前身是必崩路径**。
    ///
    /// 2026-09-18 的闪退真凶就在这个函数：空格分支曾用 `chars()` 的**字符索引**去切
    /// **字节**（`&line[i..i + 1]`）。`←` 占 3 字节，`"← x2 "` 里空格的字符索引是 1、
    /// 字节偏移是 3 → `&line[1..2]` 切在 `←` 内部 → panic；而 `panic = "abort"` + GUI
    /// 子系统下用户只看到"程序毫无征兆地没了"（按一下方向键就复现）。
    ///
    /// 旧用例全是 ASCII（`"A x3 "`），字符索引恰好等于字节偏移，所以**一直绿**——
    /// 典型的"测试数据太窄"型假绿。这里把多字节形态逐一钉住。
    #[test]
    fn split_keeping_spaces_handles_wide_chars() {
        // 方向键的显示名（`events::KeyCode::display_name` 输出 `←`/`→`/`↑`/`↓`）
        assert_eq!(split_keeping_spaces("← x2 "), vec!["←", " ", "x2", " ", ""]);
        assert_eq!(split_keeping_spaces("→ "), vec!["→", " ", ""]);
        assert_eq!(split_keeping_spaces("↑↑"), vec!["↑↑"]);

        // 中文键名（IME / 非 ASCII 字符走 `non_ascii_char_name` 的缓存路径）
        assert_eq!(split_keeping_spaces("中 A"), vec!["中", " ", "A"]);
        assert_eq!(split_keeping_spaces("中文"), vec!["中文"]);
    }

    /// 端到端：连续按两次方向键，从**真实合并路径**产出显示文本再切分。
    ///
    /// `merge` 会把重复按键折成 `"← x2 "`——这正是喂给 `split_keeping_spaces` 的输入，
    /// 也是崩溃现场的那串文本。用例从"按键 → 合并 → 显示文本 → 切分"整条走一遍，
    /// 避免只测切分函数而漏掉上游会拼出什么。
    #[test]
    fn merged_arrow_key_text_splits() {
        let mut item = KeyItem::new("←", 1.0, false);
        item.merge(&KeyItem::new("←", 2.0, false));
        let text = item.display_text();
        assert_eq!(text, "← x2");
        assert_eq!(split_keeping_spaces(&text), vec!["←", " ", "x2"]);
    }

    #[test]
    fn split_x_suffix_simple() {
        assert_eq!(split_x_suffix("Ctrl x3"), Some(("Ctrl", " x3")));
        assert_eq!(split_x_suffix("LMB x5"), Some(("LMB", " x5")));
    }

    #[test]
    fn split_x_suffix_no_match() {
        assert_eq!(split_x_suffix("Ctrl"), None);
    }

    #[test]
    fn split_x_suffix_no_digit() {
        assert_eq!(split_x_suffix("x3"), None);
        assert_eq!(split_x_suffix("Ctrl x"), None, "x 后无数字不是徽标");
        assert_eq!(
            split_x_suffix("A x 3"),
            None,
            "旧格式（x 与数字间有空格）不再是徽标"
        );
    }

    #[test]
    fn split_x_suffix_multi() {
        assert_eq!(split_x_suffix("A x3 A x4"), Some(("A x3 A", " x4")));
    }

    #[test]
    fn is_shortcut_line_true() {
        assert!(is_shortcut_line("Ctrl + C"));
        assert!(is_shortcut_line("Alt + F4"));
        assert!(is_shortcut_line("Shift + X"));
        assert!(is_shortcut_line("Win + R"));
        assert!(is_shortcut_line("Menu + A"));
    }

    #[test]
    fn is_shortcut_line_false() {
        assert!(!is_shortcut_line("A"));
        assert!(!is_shortcut_line("A + B"));
    }

    #[test]
    fn max_capacity() {
        let mut queue = KeyItemQueue::new();
        for i in 0..100 {
            queue.push_back(KeyItem::new("A", i as f64, true));
        }
        assert_eq!(queue.len(), MAX_ITEMS);
    }
}
