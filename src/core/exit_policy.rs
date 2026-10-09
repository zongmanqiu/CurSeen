//! 叠层消费方的「取消 / 确认」手势声明。
//!
//! 用户澄清过一件事：*"贴图可以 Esc 取消、也可以鼠标左键双击取消；重复按贴图快捷键只是
//! 增加新的贴图；这些操作与底层函数无关。"* 这一句里其实是两件事——
//!
//! 1. **底层只管"一份冻结帧 + 怎么铺上去"**（[`crate::core::backdrop::Backdrop`]）；
//! 2. **"怎么退出"是每个消费方自己的事**，三家的答案并不一样。
//!
//! 本模块把第 2 件事写成一张**共读的表**：三个消费方各自读自己的那份常量，而不是在各自的
//! `on_event` 里散落 `if pe.click_count >= 2`。这样"改画板顺手把贴图也改了"会在读表时就
//! 被发现，而不是等用户误触才暴露。
//!
//! | 消费方 | Esc | 快捷键再按 | 左键双击 | 右键 | 回车 / 松手 |
//! |---|---|---|---|---|---|
//! | 截图区域选择 [`ExitPolicy::REGION`] | 取消 | — | — | 取消 | 确认 |
//! | 屏幕注释 [`ExitPolicy::ANNOTATE`] | 退出标注（**保留**墨迹） | 进入 / 退出 | **不响应** | 不响应 | — |
//! | 贴图窗 [`ExitPolicy::PIN`] | 关闭该窗 | **再按 = 再贴一张** | 关闭该窗 | 关闭该窗 | — |
//!
//! # 两条"刻意的空白"，别顺手补上
//!
//! - **注释层不吃双击**：覆盖层是全屏窗，连点两下是画点、选工具这类高频动作，绑上必误退。
//! - **贴图窗不是 toggle**：用户原话——"如果重复 pin 图快捷键，只是增加新的 pin 图"。
//!   所以 `hotkey_toggle = false`，快捷键每次都当"再贴一张"处理。
//!
//! # 为什么在 `core` 而不是 `runtime`
//!
//! 三个消费方里，注释层是 `modules/drawing`，而 **modules 不得依赖 runtime**（分层纪律）。
//! 与 [`crate::core::backdrop::Backdrop`] 同一个理由：core 是纯逻辑，谁都能读。

/// 一个叠层消费方允许的"取消 / 确认"手势。
///
/// 字段全是 `bool` 且语义互不重叠——读表比读三个 `on_event` 更快。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ExitPolicy {
    /// Esc 可退出。
    pub escape: bool,
    /// 该功能的快捷键再按一次可退出。仅注释层为真（`Alt+G` / `Alt+W` 本来就是 toggle）。
    pub hotkey_toggle: bool,
    /// 鼠标左键**双击**可退出。
    ///
    /// 依赖 windui 补丁 022-a：无边框窗的外圈 8px 曾被判成缩放热区，靠近边缘的双击会
    /// 被 `WM_NCHITTEST` 截走、`click_count` 永远到不了 2。
    pub double_click: bool,
    /// 鼠标**右键**可退出（截图区域窗"单击右键取消"、贴图窗"右键关闭"）。
    pub right_click: bool,
    /// 回车 / 左键松手确认。仅截图区域窗为真。
    pub confirm: bool,
}

impl ExitPolicy {
    /// 截图区域选择：Esc / 右键取消，回车 / 松手确认。**不吃双击**——双击在这里等于
    /// 两次"按下-松开"，本就会被解读成"框了两次又取消"，绑上语义只会打架。
    pub const REGION: Self = Self {
        escape: true,
        hotkey_toggle: false,
        double_click: false,
        right_click: true,
        confirm: true,
    };

    /// 屏幕注释（透明 / 白板）：Esc 退出标注（墨迹**保留**），快捷键可进可退。
    /// 双击与右键都不响应（见模块头的"刻意的空白"）。
    pub const ANNOTATE: Self = Self {
        escape: true,
        hotkey_toggle: true,
        double_click: false,
        right_click: false,
        confirm: false,
    };

    /// 贴图窗：Esc / 双击 / 右键都关它，但**再按贴图键只是再贴一张**。
    pub const PIN: Self = Self {
        escape: true,
        hotkey_toggle: false,
        double_click: true,
        right_click: true,
        confirm: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个消费方的手势矩阵逐格锁死（与模块头的表同源）。
    ///
    /// 这条测试的意义不在于"常量写对了"，而在于：以后谁想给注释层加双击退出，就必须先改
    /// 这张表——改表会被 review 看见，而散落在 `on_event` 里的一个 `if` 不会。
    #[test]
    fn gesture_matrix_is_locked() {
        let table = [
            ("REGION", ExitPolicy::REGION, true, false, false, true, true),
            (
                "ANNOTATE",
                ExitPolicy::ANNOTATE,
                true,
                true,
                false,
                false,
                false,
            ),
            ("PIN", ExitPolicy::PIN, true, false, true, true, false),
        ];
        for (name, p, esc, toggle, dbl, right, confirm) in table {
            assert_eq!(p.escape, esc, "{name}.escape");
            assert_eq!(p.hotkey_toggle, toggle, "{name}.hotkey_toggle");
            assert_eq!(p.double_click, dbl, "{name}.double_click");
            assert_eq!(p.right_click, right, "{name}.right_click");
            assert_eq!(p.confirm, confirm, "{name}.confirm");
        }
    }

    /// 四条"刻意"的空白单独再钉一遍——它们最容易被后来者"顺手补上"。
    ///
    /// 走表驱动而不是直接 `assert!(!ExitPolicy::ANNOTATE.double_click)`：后者是**常量断言**，
    /// clippy 的 `assertions_on_constants` 会拦下。而这里想表达的是"这个字段被有意留空，
    /// 且有理由"，顺带把理由一起记在断言消息里——将来谁要改它，先把理由说服。
    #[test]
    fn deliberate_blanks_stay_blank() {
        let cases: [(&str, bool, &str); 4] = [
            (
                "ANNOTATE.double_click",
                ExitPolicy::ANNOTATE.double_click,
                "注释层是全屏窗，连点是画点 / 选工具的高频动作，加双击退出必误退",
            ),
            (
                "ANNOTATE.right_click",
                ExitPolicy::ANNOTATE.right_click,
                "注释层的右键要留给将来的画笔轮盘，不承担退出",
            ),
            (
                "PIN.hotkey_toggle",
                ExitPolicy::PIN.hotkey_toggle,
                "贴图键不是 toggle：再按只是再贴一张",
            ),
            (
                "REGION.double_click",
                ExitPolicy::REGION.double_click,
                "区域选择的双击会被解读成两次框选，不该额外定义语义",
            ),
        ];
        for (field, value, why) in cases {
            assert!(!value, "{field} 应当为假：{why}");
        }
    }
}
