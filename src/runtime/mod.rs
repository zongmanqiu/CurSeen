//! 运行时模块。管理三个特效模块的完整生命周期。

mod frame;
mod gif;
mod input;
pub mod layer;
pub mod overlay;
mod panel;
mod record;
pub mod render;
mod scroll;
mod sel_geom;
pub mod shot_edit;
mod snap_flow;
mod spotlight;
#[cfg(test)]
mod testkit;
mod timer;
mod zoom;

// 各域搬走后留下的自由函数 / 类型在这里重新导出：子模块一句 `use super::*` 就能取到，
// 不必各自去点兄弟模块的路径（兄弟互相 `use` 会让依赖关系变成一张网）。
pub(crate) use self::input::{hk_matches_or_ctrl, shot_edit_entry};
pub(crate) use self::record::{draw_rec_countdown, rec_countdown_next_change_ms, REC_COUNTDOWN_MS};
pub(crate) use self::sel_geom::{apply_zone, clamp_sel, sel_rect};
pub(crate) use self::snap_flow::timestamp;

use crate::app::UiMsg;
use crate::core::backdrop::Backdrop;
use crate::core::color::Argb;
use crate::core::config::Config;
use crate::core::config::RECORD_RESOLUTION_LONG_EDGE;
use crate::core::events::UpdateEvent;
use crate::core::image::{RgbaBitmap, Shot};
use crate::ui::temp_usage::TempUsageState;
use crate::ui::PanelHost;
use crate::update::state::UpdateState;
use std::rc::Rc;
use std::time::Instant;

use crate::core::draw::{Blend, BlendMode, DrawCmd, DrawList, Paint, Shape};
use crate::core::events::{InputEvent, KeyCode, Modifiers, MouseButton};
use crate::core::geometry::{Point, Rect};
use crate::core::gif_zone::HitZone;
use crate::core::hotkeys::{
    is_modifier_key, matches as hk_matches, pressed, pretty_hotkey, with_ctrl,
};
use crate::core::tick::FrameClock;
use crate::modules::clipboard::ClipboardModule;
use crate::modules::drawing::controller::DrawRequest;
use crate::modules::drawing::notify::Notify;
use crate::modules::drawing::DrawingModule;
use crate::modules::key_display::KeyDisplayModule;
use crate::modules::key_sound::KeySoundModule;
use crate::modules::magnifier::fullscreen::FullscreenMagnifier;
use crate::modules::magnifier::pip::PipMagnifier;
use crate::modules::sticky_note::StickyNoteModule;
use crate::modules::trail::TrailModule;
use crate::modules::virtual_keyboard::VirtualKeyboardModule;
use crate::modules::Module;
use crate::platform::display::{cursor_pos, ScreenRect};
use crate::platform::recorder::{Hardware, RecKind, RecOptions, RecordArea, Recorder};
use crate::platform::screenshot;
use crate::platform::text_raster;
use crate::platform::win_util::tick_ms;
use crate::runtime::layer::region_magnifier::RegionMagnifier;
use crate::runtime::layer::{EscapeTarget, LayerSession};
use crate::runtime::shot_edit::ShotEdit;

pub mod snap;
use crate::ui::model::PanelModel;

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windui::prelude::Sender;

// 截图 / 录屏的反馈浮字：与标注浮字用同一套 `Notify`，但生命周期独立（一条截图提示
// 不该被一条标注提示覆盖）。两者都画在屏幕中央，中心位置互斥不重叠是 UI 细节，这里不纠结。

/// 面板改动后多久落一次盘（毫秒）。拖滑块时每帧都会改，防抖避免高频写文件。
const PANEL_SAVE_DEBOUNCE_MS: u64 = 600;

/// ★ 输入门（补丁 012）：**是否有任何模块消费鼠标输入**。
///
/// 判据是"有没有鼠标的消费者"，不是"要不要出帧"：鼠标事件的消费者只有拖尾
/// （`push_sample` / `on_click`，受 `enabled_trail` 管）和屏幕书写（`drawing.tick`，受
/// `enabled_draw` 管）。击键回显不消费鼠标，故 `enabled_key` 单独打开时也不该放行鼠标。
///
/// 为什么需要它：输入桥接线程把每条事件都经 `App::channel` 发出去，而**每次 `send` 都会
/// 唤醒 UI 线程一帧**（`Waker::wake` → `PostMessageW(WM_APP_WAKE)`）。低频钩子实测鼠标
/// 移动约 1000 条/秒，于是一台什么都没开的机器也要被鼠标移动钉成 1000 帧/秒的唤醒。
/// 原 1.0 线有同样的门（`app::runtime::INPUT_NEEDS_FRAME`），2.0 重写时丢了。
///
/// 读端在桥接线程（`RUNTIME::spawn_input_bridge`），故走原子；写端在 UI 线程的
/// [`Runtime::sync_ui`]（每帧 + 250ms 对账都经过它）。
pub static MOUSE_INPUT_NEEDED: AtomicBool = AtomicBool::new(false);

/// ★ 虚拟键盘的**鼠标按下/抬起**门（2026-09-26 新增）。
///
/// 为什么它不能并进 [`MOUSE_INPUT_NEEDED`]：那道门管的是**所有**鼠标事件，而
/// `mouse_input_needed()` 里虚拟键盘只在「启用且**未勾鼠标穿透**」时才算数 ——
/// 因为穿透态不需要收 1000 条/秒的 `MouseMove`（命中区是空的，收了也没用）。
///
/// 但"鼠标穿透"只决定**点键帽要不要注入真实按键**，跟"键帽要不要亮"是两件事：
/// 后者唯一的输入就是 `MouseDown`/`MouseUp`。焊在一起的下场是 —— 勾着穿透的方案
/// （WASD 游戏板出厂即勾，2026-09-26 用户报"鼠标点击之后居然没有反应"）其鼠标
/// 事件被桥接线程整条丢弃，鼠标键帽永远不亮，而键盘键一切正常（键盘走的是按键门，
/// 不受影响），一眼看上去就像"鼠标键坏了"。
///
/// 按下/抬起是**低频**事件（一次点击最多两条），单独开这道门不会把通道钉成高频唤醒；
/// `MouseMove` 依旧只认 [`MOUSE_INPUT_NEEDED`]。
///
/// 写端在 [`Runtime::sync_input_gate`]，判据是"方案里有没有鼠标键帽"
/// （[`crate::modules::virtual_keyboard::VirtualKeyboardModule::wants_mouse`]）。
pub static VK_MOUSE_ARMED: AtomicBool = AtomicBool::new(false);

/// ★ 数位板"起笔 0.5 秒延迟"直通（2026-09-28，三报根治）。
///
/// 笔的输入走覆盖层窗口的 `WM_POINTER` 直通（windui 补丁 093），笔尖接触的**当帧**就到；
/// 而系统"指针→传统鼠标消息"翻译层补发的那份要晚 ~0.5s 才被 `WH_MOUSE_LL` 看到
/// （Windows Ink 已知行为：起笔段被手势消歧缓冲，见 docs/踩坑记录）。两份都喂绘制
/// 就会把一笔画两遍、还倒着画 —— 所以笔驱动绘制期间，钩子侧把补发的鼠标消息整段丢弃。
///
/// - `PEN_ACTIVE`：本进程真的在用笔驱动绘制（覆盖层按下置位 / 抬起清位）。
/// - `PEN_LAST_MS`：最后一次笔事件的 `win_util::tick_ms()` —— 笔的 Up 万一丢失
///   （切屏、消息异常），钩子靠 1.5s 新鲜度自动恢复放行，**绝不会**把鼠标永久吞掉。
///
/// 读端在钩子线程（`platform::input::mouse`），写端在 UI 线程（`runtime::overlay`）。
pub static PEN_ACTIVE: AtomicBool = AtomicBool::new(false);
pub static PEN_LAST_MS: AtomicU64 = AtomicU64::new(0);

/// 应用运行时。持有所有模块状态。
/// 上一呈现帧聚光灯的关键信息（增量帧判定缓存，**补丁 032**）。
///
/// 只要暗罩色不变，「洞挪了个位置」就等价于"旧洞补回暗罩、新洞挖掉"——这就是
/// 缓存里只需要色 + 圆心 + 半径的原因。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpotlightKey {
    color: Argb,
    cx: f32,
    cy: f32,
    r: f32,
}

/// 应用运行时。持有所有模块状态。
pub struct Runtime {
    pub config: Config,
    pub trail: TrailModule,
    /// 击键回显模块（面板关闭时也照常工作）。
    pub key: KeyDisplayModule,
    /// 虚拟键盘模块（按下键位高亮）。
    pub vk: VirtualKeyboardModule,
    /// 击键音效模块（按键时按 Lua 脚本发声；面板关闭时也照常工作）。
    pub key_sound: KeySoundModule,
    /// 屏幕书写模块。
    pub drawing: DrawingModule,
    /// 画中画放大（独立原生模块）。
    ///
    /// ★ **按需驻留**（2026-09-26 用户口径："卡片没勾选就不为它付出内存"）：`None` =
    ///   模块整个不存在（结构体 + 抓屏缓冲 + 原生窗都不占）。生命周期由
    ///   `frame.rs::sync_magnifier_lifecycle` 对账：勾选（载入）→ 下次渲染构造；
    ///   取消勾选 / 被全屏顶掉 → `take()` 释放（`PipWindow` 的 Drop 会 DestroyWindow）。
    pub pip_magnifier: Option<PipMagnifier>,
    /// 截图/GIF 区域选择时的**鼠标放大镜**（复用画中画那套独立分层窗，源是冻结帧）。
    ///
    /// 挂在 `Runtime` 而不是区域选择窗的控件里：它在屏上是一块**独立置顶窗**，不会随
    /// 那个窗一起消失，所以"什么时候收起来"必须是显式的——收口在
    /// [`Self::confirm_snap`] / [`Self::cancel_snap`] 两个出口（区域窗的三条退路都汇到
    /// 这两个函数之一）。控件被 windui 回收的时机不由我们决定，靠 `Drop` 收窗迟早漏。
    pub region_magnifier: RegionMagnifier,
    /// 全屏放大（与画中画同一套底层，面板 = 整个虚拟屏；两者互斥，全屏优先）。
    /// 按需驻留语义同 [`Self::pip_magnifier`]。
    pub fullscreen_magnifier: Option<FullscreenMagnifier>,
    /// 便签模块（原生自绘顶层窗）。
    pub sticky: StickyNoteModule,
    /// 剪贴板历史模块（原生 Win32 监听窗 + 面板窗 + 预览窗，见 `platform::clipboard_history`）。
    pub clipboard: ClipboardModule,
    pub clock: FrameClock,
    pub draw_list: DrawList,
    pub screen: ScreenRect,
    pub config_version: u64,
    /// 本帧真实光标位置（物理像素）。`frame()` 每帧用 `cursor_pos()` 刷新，喂给"光标跟随"
    /// 特效（悬浮标中心 / 拖尾头），消除"画在事件过期坐标上"的滞后。
    pub live_cursor: (f32, f32),
    /// 上一**呈现**帧时的光标位置。光标跟随特效的续帧门禁拿它与 `live_cursor` 比：移动了
    /// 才续帧（windui 动画分支把提交限到刷新率），静止即休眠（0% CPU）。用 `Cell` 是因为
    /// `frame_demand` 是 `&self`，却要在"决定续帧"的那一刻消费掉这次移动。
    last_presented: Cell<(f32, f32)>,
    /// ★ 聚光灯增量帧缓存（**补丁 032**）：上一**呈现**帧的暗罩信息。
    ///
    /// 聚光灯的画面每帧几乎全没变——整片压暗是恒定色，动的只有那个洞。检测到本帧
    /// 指令恰是「整屏实色矩形 + 圆形 Clear 挖洞」且上一帧同色时，这一帧就只重画
    /// 「旧洞 ∪ 新洞」的小块（见 [`Self::detect_spotlight_partial`]），呈现也只交
    /// 这一块。`None` = 上一帧不是聚光灯形态（或被作废），下一帧整窗重画。
    spot_prev: Option<SpotlightKey>,
    /// 本帧是否走增量路径、重画哪一块（世界坐标，`render()` 填、overlay 消费）。
    partial_dirty: Option<Rect>,
    /// 等待逐条处理的输入事件（击键回显 / 屏幕书写消费）。
    ///
    /// 拖尾走自己的采样队列（[`TrailModule::push_sample`]），其余模块在这里逐条消费——
    /// 一帧只喂最后一条事件的话，快速连按会丢按键。
    input_queue: Vec<InputEvent>,
    /// 虚拟键盘上被鼠标按住的键（见 [`Runtime::vk_pointer_begin`]）。
    ///
    /// 记"按下的那一颗"而不是抬起时再判一次命中：按在键帽上、拖到键盘外再松开，
    /// 那个抬起仍然属于键盘（否则下层会收到一个没有按下的抬起）。
    vk_pointer: Option<KeyCode>,
    /// 打开中的控制面板模型；`None` = 面板没开。
    panel: Option<PanelModel>,
    /// 标注文字输入窗是否开着。
    ///
    /// 与 `panel.is_some()` 并列而不是合并成一个 `ui_open`：两者来源不同（一个是模型、
    /// 一个是窗口生命周期回调），分开存才能在各自的读写点上保持清晰。判据由
    /// [`Runtime::input_suspended`] 统一合成。
    text_window_open: bool,
    /// 上一帧标注是否处于激活态（用于检测「进入标注模式」的边沿）。
    drawing_was_active: bool,
    /// 进入标注模式的边沿标志：请覆盖层收起控制面板（见 [`Self::take_panel_close_hint`]）。
    panel_close_hint: bool,
    /// ★ 截图与贴图状态机（区域选择 / 贴图窗 / 剪贴板历史环）。三个快捷键共用一条底层链：
    /// 冻结帧（GDI 同一函数）→ 区域选择 → 剪贴板 / 贴图。
    pub snap: snap::SnapMgr,
    /// 上次见过的剪贴板序列号（`GetClipboardSequenceNumber`）。0 = 还没对过账。
    clip_seq: u32,
    /// 上次**我们自己**写剪贴板的序列号（把截图放进剪贴板那一刻的值）。
    ///
    /// 用途只有一个但很关键：轮询 / 贴图快捷键读到这个序列号时，说明剪贴板里就是"我们刚截的
    /// 那张"，它**已经在环里且带着原位**（见 `SnapMgr::note_shot`）——再当"外部复制"读一遍
    /// 会丢掉原位，贴图就跑到屏幕中间（用户口径 2 报的 bug）。
    own_clip_seq: Option<u32>,
    /// 面板改动尚未落盘。
    panel_dirty: bool,
    /// 上次落盘的时刻（`tick_ms`）。
    panel_last_save_ms: u64,
    /// ★ 配置被面板改动过、请覆盖层补一帧（由 250ms 定时器消费，见
    /// [`Self::take_overlay_repaint_hint`]）。
    ///
    /// 没有它的症状就是"悬浮标的参数改了却看不见"：改完那一刻光标多半停在面板上没动，
    /// 帧门（`frame_demand`）判"光标没移动"返回 `Idle`，而分层窗帧间**不清缓冲**——
    /// 屏上留的是改动前那一帧，直到用户把鼠标移回桌面才突然变过来。用户看到的就是
    /// "有时起作用，有时不起作用"（取决于改完有没有顺手动一下鼠标）。
    overlay_repaint_hint: bool,
    /// 自更新状态（面板可见的信号 + 流程机器状态）。
    pub update: UpdateState,
    /// 在线脚本库状态（m36）：同样常驻——下载/安装可能在面板关着的时候完成。
    pub store: crate::ui::store_state::StoreState,
    /// ★ 面板「工作文件夹」卡：`<exe>/temp` 的占用（后台统计，结果经 [`UiMsg::TempUsage`] 回来）。
    ///
    /// 与 [`Self::update`] 同一条理由常驻在 `Runtime` 而不是随面板新建：统计可能在面板
    /// **已经关掉**之后才回来，状态跟着面板走的话那条消息就没处落。面板每次打开时由
    /// `ui::pages::global` 重新发起一次统计，因此读到的永远是新值。
    ///
    /// [`UiMsg::TempUsage`]: crate::core::events::UiMsg::TempUsage
    pub temp_usage: TempUsageState,
    /// 送往 UI 线程的通道：后台线程（输入桥接、更新线程）**只能**经它把意图交回来。
    ///
    /// 覆盖层建 App 时注入（见 `runtime::overlay::run_overlay`）。`None` 表示还没装配好
    /// ——单测里裸建 `Runtime` 就是这种情况，此时更新流程会给出可读错误而不是 panic。
    pub ui_tx: Option<Sender<UiMsg>>,
    /// 进行中的录屏（录制快捷键 → 3 秒倒计时 → 真正开始；停止快捷键结束）。
    ///
    /// 故意放 `Option<Recorder>` 而不是「运行时全局单例」：录屏生命周期由
    /// `start_recording_now` / `stop_recording_now` 显式管理（见下）。
    recorder: Option<Recorder>,
    /// 上一次录制**正在收尾**（`stop_recording_now` 已受理，结论还没回来）。
    ///
    /// ★★ 为什么必须有它（2026-10-02 现场）：录制线程一旦楔在 `AcquireNextFrame`
    ///   这类调用里，`Recorder` 已被 `stop_recording_now` 取走（`recorder` 变 `None`），
    ///   但线程还活着、文件还没收尾。此时按快捷键会被判成"没在录"→ **又开一段新录制**，
    ///   而新那段因为上一段的桌面复制对象还占着，直接
    ///   `建立桌面复制失败（重试 10 次）: 参数错误`。界面则一直挂着"在录"。
    ///   收尾结论回到 [`Runtime::consume_finished_recordings`] 时才置回 `false`。
    rec_finishing: bool,
    /// ★ 上一次录制的线程**楔住了**（收尾限时内没退出，见 `FinishOutcome::wedged`）。
    ///
    /// 与 [`Self::rec_finishing`] 的区别：`rec_finishing` 只是"还在收尾"（正常情况
    /// 亚秒级就回来）；**这个标记意味着录制线程没死、桌面复制仍被它占着**——在进程
    /// 结束之前，新的一段连 `DuplicateOutput` 都建不起来。此时按快捷键必须如实告知
    /// "需重启软件才能继续录"，而不是让用户得到一次没有解释的失败（2026-10-06，
    /// 对应"软件绝不因录屏卡死"原则：软件活着，但录制能力要诚实标注为不可用）。
    rec_wedged: bool,
    /// ★ 看门狗用的"写入帧数"里程表（采集/编码解耦后必需）。
    ///
    /// 采集/编码解耦（2026-10-06）之后，**采集侧照常出帧 ≠ 文件在增长**：编码器楔住时
    /// 采集线程还在按节奏跑（这正是解耦要达到的效果），于是只看采集心跳的看门狗
    /// 永远发现不了"编码卡住"。所以这里另记一条"写进文件的帧数"里程表：
    /// 出过帧之后若长时间不再推进，就判定编码器卡了（判据在 `frame.rs`）。
    rec_written_seen: u64,
    /// [`Self::rec_written_seen`] 上一次推进的时刻（`None` = 还没出过帧）。
    rec_written_at: Option<std::time::Instant>,
    /// ★ 「上一段收尾完成后接着开录某种格式」（用户 2026-10-03 口径：录着 MP4 时按 GIF
    /// 键就该录 GIF，而不是"把 MP4 停了、什么也不开始"）。
    ///
    /// 为什么不能停了直接开：桌面复制**同一时刻只容一份**，上一段的复制对象要等收尾
    ///   线程真正跑完才释放（`finish_detached`）。不等就撞 `0x80070057`。
    /// DXGI 也不容**同时**录两路，所以诚实的语义只能是"换格式"：先停上一段，收尾完成后
    ///   自动进新格式的流程。用户只按一次键。
    ///
    /// `Some(kind)` = 收尾完成后走该格式的正常开录路径。`None` = 没有待办。
    /// ★ 它也参与帧门（见 [`Runtime::input_needs_frame`]）：否则浮字淡出后覆盖层 park，
    ///   `consume_finished_recordings` 不再跑，这次"接着开录"就永远等不到。
    rec_after_finish: Option<crate::platform::recorder::RecKind>,
    /// [`Self::rec_after_finish`] 那次要录的**选区**（仅选区录屏用；`None` = 整屏）。
    rec_after_finish_crop: Option<crate::core::geometry::ScreenRect>,
    /// 录制开始前的倒计时剩余毫秒（出厂 3 秒，见 [`REC_COUNTDOWN_MS`]）。
    /// `None` = 不在倒计时。倒计时期间按停止快捷键 → 取消（不开始录制）。
    rec_countdown_ms: Option<f32>,
    /// ★ 倒计时归零后要录的东西（MP4 还是 GIF）。
    ///
    /// 倒计时是一条共用通道（录屏快捷键 / GIF 快捷键都进它），所以"录什么"必须随倒计时
    /// 一起传下去，不能等归零时再去读配置——那样两个快捷键就分不开了。
    /// `None` = 不在倒计时（与 `rec_countdown_ms` 同生共死）。
    rec_pending: Option<crate::platform::recorder::RecKind>,
    /// ★ 倒计时归零后要录的**区域裁剪矩形**（仅选区录屏用；虚拟桌面物理像素）。
    ///
    /// 与 `gif_sel` 不同：选区录屏确认后立刻进倒计时，这张矩形只活"选框确认 → 真正开录"
    /// 这一段，开录即被 `start_recording_now` 消费并清空 —— 不残留，否则下一次普通录屏
    /// 会误裁到这块旧区域。普通录屏走 `None`。
    pending_crop: Option<crate::core::geometry::ScreenRect>,
    /// ★ GIF 选框（**虚拟桌面物理像素**）。`None` = 还没框 / 已用完。
    ///
    /// 与 MP4 的区别就在这里：GIF 先让用户框一块，框好了**不立刻录**——边框 + 按钮留在
    /// 屏幕上，等用户点「开始」才进倒计时。所以这个矩形要在"确认选框"和"真正开录"之间
    /// 跨一段时间存活，而 `rec_pending` 只管"录哪种格式"。
    ///
    /// 交给录制器时被 `take()` 走（一次录制只消费一次，见 `start_recording_now`）。
    gif_sel: Option<crate::core::geometry::ScreenRect>,
    /// ★★ **截图编辑会话**（`Ctrl` + 截图快捷键 → 框选完成后进入，见 [`ShotEdit`]）。
    ///
    /// 为什么不能复用一个 `Option<ScreenRect>` 之类的轻量字段：编辑态要跨好几秒存活，
    /// 而这段时间里它必须同时握住**三样东西**——选区（用户还在拖）、那张冻结帧（既是画布
    /// 也是最终裁剪的源头）、以及"确认后要不要贴图"。三样少一个都走不完这条路。
    ///
    /// ★ 与 `gif_sel` 是**两条独立的命**：`gif_sel` 挂着会弹控件条、会让覆盖层进
    /// `MouseMode::GifRegion`、框内要**穿透**给被录应用；编辑态的框内恰恰**不能**穿透
    /// （用户要在里面画）。所以两者共用的只是那套**纯几何**（`core::gif_zone` 的命中 +
    /// `clamp_*` 的缩放钳制），不是状态。
    edit: Option<ShotEdit>,
    /// ★★ **长截图会话**（长截图快捷键 → 框选确认 → 采集线程，见 [`scroll::ScrollSession`]）。
    ///
    /// `None` = 没有会话。生命周期：`arm_scroll`（选区确认）创建，`finish_scroll` /
    /// `cancel_scroll`（快捷键 / Esc / 自动收工兜底）消费。与 `recorder` 同一条纪律：
    /// 采集线程自己在跑，这里只握柄，不参与每一帧。
    scroll: Option<scroll::ScrollSession>,
    /// ★★ 长截图**就绪态**（两段式，与 GIF 同一条流）：框选确认后先停在这里 ——
    /// 边框（蓝）+ 控件条（绿三角）挂出来，等用户点「开始」或再按一次快捷键才真正
    /// 起采集线程（`start_scroll` 把它搬进 `scroll`）。用户口径 2026-10-01：
    /// "选框结束后都不知道怎么开始和结束" —— 起点必须可见、可选。
    ///
    /// `None` = 不在就绪态。生命周期：`arm_scroll` 创建，`start_scroll`（进会话）/
    /// `cancel_scroll`（Esc / ✕ / 快捷键改道）消费。方向（横/纵）在框选那一刻定下，
    /// 与矩形绑在一起走完全程（见 `scroll::ScrollReady`）。
    scroll_ready: Option<scroll::ScrollReady>,
    /// ★ 待存盘的图（「另存为」）：`(帧, 预填文件名)`。
    ///
    /// 为什么要在 `Runtime` 手里停一下：原生保存对话框是**模态 + 自带消息泵**的，只能由
    /// UI 线程在"事件分发完全返回之后"调用（否则重入，见 `file_dialog` 模块头）。而产出
    /// 这张图的地方（`frame()` 里的合成）还借着 `&mut Runtime`。所以流程拆成两截：
    /// 本帧登记 + 发 `UiMsg::SaveAsReady`，UI 线程收到后 `ctx.defer_blocking` 里弹框、落盘。
    ///
    /// 存的是 `Shot`（GDI 帧）而不是 `RgbaBitmap`：落盘走 `platform::screenshot::save_png`，
    /// 它吃的就是 `Shot`，这样中间不再转一次格式。
    pending_save_as: Option<(Shot, String)>,
    /// 录制成功后待展示的输出文件路径（桌面右下角提示窗，见 overlay 的
    /// `take_rec_toast`）。只在开了「录完提示」且收尾成功时登记。
    rec_toast: Option<std::path::PathBuf>,
    /// 最近一次**成功落盘**的 GIF 路径（裁剪入口的输入）。
    ///
    /// 为什么不靠"去输出目录里找最新的 .gif"：那个目录是用户自己选的，可能还放着别人
    /// 拷进来的 gif，按时间戳猜在跨时区/手动改回系统时间时会挑错文件。这里记的是我们
    /// 刚刚亲手写出去的那一个，语义唯一。
    last_gif: Option<std::path::PathBuf>,
    /// 最近一次**成功落盘**的 MP4 路径（时间裁剪入口的输入）。
    ///
    /// 与 `last_gif` 同一条理由：记的是我们刚亲手写出去的那一个，不靠"去目录里找最新
    /// 的 .mp4"猜（目录可能被用户塞进别的东西）。
    last_mp4: Option<std::path::PathBuf>,
    /// 当前选框（`gif_sel`）是给**哪种格式**用的。
    ///
    /// ★ `gif_sel` 这块选框/控件条现在是 **GIF 与选区录屏（MP4）共用**的同一套 UI
    ///   （用户 2026-09-30 口径："复用 gif 的选框，录制按钮成视频录制用的，裁剪按钮成
    ///   视频裁剪用的"）。这一位决定控件条上「开始」上膛成哪种格式、「裁剪」剪刀开
    ///   哪种裁剪窗。
    sel_kind: RecKind,
    /// 开裁剪窗的请求（边沿触发，overlay 取走后开窗）。
    ///
    /// 与 `rec_toast` / `pin_request` 同一套路：`Runtime` 不认识窗口，只登记意图，
    /// 开窗是 overlay 的事（见 `take_gif_trim_request`）。
    ///
    /// ★ 是 `Option<TrimRequest>` 而不是 `bool`：两个入口开出来的窗**初始状态不同**
    ///   （见 [`TrimRequest`]），用一个布尔量表达不了"这次要不要带文件"。
    gif_trim_req: Option<TrimRequest>,
    /// 裁剪窗当前是否开着（由覆盖层在开窗 / 关窗时登记）。
    ///
    /// ★ 为什么需要它：裁剪窗**不在置顶带里**（理由见 `layer::gif_trim` 模块头），
    ///   而覆盖层是全屏最顶层 —— 一旦覆盖层不吃穿透（标注中 / 区域选择中），它就把裁剪窗
    ///   整个盖住，用户点不动。判据与"面板开着"是同一条（面板当初也是这么栽的，见
    ///   [`LayerSession::click_through`](crate::runtime::layer::LayerSession::click_through)）：
    ///   **我们自己开的独立顶层窗口，覆盖层必须给它让路**。
    ///
    /// 与 `gif_trim_req` 的分工：那个是"请开窗"的边沿请求（取走即清），这个是"窗现在在
    /// 不在"的稳态事实。用请求当事实就会出现"窗关了但请求还留着"或反过来。
    gif_trim_open: bool,
    /// 截图 / 录屏的反馈浮字（"截图已保存" / "开始录制" / "录制已停止"）。
    action_notify: Notify,
    /// 麦克风试听（面板上「开始试听」按钮）背后的真实 WASAPI 直通链。`None` = 没在听。
    ///
    /// 这是**行为对账**而不是配置：面板里的按钮只是一枚 `Signal<bool>`，真正"有没有在听"
    /// 由这里的 `Option` 决定（见 [`Self::sync_mic_monitor`]）。
    mic_monitor: Option<crate::platform::audio_capture::MicMonitor>,
    /// 起 [`Self::mic_monitor`] 时用的参数（设备 / 降噪档）。
    ///
    /// 这些参数在 WASAPI `Initialize` 时就定死了、运行中改不了，所以任一项变化都必须
    /// 拆掉重建——留着旧链路只会让"换了麦克风却还是旧设备的声音"。
    mic_monitor_key: Option<(String, String)>,
    /// ★ 上一帧各模块的启用状态，用于识别 **开 → 关** 这条边沿并触发内存回收。
    ///
    /// 为什么需要它：`release_memory` 只在**边沿**上有意义。每帧都调用的话，功能开着时
    /// 刚加载的用户图片会被立刻丢掉（下一次又要从磁盘读），那是把优化做成劣化。
    /// 只有"确实是这一帧关掉的"才回收。
    mru_enabled: ModuleEnabled,
    /// 隐藏系统指针的当前生效状态（边沿检测用，见 [`Self::sync_cursor_hide`]）。
    ///
    /// 任一特效（激光笔类）声明 `hide_cursor` 且处于启用态时藏起系统光标；全部退出后恢复。
    /// 只在"该不该藏"翻转的那条边沿上动一次 `ShowCursor`，保证计数平衡、不连累别的程序。
    cursor_hidden_active: bool,
    /// 计时器是否在跑。快捷键按下即开始、再按停止清零；卡标题栏「启用」开关关闭或
    /// 快捷键为空串时快捷键不响应（两道闸，见 `TimerSettings`）。
    timer_running: bool,
    /// 计时器走过 / 剩余毫秒（countup 累计；countdown 剩余）。
    timer_ms: f64,
    /// 计时器起点（墙钟 `Instant`，与帧率无关，空闲休眠也不丢时间）。
    timer_start: Option<Instant>,
    /// Lua 扩展脚本目录：`assets/lua`（exe 同级）。脚本 / 脚本页都会扫这里。
    lua_ext_dir: std::path::PathBuf,
}

/// 各模块的启用位。用来做**边沿检测**，见 `Runtime::mru_enabled`。
///
/// ★ 为什么录制与截图也在这里：功能开关关掉时**进行中的任务必须停**（用户 2026-10-03
///   口径：「我取消卡片标题栏勾选是成功的，但是没有关闭进行中的计时器，这不是误导
///   用户吗？」）。原先这里只有四个"常驻显示"类模块，于是计时器、录制、截图这些
///   "有进行中状态"的模块关掉开关只改了配置、任务照跑 —— 而它们的快捷键又被自己的
///   开关关掉了，用户连停都停不了。
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct ModuleEnabled {
    trail: bool,
    key: bool,
    vk: bool,
    drawing: bool,
    /// 视频录制开关（面板「录屏」卡的勾选框）。
    rec_mp4: bool,
    /// GIF 录制开关。**与 `rec_mp4` 分开**：两份独立配置各有各的卡，关 MP4 不该把
    /// 正在录的 GIF 停掉，反之亦然。合成一个布尔就分不出"关的是哪一个"。
    rec_gif: bool,
    /// 截图与贴图（框选 / 编辑会话 / 长截图任一在跑）。
    snapping: bool,
    /// 剪贴板历史面板开着。
    clipboard: bool,
    /// 计时器开关（面板「计时器」卡的勾选框）。注意它只表示"**在跑**"——
    /// `timer_running` 是运行态、`config.enabled` 是许可，见下面的注释。
    timer: bool,
}

impl ModuleEnabled {
    /// 从配置读出当前全部开关位。**唯一读口**：初值与每帧边沿比对都走它，
    /// 少一处抄写就少一处漂移（加开关时漏了初值那处，`PartialEq` 恒不等 → 每帧
    /// 都当成"关过"边沿，反复释放内存）。
    fn read(config: &Config) -> Self {
        Self {
            trail: TrailModule::enabled_in(config),
            key: config.key_display.enabled,
            vk: config.virtual_keyboard.enabled,
            drawing: config.drawing.enabled,
            rec_mp4: config.record.enabled,
            rec_gif: config.gif.enabled,
            snapping: config.screenshot.enabled,
            clipboard: config.clipboard.enabled,
            // ★ 只看开关，不看 `timer_running`：`mru_enabled` 记的是**许可**，
            //   运行态由 `prev.timer && !now.timer` 这个边沿去停它。
            timer: config.global.timer.enabled,
        }
    }
}

/// 裁剪窗要处理的是 GIF 还是 MP4。两者共用同一条请求/开窗/让路链路，只是核心与窗口不同。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrimKind {
    /// GIF 裁剪（逐帧重排，见 [`crate::core::gif_trim`]）。
    Gif,
    /// MP4 时间裁剪（区间流拷贝，见 [`crate::platform::mp4_trim`]）。
    Mp4,
}

/// 「该开裁剪窗了」这件事的**两种来意** + **格式**。开出来的窗只有初始状态不同。
///
/// | 来意 | 触发者 | 窗口初始状态 |
/// |---|---|---|
/// | [`TrimRequest::Last(kind)`] | 控件条「裁剪」/ 刚录完 | 直接载入对应格式的 `last_*` |
/// | [`TrimRequest::Blank(kind)`] | 裁剪快捷键 | 空窗，等用户按「打开…」 |
///
/// ★ 为什么不做成"快捷键也去开 `last_*`"：那会让"想裁磁盘上一个老文件"的用户先看到
///   一段无关的录像。裁剪快捷键的语义是**打开裁剪器**，不是"打开上次录的东西"。
/// ★ `Path` 臂带 `PathBuf`，所以只 `Clone` 不 `Copy`（登记/取走都是一次性的，够用）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TrimRequest {
    /// 载入最近一次成功落盘的 GIF / MP4。
    Last(TrimKind),
    /// 空窗（还没选文件）。
    Blank(TrimKind),
    /// 载入**指定**文件（MP4 快捷键"没录过"那条路：先弹「打开」对话框，选中后经
    /// 这条臂走既有开窗链 —— 对话框闭包里拿不到 `EventCtx`，只能先登记再开窗）。
    Path(TrimKind, std::path::PathBuf),
}

impl Runtime {
    pub fn new(config: Config) -> Self {
        let screen = ScreenRect::FALLBACK;
        // 开关边沿检测的初值必须在把 `config` 移动进结构体**之前**算好：
        // 结构体字面量里 `config,` 这一项之后 `config` 就不可用了。
        let initial_enabled = ModuleEnabled::read(&config);
        // 在线脚本库的"更新源"下拉初值 = 配置里存的来源（m40：切换后持久化）。
        let store_source = config.store.source_index();
        // ★ 工作文件夹自动清理（2026-09-28 用户口径）：**每次启动执行一次**，后台线程
        //   进行——递归 stat + 删除几十个录屏文件是几十毫秒到秒级，留在启动路径上就是
        //   "双击图标半天不出窗口"。days=0（永不，出厂默认）时连线程都不开。
        let cleanup_days = config.general.temp_cleanup_days;
        if cleanup_days > 0 {
            let dir = crate::core::paths::temp_dir();
            std::thread::Builder::new()
                .name("temp-cleanup".into())
                .spawn(move || {
                    match crate::core::temp_usage::cleanup_older_than(&dir, cleanup_days) {
                        Ok(r) if r.files > 0 => log::info!(
                            "[自动清理] 删除 {} 个过期文件（{}），清理空目录 {} 个：{:?}",
                            r.files,
                            crate::core::temp_usage::format_bytes(r.bytes),
                            r.dirs,
                            dir
                        ),
                        Ok(_) => log::info!("[自动清理] 没有超过 {cleanup_days} 天的文件：{dir:?}"),
                        Err(e) => log::warn!("[自动清理] 失败：{e}"),
                    }
                })
                .map(|_| ())
                .unwrap_or_else(|e| log::warn!("[自动清理] 拉起线程失败：{e}"));
        }
        // ★ 开关架构（2.0）：没有功能总开关。鼠标特效/击键回显/屏幕书写三个模块各听
        // 自己功能卡片的开关（`trail_module_active` / `key_display.enabled` /
        // `drawing.enabled`），帧门与输入门每帧按配置派生，不再需要归一化。
        let rt = Self {
            key: KeyDisplayModule::new(&config),
            vk: VirtualKeyboardModule::new(&config),
            key_sound: KeySoundModule::new(),
            drawing: DrawingModule::new(&config),
            // 放大镜按需驻留：出厂（未勾选）不构造，见字段注释。
            pip_magnifier: None,
            region_magnifier: RegionMagnifier::new(),
            fullscreen_magnifier: None,
            sticky: StickyNoteModule::new(&config),
            // 剪贴板历史：启动即拉起后台原生线程（监听 + 面板 + 预览），配置经它推送。
            clipboard: ClipboardModule::new(&config),
            // 贴图环容量来自配置（出厂 10，面板可调 5/10/15/20）。
            snap: snap::SnapMgr::with_cap(config.screenshot.pin_ring_cap),
            config,
            trail: TrailModule::new(),
            clock: FrameClock::new(),
            draw_list: DrawList::new(),
            screen,
            // 故意与 TrailModule.last_config_version(0) 错开，保证首帧就同步一次配置。
            config_version: 1,
            live_cursor: (0.0, 0.0),
            last_presented: Cell::new((0.0, 0.0)),
            spot_prev: None,
            partial_dirty: None,
            input_queue: Vec::new(),
            vk_pointer: None,
            panel: None,
            text_window_open: false,
            drawing_was_active: false,
            panel_close_hint: false,
            clip_seq: 0,
            own_clip_seq: None,
            panel_dirty: false,
            panel_last_save_ms: 0,
            overlay_repaint_hint: false,
            update: UpdateState::new(),
            store: crate::ui::store_state::StoreState::new(store_source),
            temp_usage: TempUsageState::new(),
            ui_tx: None,
            recorder: None,
            rec_finishing: false,
            rec_wedged: false,
            rec_written_seen: 0,
            rec_written_at: None,
            rec_after_finish: None,
            rec_after_finish_crop: None,
            rec_countdown_ms: None,
            rec_pending: None,
            pending_crop: None,
            gif_sel: None,
            edit: None,
            scroll: None,
            scroll_ready: None,
            pending_save_as: None,
            rec_toast: None,
            last_gif: None,
            last_mp4: None,
            sel_kind: RecKind::Gif,
            gif_trim_req: None,
            gif_trim_open: false,
            action_notify: Notify::new(),
            mic_monitor: None,
            mic_monitor_key: None,
            mru_enabled: initial_enabled,
            cursor_hidden_active: false,
            timer_running: false,
            timer_ms: 0.0,
            timer_start: None,
            lua_ext_dir: crate::core::paths::lua_ext_dir(),
        };
        // 输入门初值：`sync_ui` 要等到第一次出帧/对账才跑，而桥接线程可能比它更早收到
        // 鼠标事件——先按配置定一次，别让"启动那一瞬"漏掉或错放。
        rt.sync_input_gate();
        rt
    }

    /// 注入 UI 通道（覆盖层建完 App 后调用一次）。
    pub fn set_ui_tx(&mut self, tx: Sender<UiMsg>) {
        self.ui_tx = Some(tx);
    }

    /// 注入虚拟桌面矩形。
    pub fn set_screen(&mut self, screen: ScreenRect) {
        self.screen = screen;
        self.trail.set_screen(screen);
    }

    /// ★★★ 功能开关由「开」变「关」时：**还内存** + **停掉进行中的任务**。
    ///
    /// # 为什么两件事必须放一起判边沿
    ///
    /// 只还内存不终止任务：标注模式退不出（`release_memory` 不碰 `mode`）。
    /// 只终止不还内存：内存不回落，任务管理器里数字照旧，用户以为没生效。
    ///
    /// # 为什么"关开关要停任务"是硬要求（用户 2026-10-03 原话）
    ///
    /// 「我取消卡片标题栏勾选是成功的，但是没有关闭进行中的计时器，这不是误导用户吗？」
    /// —— 卡片上的勾选框是**功能开关**，勾掉就意味着"我不用这个功能了"。只改配置、
    /// 让任务在后台接着跑恰恰是误导：屏幕上还在跳秒数，而用户唯一的停止手段
    /// （快捷键）已经被这个开关自己关掉了 —— 变成谁都停不掉的局面。
    ///
    /// # 为什么只在边沿上做
    ///
    /// 逐帧都做的话，功能开着时刚加载好的用户图片会被立刻丢掉、下一帧又要从磁盘
    /// 重读 —— 那是把优化做成了劣化。
    ///
    /// # 为什么不 hook 到各个配置变更点
    ///
    /// 面板改动是通过 `Signal` diff 回 `Config` 的，变更入口散落几十处；而这里只是
    /// 几个布尔的比对，代价可以忽略，正确性却不依赖"有没有漏某个入口"。
    pub(crate) fn reclaim_disabled_modules(&mut self) {
        let now = ModuleEnabled::read(&self.config);
        let prev = self.mru_enabled;
        if now == prev {
            return;
        }
        self.mru_enabled = now;
        // 有模块在这次边沿上被**关闭**。
        let mut closed = false;

        // ── 常驻显示类模块：还内存 ──
        if prev.trail && !now.trail {
            self.trail.release_memory();
            log::info!("拖尾已关闭：释放特效/波纹/悬浮标资源");
            closed = true;
        }
        if prev.key && !now.key {
            self.key.release_memory();
            log::info!("击键回显已关闭：释放显示缓冲");
            closed = true;
        }
        if prev.vk && !now.vk {
            self.vk.release_memory();
            log::info!("虚拟键盘已关闭：释放显示缓冲");
            // ★ 兜底：按住虚拟键期间把功能关掉 —— 那一刻注入的按键还停在"按下"态，
            //   而鼠标抬起事件再也不会被路由回来（`vk_pointer_begin` 已随开关失效）。
            //   不补这一下，系统层面就留下一颗**卡住的 Ctrl**，直到用户亲手再按一次。
            self.vk_release_all();
            closed = true;
        }
        if prev.drawing && !now.drawing {
            // 最占地方的一块：撤销栈 + 整张虚拟桌面的冻结帧（1080p ≈ 8 MB）。
            self.drawing.release_memory();
            // ★★ 还要**退出标注模式**：只还内存的话笔迹没了、模式还在，用户看着是
            //   "关掉了但屏幕上的选框/工具条还在"，而且再也画不下去（没有笔可用）。
            //   用 `stop_quiet`（不带"已退出绘图"浮字）：那条浮字在这里是多余的 ——
            //   用户是自己关的开关，不是被 Esc 赶出来的。
            self.drawing.stop_quiet();
            log::info!("屏幕书写已关闭：退出标注并释放撤销栈与冻结帧");
            closed = true;
        }

        // ── 有"进行中任务"的模块：停任务（这是本轮新增的主体）──
        //
        // 计时器：关掉开关后快捷键不再响应，不在这里停就是"谁都按不掉"。
        if prev.timer && !now.timer && self.timer_running {
            log::info!("计时器已关闭：停止正在计时的计时器");
            self.stop_timer();
            closed = true;
        }
        // 录屏 / GIF：**分开关判**——关 MP4 不停 GIF，反之亦然（两份独立配置）。
        if self.recorder.is_some() {
            let running = self.recorder.as_ref().map(|r| r.kind());
            let off = match running {
                Some(RecKind::Mp4) => prev.rec_mp4 && !now.rec_mp4,
                Some(RecKind::Gif) => prev.rec_gif && !now.rec_gif,
                None => false,
            };
            if off {
                log::info!("录制开关已关闭：结束正在进行的 {running:?} 录制");
                self.action_notify.show("正在结束录制…");
                self.stop_recording_now();
                closed = true;
            }
        }
        // 截图：框选 / 编辑 / 长截图三态任一在跑都收掉。**贴图窗不受影响** ——
        //   它是钉在桌面上的独立窗口（用户可以钉着图继续用别的功能）。
        if prev.snapping
            && !now.snapping
            && (self.edit.is_some() || self.scroll.is_some() || self.scroll_ready.is_some())
        {
            log::info!("截图开关已关闭：取消正在进行的截图 / 编辑会话");
            self.cancel_capture_sessions();
            closed = true;
        }
        // 剪贴板历史面板：收起来。用 `hide_panel`（无条件收起）而不是 `toggle_panel`
        //   —— 面板本来就收着时 toggle 会把它**打开**（用户报"关掉开关面板自己冒出来"）。
        if prev.clipboard && !now.clipboard {
            log::info!("剪贴板开关已关闭：收起历史面板");
            crate::platform::clipboard_history::hide_panel();
            closed = true;
        }

        // ★★ 释放 ≠ 回落：还掉的页仍留在工作集里，任务管理器数字不回落。
        //   只在**进入**休眠时裁（唤醒后裁 = 缺页风暴，见 `working_set` 头注释）。
        //   请求内部会合并，多个边沿同时发生也只裁一次。开边沿没有内存可还，不请求。
        if closed {
            crate::platform::working_set::request_trim_soon();
        }
        // ★ 另一个入口：勾上「鼠标穿透」（或关掉键盘）时，正在按住的键同样再也等不到
        //   抬起。判据比上面宽 —— 只要"可点击"这个前提没了就释放。
        if self.vk_pointer.is_some() && self.config.virtual_keyboard.click_through {
            log::info!("虚拟键盘转为穿透：释放按住的键");
            self.vk_release_all();
        }
    }

    /// 隐藏系统指针的边沿同步：任一特效（激光笔类）声明 `hide_cursor` 且处于启用态时藏起
    /// 系统光标，全部退出后恢复。`ShowCursor` 是全局计数器，故只在"该不该藏"翻转的那条边
    /// 沿上动一次 `FALSE` / `TRUE`，保证计数平衡、不会连累别的程序。
    pub(crate) fn sync_cursor_hide(&mut self) {
        let want = self.trail.wants_cursor_hidden();
        if want == self.cursor_hidden_active {
            return;
        }
        if want {
            crate::platform::hide_cursor::hide();
        } else {
            crate::platform::hide_cursor::show();
        }
        self.cursor_hidden_active = want;
    }

    /// 把当前被鼠标按住的虚拟键抬起来（幂等）。
    ///
    /// 只在**确实有键按着**时才动：`vk_pointer` 为 `None` 时连 `SendInput` 都不发，
    /// 免得平白往系统输入队列里塞一次抬起。
    ///
    /// `pub` 是给 `overlay::run_overlay` 的收尾用的：进程退出时若还按着一颗键，
    /// 注入的"按下"会留在系统键盘状态里（下次物理按下并松开才复位）——**卡住的 Ctrl**
    /// 比"少注入一次抬起"严重得多。`SendInput` 的按键是系统级状态，与我们的进程生死无关。
    pub fn vk_release_all(&mut self) {
        if let Some(code) = self.vk_pointer.take() {
            crate::platform::input::inject::send_key(code, false);
            log::info!("释放虚拟键盘上按住的键 {code:?}");
        }
    }

    /// Lua 扩展脚本目录（`assets/lua`，exe 同级）。
    pub fn lua_ext_dir(&self) -> &std::path::Path {
        &self.lua_ext_dir
    }

    /// 扫 `assets/lua`，返回声明了 `menu` 的脚本 `(id, 菜单文本)`，字典序稳定。
    ///
    /// 每次调用都重扫：脚本是用户直接粘进目录的，运行期新增不应要求重启程序。
    ///
    /// ★ 2026-09-27 用户口径：`scripts/` 下每个脚本都要有开关，所以**这里按开关过滤**——
    /// 声明了 `menu` 且 id 落在 `config.lua_ext.image_active` 里才出现。
    ///
    /// ★ 2026-10-28 第二道同点过滤：脚本声明了 `requires`（外置文件）而文件不在 ⇒ **不出现**。
    /// 判据只有 [`ExtSchema::available`] 一个（路径解析复用 `fs_api::resolve`，与 `ctx.fs`、
    /// `ext:proc` 同一把闸）。这里不新增机制：可见性判据必须继续留在**这一处**，
    /// 因为 `docs/UI_COMPONENTS.md` 与发布检查单都把这条写成"唯一过滤点"。
    ///
    /// ★ 热键那条**不在这里挡**（有意）：[`Self::script_hotkeys_with_state`] 是从
    /// `config.lua_ext.params` 派生的，让它依赖磁盘就意味着每次登记热键都要扫一遍盘，
    /// 而休眠唤醒那段共用同一份名单（见该函数的注释）。缺文件时按热键会**如实弹一句**
    /// "把工具放进 ocr\\ 才能识别"——脚本自己算得出的事，不必让宿主去猜。
    ///
    /// 变了什么：此前（2026-09-26）定的是"菜单可见性只看磁盘扫描、不受任何开关影响"，
    /// 理由是"所有脚本出厂全关但菜单要列全"。那个两难在有了**每脚本独立开关**之后
    /// 不存在了：勾选 = 菜单里出现这一条，不勾 = 收起。于是"手动动作 ≠ 自动运行"这条
    /// 铁律仍然成立（开关不喂给常驻实例，只管入口存在性），而用户不必再为了用一次
    /// 图片信息跑去改配置。
    ///
    /// 调用方（pin.rs）据此恒挂载「脚本」子菜单，空表时挂空壳 + 灰色占位项。
    pub fn lua_ext_menu_items(&self) -> Vec<(String, String)> {
        let mut reg = crate::scripting::registry_ext::LuaExtRegistry::new(&self.lua_ext_dir);
        reg.scan();
        reg.menu_items()
            .into_iter()
            // 开关是唯一判据，且判定函数只有一个（config::LuaExtSettings::role_is_active）。
            .filter(|s| self.config.lua_ext.role_is_active("image", &s.id))
            // 外置文件没到位的项不出现（`requires`，m13）。
            .filter(|s| s.available())
            .map(|s| (s.id.clone(), s.menu.clone().unwrap_or_else(|| s.id.clone())))
            .collect()
    }

    /// 屏幕中央的一句短反馈，时长与淡出跟录制/GIF 提示同一套（`Notify::DURATION` = 2 s）。
    ///
    /// 用途：`ext_runner` 说「正在运行：X」/「X 还在跑，这次忽略」。它**不新起计时器**：
    /// 显示期间由 [`Self::input_needs_frame`]（`action_notify.is_showing()`）要求出帧、
    /// 由 `frame_delay` 的 `remain_ms` 定下一次唤醒——与既有浮字是同一条路，所以
    /// "多一句提示"不会变成常驻出帧。
    pub fn action_notice(&mut self, text: &str) {
        self.action_notify.show(text);
    }

    /// `config.lua_ext.params[id]` 的每脚本参数；没跑过 → 空表。
    pub fn lua_ext_params(&self, id: &str) -> std::collections::HashMap<String, serde_json::Value> {
        self.config
            .lua_ext
            .params
            .get(id)
            .cloned()
            .unwrap_or_default()
    }

    /// ★ 脚本热键清单 `(热键串, 脚本 id, 脚本开关是否打开)`。
    ///
    /// 从 `config.lua_ext.params` **派生**，不另设配置项：脚本卡里改参数即改热键，
    ///   与 ai-url / ai-model 等其它设置同一条持久化链路。只认 `scripts/` 前缀——
    ///   特效 / 贴图脚本万一也写了 `hotkey` 参数，不该被误当全局热键。
    ///
    /// 第三元 = [`LuaExtSettings::script_active`] 的结果：让调用方在**同一份**清单里
    /// 同时看到"有没有配热键"与"开关开没开"（两条数据分两次询会滑动）。
    pub fn script_hotkeys_with_state(&self) -> Vec<(String, String, bool)> {
        self.config
            .lua_ext
            .params
            .iter()
            .filter(|(id, _)| id.starts_with("scripts/"))
            .filter_map(|(id, m)| {
                let spec = m
                    .get("hotkey")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())?;
                Some((
                    spec.to_string(),
                    id.clone(),
                    self.config.lua_ext.script_active(id),
                ))
            })
            .collect()
    }

    /// **开关已打开**的脚本热键清单 `(热键串, 脚本 id)`。
    ///
    /// ★★ 这是热键派发（[`Self::run_script_hotkey`]）与 park 唤醒登记
    /// （`record::Runtime::action_hotkey_wakes`）**唯一**的名单来源——两处共用一份，
    /// 就不会出现"休眠时这段热键能唤醒、醒来又能跑"与"开关关着却照跑"的错位。
    ///
    /// ★ 2026-09-30 用户报告：脚本卡的开关关着（id 不在任何启用列表里），热键仍能跑脚本。
    ///   根因是旧的名单只读 `params[id].hotkey`、从不看启用列表；现在两张表在这里合流。
    pub fn script_hotkeys(&self) -> Vec<(String, String)> {
        self.script_hotkeys_with_state()
            .into_iter()
            .filter(|(_, _, on)| *on)
            .map(|(spec, id, _)| (spec, id))
            .collect()
    }

    /// 重扫 `assets/lua` 并重建脚本页的卡片表（面板「刷新」按钮，m33）。
    ///
    /// ★ 只换"列表"，**不碰配置、也不碰正在运行的实例**：开关/参数都在 `config` 里，
    ///   新卡片按现值初始化；常驻实例由下一次 `sync_trail_if_stale` 对账（新加的复合脚本
    ///   要用户自己勾开才会跑）。返回重建后的条目数（0 = 面板没开）。
    pub fn refresh_scripts(&mut self) -> usize {
        let Some(model) = &self.panel else {
            return 0;
        };
        let n = model.reload_lua_cards(&self.config, &self.lua_ext_dir);
        log::info!("脚本目录已重扫：{n} 项");
        n
    }

    /// 发起本体更新检查：清单地址取 `config.update`，选中源跟随商店来源。
    ///
    /// 面板按钮只发 `UiMsg::UpdateCheck`，不直接碰网络；这里在 UI 线程取配置，
    /// 再把阻塞请求交给 `update::task` 的后台线程。
    pub fn start_update_check(&mut self) {
        let urls = self.config.update.urls();
        let source = self.config.store.source_index();
        if let Err(e) = crate::update::task::start_check(&self.ui_tx, urls, source) {
            log::warn!("发起检查更新失败：{e}");
        }
    }

    /// 发起一个在线脚本库动作（面板按钮 → `UiMsg::StoreAction` → 这里）。
    ///
    /// ★ 一发起就置灰/改状态行：`Install` 下载期间**没有**"已开始"事件，不在这里置
    ///   `busy` 的话，几 MB 的包下载全程按钮都还是亮的（第二次点只会撞「上一个动作还在进行」）。
    pub fn start_store_action(
        &mut self,
        action: crate::core::events::StoreAction,
    ) -> Result<(), String> {
        use crate::core::events::StoreAction as A;
        // 状态行只有 7 个短词（m46）：检查中… / 下载中… / 已更新 / 已卸载 /
        // 安装失败 / 卸载失败 / 离线。这里只给"进行中"的两个；卸载很快，不单独报。
        let label = match &action {
            A::RefreshCatalog { .. } => "检查中…",
            A::Install { .. } => "下载中…",
            A::Uninstall(_) => "",
        };
        let install_id = match &action {
            A::Install { id, .. } => Some(id.clone()),
            _ => None,
        };
        // 清单地址来自配置（用户可改镜像）；选中来源只决定先试哪一条。
        let urls = self.config.store.urls();
        if let Err(e) = crate::scripting::store_task::start(&self.ui_tx, action, urls) {
            // 起不来（上一个还在跑 / 通道没就绪）：不冒充"失败"，只记日志。
            log::warn!("脚本库动作未发起：{e}");
            return Err(e);
        }
        self.store.busy.set(true);
        self.store.status.set(label.to_string());
        if let Some(id) = install_id {
            // 行尾按钮当场变 0%，不用等第一条 `PackProgress`（RuPix 同一口径）。
            self.store.download.set(Some((id, 0)));
        }
        Ok(())
    }

    /// 动作**没能发起**（上一个还在跑 / UI 通道没就绪）：只更新状态行，不动 `busy`
    /// （"上一个还在跑"时 `busy` 本来就该是 true，清掉会把按钮误放回可点）。
    pub fn store_report_refusal(&mut self, error: String) {
        // 按钮在忙时已经置灰；走到这里多半是极窄的并发窗口或通道未就绪。
        // 不冒充"失败"、不改状态行（7 个短词之外不新增状态）。
        log::warn!("脚本库动作被拒：{error}");
    }

    /// 把后台的进展落到信号上（状态行 / 忙碌 / 行表 / 下载百分比）。
    pub fn store_apply_store_event(&mut self, ev: crate::core::events::StoreEvent) {
        use crate::core::events::StoreEvent as E;
        match ev {
            E::CatalogStarted => {
                self.store.busy.set(true);
                self.store.status.set("检查中…".to_string());
            }
            E::CatalogReady { rows, note } => {
                self.store.busy.set(false);
                self.store.rows.set(rows.clone());
                // 摘要/镜像/跳过条目等细节进日志；状态行回到空（空闲）。
                if !note.is_empty() {
                    log::info!("脚本清单：{note}");
                }
                // 用的是随包旧清单（两个镜像都拉不到）⇒ 7 个状态里的「离线」；
                // 正常成功 ⇒ 普通态「N 个可下载」（换镜像/跳过条目的细节进日志）。
                if note.starts_with("离线") {
                    self.store.status.set("离线".to_string());
                } else {
                    let src = self.store.source.get().min(1);
                    let n = crate::ui::store_state::downloadable_count(&rows, src);
                    self.store.status.set(format!("{n} 个可下载"));
                }
            }
            E::CatalogFailed { error } => {
                self.store.busy.set(false);
                log::warn!("拉取脚本清单失败：{error}");
                self.store.status.set("离线".to_string());
            }
            E::PackProgress {
                id,
                received,
                total,
            } => {
                // 百分比进行尾按钮（RuPix 同一处）；状态行给"在动"的证据。
                let pct = if total > 0 {
                    received
                        .saturating_mul(100)
                        .checked_div(total)
                        .map_or(0, |p| p.min(100) as u8)
                } else {
                    0
                };
                self.store.download.set(Some((id.clone(), pct)));
                self.store.status.set("下载中…".to_string());
            }
            E::Installed {
                id,
                name,
                version,
                added,
                updated,
                removed,
            } => {
                self.store.busy.set(false);
                self.store.download.set(None);
                self.store.status.set("已更新".to_string());
                log::info!(
                    "脚本包 {name} v{version} 已安装：+{added} ~{updated} -{removed}（{id}）"
                );
            }
            E::InstallFailed { id, error } => {
                self.store.busy.set(false);
                self.store.download.set(None);
                log::warn!("安装脚本包 {id} 失败：{error}");
                self.store.status.set("安装失败".to_string());
            }
            E::Uninstalled { id, files } => {
                self.store.busy.set(false);
                log::info!("脚本包 {id} 已卸载：删除 {files} 个文件");
                self.store.status.set("已卸载".to_string());
            }
            E::UninstallFailed { id, error } => {
                self.store.busy.set(false);
                log::warn!("卸载脚本包 {id} 失败：{error}");
                self.store.status.set("卸载失败".to_string());
            }
        }
    }

    /// `assets/` 变化后的**自动**重扫（m35）：由覆盖层的 250 ms 对账调用。
    ///
    /// 监听线程（[`crate::platform::dir_watch`]）只置一个原子标记；真正的重扫在这里（UI 线程）
    /// ——一阵突发（解压一个 20 文件的包）会在 250 ms 的窗口里自然合流成一次。
    /// 面板没开就没有可刷新的对象：直接消费掉标记，别攒到下次开面板再白刷一遍。
    ///
    /// ★ 2026-10-29 用户口径：监听**整个 `assets/`**，不只 `assets/lua`。所以刷新面也
    ///   跟着扩到所有"从资产目录扫出来"的下拉：拖尾 / 悬浮标 / 点击效果 / 声音 /
    ///   键音脚本 / 虚拟键盘方案；脚本卡仍走 [`Self::refresh_scripts`] 的合并规则。
    pub fn refresh_assets_if_changed(&mut self) -> bool {
        if !crate::platform::dir_watch::dirty() {
            return false;
        }
        // ★ 录屏 / 长截图进行中：**延后**，标记留着。
        //   录制本身在三条独立线程上（采集/看门狗/编码），不会被这里的扫描中断；
        //   但一次全树重扫（163 个脚本 ≈ 55 ms）占的是 UI 线程，而"开始/停止录制"与
        //   长截图的滚轮拼接都对 UI 卡顿敏感。所以宁可晚一两秒刷新，也别在这两个时刻抖。
        if self.recorder.is_some() || self.scroll.is_some() {
            return false;
        }
        let _ = crate::platform::dir_watch::take_dirty();
        let Some(model) = self.panel.as_mut() else {
            return false;
        };
        let n = model.reload_lua_cards(&self.config, &self.lua_ext_dir);
        model.reload_asset_lists(&self.config);
        log::info!("assets 目录变化 → 自动刷新脚本 {n} 项 + 资产下拉");
        true
    }

    /// 快捷键触发的脚本运行：后台线程脱手执行（结果自弹，阻塞 HTTP 不碰 UI 线程）。
    ///
    /// ★ 调用方必须先勾过启用开关（名单由 [`Self::script_hotkeys`] 过滤，这里不再重复判）。
    ///
    /// ★★ **必须在 UI 线程调用**：下面用 `pin::current_bitmap` 取"选中的那张贴图此刻的
    ///   像素"，而登记表是 `thread_local` 的——站在别的线程只会查不到、静默退化成"没有图"。
    ///   现有唯一调用点（`runtime::input` 的按键分发）就在 UI 线程。
    pub fn run_script_hotkey(&self, id: &str) {
        log::info!("脚本热键触发：{id}");
        let params = self.lua_ext_params(id);
        // ★★ "按**当前图片**处理"（2026-10-28 用户口径：镜像了，就按镜像后的图处理）。
        //   图片取的是**选中的那张贴图此刻的像素**——旋转 / 镜像 / 上一次脚本回写之后的样子，
        //   不是"钉进来那一刻"的出厂图（`pin_content` 那份只给「恢复原图」与贴图环用）。
        //   没有选中的贴图 ⇒ 传 `None`，脚本会自己回落到剪贴板里的截图（m30）。
        let ctx = self.selected_pin().and_then(|seq| {
            let image = crate::runtime::layer::pin::current_bitmap(seq)?;
            let hwnd = crate::runtime::layer::pin::current_hwnd(seq)? as usize;
            Some(crate::scripting::ext_runner::ExtInput { image, hwnd })
        });
        crate::scripting::ext_runner::run_ext_detached(&self.lua_ext_dir, id, &params, ctx);
    }
}
// ── 依赖倒置：为面板提供宿主实现 ────────────────────────────────────────
//
// 这五个成员就是 `ui` 层能看到的全部运行时能力。面板因此**不认识 Runtime**，
// 分层上 `ui` 不再反向依赖 `runtime`（既有 `Runtime` 方法都已回收到 `impl Runtime` 里）。
impl PanelHost for Runtime {
    fn config_snapshot(&self) -> Config {
        self.config.clone()
    }

    fn open_panel(&mut self, model: PanelModel) {
        self.panel = Some(model);
        // 面板每次打开都是一套全新的信号，其中可能包含与"当前配置"一致的初值；
        // 清掉脏标记，避免刚打开就白写一次盘。
        self.panel_dirty = false;
        self.panel_last_save_ms = 0;
    }

    fn update_state(&self) -> UpdateState {
        self.update.clone()
    }

    fn store_state(&self) -> crate::ui::store_state::StoreState {
        self.store.clone()
    }

    fn temp_usage_state(&self) -> TempUsageState {
        self.temp_usage.clone()
    }

    fn ui_sender(&self) -> Option<Sender<UiMsg>> {
        self.ui_tx.clone()
    }
}

/// 帧需求。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDemand {
    /// 有动画在跑：立刻要下一帧。
    Now,
    /// 画面在 `ms` 毫秒后才会变：平台可以睡到那时（按键弹窗的淡出排程）。
    After(u32),
    /// 彻底静止：不再请求任何帧。
    Idle,
}

/// 进程退出兜底：若仍处于隐藏指针态（异常 / 强制退出路径来不及走边沿还原），强制恢复系统光标。
impl Drop for Runtime {
    fn drop(&mut self) {
        crate::platform::hide_cursor::ensure_shown();
    }
}

// ── GIF 选框平移 / 缩放 clamp（纯函数，独立可单测）───────────────
//
// 四个角锚点（TL=左上、TR=右上、BL=左下、BR=右下）+ 平移（Body）。
// 每次按增量移动对应边，再把矩形整体 clamp 进 `screen`（允许负原点），
// 并用 `saturating_*` 保证最小尺寸 `min`。`screen` 无边界时（退化）由调用方兜底。

#[cfg(test)]
mod tests {

    /// ★ m18：脚本那句「正在运行」只能用**既有的**浮字通道，代价上限 = `Notify::DURATION`。
    ///
    /// 这条同时钉两件事：① 显示期间 `input_needs_frame()` 为真（否则 park 之后没有 paint，
    /// 浮字永远淡不掉——这正是"提示卡在屏幕上"的形状）；② 它**不会**变成常驻需求：
    /// 2 秒之后帧门重新归假，覆盖层可以照常休眠。低占用那把尺子就落在这格上。
    /// ★ m35：目录变化 → 250 ms 对账里**自动**重扫脚本页（不重启、不点按钮）。
    /// 用临时目录而不是出厂资产：判据要证明"刷的是配置里那个目录"。
    #[test]
    fn assets_change_refreshes_the_open_panel_once() {
        let root =
            std::env::temp_dir().join(format!("curseen_auto_refresh_{}", std::process::id()));
        let scripts = root.join("scripts");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            scripts.join("demo.lua"),
            "name = \"演示\"
role = \"image\"
menu = \"演示\"
function run(ctx) return \"ok\" end
",
        )
        .unwrap();

        let mut rt = Runtime::new(crate::core::config::Config::default());
        rt.lua_ext_dir = root.clone();
        rt.panel = Some(crate::ui::model::PanelModel::from_config(&rt.config));

        // ① 有标记 ⇒ 刷一次，且能看到临时目录里的脚本
        let _ = crate::platform::dir_watch::take_dirty();
        crate::platform::dir_watch::mark_dirty();
        assert!(rt.refresh_assets_if_changed(), "标了脏就该刷一次");
        let cards = rt.panel.as_ref().unwrap().lua_ext_cards.get();
        assert!(
            cards.iter().any(|c| c.id == "scripts/demo"),
            "自动刷新后应看到临时目录里的脚本"
        );

        // ② 没有新标记 ⇒ 不刷（返回值是"这一次刷了没"，不是"面板里有几张卡"）
        let _ = crate::platform::dir_watch::take_dirty();
        assert!(!rt.refresh_assets_if_changed());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ★ 2026-10-29 用户口径：监听**整个 `assets/`**，不只 `assets/lua`——往
    /// `MouseTrail/`、`VirtualKeyboards/` 里丢文件也要触发刷新。这里钉住启动时的
    /// 监听根与标签（避免以后又收窄回 `assets/lua`）。
    #[test]
    fn assets_watch_covers_the_whole_assets_tree() {
        let src = include_str!("overlay.rs");
        assert!(
            src.contains("asset_dir(\"\")"),
            "监听根应是 assets/（`asset_dir(\"\")`），不是某个子目录"
        );
        assert!(
            src.contains("watch_dir(&dir, \"assets\")"),
            "监听标签应写明 assets"
        );
        assert!(
            !src.contains("watch_dir(&dir, \"assets/lua\")"),
            "不许再收窄回 assets/lua"
        );
    }

    #[test]
    fn a_script_notice_asks_for_frames_only_while_it_is_visible() {
        let mut rt = Runtime::new(crate::core::config::Config::default());
        assert!(!rt.input_needs_frame(), "什么都没开时覆盖层该能 park");
        rt.action_notice("正在运行：2.ocr_text");
        assert!(rt.input_needs_frame(), "浮字显示期间必须出帧");
        assert_eq!(
            rt.action_notify.remain_ms(),
            Some((crate::modules::drawing::notify::Notify::DURATION * 1000.0) as u32),
            "时长必须与既有的录制提示同一档（2 s），不许为这条另起一个数"
        );
        rt.action_notify
            .tick(crate::modules::drawing::notify::Notify::DURATION + 0.1);
        assert!(!rt.action_notify.is_showing(), "到点必须自己淡掉");
        assert!(
            !rt.input_needs_frame(),
            "淡完之后帧门要归假——这句提示不许常驻"
        );
    }
    use super::*;

    /// ★ 回归：模块由「开」变「关」的那一帧会回收内存，且**只**回收那一帧。
    ///
    /// 覆盖三点事实：① 边沿能被识别到；② 关掉之后再次调用不再重复回收（否则功能开着时
    /// 刚加载的用户图片会被每帧丢弃再重读）；③ 反向（关 → 开）不触发回收。
    #[test]
    fn disabling_a_module_reclaims_on_the_edge_only() {
        let mut cfg = Config::default();
        cfg.drawing.enabled = true;
        cfg.trail.trail_enabled = true;
        let mut rt = Runtime::new(cfg);

        // 初值：两个模块都是开。
        assert!(rt.mru_enabled.drawing, "初值应与配置一致");
        assert!(rt.mru_enabled.trail, "初值应与配置一致");

        // ① 开 → 关：这一帧应当识别到边沿（表现为 mru_enabled 随之更新）。
        rt.config.drawing.enabled = false;
        rt.reclaim_disabled_modules();
        assert!(!rt.mru_enabled.drawing, "关闭后状态必须跟进");

        // ② 已经是关的状态下再跑：不应发生任何变化（幂等）。
        let before = rt.mru_enabled;
        rt.reclaim_disabled_modules();
        assert_eq!(rt.mru_enabled, before, "关闭态下重复调用必须无副作用");

        // ③ 关 → 开：不触发回收，但状态要跟进。
        rt.config.drawing.enabled = true;
        rt.reclaim_disabled_modules();
        assert!(rt.mru_enabled.drawing);

        // 不影响别的模块：拖尾仍开着。
        assert!(rt.mru_enabled.trail, "关书写不该顺带关掉拖尾");
    }

    /// ★★★ 关掉开关必须**停掉进行中的任务**，而不只是改配置（用户 2026-10-03 报）。
    ///
    /// 原话：「我取消卡片标题栏勾选是成功的，但是没有关闭进行中的计时器，这不是误导
    /// 用户吗？」—— 而更糟的是：关掉开关后**快捷键也不再响应**，于是那个任务变成
    /// 谁都停不掉的局面（屏幕上还在跳秒数，用户却没有任何办法让它停下）。
    ///
    /// 钉住计时器这条最典型的：它是唯一一个"运行态与配置开关完全解耦"的模块
    /// （`timer_running` 只有 `toggle_timer` 会写）。
    #[test]
    fn turning_a_switch_off_stops_the_task_that_was_running() {
        let mut cfg = Config::default();
        cfg.global.timer.enabled = true;
        let mut rt = Runtime::new(cfg);

        // 先让计时器跑起来（模拟用户按了快捷键）。
        rt.toggle_timer();
        assert!(rt.timer_running, "计时器应在跑");
        assert!(rt.timer_start.is_some(), "应有计时起点");

        // ★ 用户在面板上取消勾选：只改配置，**不碰** `timer_running`。
        rt.config.global.timer.enabled = false;
        rt.reclaim_disabled_modules();

        assert!(
            !rt.timer_running,
            "关掉计时器开关后，进行中的计时必须被停掉——否则它会一直跳到用户重启程序"
        );
        assert!(
            rt.timer_start.is_none(),
            "停了必须清起点，否则下次开启会从旧起点算"
        );
        assert_eq!(rt.timer_ms, 0.0, "停了必须清零，否则读秒先跳一下");
    }

    /// 关掉开关的**幂等性**：已经在关的状态下反复调用不得反复动手。
    ///
    /// 这条不是可有可无的礼节：`reclaim_disabled_modules` 每帧都跑，不幂等等于
    /// 每帧都去停一次录制 / 取消一次截图（用户根本没法用）。
    #[test]
    fn a_disabled_module_is_only_stopped_once() {
        let mut cfg = Config::default();
        cfg.global.timer.enabled = true;
        let mut rt = Runtime::new(cfg);
        rt.toggle_timer();
        assert!(rt.timer_running);

        rt.config.global.timer.enabled = false;
        rt.reclaim_disabled_modules();
        assert!(!rt.timer_running, "第一次应当停掉");

        // 再跑三次：不该有任何副作用（这里断言的是"没 panic 且状态不变"——
        // 真要抓"重复动手"，得在计时器上做计数器，那是过度设计）。
        for _ in 0..3 {
            rt.reclaim_disabled_modules();
            assert!(!rt.timer_running);
            assert!(rt.timer_start.is_none());
        }
    }

    /// ★ 边沿对账必须挂在**休眠也会跑**的那条路上，且在读帧门之前。
    ///
    /// 判据取自源码而非行为：`reclaim_disabled_modules` 平时挂在 `frame()` 里，而
    /// `frame()` 只在 `paint()` 中被调 —— 覆盖层 park 之后一次都不跑。用户改开关时
    /// 若覆盖层正 park（屏幕上什么都不闪，恰恰是最可能 park 的场景），边沿就检测不到。
    /// 已有同款教训的正确解法可抄：`sync_trail_if_stale` 挂在 250ms 轮询里、
    /// 位置在 `want_park` 判定**之前**。
    #[test]
    fn the_disable_reconcile_runs_while_parked_and_before_the_frame_gate() {
        let overlay = include_str!("overlay.rs");
        // ⚠ 必须锚在**250ms 轮询**那处（`poll_rt` 那一组），不能取 `find` 的首个命中：
        // 文件前部还有键盘钩子那两处 `input_needs_frame()`，与本门禁无关。
        // 判据取 `poll_rt` 这个变量名 —— 它只出现在轮询回调里。
        let poll_gate = overlay
            .find("let want_park = !poll_rt.borrow().input_needs_frame();")
            .expect("应能在轮询回调里找到读帧门");
        let hook = overlay[..poll_gate]
            .rfind("poll_rt.borrow_mut().reclaim_disabled_modules();")
            .expect("休眠兜底轮询里必须调一次 reclaim_disabled_modules");
        assert!(
            hook < poll_gate,
            "对账必须排在 `input_needs_frame()` 之前：它要靠这一帧的读取结果决定能否 park"
        );
        // 且必须与同款兜底钩子（`sync_trail_if_stale`）挨在一起 —— 同一轮询、同一时机。
        let tick_hook = overlay[..poll_gate]
            .rfind("poll_rt.borrow_mut().sync_trail_if_stale();")
            .expect("应能找到同款兜底钩子");
        assert!(
            tick_hook < hook,
            "两处兜底钩子应挨在一起（同一轮询里），别把对账塞到别的时机"
        );
    }

    /// ★ 关掉「录屏」开关不得停掉正在录的 GIF，反之亦然（两份独立配置各有各的卡）。
    ///
    /// 这条挡的是一个具体写错方式：把两个开关合成一个布尔再判边沿。
    /// 那样"关 MP4 开关"会把 GIF 一起停掉，而用户压根没碰 GIF。
    #[test]
    fn the_mp4_switch_never_stops_a_running_gif() {
        let mut cfg = Config::default();
        cfg.record.enabled = true;
        cfg.gif.enabled = true;
        let mut rt = Runtime::new(cfg);
        assert!(rt.mru_enabled.rec_mp4 && rt.mru_enabled.rec_gif);

        // 只关 MP4 开关。
        rt.config.record.enabled = false;
        rt.reclaim_disabled_modules();
        assert!(!rt.mru_enabled.rec_mp4, "MP4 开关状态要跟进");
        assert!(
            rt.mru_enabled.rec_gif,
            "GIF 开关没被碰过，状态不该变（更不该因此停掉正在录的 GIF）"
        );
    }

    /// ★ 回归：`Runtime::new` 的开关边沿初值必须**等于**配置。
    ///
    /// 初值与配置不一致时，首帧会被误判成一次开关变化——功能原本开着却被当成"刚被关闭"，
    /// 于是用户刚加载的悬浮标图片在启动那一刻就被丢掉。
    #[test]
    fn initial_module_flags_follow_the_config() {
        let mut cfg = Config::default();
        cfg.drawing.enabled = true;
        cfg.key_display.enabled = false;
        cfg.virtual_keyboard.enabled = true;
        cfg.trail.trail_enabled = false;
        let rt = Runtime::new(cfg);
        assert!(rt.mru_enabled.drawing);
        assert!(!rt.mru_enabled.key);
        assert!(rt.mru_enabled.vk);
        assert!(!rt.mru_enabled.trail);
    }

    /// ★ 2026-09-27 用户口径回归：贴图右键菜单**归 image 脚本开关管**。
    ///
    /// 历史注记（别删）：2026-09-26 定的是"菜单可见性只看磁盘扫描"，理由是"所有脚本
    /// 出厂全关但菜单要列全"。当天踩的坑是"磁盘上有脚本、菜单却是空的"。次日（本条）
    /// 用户拍板"每个脚本都要有开关"，两难随之消解：勾开 = 菜单里出现，不勾 = 收起。
    /// 手动动作仍然不等于自动运行——开关只管入口存在性，不喂常驻实例。
    #[test]
    fn lua_menu_items_follow_the_image_switch() {
        // 图片体检/OCR 迁到 CurSeen-Scripts 后，内置 `scripts/` 已没有 image 脚本；
        // 这条判据只关心开关闸门，用临时目录造一枚 image 脚本，别拿出厂态碰运气。
        let dir = std::env::temp_dir().join(format!("curseen_menu_gate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("demo_image.lua"),
            "role = \"image\"\nname = \"演示\"\nmenu = \"演示\"\n",
        )
        .unwrap();

        // 独立基准：注册表自己扫出来的、声明了 menu 的脚本（不带任何过滤）。
        let mut reg = crate::scripting::registry_ext::LuaExtRegistry::new(&dir);
        reg.scan();
        let declared: Vec<String> = reg.menu_items().iter().map(|s| s.id.clone()).collect();
        assert!(
            !declared.is_empty(),
            "夹具就该扫到声明了 menu 的脚本——扫不到 = 临时目录没建对，这条会假绿"
        );

        let menu_for = |cfg: Config| {
            let mut rt = Runtime::new(cfg);
            rt.lua_ext_dir = dir.clone();
            rt.lua_ext_menu_items()
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>()
        };

        // ① 出厂态（两个列表都空 = 全部脚本关闭）：菜单**一项都不得出现**。
        let cfg = Config::default();
        assert!(
            cfg.lua_ext.composite_active.is_empty() && cfg.lua_ext.image_active.is_empty(),
            "出厂默认必须全关，否则下面两条会假绿"
        );
        let got = menu_for(cfg.clone());
        assert!(
            got.is_empty(),
            "脚本出厂全关时「脚本」应为空壳（占位项由调用方挂），got {got:?}"
        );

        // ② 勾开一个：菜单里**只有**它，且不要求重启（菜单每次调用都重扫）。
        let mut cfg2 = cfg.clone();
        cfg2.lua_ext.set_role_active("image", &declared[0], true);
        let got2 = menu_for(cfg2);
        assert_eq!(
            got2,
            vec![declared[0].clone()],
            "只勾开的那个该出现，got {got2:?}"
        );

        // ③ 勾开的复合脚本**不进**手动菜单（手动动作 ≠ 常驻运行，不许共用一张闸）。
        let mut cfg3 = cfg.clone();
        cfg3.lua_ext
            .set_role_active("composite", &declared[0], true);
        let got3 = menu_for(cfg3);
        assert!(
            got3.is_empty(),
            "复合脚本的运行开关管不到手动菜单，got {got3:?}"
        );

        // ④ `requires` 那一格挪成了独立测试 `requires_hides_menu_entries_for_missing_files`：
        // 它需要自己的脚本目录（确定性夹具），而不是拿出厂脚本碰运气——出厂态变了就会静默跳过，
        // 那正是假绿。本条 ①②③ 只管开关那两道闸。
    }

    /// ★ `requires`（m13）：**勾开了但外置文件不在 ⇒ 不进菜单**；文件补上 ⇒ 不重启就出现。
    ///
    /// 为什么单开一条而不用出厂脚本碰运气：出厂态里"有没有未满足的 requires"是会变的
    /// （谁把 `ocr\` 拷进源目录，那条就静默跳过＝假绿）。这里自带夹具，两档都是必然发生。
    #[test]
    fn requires_hides_menu_entries_for_missing_files() {
        use crate::scripting::registry_ext::LuaExtRegistry;
        let dir =
            std::env::temp_dir().join(format!("curseen_requires_gate_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plain.lua"),
            "name = \"通用\"
role = \"image\"
menu = \"通用动作\"
",
        )
        .unwrap();
        std::fs::write(
            dir.join("gated.lua"),
            "name = \"要外置\"
role = \"image\"
menu = \"跑外置\"
requires = { \"./tools/x.exe\" }
",
        )
        .unwrap();
        // 夹具自己先立起来：两份都该被扫到、都声明了 menu（扫不到就什么都不测）。
        let mut reg = LuaExtRegistry::new(&dir);
        reg.scan();
        let declared = reg.menu_items().len();
        assert_eq!(
            declared, 2,
            "夹具该扫到 2 条带 menu 的脚本，实际 {declared}"
        );
        let gated = reg.get("gated").expect("gated 该扫到");
        assert!(
            !gated.available(),
            "夹具里 ./tools/x.exe 不存在，必须算不满足"
        );

        // 两条都勾开 ⇒ 只有 plain 出现。
        let mut cfg = Config::default();
        cfg.lua_ext.set_role_active("image", "plain", true);
        cfg.lua_ext.set_role_active("image", "gated", true);
        let mut rt = Runtime::new(cfg.clone());
        rt.lua_ext_dir = dir.clone();
        let got: Vec<String> = rt
            .lua_ext_menu_items()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            got,
            vec!["plain".to_string()],
            "缺外置文件的该被挡在菜单外，got {got:?}"
        );

        // 把文件补上 ⇒ **不重启**（同一份 Runtime）下一次调用就该出现。
        std::fs::create_dir_all(dir.join("tools")).unwrap();
        std::fs::write(dir.join("tools").join("x.exe"), b"probe").unwrap();
        let got2: Vec<String> = rt
            .lua_ext_menu_items()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            got2.contains(&"gated".to_string()),
            "外置文件到位后不重启就该出现，got {got2:?}",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// m46：红色状态行只允许 7 个短词（3–4 个汉字）；旧的摘要/包名/URL 长文案不许回潮。
    #[test]
    fn store_status_vocabulary_stays_short() {
        let src = include_str!("mod.rs");
        let cut = src.rfind("\n#[cfg(test)]").unwrap_or(src.len());
        let s = &src[..cut];
        for word in [
            "检查中…",
            "下载中…",
            "已更新",
            "已卸载",
            "安装失败",
            "卸载失败",
            "离线",
        ] {
            // 必须真的写进状态信号（注释里提到不算）：精确到 `status.set("…")`。
            let needle = format!("status.set(\"{word}\".to_string())");
            assert!(s.contains(&needle), "缺少状态写入：{needle}");
        }
        assert!(
            s.contains("format!(\"{n} 个可下载\")"),
            "普通态要真的写 `format!(\"{{n}} 个可下载\")`（注释里提到不算）"
        );
        for gone in [
            "个可升级",
            "正在更新清单",
            "正在下载「",
            "已安装「",
            "已卸载 {",
            "安装 {id} 失败",
            "卸载 {id} 失败",
        ] {
            assert!(!s.contains(gone), "长文案回潮：{gone}");
        }
    }

    /// m69：切源不能只改配置——行尾动作与「N 个可下载」都在建树时按来源定档，
    /// `sync_store_source` 必须在切源当帧重放**已有清单**（不联网），否则灰字来源
    /// 会与按钮状态对不上。
    #[test]
    fn switching_source_replays_rows_and_count_without_network() {
        let src = include_str!("panel.rs");
        let at = src
            .find("fn sync_store_source(")
            .expect("找不到 sync_store_source");
        let rest = &src[at..];
        let end = rest
            .find("\n    pub(crate) fn sync_panel")
            .unwrap_or(rest.len());
        let body = &rest[..end];
        for needle in [
            "downloadable_count(&rows",
            "self.store.status.set(format!(\"{n} 个可下载\"))",
            "self.store.rows.set(rows)",
        ] {
            assert!(body.contains(needle), "切源后没有重放 UI：{needle}");
        }
        // 只读已有行表——这里不许发起清单刷新（切源不是联网动作）。
        assert!(
            !body.contains("start_store_refresh") && !body.contains("RefreshCatalog"),
            "切源不该联网：重放即可"
        );
    }

    /// m72：点「更新」后 UI 必须先切到下载态（按钮→进度条），再等网络。
    #[test]
    fn download_start_event_flips_the_update_row_immediately() {
        let p = include_str!("panel.rs");
        let at = p
            .find("UpdateEvent::DownloadStarted =>")
            .expect("panel 未处理 DownloadStarted");
        let rest = &p[at..];
        let end = rest
            .find("UpdateEvent::DownloadProgress")
            .unwrap_or(rest.len());
        let arm = &rest[..end];
        for needle in [
            "self.update.downloading.set(true)",
            "self.update.progress.set(0.0)",
            "self.update.busy.set(true)",
        ] {
            assert!(arm.contains(needle), "下载开始缺少：{needle}");
        }
    }
}
