//! 全局设置页面。
//!
//! 快捷键、模块开关、图层覆盖、工作文件夹。★ 开关架构（2.0）：功能总开关 / 开机自启 /
//! 预设方案已全部废除——每个功能卡片自己的开关就是唯一开关（见各功能页），全局页只剩
//! 真正的全局项。
//!
//! ★ 2026-09-27 用户口径：**「语言」「防休眠」两张卡整卡删除**——不做多语言，也不需要
//! 拦休眠（`global.prevent_sleep` / `platform::keep_awake` 一并摘掉）。留下一个点了
//! 没反应的控件，比没有这个控件更糟。
//!
//! # 「功能开关」卡（2026-09-25 起，★ 2026-10-07 合并）
//!
//! 一张卡、正文**两段**（横线 + 段名分隔）：「模块开关」「脚本开关」。原本是两张同构的
//! 镜像卡，用户口径合成一张——一次展开看全，不必点两次。代价：脚本段不再能单独折叠
//! （两段共用一个展开态 `global.switches`），这是合并的本意。
//!
//! ## 段一「模块开关」（2026-09-25 用户口径）
//!
//! 所有功能模块在各自页面的卡片标题栏上都有总开关；本段把它们**镜像**到一处，用户不必
//! 在七个页面之间跳着找。★ 镜像不是副本：绑的是**同一个信号**（见
//! [`module_switches`]），任意一处勾选另一处即时同步，`apply_to` 的写回路径一条不变。
//! 「点击效果」「点击声音」按鼠标键分别开关（五键 × 2 的子卡），不存在单一的"模块开关"，
//! 不进本段——硬造一个"全部开/关"就是新语义，不是镜像。
//!
//! ## 段二「脚本开关」（2026-09-28 用户口径，原独立卡）
//!
//! 与段一同构的**镜像**：脚本页（Lua 扩展页）每张脚本卡标题栏的开关收拢到一处。绑同一个
//! `card.toggle` 信号（初值 = `role_is_active`，写回仍走 `apply_to` 的
//! `set_role_active`），任意一处勾选另一处即时同步。收录范围 =
//! `LuaExtCard::is_script_card`——脚本页与本段**共用这一个判定入口**，两边不可能走偏。
//! MouseTrail/HoverDot/ClickFx 的特效脚本不进本段：它们由鼠标页下拉承接，"下拉选中"
//! 不是开关，没有可镜像的信号；硬造一个"全开/全关"同样是新语义。
//!
//! # 「快捷键」卡（2026-09-25 定形，2026-09-27 改名口径，★ 2026-10-07 合并）
//!
//! 与「功能开关」同一刀：原「快捷键」+「脚本快捷键」两张同构卡合成一张，正文**两段**
//! （「模块快捷键」「脚本快捷键」，横线 + 段名分隔），共用展开态 `global.hotkey`。
//!
//! 全项目快捷键收拢到一张卡，名称 = **「模块 > 行标签」**，两段都是功能模块页里的
//! **原文**：首段是那把键所在的功能卡标题，末段是它在那个卡（或快捷键子卡）里那一行
//! 热键框的标签。例：截图页「截图与贴图」卡里那行叫「截图」，这里就写「截图与贴图 >
//! 截图」。★ 末段**不改写**——用户在这张卡里认出名字，就能原样翻到功能页找到同一行。
//!
//! 行序 = **导航页序 → 页内卡序 → 卡内行序**（2026-09-27 用户口径；「控制面板」是全局
//! 项、不挂在任何模块下，固定在第一行）。与各页快捷键框绑同一信号 + 同一 [`HotkeyField`]
//! 字段，捕获逻辑完全复用；哪一页改动都一样生效。表的完整性与顺序由测试守门（见本文件
//! `hotkey_entries_cover_every_field_exactly_once`：每新增一个 `HotkeyField` 变体而忘了
//! 进表，测试立刻红）。
//!
//! 面板快捷键同日从文本框并入捕获体系（`HotkeyField::Panel`）——集合里 23 行全是
//! 「点击捕获」的同一形状，文本框特例反而让人疑惑"为什么就它要手打"。
//!
//! ## 段二「脚本快捷键」（2026-10-01 用户口径：脚本的开关与模块分开，键位也该分开收；
//! 原独立卡，2026-10-07 并入本卡）
//!
//! 段一收的是固定字段那套（`HotkeyField`）。脚本的键位走**另一条链路**——写在脚本自己的
//! `hotkey` 参数里，由 `Runtime::script_hotkeys_with_state` 派生派发名单——互斥规则早
//! 就这样了（改任一侧都会清掉对方同值的旧绑定），可界面上它原先只在「脚本」页出现。
//! 既然「脚本开关」已经镜像到设置页，键位也一并镜像：设置页这才真成"一处看全"的地方。
//! 行标签 = **脚本显示名 > 参数名**（与「脚本开关」段同口径），行内目标仍是脚本参数自己
//! 的 sig——行序、捕获、互斥一行没重写，只是把已有的那一行搬到一个能一眼看见的地方。
//!
//! # 「计时器」为什么没有总开关（2026-09-20 用户口径）
//!
//! 与「屏幕录制」同款：**快捷键为空串即停用**。屏上的读秒是一个只在"被快捷键叫起来"时才
//! 存在的东西，给它配一个常驻开关只会多出"勾了开关但快捷键是空的"死态 —— 用户实测就是
//! 这个症状：勾上「计时器」什么也没发生（因为开关只是闸门，而快捷键那一格当时还是个纯
//! 文本框，根本设不出合法组合）。现在开关与快捷键合成一个动作：按快捷键开始，再按一下
//! 停止并清零。
//!
//!   （补记 2026-09-25：计时器卡标题栏**有**开关了（`global.timer.enabled`），上文的
//!   "没有总开关"已成历史；保留这段是因为"快捷键空串 = 第二道闸"的语义没变。）
//!
//! # 「工作文件夹」卡（2026-09-27 用户口径）
//!
//! 显示 `<exe>/temp`（运行期产物的统一落点，见 `core::paths`）有多大，并给一个键直接
//! 跳过去。这一页其余三张卡都在管**行为**，唯独它管的是一块**磁盘占用**——放这里是因为
//! 它不属于任何单个功能（截图、录屏、GIF、标注的产物都往这个目录里堆），是全局项。
//!
//! ★ 卡名不用「工作文件夹」：这个目录装的是用户自己的**产物**（截图 / 录屏 / 标注），
//! "临时"两个字会让人以为可以随手清掉。`paths::temp_dir()` 这个技术名不改。
//!
//! 两条实现口径：
//! - **占用在后台线程算**（`ui::temp_usage`，结果经 `UiMsg::TempUsage` 回来）。首次统计
//!   跑在**建面板那一帧**，而递归 stat 一个装了录屏的目录是几十毫秒量级——留在 UI 线程
//!   就是"开面板卡一下"，且这种卡顿几乎不可能被归因到一张"只显示个数字"的卡上。
//! - **每次打开面板重算**，不缓存、也不给「刷新」按钮：数字只在打开那一刻有意义（用户
//!   多半刚在资源管理器里清过东西），而"面板开着的时候清盘"这种用法可以由"关掉再开"
//!   覆盖——为一个能靠重开面板替代的动作常驻一个按钮，是白摆控件。
//! - **不摆「位置」行**（2026-09-28 用户口径）：路径长、显示不全也抄不了，要过去
//!   点「打开文件夹」就行。

use windui::prelude::*;

use crate::core::events::UiMsg;
use crate::ui::card::{self, FeatureCard};
use crate::ui::model::{HotkeyField, PanelModel};
use crate::ui::temp_usage::{self, TempUsageState};
use crate::ui::tokens;
use crate::ui::tokens::to_color;

/// 模块开关集合的**显示名**（与 [`module_switches`] 同序；给列宽计算用的静态表）。
///
/// ★ 名字必须与各功能页卡片标题**逐字相同**——这张卡的价值就在于"名字对得上"，用户
/// 在设置里看到「截图与贴图」，翻到屏幕页找到的卡片标题也叫「截图与贴图」。名字各写
/// 一份就会慢慢走偏，所以有 `module_switch_names_match` 测试钉住。
#[cfg(test)]
const MODULE_NAMES: [&str; 16] = [
    // 鼠标页
    "拖尾效果",
    "悬浮标",
    "点击效果",
    "点击声音",
    // 键盘页
    "击键回显",
    "击键音效",
    "虚拟键盘",
    // 屏幕页
    "屏幕标注",
    "画中画放大",
    "全屏放大",
    "截图与贴图",
    "GIF录制",
    "视频录制",
    // 工具页
    "便签",
    "剪贴板",
    "计时器",
];

/// 模块开关集合：`(显示名, 总开关信号)`。
///
/// ★ 顺序 = **侧栏导航页序 → 页内卡序**（2026-09-27 用户口径："模块开关与快捷键一样，
/// 都按 UI 实际顺序排"）。刷 UI 时用户可以顺着往下看，不用在设置页里找"怎么顺序不对"。
/// 这张表的顺序由 [`module_switches_follow_the_page_card_order`] 扫各页源码看门——
/// 哪个模块的卡片被挪了位置，测试立刻红。
///
/// 信号与各页卡片标题栏 `.switch(...)` 绑的是**同一个**——复选框写信号、`apply_to`
/// 照常 diff 回配置，本卡不新增任何写回路径。
fn module_switches(m: &PanelModel) -> Vec<(&'static str, Signal<bool>)> {
    vec![
        ("拖尾效果", m.trail_enabled),
        ("悬浮标", m.hover_enabled),
        ("点击效果", m.ripple_master),
        ("点击声音", m.sound_master),
        ("击键回显", m.enabled_key),
        ("击键音效", m.ksound_enabled),
        ("虚拟键盘", m.vk_enabled),
        ("屏幕标注", m.enabled_draw),
        ("画中画放大", m.pip_enabled),
        ("全屏放大", m.fs_enabled),
        ("截图与贴图", m.shot_enabled),
        ("GIF录制", m.gif_enabled),
        ("视频录制", m.rec_enabled),
        ("便签", m.sticky_enabled),
        ("剪贴板", m.clip_enabled),
        ("计时器", m.timer_enabled),
    ]
}

/// 快捷键集合：`(显示名「模块 > 行标签」, 快捷键字段)`。
///
/// ★ 完整性约束：**每个 `HotkeyField` 变体必须恰好出现一次**（测试守门）。
///
/// ★ 2026-09-27 用户口径：末段是**功能模块页里那一行快捷键框的标签原文**——逐字复用，
/// 不许在这里另写一个"更短更好看"的名字。所以「截图与贴图」卡里的那行在页里叫「截图」，
/// 这里就写「截图与贴图 > 截图」。★ 二次口径（同日）这三行省掉「快捷键」三字，与页内
/// 完全一致（页里现在是「截图」「截图后贴图」「贴图」）。行序 = 导航页序 → 页内卡序 →
/// 卡内行序，「控制面板」是全局项、不归属任何模块卡，固定在首行。两件事都由
/// [`hotkey_collection_follows_the_ui_order_and_reuses_ui_text`] 扫各页源码看门。
/// 快捷键集合（设置页「快捷键」卡）：顺序 = 面板/页内卡序，**名称逐字复用各页原文**。
///
/// ★ 2026-09-28 起它还是**互斥回收的字段全集**（`PanelModel::set_hotkey_exclusive`
/// 遍历它）：新增一把快捷键必须在这里登记，于是"设置页看得到的"与"参与互斥的"天然
/// 同一个集合，不必再维护第二张表。
pub(crate) const HOTKEY_ENTRIES: &[(&str, HotkeyField)] = &[
    // 全局项：面板不挂在任何功能模块下，永远第一行（用户口径）。
    ("控制面板 > 显示/隐藏", HotkeyField::Panel),
    // ── 屏幕页（屏内卡序）/ 屏幕标注卡的「快捷键」子卡 ──
    ("屏幕标注 > 屏幕标注", HotkeyField::Screen),
    ("屏幕标注 > 白板", HotkeyField::White),
    ("屏幕标注 > 画笔1", HotkeyField::Pen1),
    ("屏幕标注 > 画笔2", HotkeyField::Pen2),
    ("屏幕标注 > 画笔3", HotkeyField::Pen3),
    ("屏幕标注 > 直线", HotkeyField::Line),
    ("屏幕标注 > 箭头", HotkeyField::Arrow),
    ("屏幕标注 > 矩形", HotkeyField::Rect),
    ("屏幕标注 > 椭圆", HotkeyField::Ellipse),
    ("屏幕标注 > 文字", HotkeyField::Text),
    ("屏幕标注 > 序号", HotkeyField::Stamp),
    ("屏幕标注 > 马赛克", HotkeyField::Mosaic),
    // ★ 橡皮擦在马赛克之后 = 屏幕页「快捷键」子卡的顺序 = 工具栏按钮的最后两格；
    //   三处同序的正本是 `core::hotkeys::DRAW_TOOL_ORDER`（见 `hotkey_rows_follow_the_tool_order`）。
    ("屏幕标注 > 橡皮擦", HotkeyField::Eraser),
    // ── 屏幕页 / 画中画放大卡的「快捷键」子卡 ──
    ("画中画放大 > 快捷键", HotkeyField::PipToggle),
    // ── 屏幕页 / 全屏放大卡 ──
    ("全屏放大 > 快捷键", HotkeyField::FullscreenZoom),
    // ── 屏幕页 / 抓取三卡（capture::build_capture_section 的同页顺序）──
    ("截图与贴图 > 截图", HotkeyField::Shot),
    ("截图与贴图 > 截图后贴图", HotkeyField::ShotPin),
    ("截图与贴图 > 贴图", HotkeyField::PinClip),
    ("截图与贴图 > 长截图", HotkeyField::ShotScroll),
    ("截图与贴图 > 宽截图", HotkeyField::ShotScrollWide),
    ("GIF录制 > 录制", HotkeyField::Gif),
    ("GIF录制 > 裁剪", HotkeyField::Trim),
    ("视频录制 > 录屏", HotkeyField::Record),
    ("视频录制 > 选区录屏", HotkeyField::RecordRegion),
    ("视频录制 > 时间裁剪", HotkeyField::RecordTrim),
    // ── 工具页（页内卡序：便签 → 剪贴板 → 计时器）──
    ("便签 > 快捷键", HotkeyField::Sticky),
    ("剪贴板 > 快捷键", HotkeyField::Clipboard),
    ("计时器 > 快捷键", HotkeyField::Timer),
];

/// 「工作文件夹」卡里路径文本的预算（逻辑 px）。
///
/// 由**布局常量直接推**，不手抄一个数——手抄的数会在下次调 `panel::WIDTH` 时静默过期
/// （超了不报错，只会把路径画到卡片外面去）：
/// 内容区（`panel::WIDTH` 616 − `nav::NAV_WIDTH` 66 = 550）− 滚动容器内边距 16×2 −
pub fn build_global_page(
    m: &PanelModel,
    temp: TempUsageState,
    sender: Option<Sender<UiMsg>>,
) -> Element {
    // ★ 每次打开面板都重算一次 `temp/` 的占用：数字只在**这一刻**有意义（用户很可能刚在
    //   资源管理器里清了几个大文件又回来看）。走后台线程，见 `ui::temp_usage` 的头注释。
    if let Err(e) = temp_usage::spawn_scan(&temp, &sender) {
        log::warn!("统计临时目录失败: {e}");
    }

    // 展开态来自模型（落盘 + 各卡独立），id 见 `core::config::CARD_IDS`。
    // ★ 2026-10-07：原「脚本开关」「脚本快捷键」两张卡的展开态 id 随合并作废，
    //   两组各只留一个（`global.switches` / `global.hotkey`）。
    let switches_expanded = m.expand("global.switches");
    let hotkey_expanded = m.expand("global.hotkey");
    let overlay_expanded = m.expand("global.overlay");
    // 「工作文件夹」卡（`<exe>/temp` 的占用 + 快捷入口）。
    let temp_expanded = m.expand("global.temp");
    // 「系统」卡（管理员权限启动勾选项）。
    let system_expanded = m.expand("global.system");
    let help_expanded = m.expand("global.help");

    // 控件列跟随标题列（见 `card::option_row`）：本页最长名是
    // 「屏幕标注 > 画笔1」一类（本页最长 ≈ 「便签 > 快捷键」7 汉字 + 3 半角，「截图与
    // 贴图 > 截图后贴图」10 汉字折算 165.2px），都远窄于 COL1_W=208，标签列留着余量不
    // 折行 —— 「截图与贴图 > 截图并贴图快捷键」那 207.2px 的极限项已于 2026-09-27 改名
    // 消失；COL1_W 是全项目唯一对齐常量，收缩它要挪动所有页面的控件列 x，不动。
    // 列1宽度：全项目唯一常量（2026-09-25 第六定案），所有页面同一条对齐线。
    let col1_w = card::COL1_W;

    let mut content = Element::col();

    // ── 功能开关（模块开关 + 脚本开关，两段同卡）──
    //
    // ★ 2026-10-07 用户口径：两张同构的镜像卡合成一张，正文用**横线 + 段名**分成
    //   「模块开关」「脚本开关」两段——原本要展开两次才看全，现在一次。代价是脚本段
    //   不再能单独折叠（共用一个展开态 `global.switches`），这是合并的本意。
    let (title, body, exp) = FeatureCard::new("功能开关", switches_expanded)
        .body({
            // 段一：模块开关（镜像所有功能卡标题栏的总开关）。
            let mut col = Element::col();
            col = col.child(card::group_label("模块开关"));
            for (name, sig) in module_switches(m) {
                col = col.child(card::option_row(name, Element::checkbox("", sig), col1_w));
            }
            // 段二：脚本开关（镜像脚本页每张脚本卡标题栏的开关，与段一同构）。
            //
            // 绑**同一个** `card.toggle` 信号（`LuaExtCard` 的 toggle 初值来自
            // `role_is_active`，写回仍走 `apply_to` 的 `set_role_active` 一条路），
            // 任意一处勾选另一处即时同步。收录范围 = `LuaExtCard::is_script_card`
            // （`scripts/` 目录、排除键盘页专属的 keysound）——与脚本页同一入口，两边
            // 不可能走偏。MouseTrail/HoverDot/ClickFx 的特效脚本由鼠标页下拉承接
            // （下拉 ≠ 开关，没有可镜像的开关信号），不进本段。
            let scripts: Vec<&crate::ui::model::LuaExtCard> = m
                .lua_ext_cards
                .iter()
                .filter(|c| c.is_script_card())
                .collect();
            col = col
                .child(card::divider())
                .child(card::group_label("脚本开关"));
            if scripts.is_empty() {
                col = col.child(
                    Element::label(
                        "还没有可开关的脚本。往 assets/lua/scripts 放入 .lua 后重启即可。",
                    )
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
                );
            }
            for c in scripts {
                col = col.child(card::option_row(
                    &c.name,
                    Element::checkbox("", c.toggle),
                    col1_w,
                ));
            }
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 快捷键（模块快捷键 + 脚本快捷键，两段同卡）──
    //
    // ★ 2026-10-07 用户口径：与「功能开关」同一刀——原「快捷键」「脚本快捷键」两张同构卡
    //   合成一张，正文横线 + 段名分两段，共用展开态 `global.hotkey`。
    // 段一收"固定字段那套"快捷键（`HotkeyField`），名称 =「模块 > 快捷键」。
    let (title, body, exp) = FeatureCard::new("快捷键", hotkey_expanded)
        .body({
            let mut col = Element::col();
            col = col.child(card::group_label("模块快捷键"));
            for (label, field) in HOTKEY_ENTRIES {
                col = col.child(card::hotkey_row(
                    label,
                    *m.hotkey_signal(*field),
                    *field,
                    m.hotkey_capture,
                    col1_w,
                ));
            }
            // 段二：脚本快捷键（与段一同构，但收录**脚本**的键位）。
            //
            // 脚本的键位是**另一条链路**：写在脚本自己的 `hotkey` 参数里
            // （`config.lua_ext.params[id]`，随脚本卡一起被扫出来），由
            // `Runtime::script_hotkeys_with_state` 派生派发名单。两者互斥规则早就打通
            // （改任一侧都会把对方同值的旧绑定清空，见 `PanelModel::set_hotkey_exclusive`），
            // 可**界面上**脚本热键在设置页原先完全看不见——用户只能去「脚本」页逐卡翻。
            // 既然「脚本开关」已经镜像到设置页，段二把它的键位一并收上来：设置页于是
            // 真的成了一处看全的地方。
            //
            // 行标签 = **脚本显示名 + 参数名**（`LuaExtCard::name` + 参数单元的 `label`，
            // 2026-10-01 用户口径「也要是 `xx > xx` 的格式，文本都用现成的」）：与段一同是
            // 「模块 > 行」两段式，且每一段都取现成文本（脚本显示名与「脚本开关」段、
            // 脚本页卡标题同一份，参数名与脚本页那一行同一个 `label`），这样用户在这一行
            // 认出的名字总能翻到别处。
            // 收录范围只认**声明了 `hotkey` 参数**的脚本——没声明就没有键位可镜像，硬画一行
            // 空框比不画更糟。与脚本页共用同一份 `params` 信号（走 `host_signal`，参数整批
            // 重建时本段的行跟着重建），捕获态与互斥逻辑一行不重写。
            col = col.child(card::divider()).child(card::group_label("脚本快捷键"));
            // ★ 先数一遍有没有脚本真的声明了 `hotkey` 参数：行数要在**建卡那一刻**就知道
            //   （空态提示与行是二选一），而 `host_signal` 的构建闭包是 `Fn`，塞不进计数器。
            let any_script_hotkeys = m.lua_ext_cards.iter().filter(|c| c.is_script_card()).any(
                |c| {
                    c.params
                        .get()
                        .iter()
                        .any(|cell| matches!(cell, crate::ui::model::EffectParamCell::Hotkey { .. }))
                },
            );
            for c in m.lua_ext_cards.iter().filter(|c| c.is_script_card()) {
                let params = c.params;
                let name = c.name.clone();
                let hk_cap = m.script_hotkey_cap;
                // ★ 一个脚本地最多一行（`hotkey` 是单值参数）；没有该单元就给出一个零高
                //   占位，整行无形——画面上与"这脚本没有快捷键"一致，不留空框。
                col = col.child(Element::host_signal(params, move |cell| match cell {
                    // ★ 行名 = 「脚本名 > 参数名」（2026-10-01 用户口径）：两段都取**现成
                    //   文本**——脚本显示名（`LuaExtCard::name`，与「脚本开关」卡同一份）
                    //   与参数单元自己的 `label`，一个字都不新造。这样设置页这一行与
                    //   「脚本」页那张卡里的同一行能对上号（那边也显示 `label`），
                    //   而"缩进 > "这个前缀到了集合卡里也还是它规定的两段式（见
                    //   `hotkey_labels_are_module_qualified`）。
                    crate::ui::model::EffectParamCell::Hotkey { sig, label, .. } => {
                        let text = format!("{} > {}", name, label);
                        card::hotkey_row_script(
                            &text,
                            sig,
                            crate::ui::model::ScriptHotkeyCap { sig },
                            hk_cap,
                            col1_w,
                        )
                    }
                    _ => Element::leaf().height(0),
                }));
            }
            if !any_script_hotkeys {
                col = col.child(
                    Element::label(
                        "还没有脚本配了快捷键。在「脚本」页那张卡的参数里加一个「快捷键」参数即可。",
                    )
                    .font_size(tokens::FONT_MD)
                    .fg(to_color(tokens::TEXT_SECONDARY)),
                );
            }
            col
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 图层覆盖（全局置顶：对**所有**屏幕工具生效，勾一次全体作用）──
    //
    // 这两项原本藏在虚拟键盘页的「窗口行为」里，却管的是整层覆盖层的 z 序（拖尾 / 击键
    // 回显 / 虚拟键盘都在同一层上），不该跟某个功能绑在一起 —— 搬到这里，对所有功能适用。
    // 鼠标穿透是虚拟键盘私有属性，仍留在键盘页。
    //
    // ★ 2026-09-27 用户口径：卡名「覆盖层」→「图层覆盖」——它管的是整层覆盖层的 z 序，
    //   "图层"比"覆盖"更贴切（覆盖层窗口本身仍叫覆盖层，代码注释不改）。
    let (title, body, exp) = FeatureCard::new("图层覆盖", overlay_expanded)
        .body(
            Element::col()
                .child(card::option_row(
                    "总在最上",
                    Element::checkbox("", m.vk_always_on_top),
                    col1_w,
                ))
                .child(card::option_row(
                    "全屏避让",
                    Element::checkbox("", m.vk_fullscreen_avoid),
                    col1_w,
                )),
        )
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 工作文件夹（`<exe>/temp` 的占用 + 快捷入口）──
    //
    // 用户口径（2026-09-27）：设置页要能看见 `temp/` 有多大、并且一键跳过去。这两件事
    // 是一体的——**看见数字**才知道该不该清，**跳过去**才是动手的地方；只给数字用户
    // 还得自己去 `C:\...\CurSeen\temp` 里翻。卡名取「工作文件夹」而非「临时文件」：
    // 里面堆的是用户自己的产物（截图 / 录屏 / 标注），"临时"两个字会让人以为可以随手清掉。
    //
    // ★ 占用是**后台线程**算出来的（`ui::temp_usage`），这里只读信号。扫描几十毫秒起步，
    //   而这段代码跑在建面板那一帧——留在 UI 线程就是"开面板卡一下"。
    let temp_dir = crate::core::paths::temp_dir();
    let (title, body, exp) = FeatureCard::new("工作文件夹", temp_expanded)
        .body({
            let open_target = temp_dir.clone();
            let open_btn = Element::button("打开文件夹").on_click(move |_ctx| {
                // 只发起、不等：`ShellExecuteW` 把开资源管理器这件事交给系统，
                // 失败由平台层记日志（这里没有可展示错误的地方，面板上还有路径可照抄）。
                //
                // ★ 这一行 `info` 是**有意的**：这是唯一能区分"按钮没被点到"与"打开了但
                //   系统没照做"的证据（真机验收时踩过——注入的点击没落到按钮上，日志里
                //   什么都没有，光看"没弹资源管理器"分不清是哪一半）。
                log::info!("打开临时目录：{}", open_target.display());
                crate::platform::win_util::open_in_explorer(&open_target);
            });
            // ★ 没有「刷新」按钮：占用**每次打开面板都会重算**，用户要新数字就重开面板
            //   ——为一个能被"重开"替代的动作常驻一个按钮，是白摆控件。
            Element::col()
                .child(card::option_row(
                    "占用",
                    Element::label_signal(temp.text)
                        .font_size(tokens::FONT_MD)
                        .fg(to_color(tokens::TEXT)),
                    col1_w,
                ))
                // ★ 自动清理（2026-09-28 用户口径）：每次启动执行一次，把修改时间早于
                //   N 天前的文件删掉（0 = 永不，出厂默认）。阈值表与写回见
                //   `ui::model::TEMP_CLEANUP_DAYS`，执行点在 `Runtime::new` 的后台线程。
                .child(card::choice_row(
                    "自动清理",
                    crate::ui::model::TEMP_CLEANUP_LABELS
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                    m.temp_cleanup,
                    col1_w,
                ))
                .child(card::option_row(
                    // ★ 空标签是**刻意的**：这一行只有按钮，而按钮必须落在**控件列**
                    //   上——与上面「占用」「自动清理」两行的值同一条竖线。给这一行编一个
                    //   「操作」之类的标签，只是为了给缩进找借口。
                    "",
                    Element::row().child(open_btn).cross(Align::Center),
                    col1_w,
                ))
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 系统（管理员权限启动）──
    //
    // 勾选 = **下一次启动**走 UAC 提权（`main.rs` 启动分支读 `general.admin_launch`）；
    // 当前进程的权限不变，也没有"立即重启"入口——用户自己退出再开，最简且无魔法。
    // 出厂不勾：普通权限启动链路干净（无 UAC、无提权重启的启动状态残留）。
    //
    // ★ 行名即状态（2026-10-07 用户口径，四态）：**当前进程**（已启用/未启用）×
    //   **勾选**（重启后生效/取消）两个维度直接写进行名，不另配说明行。
    //   进程提权态在存活期内不变，建面板时读一次；勾选态经 `map` 实时联动。
    let now_elevated = crate::platform::elevate::is_elevated();
    let (title, body, exp) = FeatureCard::new("系统", system_expanded)
        .body(Element::col().child(card::option_row_signal(
            m.admin_launch.map(move |on| {
                match (now_elevated, *on) {
                    (false, false) => "管理员权限 (未启用)",
                    (false, true) => "管理员权限 (重启后有效)",
                    (true, false) => "管理员权限 (重启后取消)",
                    (true, true) => "管理员权限 (已启用)",
                }
                .to_string()
            }),
            Element::checkbox("", m.admin_launch),
            col1_w,
        )))
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    // ── 帮助（随包文档的入口）──
    //
    // 用户口径（2026-09-29）：说明书要"设置里点一下就能看"，而不是让用户去安装目录翻；
    // Lua 指南与 API 参考**合并为一份 HTML 手册**（`Lua脚本手册.html`，由仓库两份 .md
    // 生成，见 `temp/build_lua_manual.py`），避免"两个入口一个内容"的困惑。
    // 三份文档全部随 assets 发布，按钮直接 `ShellExecuteW` 打开：html 走默认浏览器。
    let (title, body, exp) = FeatureCard::new("帮助", help_expanded)
        .body({
            let manual = crate::core::paths::from_app_relative("assets/manual/隐藏快捷键手册.html");
            let lua_manual = crate::core::paths::from_app_relative("assets/lua/Lua脚本手册.html");
            Element::col()
                .child(card::option_row(
                    "隐藏快捷键手册",
                    Element::button("打开").on_click(move |_ctx| {
                        log::info!("打开文档：隐藏快捷键手册（{}）", manual.display());
                        crate::platform::win_util::open_doc(&manual);
                    }),
                    col1_w,
                ))
                .child(card::option_row(
                    "Lua 脚本手册",
                    Element::button("打开").on_click(move |_ctx| {
                        log::info!("打开文档：Lua 脚本手册（{}）", lua_manual.display());
                        crate::platform::win_util::open_doc(&lua_manual);
                    }),
                    col1_w,
                ))
        })
        .build_title_row(col1_w, card::INDENT_CARD);
    content = content
        .child(title)
        .child(body.visible_when(move || exp.get()));

    Element::scroll().child(content.padding(16))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本文件的**生产代码**文本（测试模块之前那一段）。
    /// 同款先例见 `ui::pages::screen::tests::production_src`——
    /// `include_str!` 会把测试自己也包进来，必须切一刀，否则"数出现次数"
    /// 的断言会被断言自己的文本影响。
    ///
    /// ★ 切点是**测试模块**（`mod tests`）而不是第一个 `#[cfg(test)]`：本文件在
    ///   `MODULE_NAMES`（第 56 行）上就挂了 `#[cfg(test)]`，按"第一个 `#[cfg(test)]`"
    ///   切下来只有文档与 import——那会让下面每条"源码里有 X"的断言都变成在查文档，
    ///   恒真（`HotkeyField::Panel` 恰好也在文档里提过，才一直没暴露）。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("global.rs");
        let cut = SRC
            .find("#[cfg(test)]\nmod tests")
            .unwrap_or_else(|| panic!("global.rs 里找不到测试模块起点"));
        &SRC[..cut]
    }

    /// 列宽表的显示名与集合表的信号名必须逐字同序——两张表各写一份就会走偏，
    /// 表现为"设置里的名字"与"功能页卡片标题"对不上（这张卡的核心价值所在）。
    #[test]
    fn module_switch_names_match_the_static_table() {
        let cfg = crate::core::config::Config::default();
        let m = PanelModel::from_config(&cfg);
        let switches = module_switches(&m);
        assert_eq!(
            switches.len(),
            MODULE_NAMES.len(),
            "MODULE_NAMES 与 module_switches 长度不一致"
        );
        for ((name, _), expect) in switches.iter().zip(MODULE_NAMES.iter()) {
            assert_eq!(name, expect, "模块开关名字表错位：{name} ≠ {expect}");
        }
    }

    /// ★ 集合完整性门禁：每个 `HotkeyField` 变体必须在 [`HOTKEY_ENTRIES`] 里恰好出现
    /// 一次。新加快捷键字段（新模块或新键）忘了登记时，本测试立刻红——否则设置页的
    /// 集合悄悄缺一行，用户只能去各页找。`all` 表逐个列出变体：加变体不更新这里，
    /// 长度断言（24 ↔ 表长）也会红。
    #[test]
    fn hotkey_entries_cover_every_field_exactly_once() {
        use HotkeyField::*;
        let all = [
            Screen,
            White,
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
            // 序号贴纸（2026-10-01 新增，出厂留空）。
            Stamp,
            Shot,
            ShotPin,
            PinClip,
            ShotScroll,
            ShotScrollWide,
            Record,
            RecordRegion,
            RecordTrim,
            Gif,
            Trim,
            Timer,
            FullscreenZoom,
            PipToggle,
            Sticky,
            Clipboard,
            Panel,
        ];
        assert_eq!(
            all.len(),
            HOTKEY_ENTRIES.len(),
            "HotkeyField 变体数与快捷键集合行数不一致：有变体没进表，或表里有废行"
        );
        for f in all {
            let hits = HOTKEY_ENTRIES.iter().filter(|(_, g)| *g == f).count();
            assert_eq!(hits, 1, "{f:?} 在快捷键集合里出现了 {hits} 次（应为 1）");
        }
    }

    /// 集合行名必须是「模块 > 行标签」两段式（用户口径）：分隔符两边都要有内容，
    /// 否则用户在集合里认不出这把键属于哪个模块、也翻不到功能页对应的那一行。
    ///
    /// ★ 末段"不许改写"那条（旧口径）已取消：2026-09-27 起末段就是各页原文的逐字复用，
    ///   改写由 [`hotkey_collection_follows_the_ui_order_and_reuses_ui_text`] 管。
    #[test]
    fn hotkey_labels_are_module_qualified() {
        for (label, _) in HOTKEY_ENTRIES {
            let parts: Vec<&str> = label.split(" > ").collect();
            assert_eq!(
                parts.len(),
                2,
                "快捷键名「{label}」必须只有一段模块前缀 + 一段行标签"
            );
            assert!(
                !parts[0].is_empty() && !parts[1].is_empty(),
                "「{label}」两段都不能为空"
            );
        }
    }

    /// 功能页源码 + 该文件里顶层功能卡的模块序（= 侧栏导航页序 → 页内卡序）。
    ///
    /// 两张集合卡（模块开关 / 快捷键）都以它为正本。`trail` / `key_display` 两页此刻没有
    /// 任何快捷键行，模块序给空表即可。`lua` 页的卡片标题是运行期名字（`&card.name`），
    /// 不是字面量、扫不到，也不归本表的职责。
    ///
    /// ★ 用 `include_str!` 而不是运行时读文件：测试的工作目录不保证是 `main/`，路径会漂。
    const MODULE_PAGES: [(&str, &str, &[&str]); 5] = [
        ("trail.rs", include_str!("trail.rs"), &[]),
        ("key_display.rs", include_str!("key_display.rs"), &[]),
        (
            "screen.rs",
            include_str!("screen.rs"),
            &["屏幕标注", "画中画放大", "全屏放大"],
        ),
        (
            "capture.rs",
            include_str!("capture.rs"),
            &["截图与贴图", "GIF录制", "视频录制"],
        ),
        (
            "tool.rs",
            include_str!("tool.rs"),
            &["便签", "剪贴板", "计时器"],
        ),
    ];

    /// 抽源码里 `FeatureCard::new("…")` 的卡片标题（**源码顺序**）。
    ///
    /// 只认**字面量**标题：运行期名字（`FeatureCard::new(&card.name)` 那类）扫不出来，
    /// 也正因如此本门禁只覆盖"有固定中文标题"的功能卡。
    fn card_titles(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = src;
        const KEY: &str = "FeatureCard::new(\"";
        while let Some(i) = rest.find(KEY) {
            let start = i + KEY.len();
            match rest[start..].find('"') {
                Some(len) => {
                    out.push(rest[start..start + len].to_string());
                    rest = &rest[start + len + 1..];
                }
                None => break,
            }
        }
        out
    }

    /// 抽源码里 `card::hotkey_row("…")` 的行标签，并按 [`MODULE_PAGES`] 给每行认领所属
    /// 模块：**归"本文件里最后出现的那张模块卡"**（文件第一张模块卡之前的行归第一张）。
    ///
    /// ★ 认领靠"最近一次出现"，而不是"下一个待认领"——区别在于 `screen.rs` 的 11 根书写
    ///   快捷键行属于子卡 `快捷键`，而那张子卡是**先构建、后挂进父卡**的，源码里排在父卡
    ///   「屏幕标注」前面；按"下一个待认领"算，它们会被算到「画中画放大」头上。同理，
    ///   `画中画放大` 子卡里的那根行排在「画中画放大」卡之后、「全屏放大」卡之前，按
    ///   "最近出现"认领才落回「画中画放大」。
    fn hotkey_rows_in(src: &str, modules: &[&str]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut active = 0usize;
        let lines: Vec<&str> = src.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if let Some(title) = card_titles(line).into_iter().next() {
                if let Some(k) = modules.iter().position(|m| *m == title) {
                    active = k;
                }
            }
            if !line.contains("card::hotkey_row(") {
                continue;
            }
            // 标签在下一行（所有页都是这种换行写法）。
            let label = lines
                .get(i + 1)
                .and_then(|l| l.trim().strip_prefix('"'))
                .and_then(|l| l.split_once('"'))
                .map(|(l, _)| l.to_string())
                .unwrap_or_else(|| panic!("第 {i} 行的 card::hotkey_row 后没找到行标签"));
            out.push((modules.get(active).unwrap_or(&"?").to_string(), label));
        }
        out
    }

    /// 脚本开关镜像的**完整性**（2026-09-28）：镜像行 = 脚本页收录范围
    /// （`LuaExtCard::is_script_card`），一行不多、一行不少；每行的开关初值必须与
    /// 配置判定 `role_is_active` 一致（绑错信号在这里就会红）。
    #[test]
    fn script_switches_mirror_the_lua_page_cards() {
        let cfg = crate::core::config::Config::default();
        let m = PanelModel::from_config(&cfg);
        let mirror: Vec<&crate::ui::model::LuaExtCard> = m
            .lua_ext_cards
            .iter()
            .filter(|c| c.is_script_card())
            .collect();
        // 出厂脚本全关（2026-09-27 口径），镜像初值必须如实反映。
        for c in &mirror {
            assert!(
                !c.toggle.get(),
                "脚本 {} 的镜像开关初值应为关（出厂全关）",
                c.id
            );
            assert_eq!(
                c.toggle.get(),
                cfg.lua_ext.role_is_active(&c.role, &c.id),
                "脚本 {} 的开关初值与配置判定不一致",
                c.id
            );
        }
        // 脚本页出卡数 == 镜像行数（同一谓词，跑两遍防手抖改掉其中一处）。
        let page_cards = m
            .lua_ext_cards
            .iter()
            .filter(|c| c.is_script_card())
            .count();
        assert_eq!(mirror.len(), page_cards);
    }

    /// 模块开关的顺序 = UI 实际卡序（用户口径 2026-09-27）。
    ///
    /// 正本不是 [`MODULE_NAMES`] 自己，而是**各页源码**：把 5 个功能页里所有字面量卡片
    /// 标题按源码顺序列出来，凡属 [`MODULE_NAMES`] 的（= 有总开关的模块）就是 UI 顺序。
    /// 于是"把「画中画放大」拖到「屏幕标注」上面"这类改动会直接红——设置页跟着漂而没人
    /// 改表的时代到此为止。
    #[test]
    fn module_switches_follow_the_page_card_order() {
        let mut ui_order: Vec<String> = Vec::new();
        for (_, src, _) in MODULE_PAGES {
            for title in card_titles(src) {
                if MODULE_NAMES.contains(&title.as_str()) {
                    ui_order.push(title);
                }
            }
        }
        let cfg = crate::core::config::Config::default();
        let switches = module_switches(&PanelModel::from_config(&cfg));
        assert_eq!(
            switches.len(),
            ui_order.len(),
            "UI 里带总开关的卡数与 `module_switches` 行数不一致（有模块没镜像，或镜像了不存在的卡）"
        );
        for ((name, _), expect) in switches.iter().zip(ui_order.iter()) {
            assert_eq!(
                *name,
                expect.as_str(),
                "模块开关顺序与 UI 卡序错位：{name} ≠ {expect}"
            );
        }
    }

    /// 快捷键集合的**行序与行名** = 各页 UI（用户口径 2026-09-27）。
    ///
    /// 正本同样是源码：按导航页序扫 [`MODULE_PAGES`]，把 `card::hotkey_row` 的行标签按
    /// 出现顺序列出来，段名取它所属模块卡。集合第一行是「控制面板」（全局项，不归属任何
    /// 模块卡，用户口径要求固定第一），故从第二行起比对。
    ///
    /// 这一条同时钉住两件事：**末段逐字复用各页原文**（改短/改写 → 红）与**顺序 = 页序**
    /// （挪动卡片或行 → 红）。改动功能页的行名时，请顺手把集合里的对应项一起改——红就是
    /// 在提醒"用户拿这两个名字对不上了"。
    #[test]
    fn hotkey_collection_follows_the_ui_order_and_reuses_ui_text() {
        let mut derived: Vec<String> = Vec::new();
        for (_, src, modules) in MODULE_PAGES.iter() {
            for (module, label) in hotkey_rows_in(src, modules) {
                derived.push(format!("{module} > {label}"));
            }
        }
        let (first, rest) = HOTKEY_ENTRIES.split_first().unwrap();
        assert_eq!(
            first.0, "控制面板 > 显示/隐藏",
            "「控制面板」是全局项，集合第一行（用户口径）"
        );
        assert_eq!(
            derived.len(),
            rest.len(),
            "各页 UI 里共 {} 行快捷键，与集合 {} 行不一致：有新键忘了进集合，或集合里混了废行",
            derived.len(),
            rest.len()
        );
        for (got, (label, _)) in derived.iter().zip(rest.iter()) {
            assert_eq!(got, label, "快捷键集合与 UI 不同步：{got} ≠ {label}");
        }
    }

    /// 屏幕页「快捷键」子卡里那批**工具**行，行序必须与 [`DRAW_TOOL_ORDER`] 一致
    /// （2026-10-01 用户口径："画笔等等的顺序也和工具栏一样"，橡皮擦在马赛克**后面**）。
    ///
    /// 顺序正本在 core（`DRAW_TOOL_ORDER`，工具栏按钮序也照它排），本测试负责"界面这一头"
    /// 没抄歪——与 `hotkey_collection_follows_the_ui_order_and_reuses_ui_text` 合起来
    /// 钉住三处同序（工具栏 / 屏幕页行 / 设置页集合表）。
    ///
    /// ★ 抽出来的是 `screen.rs` 源码里 `card::hotkey_row("…")` 的**字面量标签**，因此
    ///   改行名（而不是只改顺序）同样会在这里暴露——这正是要的（行名只应改一处）。
    #[test]
    fn screen_hotkey_rows_follow_the_tool_order() {
        const TOOL_ROWS: [&str; 11] = [
            "画笔1",
            "画笔2",
            "画笔3",
            "直线",
            "箭头",
            "矩形",
            "椭圆",
            "文字",
            "序号",
            "马赛克",
            "橡皮擦",
        ];
        let (_, src, modules) = MODULE_PAGES
            .iter()
            .find(|(f, _, _)| *f == "screen.rs")
            .expect("MODULE_PAGES 里没有 screen.rs");
        let rows = hotkey_rows_in(src, modules);
        let screen_rows: Vec<String> = rows
            .into_iter()
            .filter(|(module, _)| module == "屏幕标注")
            .map(|(_, label)| label)
            .collect();
        // 前两行是开关（屏幕标注 / 白板），之后才是工具。
        let tools = &screen_rows[2..];
        assert_eq!(
            tools, TOOL_ROWS,
            "屏幕页快捷键的工具行序与工具栏顺序不一致：{tools:?}"
        );
        assert_eq!(
            TOOL_ROWS.len(),
            crate::core::hotkeys::DRAW_TOOL_ORDER.len(),
            "工具序表（core）与界面行标签数不一致"
        );
    }

    /// 结构门禁：面板快捷键必须走捕获框（`HotkeyField::Panel`），不得退回文本框——
    /// 集合里 23 行同一形状是本次拍板的口径；文本框一回潮，"为什么就它要手打"的
    /// 疑惑也跟着回来。
    #[test]
    fn panel_hotkey_is_a_capture_row_not_a_text_input() {
        let p = production_src();
        assert!(
            p.contains("HotkeyField::Panel"),
            "面板快捷键必须以 HotkeyField::Panel 进集合"
        );
        assert!(
            !p.contains("text_input"),
            "设置页不得再出现文本框形态的快捷键（面板快捷键已并入捕获体系）"
        );
    }

    /// 「工作文件夹」卡三件事必须齐备：**占用（信号驱动的文本）** /
    /// **打开目录按钮**，以及**打开面板时先算一次**。
    ///
    /// ★ 不摆「位置」行（2026-09-28 用户口径）：路径显示不全也抄不了，要过去点
    ///   「打开文件夹」就行——负面钉住，防止它被"顺手加回来"。
    ///
    /// 缺哪一样都不会报错——那张卡只是少一行、或者点了没反应。这类退步在真机上要
    /// 点到那张卡才看得见，所以在源码层钉住。
    #[test]
    fn temp_card_wires_path_size_button_and_the_initial_scan() {
        let p = production_src();
        for needle in [
            "FeatureCard::new(\"工作文件夹\"",
            "m.expand(\"global.temp\")",
            "\"占用\"",
            "\"打开文件夹\"",
            // 打开目录走平台层的收口函数，不在这页自己拼 `explorer.exe` 命令行。
            "open_in_explorer",
            // 首次统计（建面板那一帧）——走后台线程那条通路。
            "temp_usage::spawn_scan",
        ] {
            assert!(p.contains(needle), "设置页「工作文件夹」卡缺少：{needle}");
        }
        // ★ 负面：卡里不得再出现「刷新」按钮——占用**每次打开面板都会重算**，那个按钮
        //   纯属白摆的控件（2026-09-27 用户口径）。它很容易被"顺手加回来"，所以钉住。
        assert!(
            !p.contains("\"刷新\""),
            "「工作文件夹」卡不该再有「刷新」按钮（重开面板即是重算）"
        );
        // ★ 负面：「位置」行已删——路径文本显示不全，等于白摆一行。
        assert!(
            !p.contains("\"位置\""),
            "「工作文件夹」卡不该再有「位置」行（显示不全，要过去用「打开文件夹」）"
        );
    }

    /// 两组合并卡的结构（2026-10-07）：**「功能开关」=「模块开关」+「脚本开关」两段、
    /// 「快捷键」=「模块快捷键」+「脚本快捷键」两段**，段与段之间用 `divider()` 分隔，
    /// 段名由 `group_label` 给出。原来那两张独立卡（`脚本开关` / `脚本快捷键`）不该再有。
    #[test]
    fn merged_cards_are_two_sections_each_with_dividers() {
        let p = production_src();
        for needle in [
            "FeatureCard::new(\"功能开关\"",
            "card::group_label(\"模块开关\")",
            "card::group_label(\"脚本开关\")",
            "FeatureCard::new(\"快捷键\"",
            "card::group_label(\"模块快捷键\")",
            "card::group_label(\"脚本快捷键\")",
            "card::divider()",
        ] {
            assert!(p.contains(needle), "设置页合并卡缺少：{needle}");
        }
        for stale in [
            "FeatureCard::new(\"模块开关\"",
            "FeatureCard::new(\"脚本开关\"",
            "FeatureCard::new(\"脚本快捷键\"",
        ] {
            assert!(!p.contains(stale), "已合并的独立卡不该还在：{stale}");
        }
        // 段名组件必须存在（行组件正本在 card.rs）。
        let c = include_str!("../card.rs");
        assert!(
            c.contains("pub fn group_label("),
            "card.rs 缺少 group_label"
        );
    }

    /// 段名必须**占满一行**（`height(ROW_H)` + `cross(Align::Center)`）：只给文字高度
    /// 会让段名到下一行的中心距短于行距，整列节奏在段名处塌一下（2026-10-07 实测）。
    #[test]
    fn group_label_shares_the_row_rhythm() {
        let src = include_str!("../card.rs");
        let at = src
            .find("pub fn group_label(")
            .expect("card.rs 缺少 group_label");
        // ★ 按**行**取，不按字节切：源码里全是中文注释，切在字符中间会直接 panic。
        let body: String = src[at..].lines().take(14).collect::<Vec<_>>().join("\n");
        for needle in [
            ".height(ROW_H)",
            "cross(Align::Center)",
            "padding_xy(16, 0)",
        ] {
            assert!(body.contains(needle), "group_label 缺少行节奏：{needle}");
        }
        // ★ 段名 = 暗红强调色 + 加粗（2026-10-07 用户口径：主色不够显眼）。
        //   这两条一起才是"段名看得出来"的判据，缺任一条就退回"跟普通行一样"。
        assert!(
            body.contains("fg(to_color(tokens::TEXT_ACCENT))"),
            "段名必须是强调色 TEXT_ACCENT"
        );
        assert!(body.contains("font_weight(600)"), "段名必须加粗");
    }

    /// 「系统」卡（2026-10-07）：管理员权限勾选项必须齐备——行名即状态（四态：当前
    /// 进程权限 × 勾选，随勾选联动）、勾选信号绑定，以及 CARD_IDS 里的卡片 id。
    #[test]
    fn system_card_wires_admin_launch_toggle_and_hint() {
        let p = production_src();
        for needle in [
            "FeatureCard::new(\"系统\"",
            "m.expand(\"global.system\")",
            "管理员权限 (未启用)",
            "管理员权限 (重启后有效)",
            "管理员权限 (重启后取消)",
            "管理员权限 (已启用)",
            "m.admin_launch",
            "option_row_signal(",
        ] {
            assert!(p.contains(needle), "设置页「系统」卡缺少：{needle}");
        }
    }
}
