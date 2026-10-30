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
    let url = prompt.url.clone();
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
            .content(move || content(&latest, &notes, &url, &sha256, sender_for_btn.clone())),
    );
}

fn content(
    latest: &str,
    notes: &str,
    url: &str,
    sha256: &str,
    sender: Option<Sender<UiMsg>>,
) -> Element {
    let body = signal(notes.to_string());
    let url = url.to_string();
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
        if let Err(e) = crate::update::task::start_download(&sender, &url, &sha256) {
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
        .child(Element::scroll().weight(1.0).child(text))
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
