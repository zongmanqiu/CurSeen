//! 覆盖层 z 序守护（CurSeen 补丁 014）。
//!
//! # 为什么需要它
//!
//! 建窗时带 `WS_EX_TOPMOST` **只把窗口放进"置顶带"，不保证它在带内靠前**。晚于我们创建
//! （或被抬起）的置顶窗口会插在我们上面，而 `WS_EX_TOPMOST` 是同一条带里的平级关系：
//! 谁后到谁在上，系统从不替我们"守住"位置。
//!
//! 实测（`temp/notes-archive/tools/zorder_probe.py ontop`，历史探针）：在覆盖层上方放一个置顶小窗，覆盖层立刻被挤到
//! 它下面，鼠标拖尾在该区域内**完全看不见**（墨迹 0 字节），而进程 CPU 仍是 84%——也就是
//! "照常在画，只是被盖住"。真实桌面上这类窗口非常多：Quicker 面板、输入法皮肤窗、悬浮
//! 工具条、桌面歌词、截图工具、其它软件的覆盖层。本机启动时排在我们上面的就有 6 个。
//!
//! 所以需要一个**对账**（不是逐个埋点，因为"谁上来压住我们"没有任何通知可接）：每隔
//! 一段时间检查"上方有没有真的盖住我们的置顶窗口"，有就把自己重新插进置顶带。
//!
//! 判据是纯几何/样式查询（枚举 z 序 + 扩展样式 + 矩形），稳定后判据为假就一次 API 都不调。
//! 具体判据与踩过的坑见 [`cover_scan_proc`]。
//!
//! # 实测记下的两条平台语义
//!
//! · `SetWindowPos(HWND_TOPMOST)` 只把窗口送进置顶带（`z_rank` 48 → 6），**反复调用不会
//!   继续爬升**——它插在"所有已存在的置顶窗口之后"。但这已经够用：真实遮挡者（晚于我们
//!   上台的普通置顶窗）也在同一层，我们再插一次就排到它前面。
//! · 本机压在我们上面的 6 个窗口全是 `0×0` / `1×1` 的系统 helper，**根本不构成遮挡**——
//!   判据必须看矩形，否则会为它们空转（见 [`cover_scan_proc`]）。
//!
//! # 主动让位的例外
//!
//! 前台是全屏应用/游戏时**让位**（用户明确要求）。我们的覆盖层是整屏的：不让位就会盖住
//! 游戏画面和全屏视频。判据见 [`yield_reason`]——它对误报非常敏感，同一处坑（自己把自己
//! 判成全屏）记在那里。
//!
//! 弹出菜单（[`visible_menu`]）同样必须让位，理由不同：那是**本模块的副作用**——守护把
//! 覆盖层插回置顶带顶部时，会连系统菜单一起压到下面，菜单随之不响应鼠标。详见该函数的
//! 说明与 2026-09-15 的实测记录。
//!
//! 让位动作是 `HWND_BOTTOM`（置底）而不是 `HWND_NOTOPMOST`：后者按文档只保证"在所有非
//! 置顶窗之上"，仍然会盖住无边框全屏的应用。全屏应用退出后由下一次对账恢复置顶。
//!
//! # 与其它置顶窗口的争抢
//!
//! 若某个窗口在我们抬升之后又立刻压回来（自己会不断置顶的窗口），每 250ms 抢一次会变成
//! z 序抖动。所以同一个"遮挡者"连续抢 [`MAX_STRIKES`] 次后进入退避：改为每
//! [`RETRY_EVERY`] 次对账（≈2s）重试一次，而不是永久放弃。
//!
//! 为什么不能永久放弃：任务管理器默认勾选「窗口置顶」，且每次刷新 UI 都会重新把自己抬
//! 到置顶带顶部。曾实测它在我们两次抢回后又一次压住，守护就此永久退出争抢——覆盖层被
//! 压到任务管理器下面，拖尾"消失"（用户报告：开任务管理器后特效不见了）。2 秒一次的
//! 重试在两次刷新之间足够插回顶部，抖动幅度也小到不可感知。

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetClassNameW, GetDesktopWindow, GetForegroundWindow, GetShellWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindow, IsWindowVisible, IsZoomed, SetWindowPos, GWL_EXSTYLE,
    GWL_STYLE, HWND_BOTTOM, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_CAPTION,
    WS_EX_TOPMOST,
};

/// 覆盖层窗口标题。同时是 z 序守护定位窗口的唯一凭据——两者必须同源。
pub const OVERLAY_TITLE: &str = "CurSeen Overlay";

/// 同一个遮挡者连续快抢几次后转入退避（见模块头"与其它置顶窗口的争抢"）。
const MAX_STRIKES: u32 = 2;

/// 退避期间的重试周期（单位：次对账）。250ms × 8 = 2s。
const RETRY_EVERY: u32 = 8;

/// 覆盖层 z 序守护。
///
/// 由 UI 线程持有并周期调用 [`TopmostKeeper::reconcile`]（与面板对账同一个定时器）。
pub struct TopmostKeeper {
    /// 覆盖层句柄。首次对账时按标题查一次，之后缓存（窗口销毁时才需要重查）。
    hwnd: Option<HWND>,
    /// 是否因为"前台是全屏应用"而主动置底。
    yielded: bool,
    /// 上一个把我们从置顶带顶部挤下来的窗口，以及连续被压的次数。
    last_cover: Option<HWND>,
    strikes: u32,
    /// «全屏避让»：前台是全屏应用/游戏时**让位**（覆盖层置底）。
    ///
    /// ★ 出厂 false（屏幕行为工具默认盖住全屏以显示特效，用户口径 2026-09-20）。勾上
    ///   = 全屏时让位、不压全屏 —— 由全局 `Config::overlay.avoid_fullscreen` 喂入。
    ///
    /// 只门控**全屏**让位；系统弹窗/菜单那条让位不受它影响（那些必须留在覆盖层之上）。
    avoid_fullscreen: bool,
    /// 最近一次为它让位的弹出菜单（见 [`visible_menu`]）。菜单句柄变化才算"新菜单"。
    menu_yield: Option<HWND>,
    /// 是否**主动夺回**置顶带顶部（面板「总在最上」，出厂 true）。
    ///
    /// ★ 这一位在 2026-09-18 之前是**死字段**：配置里有、面板上有勾选框、也能落盘，
    ///   但没有任何消费者——勾不勾都一样，守护恒把覆盖层插回置顶带顶部。这里把它接上，
    ///   语义取"是否与别的置顶窗争抢"：
    ///
    ///   · true（出厂）= 被置顶窗口挡住就抢回顶部。**有它才看得见特效**：模块头记的
    ///     那 6 个常驻置顶 helper、Quicker 面板、任务管理器置顶刷新，都会把我们压住。
    ///   · false = 只"普通置顶"：覆盖层仍在置顶带里（建窗时带 `WS_EX_TOPMOST`），
    ///     但被后来的置顶窗压住时不抢回来。
    ///
    /// **不影响让位逻辑**（全屏避让、系统弹窗）：那两条是"主动退让"，不是"争抢"。
    always_on_top: bool,
    /// 除覆盖层外，还要一起守住的**我们自己的工具窗**（按标题 + **置顶带内 rank**）。
    ///
    /// 目前是 GIF 控件条与 GIF 裁剪窗。为什么拿标题而不是句柄：与 [`TopmostKeeper::resolve`]
    /// 同一个理由 —— 窗口句柄只在 windui 的 UI 回调里可见，存不进 `Runtime`。
    ///
    /// ★ 标题由调用方喂进来，**不是**这里 `use` 过来的：分层铁律是
    ///   `core ← platform ← … ← ui ← runtime`，`platform` 不能反向依赖 `runtime`。
    ///
    /// ★ 这是「轴 2·跨窗优先级」：rank 决定这些工具窗在**置顶带内**的相对高度。
    ///   [`enforce_aux_windows`](Self::enforce_aux_windows) 按 rank 升序抬升——rank 小＝带内
    ///   偏底、先抬；rank 大＝偏向用户、后抬（同一个 `SetWindowPos(HWND_TOPMOST)` 里最后
    ///   抬的那个留在最上面）。默认 `guard_aux` 按喂入顺序赋 `0..n` 的 rank，行为与现在完全
    ///   一致；要改排序就 `set_aux_rank` 调 rank。
    aux: Vec<AuxWindow>,
    /// 临时**不让位**（全屏避让失效）。
    ///
    /// 由 `overlay` 每个对账周期喂一次，判据是"当前有 GIF 选框"。理由：
    ///   · 边框画在覆盖层上，让位就等于用户**看不见自己在录哪一块**；
    ///   · 录制中用户要的就是"特效与边框可见"（拖尾本就该被录进产物）。
    /// 只在选框存在期间生效，选框一收立刻恢复常规让位语义。
    suppress_yield: bool,
}

/// 一个要一起守卫的工具窗：标题 + 在置顶带内的 rank（见 [`TopmostKeeper::aux`]）。
#[derive(Clone, Copy)]
struct AuxWindow {
    title: &'static str,
    rank: i32,
}

impl TopmostKeeper {
    pub fn new() -> Self {
        Self {
            hwnd: None,
            yielded: false,
            last_cover: None,
            strikes: 0,
            menu_yield: None,
            // 与出厂配置 `overlay.always_on_top: true` 同源：对账第一次跑之前的窗口期内，
            // 行为必须与配置声明的默认一致，否则首帧会闪一下顶在最前。
            always_on_top: true,
            // 出厂取消全屏避让（与配置 `overlay.avoid_fullscreen: false` 同源：屏幕工具默认盖全屏）。
            avoid_fullscreen: false,
            // 由 `guard_aux` 喂入；没喂之前不守任何辅助窗（覆盖层照常守）。
            aux: Vec::new(),
            suppress_yield: false,
        }
    }

    /// «全屏避让»（全局 `Config::overlay.avoid_fullscreen`，每个对账周期喂一次；
    /// 见字段 [`Self::avoid_fullscreen`]）。
    ///
    /// 由关变开时不必在这里立刻让位：[`reconcile`](Self::reconcile) 看到 `yielded == false`
    /// 而判据已开，下一周期就会走"让位"那条路。
    pub fn set_avoid_fullscreen(&mut self, on: bool) {
        self.avoid_fullscreen = on;
    }

    /// 登记要一起守住的工具窗标题（见字段 [`Self::aux`]）。幂等，重复喂同一个列表无害。
    ///
    /// 按喂入顺序赋 rank `0..n`：先喂的偏底、后喂的偏上（即"保持现状"的默认排序）。
    /// 要改排序，之后用 [`Self::set_aux_rank`] 调。
    pub fn guard_aux(&mut self, titles: Vec<&'static str>) {
        self.aux = titles
            .into_iter()
            .enumerate()
            .map(|(rank, title)| AuxWindow {
                title,
                rank: rank as i32,
            })
            .collect();
    }

    /// 调整某个工具窗在置顶带内的 rank（「轴 2·跨窗优先级」，见字段 [`Self::aux`]）。
    ///
    /// 标题不在守卫列表里就忽略（不报错）：列表由 [`Self::guard_aux`] 一次性喂入，
    /// 这里只改它的排序位置。
    pub fn set_aux_rank(&mut self, title: &str, rank: i32) {
        if let Some(ward) = self.aux.iter_mut().find(|w| w.title == title) {
            ward.rank = rank;
        }
    }

    /// 工具窗按 rank 升序的抬升顺序（[`enforce_aux_windows`] 用；也便于单测断言排序）。
    fn aux_raise_order(&self) -> Vec<&'static str> {
        let mut wards: Vec<(&'static str, i32)> =
            self.aux.iter().map(|w| (w.title, w.rank)).collect();
        wards.sort_by_key(|(_, rank)| *rank);
        wards.into_iter().map(|(t, _)| t).collect()
    }

    /// 临时不让位（见字段 [`Self::suppress_yield`]）。每个对账周期喂一次。
    pub fn set_suppress_yield(&mut self, on: bool) {
        self.suppress_yield = on;
    }

    /// 面板「总在最上」开关（每个对账周期喂一次）。
    ///
    /// 关掉 = 不与别的置顶窗争抢（"普通置顶"）。由关变开时下一周期就会重新对账并夺回
    /// 顶部，不需要在这里补一次 API 调用。
    pub fn set_always_on_top(&mut self, on: bool) {
        self.always_on_top = on;
    }

    /// 对账一次：让覆盖层处在"该在的位置"。幂等——状态正确时不调用任何 OS API。
    ///
    /// `overlay_parked` = 覆盖层当前休眠（没有像素在屏上，见 `runtime::overlay` 的缓冲
    /// 休眠）。此时**不碰覆盖层**：抬一个全透明窗口既没有视觉收益，还会和 Quicker、
    /// 任务管理器之类的置顶窗每 250ms 抢一次 z 序（日志里成串的"重新插回置顶带顶部"）。
    /// 工具窗（GIF 控件条、放大镜）仍照常守——它们可能正被用户操作。
    pub fn reconcile(&mut self, overlay_parked: bool) {
        if overlay_parked {
            if self.always_on_top {
                self.enforce_aux_windows();
            }
            // 休眠期间覆盖层可能被别的窗口压到任何位置：唤醒后第一拍按真实 z 序重新
            // 判断，而不是拿休眠前的旧账继续退避/让位。
            self.yielded = false;
            self.menu_yield = None;
            self.last_cover = None;
            self.strikes = 0;
            return;
        }

        let Some(hwnd) = self.resolve() else {
            return; // 覆盖层还没建出来
        };

        // ★ 工具窗（控件条）先守：它们**不参与**下面的全屏/弹窗让位逻辑（用户正要用，
        // 别把正在操作的控件藏起来）。但它要**跟随全局「总在最上」**：关掉开关 = "普通置顶"，
        // 控件条也一样不跟别的置顶窗争抢（用户 2026-09-20 决定：开关统一管全部）。
        if self.always_on_top {
            self.enforce_aux_windows();
        }

        if self.avoid_fullscreen && !self.suppress_yield {
            if let Some(why) = yield_reason(hwnd) {
                if !self.yielded {
                    self.yielded = true;
                    log::info!("前台是全屏应用/游戏，覆盖层让位（置底）：{why}");
                    unsafe { raise_to(hwnd, HWND_BOTTOM, "让位") };
                }
                return;
            }
        }
        if self.yielded {
            self.yielded = false;
            log::info!("全屏应用已退出（或「全屏避让」已关闭/已临时抑制），覆盖层恢复置顶");
            unsafe { raise_to(hwnd, HWND_TOPMOST, "恢复置顶") };
            return;
        }

        // ★ 弹出菜单 / 系统提示条必须留在覆盖层**上面**：压在下面就不响应鼠标、看不见了。
        // 让位动作是"插到它正下方"而非置底——覆盖层仍在其它所有窗口之上，
        // 拖尾照常可见，只有那一小块在它上面。
        //
        // ★★ 锚点粘性（2026-09-26 用户报：贴图右键菜单一开子菜单、父菜单就被盖住）：
        //   子菜单与父菜单**同为 `#32768` 类**，子菜单显示在后、z 序在父菜单之上——
        //   `visible_system_popup()` 每次都扫到"当前最上面的那个"，锚点就跟着跳：
        //   跳到子菜单时覆盖层被插到"子菜单正下方"，也就是**父菜单上面**，拖尾墨迹
        //   从此直接画在父菜单上（当天日志 17:48 一次菜单会话连跳 4 次锚，实锤）。
        //   修法：锚住第一个菜单后**粘住不放**，只要它还活着且可见就不再换锚——
        //   子菜单天然开在父菜单上方，"父菜单正下方"必然也是"子菜单下方"，无需跟着换。
        //   锚死了（菜单收场）才允许重新找弹窗，恢复置顶走下面 take() 分支。
        if let Some(menu) = visible_system_popup() {
            let anchor_alive = self.menu_yield.is_some_and(|m| {
                unsafe { IsWindow(Some(m)) }.as_bool() && unsafe { IsWindowVisible(m) }.as_bool()
            });
            if anchor_alive {
                return;
            }
            if self.menu_yield != Some(menu) {
                self.menu_yield = Some(menu);
                log::info!("系统弹窗在前，覆盖层让到它下方：{}", describe(menu));
                unsafe { raise_to(hwnd, menu, "让位（系统弹窗）") };
            }
            return;
        }
        if self.menu_yield.take().is_some() {
            // 弹窗刚关闭：把它腾出的位置收拾干净（否则覆盖层可能停在某个窗口下面）。
            log::info!("系统弹窗已关闭，覆盖层恢复置顶");
            unsafe { raise_to(hwnd, HWND_TOPMOST, "恢复置顶（弹窗关闭）") };
            return;
        }

        // ★ 「总在最上」关掉 = **不主动夺回顶部**（用户说的"普通置顶"）。
        //   覆盖层仍在置顶带里（建窗时带 `WS_EX_TOPMOST`），只是被后来的置顶窗压住时
        //   不再抢回来。放在让位逻辑**之后**：全屏避让与系统弹窗是"主动退让"，
        //   与这一位无关，关掉"总在最上"不该把它们一起废掉。
        if !self.always_on_top {
            self.last_cover = None;
            self.strikes = 0;
            return;
        }

        // 括号不是装饰：`let-else` 的 scrutinee 直接是 `unsafe { .. }` 时，
        // rustc 会把紧跟的 `else` 判成"块后 else"语法错误。
        let Some(cover) = (unsafe { cover_above(hwnd) }) else {
            // 上方没有可见的置顶窗口——已是该在的位置，重置争抢计数。
            self.last_cover = None;
            self.strikes = 0;
            return;
        };

        if self.last_cover == Some(cover) {
            self.strikes = self.strikes.wrapping_add(1);
        } else {
            self.last_cover = Some(cover);
            self.strikes = 1;
        }

        if self.strikes > MAX_STRIKES {
            // 对方抢回去也不放弃：退避到每 RETRY_EVERY 次对账（≈2s）重试一次。
            // 任务管理器每次刷新都会重新置顶自己，永久放弃会让覆盖层被压在下面再也
            // 回不来（用户实测：开任务管理器后拖尾"消失"）。2s 的重试间隔在它的两次
            // 刷新之间足够插回顶部，抖动又小到不可感知。
            if !self.strikes.is_multiple_of(RETRY_EVERY) {
                return;
            }
            log::debug!("退避重试：再次插回置顶带顶部（对手：{}）", describe(cover));
        } else {
            log::info!(
                "覆盖层被置顶窗口遮挡，重新插回置顶带顶部：{}",
                describe(cover)
            );
        }
        unsafe { raise_to(hwnd, HWND_TOPMOST, "重新插回置顶带顶部") };
    }

    /// 定位覆盖层窗口（首次查到后缓存；窗口重建时自动重查）。
    fn resolve(&mut self) -> Option<HWND> {
        if let Some(h) = self.hwnd {
            if unsafe { IsWindow(Some(h)) }.as_bool() {
                return Some(h);
            }
            self.hwnd = None;
        }
        // 按标题查窗：标题单一来源是 [`OVERLAY_TITLE`]（覆盖层建窗处同源，改一处即可）。
        let mut wide: Vec<u16> = OVERLAY_TITLE.encode_utf16().collect();
        wide.push(0);
        let h = unsafe { FindWindowW(None, windows::core::PCWSTR(wide.as_ptr())) }.ok()?;
        if h.is_invalid() {
            return None;
        }
        self.hwnd = Some(h);
        Some(h)
    }

    /// 守住我们自己的工具窗（控件条 / 裁剪窗）：周期性地把它们插回置顶带顶部。
    ///
    /// 道理与覆盖层完全相同（模块头已述）：建窗时的 `WS_EX_TOPMOST` 只把窗口放进置顶带，
    /// **不保证带内靠前**，后创建的置顶窗会插在它上面。控件条是一条没边框的小工具，
    /// 被盖住就等于功能消失 —— 用户 2026-09-20 报告"选框会被其它软件窗口遮挡"。
    ///
    /// ★ **无条件**重插（而非只在发现置顶遮挡时）：全屏网页视频这类"非置顶的最大化窗"
    ///   能盖住它却不会被 cover 扫描认出，必须每周期主动重插才能压回去（见函数体内说明）。
    /// 窗口不存在（还没开 / 已关）就跳过，不报错。
    fn enforce_aux_windows(&mut self) {
        // 先定抬升顺序（按 rank 升序）：一次排好，避免借 &mut self 之外再借 self.aux。
        let order = self.aux_raise_order();
        for title in order {
            let Some(hwnd) = find_by_title(title) else {
                continue;
            };
            // ★ 无条件插回置顶带顶部，而不是等 `cover_above` 发现被**置顶**窗遮挡才抬：
            //   网页全屏视频 = 浏览器窗口**最大化**（普通窗，非置顶），cover 扫描只查置顶窗、
            //   认不出它，于是按钮区被盖却没人抬（2026-09-20 用户报告：全屏播放视频时框线
            //   还在、按钮区被挡）。周期对账每 250ms 重插一次，代价只是每周期一次
            //   `SetWindowPos`（辅助窗就一两个），换来按钮区恒在最上。
            if unsafe { raise_aux(hwnd, title) } {
                log::debug!("工具窗「{title}」周期置顶");
            }
        }
    }
}

impl Default for TopmostKeeper {
    fn default() -> Self {
        Self::new()
    }
}

/// 把窗口插到置顶带顶部 / 置底。`SWP_NOACTIVATE` 保证不抢前台焦点。
///
/// 返回是否成功。**失败必须报出来**：`SetWindowPos` 静默失败时现象与"守护没生效"完全
/// 一样，而失败原因（句柄失效、被 UIPI 拦、参数写错）只能从这里看出来。
///
/// # Safety
/// `hwnd` 必须是有效窗口句柄；`anchor` 取 `HWND_TOPMOST` / `HWND_BOTTOM` 这两个哨兵值，
/// 或一个真实窗口句柄（表示"插到它**下面**"——弹出菜单让位就是这么用的）。
unsafe fn raise_to(hwnd: HWND, anchor: HWND, what: &str) -> bool {
    let r = unsafe {
        SetWindowPos(
            hwnd,
            Some(anchor),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
    if r.is_err() {
        log::warn!("覆盖层{what}失败（SetWindowPos 返回错误）");
    }
    r.is_ok()
}

/// 按标题找顶层窗口（与 `win_util` 里那几处同款：句柄存不进 `Runtime`，只能按标题查）。
fn find_by_title(title: &str) -> Option<HWND> {
    let mut wide: Vec<u16> = title.encode_utf16().collect();
    wide.push(0);
    let h = unsafe { FindWindowW(None, PCWSTR(wide.as_ptr())) }.ok()?;
    (!h.is_invalid()).then_some(h)
}

/// 把工具窗插回置顶带顶部。
///
/// 与 [`raise_to`] 分开只为日志准确：那条的失败信息写死了"覆盖层"，工具窗借它会误导排查。
/// `SWP_NOACTIVATE` 是必须的 —— 抬升不能把用户正在操作的窗口抢走焦点。
unsafe fn raise_aux(hwnd: HWND, title: &str) -> bool {
    let r = unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
    if r.is_err() {
        log::warn!("工具窗「{title}」重新置顶失败（SetWindowPos 返回错误）");
    }
    r.is_ok()
}

/// 枚举回调用的临时状态。
struct CoverScan {
    target: HWND,
    /// 覆盖层的矩形（整屏）。只有与它重叠的窗口才谈得上遮挡。
    ours: RECT,
    cover: Option<HWND>,
}

/// 能构成遮挡的最小边长（px）。见 [`cover_scan_proc`] 的说明。
const COVER_MIN_PX: i32 = 32;

/// 找到"真的盖在 `target` 上面"的置顶窗口。
///
/// `EnumWindows` 按 z 序自顶向下枚举，所以**先遇到的**就是最上层那个；走到 `target` 自己
/// 就说明上方没有合格的窗口，可以提前停。
///
/// # 判据为什么不能只看"置顶 + 可见"
///
/// 实测本机压在我们上面的 6 个窗口**全都不是遮挡**：两个 `0×0`、两个 `1×1`、其余隐藏
/// （都是 DWM／输入法／前台切换用的系统 helper）。只看"置顶且可见"会把那个 `1×1` 的
/// `ThumbnailDeviceHelperWnd` 当成遮挡者，于是每 250ms 徒劳抬升一次、几次后退出争抢——
/// 而**真正**盖住我们的窗口排在它下面，就此完全没人管（这正是第一版修复没生效的原因）。
///
/// 所以补两条：矩形非空且够大（[`COVER_MIN_PX`]）、且与覆盖层矩形相交。
///
/// # Safety
/// 仅作为 `EnumWindows` 的回调使用，`lparam` 必须指向存活到调用结束的 [`CoverScan`]。
unsafe extern "system" fn cover_scan_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = unsafe { &mut *(lparam.0 as *mut CoverScan) };
    if hwnd == scan.target {
        return BOOL(0); // 走到自己：上方没有构成遮挡的窗口
    }
    // ★ 同进程的窗口**默认**跳过：截图区域窗 / 贴图窗经 `win_util::activate_window_by_title`
    //   补了 `WS_EX_TOPMOST`，会被判成"遮挡者"——但那是**我们自己**的叠层 UI，不是外部
    //   遮挡。不豁免的话守护每 250ms 把覆盖层插回置顶带顶部，正好压住正在框选的区域窗
    //   （表现：选区被盖住、点不动，而日志里是一串"被置顶窗口遮挡"）。
    //   贴图窗是 owned 子窗、不带 TOPMOST，本来就不会被认出来。
    //
    // ★★ 例外：**全屏放大窗**（★ 2026-10-03 用户报"全屏放大的时候没有鼠标特效"）。
    //   它是置顶、alpha=255 的**整屏不透明**窗，盖住整块覆盖层 ⇒ 拖尾/按键回显/
    //   虚拟键盘全被遮掉。上面那条豁免的理由对它**一条都不成立**：它不参与任何
    //   交互（`WS_EX_TRANSPARENT` + `NOACTIVATE`），压住覆盖层不会让任何东西点不动，
    //   只会让特效凭空消失。所以这里不豁免，让守护能把它当遮挡者抢上去 ——
    //   覆盖层回到置顶带顶部，拖尾就画在放大画面**之上**（放大看的是屏幕内容，
    //   特效叠在上面正是画中画放大时已有的行为，两档一致）。
    if same_process(hwnd) && !is_fullscreen_zoom(hwnd) {
        return BOOL(1);
    }
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    if ex & WS_EX_TOPMOST.0 == 0 || !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut wr = RECT::default();
    if unsafe { GetWindowRect(hwnd, &mut wr) }.is_err() {
        return BOOL(1);
    }
    if wr.right - wr.left < COVER_MIN_PX
        || wr.bottom - wr.top < COVER_MIN_PX
        || !overlaps(scan.ours, wr)
    {
        return BOOL(1); // 系统 helper（0×0 / 1×1）或屏幕外窗口，够不着我们
    }
    scan.cover = Some(hwnd);
    BOOL(0) // 找到遮挡者，停
}

/// 两个矩形是否相交。
fn overlaps(a: RECT, b: RECT) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

/// `target` 上方是否压着**构成遮挡**的置顶窗口；有则返回最上层那个。
unsafe fn cover_above(target: HWND) -> Option<HWND> {
    let mut ours = RECT::default();
    if unsafe { GetWindowRect(target, &mut ours) }.is_err() {
        return None;
    }
    let mut scan = CoverScan {
        target,
        ours,
        cover: None,
    };
    let _ = unsafe {
        EnumWindows(
            Some(cover_scan_proc),
            LPARAM(std::ptr::from_mut(&mut scan) as isize),
        )
    };
    scan.cover
}

/// 必须留在覆盖层**之上**的系统弹窗窗口类。
///
/// · [`MENU_CLASS`] —— Win32 的**所有**弹出菜单：系统托盘右键菜单、桌面右键菜单、
///   应用菜单栏、下拉框展开的列表（`ComboLBox` 是另一类，暂未纳入）。
/// · [`TOOLTIP_CLASS`] —— 系统提示条：鼠标悬停在托盘图标/任务栏按钮上弹出的那条。
///
/// 两者都是 `WS_EX_TOPMOST`、矩形也够大，会被 [`cover_scan_proc`] 如实认成"遮挡者"，
/// 守护随即把覆盖层插回置顶带顶部——也就是插到了它们**上面**。于是"弹出的信息被虚拟
/// 键盘遮挡"（2026-09-18 用户报告）。判据与让位动作见 [`visible_system_popup`]。
const ABOVE_CLASSES: [&str; 2] = [MENU_CLASS, TOOLTIP_CLASS];

/// 弹出菜单的窗口类名。Win32 的**所有**弹出菜单都是这个类名的顶层窗口：系统托盘右键
/// 菜单、桌面右键菜单、应用菜单栏、下拉框展开的列表（`ComboLBox` 是另一类，暂未纳入）。
const MENU_CLASS: &str = "#32768";

/// 系统提示条（tooltip）的窗口类名。悬停托盘图标/任务栏按钮时弹的就是它。
const TOOLTIP_CLASS: &str = "tooltips_class32";

/// 当前是否有**可见的**系统弹窗（菜单 / 提示条）；有则返回它的窗口句柄。
///
/// # 为什么守护必须单独判它
///
/// 这是**本模块自己的副作用**，不是外部遮挡：菜单/提示条都是 `WS_EX_TOPMOST` 且矩形
/// 够大，[`cover_scan_proc`] 的判据会如实把它们认成"遮挡者"，守护随即把覆盖层插回置顶带
/// 顶部——也就是插到了它们上面。
///
/// 用户实测（2026-09-15）：开启拖尾后右键系统托盘图标"非常卡顿"，而任务管理器里 CPU 与
/// 内存都没有变化。当天日志里对应的正是三条
/// `覆盖层被置顶窗口遮挡，重新插回置顶带顶部：#32768 ""`——`#32768` 就是菜单窗口类。
/// 三次记录 = 用户反复右键尝试，随即退出程序。
///
/// 2026-09-18 又收到一条同源的现象：右键托盘时弹出的东西被虚拟键盘盖住——判据有洞，
/// 见 [`visible_window_of_class`]。
///
/// 原版 1.0.0 从来没有这个问题：**它没有这道守护**，菜单始终留在最上层。补丁 014 是为
/// 解决"被 Quicker 之类置顶窗压住看不见拖尾"才加的，代价就是会误伤菜单，故在此补回
/// 系统原本的 z 序语义。
///
/// 修复后菜单/提示条仍在覆盖层之上：它们正常响应鼠标，而覆盖层只落在它们正下方、
/// 依旧压着其它所有窗口，拖尾不会消失。
fn visible_system_popup() -> Option<HWND> {
    ABOVE_CLASSES
        .iter()
        .find_map(|c| visible_window_of_class(c))
}

/// 窗口类为 `class` 的第一个**可见**顶层窗口。
///
/// # 为什么不能只用 `FindWindowW`
///
/// `FindWindowW(class, None)` **只返回一个**该类的顶层窗口，且**不保证它可见**。而很多
/// 常驻进程（输入法皮肤、shell 组件、某些 Electron 外壳）都留着一个**隐藏的** `#32768`。
/// 一旦隐藏的那个排在前面，查到的就恒是它 → `IsWindowVisible` 判假 → 本函数返回 None →
/// 守护认定"此刻没有菜单"，把覆盖层插回置顶带顶部 —— **真菜单反而被压住**。这正是
/// 2026-09-18 用户报告的"右键系统托盘，弹出的东西被虚拟键盘遮挡"。
///
/// 所以分两步：先 `FindWindowW` 一发命中（常见情形，O(1)）；**只有**命中的那个不可见时
/// 才升级为 `EnumWindows` 全枚举找可见的。判据修好了，同时保住"该类窗口一个都没有时
/// 一次枚举都不做"的开销纪律（本函数每 250ms 被对账调用一次）。
fn visible_window_of_class(class: &str) -> Option<HWND> {
    let h = find_top_level(class)?;
    if unsafe { IsWindowVisible(h) }.as_bool() {
        return Some(h);
    }
    first_visible_of_class(class)
}

/// `FindWindowW` 按类名取一个顶层窗口（不保证可见）。
fn find_top_level(class: &str) -> Option<HWND> {
    let cls = windows::core::HSTRING::from(class);
    let h = unsafe { FindWindowW(windows::core::PCWSTR(cls.as_ptr()), None) }.ok()?;
    if h.is_invalid() {
        None
    } else {
        Some(h)
    }
}

/// [`first_visible_of_class`] 的枚举状态。
struct ClassScan {
    class: String,
    found: Option<HWND>,
}

/// 找"可见 + 类名匹配"的顶层窗口；找到即停。
///
/// # Safety
/// 仅作为 `EnumWindows` 的回调使用，`lparam` 必须指向存活到调用结束的 [`ClassScan`]。
unsafe extern "system" fn class_scan_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let scan = unsafe { &mut *(lparam.0 as *mut ClassScan) };
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 {
        return BOOL(1);
    }
    if String::from_utf16_lossy(&buf[..n as usize]) == scan.class {
        scan.found = Some(hwnd);
        return BOOL(0); // 找到，停
    }
    BOOL(1)
}

/// 第一个可见的同类顶层窗口。`EnumWindows` 自顶向下枚举，故拿到的是**最上层**那个。
fn first_visible_of_class(class: &str) -> Option<HWND> {
    let mut scan = ClassScan {
        class: class.to_string(),
        found: None,
    };
    let _ = unsafe {
        EnumWindows(
            Some(class_scan_proc),
            LPARAM(std::ptr::from_mut(&mut scan) as isize),
        )
    };
    scan.found
}

/// 前台是否有全屏应用/游戏占着屏幕；有则返回让位原因（写日志用，也便于判据调试）。
///
/// 判据只认两条**不会误报**的信号。踩过的坑都记在这里：
///
/// · **不用 `QUNS_BUSY`（=2）**：实测本机一启动就报 2——因为触发它的正是**我们自己**：
///   覆盖层是前台窗口，铺满整屏又没有标题栏，shell 判定"有全屏应用在前台"。于是形成
///   "自己让自己让位"的循环（先置底、下个周期又恢复、再置底）。改成只认
///   `QUNS_RUNNING_D3D_FULL_SCREEN`（D3D 独占全屏）与 `QUNS_PRESENTATION_MODE`。
/// · **前台窗口属于本进程时一律不算全屏应用**：同上，罪魁是自己。
fn yield_reason(ours: HWND) -> Option<String> {
    let fg = unsafe { GetForegroundWindow() };
    let ours_is_fg = !fg.is_invalid() && same_process(fg);

    if !ours_is_fg {
        if let Ok(state) = unsafe { SHQueryUserNotificationState() } {
            if state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE {
                return Some(format!(
                    "系统报告独占全屏/演示态（SHQueryUserNotificationState={}）",
                    state.0
                ));
            }
        }
    }

    // 兜底（覆盖无边框全屏：上面的信号对它不一定报全屏）：前台窗口铺满所在显示器的完整
    // 矩形，且既非最大化、也没有标题栏。
    //
    // "最大化"与"shell 窗口/本进程窗口"几个排除项都不是可选的：最大化窗口同样铺满显示器
    // （但那是普通窗口，我们本该压在它上面）；桌面（Progman/WorkerW）、任务栏这类 shell
    // 窗口也是铺满整屏且无标题栏。
    unsafe {
        if fg.is_invalid() || fg == ours || !IsWindowVisible(fg).as_bool() {
            return None;
        }
        if same_process(fg) || fg == GetShellWindow() || fg == GetDesktopWindow() {
            return None;
        }
        if IsZoomed(fg).as_bool() {
            return None;
        }
        let style = GetWindowLongPtrW(fg, GWL_STYLE) as u32;
        if style & WS_CAPTION.0 != 0 {
            return None;
        }
        if !covers_its_monitor(fg) {
            return None;
        }
        Some(format!("前台窗口铺满显示器且无边框：{}", describe(fg)))
    }
}

/// 该窗口是否属于当前进程。
fn same_process(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid == unsafe { GetCurrentProcessId() }
}

/// 该窗口是不是**全屏放大**那个窗（[`FULLSCREEN_WINDOW_TITLE`]）。
///
/// 为什么用标题而不是记一个 HWND：放大窗是**惰性建窗**的（第一次真正 present 才建），
/// 守护每 250ms 独立跑一次、拿不到 `Runtime` 里的实例字段；而标题是建窗时就固定的
/// （[`PipWindow::with_title`]），同进程同标题唯一 —— 与"z 序守护定位覆盖层靠标题"
/// 是同一套路子。
///
/// ★ 只认**全屏**不认画中画：画中画只占屏幕一小块，让覆盖层抢到它上面会在那一小块
///   遮住放大画面（点得到、看得见，只是被特效压着），得不偿失；全屏是整屏，不抢就
///   全屏都没有特效。二者的取舍不同，所以不能一并豁免。
fn is_fullscreen_zoom(hwnd: HWND) -> bool {
    window_title(hwnd).as_deref() == Some(crate::platform::pip_window::FULLSCREEN_WINDOW_TITLE)
}

/// 读窗口标题（`GetWindowTextW`）。读不到 / 空标题都返回 `None`。
fn window_title(hwnd: HWND) -> Option<String> {
    let n = unsafe { GetWindowTextLengthW(hwnd) };
    if n <= 0 {
        return None;
    }
    let mut buf = vec![0u16; (n as usize) + 1];
    let got = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if got <= 0 {
        return None;
    }
    buf.truncate(got as usize);
    Some(String::from_utf16_lossy(&buf))
}

/// 前台窗口是否覆盖了它所在显示器的完整矩形。
unsafe fn covers_its_monitor(fg: HWND) -> bool {
    let mon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(mon, &mut info) }.as_bool() {
        return false;
    }
    let mut wr = RECT::default();
    if unsafe { GetWindowRect(fg, &mut wr) }.is_err() {
        return false;
    }
    wr.left <= info.rcMonitor.left
        && wr.top <= info.rcMonitor.top
        && wr.right >= info.rcMonitor.right
        && wr.bottom >= info.rcMonitor.bottom
}

/// 窗口的"类名 + 标题"，用于日志里指明是谁在遮挡。
fn describe(hwnd: HWND) -> String {
    let mut cls = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut cls) };
    let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);

    let mut title = [0u16; 256];
    let n = unsafe { GetWindowTextW(hwnd, &mut title) };
    let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);

    format!("{cls} {title:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 窗口定位用的是标题常量，两处必须同源——这里守住这一点。
    #[test]
    fn overlay_title_is_non_empty() {
        assert!(!OVERLAY_TITLE.is_empty());
        assert!(OVERLAY_TITLE.is_ascii());
    }

    /// 本文件**生产代码**文本（`#[cfg(test)]` 之前那一段）——结构性门禁用。
    /// 同款先例见 `platform::clipboard_history::tests::production_src`。
    fn production_src() -> &'static str {
        const SRC: &str = include_str!("topmost.rs");
        let cut = SRC
            .find("mod tests {")
            .and_then(|i| SRC[..i].rfind("#[cfg(test)]"))
            .unwrap_or(SRC.len());
        &SRC[..cut]
    }

    /// ★★ 任务 5 的回归锁：全屏放大窗**不能**被"同进程一律跳过"那条豁免吃掉。
    ///
    /// 用户口径（2026-10-03）：*"画中画放大时，有鼠标特效。但是全屏放大的时候没有。"*
    ///
    /// 这个不对称有**两个**独立成因，`runtime::frame` 那边修掉了几何（提前 `return`），
    /// 这边是 z 序：全屏放大窗是置顶、alpha=255 的整屏不透明窗，而 `cover_scan_proc`
    /// 原本豁免所有同进程窗口 ⇒ 覆盖层永远抢不到它上面去，特效画了也看不见。
    ///
    /// 这条只能写成结构性门禁：`is_fullscreen_zoom` 要真判出一个 HWND 得先有那个窗，
    /// 单测环境里没有。断言改豁免条件本身，比"造个窗再验"轻得多、也更能钉住意图。
    #[test]
    fn the_fullscreen_zoom_window_is_not_exempted_from_the_topmost_guard() {
        let src = production_src();
        // 豁免判据必须带 `!is_fullscreen_zoom(hwnd)` 这个例外项。
        assert!(
            src.contains("if same_process(hwnd) && !is_fullscreen_zoom(hwnd) {"),
            "全屏放大窗必须从\"同进程一律跳过\"里豁免出去，否则它会盖住整块覆盖层、\
             鼠标特效全被吞掉（用户 2026-10-03 报）"
        );
        // 例外只认**全屏**，不能顺手把画中画也放进来：画中画只占一小块，让覆盖层抢上去
        // 会遮住放大画面本身（点得到、看得见，只是被特效压着）。
        assert!(
            !src.contains("if same_process(hwnd) && !is_fullscreen_zoom(hwnd) && !is_pip"),
            "别把画中画一起豁免（它只占一小块，抢上去会遮住放大画面）"
        );
        // 判据认的是标题常量，两个窗不能同名（否则画中画也会被当成全屏）。
        assert_eq!(
            crate::platform::pip_window::PIP_WINDOW_TITLE,
            "CurSeen 画中画放大"
        );
        assert!(
            crate::platform::pip_window::PIP_WINDOW_TITLE
                != crate::platform::pip_window::FULLSCREEN_WINDOW_TITLE,
            "画中画与全屏放大的标题不能相同：判据按标题认，相同就会互相误认"
        );
    }

    #[test]
    fn keeper_starts_neutral() {
        let k = TopmostKeeper::new();
        assert!(k.hwnd.is_none());
        assert!(!k.yielded);
        assert!(k.last_cover.is_none());
        assert_eq!(k.strikes, 0);
        assert!(k.menu_yield.is_none());
        // 出厂态：总在最上开、全屏避让关（与 `OverlaySettings::default()` 同源）。
        assert!(k.always_on_top);
        assert!(!k.avoid_fullscreen);
    }

    /// 「全屏避让」的取值必须真被存下来（门控 `reconcile` 里就是
    /// `if self.avoid_fullscreen`，改它必然要来这里同步）。
    #[test]
    fn avoid_fullscreen_flag_is_stored() {
        let mut k = TopmostKeeper::new();
        k.set_avoid_fullscreen(false);
        assert!(!k.avoid_fullscreen);
        k.set_avoid_fullscreen(true);
        assert!(k.avoid_fullscreen);
    }

    /// ★ 「轴 2·跨窗优先级」：工具窗在置顶带内按 rank 升序抬升，rank 大者更靠上。
    ///
    /// 默认 `guard_aux` 按喂入顺序赋 `0..n`；`set_aux_rank` 可把任意一个窗口提到更高层，
    /// 抬升顺序随之由高到底、且不依喂入顺序。rank 排序是纯逻辑，单测直接断言。
    #[test]
    fn aux_raise_order_follows_rank_not_feed_order() {
        let mut k = TopmostKeeper::new();
        k.guard_aux(vec!["bar", "trim", "note"]);
        // 默认：喂入顺序 = rank 0,1,2 → 抬升顺序同喂入。
        assert_eq!(k.aux_raise_order(), vec!["bar", "trim", "note"]);

        // 把 "bar" 提到最高（rank 99）：抬升顺序应把最高 rank 放到最后（最后抬=最上）。
        k.set_aux_rank("bar", 99);
        assert_eq!(k.aux_raise_order(), vec!["trim", "note", "bar"]);

        // 改成并列之后的相对顺序仍稳定；未登记的标题被 setter 忽略、不报错。
        k.set_aux_rank("nonexistent", -5);
        assert_eq!(k.aux_raise_order(), vec!["trim", "note", "bar"]);
    }

    /// 「总在最上」关闭后，**夺回顶部**那条路必须整体失效——否则关开关等于没关。
    ///
    /// 与 `avoid_fullscreen_flag_is_stored` 同构：`reconcile` 需要真实窗口句柄（测试环境
    /// 拿不到），所以这里只钉住"开关值确实存下来了"这一层；门控在 `reconcile` 里是一行
    /// `if !self.always_on_top { .. return }`，改动它必然要来这里同步。
    ///
    /// ★ 这一位在 2026-09-18 之前是**死字段**（配置有、勾选框有、无人读），
    ///   这条用例把"它真的被读走了"钉进回归网。
    #[test]
    fn always_on_top_flag_is_stored() {
        let mut k = TopmostKeeper::new();
        k.set_always_on_top(false);
        assert!(!k.always_on_top);
        k.set_always_on_top(true);
        assert!(k.always_on_top);
    }

    /// 菜单类名常量必须带 `#`：`FindWindowW` 用的是类名而不是标题，写成 `"32768"`
    /// 会静默查不到任何窗口，菜单让位也就从未生效——与"守护没生效"现象完全一样。
    #[test]
    fn menu_class_is_the_win32_menu_window_class() {
        assert_eq!(MENU_CLASS, "#32768");
    }

    /// ★ 让位判据必须**同时**覆盖菜单与系统提示条。
    ///
    /// 2026-09-18 用户报告"右键系统托盘，弹出的信息被虚拟键盘遮挡"：提示条
    /// （`tooltips_class32`）与菜单一样会被 [`cover_scan_proc`] 认成遮挡者，守护随即把
    /// 覆盖层插到它上面。这条把两个类名一起钉住——将来有人只留一个，回归网立刻响。
    #[test]
    fn yield_set_covers_menus_and_tooltips() {
        assert!(
            ABOVE_CLASSES.contains(&MENU_CLASS),
            "弹出菜单必须在让位集合里"
        );
        assert!(
            ABOVE_CLASSES.contains(&TOOLTIP_CLASS),
            "系统提示条必须在让位集合里"
        );
        assert_eq!(TOOLTIP_CLASS, "tooltips_class32");
    }

    /// ★ 可见性判据不能采信 `FindWindowW` 的那一发命中。
    ///
    /// `FindWindowW(class, None)` 只返回**一个**同类顶层窗口，且不保证可见。旧实现直接
    /// 拿它 + `IsWindowVisible` 下结论：隐藏的那个排在前面时判据返回 None，守护遂认定
    /// "此刻没有菜单"，把覆盖层插到**真菜单**上面——用户看到的就是"弹出的东西被虚拟键盘
    /// 遮挡"。
    ///
    /// 这里用一对**自注册类**的真实窗口把两种情形都跑一遍（用自造类名而不是 `#32768`：
    /// 判据函数参数化的就是类名，自造类名同样能证明逻辑，还不会去碰系统菜单窗口、
    /// 也不会干扰正在运行的实例）：
    ///
    /// · 先建隐藏、后建可见 → `FindWindowW` 若命中隐藏的那个（大多数 z 序下如此），
    ///   就必须落到枚举兜底上，`visible_window_of_class` 仍要交出**可见**窗口；
    /// · 反过来（先可见、后隐藏）同样只要求"交出来的必须可见"。
    ///
    /// 契约是"绝不返回不可见窗口"——它不依赖 z 序，所以两种顺序都断言同一个契约；
    /// z 序决定的是走哪条路径，路由由 [`find_top_level`] 的返回值如实报告。
    #[test]
    fn visibility_check_never_returns_a_hidden_window() {
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, ShowWindow,
            UnregisterClassW, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WNDCLASSW, WS_EX_TOOLWINDOW,
            WS_POPUP,
        };

        const PROBE_CLASS: &str = "CurSeenZorderProbeWnd";

        unsafe extern "system" fn wndproc(
            hwnd: HWND,
            msg: u32,
            wp: windows::Win32::Foundation::WPARAM,
            lp: LPARAM,
        ) -> windows::Win32::Foundation::LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }

        // 类名唯一，重复注册（并行跑测试时）只会失败，视作"类已在"。
        let cls = windows::core::HSTRING::from(PROBE_CLASS);
        let hinst = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: windows::core::PCWSTR(cls.as_ptr()),
            ..Default::default()
        };
        let atom = unsafe { RegisterClassW(&wc) };
        if atom == 0 && find_top_level(PROBE_CLASS).is_none() {
            return; // 建不了窗（无桌面会话等）：跳过
        }

        struct Win(HWND);
        impl Drop for Win {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let create = |visible: bool| -> Option<Win> {
            let h = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
                    windows::core::PCWSTR(cls.as_ptr()),
                    windows::core::w!(""),
                    WS_POPUP,
                    0,
                    0,
                    40,
                    40,
                    None,
                    None,
                    Some(hinst.into()),
                    None,
                )
            }
            .ok()?;
            if h.is_invalid() {
                return None;
            }
            let w = Win(h);
            if visible {
                unsafe {
                    let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                }
            }
            Some(w)
        };

        let check = |order: &str, hidden: &Win, shown: &Win| {
            assert!(
                !unsafe { IsWindowVisible(hidden.0) }.as_bool(),
                "[{order}] 前提：这一只确实隐藏"
            );
            assert!(
                unsafe { IsWindowVisible(shown.0) }.as_bool(),
                "[{order}] 前提：另一只确实可见"
            );
            let first = find_top_level(PROBE_CLASS);
            let picked = visible_window_of_class(PROBE_CLASS);
            let picked = picked.unwrap_or_else(|| {
                panic!("[{order}] 存在可见的同类窗口，判据不该返回 None（首发命中 {first:?}）")
            });
            assert!(
                unsafe { IsWindowVisible(picked) }.as_bool(),
                "[{order}] 交出的必须是可见窗口（旧实现会交出首发命中的隐藏窗口 {first:?}）"
            );
            assert_eq!(picked, shown.0, "[{order}] 应当交出的正是那只可见窗口");
        };

        // 顺序 A：先隐藏、后可见。--nocapture 下会打印首发命中，用于确认兜底路径被走到。
        let (h1, s1) = match (create(false), create(true)) {
            (Some(a), Some(b)) => (a, b),
            _ => return,
        };
        eprintln!(
            "ZORDER-DIAG 先隐藏后可见: first_hit_is_hidden={:?}",
            find_top_level(PROBE_CLASS).map(|h| !unsafe { IsWindowVisible(h) }.as_bool())
        );
        check("先隐藏后可见", &h1, &s1);
        drop(h1);
        drop(s1);

        // 顺序 B：先可见、后隐藏。
        let (s2, h2) = match (create(true), create(false)) {
            (Some(a), Some(b)) => (a, b),
            _ => return,
        };
        eprintln!(
            "ZORDER-DIAG 先可见后隐藏: first_hit_is_hidden={:?}",
            find_top_level(PROBE_CLASS).map(|h| !unsafe { IsWindowVisible(h) }.as_bool())
        );
        check("先可见后隐藏", &h2, &s2);
        drop(h2);
        drop(s2);

        unsafe {
            let _ = UnregisterClassW(windows::core::PCWSTR(cls.as_ptr()), Some(hinst.into()));
        }
    }

    /// ★★ 覆盖层休眠时不得再抢 z 序（2026-10-24 用户口径"全关时还有东西在跑"）。
    ///
    /// 休眠态的覆盖层是全透明的、屏上没有一个像素；此时每 250ms 扫描遮挡并
    /// `SetWindowPos` 抬升，只会和 Quicker 等置顶窗反复争抢（日志成串）。判据必须
    /// 在定位窗口之前短路，且工具窗守卫要留在短路分支里（放大镜/控件条可能正被用）。
    #[test]
    fn parked_overlay_short_circuits_before_resolving_the_window() {
        let src = production_src();
        let parked_at = src.find("if overlay_parked {").unwrap_or(usize::MAX);
        let resolve_at = src
            .find("let Some(hwnd) = self.resolve()")
            .unwrap_or(usize::MAX);
        assert!(
            parked_at < resolve_at,
            "休眠短路必须写在 resolve 之前，否则仍会枚举窗口/抢 z 序"
        );
        let block = &src[parked_at..resolve_at];
        assert!(
            block.contains("self.enforce_aux_windows();"),
            "休眠时工具窗（放大镜/控件条）仍要守"
        );
        assert!(block.contains("return;"), "短路分支必须直接返回");
        assert!(
            !block.contains("cover_above") && !block.contains("raise_to("),
            "休眠分支里不该出现任何覆盖层抬升/遮挡扫描"
        );
    }

    /// 退避参数的自洽性：重试周期必须大于快抢次数，否则退避形同虚设。
    /// （两个常量本可静态断言，这里写成测试是为了给改参数的人留一条门禁。）
    #[test]
    fn backoff_parameters_are_sane() {
        const _CHECK: () = assert!(RETRY_EVERY > MAX_STRIKES);
    }
}
