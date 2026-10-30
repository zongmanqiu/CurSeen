//! 脚本用的**宿主文本窗**（m08）：系统标题栏 + 一片记事本式的文本区域。
//!
//! # 形状为什么长这样（2026-10-27 用户口径，两轮改判）
//!
//! 旧通路是工作线程直接弹原生框（`platform::win_util::text_result_box`），控件长相由系统决定，
//! 与主窗口不是一套。第一版宿主窗做成"自绘标题带 + 圆角输入框 + 底部按钮"，用户看完真窗说：
//!
//! > 「文本框为什么不是圆角？主窗口 UI 是圆角的，可文本框边框不是，反而内部文本区域是圆角，
//! > 这部分需要清除。或者标题栏就用 windui 的标题栏，名称最好是脚本的名字（图片信息）；
//! > 内部文本区域应当是直角、甚至没有角、没有按钮，仅仅只是区域，就像记事本那样。」
//!
//! 于是定成现在这样：
//!
//! · **标题栏用系统的**，标题文字 = 脚本给的名字（「图片体检」/「OCR」）——
//!   任务栏、Alt+Tab、窗口本身都认得它，用户不用猜这扇窗是谁；
//! · **正文是一片 `.frameless()` 的文本区域**：windui 的输入控件在 frameless 下**不画圆角底、
//!   不画边框**（`ui/inputs.rs` 里那段 `if !self.config.frameless`），正好就是"直角、没有边框、
//!   只是区域"；
//! · **没有按钮**：选中后 Ctrl+C / Ctrl+A 直接可用（windui 建窗时注入平台剪贴板，
//!   `app/mod.rs` 的 `tree.clipboard = Some(Clipboard)`），关窗走标题栏 X 或 Esc——
//!   记事本怎么用，它就怎么用；
//! · 只有 `ext:input`（要回一个值给脚本）那一档保留底部「确定 / 取消」，因为没有按钮就没法提交。
//!
//! # 为什么只做"一次性模态"，不做控件树
//!
//! windui 的窗只能在 UI 线程开，而 `Runtime` 是 `Rc<RefCell<_>>`、非 `Send`
//! （见 [`crate::core::events`] 模块头）。脚本每次触发都在自己的工作线程上，跨线程能送的
//! 只有**纯数据**，送不了 Lua 函数。所以窗内事件全在 Rust 侧闭合，用户点「确定」时把值
//! 经通道一次性回给工作线程——不需要"signal + 回调进 Lua"那座桥，也没有
//! "脚本已退出但窗还开着"的生命周期问题。常驻 / 自定义界面是另一档能力（外部程序）。
//!
//! # 高度与裁切
//!
//! 上一版把窗高靠**手加常量**算，footer 少算 18px → 整棵树比客户区高，windui 从尾部裁，
//! 用户看到"按钮被遮挡"。现在文本区 `.weight(1.0)` 吃剩余空间，被压缩的永远是区域，
//! 不是尾部的按钮行；判据也不再信常量，改成读 windui 真实排版的节点边界（见本文件测试）。

use std::rc::Rc;
use std::sync::Arc;

use windui::core::EventCtx;
use windui::prelude::*;

use super::tokens;
use super::tokens::to_color;
use crate::core::events::ScriptDialogSpec;

/// 窗宽（**逻辑像素**；`Window::position` 收的是物理像素，那条混用口径在
/// `runtime::overlay::open_text_input` 已确立，这里跟随，不自创第二套）。
/// `pub` 是给预览 example 用的——它要按同一个宽度排版才能拍出真实效果。
pub const DIALOG_W: i32 = 520;
/// 内边距（只用在输入档的按钮行；信息档是记事本式的一片区域，不留边）。
const PAD: i32 = 12;
/// 输入档底部按钮区的高度预算：按钮自然高 + 它自己那层容器的上下内边距。
///
/// ★ 这个数**不参与裁切判据**——按钮行固定、文本区吃剩余（`.weight(1.0)`），
///   窗高估多估少都只会改到文本区。它只用来决定"窗开多高"。
const FOOT_H: i32 = 58;
/// 一行的估算高度（只用于估窗高；字号只有一档，见 [`tokens`] 的字号纪律）。
const LINE_H: i32 = 22;
/// 输入档文本区的期望高度（沿用标注输入窗实测过的三行高）。
const INPUT_BODY_H: i32 = 72;
/// 正文区高度的上下限：再长的内容也交给区内滚动，窗不越过一屏。
const BODY_MIN_H: i32 = 120;
const BODY_MAX_H: i32 = 460;
/// 窗高下限：低于这个数，文本区连一行都看不全。
///
/// ★ windui 这一版**没有** `Element::min_height`，所以"区域不被压扁"只能靠窗高下界保证。
const WINDOW_MIN_H: i32 = 160;
/// 可缩放窗的最小尺寸（用户拖小也要能读一行）。
const MIN_W: i32 = 320;
const MIN_H: i32 = 120;

/// 按钮行为：窗内的每颗按钮都是"UI 线程上的一个闭包"。
///
/// 用 `Rc<dyn Fn>` 而不是泛型闭包：内容树可能被反复重建，处理器必须**可重复调用**，
/// 而 `Rc` 让同一份行为能塞进多个子节点，也能被预览 example 塞一份空操作进去——
/// 生产与预览因此共用同一个正本（复刻一份私有函数的那种 example 永远不会红，不算验证）。
type Click = Rc<dyn Fn(&mut EventCtx)>;

fn inert() -> Click {
    Rc::new(|_ctx| {})
}

/// 一个可见行按宽度会折成几行：CJK 一字 ≈ 字号宽，ASCII 取半宽。
///
/// 只用于**估高**，不参与排版——真正的折行由文本框自己做。宁可估多一点：估少了窗矮、
/// 内容被滚动藏住，比窗高出一点更难被发现。
fn visual_lines(text: &str) -> usize {
    // 文本区可用宽度（逻辑像素）：信息档整幅都是它，输入档左右各留一层内边距。
    const AVAIL_PX: f32 = (DIALOG_W - 2 * PAD) as f32;
    let per_char = tokens::FONT_MD; // CJK 一字 ≈ 14px
    let mut total = 0usize;
    for line in text.split('\n') {
        let px: f32 = line
            .chars()
            .map(|c| {
                if (c as u32) >= 0x3000 {
                    per_char
                } else {
                    per_char * 0.55
                }
            })
            .sum();
        total += ((px / AVAIL_PX).ceil() as usize).max(1);
    }
    total
}

/// 文本框的**期望**高度：按行数实算，钳在上下限之间。
///
/// 只是"窗开多高"的依据——真正的分配由 `.weight(1.0)` 决定（见 [`content_tree`]），
/// 所以这个数估偏了不会裁掉按钮。
fn desired_body(text: &str, editable: bool) -> i32 {
    if editable {
        return INPUT_BODY_H;
    }
    (visual_lines(text) as i32 * LINE_H + 6).clamp(BODY_MIN_H, BODY_MAX_H)
}

/// 整窗（客户区）高度。信息档就是那片区域本身，所以不加边距与页脚。
pub fn height_of(text: &str, editable: bool) -> i32 {
    let body = desired_body(text, editable);
    let h = if editable { body + FOOT_H } else { body };
    h.clamp(WINDOW_MIN_H, 640)
}

/// 窗的内容树。生产（[`open`]）与预览 example 共用这一份。
///
/// · **信息档**（`editable=false`）：整幅就是一片文本区，没有标题带、没有按钮。
/// · **输入档**（`editable=true`）：文本区吃剩余空间 + 底部「确定 / 取消」（没按钮就没法提交）。
pub fn content_tree(
    editable: bool,
    text: Signal<String>,
    on_confirm: Click,
    on_close: Click,
) -> Element {
    let mut area = Element::text_input(text, "")
        .multiline()
        .wrap(true)
        .font_size(tokens::FONT_MD)
        // ★ 记事本式的一片区域：windui 的输入控件在 frameless 下不画圆角底、不画边框
        //   （`ui/inputs.rs` 的 `if !self.config.frameless` 那一段），正是"直角、无边框、
        //   只是区域"。
        .frameless()
        // 文字不贴窗边：记事本自己也留了这一线。
        .padding_xy(8, 6);
    // 视图起点必须显式管，否则 windui 会把**光标**滚进可视区：程序塞进去的文本光标默认在
    // 末尾，开窗看到的就是最后几行（实测像素如此）。
    // · 输入档：抢焦点 + 全选（原语义是替换掉旧值，全选正好）；
    // · 信息档：不抢焦点（一敲键盘就把要抄的内容打没），光标钉在 0 号位 → 视图从头开始。
    area = if editable {
        area.autofocus_select_all()
    } else {
        area.select_range(0, 0)
    };

    if !editable {
        return area.fill();
    }
    let bottom = Element::row()
        .spacing(8)
        .align(Align::End)
        .child(Element::button("确定").on_click(move |ctx| on_confirm(ctx)))
        .child(
            Element::button("取消")
                .neutral()
                .on_click(move |ctx| on_close(ctx)),
        );
    // **吃剩余 / 固定**：被压缩的永远是文本区，不是尾部的按钮行。
    // ★ `width_match` 必须显式给：windui 的 `TextInput::measure` 恒返回固定 160px 宽
    //   （inputs.rs 里那条老口径），不撑满就是一条窄窄的框贴在左上。
    Element::col()
        .fill()
        .child(area.width_match().weight(1.0))
        .child(Element::col().padding_xy(PAD, PAD).child(bottom))
}

/// 落点：**触发它的那块屏的正中**（2026-10-28 用户口径："文本信息窗口最好是居中吧"）。
///
/// 以前是"贴着触发它的那张贴图右下"——设计意图是"窗要开在手边"，但贴图常被摆在屏幕
/// 右半（用户往往把参考图放右边），结果每次结果窗都贴着右缘冒出来，既不居中也不好读。
/// 现在按 **owner 所在那块显示器的工作区**（去掉任务栏）居中；拿不到 owner 就用主屏。
/// 单位为物理像素（`Window::position` 的坐标系，副屏在左时 x 可为负）。
fn place(spec: &ScriptDialogSpec, w: i32, h: i32) -> (i32, i32) {
    let (ax, ay, aw, ah) = crate::platform::win_util::monitor_work_rect(spec.owner_hwnd)
        .unwrap_or_else(|| {
            // 回退：整块虚拟桌面（多屏拼起来）——总比 0,0 好，且仍然居中
            let (vx, vy, vr, vb) = crate::platform::win_util::virtual_screen_rect();
            (vx, vy, vr - vx, vb - vy)
        });
    let (aw, ah) = (aw.max(1), ah.max(1));
    let x = ax.saturating_add((aw - w) / 2);
    let y = ay.saturating_add((ah - h) / 2);
    // 钳进那块工作区：小屏 / 窗比屏还大时，至少左上角与标题栏可见
    let max_x = ax.saturating_add((aw - w).max(0));
    let max_y = ay.saturating_add((ah - h).max(0));
    (x.clamp(ax, max_x), y.clamp(ay, max_y))
}

/// 开一扇脚本文本窗（**UI 线程**调用——由 `UiMsg::ScriptDialog` 派发，见 `runtime::overlay`）。
pub fn open(ctx: &mut EventCtx, spec: Arc<ScriptDialogSpec>) {
    let editable = spec.editable;
    let body = if editable {
        spec.initial.clone()
    } else {
        spec.body.clone()
    };
    let h = height_of(&body, editable);
    let (x, y) = place(&spec, DIALOG_W, h);
    log::info!(
        "脚本文本窗「{}」{} → @ ({x},{y}) {DIALOG_W}x{h}（{} 行）",
        spec.title,
        if editable { "输入" } else { "信息" },
        body.lines().count()
    );

    let text = signal(body);
    let spec_close = spec.clone();
    ctx.open_window(
        // ★ 标题 = 脚本给的名字（系统标题栏 / 任务栏 / Alt+Tab 都显示它，用户一眼认得出是谁）。
        //   也因此**不再**把窗标题登记进 `TopmostKeeper::guard_aux`：那道守护按标题找窗，
        //   动态标题守不住。不守也不会被压住——本窗 `.owned(true)` 挂在覆盖层名下，
        //   Win32 里 owned 窗恒在其 owner 之上，而覆盖层自己正被守护抬着。
        Window::new(&spec.title, DIALOG_W, h)
            .owned(true)
            .resizable(true)
            .min_size(MIN_W, MIN_H)
            .position(x, y)
            .bg(to_color(tokens::BG))
            .on_close_request(move |_ctx| {
                // 关窗 = 取消 / 窗被关掉。已经回过值（点了「确定」）时这里是空操作——
                // `reply` 只生效一次，不必分辨是哪条路。
                spec_close.reply(None);
                true
            })
            // Esc 在这里**允许**关窗（走上面的 `on_close_request` = 取消）。标注输入窗当年
            // 特意吞掉 Esc，是因为连按两下、第二下会落到画板把整个标注退出；独立文本窗没有
            // 那个连带风险。
            .content(move || {
                let spec_ok = spec.clone();
                let text_for_confirm = text; // `Signal<String>` 是 Copy，不用 clone
                let on_confirm: Click = Rc::new(move |ctx: &mut EventCtx| {
                    // 空串也算一次有效回答（与旧 `text_input_box` 一致：是否作废由脚本自己判）。
                    spec_ok.reply(Some(text_for_confirm.get()));
                    ctx.request_close();
                });
                let on_close: Click = Rc::new(|ctx: &mut EventCtx| ctx.request_close());
                content_tree(editable, text, on_confirm, on_close)
            }),
    );
}

/// 给预览 example 用的一对空操作（内容树与生产同一份，行为换成什么都不做）。
pub fn inert_click() -> Click {
    inert()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windui::core::Tree;
    use windui::text::NullTextEngine;

    /// 把生产的内容树离屏排版一次，返回（根节点边界，各子节点边界）。
    ///
    /// 走的是 windui 真实布局（`Tree::layout_root`），不是我们自己算的数——**"按钮被遮挡"
    /// 这类缺陷只有排版结果能证明**，手加的常量只能证明公式自洽。
    fn laid_out(editable: bool, text: &str, win_h: i32) -> (Rect, Vec<Rect>) {
        let root = content_tree(editable, signal(text.to_string()), inert(), inert());
        let mut tree = Tree::new();
        let id = root.build(&mut tree);
        tree.root = Some(id);
        tree.layout_root(Size::new(DIALOG_W, win_h), &mut NullTextEngine);
        let node = tree.get(id).expect("根节点");
        let kids = node
            .children
            .iter()
            .map(|k| tree.get(*k).expect("子节点").bounds)
            .collect();
        (node.bounds, kids)
    }

    /// 窗高按内容实算：行数越多越高，到顶就交给区内滚动，不越过一屏。
    #[test]
    fn height_follows_line_count_and_caps() {
        let one = height_of("尺寸：1920 × 1080 像素", false);
        let eight = height_of(
            "尺寸：1 × 1 像素\n宽高比：1:1\n像素量：0.00 MP\n朝向：正方形\n平均色：#808080\n平均通道：R=1 G=2 B=3\n平均不透明度：100%\n采样：1 点",
            false,
        );
        let huge = height_of(&"很长的一段中文".repeat(200), false);
        assert!(one < eight, "八行应当比一行高：{one} !< {eight}");
        assert_eq!(
            huge, BODY_MAX_H,
            "超长内容必须撞到高上限，多出来的交给区内滚动"
        );
        // 输入档的期望高 = 文本区 + 页脚，但**不低于窗高下限**（低于就按下限开，
        // 多出来的那几像素由 `.weight(1.0)` 补给文本区，不会去动按钮行）。
        assert_eq!(
            height_of("x", true),
            (INPUT_BODY_H + FOOT_H).max(WINDOW_MIN_H)
        );
    }

    /// 折行估算：一行超长要算成多行（否则窗按"1 行"开矮框，内容全被滚动藏住）。
    #[test]
    fn long_single_line_counts_as_several_visual_lines() {
        let short = visual_lines("短");
        let long = visual_lines(&"很长很长很长的一段中文内容".repeat(20));
        assert!(
            long > short * 4,
            "长行应当被估成多行：short={short} long={long}"
        );
        assert_eq!(visual_lines(""), 1, "空串至少算一行，不能估成零高");
    }

    /// ASCII 比 CJK 窄：同样字符数的两段，CJK 那段应当更高。
    ///
    /// 取 100 字而不是 60 字：60 个汉字（840px）与 60 个字母（约 462px）在可用宽里都只折成
    /// 2 行，差值被"向上取整"吃掉了——那种样本证明不了宽度模型。
    #[test]
    fn cjk_is_wider_than_ascii() {
        let cjk = visual_lines(&"很".repeat(100));
        let ascii = visual_lines(&"a".repeat(100));
        assert!(
            cjk > ascii,
            "100 个汉字应比 100 个字母占更多行：{cjk} !> {ascii}"
        );
    }

    /// 合并后的「图片信息」一次给全所有字段：八行的区域必须容得下八行。
    #[test]
    fn merged_output_fits_all_lines() {
        let merged = "尺寸：3840 × 2160 像素\n宽高比：16:9（约 1.778）\n像素量：8.29 MP\n朝向：横图\n平均色：#808080\n平均通道：R=128  G=128  B=128\n平均不透明度：100%\n采样：40000 点（步长 4）";
        assert_eq!(visual_lines(merged), 8, "八行短句不该再折行");
        assert_eq!(
            desired_body(merged, false),
            8 * LINE_H + 6,
            "区域高度必须按行数实算，多一行就多一行"
        );
        assert!(height_of(merged, false) > height_of("尺寸：1 × 1 像素", false));
        assert!(
            !merged.contains('…'),
            "整段原文就是内容，不许出现截断省略号"
        );
    }

    /// ★ 信息档**没有按钮**：整窗就是那片区域，边界必须铺满客户区（记事本式）。
    #[test]
    fn info_mode_is_one_bare_area_filling_the_window() {
        let text = "尺寸：1920 × 1080 像素";
        let h = height_of(text, false);
        let (root, kids) = laid_out(false, text, h);
        assert!(
            kids.is_empty(),
            "信息档不该有子节点（没有标题带、没有按钮行）"
        );
        assert_eq!(root.w, DIALOG_W, "区域宽度必须铺满客户区");
        assert_eq!(root.h, h, "区域高度必须铺满客户区");
    }

    /// ★ 输入档：按钮行的下边界**不许越过**客户区底边；被压缩的只能是文本区。
    #[test]
    fn input_buttons_stay_inside_even_a_very_short_window() {
        let long = &"很长的一段中文内容用来把折行数撑起来".repeat(40);
        let (root, kids) = laid_out(true, long, height_of(long, true));
        assert_eq!(kids.len(), 2, "输入档必须是「文本区 + 按钮行」两段");
        let foot = kids[1];
        assert!(
            foot.y + foot.h <= root.h,
            "按钮行越界：下边界 {} > 客户区 {}",
            foot.y + foot.h,
            root.h
        );
        // 故意压到 150 的极矮窗：按钮行仍要在里面，牺牲的是文本区。
        let (_, kids2) = laid_out(true, long, 150);
        let foot2 = kids2[1];
        assert!(
            foot2.y + foot2.h <= 150,
            "窗被压到 150 时按钮行仍被裁：下边界 {}",
            foot2.y + foot2.h
        );
        // 被压缩的只能是文本区：同一档在"正常高"与"极矮窗"下，按钮行的高度必须一致。
        assert_eq!(
            foot.h, foot2.h,
            "按钮行高度不该随窗高变化（被压缩的只能是文本区）：{} vs {}",
            foot.h, foot2.h
        );
        assert!(
            kids2[0].h < kids[0].h,
            "矮窗下文本区应当变矮：{} !< {}",
            kids2[0].h,
            kids[0].h
        );
    }
}
