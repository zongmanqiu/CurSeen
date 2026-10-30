//! 在线脚本库的 UI 状态（m36 / m38 / m46）。
//!
//! 与 [`crate::update::state::UpdateState`] 同一条理由**随 `Runtime` 常驻**而不是随面板新建：
//! 下载/安装可能在面板已经关掉之后完成，状态跟着面板走的话那条消息就没处落。

use windui::prelude::*;

use crate::core::events::StorePackRow;
use crate::scripting::store::SOURCE_LABELS;

/// 面板可见的在线脚本库状态。
#[derive(Clone)]
pub struct StoreState {
    /// 红色状态行：**普通态 = `N 个可下载`**；动作态只放 7 个短词之一
    /// （`检查中…` / `下载中…` / `已更新` / `已卸载` / `安装失败` / `卸载失败` / `离线`）。
    /// 包名、URL、错误原文、增删计数都进日志。
    pub status: Signal<String>,
    /// 后台动作进行中（刷新/安装/卸载同一时刻只跑一个；按钮据此置灰）。
    pub busy: Signal<bool>,
    /// 清单 × 本机安装记录的合并视图。
    pub rows: Signal<Vec<StorePackRow>>,
    /// 选中的镜像（下标同 [`SOURCE_LABELS`]；只影响"先试哪个地址"）。
    pub source: Signal<usize>,
    /// 正在下载的那一枚 + 百分比（行尾按钮显示 `42%`；`None` = 没有下载在进行）。
    pub download: Signal<Option<(String, u8)>>,
}

impl Default for StoreState {
    fn default() -> Self {
        Self::new(0)
    }
}

impl StoreState {
    /// 在 UI 线程创建（`Signal` 是 `!Send`，只能在这儿建）。
    ///
    /// 行表先读**随包清单 + 已装记录**（`store_task::cached_rows`，不联网）；联网只发生在
    /// 用户点「刷新」那一刻。出厂状态 = `N 个可下载`（普通态），不摆摘要。
    pub fn new(source: usize) -> Self {
        let source = source.min(SOURCE_LABELS.len() - 1);
        let rows = crate::scripting::store_task::cached_rows();
        let status = format!("{} 个可下载", downloadable_count(&rows, source));
        Self {
            status: signal(status),
            busy: signal(false),
            rows: signal(rows),
            source: signal(source),
            download: signal(None),
        }
    }
}

/// 普通（空闲）状态：当前镜像有地址、且不需要更新宿主的清单条目数。
///
/// 它包含"已装但可升级/已装最新"的条目——用户问的是商店里能下载多少个包，
/// 不是"本机还缺几个"；行上的按钮才区分下载/升级/已装。
pub fn downloadable_count(rows: &[StorePackRow], source: usize) -> usize {
    let bit = 1u8 << source.min(SOURCE_LABELS.len() - 1);
    rows.iter()
        .filter(|r| !r.too_new && r.source_mask & bit != 0)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(mask: u8, too_new: bool) -> StorePackRow {
        StorePackRow {
            id: "p".to_string(),
            name: "p".to_string(),
            version: "1.0".to_string(),
            description: String::new(),
            size: 1,
            installed_version: None,
            too_new,
            files: 0,
            source_mask: mask,
            author: String::new(),
            license: String::new(),
            released: String::new(),
            api: 0,
            unpacked_size: 0,
            file_count: 1,
        }
    }

    /// 普通状态只数"当前镜像有地址、宿主够新"的条目；没地址/太新都不算。
    #[test]
    fn downloadable_count_follows_the_selected_source() {
        let rows = [
            row(0b11, false),
            row(0b01, false),
            row(0b11, true),
            row(0b00, false),
        ];
        assert_eq!(downloadable_count(&rows, 0), 2, "太新的那条不算");
        assert_eq!(downloadable_count(&rows, 1), 1, "只有双镜像那条算 GitHub");
    }
}
