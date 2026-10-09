//! 侧边栏导航组件。
//!
//! 七个分类：**鼠标 · 键盘 · 屏幕 · 工具 · 脚本 · 设置 · 关于**（2026-09-25 重排）。
//!
//! # 分类判据（用户口径）
//!
//! 看**这个功能操作 / 处理的东西是什么**——不是它显示在哪，也不是靠什么触发：
//!
//! · 拖尾 / 悬浮标 / 点击效果 / 点击声音 → 操作的是鼠标（指针位置、鼠标键）→ 鼠标页；
//! · 击键回显 / 击键音效 / 虚拟键盘 → 操作的是键盘 → 键盘页（虚拟键盘显示在屏上，
//!   但它是键盘，归键盘）；
//! · 屏幕标注（画它）/ 放大镜（放大它）/ 截图与贴图 / GIF录制 / 视频录制（抓它）
//!   → 操作的都是屏幕画面 → 屏幕页。
//!
//! 上一版把「截图」留在屏幕页而把「GIF / 录屏」单独开一个「录制」页，是人为割裂：
//! 三者共用框选与采集管线（`recorder::Session`），差别只在产物是一帧、多帧还是视频。
//! 现在合并，录制作为**页内分组**保留（任务没丢，只是不再占一页）。
//!
//! 「工具」页不是"没想好"的收容区：便签（操作的是我的文字）、剪贴板（操作的是复制
//! 的内容）、计时器（操作的是时间）**不对应任何硬件**，它们是一类正式成员，所以页名
//! 不再叫「其它」——那个名字会让人以为是不重要的杂项。
//!
//! 标签刻意取短（2 字）：它们是分类入口，具体功能名（拖尾/悬浮标/点击效果/点击声音…）
//! 在各自页面里再由卡片标题表达。短标签让侧边栏能收窄到 66px 也不必担心文字折行。
//!
//! 选中态用一个信号驱动的指示条表示——背景色没法绑信号（`Element::bg` 只吃常量），
//! 而 `visible_signal` 可以，所以高亮做成「选中时出现一条强调色竖条」。
//!
//! # 页码持久化：记名字，不记下标
//!
//! `ui.panel_page` 存的是**下标**，而下标会随页序变化漂移：本次把 8 页重排成 7 页后，
//! 老配置里的 3（录制）应该落到屏幕、7（Lua）应该落到脚本——若照旧按下标读，用户上次
//! 停在「其它」，改完会停在「脚本」。
//!
//! 所以另存一份**页 id**（见 [`NAV_IDS`] / [`crate::core::config::UiSettings::panel_page_id`]）：
//! 读的时候 id 优先，读不到（老配置）再走 [`legacy_page_index`] 把老下标翻成新下标。
//! 以后加减页都不必再为这件事担心。

use windui::prelude::*;

use super::tokens;
use super::tokens::to_color;

/// 导航项索引常量
pub const NAV_TRAIL: usize = 0;
pub const NAV_KEY: usize = 1;
/// 屏幕页：屏幕标注 + 画中画/全屏放大 + 截图与贴图 + GIF录制 + 视频录制。
pub const NAV_SCREEN: usize = 2;
/// 工具页：便签 + 剪贴板 + 计时器（不对应硬件的那一类功能）。
pub const NAV_TOOL: usize = 3;
/// 脚本页（原 Lua 页）：列 `assets/lua` 脚本、配置参数、运行。
pub const NAV_LUA: usize = 4;
pub const NAV_GLOBAL: usize = 5;
pub const NAV_ABOUT: usize = 6;

/// 侧边栏宽度（逻辑像素）。
///
/// ★ 2026-09-25 从 180 收窄到 110，再收到 66：标签刻意全是 2 字中文（见下），
/// 2 字 × [`tokens::FONT_MD`](14) = 28px + 左右内边距 13×2 + 选中指示条 3 = **下限 57**。
/// 66 是 2026-09-28 字号统一（导航从 16 降到 14）之后仍然沿用的值——下限降到 57 也不
/// 收窄：这个宽度会牵动内容区宽度与全项目控件列的 x，收益（省 9px）不值得冒这个险。
pub const NAV_WIDTH: i32 = 66;

const LABELS: [&str; 7] = ["鼠标", "键盘", "屏幕", "工具", "脚本", "设置", "关于"];

/// 页 id（落盘用，`ui.panel_page_id`）。
///
/// ★ 与 [`LABELS`] **同序**——下标 ↔ id 的换算只在这两张表之间发生，不散在别处手算。
/// 用 id 而不是下标，是为了让页序变化不再影响"用户上次停在哪一页"。
pub const NAV_IDS: [&str; 7] = [
    "mouse", "keyboard", "screen", "tool", "script", "settings", "about",
];

/// 创建侧边栏导航。
pub fn build_nav(current: Signal<usize>) -> Element {
    let mut col = Element::col().width(NAV_WIDTH).bg(to_color(tokens::NAV_BG));

    for (idx, label) in LABELS.iter().enumerate() {
        col = col.child(nav_item(current, idx, label));
    }

    col
}

/// 单个导航项：选中指示条 + 名称。
fn nav_item(current: Signal<usize>, idx: usize, label: &'static str) -> Element {
    let selected = current.map(move |c| *c == idx);

    Element::row()
        .child(
            Element::leaf()
                .width(3)
                .height(20)
                .bg(to_color(tokens::ACCENT))
                .visible_signal(selected),
        )
        .child(
            Element::label(label)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT))
                .padding_xy(13, 0),
        )
        .child(Element::flex_spacer())
        .cross(Align::Center)
        .height(40)
        .clickable()
        .on_click(move |_ctx| current.set(idx))
}

/// 获取当前页面名称
///
/// 直接取 [`LABELS`]：**不另抄一份**名字表——抄一份就会出现"标签改了页名没改"的两张皮。
pub fn get_page_name(idx: usize) -> &'static str {
    LABELS.get(idx).copied().unwrap_or("未知")
}

/// 下标 → 页 id（越界兜底第一页，与 `get_page_name` 的兜底同思路）。
pub fn page_id(idx: usize) -> &'static str {
    NAV_IDS.get(idx).copied().unwrap_or(NAV_IDS[0])
}

/// 页 id → 下标；id 不认识返回 `None`（调用方兜底到第一页）。
pub fn page_index(id: &str) -> Option<usize> {
    NAV_IDS.iter().position(|p| *p == id)
}

/// 老下标（8 页时代）→ 新下标（7 页）。
///
/// 老顺序是「鼠标 / 键盘 / 屏幕 / 录制 / 其它 / 设置 / 关于 / Lua」：
/// 录制(3) 并进屏幕(2)、其它(4) 改名工具(3)、Lua(7) 改名脚本(4) 且挪到设置之前；
/// 设置 5→5、关于 6→6。越界一律兜底到第一页（老配置里可能存着 99 之类的脏值）。
pub fn legacy_page_index(old: usize) -> usize {
    const MAP: [usize; 8] = [
        NAV_TRAIL,  // 0 鼠标
        NAV_KEY,    // 1 键盘
        NAV_SCREEN, // 2 屏幕
        NAV_SCREEN, // 3 录制 → 屏幕
        NAV_TOOL,   // 4 其它 → 工具
        NAV_GLOBAL, // 5 设置
        NAV_ABOUT,  // 6 关于
        NAV_LUA,    // 7 Lua → 脚本
    ];
    MAP.get(old).copied().unwrap_or(NAV_TRAIL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_names_cover_all_indices() {
        for (i, name) in LABELS.iter().enumerate() {
            assert_eq!(get_page_name(i), *name);
        }
        assert_eq!(get_page_name(99), "未知");
    }

    /// 下标 ↔ id 必须严格往返：任何一侧错位，"用户上次停在哪页"就会静默漂到别的页，
    /// 而这类错误没有任何报错（只是开面板时停在了一个陌生的页上）。
    #[test]
    fn page_id_and_index_roundtrip() {
        for (i, id) in NAV_IDS.iter().enumerate() {
            assert_eq!(page_id(i), *id);
            assert_eq!(page_index(id), Some(i));
        }
        assert_eq!(page_id(99), NAV_IDS[0]);
        assert_eq!(page_index("nope"), None);
    }

    /// 8 页时代的下标必须翻到新 7 页的正确页——这是本次重排唯一会伤到老用户的地方。
    #[test]
    fn legacy_page_indices_migrate_to_the_new_order() {
        assert_eq!(legacy_page_index(0), NAV_TRAIL);
        assert_eq!(legacy_page_index(1), NAV_KEY);
        assert_eq!(legacy_page_index(2), NAV_SCREEN);
        // 录制页并进屏幕页
        assert_eq!(legacy_page_index(3), NAV_SCREEN);
        // 其它页改名工具页
        assert_eq!(legacy_page_index(4), NAV_TOOL);
        assert_eq!(legacy_page_index(5), NAV_GLOBAL);
        assert_eq!(legacy_page_index(6), NAV_ABOUT);
        // Lua 页改名脚本页
        assert_eq!(legacy_page_index(7), NAV_LUA);
        // 脏值兜底
        assert_eq!(legacy_page_index(99), NAV_TRAIL);
    }
}
