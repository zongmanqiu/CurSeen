//! 便签（Sticky Note）原生窗口。
//!
//! # 为什么是**原生 Win32 窗**而不是 windui 绘制窗 / 覆盖层子窗
//!
//! 便签要一个**完整可编辑的多行文本框（含中文 IME）**。CurSeen 其余窗口（pin / timer /
//! spotlight）都是"只读绘制 + 不抢焦点"，没有文本编辑能力；windui 也没有 textbox 控件。
//! 与其自绘光标 / 选词 / IME 候选框（违背"克制引库"、工程量极大），不如把文本区挂一个
//! **原生 EDIT 子控件**——IME / 光标 / 滚动全是系统能力，免费且成熟（参照 WeNote 用
//! RichEdit 的同一思路，但我们用更轻的 EDIT：用不上富文本，少注册一个 riched20 类）。
//!
//! # 键盘钩子不吞键（可行性钥匙）
//!
//! 全局低级键盘钩子（`platform::input::keyboard`）只做归一化 + `tx.send`，末尾恒
//! `CallNextHookEx` 透传、**从不吞键**。所以 EDIT 子控件能正常收到键盘与 IME，钩子只是
//! 旁路 —— 这是"便签能打中文"成立的前提。
//!
//! # 线程模型：独立专用线程，脱离 windui 主循环
//!
//! 每条便签是一个 `WS_POPUP` 原生顶层窗（标题栏自绘 + 一个 EDIT 子控件），跑在**一个
//! 独立的专用线程**上，用 `PeekMessage/TranslateMessage/DispatchMessage` 轮询泵消息。
//! 主线程只经 `Sender<Cmd>` 下发「新增 / 应用配置 / 退出」，不挤占 windui 每帧循环。
//! EDIT 的 IME / 键盘在 pump 线程上正常派发。
//!
//! # 窗口组成：**两张 layered 窗**（而不是一张）
//!
//! ```text
//! ┌──────────────────────────────┐
//! │  [+]  [透][置][锁][收]  [×]  │  ← A 标题栏窗（自绘；只上色这一条 + 三边 1px 边环）
//! ├──────────────────────────────┤
//! │                              │  ← B 正文窗（独立顶层窗）
//! │         EDIT 文本区            │     里面是它的子 EDIT（自动换行 / IME / Ctrl+滚轮改字号）
//! │          （可编辑 + IME）        │
//! └──────────────────────────────┘
//! ```
//! ★★★ **为什么是两张：为了让「背景颜色的透明度」和「编辑区颜色的透明度」真的能分开生效。**
//!   `LWA_ALPHA` 只给**整张窗口**一个 alpha：一张窗里无论哪个区域，最终都乘同一个值
//!   —— GDI 又完全不认 alpha（`COLORREF` 就是 `0x00BBGGRR`，第 4 个字节直接被丢），
//!   于是"颜色里的透明度"在单窗结构下**永远没有任何作用**（旧 bug：设了跟没设一个样）。
//!   拆成两张 layered 窗后，标题栏与正文各持一个 alpha，与桌面合成的结果和
//!   "每个区域独立半透明"**数学上完全等价**。
//!   · TITLE_H 以下的区域标题栏窗**一字不上色**（上了色就会被正文层叠一遍，拿出双重底色）；
//!   · 正文窗里 EDIT 仍是真子窗体，所以 `WM_CTLCOLOREDIT` / IME / 焦点三件套全都照旧
//!     （只是接收者从标题栏窗换成了正文窗）。
//!
//! ★ 图标：`＋` 新增；`◐` 透明度（弹出拖动条）；`\u{1f4cc}` 置顶开关；
//!   `\u{1f512}/\u{1f513}` 锁定（\u{1f512}=已锁）；`▼/▲` 收起/展开；`×` 关闭。
//!   左侧 ＋，右侧五个从右往左排（透明度按用户口径放在**置顶左侧**）。
//!   （字形定义见 `btn_glyph`，Segoe UI Symbol 单色字形，随文字色着色。）
//!   锁定 = 文本只读 + 不可拖动/缩放 + 置顶/收起/关闭全禁（只留 ＋ 与解锁）。
//! * 顶层置顶：`WS_EX_TOOLWINDOW`（不进任务栏），置顶切换走 `SetWindowPos`。
//!   **不带** `WS_EX_NOACTIVATE`，否则点不进 Edit 输入（要抢一次焦点才可打字）。
//!   正文窗以标题栏窗为 owner（所有者窗恒在 owner 之上，owner 销毁时自动跟着走）。
//!
//! # 透明度：**总不透明度 × 颜色的不透明度**
//!
//! | 层 | 它的 alpha | 算法 |
//! |---|---|---|
//! | 标题栏窗（含边环） | 总不透明度 × 背景色 alpha | [`layered_alpha`] |
//! | 正文窗（含 EDIT） | 总不透明度 × 编辑区色 alpha | [`layered_alpha`] |
//! | 文字 / 图标 | 按文字色 alpha 预混进**它下面那层底色** | [`over`] |
//!
//! 层内部本来就不透光，所以"文字半透明"把它预混到背景上再乘整层 alpha，结果与
//! "文字独立半透明合成"**完全等价**（不是近似）——这正是 GDI 不认 alpha 时唯一正确的做法。
//! 总不透明度 = 单张便签自己的值（标题栏「透明度」按钮弹出的拖动条，10–100）；
//! 面板里的「透明度」是**出厂默认值**，只用于新建便签与配置变更时的广播，不会被单张便签写回。
//! * 拖拽：`WM_NCHITTEST` 对标题栏返回 `HTCAPTION`（自管，不调 `Element::window_drag`）。
//! * 标题栏按钮：自绘文字方块 + `WM_LBUTTONUP` 命中判断，不逐按钮建控件。
//!
//! # 单位
//!
//! 逻辑像素 = 物理像素（进程是 Per-Monitor DPI Aware v2）。

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Mutex;
use std::thread::{self, JoinHandle};

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateFontW, CreatePen, CreateRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    FillRect, GetDC, InvalidateRect, LineTo, MoveToEx, ReleaseDC, SelectObject, SetBkColor,
    SetBkMode, SetGraphicsMode, SetTextColor, SetWindowRgn, SetWorldTransform, CLEARTYPE_QUALITY,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DEFAULT_PITCH, DT_CENTER, DT_SINGLELINE, DT_VCENTER,
    FF_DONTCARE, FW_NORMAL, GM_ADVANCED, GM_COMPATIBLE, HBRUSH, HDC, HFONT, HGDIOBJ, OUT_TT_PRECIS,
    PS_SOLID, RGN_DIFF, TRANSPARENT, XFORM,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::{EM_SETREADONLY, WM_MOUSELEAVE};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, SetFocus, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, VK_ESCAPE, VK_LBUTTON, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
    GetGUIThreadInfo, GetParent, GetWindowLongPtrW, GetWindowRect, MoveWindow, PeekMessageW,
    PostMessageW, RegisterClassExW, SendMessageW, SetWindowLongPtrW, SetWindowPos, ShowWindow,
    TranslateMessage, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE, ES_WANTRETURN,
    GUITHREADINFO, GUITHREADINFO_FLAGS, GUI_INMOVESIZE, GWLP_USERDATA, GWLP_WNDPROC,
    HWND_NOTOPMOST, HWND_TOPMOST, MINMAXINFO, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SWP_NOZORDER, SW_HIDE, SW_SHOW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_ACTIVATE, WM_APP,
    WM_CLOSE, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO,
    WM_KEYDOWN, WM_KILLFOCUS, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEACTIVATE, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WM_SETFOCUS, WM_SETFONT, WM_SIZE,
    WM_WINDOWPOSCHANGED, WNDCLASSEXW, WNDCLASS_STYLES, WNDPROC, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
};

use crate::core::config::{NOTE_ALPHA_MAX, NOTE_ALPHA_MIN, NOTE_FONT_MAX, NOTE_FONT_MIN};
use crate::platform::win_util;

/// 窗口类名（进程内一次性注册）。`pub` 给真机探针（`tests/probes/note_alpha_probe`）
/// 用：它要按类名把两张窗找出来，才能把 Windows 真正生效的 alpha 读回来。
pub const CLASS_NAME: &str = "CurSeenStickyNoteClass";
/// 窗口标题（任务栏 / 排查识别；不登记 TopmostKeeper，自管置顶）。
const NOTE_TITLE: &str = "CurSeen 便签";
/// 正文窗类名（**独立顶层窗**，承载 EDIT；与标题栏窗分开才有自己的 alpha）。
/// `pub` 同上（探针读回 alpha 时要按类名找它）。
pub const BODY_CLASS: &str = "CurSeenStickyBodyClass";
/// 「透明度」拖动条弹窗类名（标题栏按钮点出来的那个小窗）。
const ALPHA_CLASS: &str = "CurSeenStickyAlphaClass";

/// 标题栏高度（物理像素）。
const TITLE_H: i32 = 26;
/// 标题栏按钮宽与间距（高度占满整条标题栏，不再单独设高）。
const BTN_W: i32 = 24;
const BTN_GAP: i32 = 2;
/// 正文区边距。★★★ 1px：便签要小巧，6px 的边框浪费一圈无用空间（用户定案）。
const MARGIN: i32 = 1;
/// 便签默认客户区宽高（物理像素）。
const DEFAULT_W: i32 = 272;
const DEFAULT_H: i32 = 220;

/// 标题栏按钮索引（自绘命中）。
const BTN_ADD: u8 = 0;
const BTN_TOP: u8 = 1;
const BTN_LOCK: u8 = 2;
const BTN_COLLAPSE: u8 = 3;
const BTN_CLOSE: u8 = 4;
/// 透明度按钮（弹拖动条）。★ 排在**置顶左侧**：右侧一串从右往左的下标 3 是置顶，
/// 4 就是它左边那格（用户口径："标题栏、置顶左侧，增加一个透明度设置按钮"）。
const BTN_ALPHA: u8 = 5;

/// 从 `LPARAM` 里取出打包的两个坐标：**低 16 位 x、高 16 位 y，且都是有符号**。
///
/// ★★★ 这个函数存在的唯一理由是把"取 y"写对。曾经就地写成
///   `let cy = (lparam.0 as u16 as u32 >> 16) as i32;` —— Rust 里 `as` 的优先级
///   **高于** `>>`，所以它等于 `(低位 as u32) >> 16`，**恒为 0**。后果是整条标题栏
///   只有"按住能拖"，五个按钮一个都点不中（`hit_btn` 收到的 y 永远落在按钮矩形上方）。
///   凡要拆 `LPARAM` 坐标，一律走这里，别再手抄表达式。
fn lparam_xy(lparam: LPARAM) -> (i32, i32) {
    let v = lparam.0;
    let x = (v & 0xFFFF) as u16 as i16 as i32;
    let y = ((v >> 16) & 0xFFFF) as u16 as i16 as i32;
    (x, y)
}

/// 拆 `WM_CTLCOLOR*` 的两个参数：**wParam 是 DC、lParam 是控件 HWND**。
///
/// ★★★ 抽成纯函数只为把这条**反直觉**的约定钉进单测。它跟 `WM_NCHITTEST` /
/// `WM_LBUTTON*` 那批"东西在 lParam"的习惯正好反过来 —— 当年顺手把 lParam 当 DC 用，
/// `SetBkColor`/`SetTextColor`/`SetBkMode` 三个调用**全部静默失败**（句柄非法、不报错），
/// 只有返回的画刷生效。症状极具迷惑性：整块背景看着是对的，但 EDIT 画文本行时用
/// 默认白底 —— 深底便签上就是"正文顶部一条白"。
fn ctl_color_parts(wparam: WPARAM, lparam: LPARAM) -> (HDC, HWND) {
    (HDC(wparam.0 as *mut _), HWND(lparam.0 as *mut _))
}

/// 窗口 `/ WM_ACTIVATE` 的 wParam 低 16 位：0=失活、1=鼠标激活、2=其它方式激活。
const WA_INACTIVE: usize = 0;

/// 状态位。
const FLG_TOPMOST: u8 = 1;
const FLG_LOCKED: u8 = 2;
const FLG_COLLAPSED: u8 = 4;

/// 现存便签主窗句柄（pump 线程独占访问，主线程只经命令通道，故无并发竞争）。
/// 存 `isize`（`HWND.0` 地址）而非 `HWND`：`HWND` 内部是 `*mut c_void`，非 `Send`，
/// 静态初始化器要求 `Send`。使用处转回 `HWND`。
static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// 便签配置镜像（值拷贝发给 pump 线程，避免把 `&mut Config` 送进别线程造成别名）。
#[derive(Debug, Clone, PartialEq)]
pub struct NoteCfg {
    /// 整体背景（标题栏 + 边框一圈）。
    pub bg: [u8; 4],
    /// 文字色（正文 + 标题栏按钮图标）。
    pub text: [u8; 4],
    /// 编辑区背景（正文区，可与背景区分）。
    pub edit: [u8; 4],
    pub alpha: u32,
    pub font_size: f32,
}

impl Default for NoteCfg {
    fn default() -> Self {
        Self {
            bg: [40, 44, 52, 235],
            text: [232, 236, 240, 255],
            edit: [52, 56, 66, 235],
            alpha: 100,
            font_size: 16.0,
        }
    }
}

/// pump 线程命令。
enum Cmd {
    Spawn(NoteCfg),
    Apply(NoteCfg),
    /// 关掉**全部**已开的便签（功能开关关掉的边沿，见 `StickyNoteModule::tick`）。
    ///
    /// ★ 为什么不用 [`Cmd::Shutdown`]：那个会 `quit = true` 并让 pump 线程退出、
    ///   顺带 `join` —— 而功能开关是**可以再开**的，用户重开勾选后还得能呼出便签。
    ///   这条只销毁窗、线程继续活着。
    CloseAll,
    Shutdown,
}

/// 一条便签窗口的运行态。
struct WinState {
    /// 承载 EDIT 的**独立顶层窗**（正文窗），以标题栏窗为 owner。
    /// ★ 它单独是一张 layered 窗，才有资格持有与标题栏不同的 alpha——见模块头。
    body: HWND,
    edit: HWND,
    /// 「透明度」拖动条弹窗句柄（`None` = 没开着）。再按一次按钮即 toggle 关。
    popup: Option<HWND>,
    cfg: NoteCfg,
    /// 背景画刷（`WM_CTLCOLOREDIT` 返回用）。**只建一次、复用**：每次 repaint 新建再丢
    /// 就是 GDI 画刷泄漏（打字会越打越积，最终编辑区失配、卡到不能打字）。`WM_NCDESTROY`
    /// 里 `DeleteObject` 释放。
    bg_brush: HBRUSH,
    /// 编辑区画刷（`WM_CTLCOLOREDIT` 返回）。锁定与否**同一支**——用户定案：锁定
    /// 不改任何颜色，只锁交互。
    edit_brush: HBRUSH,
    /// EDIT 的字号字体（HFONT）。`apply_look` 换字号时 DeleteObject 旧再换新；
    /// `WM_NCDESTROY` 删最后一份。**不记这份就是 GDI 句柄泄露**（Ctrl+滚轮每格一个）。
    edit_font: HFONT,
    /// 当前鼠标悬停的标题栏按钮（`None` = 没悬在任何按钮上）。只用于画悬停底纹，
    /// 变化时才 `InvalidateRect` 标题栏那条，避免每帧重画。
    hover: Option<u8>,
    flags: u8,
    /// 收起前的客户区高度（展开时还原）。
    normal_h: i32,
}

/// 原生窗管理器：拥有并驱动 pump 线程（见模块文档）。
pub struct StickyManager {
    thread: Option<JoinHandle<()>>,
    tx: Option<Sender<Cmd>>,
    cfg: NoteCfg,
}

impl Default for StickyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StickyManager {
    pub fn new() -> Self {
        Self {
            thread: None,
            tx: None,
            cfg: NoteCfg::default(),
        }
    }

    /// 确保 pump 线程已启动（幂等：已起则什么都不做）。**不负责开窗**——开窗由
    /// [`StickyManager::spawn`] 单独发 `Cmd::Spawn`，避免和快捷键自身的一次 spawn 撞出两张。
    pub fn ensure(&mut self, cfg: &NoteCfg) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel::<Cmd>();
        let Ok(handle) = thread::Builder::new()
            .name("curseen-sticky".into())
            .spawn(move || pump(rx))
        else {
            // 线程创建失败（资源耗尽等极端情况）：不 panic，置空通道留待下次
            // `ensure` 重试；代价只是本次便签功能暂不可用，不能带崩常驻进程。
            log::warn!("便签泵线程启动失败，本轮便签功能不可用");
            return;
        };
        self.tx = Some(tx);
        self.thread = Some(handle);
        self.cfg = cfg.clone();
    }

    /// 新增一张便签（`+` 或快捷键）。
    pub fn spawn(&self, cfg: &NoteCfg) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::Spawn(cfg.clone()));
        }
    }

    /// 配置变更时广播给所有便签。
    pub fn apply(&mut self, cfg: &NoteCfg) {
        if cfg != &self.cfg {
            self.cfg = cfg.clone();
            if let Some(tx) = &self.tx {
                let _ = tx.send(Cmd::Apply(cfg.clone()));
            }
        }
    }

    /// 结束 pump 线程（幂等）。
    /// 关掉全部已开的便签，**但让 pump 线程继续活着**（功能开关用；重开勾选还能再呼出）。
    ///
    /// ★ 不在调用方线程直接 `DestroyWindow`：便签窗是**别的线程**建的（`spawn_note`
    ///   在 pump 线程里跑），跨线程销毁要靠消息，命令队列正是为此存在的。
    /// 不为"关开关"这件事**起** pump 线程：还没起过 ⇒ 一张便签都没有 ⇒ 无事可做。
    pub fn close_all_notes(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Cmd::CloseAll);
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Cmd::Shutdown);
        }
        if let Some(th) = self.thread.take() {
            let _ = th.join();
        }
        if let Ok(mut g) = WINDOWS.lock() {
            g.clear();
        }
    }
}

impl Drop for StickyManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// pump 线程主体：注册类 → 处理命令 → 轮询泵消息。
fn pump(rx: Receiver<Cmd>) {
    register_class();
    let mut quit = false;
    // 常驻一条便签（快捷键首开即此）：避免"按一次快捷键一定有一张"。
    while !quit {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Spawn(cfg) => {
                    spawn_note(&cfg);
                }
                Cmd::Apply(cfg) => {
                    let guards = snapshot_windows();
                    for h in guards {
                        apply_to(h, &cfg);
                    }
                }
                Cmd::CloseAll => {
                    for h in snapshot_windows() {
                        unsafe {
                            let _ = DestroyWindow(h);
                        }
                    }
                }
                Cmd::Shutdown => {
                    quit = true;
                }
            }
        }
        if quit {
            break;
        }
        unsafe {
            let mut msg = MSG::default();
            let got = PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE);
            if got.as_bool() {
                if msg.message == windows::Win32::UI::WindowsAndMessaging::WM_QUIT {
                    break;
                }
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    // 收尾：销毁全部便签。
    for h in snapshot_windows() {
        unsafe {
            let _ = DestroyWindow(h);
        }
    }
}

/// 现存便签句柄快照。
fn snapshot_windows() -> Vec<HWND> {
    if let Ok(mut g) = WINDOWS.lock() {
        g.retain(|addr| {
            let h = HWND(*addr as *mut _);
            unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(Some(h)).as_bool() }
        });
        g.iter().map(|&addr| HWND(addr as *mut _)).collect()
    } else {
        Vec::new()
    }
}

fn register_lock(h: HWND) {
    if let Ok(mut g) = WINDOWS.lock() {
        if !g.contains(&(h.0 as isize)) {
            g.push(h.0 as isize);
        }
    }
}
fn unregister_lock(h: HWND) {
    if let Ok(mut g) = WINDOWS.lock() {
        g.retain(|&x| x != h.0 as isize);
    }
}

fn register_wnd_class(name: &str, proc: WNDPROC) {
    let class: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
        lpfnWndProc: proc,
        lpszClassName: PCWSTR(class.as_ptr()),
        ..Default::default()
    };
    unsafe {
        let _ = RegisterClassExW(&wc);
    }
}

/// 注册便签用到的三个窗口类（进程内一次，重复注册无害）：标题栏窗 / 正文窗 / 透明度弹窗。
fn register_class() {
    register_wnd_class(CLASS_NAME, Some(note_wndproc));
    register_wnd_class(BODY_CLASS, Some(body_wndproc));
    register_wnd_class(ALPHA_CLASS, Some(alpha_popup_wndproc));
}

/// 建一张便签（**两**张窗：标题栏窗 + 正文窗）。失败返回 None（调用方忽略）。
fn spawn_note(cfg: &NoteCfg) -> Option<HWND> {
    register_class();
    let class: Vec<u16> = CLASS_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let body_cls: Vec<u16> = BODY_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let title: Vec<u16> = NOTE_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let edit_cls: Vec<u16> = "EDIT\0".encode_utf16().collect();
    let (sw, sh) = {
        let (_x, _y, rw, rh) = win_util::virtual_screen_rect();
        (rw.max(1), rh.max(1))
    };
    let (x, y) = ((sw - DEFAULT_W) / 2, (sh - DEFAULT_H) / 2);

    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE((WS_EX_TOOLWINDOW | WS_EX_LAYERED).0),
            PCWSTR(class.as_ptr()),
            PCWSTR(title.as_ptr()),
            // 纯 `WS_POPUP`，**绝不**加 WS_CAPTION/WS_THICKFRAME 之类的原生 chrome：
            //   那些会让 DWM 渲染出原生标题条 / 分割线 / 边缘悬停残留（用户看到"标题栏
            //   没遮挡干净"的元凶）。缩放与拖动完全由 `WM_NCHITTEST` 手动命中接管
            //   （HTLEFT/HTTOP/…、HTCAPTION），与 WeNote 的 clsWindow 同款做法——
            //   返回 HT* 后系统自动跑 resize/move 模态循环，无需 WS_SIZEBOX。
            WINDOW_STYLE(WS_POPUP.0 | WS_CLIPCHILDREN.0),
            x,
            y,
            DEFAULT_W,
            DEFAULT_H,
            None,
            None,
            None,
            None,
        )
        .ok()?;

        // ── 正文窗：**独立顶层窗**（owner = 标题栏窗），自己也是一张 layered 窗。
        //   ★ 为什么要单独一张：标题栏要能比正文更透（或反之），而 `LWA_ALPHA` 一张窗
        //     只发一个 alpha。两窗的 hazards 是位置要自己同步（见 `layout_body`）。
        //   owner 关系顺带解决两件事：恒在 owner 之上、owner 销毁时自动跟着销毁。
        let body = CreateWindowExW(
            WINDOW_EX_STYLE((WS_EX_TOOLWINDOW | WS_EX_LAYERED).0),
            PCWSTR(body_cls.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0 | WS_CLIPCHILDREN.0),
            x + MARGIN,
            y + TITLE_H,
            (DEFAULT_W - 2 * MARGIN).max(1),
            (DEFAULT_H - TITLE_H - MARGIN).max(1),
            Some(hwnd),
            None,
            None,
            None,
        )
        .ok()?;

        // EDIT 子控件（多行、自动换行、回车换行）—— 现在挂在**正文窗**下。
        // ★ 不带 `WS_VSCROLL`：那是"强制常显滚动条"。`ES_MULTILINE | ES_AUTOVSCROLL`
        //   是"内容超出一屏才出现滚动条 + 自动下滚"，正是用户要求的第三种行为。
        let edit_style = WINDOW_STYLE(
            WS_CHILD.0
                | WS_VISIBLE.0
                | WS_CLIPSIBLINGS.0
                | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | ES_LEFT) as u32,
        );
        let edit = CreateWindowExW(
            Default::default(),
            PCWSTR(edit_cls.as_ptr()),
            PCWSTR::null(),
            edit_style,
            0,
            0,
            (DEFAULT_W - 2 * MARGIN).max(1),
            (DEFAULT_H - TITLE_H - MARGIN).max(1),
            Some(body),
            None,
            None,
            None,
        )
        .ok()?;

        // 字号字体（EDIT 初始字体）。★ 挂进 WinState 管生命周期：apply_look 换字号时
        // DeleteObject 旧的、WM_NCDESTROY 删最后的——否则 Ctrl+滚轮每调一格泄一个
        // HFONT（GDI 句柄进程级，泄露审计 2026-09-30）。
        let font = make_font(cfg.font_size);
        let st = Box::new(WinState {
            body,
            edit,
            popup: None,
            cfg: cfg.clone(),
            bg_brush: CreateSolidBrush(win_util::colorref(cfg.bg)),
            edit_brush: CreateSolidBrush(win_util::colorref(cfg.edit)),
            edit_font: font,
            hover: None,
            flags: FLG_TOPMOST,
            normal_h: DEFAULT_H,
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(st) as isize);
        SetWindowLongPtrW(
            body,
            GWLP_USERDATA,
            Box::into_raw(Box::new(BodyState { frame: hwnd, edit })) as isize,
        );
        register_lock(hwnd);

        // ★ EDIT 子类化：Ctrl+滚轮（改字号）必须自己转发给正文窗。EDIT 会把滚轮当滚动
        //   吃掉，而它的母公司现在是正文窗（顶层），再没有 DefWindowProc 替我们往上层转。
        //   原 proc 存在 EDIT 的 `GWLP_USERDATA` 里——系统 EDIT 控件不用这一段。
        let prev_proc = SetWindowLongPtrW(
            edit,
            GWLP_WNDPROC,
            edit_wndproc as *const () as usize as isize,
        );
        SetWindowLongPtrW(edit, GWLP_USERDATA, prev_proc);

        // EDIT 初始字体已在上面建好并挂进 WinState（edit_font），这里只下发。
        let _ = SendMessageW(
            edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        let _ = SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        sync_topmost(hwnd);
        // 两个 alpha 一起按当前配置刷（新开窗直接用 `NoteCfg` 的 alpha）。
        // ★ 先把窗口区域裁掉正文内区再刷 alpha：不裁的话标题栏窗的层 alpha
        //   会透过它的未绘区域漏进正文（见 `update_frame_region`）。
        update_frame_region(hwnd);
        layout_body(hwnd);
        apply_note_alpha(hwnd);
        let _ = InvalidateRect(Some(hwnd), None, true);
        let _ = ShowWindow(body, SW_SHOW);
        let _ = ShowWindow(hwnd, SW_SHOW);
        // 聚焦：先把整个窗带前台（后台进程直接 SetFocus 会被拒），再**真的**把焦点给
        // 到 EDIT。
        //
        // ★★★ `SendMessageW(edit, WM_SETFOCUS)` 是**伪造焦点** —— 它只让 EDIT 画个光标，
        //      `GetFocus()` 仍然停在母窗上。键盘消息因此全发给母窗（它的 DefWindowProc
        //      不处理字符），表现就是"文本框看得见、打不进字"（真机提权探针实测：
        //      `GetGUIThreadInfo(note线程).hwndFocus` = 母窗 hwnd，不是 EDIT）。
        //      焦点必须是 SetFocus 真给。
        win_util::focus_window_handle(hwnd);
        let _ = SetFocus(Some(edit));
        Some(hwnd)
    }
}

/// 正文窗的运行态（挂在正文窗的 `GWLP_USERDATA` 上）。
struct BodyState {
    /// 归属的标题栏窗（owner）。
    frame: HWND,
    edit: HWND,
}

/// 取正文窗状态指针（pump 线程内）。
fn body_state(h: HWND) -> Option<*mut BodyState> {
    unsafe {
        let p = GetWindowLongPtrW(h, GWLP_USERDATA);
        if p == 0 {
            None
        } else {
            Some(p as *mut BodyState)
        }
    }
}

/// 改**这一张**便签的总不透明度：只动它自己的配置镜像与两层的 alpha，**从不回写配置**。
///
/// ★ 用户口径（2026-09-27）：标题栏弹出来的那个条只改当前便签；面板里「透明度」
///   是出厂默认值，两者不是一个东西。写回去等于"拖一张便签把全局默认也改了"。
fn apply_total_alpha(hwnd: HWND, v: u32) {
    let Some(p) = state(hwnd) else { return };
    let v = v.clamp(NOTE_ALPHA_MIN, NOTE_ALPHA_MAX);
    let popup = {
        let st = unsafe { &mut *p };
        if st.cfg.alpha == v {
            return;
        }
        st.cfg.alpha = v;
        st.popup
    };
    apply_note_alpha(hwnd);
    if let Some(h) = popup {
        let _ = unsafe { InvalidateRect(Some(h), None, true) };
    }
}

/// Ctrl+滚轮改字号（正文窗 / EDIT 转发过来）。
fn bump_font(frame: HWND, delta: f32) {
    let Some(p) = state(frame) else { return };
    if unsafe { (*p).flags } & FLG_LOCKED != 0 {
        return; // 锁定 = 字号也锁（旧口径：透明度/字号都属"其它状态"）。
    }
    let nf = unsafe { ((*p).cfg.font_size + delta).clamp(NOTE_FONT_MIN, NOTE_FONT_MAX) };
    unsafe { (*p).cfg.font_size = nf };
    apply_look(frame, None);
}

/// 建便签文本字体（HFONT）。
/// 建雅黑字体（负 em 高度 + CLEARTYPE）。`pub`：AI 对话窗同款口径复用（字号 14=面板 FONT_MD）。
pub fn make_font(size: f32) -> HFONT {
    let face = HSTRING::from("Microsoft YaHei UI");
    unsafe {
        CreateFontW(
            -((size.round() as i32).clamp(9, 72)),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            &face,
        )
    }
}

/// 标题栏按钮的**状态符号**（Segoe UI Symbol 的单色字形，随 `SetTextColor` 着色，
/// 字体渲染自带抗锯齿，比手绘 GDI 多边形干净得多——用户定案）：
/// 📌 置顶 ／ 🔒已锁·🔓未锁 ／ ▼收起·▲展开。
fn btn_glyph(flags: u8, which: u8) -> char {
    match which {
        BTN_TOP => '\u{1F4CC}',
        // ◐ 半明半暗的圆 —— 一看就是"透明度"，且这个字形在 Segoe UI Symbol 里是
        //   **单色**的（跟 SetTextColor 走），不会被回退成彩色 emoji。
        BTN_ALPHA => '\u{25D0}',
        BTN_LOCK => {
            if flags & FLG_LOCKED != 0 {
                '\u{1F512}'
            } else {
                '\u{1F513}'
            }
        }
        BTN_COLLAPSE => {
            if flags & FLG_COLLAPSED != 0 {
                '\u{25B2}'
            } else {
                '\u{25BC}'
            }
        }
        _ => '\u{FF0B}',
    }
}

/// 符号字体：**Segoe UI Symbol**（📌 🔒 🔓 ▲ ▼ 在此字体里都是单色轮廓字形，
/// 不会被回退成彩色 emoji，颜色完全跟 `SetTextColor` 走）。
fn symbol_font(px: i32) -> HFONT {
    let face = HSTRING::from("Segoe UI Symbol");
    unsafe {
        CreateFontW(
            -px.clamp(9, 64),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_TT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (DEFAULT_PITCH.0 | FF_DONTCARE.0) as u32,
            &face,
        )
    }
}

/// 取窗口状态指针（pump 线程内）。
fn state(h: HWND) -> Option<*mut WinState> {
    unsafe {
        let p = GetWindowLongPtrW(h, GWLP_USERDATA);
        if p == 0 {
            None
        } else {
            Some(p as *mut WinState)
        }
    }
}

/// ★★ **最终不透明度 = 总不透明度 × 颜色自身的不透明度**（用户口径
/// 2026-09-27），换算成 `SetLayeredWindowAttributes` 要的 0–255。
///
/// ```text
/// alpha = round( total_pct/100 \u{00d7} color_a/255 \u{00d7} 255 )
/// ```
///
/// 两个因子都是**百分比**:`total_pct` 是标题栏拖动条那个 10–100，`color_a` 是取色器里
/// 颜色自身的 alpha（0–255）。两者独立存在，最终相乘 —— 所以"选项里的不透明度"和
/// "颜色选择器的透明度"必须**同时有效**，缺一个就退化成旧 bug（设了跟没设一样）。
///
/// 下限取 1 而不是 0：`LWA_ALPHA = 0` 的窗是一张什么都看不见的黑箱，用户会以为便签
/// 崩了；留 1/255，至少还能看见位置（这与 `set_window_alpha` 一侧的历史惯例一致）。
pub fn layered_alpha(total_pct: u32, color_a: u8) -> u8 {
    let total = (total_pct.clamp(NOTE_ALPHA_MIN, NOTE_ALPHA_MAX) as f32) / 100.0;
    let a = (color_a as f32) / 255.0;
    let raw = (total * a * 255.0).round() as i32;
    raw.clamp(1, 255) as u8
}

/// 前景（`fg`，含自己的 alpha）画在不透光的底色 `bg` 上的**最终 RGB**。
///
/// ★ GDI 不认 alpha：`COLORREF` 里就 3 个字节，`SetTextColor` 给的第 4 个字节直接被丢。
///   好在 layer 内部本来就不透光，把"半透明的 fg"按 alpha 预混进它下面的底色再交给 GDI，
///   结果和"fg 自己半透明合成上去"**完全等价**（不是近似）——见模块头的表格。
fn over(bg: [u8; 4], fg: [u8; 4]) -> [u8; 4] {
    mix_color(bg, fg, (fg[3] as f32) / 255.0)
}

/// 延迟聚焦消息（`WM_APP` 段私有）：激活时**不**同步聚焦，见 `WM_ACTIVATE` 处的注释。
const WM_APP_NOTE_FOCUS: u32 = WM_APP + 0x4E31;

/// 当前线程是否正处系统的移动/缩放模态循环（`GUI_INMOVESIZE` / `GUI_INSIZE`）。
/// 延迟聚焦在这两种状态下必须让路：抢焦 = 反激活 = 循环中途夭折（"拖一半掉了"）。
fn thread_in_modal_loop() -> bool {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetGUIThreadInfo(GetCurrentThreadId(), &mut info) };
    ok.is_ok() && (info.flags & GUITHREADINFO_FLAGS(GUI_INMOVESIZE.0)) != GUITHREADINFO_FLAGS(0)
}

/// 把标题栏窗的**窗口区域**裁成"标题条 + 边缘环"：从全矩形挖掉正文内区。
///
/// ★★★ 这是"背景色 alpha 一调，整个窗口都跟着变"的修法。探针实证：正文窗垫在
///   标题栏窗上，而标题栏窗的正文区域虽然没画任何东西，GDI 未绘区域**并不透明**
///   —— 正文窗半透明合成时透出的正是它，于是背景色的层 alpha 漏进了正文区。
///   `SetWindowRgn` 之后这块区域从窗口上物理消失：不渲染、不挡鼠标、层 alpha
///   管不到它，两窗的透明度彻底解耦。
///
/// 系统接管区域对象（无需 DeleteObject）；`WM_SIZE` 与开窗时都要重算。
fn update_frame_region(hwnd: HWND) {
    let mut rc = RECT::default();
    if unsafe { GetClientRect(hwnd, &mut rc) }.is_err() {
        return;
    }
    let (w, h) = (rc.right.max(1), rc.bottom.max(1));
    let full = unsafe { CreateRectRgn(0, 0, w, h) };
    // 展开态才挖得动：挖掉的正文内区 = 正文窗的客户区占位（MARGIN=1 的环留作边）。
    if h > TITLE_H + MARGIN && w > 2 * MARGIN {
        let inner = unsafe { CreateRectRgn(MARGIN, TITLE_H, w - MARGIN, h - MARGIN) };
        unsafe {
            let _ = CombineRgn(Some(full), Some(full), Some(inner), RGN_DIFF);
            let _ = DeleteObject(HGDIOBJ(inner.0));
        }
    }
    unsafe {
        let _ = SetWindowRgn(hwnd, Some(full), true);
    }
}

/// 把标题栏窗与正文窗的 alpha 一起按当前配置刷一遍。
///
/// ★ 两窗各自的系数不同（背景色 alpha vs 编辑区色 alpha），所以必须**成对**刷新：
///   只刷一张就会在同一张便签上出现"标题栏比正文更透"这种没人要的效果。
fn apply_note_alpha(hwnd: HWND) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &*p };
    let _ = win_util::set_window_alpha(hwnd.0 as isize, layered_alpha(st.cfg.alpha, st.cfg.bg[3]));
    if !st.body.0.is_null() {
        let _ = win_util::set_window_alpha(
            st.body.0 as isize,
            layered_alpha(st.cfg.alpha, st.cfg.edit[3]),
        );
    }
}

/// 应用外观（字号字体 / 触发重绘）。
fn apply_look(hwnd: HWND, cfg: Option<&NoteCfg>) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &mut *p };
    if let Some(c) = cfg {
        // 背景色变了 → 重建缓存的背景画刷，EDIT 底色才会跟着配置走。
        if c.bg != st.cfg.bg {
            let nb = unsafe { CreateSolidBrush(win_util::colorref(c.bg)) };
            let old = std::mem::replace(&mut st.bg_brush, nb);
            let _ = unsafe { DeleteObject(old.into()) };
        }
        if c.edit != st.cfg.edit {
            let nb = unsafe { CreateSolidBrush(win_util::colorref(c.edit)) };
            let old = std::mem::replace(&mut st.edit_brush, nb);
            let _ = unsafe { DeleteObject(old.into()) };
        }
        st.cfg = c.clone();
    }
    // 字号字体：新建 → 下发 → 删旧的（EDIT 已接手新字体；旧的不删就是 HFONT 泄露）。
    let font = make_font(st.cfg.font_size);
    unsafe {
        let _ = SendMessageW(
            st.edit,
            WM_SETFONT,
            Some(WPARAM(font.0 as usize)),
            Some(LPARAM(1)),
        );
        let old = std::mem::replace(&mut st.edit_font, font);
        let _ = DeleteObject(old.into());
        // 触发标题栏与 EDIT 重绘（文字色/背景色由 WM_CTLCOLOREDIT 提供）。
        let _ = InvalidateRect(Some(hwnd), None, true);
        let _ = InvalidateRect(Some(st.edit), None, true);
    }
}

/// 把标题栏窗的置顶状态**同步给正文窗**。
///
/// ★ owner 关系只保证"正文窗恒在标题栏窗之上"，**不能**替它决定是不是 topmost
///   （它是独立顶层窗，有自己的 z 序）。不同步就会出现"标题栏在最上层、正文被别的
///   窗口盖住"——这正是把一扇窗拆成两扇要还的债。
fn sync_topmost(hwnd: HWND) {
    let Some(p) = state(hwnd) else {
        return;
    };
    let st = unsafe { &*p };
    if st.body.0.is_null() {
        return;
    }
    let _ = unsafe {
        SetWindowPos(
            st.body,
            Some(if st.flags & FLG_TOPMOST != 0 {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            }),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
}

fn apply_to(hwnd: HWND, cfg: &NoteCfg) {
    apply_look(hwnd, Some(cfg));
    // ★ 两张窗的 alpha 都要按新配置刷：颜色 alpha 也可能刚变过（[`apply_note_alpha`]
    //   读的是已写进 `WinState` 的镜像，所以顺序必须是"先 look 后 alpha"）。
    apply_note_alpha(hwnd);
}

/// 标题栏按钮矩形（按客户区宽度，右侧四个从右往左排）。
fn btn_rect(cw: i32, which: u8) -> RECT {
    // ★ 按钮占满标题栏整个高度（用户定案：上下不留间隔），悬停底纹也是整条。
    let (ty, ty_b) = (0, TITLE_H);
    match which {
        BTN_ADD => RECT {
            left: MARGIN,
            top: ty,
            right: MARGIN + BTN_W,
            bottom: ty_b,
        },
        _ => {
            let idx = match which {
                BTN_CLOSE => 0,
                BTN_COLLAPSE => 1,
                BTN_LOCK => 2,
                BTN_TOP => 3,
                BTN_ALPHA => 4,
                _ => 0,
            };
            let r = cw - MARGIN - idx * (BTN_W + BTN_GAP);
            RECT {
                left: r - BTN_W,
                top: ty,
                right: r,
                bottom: ty_b,
            }
        }
    }
}

fn point_in(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

/// 标题栏按钮命中。
fn hit_btn(cw: i32, x: i32, y: i32) -> Option<u8> {
    if y >= TITLE_H {
        return None;
    }
    [
        BTN_ADD,
        BTN_CLOSE,
        BTN_COLLAPSE,
        BTN_LOCK,
        BTN_TOP,
        BTN_ALPHA,
    ]
    .into_iter()
    .find(|&b| point_in(btn_rect(cw, b), x, y))
}

/// 把 `a` 朝 `b` 混一点（`t`∈[0,1]，0=原色 1=全变成 `b`）。悬停底纹用。
fn mix_color(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let mut out = a;
    for i in 0..3 {
        let v = a[i] as f32 + (b[i] as f32 - a[i] as f32) * t.clamp(0.0, 1.0);
        out[i] = v.clamp(0.0, 255.0) as u8;
    }
    out
}

/// 标题栏 ＋ / × 的矢量绘制（其余按钮走 Segoe UI Symbol 符号字形，见
/// [`btn_glyph`]）。
///
/// ★★★ 几何铁律（用户逐条挑刺后定案）：
///   ① **横平竖直的笔画一律 `FillRect` + 显式矩形，不用 `LineTo`**——GDI 宽笔的端点
///      会向右/下多铺一像素，笔画两端不对称。`RECT` 左闭右开：盖住 `[cx-a, cx+a]`
///      得写 `left=cx-a, right=cx+a+1`；笔画本身偏在中心一格时两端再各让 1px。
///   ② **斜线只允许 45°**（dx == dy），像素阶梯最干净、无锯齿。
///
fn draw_icon(dc: HDC, r: RECT, which: u8, flags: u8, bg: [u8; 4], fg: [u8; 4]) {
    let active = btn_active(flags, which);
    let rgb = win_util::colorref(if active { fg } else { mix_color(bg, fg, 0.35) });
    let cx = (r.left + r.right) / 2;
    let cy = (r.top + r.bottom) / 2;
    unsafe {
        let solid = CreateSolidBrush(rgb);
        let pen1 = CreatePen(PS_SOLID, 1, rgb);
        let pen2 = CreatePen(PS_SOLID, 2, rgb);
        let old_pen = SelectObject(dc, HGDIOBJ(pen2.0));
        let old_brush = SelectObject(dc, HGDIOBJ(solid.0));
        // 实心笔画：盖住像素 [l, rt-1] × [t, b-1]。右边界 +1 才能左右对称。
        let bar = |l: i32, t: i32, rt: i32, b: i32| {
            let rc = RECT {
                left: l,
                top: t,
                right: rt,
                bottom: b,
            };
            FillRect(dc, &rc, solid);
        };
        match which {
            BTN_ADD => {
                // ＋：宽 2 的横竖笔画。★★ 笔画本身偏在中心左/上一格
                // （RECT 左闭右开），所以两端各多伸出 1px 抵消——左臂/右臂、
                // 上臂/下臂的可见长度才相等（上一轮"左边和上边短"就是它）。
                bar(cx - 7, cy - 1, cx + 7, cy + 1);
                bar(cx - 1, cy - 7, cx + 1, cy + 7);
            }
            BTN_CLOSE => {
                // ×：两条 45° 斜线。
                let _ = MoveToEx(dc, cx - 6, cy - 6, None);
                let _ = LineTo(dc, cx + 6, cy + 6);
                let _ = MoveToEx(dc, cx + 6, cy - 6, None);
                let _ = LineTo(dc, cx - 6, cy + 6);
            }
            _ => {}
        }
        let _ = SelectObject(dc, old_pen);
        let _ = SelectObject(dc, old_brush);
        let _ = DeleteObject(pen1.into());
        let _ = DeleteObject(pen2.into());
        let _ = DeleteObject(solid.into());
    }
}

/// 按钮当前是否可用。★★ 锁定 = 除 `+`（新增）和锁定本身（解锁入口）外全部禁用：
/// 不能拖动、不能改置顶、不能收起、不能关闭（`WM_CLOSE` 也直接吞掉）。
fn btn_active(flags: u8, which: u8) -> bool {
    if flags & FLG_LOCKED == 0 {
        return true;
    }
    matches!(which, BTN_ADD | BTN_LOCK)
}

/// 标题栏窗**允许上色**的矩形：标题栏那一条 + 左/右/底三条 1px 边环。
///
/// ★★★ 正文区**整块不在其中** —— 那里归正文窗（[`BODY_CLASS`]）。标题栏窗一旦在正文区
///   也铺一层底色，正文层半透明时它就会透上来，配出来的颜色比配置深一档
///   （"明明设的是一个色，看着像两个"），而且**不报错**。
/// ★ 它同时是单测的抓手："标题栏窗究竟有没有僭越到正文层"这件事只能算，不能看。
fn frame_paint_rects(cw: i32, ch: i32) -> Vec<RECT> {
    // 收起态：整窗就剩标题栏那一条，画边环没意义（还会画到窗外）。
    if ch <= TITLE_H {
        return vec![RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: ch,
        }];
    }
    vec![
        // 标题栏整条。
        RECT {
            left: 0,
            top: 0,
            right: cw,
            bottom: TITLE_H,
        },
        // 左边距 1px。
        RECT {
            left: 0,
            top: TITLE_H,
            right: MARGIN,
            bottom: ch,
        },
        // 右边距 1px。
        RECT {
            left: (cw - MARGIN).max(0),
            top: TITLE_H,
            right: cw,
            bottom: ch,
        },
        // 底边距 1px。
        RECT {
            left: 0,
            top: (ch - MARGIN).max(0),
            right: cw,
            bottom: ch,
        },
    ]
}

/// 标题栏：只在 [`frame_paint_rects`] 给出的范围里铺底，再逐个画按钮图标（见 [`draw_icon`]）。
///
/// ★★★ 以前是**整张客户区先铺一遍底色**，现在不行了：正文是独立 layered 窗，标题栏窗
///   在那块区域上色就会与它叠一遍（两张窗各自半透明 → 拿出双重底色）。
///   现在只上"标题栏 + 1px 边环"，没上色的地方仍然是"透明黑"（layered 窗的本性），
///   不会透出桌面，也不会抢正文的地盘。
fn draw_titlebar(hwnd: HWND) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &*p };
    let dc = unsafe { GetDC(Some(hwnd)) };
    if dc.is_invalid() {
        return;
    }
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_ok() {
            let cw = rc.right;
            let bg = win_util::colorref(st.cfg.bg);
            // 用缓存的那支背景画刷（`apply_look` 在颜色变化时重建）：每次 repaint 新建
            // 再丢就是 GDI 画刷泄漏（打字时越积越多）。
            for r in frame_paint_rects(cw, rc.bottom) {
                FillRect(dc, &r, st.bg_brush);
            }
            let _ = SetBkColor(dc, bg);
            let _ = SetBkMode(dc, TRANSPARENT);
            // ★ 图标 / 文字的**最终色**：把文字色自带的 alpha 预混进底色（见 [`over`]——
            //   层内不透光，这样处理与"文字独立半透明"等价）。不这么做，取色器里调文字
            //   透明度依旧是"设了跟没设一个样"（GDI 直接丢掉 COLORREF 的第 4 个字节）。
            let ink = over(st.cfg.bg, st.cfg.text);
            // 每按钮一个**矢量状态图标**（悬停底纹 = 整个按钮矩形，按钮已占满栏高）。
            for which in [
                BTN_ADD,
                BTN_ALPHA,
                BTN_TOP,
                BTN_LOCK,
                BTN_COLLAPSE,
                BTN_CLOSE,
            ] {
                let mut r = btn_rect(cw, which);
                if btn_active(st.flags, which) && st.hover == Some(which) {
                    let hb = CreateSolidBrush(win_util::colorref(mix_color(st.cfg.bg, ink, 0.18)));
                    FillRect(dc, &r, hb);
                    let _ = DeleteObject(hb.into());
                }
                if matches!(which, BTN_ADD | BTN_CLOSE) {
                    // ＋ / ×：矢量（两条笔画，横平竖直，手绘足够干净）。
                    draw_icon(dc, r, which, st.flags, st.cfg.bg, ink);
                } else {
                    // 其余：现成符号字形。
                    let g = btn_glyph(st.flags, which);
                    // ★ 置顶状态靠**图钉姿态**区分。\u{1f4cc} 字形**天生斜躺**（针头朝左下）：
                    //   未置顶 = 原生姿态直接画；已置顶 = 绕**按钮中心**视觉逆时针转
                    //   45°，针头立直朝下。不用 lfEscapement——DrawTextW 布局按未旋转
                    //   度量计算，旋转字形会整体漂出按钮；世界变换绕中心转，零漂移。
                    let tilted = which == BTN_TOP && st.flags & FLG_TOPMOST != 0;
                    let sym = symbol_font(17);
                    let sym_obj = HGDIOBJ(sym.0);
                    let oldf = SelectObject(dc, sym_obj);
                    let _ = SetTextColor(
                        dc,
                        win_util::colorref(if btn_active(st.flags, which) {
                            ink
                        } else {
                            mix_color(st.cfg.bg, ink, 0.35)
                        }),
                    );
                    let mut buf = [0u16; 2];
                    let n = g.encode_utf16(&mut buf).len();
                    if tilted {
                        // ★ 立正姿态的视觉重心比 DT_VCENTER 的排版中心偏下 3px
                        //   （字形斜边质量分布所致，用户实测），旋转前先上移。
                        r.top -= 3;
                        r.bottom -= 3;
                        // 绕 (mx, my) 做**视觉逆时针** 45° 旋转：M = T·R·T⁻¹
                        // （y 向下坐标系：eM12 取负才是视觉逆时针；顺时针那版会把
                        // 针头从左下转到正左，用户实测）。
                        let f = std::f32::consts::FRAC_1_SQRT_2;
                        let (fx, fy) = (r.left as f32, r.top as f32);
                        let (fw, fh) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
                        let (mx, my) = (fx + fw / 2.0, fy + fh / 2.0);
                        let xf = XFORM {
                            eM11: f,
                            eM12: -f,
                            eM21: f,
                            eM22: f,
                            eDx: mx - f * mx - f * my,
                            eDy: my + f * mx - f * my,
                        };
                        SetGraphicsMode(dc, GM_ADVANCED);
                        let _ = SetWorldTransform(dc, &xf);
                        let _ = DrawTextW(
                            dc,
                            &mut buf[..n],
                            &mut r,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                        let identity = XFORM {
                            eM11: 1.0,
                            eM12: 0.0,
                            eM21: 0.0,
                            eM22: 1.0,
                            eDx: 0.0,
                            eDy: 0.0,
                        };
                        let _ = SetWorldTransform(dc, &identity);
                        SetGraphicsMode(dc, GM_COMPATIBLE);
                    } else {
                        let _ = DrawTextW(
                            dc,
                            &mut buf[..n],
                            &mut r,
                            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                        );
                    }
                    let _ = SelectObject(dc, oldf);
                    let _ = DeleteObject(sym_obj);
                }
            }
        }
    }
    unsafe {
        ReleaseDC(Some(hwnd), dc);
    }
}

/// 把键盘焦点交给 EDIT 子控件。
///
/// ★ 便签"能不能打字"全看焦点在不在这个 EDIT 上：键盘消息永远发给焦点窗，而母窗自己
///   不处理字符（`DefWindowProc` 也不会替子控件收字），焦点停在母窗 = 打不出第二个字。
///   这几条路径都会被激活（点标题栏按钮后焦点回到母窗、点子窗、窗口被激活……），
///   所以统一在这里补给 EDIT，别指望系统替我们记住。
fn focus_edit(hwnd: HWND) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &*p };
    unsafe {
        // 收起态 EDIT 被藏了，给它焦点会失败也没关系（返回值只用于日志）。
        let _ = SetFocus(Some(st.edit));
    }
}

/// 折叠这次该用的两个高度：**（要记住的高度，窗口的目标高度）**。
///
/// ★ 抽成纯函数只为把"展开必须回到折叠前的高度"钉进单测。真机踩过：折叠过程自己派发的
///   `WM_SIZE` 把记着的高度覆盖成 26，展开只能回到 `26.max(26+40) = 66`（220 → 26 → **66**）。
fn collapse_height(coll: bool, cur_h: i32, normal_h: i32) -> (i32, i32) {
    if coll {
        (cur_h.max(TITLE_H), TITLE_H)
    } else {
        (normal_h, normal_h.max(TITLE_H + 40))
    }
}

/// 收起 / 展开。
///
/// ★★★ 标志必须**在改尺寸之前**立好。`SetWindowPos` 会**同步**派发 `WM_SIZE`，
///   `reposition_edit` 正是靠 `FLG_COLLAPSED` 决定"要不要把 `normal_h` 记成当前高度"——
///   标志若晚一步（旧代码放在函数末尾），折叠时 `normal_h` 就被同步改成的 26 覆盖，
///   于是展开只能回到 `26.max(26+40) = 66`（真机实测 220 → 26 → **66**，回不去）。
fn set_collapsed(hwnd: HWND, coll: bool) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &mut *p };
    st.flags = (st.flags & !FLG_COLLAPSED) | if coll { FLG_COLLAPSED } else { 0 };
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_ok() {
            let cw = rc.right;
            let (remember, target) = collapse_height(coll, rc.bottom, st.normal_h);
            st.normal_h = remember;
            if coll {
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    cw,
                    target,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .ok();
                // ★ 收起 = 藏**正文窗**（它现在是独立顶层窗），EDIT 跟着它一起消失。
                let _ = ShowWindow(st.body, SW_HIDE);
            } else {
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    cw,
                    target,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                )
                .ok();
                let _ = ShowWindow(st.body, SW_SHOW);
            }
        }
    }
}

/// 客户区变化时记录"展开该用的高度"，并把正文窗贴到正文区上（见 [`layout_body`]）。
fn on_frame_resized(hwnd: HWND) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &mut *p };
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_ok() && st.flags & FLG_COLLAPSED == 0 {
            st.normal_h = rc.bottom.max(TITLE_H);
        }
    }
    // 尺寸变了 → 窗口区域跟着重挖（否则挖掉的还是旧矩形）。
    update_frame_region(hwnd);
    layout_body(hwnd);
}

/// 把正文窗贴到标题栏窗的正文区上（**屏幕坐标**）。
///
/// ★★★ 拆成两窗的代价就在这里：以前 EDIT 是子窗体，移动/缩放由系统代劳；现在正文是
///   独立顶层窗，必须自己跟上。`WM_WINDOWPOSCHANGED` 是唯一同时覆盖"移动 + 缩放 +
///   层级变化"的消息，所以只在那一处调用 —— 分开钩 `WM_MOVE`/`WM_SIZE` 早晚漏一条
///   （漏了就是"正文停在原地、标题栏跑了"）。
fn layout_body(hwnd: HWND) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &*p };
    if st.body.0.is_null() {
        return;
    }
    unsafe {
        let mut wr = RECT::default();
        if GetWindowRect(hwnd, &mut wr).is_err() {
            return;
        }
        // WS_POPUP 无边框：窗口矩形 == 客户区矩形，不需要 ClientToScreen。
        let x = wr.left + MARGIN;
        let y = wr.top + TITLE_H;
        let w = (wr.right - wr.left - 2 * MARGIN).max(1);
        let h = (wr.bottom - wr.top - TITLE_H - MARGIN).max(1);
        let _ = SetWindowPos(st.body, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
    }
}

/// 正文窗尺寸变化 → 里面的 EDIT 子控件铺满它。
fn layout_edit_in_body(body: HWND) {
    let Some(p) = body_state(body) else { return };
    let st = unsafe { &*p };
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(body, &mut rc).is_ok() {
            let _ = MoveWindow(
                st.edit,
                0,
                0,
                (rc.right - rc.left).max(1),
                (rc.bottom - rc.top).max(1),
                true,
            );
        }
    }
}

extern "system" fn note_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    unsafe {
        match msg {
            // 命中测试（WeNote/clsWindow 同款：完全自管，不借任何原生 chrome）：
            //   ・四边 7px 内返回 HTLEFT/HTTOP/… —— 系统据此自动跑 resize 模态循环，
            //     实现"拖边框线放大"，窗口本身仍是裸 WS_POPUP、无原生标题条；
            //   ・标题栏非按钮区返回 HTCAPTION —— 拖动移动；有按钮则进 HTCLIENT 走点击；
            //   ・正文区 HTCLIENT —— EDIT 子控件收鼠标，点进可打字（含中文 IME）。
            //   lParam 低/高 16 位各是一个**有符号**屏幕坐标（不能当 POINT 读，POINT 是
            //   32 位字段会把 x 吞进 y）。无边框 ⇒ 窗口矩形 == 客户区，坐标可直接用。
            WM_NCHITTEST => {
                const RB: i32 = 7; // 可拖边的命中宽度
                const HT_CLIENT: isize = 1;
                const HT_CAPTION: isize = 2;
                const HT_LEFT: isize = 10;
                const HT_RIGHT: isize = 11;
                const HT_TOP: isize = 12;
                const HT_TOPLEFT: isize = 13;
                const HT_TOPRIGHT: isize = 14;
                const HT_BOTTOM: isize = 15;
                const HT_BOTTOMLEFT: isize = 16;
                const HT_BOTTOMRIGHT: isize = 17;

                let (cx, cy) = lparam_xy(lparam); // 屏幕 x / y（有符号）
                let mut wr = RECT::default();
                let _ = GetWindowRect(hwnd, &mut wr);
                let x = cx - wr.left;
                let y = cy - wr.top;
                let w = (wr.right - wr.left).max(1);
                let h = (wr.bottom - wr.top).max(1);

                let l = x < RB;
                let r = x >= w - RB;
                let t = y < RB;
                // ★ 收起态（整窗只剩标题栏高）底部**不做**缩放边：否则 ▾ 按钮下半截
                //   全落在 HT_BOTTOM，点击变成拖高度，"展开"永远点不中（用户实测）。
                let b = y >= h - RB && h > TITLE_H;
                // ★ 锁定 = 位置也锁：全客户区 HTCLIENT，既不给 HTCAPTION（拖不动）
                //   也不给 HT*EDGE（缩不了）。EDIT 子控件在正文区照常收鼠标（只读）。
                let code = if state(hwnd).map(|p| (*p).flags & FLG_LOCKED != 0) == Some(true) {
                    HT_CLIENT
                } else if t && l {
                    HT_TOPLEFT
                } else if t && r {
                    HT_TOPRIGHT
                } else if b && l {
                    HT_BOTTOMLEFT
                } else if b && r {
                    HT_BOTTOMRIGHT
                } else if t {
                    HT_TOP
                } else if b {
                    HT_BOTTOM
                } else if l {
                    HT_LEFT
                } else if r {
                    HT_RIGHT
                } else if y < TITLE_H {
                    // 标题栏：非按钮区拖动，按钮区放行点按。
                    if hit_btn(w, x, y).is_none() {
                        HT_CAPTION
                    } else {
                        HT_CLIENT
                    }
                } else {
                    HT_CLIENT
                };
                LRESULT(code)
            }
            // 尺寸变化：记下"展开该用的高度"，并把正文窗重贴到新的正文区。
            WM_SIZE => {
                let _ = DefWindowProcW(hwnd, msg, wparam, lparam);
                on_frame_resized(hwnd);
                LRESULT(0)
            }
            // 位置 / 尺寸 / 层级任一变化 → 正文窗跟着走（见 `layout_body`：它是独立
            // 顶层窗，没有"子窗自动跟随"这回事）。
            WM_WINDOWPOSCHANGED => {
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                layout_body(hwnd);
                r
            }
            // 最小尺寸下限，防止把便签拖/缩到只剩一条缝。
            WM_GETMINMAXINFO => {
                let mm = &mut *(lparam.0 as *mut MINMAXINFO);
                mm.ptMinTrackSize = POINT {
                    x: 120,
                    y: (TITLE_H + 40),
                };
                LRESULT(0)
            }
            // 自绘标题栏。
            WM_PAINT => {
                let _ = DefWindowProcW(hwnd, msg, wparam, lparam);
                draw_titlebar(hwnd);
                LRESULT(0)
            }
            // 任何点击都激活窗口（MA_ACTIVATE=1）：否则后台线程窗口可能因前台锁一直
            // 不激活，键盘输入被吸去别的窗口 → "点进去也没法打字"。激活后点 EDIT 即聚焦。
            WM_MOUSEACTIVATE => LRESULT(1),
            // 按钮点击。
            //
            // ★★★ 坐标**必须**走 [`lparam_xy`]：就地写 `(lparam.0 as u16 as u32 >> 16)` 会
            //   因为 `as` 优先级高于 `>>` 而恒为 0，于是 `hit_btn` 永远比不到任何按钮
            //   （按钮矩形占满整条标题栏，y∈[0,TITLE_H)）—— 症状正是"标题栏只能拖，五个按钮全哑"。
            //   顺带：正文区按下时把焦点递给 EDIT，点空白也不至于把焦点丢回母窗。
            WM_LBUTTONDOWN | WM_LBUTTONUP => {
                let (cx, cy) = lparam_xy(lparam);
                if msg == WM_LBUTTONUP {
                    let mut rc = RECT::default();
                    if GetClientRect(hwnd, &mut rc).is_ok() {
                        if let Some(b) = hit_btn(rc.right, cx, cy) {
                            on_btn(hwnd, b);
                        }
                    }
                } else if cy >= TITLE_H {
                    focus_edit(hwnd);
                }
                LRESULT(0)
            }
            // 鼠标离开窗口：清 hover 底纹（快速划出时不会有后续 WM_MOUSEMOVE）。
            WM_MOUSELEAVE => {
                if let Some(p) = state(hwnd) {
                    let st = &mut *p;
                    if st.hover.take().is_some() {
                        let bar = RECT {
                            left: 0,
                            top: 0,
                            right: i32::MAX,
                            bottom: TITLE_H,
                        };
                        let _ = InvalidateRect(Some(hwnd), Some(&bar), false);
                    }
                }
                LRESULT(0)
            }
            // 悬停反馈：标题栏按钮画一层浅浅的底纹，告诉用户"这里能点"。
            // 只在**悬停目标变了**时才请求重画标题栏那条，Move 风暴下几乎零成本。
            WM_MOUSEMOVE => {
                let (cx, cy) = lparam_xy(lparam);
                let mut rc = RECT::default();
                if GetClientRect(hwnd, &mut rc).is_ok() {
                    let now = hit_btn(rc.right, cx, cy);
                    let changed = {
                        let Some(p) = state(hwnd) else {
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        };
                        let st = &mut *p;
                        let old = st.hover;
                        st.hover = now;
                        if now.is_some() && old != now {
                            // 进按钮时登记 LEAVE：鼠标快速划出窗口时不会再有
                            // WM_MOUSEMOVE，底纹恢复全靠这条兜底（用户报的
                            // "移出后底纹有时不恢复"就是它）。
                            let mut tme = TRACKMOUSEEVENT {
                                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                                dwFlags: TME_LEAVE,
                                hwndTrack: hwnd,
                                dwHoverTime: 0,
                            };
                            let _ = TrackMouseEvent(&mut tme);
                        }
                        old != now
                    };
                    if changed {
                        let bar = RECT {
                            left: 0,
                            top: 0,
                            right: rc.right,
                            bottom: TITLE_H,
                        };
                        let _ = InvalidateRect(Some(hwnd), Some(&bar), false);
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            // 焦点：两条路径都得把焦点送到 EDIT。
            //   ・WM_ACTIVATE —— 窗口被激活（点窗口任意处、Alt+Tab 回来）；
            //   ・WM_SETFOCUS —— 点标题栏按钮后焦点回到本窗。
            //
            // ★★★ 激活时的聚焦**必须延迟**：真机探针（note_drag_probe）抓到的铁证是
            //   「按下 → WM_NCHITTEST → WM_MOUSEACTIVATE(HTCAPTION) → WM_ACTIVATE」之后
            //   WM_NCLBUTTONDOWN 再也没来 —— 旧代码在 WM_ACTIVATE 里**同步** SetFocus 到
            //   正文窗里的 EDIT，而正文窗是另一张**顶层**窗，这一下把标题栏窗当场反激活
            //   （WM_ACTIVATE wp=1 紧跟 wp=0），系统的标题点击派发链（按下 → NC 消息 →
            //   SC_MOVE 模态循环）被自己人掐断，「按住标题栏拖不动」。单窗旧版 EDIT 是
            //   同窗子控件，SetFocus 不跨顶层，所以从来没暴露。
            //   现在只投一条延迟焦点消息；真正聚焦时若正处移动/缩放模态循环或透明度
            //   弹窗开着，则跳过（拖动中抢焦会打断循环；弹窗抢焦会让它闪开即关）。
            //   （EDIT 现在是**正文窗**的子控件，`WM_PARENTNOTIFY` 这条旧兜底因此收不到
            //    了——好在 EDIT 所在的正文窗自己也会被打字之路正确聚焦。）
            WM_ACTIVATE => {
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                if (wparam.0 & 0xFFFF) != WA_INACTIVE {
                    let _ = PostMessageW(Some(hwnd), WM_APP_NOTE_FOCUS, WPARAM(0), LPARAM(0));
                    // ★ 自愈：置顶态被系统/别的窗口挤掉时，激活瞬间重断一次
                    //   （WS_EX_TOPMOST 读不回来，只能靠标志重放）。
                    if let Some(pp) = state(hwnd) {
                        if (*pp).flags & FLG_TOPMOST != 0 {
                            let _ = SetWindowPos(
                                hwnd,
                                Some(HWND_TOPMOST),
                                0,
                                0,
                                0,
                                0,
                                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                            );
                            sync_topmost(hwnd);
                        }
                    }
                }
                r
            }
            // 延迟聚焦（见 WM_ACTIVATE 的注释）。三个让路条件，缺一不可：
            //   ・模态循环中 —— 抢焦 = 反激活 = 移动/缩放循环中途夭折；
            //   ・左键按住 —— 积压的延迟消息若在"按下→派发"的窗口内被处理，
            //     同样会把激活状态打掉，`WM_NCLBUTTONDOWN` 就再也不投了（探针实证）；
            //   ・弹窗开着 —— 抢焦会触发它的 WM_KILLFOCUS，"点开即关"。
            WM_APP_NOTE_FOCUS => {
                let btn_down = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0;
                if !thread_in_modal_loop() && !btn_down {
                    let popup_open = state(hwnd).map(|p| (*p).popup.is_some()) == Some(true);
                    if !popup_open {
                        focus_edit(hwnd);
                    }
                }
                LRESULT(0)
            }
            WM_SETFOCUS => {
                // ★★★ 与 WM_ACTIVATE 同理：**同步** SetFocus(EDIT) 会把激活权交回正文窗
                //   （EDIT 在另一张顶层窗里），标题点击派发链当场被打断（探针实证：
                //   WM_SETFOCUS 紧跟 WM_ACTIVATE(0→1)，随后 WM_NCLBUTTONDOWN 消失）。
                //   一律走延迟消息，让点击先走完自己的派发。
                let r = DefWindowProcW(hwnd, msg, wparam, lparam);
                let _ = PostMessageW(Some(hwnd), WM_APP_NOTE_FOCUS, WPARAM(0), LPARAM(0));
                r
            }
            // 滚轮：**不做事**（2026-09-29 用户口径——透明度弹窗上的滚轮与标题栏
            //   Ctrl+滚轮都删了：拖动条已直观可调，滚轮档与"正文滚轮调字号"的心智
            //   冲突、有误触风险）。吞掉即可，别转 DefWindowProc（滚动无关）。
            WM_MOUSEWHEEL => {
                // 刻意吞掉：标题栏不再支持滚轮调透明度（见上注释）。
                LRESULT(0)
            }
            // 拖动 / 缩放模态循环结束：期间"左键按住"把延迟聚焦挡掉了，这里补发一次，
            // 让"拖完标题栏直接打字"和旧单窗版行为一致。
            WM_EXITSIZEMOVE => {
                let _ = PostMessageW(Some(hwnd), WM_APP_NOTE_FOCUS, WPARAM(0), LPARAM(0));
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            // EDIT 的配色（`WM_CTLCOLOREDIT`）已经随 EDIT 一起搬到正文窗去了，见
            //   `body_wndproc` —— 本窗不再有子 EDIT，这段如果留着就是死代码。
            // 关闭：锁定态**直接吞掉**（用户定案：锁定 = 不可关，不再弹二次确认）。
            WM_CLOSE => {
                let locked = state(hwnd).map(|p| (*p).flags & FLG_LOCKED != 0) == Some(true);
                if !locked {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            // 释放状态（含背景画刷）。
            WM_NCDESTROY => {
                // 先把可能开着的「透明度」弹窗收掉：它是本窗的 owned 窗，理论上下一步
                //   系统也会连它一起销毁，但那样它的 `WM_NCDESTROY` 会反过来访问一个
                //   已经释放的 `WinState`。先销毁 = 收尾永远是我主导。
                if let Some(p) = state(hwnd) {
                    if let Some(h) = (*p).popup.take() {
                        let _ = DestroyWindow(h);
                    }
                }
                if let Some(p) = state(hwnd) {
                    let st = Box::from_raw(p);
                    let _ = DeleteObject(st.bg_brush.into());
                    let _ = DeleteObject(st.edit_brush.into());
                    let _ = DeleteObject(st.edit_font.into());
                }
                unregister_lock(hwnd);
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 标题栏按钮行为。
fn on_btn(hwnd: HWND, which: u8) {
    let Some(p) = state(hwnd) else { return };
    let st = unsafe { &mut *p };
    unsafe {
        match which {
            BTN_ADD => {
                // 新增一张同配置的便签：本线程即 pump，直接再建。
                let cfg = st.cfg.clone();
                spawn_note(&cfg);
            }
            BTN_TOP => {
                if !btn_active(st.flags, BTN_TOP) {
                    return;
                }
                st.flags ^= FLG_TOPMOST;
                let _ = SetWindowPos(
                    hwnd,
                    Some(if st.flags & FLG_TOPMOST != 0 {
                        HWND_TOPMOST
                    } else {
                        HWND_NOTOPMOST
                    }),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
                // ★ GWL_EXSTYLE **读不出** WS_EX_TOPMOST（Win32 文档明言该样式不可
                //   检索，读回恒 0）—— 不能拿它当"真实状态"。以 SetWindowPos 返回值
                //   为准：失败就回滚标志，图标与实际永不失步。
                if SetWindowPos(
                    hwnd,
                    Some(if st.flags & FLG_TOPMOST != 0 {
                        HWND_TOPMOST
                    } else {
                        HWND_NOTOPMOST
                    }),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
                .is_err()
                {
                    st.flags ^= FLG_TOPMOST;
                }
                // ★ 置顶是**两张窗**的事：正文窗是独立顶层窗，得一起切。
                sync_topmost(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            BTN_ALPHA => {
                if !btn_active(st.flags, BTN_ALPHA) {
                    return;
                }
                // 弹出 / 收起拖动条（toggle）。★ 先把 `st` 这段可变借用收掉
                //   （`open_alpha_popup` 自己要拿 `WinState`，不能抱着借用来开）。
                let _ = st;
                open_alpha_popup(hwnd);
            }
            BTN_LOCK => {
                st.flags ^= FLG_LOCKED;
                let locked = st.flags & FLG_LOCKED != 0;
                let _ = SendMessageW(
                    st.edit,
                    EM_SETREADONLY,
                    Some(WPARAM(if locked { 1 } else { 0 })),
                    Some(LPARAM(0)),
                );
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            BTN_COLLAPSE => {
                if !btn_active(st.flags, BTN_COLLAPSE) {
                    return;
                }
                let coll = st.flags & FLG_COLLAPSED == 0;
                set_collapsed(hwnd, coll);
            }
            BTN_CLOSE => {
                if !btn_active(st.flags, BTN_CLOSE) {
                    return;
                }
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
    }
}

// ── 正文窗 / EDIT 子类 / 「透明度」弹窗 ────────────────────────────────────

/// 正文窗的窗口过程：它是 EDIT 的母窗，负责编辑区配色 + 接住 Ctrl+滚轮（改字号）。
///
/// ★ 为什么需要这层壳：正文区必须是一张**独立 layered 窗**（才有自己的 alpha），而 EDIT
///   又必须是某个窗的子控件（才有 IME / 焦点 / `WM_CTLCOLOREDIT`）。壳只做转发与配色，
///   不画任何东西（正文区里的一切都是 EDIT 画的）。
extern "system" fn body_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    unsafe {
        match msg {
            WM_SIZE => {
                let _ = DefWindowProcW(hwnd, msg, wparam, lparam);
                layout_edit_in_body(hwnd);
                LRESULT(0)
            }
            // EDIT 铺满整窗、自己刷底 —— 本窗不参与擦背景（也省一次全窗刷新）。
            WM_ERASEBKGND => LRESULT(1),
            WM_MOUSEACTIVATE => LRESULT(1),
            // Ctrl+滚轮 = 字号（由 EDIT 子类转发过来，见 `edit_wndproc`）。
            WM_MOUSEWHEEL => {
                if win_util::control_down() {
                    let delta = ((wparam.0 as u32) >> 16) as i16;
                    if let Some(bp) = body_state(hwnd) {
                        bump_font((*bp).frame, if delta > 0 { 1.0 } else { -1.0 });
                    }
                }
                LRESULT(0)
            }
            // 编辑区配色（**wParam 才是 HDC，lParam 是控件 HWND** —— 与直觉相反，且与
            //   `WM_NCHITTEST`/`WM_LBUTTON*` 那批"坐标在 lParam"的习惯正好反过来）。
            //   把 lParam 当 DC 用时 `SetBkColor`/`SetTextColor`/`SetBkMode` 全都静默失败
            //   （句柄非法，不报错）—— 于是只有**返回的画刷**生效，表现极具迷惑性：
            //     ・整块背景看着是对的（擦除用了我们的 bg 画刷）；
            //     ・但 **EDIT 画文本行时用默认白底**，深底便签上就是"正文顶部一条白"，
            //       文字色也回落到默认黑 —— 用户报的"颜色不统一"正是这一条。
            //   真机像素判据（探针 v11/v12/v13）：白条 = 便签内 y=28..47（EDIT 内 y=2..21），
            //   每行文本一条、跟着 EDIT 走、隐藏 EDIT 即消失。
            // ★ 只读 / 禁用的 EDIT 发的是 **WM_CTLCOLORSTATIC**（不是 CTLCOLOREDIT）——
            //   不接管的话 DefWindowProc 回 COLOR_3DFACE（240,240,240）灰白，锁定态
            //   编辑区整块变浅。两条消息共用同一套配色。
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                if let Some(bp) = body_state(hwnd) {
                    let frame = (*bp).frame;
                    if let Some(p) = state(frame) {
                        let st = &*p;
                        let (dc, _who) = ctl_color_parts(wparam, lparam);
                        // 编辑区用自己的配色（可与整体背景区分）。锁定**不改色**：
                        // 只锁交互（用户定案），颜色状态只认配置。
                        let _ = SetBkColor(dc, win_util::colorref(st.cfg.edit));
                        // ★ 文字色自带的 alpha：预混进**编辑区底色**（见 `over` 的说明）。
                        let _ =
                            SetTextColor(dc, win_util::colorref(over(st.cfg.edit, st.cfg.text)));
                        // TRANSPARENT：文字行的底不再单独刷一层，透出下面那支画刷铺的底。
                        let _ = SetBkMode(dc, TRANSPARENT);
                        return LRESULT(st.edit_brush.0 as isize);
                    }
                }
                LRESULT(0)
            }
            // 回收挂在 GWLP_USERDATA 上的 Box<BodyState>：没有这一支，每张便签关一次
            // 就漏一个堆块（泄露审计 2026-09-30）。清掉指针防重入。
            WM_NCDESTROY => {
                let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut BodyState;
                if !p.is_null() {
                    drop(Box::from_raw(p));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 原 EDIT 窗口过程的裸类型（子类化后要自己往回调用）。
type RawWndProc =
    unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> windows::Win32::Foundation::LRESULT;

/// EDIT 控件的子类过程：**只做一件事** —— 把 Ctrl+滚轮交给正文窗。
///
/// ★ EDIT 会把滚轮当"滚动正文"吃掉；它的母窗现在是正文窗（顶层），再没有
///   `DefWindowProc` 替我们往上层转发。不转这一手，Ctrl+滚轮改字号就废了。
extern "system" fn edit_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    unsafe {
        if msg == WM_MOUSEWHEEL && win_util::control_down() {
            // `GetParent` 在这个 windows 版本返回 `Result`（失败 = 取不到母窗）。
            let Some(owner) = GetParent(hwnd).ok() else {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            };
            if !owner.0.is_null() {
                return SendMessageW(owner, WM_MOUSEWHEEL, Some(wparam), Some(lparam));
            }
        }
        // 原 proc 存在 `GWLP_USERDATA`（系统 EDIT 控件不用这一段）。
        let prev = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if prev == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let f: RawWndProc = std::mem::transmute(prev as usize as *const ());
        f(hwnd, msg, wparam, lparam)
    }
}

// ── 「透明度」拖动条弹窗（标题栏 ◐ 按钮点出来的那个）─────────────────────

/// 弹窗尺寸（物理像素）。
const POPUP_W: i32 = 176;
const POPUP_H: i32 = 44;
/// 轨道左右留白。
const POPUP_PAD: i32 = 12;
/// 轨道上下位置 / 厚度。
const TRACK_TOP: i32 = 28;
const TRACK_H: i32 = 6;
/// 滑块宽（比轨道上下各高出 `THUMB_PAD`）。
const THUMB_W: i32 = 12;
const THUMB_PAD: i32 = 4;

/// 轨道矩形（弹窗客户区坐标，窗口尺寸固定 [`POPUP_W`]×[`POPUP_H`]）。
fn popup_track_rect() -> RECT {
    RECT {
        left: POPUP_PAD,
        top: TRACK_TOP,
        right: POPUP_W - POPUP_PAD,
        bottom: TRACK_TOP + TRACK_H,
    }
}

/// 数值文字那一行的矩形（轨道上方）。
fn popup_label_rect() -> RECT {
    RECT {
        left: 0,
        top: 3,
        right: POPUP_W,
        bottom: TRACK_TOP - 3,
    }
}

/// 横坐标 → 不透明度（[`NOTE_ALPHA_MIN`]–[`NOTE_ALPHA_MAX`]），越界自动夹住。
fn alpha_from_x(x: i32) -> u32 {
    let t = popup_track_rect();
    let span = (t.right - t.left).max(1) as f32;
    // ★ +0.5：按**像素中心**映射。不留这个偏移，拖到轨道最右端只能到 99（差一格），
    //   用户一定会来报"拖到底也不是 100"。
    let f = (((x - t.left) as f32 + 0.5) / span).clamp(0.0, 1.0);
    let v = NOTE_ALPHA_MIN as f32 + f * (NOTE_ALPHA_MAX - NOTE_ALPHA_MIN) as f32;
    v.round()
        .clamp(NOTE_ALPHA_MIN as f32, NOTE_ALPHA_MAX as f32) as u32
}

/// 不透明度 → 滑块中心 x（与 [`alpha_from_x`] 互逆，误差 ≤ 1px）。
fn thumb_x(v: u32) -> i32 {
    let t = popup_track_rect();
    let v = v.clamp(NOTE_ALPHA_MIN, NOTE_ALPHA_MAX);
    let f = (v - NOTE_ALPHA_MIN) as f32 / (NOTE_ALPHA_MAX - NOTE_ALPHA_MIN) as f32;
    t.left + (f * (t.right - t.left) as f32).round() as i32
}

/// 滑块矩形（比轨道上下各高出 [`THUMB_PAD`]，好抓）。
fn thumb_rect(v: u32) -> RECT {
    let t = popup_track_rect();
    let cx = thumb_x(v);
    RECT {
        left: cx - THUMB_W / 2,
        top: t.top - THUMB_PAD,
        right: cx + THUMB_W / 2 + 1,
        bottom: t.bottom + THUMB_PAD,
    }
}

/// 弹窗 → 它归属的标题栏窗（`GWLP_USERDATA` 里存的就是它）。
fn alpha_popup_frame(h: HWND) -> Option<HWND> {
    unsafe {
        let p = GetWindowLongPtrW(h, GWLP_USERDATA);
        if p == 0 {
            None
        } else {
            Some(HWND(p as *mut _))
        }
    }
}

/// 拖动：横坐标 → 设当前便签的总不透明度。
fn set_alpha_from_x(popup: HWND, x: i32) {
    let Some(frame) = alpha_popup_frame(popup) else {
        return;
    };
    apply_total_alpha(frame, alpha_from_x(x));
}

extern "system" fn alpha_popup_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    unsafe {
        match msg {
            WM_PAINT => {
                let _ = DefWindowProcW(hwnd, msg, wparam, lparam);
                paint_alpha_popup(hwnd);
                LRESULT(0)
            }
            WM_MOUSEACTIVATE => LRESULT(1),
            // 按下即跳到该位置（"拖动条"要能一点就到，不能只能拖）。
            WM_LBUTTONDOWN => {
                let _ = SetCapture(hwnd);
                set_alpha_from_x(hwnd, lparam_xy(lparam).0);
                LRESULT(0)
            }
            // 只有已经 capture（按着）时才跟着动 —— 划过就改值是灾难。
            WM_MOUSEMOVE => {
                if GetCapture().0 == hwnd.0 {
                    set_alpha_from_x(hwnd, lparam_xy(lparam).0);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                LRESULT(0)
            }
            // ★ 滚轮调节已删（2026-09-29 用户口径）：拖动条已直观可调，且便签正文
            //   滚轮另有"调字号"语义，弹窗上再挂一份既重复又有误触风险。
            WM_KEYDOWN => {
                let k = wparam.0 as u32;
                if k == VK_ESCAPE.0 as u32 || k == VK_RETURN.0 as u32 {
                    let _ = DestroyWindow(hwnd);
                }
                LRESULT(0)
            }
            // 焦点一丢就关（点别处 / 切窗口）。便签本体随后会把焦点收回去。
            WM_KILLFOCUS => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_NCDESTROY => {
                // 清掉标题栏窗里记的句柄，否则二次点按钮以为它还开着（点了没反应）。
                if let Some(frame) = alpha_popup_frame(hwnd) {
                    if let Some(p) = state(frame) {
                        if (*p).popup == Some(hwnd) {
                            (*p).popup = None;
                        }
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 画弹窗：数值一行 + 轨道 + 滑块（配色跟着便签自己走）。
fn paint_alpha_popup(hwnd: HWND) {
    let Some(frame) = alpha_popup_frame(hwnd) else {
        return;
    };
    let Some(p) = state(frame) else { return };
    let st = unsafe { &*p };
    let v = st.cfg.alpha;
    let dc = unsafe { GetDC(Some(hwnd)) };
    if dc.is_invalid() {
        return;
    }
    unsafe {
        let bg = win_util::colorref(st.cfg.bg);
        let ink = win_util::colorref(over(st.cfg.bg, st.cfg.text));
        let all = RECT {
            left: 0,
            top: 0,
            right: POPUP_W,
            bottom: POPUP_H,
        };
        let brush = CreateSolidBrush(bg);
        FillRect(dc, &all, brush);
        let _ = DeleteObject(brush.into());

        // 轨道：底（淡）+ 已填充段（浓）+ 滑块（实色），三段都用同一套配色插值。
        let t = popup_track_rect();
        let track_b = CreateSolidBrush(win_util::colorref(mix_color(st.cfg.bg, st.cfg.text, 0.22)));
        FillRect(dc, &t, track_b);
        let _ = DeleteObject(track_b.into());
        let fill = RECT {
            left: t.left,
            top: t.top,
            right: thumb_x(v).max(t.left),
            bottom: t.bottom,
        };
        let fill_b = CreateSolidBrush(win_util::colorref(mix_color(st.cfg.bg, st.cfg.text, 0.7)));
        FillRect(dc, &fill, fill_b);
        let _ = DeleteObject(fill_b.into());
        let knob = thumb_rect(v);
        let knob_b = CreateSolidBrush(ink);
        FillRect(dc, &knob, knob_b);
        let _ = DeleteObject(knob_b.into());

        // 数值：不写在控件里是因为这整块是手绘的（没有第三方滑条控件，也不打算引）。
        let _ = SetBkMode(dc, TRANSPARENT);
        let _ = SetTextColor(dc, ink);
        let font = make_font(13.0);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let text = format!("不透明度 {}%", v);
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        let mut r = popup_label_rect();
        let _ = DrawTextW(
            dc,
            &mut wide[..],
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        let _ = SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(font.0));
    }
    unsafe {
        ReleaseDC(Some(hwnd), dc);
    }
}

/// 打开 / 收起「透明度」拖动条（toggle）。位置贴在 ◐ 按钮正下方，并夹进虚拟屏。
fn open_alpha_popup(frame: HWND) {
    // 已经开着 → 收起（第二次点同一个按钮）。
    let opened = state(frame).and_then(|p| unsafe { (*p).popup });
    if let Some(h) = opened {
        unsafe {
            let _ = DestroyWindow(h);
        }
        return;
    }
    let (wr, btn) = {
        let mut wr = RECT::default();
        if unsafe { GetWindowRect(frame, &mut wr) }.is_err() {
            return;
        }
        let cw = wr.right - wr.left;
        (wr, btn_rect(cw, BTN_ALPHA))
    };
    let (vx, vy, vw, vh) = win_util::virtual_screen_rect();
    let mut x = wr.left + (btn.left + btn.right) / 2 - POPUP_W / 2;
    let mut y = wr.top + TITLE_H + 2;
    x = x.clamp(vx, (vx + vw - POPUP_W).max(vx));
    if y + POPUP_H > vy + vh {
        // 便签贴着屏幕下缘 → 翻到标题栏上方，别把拖动条甩出屏幕。
        y = (wr.top - POPUP_H - 2).max(vy);
    }
    let cls: Vec<u16> = ALPHA_CLASS
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let h = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0),
            PCWSTR(cls.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(WS_POPUP.0),
            x,
            y,
            POPUP_W,
            POPUP_H,
            Some(frame),
            None,
            None,
            None,
        )
        .ok()
    };
    let Some(h) = h else { return };
    unsafe {
        SetWindowLongPtrW(h, GWLP_USERDATA, frame.0 as isize);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        let _ = ShowWindow(h, SW_SHOW);
        // 拿到焦点，Esc 与"点别处自动关"才成立（WM_KILLFOCUS 就靠它）。
        let _ = SetFocus(Some(h));
    }
    if let Some(p) = state(frame) {
        unsafe { (*p).popup = Some(h) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_clamped_10_100() {
        // ★ 走**真函数**（`layered_alpha`），不再就地复算一遍公式：复算的那份改了
        //   实现不会红，等于给错了的换算发通行证（踩过：旧用例算的是整除 127，
        //   真函数四舍五入是 128）。
        for (v, want) in [(10u32, 26u8), (50, 128), (100, 255)] {
            let a = layered_alpha(v, 255);
            assert_eq!(a, want, "总不透明度 {v}（颜色完全不透明）换算不符");
        }
        // 越界的总不透明度被夹住（拖动条不会给，但配置是用户手写的 json）。
        assert_eq!(layered_alpha(0, 255), layered_alpha(NOTE_ALPHA_MIN, 255));
        assert_eq!(layered_alpha(9999, 255), 255);
    }

    /// ★ 用户口径（2026-09-27）：**最终不透明度 = 总不透明度 × 颜色的不透明度**。
    ///
    /// 这不是"两个旋钮各自有效"就完事——必须真的是**相乘**：只让后者生效（旧 bug）
    /// 或只让前者生效，都会让另一半变成摆设。
    #[test]
    fn layered_alpha_is_the_total_times_the_color_alpha() {
        // ① 满档：100% × 完全不透明 = 255。
        assert_eq!(layered_alpha(100, 255), 255, "100% × 不透明 = 全不透明");
        assert_eq!(
            layered_alpha(100, 128),
            128,
            "总 100% 时颜色 alpha 直接就是它自己"
        );
        // ② 总不透明度减半，颜色不动 —— 必须跟着减半（不是不变）。
        assert_eq!(layered_alpha(50, 255), 128, "50% × 不透明 ≈ 128");
        assert_eq!(layered_alpha(60, 200), 120, "60% × 200/255 ≈ 120");
        // ③ 颜色 alpha 减半，总的不动 —— 同样必须跟着减（★ 旧实现这里恒等于 255）。
        assert_eq!(layered_alpha(100, 128), 128, "颜色半透明必须真的变透");
        assert!(
            layered_alpha(100, 64) < layered_alpha(100, 200),
            "颜色越透，层越透 —— 单调"
        );
        // ④ 两侧都不能把对方"吃掉"。
        assert_eq!(layered_alpha(10, 255), 26, "10% 下限仍在（不是塌成 0）");
        assert!(
            layered_alpha(30, 255) < layered_alpha(60, 255),
            "总不透明度单调"
        );
        // ⑤ 颜色 alpha = 0 → 夹到 1：`LWA_ALPHA = 0` 是"整张窗彻底不可见"，
        //    用户会以为便签崩了，宁可留 1/255。
        assert_eq!(layered_alpha(100, 0), 1, "全透色也要留一点可见性");
    }

    /// 两层的 alpha **必须能不一样** —— 这正是拆成两张窗的全部理由。
    #[test]
    fn the_two_layers_get_their_own_alpha() {
        let mut cfg = NoteCfg::default();
        cfg.bg[3] = 255; // 标题栏：全实
        cfg.edit[3] = 128; // 编辑区：半透
        let title = layered_alpha(cfg.alpha, cfg.bg[3]);
        let body = layered_alpha(cfg.alpha, cfg.edit[3]);
        assert_ne!(title, body, "两张窗的 alpha 必须各自独立，否则白拆了");
        assert_eq!(title, 255);
        assert_eq!(body, 128);
    }

    /// GDI 不认 alpha，所以"文字半透明"只能**预混进它下面那层底色**。
    /// 这里把预混结果与"真 alpha 合成"逐个通道对账（不是"看起来差不多"）。
    #[test]
    fn over_is_exactly_alpha_compositing_over_an_opaque_layer() {
        let bg = [40u8, 44, 52, 235];
        let fg = [232u8, 236, 240, 255];
        // 完全不透明 = 原色；全透 = 底色。
        assert_eq!(over(bg, fg)[0..3], fg[0..3], "alpha=255 → 就是前景色");
        let invisible = [232u8, 236, 240, 0];
        assert_eq!(over(bg, invisible)[0..3], bg[0..3], "alpha=0 → 就是底色");
        // 中间值：与"手动按 alpha 合成"逐通道相等（误差 ≤ 舍入 1）。
        for a in [32u8, 64, 100, 128, 200] {
            let mut fg_a = fg;
            fg_a[3] = a;
            let got = over(bg, fg_a);
            for i in 0..3 {
                let want = bg[i] as f32 + (fg[i] as f32 - bg[i] as f32) * (a as f32 / 255.0);
                assert!(
                    (got[i] as f32 - want).abs() <= 1.0,
                    "alpha={a} 通道 {i}：预混 {got:?} 与真合成 {want} 不一致"
                );
            }
        }
    }

    /// ★ 标题栏窗**不许**在正文区上色 —— 上了就是"双重底色"（两张窗各自半透明，
    /// 叠出来的颜色比配置深一档，且不报错）。
    #[test]
    fn frame_paint_rects_never_touch_the_body_area() {
        let (cw, ch) = (272, 220);
        // 正文窗占的那一块（见 `layout_body`）：左/右各让 MARGIN，下让 MARGIN，
        // 上边从 TITLE_H 起。
        let interior = RECT {
            left: MARGIN,
            top: TITLE_H,
            right: cw - MARGIN,
            bottom: ch - MARGIN,
        };
        for r in frame_paint_rects(cw, ch) {
            let overlap_w = (r.right.min(interior.right) - r.left.max(interior.left)).max(0);
            let overlap_h = (r.bottom.min(interior.bottom) - r.top.max(interior.top)).max(0);
            assert_eq!(
                overlap_w * overlap_h,
                0,
                "标题栏窗在正文区上了色（重叠 {overlap_w}×{overlap_h}）：{r:?}"
            );
        }
        // 反过来：标题栏整条必须被覆盖（否则标题栏会出现透明洞）。
        let painted = frame_paint_rects(cw, ch);
        assert!(
            painted
                .iter()
                .any(|r| r.left == 0 && r.top == 0 && r.right == cw && r.bottom == TITLE_H),
            "标题栏整条必须上色"
        );
        // 收起态：整窗就剩标题栏那一条。
        let folded = frame_paint_rects(cw, TITLE_H);
        assert_eq!(folded.len(), 1, "收起态只画标题栏那一条");
        assert_eq!(folded[0].bottom, TITLE_H);
    }

    #[test]
    fn alpha_slider_maps_the_whole_range_and_round_trips() {
        let t = popup_track_rect();
        // 端点必须正好是量程的两端（不能"拖到最后还差一点"）。
        assert_eq!(alpha_from_x(t.left), NOTE_ALPHA_MIN, "最左 = 下限");
        assert_eq!(alpha_from_x(t.right - 1), NOTE_ALPHA_MAX, "最右 = 上限");
        // 越界夹住（拖动时鼠标会跑出窗口）。
        assert_eq!(alpha_from_x(t.left - 50), NOTE_ALPHA_MIN);
        assert_eq!(alpha_from_x(t.right + 50), NOTE_ALPHA_MAX);
        assert_eq!(alpha_from_x(0), NOTE_ALPHA_MIN);
        // x ↔ 值 往返一致（滑块画的位置必须就是点下时得到的值，差 1px 以内）。
        for v in [10u32, 25, 50, 73, 90, 100] {
            let x = thumb_x(v);
            assert!(
                (alpha_from_x(x) as i32 - v as i32).abs() <= 1,
                "v={v} 往返丢了精度（x={x}）"
            );
            let r = thumb_rect(v);
            assert!(r.left >= 0 && r.right <= POPUP_W, "滑块不能画出窗口：{r:?}");
            assert!(
                r.top < t.top && r.bottom > t.bottom,
                "滑块要比轨道高一点才好抓"
            );
        }
        assert_eq!(thumb_x(NOTE_ALPHA_MIN), t.left, "最小值贴最左");
        assert_eq!(thumb_x(NOTE_ALPHA_MAX), t.right, "最大值贴最右");
    }

    #[test]
    fn collapse_restores_the_height_it_was_before() {
        // 220 → 折叠(记 220) → 展开必须回到 220。
        // ★ 旧代码的失败形态：折叠时 WM_SIZE 把记着的高度覆盖成 26，
        //   展开就只能回到 26.max(26+40) = 66。
        let (remember, target) = collapse_height(true, 220, DEFAULT_H);
        assert_eq!(
            (remember, target),
            (220, TITLE_H),
            "折叠：记住 220、收到标题栏高"
        );
        let (_, back) = collapse_height(false, TITLE_H, remember);
        assert_eq!(back, 220, "★ 展开必须回到折叠前的 220，不能是 66");
    }

    #[test]
    fn ctl_color_puts_dc_in_wparam() {
        // ★ 反直觉但必须如此：DC 在 wParam、控件 HWND 在 lParam。
        //   取反的话 SetBkColor 全部静默失败 → 深底便签上"正文顶部一条白"。
        let dc = 0x1234usize;
        let edit = 0xABCDusize;
        let (got_dc, got_hwnd) = ctl_color_parts(WPARAM(dc), LPARAM(edit as isize));
        assert_eq!(got_dc.0 as usize, dc, "DC 必须来自 wParam");
        assert_eq!(got_hwnd.0 as usize, edit, "控件 HWND 必须来自 lParam");
    }

    #[test]
    fn lparam_xy_unpacks_signed_coords() {
        // 三条：屏内正值、负坐标（多显示器左上象限）、以及"曾经踩的那个"
        //   错写 `(lparam.0 as u16 as u32 >> 16)` —— 它对**任何** lParam 都给 y=0。
        let pack = |x: i32, y: i32| LPARAM(((y as u16 as isize) << 16) | (x as u16 as isize));
        assert_eq!(lparam_xy(pack(120, 13)), (120, 13));
        assert_eq!(lparam_xy(pack(-40, -7)), (-40, -7));
        assert_eq!(lparam_xy(pack(1919, 1079)), (1919, 1079));
        let l = pack(254, 13);
        let wrong = (l.0 as u16 as u32 >> 16) as i32; // ★ 旧写法
        assert_eq!(wrong, 0, "旧写法的 y 恒为 0 —— 这正是'按钮全哑'的根因");
        assert_eq!(lparam_xy(l).1, 13, "新写法必须把 13 拆出来");
    }

    #[test]
    fn every_titlebar_button_is_hittable_at_its_center() {
        // ★ 守卫发生过一次的真 bug：WM_LBUTTONUP 里把 y 拆错成 0，于是五个按钮一个都
        //   点不中。这里取**每个按钮自己的矩形中心**喂给 `hit_btn`，逐个断言命中。
        let cw = 272;
        for b in [
            BTN_ADD,
            BTN_TOP,
            BTN_LOCK,
            BTN_COLLAPSE,
            BTN_CLOSE,
            BTN_ALPHA,
        ] {
            let r = btn_rect(cw, b);
            let hit = hit_btn(cw, (r.left + r.right) / 2, (r.top + r.bottom) / 2);
            assert_eq!(hit, Some(b), "按钮 {b} 的矩形中心竟然没命中自己");
        }
        // 对照组：标题栏正中（按钮之间的空档）应判为"没有按钮"→ 走拖动。
        assert_eq!(hit_btn(cw, cw / 2, TITLE_H / 2), None);
        // 对照组：正文区一律没有按钮（不能因为 y 拆错而误判进标题栏）。
        assert_eq!(hit_btn(cw, cw / 2, TITLE_H + 40), None);
    }

    #[test]
    fn lock_disables_everything_but_add_and_lock() {
        // ★ 锁定语义（用户定案）：文本只读 + 位置锁死 + 置顶/收起/关闭全禁，仅 ＋ 可用。
        let unlocked = 0u8;
        assert!(
            [BTN_ADD, BTN_TOP, BTN_LOCK, BTN_COLLAPSE, BTN_CLOSE]
                .iter()
                .all(|&b| btn_active(unlocked, b)),
            "未锁：全可用"
        );
        let locked = FLG_LOCKED;
        assert!(btn_active(locked, BTN_ADD), "锁定后 ＋ 仍可用");
        assert!(btn_active(locked, BTN_LOCK), "锁定按钮本身是解锁入口");
        assert!(!btn_active(locked, BTN_TOP));
        assert!(!btn_active(locked, BTN_COLLAPSE));
        assert!(!btn_active(locked, BTN_CLOSE));
        assert!(!btn_active(locked, BTN_ALPHA), "锁定后透明度也不给动");
    }

    #[test]
    fn glyph_reflects_state() {
        assert_eq!(btn_glyph(FLG_TOPMOST, BTN_TOP), '\u{1F4CC}');
        assert_eq!(btn_glyph(FLG_LOCKED, BTN_LOCK), '\u{1F512}');
        assert_eq!(btn_glyph(0, BTN_LOCK), '\u{1F513}');
        assert_eq!(btn_glyph(FLG_COLLAPSED, BTN_COLLAPSE), '\u{25B2}');
        assert_eq!(btn_glyph(0, BTN_COLLAPSE), '\u{25BC}');
        assert_eq!(btn_glyph(0, BTN_ALPHA), '\u{25D0}');
    }

    #[test]
    fn mix_color_pushes_toward_target() {
        let bg = [40u8, 44, 52, 235];
        let fg = [232u8, 236, 240, 255];
        assert_eq!(mix_color(bg, fg, 0.0), bg, "t=0 应原样");
        assert_eq!(
            mix_color(bg, fg, 1.0),
            [232, 236, 240, 235],
            "t=1 应到目标色"
        );
        let mid = mix_color(bg, fg, 0.5);
        assert!(mid[0] > bg[0] && mid[0] < fg[0], "中间值要落在两者之间");
    }

    #[test]
    fn hit_btn_geometry_right_aligned() {
        // 四个右侧按钮从右往左排：× 最右，置 最左。
        let cw = 272;
        let close = btn_rect(cw, BTN_CLOSE);
        let top = btn_rect(cw, BTN_TOP);
        assert!(close.right > top.right, "× 应在最右");
        assert!(close.left >= top.right, "× 在置 右侧");
    }
}
