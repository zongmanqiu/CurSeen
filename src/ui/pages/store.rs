//! 设置页「官方资源下载」卡（m37；m38 按 RuPix「解码器 → 可下载」逐件复刻）。
//!
//! # 版式（对齐 RuPix `settings/decoders.rs::download_body`）
//!
//! ```text
//! [ 搜索名称或说明                              ]   ← 40px 行，输入框吃满
//! （空 8px）
//! [ 3 个可下载 · 1 个可升级 ]        Gitee  [刷新]   ← 红字状态 + 灰字来源 + 刷新
//! （空 8px）
//! 名称（吃剩余、单行省略）      v1.0.0   [!] [下载] ← 版本格 64、动作槽 60
//! …
//! （搜索无结果时）[（没有匹配的资源包）]
//! ```
//!
//! ★ **动作只有三种外观**（RuPix 同款）：可装 = 蓝「下载」、可升级 = 橙「升级」、
//!   无事可做 = 灰「下载」（`disabled`，仍占满 60px 动作槽，右缘不塌）。
//!   下载中那一枚的按钮当场变百分比（`42%`），其它行不动。
//! ★ **卸载只在「!」详情窗里**（用户口径：下载以后信息界面多一个卸载就行）——
//!   行里没有第二个动作。
//! ★ 没有开关：下载来的脚本在脚本页各有自己的卡片与开关，这里只管"获取 / 移除"。
//! ★ 2026-10-31（m69）：来源下拉搬去设置页「高级」卡——下载源是商店与本体更新共用的
//!   全局设置，控件只该有一处；本模块保留控件正本 [`source_dropdown`]，商店卡不再摆它。

use windui::core::EventCtx;
use windui::prelude::*;

use crate::core::events::{StoreAction, StorePackRow, UiMsg};
use crate::scripting::store::{version_newer, SOURCE_LABELS};
use crate::ui::card::{self, FeatureCard};
use crate::ui::store_state::StoreState;
use crate::ui::tokens::{self, to_color};

/// 版本格宽度（RuPix `VERSION_COL_W`）：各版本串长短不一，钉死它自己的宽，后面的「!」不动。
const VERSION_COL_W: i32 = 64;
/// 行尾动作槽宽度（两个汉字按钮的自然宽；灰档也占满，右缘才齐）。
const ACTION_COL_W: i32 = 60;
/// 来源下拉宽度（`Gitee` / `GitHub` 都放得下）。
const SOURCE_W: i32 = 108;
/// 控件高（搜索框 / 下拉 / 按钮同一条线）。
const CONTROL_H: i32 = 30;
/// 段间空档（搜索行 ↔ 状态行 ↔ 列表）。
const GAP: i32 = 8;

/// 行尾动作的三种外观（RuPix `ActionTone`）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ActionTone {
    /// 可装：主题主色（蓝）。
    Primary,
    /// 可升级：主题警告色（橙）。
    Warning,
    /// 无事可做：置灰、点不动，但占的格子分毫不差。
    Idle,
}

/// 统一按钮：字号 14、控件圆角（与面板里其它按钮同源）。
fn btn(label: impl Into<TextContent>) -> Element {
    Element::button(label)
        .font_size(tokens::FONT_MD)
        .corner(tokens::CORNER_CONTROL)
}

/// 行尾动作按钮：蓝 / 橙 / 灰三档。
fn btn_action(label: impl Into<TextContent>, tone: ActionTone) -> Element {
    let b = btn(label);
    match tone {
        ActionTone::Primary => b,
        ActionTone::Warning => b.intent(Intent::Warning),
        ActionTone::Idle => b.disabled(true),
    }
}

/// 固定宽度、右对齐的动作槽（灰档也占满，整列右缘不塌）。
fn action_slot(control: Element) -> Element {
    Element::row()
        .width(ACTION_COL_W)
        .cross(Align::Center)
        .child(Element::flex_spacer())
        .child(control)
}

/// 版本格：钉死宽 + 右对齐 + 灰字（RuPix 同款）。
fn version_cell(version: &str) -> Element {
    Element::label(format!("v{version}"))
        .font_size(tokens::FONT_MD)
        .fg(to_color(tokens::TEXT_SECONDARY))
        .width(VERSION_COL_W)
        .text_align(Align::End)
        .max_lines(1)
        .truncate(Truncate::End)
}

/// 标准搜索框（RuPix 同款包一层 col：windui 水平容器的 `width_match` 会降级成按内容宽）。
///
/// 放大镜：启动时把 Segoe MDL2 注册成私用区回退字体，这里用 **E71E**（原生"圆圈左、
/// 手柄右下"的常规方向，与 RuPix 镜像 E721 后同形）。
fn search_box(filter: Signal<String>) -> Element {
    Element::col().fill().child(
        Element::text_input(filter, "搜索名称或说明")
            .leading_icon('\u{E71E}')
            .width_match(),
    )
}

/// 行尾动作的（文案, 色调）——纯函数，把"只有三种状态"钉成表。
///
/// | 情形 | 结果 |
/// |---|---|
/// | 选中镜像没有地址 / `min_host` 高于宿主 | 下载 · 灰 |
/// | 没装 | 下载 · 蓝 |
/// | 装了旧版 | 升级 · 橙 |
/// | 已是最新 | 下载 · 灰 |
fn action_of(row: &StorePackRow, source: usize) -> (&'static str, ActionTone) {
    let bit = 1u8 << source.min(SOURCE_LABELS.len() - 1);
    let has_url = row.source_mask & bit != 0;
    if row.too_new || !has_url {
        return ("下载", ActionTone::Idle);
    }
    match &row.installed_version {
        Some(v) if version_newer(&row.version, v) => ("升级", ActionTone::Warning),
        Some(_) => ("下载", ActionTone::Idle),
        None => ("下载", ActionTone::Primary),
    }
}

/// 搜索命中判据（纯函数）：名称 / 标识 / 版本 / 说明。
///
/// 不搜大小与文件数——那是核对时才看的数字，拿它当搜索键只会得到"输入 1 全中"的噪音。
fn remote_matches(row: &StorePackRow, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    row.name.to_lowercase().contains(&q)
        || row.id.to_lowercase().contains(&q)
        || row.version.to_lowercase().contains(&q)
        || row.description.to_lowercase().contains(&q)
}

/// 来源名（越界夹紧到最后一路）：下拉与灰字提示共用，别让下标越界。
pub(crate) fn source_name(index: usize) -> &'static str {
    SOURCE_LABELS[index.min(SOURCE_LABELS.len() - 1)]
}

/// 下载源下拉（Gitee / GitHub）。
///
/// 控件正本留在本模块（来源标签表只有 `SOURCE_LABELS` 一份），摆放位置在设置页
/// 「高级」卡：下载源是商店与本体更新共用的全局设置，不该跟着某一张卡走。
pub fn source_dropdown(source: Signal<usize>) -> Element {
    Element::dropdown(
        SOURCE_LABELS.iter().map(|s| s.to_string()).collect(),
        source,
    )
    .width(SOURCE_W)
}

/// 只读来源名（`Gitee` / `GitHub`），灰字。
///
/// 商店卡与关于页更新区共用：来源切换搬家后，置灰的包与「检查更新」都需要一处能
/// 解释当下走哪一路。**不是控件**——切换只在设置页「高级」卡，这里不提供第二个入口。
pub fn source_hint(source: Signal<usize>) -> Element {
    Element::label_signal(source.map(|i| source_name(*i).to_string()))
        .font_size(tokens::FONT_MD)
        .fg(to_color(tokens::TEXT_SECONDARY))
}

/// 组装「官方资源下载」卡（标题 + 可折叠正文）。
pub fn build_store_card(
    store: StoreState,
    sender: Option<Sender<UiMsg>>,
    expanded: Signal<bool>,
    col1_w: i32,
) -> Element {
    let rows_sig = store.rows;
    let source = store.source;
    let download = store.download;
    let busy = store.busy;
    let search = signal(String::new());

    // ① 搜索行（40px，输入框吃满）。
    let search_row = Element::row()
        .height(card::ROW_H)
        .cross(Align::Center)
        .padding_xy(16, 0)
        .child(search_box(search).width_match().height(CONTROL_H));

    // ② 状态行：红字摘要（长错误尾部截断）+ 灰字来源 + 刷新。来源切换在设置页「高级」卡。
    let refresh = {
        let sender = sender.clone();
        move |_ctx: &mut EventCtx| {
            if let Some(s) = &sender {
                let _ = s.send(UiMsg::StoreAction(StoreAction::RefreshCatalog {
                    source: source.get(),
                }));
            }
        }
    };
    let status_row = Element::row()
        .spacing(GAP)
        .cross(Align::Center)
        .height(card::ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label_signal(store.status)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_ACCENT))
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(source_hint(source))
        .child(
            btn("刷新")
                .enabled_when(move || !busy.get())
                .on_click(refresh),
        );

    // ③ 每一枚包一行（后台事件改信号 → 只重建这些行；搜索交给每行自己的可见性）。
    let rows = Element::host_signal(rows_sig, move |row: StorePackRow| {
        store_row(row, sender.clone(), source, download, search, busy)
    });
    // 搜不到时的空态：只在这一刻出现（不搜时的空白另有含义）。
    let empty = Element::row()
        .cross(Align::Center)
        .height(card::ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label("（没有匹配的资源包）")
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY)),
        )
        .visible_when(move || {
            !search.get().trim().is_empty()
                && !rows_sig
                    .get()
                    .iter()
                    .any(|r| remote_matches(r, &search.get()))
        });

    let (title, body, exp) = FeatureCard::new("官方资源下载", expanded)
        .body(
            Element::col()
                .fill()
                .spacing(0)
                .child(search_row)
                .child(Element::leaf().height(GAP))
                .child(status_row)
                .child(Element::leaf().height(GAP))
                .child(rows)
                .child(empty),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    Element::col()
        .child(title)
        .child(body.visible_when(move || exp.get()))
}

/// 清单里的一枚：名称 + 版本格 +「!」+ 行尾动作（RuPix 同款骨架）。
fn store_row(
    row: StorePackRow,
    sender: Option<Sender<UiMsg>>,
    source: Signal<usize>,
    download: Signal<Option<(String, u8)>>,
    search: Signal<String>,
    busy: Signal<bool>,
) -> Element {
    let (label, tone) = action_of(&row, source.get());
    let row_id = row.id.clone();
    // 下载中：按钮文案绑 `download`（`42%`）；有动作在跑时全部置灰（不再靠"撞 BusyGuard"
    // 报错——状态行只留 7 个短词，不新增"进行中"）。
    let caption = download.map({
        let id = row_id.clone();
        move |p| match p {
            Some((pid, pct)) if pid == &id => format!("{pct}%"),
            _ => label.to_string(),
        }
    });
    let can_click = {
        let id = row_id.clone();
        move || !busy.get() && !matches!(download.get(), Some((pid, _)) if pid == id)
    };
    let open = {
        let snapshot = row.clone();
        let sender = sender.clone();
        move |ctx: &mut EventCtx| open_info_window(ctx, &snapshot, sender.clone(), busy)
    };
    let install = {
        let sender = sender.clone();
        let id = row_id.clone();
        move |_ctx: &mut EventCtx| {
            if let Some(s) = &sender {
                let _ = s.send(UiMsg::StoreAction(StoreAction::Install {
                    id: id.clone(),
                    source: source.get(),
                }));
            }
        }
    };

    let action = btn_action(caption, tone);
    // 灰档不挂回调（按钮自己也拦事件）；可点档才挂 `enabled_when`。
    let action = if tone == ActionTone::Idle {
        action
    } else {
        action.enabled_when(can_click).on_click(install)
    };

    let snapshot = row.clone();
    Element::row()
        .cross(Align::Center)
        .spacing(GAP)
        .height(card::ROW_H)
        .padding_xy(16, 0)
        .child(
            Element::label(row.name.clone())
                .font_size(tokens::FONT_MD)
                .weight(1.0)
                .max_lines(1)
                .truncate(Truncate::End),
        )
        .child(version_cell(&row.version))
        .child(btn("!").on_click(open))
        .child(action_slot(action))
        .visible_when(move || remote_matches(&snapshot, &search.get()))
}

/// 「!」详情窗：内容取自点击时的行快照（翻列表/刷新都不影响已开的窗）。
///
/// 形式对齐 RuPix 的「解码器信息」：owned + 居中 + 单例键；**高度按内容估算**（不再固定
/// 360 留一大截空白），正文可滚；已装时底部固定一条**右对齐的红色「卸载」**（两段式确认，
/// 确认后关窗，列表由后台事件刷新）。字段严格按用户编号清单：名称 / 标识 / 版本 /
/// Lua API / 作者 / 许可证 / 发行时间 / 说明 / 大小。
fn open_info_window(
    ctx: &mut EventCtx,
    row: &StorePackRow,
    sender: Option<Sender<UiMsg>>,
    busy: Signal<bool>,
) {
    let key = format!("store-info-{}", row.id);
    let snapshot = row.clone();
    let height = info_height(&snapshot);
    ctx.open_window(
        Window::new("资源包信息", 520, height)
            .resizable(true)
            .centered(true)
            .owned(true)
            .single(key)
            .bg(to_color(tokens::BG))
            .content(move || info_content(&snapshot, sender.clone(), busy)),
    );
}

/// 说明文本（空说明给一句兜底；高度估算与正文用同一份）。
fn description_of(row: &StorePackRow) -> String {
    if row.description.is_empty() {
        "（当前清单没有写说明）".to_string()
    } else {
        row.description.clone()
    }
}

/// 详情窗高度估算（windui 0.30 没有 `fit_content`，只能按行数/折行估）。
///
/// 9 个字段行 × 30px + 内边距 32 + 说明折行（每多一行 +20；估算最多按 6 行给高，
/// 更长的说明由正文滚动容器承接——**控件本身不再裁行**）+ 已装时的底部卸载条
/// （12×2 + 30）。宁可略矮让正文滚，也不要固定高度在按钮下面留一大截空白。
fn info_height(row: &StorePackRow) -> i32 {
    let value_w = (520.0f32 - 16.0 * 2.0 - 92.0 - 12.0).max(80.0);
    let lines = ((card::text_width(&description_of(row), tokens::FONT_MD) / value_w).ceil() as i32)
        .clamp(1, 6);
    let base = 16 * 2 + 9 * 30 + (lines - 1) * 20;
    let footer = if row.installed_version.is_some() {
        24 + 30
    } else {
        0
    };
    (base + footer + 8).clamp(260, 560)
}

fn info_content(row: &StorePackRow, sender: Option<Sender<UiMsg>>, busy: Signal<bool>) -> Element {
    let api = if row.api == 0 {
        format!("未声明（宿主支持 {}）", crate::scripting::store::LUA_API)
    } else {
        format!(
            "{}（宿主支持 {}）",
            row.api,
            crate::scripting::store::LUA_API
        )
    };
    let author = if row.author.is_empty() {
        "—".to_string()
    } else {
        row.author.clone()
    };
    let license = if row.license.is_empty() {
        "—".to_string()
    } else {
        row.license.clone()
    };
    let released = if row.released.is_empty() {
        "—".to_string()
    } else {
        row.released.clone()
    };
    let size = match (row.size, row.unpacked_size) {
        (0, _) => "—".to_string(),
        (zip, 0) => human_size(zip),
        (zip, unpacked) => format!("{}（安装后 {}）", human_size(zip), human_size(unpacked)),
    };

    let col = Element::col()
        .fill()
        .spacing(0)
        .child(info_row("名称", row.name.clone()))
        .child(info_row("标识", row.id.clone()))
        .child(info_row("版本", format!("v{}", row.version)))
        .child(info_row("Lua API", api))
        .child(info_row("作者", author))
        .child(info_row("许可证", license))
        .child(info_row("发行时间", released))
        .child(info_row("说明", description_of(row)))
        .child(info_row("大小", size));

    let mut root = Element::col().fill().spacing(0).child(
        Element::scroll()
            .weight(1.0)
            .child(col.padding(16).width_match()),
    );
    if row.installed_version.is_some() {
        root = root
            .child(card::divider())
            .child(uninstall_actions(row.id.clone(), sender, busy));
    }
    root
}

/// 详情窗底部的卸载条：**右对齐、红色、两段式**（第一次点只摆出「确认卸载 / 取消」）。
///
/// 「取消」在最右（与对话框口径一致），间距 8px。
fn uninstall_actions(id: String, sender: Option<Sender<UiMsg>>, busy: Signal<bool>) -> Element {
    let arm = signal(false);
    let ask = btn("卸载")
        .danger()
        .enabled_when(move || !busy.get())
        .visible_when(move || !arm.get())
        .on_click(move |_ctx| arm.set(true));
    let (yes_tx, yes_id) = (sender, id);
    let yes = btn("确认卸载")
        .danger()
        .enabled_when(move || !busy.get())
        .visible_when(move || arm.get())
        .on_click(move |ctx: &mut EventCtx| {
            arm.set(false);
            if let Some(s) = &yes_tx {
                let _ = s.send(UiMsg::StoreAction(StoreAction::Uninstall(yes_id.clone())));
            }
            // 文件马上会被删：这扇窗留着只会显示过期状态（RuPix 卸载后同样 request_close）。
            ctx.request_close();
        });
    let no = btn("取消")
        .enabled_when(move || !busy.get())
        .visible_when(move || arm.get())
        .on_click(move |_ctx| arm.set(false));
    Element::row()
        .spacing(8)
        .cross(Align::Center)
        .padding_xy(16, 12)
        .width_match()
        .child(Element::flex_spacer())
        .child(ask)
        .child(yes)
        .child(no)
}

fn info_row(label: &str, value: String) -> Element {
    Element::row()
        .spacing(12)
        .padding_xy(0, 6)
        .child(
            Element::label(label)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT_SECONDARY))
                .width(92),
        )
        .child(
            // 说明可能很长：**不限行**，完整排版后交给外层滚动容器；
            // `max_lines(6)` 会把长说明直接裁掉半截（CursorFX 这类长描述必现）。
            Element::label(value)
                .font_size(tokens::FONT_MD)
                .fg(to_color(tokens::TEXT))
                .weight(1.0),
        )
        .cross(Align::Start)
        .width_match()
}

/// 下载体积的人话写法（清单里给的是字节；0 = 清单没写，显示 —）。
fn human_size(n: u64) -> String {
    const MIB: u64 = 1 << 20;
    const KIB: u64 = 1 << 10;
    if n >= MIB {
        format!("{:.1} MB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{} KB", n / KIB)
    } else {
        format!("{n} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(version: &str, installed: Option<&str>, source_mask: u8, too_new: bool) -> StorePackRow {
        StorePackRow {
            id: "cursorfx".to_string(),
            name: "CursorFX".to_string(),
            version: version.to_string(),
            description: "光标特效".to_string(),
            size: 1024,
            installed_version: installed.map(str::to_string),
            too_new,
            files: 0,
            source_mask,
            author: "CurSeen".to_string(),
            license: "AGPL-3.0".to_string(),
            released: "2026-10-28".to_string(),
            api: 1,
            unpacked_size: 2048,
            file_count: 2,
        }
    }

    /// 本文件的**生产代码**文本（截至测试模块）。`include_str!` 会连测试一起包进来，
    /// 不切的话断言会被断言自己的文本满足（同款先例见 `ui::pages::global`）。
    fn production_src() -> &'static str {
        let all = include_str!("store.rs");
        let cut = all.find("\n#[cfg(test)]").unwrap_or(all.len());
        &all[..cut]
    }

    /// 行尾三态（RuPix 同款表）：没装=蓝下载、旧版=橙升级、最新/缺地址/宿主太旧=灰下载。
    #[test]
    fn action_tones_follow_state_and_source() {
        assert_eq!(
            action_of(&row("1.0", None, 0b11, false), 0),
            ("下载", ActionTone::Primary)
        );
        assert_eq!(
            action_of(&row("1.2", Some("1.0"), 0b11, false), 0),
            ("升级", ActionTone::Warning)
        );
        assert_eq!(
            action_of(&row("1.0", Some("1.0"), 0b11, false), 0),
            ("下载", ActionTone::Idle)
        );
        assert_eq!(
            action_of(&row("1.0", None, 0b11, true), 0),
            ("下载", ActionTone::Idle),
            "宿主太旧：置灰"
        );
        assert_eq!(
            action_of(&row("2.0", None, 0b01, false), 1),
            ("下载", ActionTone::Idle),
            "选了 GitHub 但这枚只发了 Gitee：置灰"
        );
        assert_eq!(
            action_of(&row("2.0", None, 0b01, false), 0),
            ("下载", ActionTone::Primary)
        );
    }

    /// 搜索只认名称 / 标识 / 版本 / 说明（大小文件数不进搜索键）。
    #[test]
    fn search_matches_name_id_version_description_only() {
        let r = row("1.0", None, 0b11, false);
        assert!(remote_matches(&r, ""));
        assert!(remote_matches(&r, "cursor"));
        assert!(remote_matches(&r, "光标"));
        assert!(remote_matches(&r, "1.0"));
        assert!(!remote_matches(&r, "1024"), "大小不是搜索键");
        assert!(remote_matches(&r, "  "), "空白搜索 = 不过滤");
    }

    /// 静态门禁：搜索行 / 状态行 + 刷新 / 行骨架 / 三种动作 /「!」详情窗**缺一不可**。
    ///
    /// ★ 来源下拉是本模块的控件正本（`source_dropdown`），但 m69 起**不在商店卡里**：
    /// 下载源是商店与本体更新共用的全局设置，挂到设置页「高级」卡。
    #[test]
    fn store_card_wires_the_download_ui() {
        let s = production_src();
        for needle in [
            "FeatureCard::new(\"官方资源下载\"",
            "Element::text_input(filter, \"搜索名称或说明\")",
            "leading_icon('\\u{E71E}')",
            "Element::dropdown(",
            "\"刷新\"",
            "StoreAction::RefreshCatalog",
            "StoreAction::Install",
            "StoreAction::Uninstall",
            "Element::host_signal",
            "ctx.open_window(",
            "\"资源包信息\"",
            "TEXT_ACCENT",
            "ACTION_COL_W",
            "VERSION_COL_W",
            // m69：来源切换搬到「高级」卡后，商店卡保留一枚**灰字只读**的来源名。
            "source_hint(source)",
        ] {
            assert!(s.contains(needle), "「官方资源下载」卡缺少：{needle}");
        }
        // ★ 用户口径：这里**没有开关**——下载来的脚本在脚本页各有开关。
        assert!(!s.contains("Element::switch("), "资源下载卡不该有开关");
        // ★ m69：来源下拉搬到设置页「高级」卡，商店卡里不得再摆一个；控件正本仍在本模块。
        let card_fn = s
            .split("pub fn build_store_card(")
            .nth(1)
            .and_then(|rest| rest.split("/// 清单里的一枚").next())
            .expect("找不到 build_store_card 的生产代码");
        assert!(
            !card_fn.contains("Element::dropdown("),
            "下载源已归「高级」卡，商店卡里不该再有来源下拉"
        );
        assert!(
            s.contains("pub fn source_dropdown("),
            "来源下拉控件正本（source_dropdown）丢了"
        );
        // ★ 卸载只在详情窗里：行里不许再出现卸载按钮（`uninstall_actions` 在窗口内容里）。
        let row_fn = s
            .split("fn store_row(")
            .nth(1)
            .and_then(|rest| rest.split("/// 「!」详情窗").next())
            .expect("找不到 store_row 的生产代码");
        assert!(!row_fn.contains("卸载"), "行里不该有卸载动作");
    }

    /// m69：来源名是**只读灰字**（`label_signal`），不是第二个切换控件；下标越界夹紧。
    #[test]
    fn source_hint_is_read_only_and_clamps_the_index() {
        let s = production_src();
        let hint_fn = s
            .split("pub fn source_hint(")
            .nth(1)
            .and_then(|rest| rest.split("\n/// 组装「官方资源下载」卡").next())
            .expect("找不到 source_hint 的生产代码");
        assert!(
            hint_fn.contains("Element::label_signal("),
            "来源提示必须是只读文本"
        );
        assert!(
            !hint_fn.contains("Element::dropdown("),
            "来源提示不得再摆一个下拉"
        );
        assert_eq!(source_name(0), SOURCE_LABELS[0]);
        assert_eq!(source_name(1), SOURCE_LABELS[1]);
        assert_eq!(
            source_name(99),
            SOURCE_LABELS[SOURCE_LABELS.len() - 1],
            "越界下标必须夹紧，不能 panic"
        );
    }

    /// 「!」窗里至少要能回答：版本 / 已装版本 / 大小 / 文件数 / 状态 / 说明，
    /// 且已装时底部才有卸载。
    #[test]
    fn info_window_lists_the_requested_fields_and_red_uninstall() {
        let s = production_src();
        for needle in [
            "info_row(\"名称\"",
            "info_row(\"标识\"",
            "info_row(\"版本\"",
            "info_row(\"Lua API\"",
            "info_row(\"作者\"",
            "info_row(\"许可证\"",
            "info_row(\"发行时间\"",
            "info_row(\"说明\"",
            "info_row(\"大小\"",
            "fn info_height",
            ".danger()",
            "Element::flex_spacer()",
            "Element::scroll()
            .weight(1.0)",
        ] {
            assert!(s.contains(needle), "资源包详情窗缺少：{needle}");
        }
        // ★ 用户按编号点名的字段之外不添油加醋（旧窗里的三行已按口径去掉）。
        for gone in [
            "info_row(\"已装版本\"",
            "info_row(\"文件数\"",
            "info_row(\"状态\"",
        ] {
            assert!(!s.contains(gone), "用户没点名的字段不该出现：{gone}");
        }
    }

    /// 长说明必须完整排版后交给滚动容器；恢复 `.max_lines(6)` 会把 CursorFX 这类说明书裁半截。
    #[test]
    fn info_value_is_not_line_clamped() {
        let s = production_src();
        let info_row = s
            .split("fn info_row(")
            .nth(1)
            .expect("找不到 info_row 的生产代码");
        assert!(
            !info_row.contains(".max_lines("),
            "说明字段不许限行（外层已有滚动），否则长说明会被裁断"
        );
    }

    /// 高度估算是"内容自适应"的机器读数：说明长更高、已装多一条卸载栏；且夹在窗口上下限内。
    #[test]
    fn info_height_grows_with_description_and_uninstall_footer() {
        let base = row("1.0", None, 0b11, false);
        let mut long = base.clone();
        long.description = "很长的说明".repeat(200);
        let mut installed = base.clone();
        installed.installed_version = Some("0.0.0".to_string());
        assert!(info_height(&long) > info_height(&base), "说明长要高一些");
        assert!(
            info_height(&installed) > info_height(&base),
            "已装时多一条卸载栏"
        );
        assert!((260..=560).contains(&info_height(&base)));
        assert!((260..=560).contains(&info_height(&long)));
    }
}
