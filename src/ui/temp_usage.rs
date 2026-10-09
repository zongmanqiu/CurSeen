//! 面板「工作文件夹」卡（设置页）的可见状态与后台统计编排。
//!
//! # 为什么状态单独放一处
//!
//! 与 `update::state` 同一套形状：`Signal` 是 windui 的类型（`core` 禁 windui），所以
//! "后台算出来、面板上显示"的这份状态进不了 `core`；而扫描本身（纯 `std::fs`）也不该
//! 挤进 `ui`——它在 `core::temp_usage`，可脱离 UI 单测。
//!
//! # 为什么必须在后台线程
//!
//! 首次统计发生在**建面板那一帧**（`build_global_page`）。`temp/record` 里攒上几十个 MP4
//! 时一次递归 stat 是几十毫秒量级——留在 UI 线程就是"开面板卡一下"，而这种卡顿极难归因
//! 到一张"只是显示个数字"的卡上（用户会去怀疑渲染层）。
//!
//! ★ `Signal` 是 `!Send`（windui 用裸指针把它变成编译期约束），所以后台线程连"顺手
//! `set` 一下文本"都做不到——只能把结果作为**纯数据**发消息回 UI 线程。这条纪律有编译器
//! 兜着，写错根本编不过。
//!
//! # 状态为什么常驻 `Runtime` 而不是随面板新建
//!
//! 与 `UpdateState` 同理：统计可能在面板**已经关掉**的时候才回来。状态跟着面板走的话，
//! 用户扫一眼就关面板，那条消息就没处落——`apply_temp_usage` 会去写一个已丢弃的信号。
//! 常驻则天然幂等：面板再打开时读到的就是最后一次算出来的值。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use windui::prelude::*;

use crate::core::events::UiMsg;
use crate::core::temp_usage::{scan_temp, TempUsage};

/// 「统计中」文案：每次统计一开跑就先落到它上面。
///
/// 先写它而不是留着上一次的数字，是为了让"正在算"这件事**看得见**——大目录要算几百
/// 毫秒，期间停在旧数字上会让人以为面板读的是缓存。
const BUSY_TEXT: &str = "统计中…";

/// 面板可见的临时目录占用状态。
#[derive(Clone)]
pub struct TempUsageState {
    /// 「占用」那一行的文本（见 [`describe`]）。**本结构唯一的显示状态。**
    pub text: Signal<String>,
    /// 一次统计在飞（重入保护）。
    ///
    /// 不是显示状态、也不驱动任何控件，所以是 `AtomicBool` 而不是 `Signal`——它的读者
    /// 只有 [`spawn_scan`] 自己。没有它的症状是连点（或面板反复重建）开出好几条线程、
    /// 各自把结果写一遍：数字最终是对的，但中间会在几个旧值之间闪。
    ///
    /// ★ 必须 `Arc` 共享：`TempUsageState` 是按值克隆着传的（`Runtime` 持有，再经
    /// `PanelHost::temp_usage_state()` 克隆一份进面板），普通字段一克隆就分成两份，
    /// 这道保护位当场失效。
    scanning: Arc<AtomicBool>,
}

impl Default for TempUsageState {
    fn default() -> Self {
        Self::new()
    }
}

impl TempUsageState {
    /// 在 UI 线程创建（`Signal` 是 `!Send`，只能在这儿建）。
    pub fn new() -> Self {
        Self {
            text: signal(BUSY_TEXT.to_string()),
            scanning: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 一次统计是否还在飞（[`spawn_scan`] 的重入判据）。
    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Relaxed)
    }

    /// 把一次统计结果写进信号（由 `Runtime::apply_temp_usage` 在 UI 线程调用）。
    pub fn apply(&self, usage: &TempUsage) {
        self.text.set(describe(usage));
        self.scanning.store(false, Ordering::Relaxed);
    }

    /// 统计起不来时把原因写到面板上。
    ///
    /// 不能只留「统计中…」：那句话的意思是"稍等"，而这句话反过来会永久停在那里，
    /// 用户会一直等一个不会来的数字。
    pub fn fail(&self, why: &str) {
        self.text.set(format!("（统计失败：{why}）"));
        self.scanning.store(false, Ordering::Relaxed);
    }
}

/// 统计结果 → 面板文案。
///
/// 三种形态各有各的话说：**还没产生过**（目录都没有）/ **空的**（有目录没文件）/
/// 有东西。前两种都会被 `format_bytes` 印成 `0 B`，那会让人以为"有东西但很小"。
pub fn describe(usage: &TempUsage) -> String {
    if !usage.exists {
        return "尚未产生文件".to_string();
    }
    if usage.usage.files == 0 {
        return "暂无文件".to_string();
    }
    format!(
        "{}（{} 个文件）",
        crate::core::temp_usage::format_bytes(usage.usage.bytes),
        usage.usage.files
    )
}

/// 起一次后台统计；结果经 [`UiMsg::TempUsage`] 回来。
///
/// 已在统计中则直接返回（**不是错误**）：本函数在建面板那一帧就会被调一次，而面板会因
/// 切页、改展开态等缘故重建——没有这层保护，每次重建都会再开一条线程。
///
/// 没有 UI 通道（未装配的裸 `Runtime`、单测）或线程起不来时，把失败写在面板文本上并
/// 返回可读错误——**绝不 panic**。
pub fn spawn_scan(state: &TempUsageState, tx: &Option<Sender<UiMsg>>) -> Result<(), String> {
    if state.is_scanning() {
        return Ok(());
    }
    let tx = tx
        .clone()
        .ok_or_else(|| "UI 通道尚未就绪".to_string())
        .inspect_err(|e| state.fail(e))?;
    state.text.set(BUSY_TEXT.to_string());
    state.scanning.store(true, Ordering::Relaxed);
    // 线程名固定，便于在调试器里认出来（与 `update::task::spawn` 同款）。
    let spawned = std::thread::Builder::new()
        .name("curseen-temp-usage".to_string())
        .spawn(move || {
            let _ = tx.send(UiMsg::TempUsage(scan_temp()));
        });
    if let Err(e) = spawned {
        log::error!("启动临时目录统计线程失败: {e}");
        state.fail(&e.to_string());
        return Err(format!("启动统计线程失败: {e}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::temp_usage::Usage;

    fn made(bytes: u64, files: u64) -> TempUsage {
        TempUsage {
            exists: true,
            usage: Usage { bytes, files },
        }
    }

    /// 三种形态三句话——把 `0 B` 与"空"、"尚未产生"分开，是本函数的全部价值。
    #[test]
    fn describe_tells_the_three_states_apart() {
        assert_eq!(describe(&TempUsage::default()), "尚未产生文件");
        assert_eq!(describe(&made(0, 0)), "暂无文件");
        assert_eq!(describe(&made(46_246_936, 30)), "44.1 MB（30 个文件）");
    }

    /// 没通道时必须**写清原因**而不是留「统计中…」——后者会让用户一直等一个不会来的数字。
    #[test]
    fn scan_without_channel_reports_the_reason_on_the_panel() {
        let state = TempUsageState::new();
        assert!(spawn_scan(&state, &None).is_err());
        let text = state.text.get();
        assert!(text.contains("统计失败"), "面板文本应写明失败：{text}");
        assert!(!state.is_scanning(), "失败后不得留在『统计中』态");
    }

    /// 上一次还在飞时必须**直接放行**（返回 `Ok`），而不是往下走去报"通道未就绪"——
    /// 它本来就不是失败。判据：拿一个**没有通道**的状态来试，若保护失效就会变成 `Err`。
    #[test]
    fn a_second_scan_while_one_is_in_flight_is_not_an_error() {
        let state = TempUsageState::new();
        state.scanning.store(true, Ordering::Relaxed); // 模拟"统计在飞"
        assert!(
            spawn_scan(&state, &None).is_ok(),
            "重入必须静默放行，不得报错"
        );
    }
}
