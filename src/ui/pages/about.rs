//! 关于页面：软件信息 + 版本更新。
//!
//! 更新按钮不直接联网——它们只把意图交给 [`crate::update::task`] 的后台线程，结果经
//! [`UiMsg::Update`] 回到 `Runtime`，再由这里绑定的信号把状态行/进度条刷新出来。
//! 面板因此永远不会被 8~10 秒的网络等待冻住。
//!
//! 本页拿的是 [`UpdateState`] + 发送端这两个**值**，不认识 `Runtime`（依赖倒置，
//! 见 [`crate::ui::PanelHost`]）。
//!
//! ★ 2026-09-27 用户口径：软件信息卡的文本是**定稿文案**（`CurSeen` / Copyright /
//! 许可证 / Gitee 仓库 / GitHub 镜像），仓库地址是可点击链接；「更新日志」卡删除——版本变更史交给
//! Gitee Release，面板里手抄一份必然与线上脱节。

use windui::prelude::*;

use crate::core::events::UiMsg;
use crate::ui::tokens;
use crate::ui::tokens::to_color;
use crate::update::state::UpdateState;
use crate::update::task;

/// 官方仓库主页（Gitee，主仓 + 更新源）。**唯一来源**：本页的"Gitee仓库"标签与
/// 可点击链接都读它，改地址只改这一处（两条文本各写一份必然走偏）。
const HOMEPAGE: &str = "https://gitee.com/qiuzongman/curseen";

/// GitHub 镜像仓库（只读镜像，不提供更新下载——更新包唯一来源是 Gitee，
/// 见 `update::checker` 的域名白名单）。2026-10-08 用户建仓后补上。
const GITHUB_MIRROR: &str = "https://github.com/zongmanqiu/CurSeen";

/// 赞助二维码（真码）。**源是矢量**：`assets/images/WeChatPay.svg` / `ALiPay.svg`；
/// 这里内嵌的 PNG 是 `cargo run --example gen_icon --features icon-gen` 的**渲染产物**
/// （渲染时补了 4 模块白边作静默区——源 SVG 的模块铺满 viewBox，没有静默区，
/// 扫码器贴着背景时识别率会掉）。★ 换二维码 = 覆盖那两个 SVG 再跑该命令并重新编译。
///
/// ★ 为什么源用矢量而产物是位图：二维码是**编码数据**，模块必须落在像素网格上，
/// 位图缩放/抗锯齿会让模块边界发虚、拉低识别率；矢量则能按显示尺寸精确渲染，
/// 高 DPI（面板 160 逻辑 px 在 200% 下要 320 物理 px）也不糊。
const DONATE_WECHAT_PNG: &[u8] = include_bytes!("../../../assets/images/WeChatPay.png");
const DONATE_ALIPAY_PNG: &[u8] = include_bytes!("../../../assets/images/ALiPay.png");

/// 二维码显示边长（px）。照抄 Pecia 的 `AboutPanel::kQrSize = 160`：手机扫码余量充足，
/// 两张并排也不挤。
const DONATE_QR_SIZE: i32 = 160;

/// 两张二维码的间距。照抄 Pecia 的 `kQrGap = 28`——刻意留宽，避免瞄着一个码扫时
/// 手机误识别另一个。
const DONATE_QR_GAP: i32 = 28;

/// 二维码外白框厚度。照抄 Pecia 的 `kQrBorder = 2`：码的白画布和卡片浅灰底（246,248,250）
/// 挨太近会看不出边界，套一圈纯白让"这是一张码"有明确的轮廓。
const DONATE_QR_BORDER: i32 = 2;

/// 构建关于页面。
pub fn build_about_page(up: UpdateState, sender: Option<Sender<UiMsg>>) -> Element {
    let mut content = Element::col();

    // ── 软件信息 ──
    //
    // 标题行：中英双语名「CurSeen 可寻」（2026-10-08 用户定稿：英文名 + 中文名谐音
    // Seek→寻）整体用 FONT_DISPLAY（2026-09-27 用户口径"要大一点"），后跟正常字号
    // 的版本号——版本取编译期 `CARGO_PKG_VERSION`，与 Cargo.toml 永远同源，不会手抄漂移。
    //
    // 仓库地址走 `Element::link`：链接色 + 下划线 + 悬停手型，点击由 windui 的
    // `EventCtx::open_url` 交平台层 `ShellExecuteW` 用系统默认浏览器打开——不自己拼
    // `Command::new("explorer")`，那是"碰运气"通路（有默认浏览器被改过的机器会开错）。
    //
    // `.spacing(10)` 是行距（2026-09-27 用户反馈：不带间距的四行挤成一坨）。
    let title_row = Element::row()
        .child(
            Element::label("CurSeen 可寻")
                .font_size(tokens::FONT_DISPLAY)
                .fg(to_color(tokens::TEXT)),
        )
        .child(
            Element::label(env!("CARGO_PKG_VERSION"))
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY)),
        )
        .cross(Align::End)
        .spacing(10);

    let homepage_row = Element::row()
        .child(
            Element::label("Gitee仓库：")
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY)),
        )
        .child(
            Element::link(HOMEPAGE)
                .url(HOMEPAGE)
                .font_size(tokens::FONT_MD),
        )
        .cross(Align::Center);

    let github_row = Element::row()
        .child(
            Element::label("GitHub镜像：")
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY)),
        )
        .child(
            Element::link(GITHUB_MIRROR)
                .url(GITHUB_MIRROR)
                .font_size(tokens::FONT_MD),
        )
        .cross(Align::Center);

    content = content.child(
        Element::col()
            .child(title_row)
            .child(
                // ★ 广告语（2026-10-08 定稿，与 README 主标题同源）：副标题位，
                //   次级色 + 与正文同字号——只降层级，不抢主标题。
                Element::label("键鼠所至 · 屏幕所书 · 不止所见")
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
            )
            .child(
                // ★ 版权符号用 `©`（U+00A9）而不是 `(C)`：面板与 README 同一写法，
                //   且 `©` 是法定标记的通行写法——此前这里写成两个空格，是符号被
                //   漏掉的痕迹（2026-09-28 修正）。
                Element::label("Copyright © 2026 邱宗满")
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
            )
            .child(
                Element::label("许可证：AGPL-3.0")
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
            )
            .child(homepage_row)
            .child(github_row)
            .cross(Align::Start)
            .spacing(10)
            // ★ 卡不撑满（2026-09-27 用户口径定稿）：文字等内容靠左，背景色块按内容
            //   所需宽度收缩——本页三张卡都不加 `.width_match()`。此前为修"一分为二"
            //   加过撑满，用户实测后改要 Pecia 式的紧凑靠左观感。
            // ★ 顶部内 padding = `PAGE_TOP_MARGIN`（2026-09-27 二轮"顶部间隔仍不同"）：
            //   本卡底色与页面白底几乎不可分辨，视觉上第一个"看得见的元素"是卡内
            //   文字——padding(16) 把它压到 y≈66，而卡片页标题色带内的文字只偏
            //   ~2px（y≈53）。色带/卡底的**边缘**七页早已统一（y=51），差的是
            //   **文字锚点**。顶 padding 收到 4 后文字 y≈55，与卡片页带内文字对齐；
            //   其余三边仍 16。
            .padding_edges(windui::geometry::Insets {
                left: 16,
                top: tokens::PAGE_TOP_MARGIN,
                right: 16,
                bottom: 16,
            })
            .bg(to_color(tokens::CARD_BG))
            .corner(tokens::CORNER_RADIUS)
            // ★ 首卡顶部间隔与卡片页同源（`tokens::PAGE_TOP_MARGIN`，2026-09-27 用户
            //   口径"每个标签页首个 UI 元素距顶的间隔都要相同"）：此前是 16，比卡片页
            //   标题条的 4 多 12px（真机实测色带 y=51 vs 本卡 y=63）。只改 top——
            //   bottom 16 仍是卡间距。
            // ★ 卡间距（2026-09-28 用户口径："三个内容块之间的间隔太大"）：此前卡间
            //   16+16=32px，是页面上下边距（16px）的两倍，观感失衡。收敛为**卡间 16px**
            //   （相邻两卡各出 8）——与滚动容器的 16px 边距同量级；末卡 bottom=0，
            //   页底只剩容器 padding 16。
            .margin_edges(windui::geometry::Insets {
                left: 0,
                top: tokens::PAGE_TOP_MARGIN,
                right: 0,
                bottom: 8,
            }),
    );

    // ── 版本更新 ──
    let check_btn = {
        let sender = sender.clone();
        Element::button("检查更新")
            .outline()
            // 正在检查/下载时置灰：网络请求是"发一次就够了"的动作，连点会开好几条线程。
            .enabled_signal(up.busy.map(|b| !*b))
            .on_click(move |_ctx| {
                if let Err(e) = task::start_check(&sender) {
                    log::warn!("发起检查更新失败: {e}");
                }
            })
    };

    let install_btn = {
        let sender = sender.clone();
        // ★ 地址在**点击那一刻**才读：`update.url` 是"检查完之后才填入"的，若在建树时
        //   捕捉就会永远是空串（面板刚打开时必然还没检查过）。`UpdateState` 是可拥有的
        //   值，直接进闭包即可。
        let flag = up.clone();
        // 只有"拿到下载地址且不忙"时才可点：否则用户点到的只是个必然报错的按钮。
        // 这里用 `enabled_when`（每帧读一次）而不是信号映射——两个信号（ready/busy）
        // 做逻辑与没有现成的组合子，逐帧求值正是想要的语义。
        let gate = flag.clone();
        Element::button("下载并更新")
            .enabled_when(move || gate.ready.get() && !gate.busy.get())
            .on_click(move |_ctx| {
                if let Err(e) = task::start_download(&sender, &flag.url()) {
                    log::warn!("发起下载失败: {e}");
                }
            })
    };

    let progress = Element::progress(up.progress)
        // 没有进度（-1）时不显示进度条：只在真的开始下载后才出现。
        .visible_signal(up.progress.map(|p| *p >= 0.0));

    content = content.child(
        Element::col()
            .child(
                Element::label("版本更新")
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT)),
            )
            .child(
                Element::row()
                    .child(check_btn)
                    .child(Element::leaf().width(8))
                    .child(install_btn)
                    .cross(Align::Center)
                    .margin_xy(0, 12),
            )
            .child(
                Element::label_signal(up.status)
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
            )
            .child(progress.margin_xy(0, 12))
            .padding(16)
            .bg(to_color(tokens::CARD_BG))
            .corner(tokens::CORNER_RADIUS)
            // 卡间 16px（与本卡上缘 8 + 首卡下缘 8 凑成，见首卡 margin 注释）。
            .margin_edges(windui::geometry::Insets {
                left: 0,
                top: 8,
                right: 0,
                bottom: 8,
            }),
    );

    // ── 赞助 ──
    // 布局照抄 Pecia 的 AboutPanel::draw()：**说明字在图的正上方**（不是下方）、
    // 两码并排居中、间隔 kQrGap、每码外套一圈 kQrBorder 厚的白框。
    let qr_block = |bytes: &'static [u8], caption: &str| {
        Element::col()
            .child(
                Element::label(caption)
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT)),
            )
            .child(
                // 白框：`padding(2) + 纯白底`，码的白画布与卡片浅灰底之间就有了轮廓。
                Element::image_bytes(bytes)
                    .width(DONATE_QR_SIZE)
                    .height(DONATE_QR_SIZE)
                    .fit(Fit::Contain)
                    .padding(DONATE_QR_BORDER)
                    .bg(to_color([255, 255, 255])),
            )
            .cross(Align::Center)
            .spacing(8)
    };

    let wechat_block = qr_block(DONATE_WECHAT_PNG, "微信");
    let alipay_block = qr_block(DONATE_ALIPAY_PNG, "支付宝");

    content = content.child(
        Element::col()
            .child(
                Element::label("赞助")
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT)),
            )
            .child(
                Element::row()
                    .child(wechat_block)
                    .child(Element::leaf().width(DONATE_QR_GAP))
                    .child(alipay_block)
                    .cross(Align::Start)
                    .margin_xy(0, 12),
            )
            // ★ 卡不撑满、内容靠左（2026-09-27 用户口径定稿）：卡宽 = 两码实际宽度
            //   （164×2 + 间隔 28 + 自身 padding 32 = 388px），"赞助"标题与码组都靠左，
            //   与前两张卡同一套对齐口径。qr_block 内部的 `cross(Align::Center)` 只管
            //   "微信/支付宝"标签在码宽内居中，保持不变。
            .padding(16)
            .bg(to_color(tokens::CARD_BG))
            .corner(tokens::CORNER_RADIUS)
            // 卡间 16px 的下半（8）；bottom=0：页底只剩滚动容器 padding 16，
            // 与页面顶边距同量级（2026-09-28 卡间距收敛）。
            .margin_edges(windui::geometry::Insets {
                left: 0,
                top: 8,
                right: 0,
                bottom: 0,
            }),
    );

    // 滚动容器。★ content 保留 `width_match()`：让内边距与卡片左缘稳定在视口上，
    // 三张卡自身按内容宽收缩（都不加 Match）、在整宽的 content 里靠左（col 默认
    // `Align::Start`；2026-09-27 用户口径定稿：文字靠左、色块按所需宽度显示）。
    Element::scroll().child(content.padding(16).width_match())
}
