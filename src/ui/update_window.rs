//! 发现新版本时的说明窗：HISTORY 原文 + 底部「更新」。
//!
//! 正文沿用脚本文本窗的记事本式文本区（可选中、Ctrl+C），不渲染 Markdown；
//! 用户看到的是可以原地复制进 HISTORY 的原文。更新按钮与下载动作在同一处，
//! 关闭窗后由既有后台更新线程接管，UI 不阻塞。

use windui::core::EventCtx;
use windui::prelude::*;

use super::tokens::{self, to_color};
use crate::core::events::UiMsg;
use crate::update::UpdatePrompt;

const W: i32 = 560;
const H: i32 = 480;
const MIN_W: i32 = 360;
const MIN_H: i32 = 220;

/// 打开说明窗。`sender` 是 UI 通道；没有通道时按钮仍可见但点击只记日志。
pub fn open(ctx: &mut EventCtx, prompt: UpdatePrompt, sender: Option<Sender<UiMsg>>) {
    let latest = prompt.latest.clone();
    let notes = prompt.notes.clone();
    let urls = prompt.urls;
    let sha256 = prompt.sha256.clone();
    let sender_for_btn = sender.clone();
    ctx.open_window(
        Window::new("发现新版本", W, H)
            .owned(true)
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .centered(true)
            .single("update-notes")
            .bg(to_color(tokens::BG))
            .content(move || content(&latest, &notes, &urls, &sha256, sender_for_btn.clone())),
    );
}

fn content(
    latest: &str,
    notes: &str,
    urls: &[String; 2],
    sha256: &str,
    sender: Option<Sender<UiMsg>>,
) -> Element {
    let body = signal(notes.to_string());
    let urls = urls.clone();
    let sha256 = sha256.to_string();
    let text = Element::text_input(body, "")
        .multiline()
        .wrap(true)
        .font_size(tokens::FONT_MD)
        .frameless()
        .padding_xy(8, 6)
        // 程序塞入的文本光标默认在末尾；钉到开头，开窗先看到 HISTORY 第一段。
        .select_range(0, 0)
        .weight(1.0)
        .width_match();

    let update = Element::button("更新").on_click(move |ctx| {
        ctx.request_close();
        if let Err(e) = crate::update::task::start_download(&sender, &urls, &sha256) {
            log::warn!("发起更新下载失败：{e}");
        }
    });
    let cancel = Element::button("取消")
        .neutral()
        .on_click(|ctx| ctx.request_close());

    Element::col()
        .fill()
        .spacing(0)
        .child(
            Element::label(format!("发现新版本 {latest}"))
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT))
                .padding_xy(16, 12),
        )
        .child(
            // ★ 说明区**自己就是滚动容器**（多行文本框内部滚），外面不许再套
            //   `Element::scroll()`：外层滚动按无界高度测量子节点，文本框拿不到确定
            //   高度，会缩成一小块、余下全留白，外层还多出一条滚动条（真机现象）。
            //   形状与脚本文本窗的信息档一致（`ui::script_dialog::content_tree`）。
            text,
        )
        .child(
            Element::row()
                .spacing(8)
                .cross(Align::Center)
                .padding_xy(16, 12)
                .width_match()
                .child(Element::flex_spacer())
                .child(update)
                .child(cancel),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::text::NullTextEngine;

    /// 本文件的生产代码文本（截至测试模块）——`include_str!` 会把测试自身也包进来。
    fn production_src() -> &'static str {
        let all = include_str!("update_window.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    /// ★ 回归（2026-10-31 真机）：说明窗的正文只占一小块、余下大片空白，还多出一条
    /// 垂直滚动条。原因是在多行文本框外面又套了一层 `Element::scroll()`：外层按无界
    /// 高度测量子节点，文本框拿不到确定高度就缩成一小块，剩余空间成了外层滚动视口。
    /// 判据两条：源码里不许再有外层滚动容器；真实排版下文本区必须吃满剩余高度与宽度。
    #[test]
    fn notes_area_fills_the_window_and_owns_its_scroll() {
        let p = production_src();
        assert!(
            !p.contains(".child(Element::scroll()"),
            "说明窗不该再套外层滚动容器（文本框自己滚）"
        );

        let notes = "### 修复\n\n- 很长的说明。\n".repeat(60);
        let root = content(
            "2.1.0",
            &notes,
            &[
                "https://gitee.com/x/a.zip".to_string(),
                "https://github.com/x/a.zip".to_string(),
            ],
            &"a".repeat(64),
            None,
        );
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(W, H), &mut NullTextEngine);
        let node = tree.get(id).expect("根节点");
        let kids: Vec<Rect> = node
            .children
            .iter()
            .map(|k| tree.get(*k).expect("子节点").bounds)
            .collect();
        assert_eq!(kids.len(), 3, "应为「标题 + 文本区 + 按钮行」三段");
        // ★ 文本区必须是**文本框本身**（叶子），外面包一层滚动容器正是这次的缺陷：
        //   容器自己铺满、把文本框压成一小块，还多出一条外层滚动条。
        let area_node = tree.get(node.children[1]).expect("文本区节点");
        assert!(
            area_node.children.is_empty(),
            "文本区不许再包一层容器（应直接是文本框）"
        );
        let area = kids[1];
        assert_eq!(area.w, W, "文本区必须铺满宽度");
        assert!(
            area.h > H / 2,
            "文本区必须吃剩余高度，不能缩成一小块：{} / {}",
            area.h,
            H
        );
        // 按钮行仍在客户区内（与脚本文本窗同一条纪律：被压缩的只能是文本区）。
        let foot = kids[2];
        assert!(
            foot.y + foot.h <= H,
            "按钮行越界：{} > {}",
            foot.y + foot.h,
            H
        );
    }
}
