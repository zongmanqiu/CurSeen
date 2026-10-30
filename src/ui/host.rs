//! 控制面板的**宿主接口**——依赖倒置（对齐 Pecia 的 `core/UiBridge.h`）。
//!
//! # 为什么要这一层
//!
//! 面板本来直接抓 `Rc<RefCell<Runtime>>`：要配置快照、要把模型登记回去、要更新状态、
//! 要一个往 UI 线程发意图的发送端。代价是 `ui` 反向依赖 `runtime`——**界面层摸主循环**，
//! 于是：
//! - 想给面板写个测试就得先装配整个运行时；
//! - `runtime` 一改签名，`ui` 跟着抖；
//! - 分层图上出现 `ui → runtime` 这条逆向边，`core ← modules ← ui ← runtime` 的纪律作废。
//!
//! 正解是**宿主回调**：面板声明"我需要什么"（本 trait），由 `Runtime` 实现并在建窗时注入。
//! `ui` 因此只依赖 `core` / `update` / `app`，再也沒有 [`crate::runtime`] 这个依赖。
//!
//! # 为什么这里是 trait 而不是一串参数
//!
//! 五个成员里有两个（`config_snapshot` / `open_panel`）是**建树一次性**动作，另外三个
//! （`update_state` / `temp_usage_state` / `ui_sender`）是要交给 `'static` 闭包捕获的
//! **值**。trait 把"面板与宿主的全部接触面"钉在一处，将来要加成员只会在这里出现，
//! 比散在函数签名里好审。

// ★★ `Sender` 必须是 **windui** 的那个（`windui::sync::Sender`），不是
// `std::sync::mpsc::Sender`。前者是后者的包装，额外携带"唤醒 UI 线程一帧"的 waker——
// 换成裸 std 通道，后台线程的消息虽然能 `send` 成功，UI 却永远不会被唤醒重绘，
// 表现为"更新进度条不动"。而 `windui::prelude::*` 会把 `Sender` 这个名字遮蔽掉，
// 一旦在本 crate 里写 `use std::sync::mpsc::Sender` 就会静默踩坑，故此处显式标注来源。
use windui::prelude::Sender;

use crate::core::config::Config;
use crate::core::events::UiMsg;
use crate::ui::model::PanelModel;
use crate::ui::temp_usage::TempUsageState;
use crate::update::state::UpdateState;

/// 面板向其宿主索要的全部能力。
pub trait PanelHost {
    /// 建树时的配置快照。
    ///
    /// 返回**值**而不是引用：调用方马上要用它造 `PanelModel`，没必要把整个宿主借用
    /// 到那一刻之后（`RefCell` 借用持太久了容易和自家成员的借用撞车）。
    fn config_snapshot(&self) -> Config;

    /// 把刚建好的模型登记回宿主（每帧 diff 回配置就靠这份句柄）。
    fn open_panel(&mut self, model: PanelModel);

    /// 更新相关的 UI 可见状态。**返回拥有值**：按钮的 `on_click` 闭包是 `'static`，
    /// 必须拿到能拷贝进去的东西。
    fn update_state(&self) -> UpdateState;

    /// 在线脚本库状态（m36）：行表 / 状态行 / 忙碌位 / 两段式卸载的待确认项。
    ///
    /// 与 [`Self::update_state`] 同款：`Signal` 组成的值，只能由宿主（UI 线程）持有并拷进面板。
    fn store_state(&self) -> crate::ui::store_state::StoreState;

    /// 设置页「工作文件夹」卡的状态（`<exe>/temp` 的占用文本 + 统计重入位）。
    ///
    /// 与 [`Self::update_state`] 同款：`Signal` 组成的值，只能由宿主（UI 线程）持有并
    /// 拷贝给面板；后台统计线程碰不到它（见 `ui::temp_usage`）。
    fn temp_usage_state(&self) -> TempUsageState;

    /// 往 UI 线程送意图的发送端。未装配时为 `None`（此时更新按钮点了会报错而不是 panic）。
    fn ui_sender(&self) -> Option<Sender<UiMsg>>;
}
