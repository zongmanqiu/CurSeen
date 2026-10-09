//! AI 对话原生窗口（Pecia 式）：顶部对话区、中部「清除 / 发送」、底部输入框。
//!
//! 与 [`crate::platform::win_util`] 的原生框同族：调用线程上建窗 + 消息泵，阻塞到
//! 关窗才返回；HTTP 走独立 worker 线程（[`crate::platform::http::post_json`]），结果
//! 打包 `PostMessageW` 回窗——等待回复期间界面全程不卡，标题栏读秒（`AI 写脚本 (Ns)`，
//! Pecia `showTitleElapsed` 同款）。
//!
//! # 观感（2026-09-30 用户反馈定稿：全黑正文 + 平面黑框线）
//!
//! - 正文**全部一种颜色**（近黑）：上一版「我的黑 / AI 红」靠 RichEdit
//!   `EM_SETCHARFORMAT` 按段上色——需求取消后富文本没了存在理由，两个框都回归
//!   普通 EDIT，**不再加载 riched20.dll**（少拉一条 DLL 依赖链；真机探针实测它
//!   本身只占 ~0.4MB，内存大头是 Lua 引擎 + 窗机制，见探针内存里程碑）。
//! - **边框全部平面化、纯黑 1px**：EDIT 自带边框（WS_BORDER）在经典主题下画成
//!   凹陷 3D 斜面（用户原话「有 3D 效果，很丑」）。两个 EDIT 都去掉系统边框，
//!   由主窗在 `WM_PAINT` 里 `FrameRect` 画外圈 1px 黑线；按钮 `BS_OWNERDRAW`
//!   自绘同样画 1px 黑框。悬停换浅灰底（`WM_MOUSEMOVE` + `TrackMouseEvent`，
//!   子类化 BUTTON 实现——ownerdraw 按钮不送 hover 通知，只能自己记）。
//! - **滚动条按需出现**：EDIT 挂 `WS_VSCROLL` 后滚动条常驻；改用
//!   [`update_vscroll`]（行数 × 行高 > 客户区高才 `ShowScrollBar`）在内容 /
//!   尺寸变化时重算——超内容才出现，输入框与对话区行为一致（用户口径）。
//!
//! # 消息块之间的空行（2026-09-30 用户反馈「依然 2 个空行」）
//!
//! 旧实现只把 **AI 回复** trim，块与块之间靠「上一块尾部 `\n\n`」维持间隔——一旦
//! 上一块尾部带多余换行（最常见：用户在输入框按了 Enter 再 Ctrl+Enter 发送，
//! 正文自带尾换行），就会叠出第 2 个空行。新实现反过来：[`append_message`] 先把
//! 既有文本**尾部所有换行剥干净**，非空则补恰好两个 `\r\n`（= 一个空行），再写
//! 块正文（块正文不带尾换行）——无论上一块长什么样，块间恒为一个空行。
//!
//! # 交互约定
//!
//! 输入框 `Ctrl+Enter` 发送（子类化 EDIT，仿 `sticky.rs` 的 `edit_wndproc` 链：原过程
//! 存 `GWLP_USERDATA`），`Esc` 关窗；多轮上下文在窗口内自持，关窗即弃；「清除」清空
//! 对话区与历史（等待回复时忽略，避免回复追加进已清空的历史造成 user/assistant 错位）。
//!
//! 状态（配置 + 历史 + 忙标记）挂**线程局部**：窗、泵、子类过程全在调用线程上
//! 派发，thread_local 即可；同线程重复呼出（热键二连）把已开的窗提到前台，不重开。

use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, GetDC,
    GetTextMetricsW, InvalidateRect, MapWindowPoints, ReleaseDC, SelectObject, SetBkColor,
    SetBkMode, SetTextColor, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_NOCLIP, DT_SINGLELINE, HBRUSH, HDC,
    HGDIOBJ, OPAQUE, PAINTSTRUCT, TEXTMETRICW, TRANSPARENT,
};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_CONTROL, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetDlgCtrlID,
    GetDlgItem, GetMessageW, GetParent, GetWindowLongPtrW, IsWindow, KillTimer, MoveWindow,
    PostMessageW, RegisterClassExW, SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowTextW,
    TranslateMessage, BS_OWNERDRAW, CS_HREDRAW, CS_VREDRAW, ES_AUTOVSCROLL, ES_LEFT, ES_MULTILINE,
    ES_READONLY, ES_WANTRETURN, GWLP_USERDATA, GWLP_WNDPROC, HMENU, MSG, WINDOW_STYLE, WM_DRAWITEM,
    WM_MOUSEMOVE, WM_SETFONT, WNDCLASSEXW, WNDCLASS_STYLES, WS_CHILD, WS_OVERLAPPEDWINDOW,
    WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

/// 客户区尺寸（物理像素；布局全按客户区相对算，外框只要差不多即可）。
const CLIENT_W: i32 = 560;
const CLIENT_H: i32 = 640;
/// 内边距 / 控件尺寸 / 条带间隔。
const PAD: i32 = 8;
const BTN_W: i32 = 84;
const BTN_H: i32 = 28;
const INPUT_H: i32 = 110;
const BAND_GAP: i32 = 6;

/// 控件 ID（`HMENU` 低 16 位）。
const CI_DISPLAY: i32 = 1; // 顶部对话区（只读）
const CI_BTN_CLEAR: i32 = 2; // 「清除」
const CI_BTN_SEND: i32 = 3; // 「发送」
const CI_EDIT: i32 = 4; // 底部输入框
const CI_BTN_SAVE: i32 = 5; // 「保存…」：回复落盘（2026-10-08）
const CI_BTN_COPY: i32 = 6; // 「复制」：回复进剪贴板（2026-10-08）

/// 开窗时对话区的**欢迎段**（2026-09-30 用户要求：进窗先知道这窗口是干嘛的）。
/// ★ 只上屏、不入 [`ChatState::history`]——`build_body` 只发 history，欢迎语不会
/// 被当作对话上下文发给模型；「清除」清空后重新显示这一段。
const WELCOME: &str = "── CurSeen AI 写脚本 ──\n\
我可以帮你编写 CurSeen 的 Lua 脚本与虚拟键盘方案，直接说需求即可，例如：\n\
· 鼠标拖尾特效（保存到 assets/lua/MouseTrail/）\n\
· 点击波纹特效（assets/lua/ClickFx/）\n\
· 悬浮标（assets/lua/HoverDot/）\n\
· 复合脚本：面板参数 / 热键 / AI 对话等（assets/lua/scripts/）\n\
· 虚拟键盘配色方案 JSON（assets/VirtualKeyboards/）\n\
回复会给出完整文件内容并说明保存位置；「保存…」一键把回复存成文件（目录/文件名\
按回复里点名的路径预填），「复制」一键进剪贴板。多轮对话，「清除」一键清空。";

/// 自定义消息（`WM_APP` = 0x8000 起）。
const WM_APP_CHAT_RESULT: u32 = 0x8000 + 1; // wparam = Box<(status, body)>
const WM_APP_CHAT_SEND: u32 = 0x8000 + 2; // 子类过程 Ctrl+Enter 投递
/// 标题栏读秒定时器 ID。
const CHAT_TIMER: usize = 1;

/// 对话区字号：对齐面板 `FONT_MD(14)`（界面只有一种字号的用户口径）。
const CHAT_FONT_PX: f32 = 14.0;

/// 配色（2026-09-30 定稿：全黑正文 + 纯黑框线；platform 层不得反向引 `ui`，
/// 故字面抄值并注明出处）。
///
/// ★★ 通道顺序是 **`R,G,B,A`**（= `core::color::Rgba::to_array` 的 8 位字节序，
/// **不是** `A,R,G,B`）。[`win_util::colorref`] 按 `c[0]=R, c[1]=G, c[2]=B` 组装
/// COLORREF（`0x00BBGGRR`），末位 A 被忽略；一旦写成 `A,R,G,B`，近黑会变亮红——
/// 它的症状是「颜色看着怪」**而不是报错**，肉眼极易放过去（2026-09-29 真机探针
/// 实测过一例）。改动这里任何一色后，跑 unit test `chat_colors_map_to_expected_colorrefs`。
const CHAT_BG: [u8; 4] = [255, 255, 255, 255]; // 窗底白（tokens::BG）
const CHAT_TEXT: [u8; 4] = [45, 52, 54, 255]; // 正文近黑（tokens::TEXT），全部消息一种色
const CHAT_FRAME: [u8; 4] = [0, 0, 0, 255]; // 框线纯黑（用户要求与按钮框一致）
const CHAT_HOVER: [u8; 4] = [232, 236, 240, 255]; // 按钮悬停浅灰
const CHAT_PRESSED: [u8; 4] = [216, 222, 228, 255]; // 按钮按下灰

/// 线程级 GDI 五件套（字体 + 白/黑/悬停/按下四把刷子）的裸句柄。
struct ChatGdi {
    font: isize,
    white: isize,
    frame: isize,
    hover: isize,
    pressed: isize,
}

/// 原 EDIT / BUTTON 窗口过程的裸类型（子类化后要自己往回调用，仿 `sticky.rs`）。
type RawWndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 多轮历史上限（完整轮 = user+assistant）：超限丢最旧。与 pecia 的
/// `kMaxContextTurns = 50` 同口径；显示区另有 2M 字符护栏，这里管的是请求体。
const HISTORY_MAX_TURNS: usize = 50;

/// 历史入账后按轮数淘汰最旧（一"user"一"assistant"算一轮）。
fn trim_history(history: &mut Vec<(String, String)>) {
    let max = HISTORY_MAX_TURNS * 2;
    if history.len() > max {
        let drop = history.len() - max;
        history.drain(..drop);
    }
}

/// 一窗之内的全部可变状态（线程局部：窗与泵同线程）。
struct ChatState {
    url: String,
    key: String,
    model: String,
    sys: String,
    /// 多轮历史，只含 user / assistant（system 每次请求现拼）。
    /// ★ 上限 [`HISTORY_MAX_TURNS`] 轮（超限丢最旧）：历史会被整体序列化进每次
    /// 请求体，无上限 = 长会话内存与请求体线性膨胀（泄露审计 2026-09-30）。
    history: Vec<(String, String)>,
    /// 有请求在飞（期间禁再发、忽略清除）。
    busy: bool,
    /// 标题栏读秒计数。
    ticks: u32,
}

/// ★ 当前 AI 对话窗句柄（**进程级**）。
///
/// ★★ 为什么必须是进程级：本窗由 `run_ext_detached` 每次新 spawn 的线程调用，而
/// `ChatState` 的载体是 thread_local —— 新线程上那份永远为空。"同一个进程里同时只该
/// 有一个 AI 对话窗"这条只能落在跨线程可见的地方。
static CHAT_HWND: AtomicIsize = AtomicIsize::new(0);

thread_local! {
    static CHAT: RefCell<Option<ChatState>> = const { RefCell::new(None) };
    /// 线程级 GDI 五件套：懒建一次复用；**泵结束（关窗）时整体 DeleteObject 回收**。
    /// ★ 本窗的调用线程是 `run_ext_detached` 每次新 spawn 的（ext_runner.rs），线程
    /// 用完即退——若只建不删，每次呼出泄 5 个 GDI 句柄（GDI 句柄是进程级的，线程
    /// 退出不回收；进程上限 10000，约 2000 次呼出耗尽）。2026-09-30 泄露审计修复。
    static GDI: RefCell<Option<ChatGdi>> = const { RefCell::new(None) };
    /// 当前悬停的按钮 ID（ownerdraw 无 hover 通知，子类过程自己记）。
    static HOVER: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
}

/// UTF-16 以 NUL 结尾（Win32 字符串实参）。
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 取线程级 GDI 五件套，没有就建。
fn chat_gdi() -> ChatGdi {
    GDI.with(|g| {
        let mut g = g.borrow_mut();
        if g.is_none() {
            let font = crate::platform::sticky::make_font(CHAT_FONT_PX);
            // SAFETY：GDI 调用，参数都是常量色值。
            let br = |c: [u8; 4]| unsafe {
                CreateSolidBrush(crate::platform::win_util::colorref(c)).0 as isize
            };
            *g = Some(ChatGdi {
                font: font.0 as isize,
                white: br(CHAT_BG),
                frame: br(CHAT_FRAME),
                hover: br(CHAT_HOVER),
                pressed: br(CHAT_PRESSED),
            });
        }
        match g.as_ref() {
            Some(v) => ChatGdi {
                font: v.font,
                white: v.white,
                frame: v.frame,
                hover: v.hover,
                pressed: v.pressed,
            },
            None => ChatGdi {
                font: 0,
                white: 0,
                frame: 0,
                hover: 0,
                pressed: 0,
            },
        }
    })
}

/// 释放线程级 GDI 五件套（关窗后调用；句柄进程级，必须显式 DeleteObject）。
fn release_chat_gdi() {
    let taken = GDI.with(|g| g.borrow_mut().take());
    if let Some(v) = taken {
        // SAFETY：都是本线程 Create 出来的合法 GDI 句柄，各删一次。
        unsafe {
            let _ = DeleteObject(HGDIOBJ(v.font as *mut _));
            let _ = DeleteObject(HGDIOBJ(v.white as *mut _));
            let _ = DeleteObject(HGDIOBJ(v.frame as *mut _));
            let _ = DeleteObject(HGDIOBJ(v.hover as *mut _));
            let _ = DeleteObject(HGDIOBJ(v.pressed as *mut _));
        }
    }
}

/// 作用域守卫：无论 `chat_window` 从哪条路径返回（含建窗失败的 early return），都在
/// 当前线程销毁时回收线程级 GDI 五件套。比在末尾手工 `release_chat_gdi()` 更稳——不会漏掉
/// 任何新加的提前返回分支（泄露审计 2026-09-30 复查补强）。
struct GdiGuard;
impl Drop for GdiGuard {
    fn drop(&mut self) {
        release_chat_gdi();
    }
}

/// 当前时刻 `HH:MM:SS`（消息块时间戳；chrono 已是项目既有依赖）。消息块带秒级
/// 时刻：秒级能直接看出请求耗时。
fn now_hms() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// 呼出 AI 对话窗（阻塞至关窗；与 `ext:result` / `ext:input` 同一条线程语义）。
pub fn chat_window(owner_hwnd: isize, url: &str, model: &str, key: &str, sys: &str) {
    // ★★ 已有对话窗（**任意线程**开的）→ 只把它叫回前台，不重开第二个。
    //   进程级句柄而不是 thread_local `CHAT`：本函数由 `run_ext_detached` 每次
    //   **新 spawn 的线程**调用，线程局部状态在新线程里恒为空 —— 旧实现的去重
    //   等于没写，表现为"重复按热键冒出一串窗口"（2026-09-30 用户报告）。
    //   只登记句柄：窗口属于创建它的线程，别的线程不该去读写它的 `ChatState`。
    //   ★ 用句柄而不是标题做判据：等回复时标题是「AI 写脚本 (Ns)」，`FindWindowW`
    //   全等匹配会漏判（漏判不报错，只会默默再开一个）。
    let existing = CHAT_HWND.load(Ordering::SeqCst);
    if existing != 0 {
        if crate::platform::win_util::focus_window_hwnd(existing) {
            return;
        }
        CHAT_HWND.store(0, Ordering::SeqCst); // 窗已销毁，句柄是残留的。
    }

    register_class();
    chat_gdi(); // 预热线程级字体/刷子（首个 WM_CTLCOLORED* 到来前就绪）。
    let _gdi_guard = GdiGuard; // 任何退出路径都回收 GDI 五件套（含下方建窗失败 return）。

    // 外框 ≈ 客户区 + 边框/标题栏（粗估值；内部布局全按真实客户区相对排）。
    let (ww, wh) = (CLIENT_W + 16, CLIENT_H + 52);
    let (vx, vy, vw, vh) = crate::platform::win_util::virtual_screen_rect();
    let wx = vx + (((vw - ww) / 2).max(0));
    let wy = vy + (((vh - wh) / 2).max(0));

    let title_w = wide("AI 写脚本");
    let class_w = wide("CurSeen.AiChat");
    let owner = (owner_hwnd != 0).then_some(HWND(owner_hwnd as *mut _));

    let hwnd = unsafe {
        CreateWindowExW(
            Default::default(),
            PCWSTR(class_w.as_ptr()),
            PCWSTR(title_w.as_ptr()),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            wx,
            wy,
            ww,
            wh,
            owner,
            None,
            None,
            None,
        )
    }
    .unwrap_or_default();
    if hwnd.0.is_null() {
        return;
    }

    create_controls(hwnd);
    layout_chat(hwnd);
    // ★ 滚动条按需显隐要在开局算一次：CreateWindowExW 自带的 WM_SIZE 早于
    // create_controls（控件还不存在），之后没人再触发——漏了这条，输入框空着
    // 也会挂一根常驻滚动条（2026-09-30 探针实测）。
    update_scrollbars(hwnd);
    // 开窗先亮出「这个窗口能做什么」（只上屏不入 history，见 WELCOME 注释）。
    append_message(hwnd, CI_DISPLAY, WELCOME);

    CHAT_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
    CHAT.with(|c| {
        *c.borrow_mut() = Some(ChatState {
            url: url.to_string(),
            key: key.to_string(),
            model: model.to_string(),
            sys: sys.to_string(),
            history: Vec::new(),
            busy: false,
            ticks: 0,
        })
    });

    // 嵌套消息泵：窗口销毁即结束（不 PostQuitMessage，避免泄 WM_QUIT）。
    let mut msg = MSG::default();
    loop {
        let ok = unsafe { GetMessageW(&mut msg, None, 0, 0).as_bool() };
        if !ok {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            let _ = DispatchMessageW(&msg);
        }
        if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            break;
        }
    }
    CHAT.with(|c| *c.borrow_mut() = None);
    CHAT_HWND.store(0, Ordering::SeqCst); // ★ 必须放回 0：否则下一次呼出会去找一个
                                          //   已经销毁的句柄（`focus_window_hwnd` 会返回 false 再兜一层，但别依赖兜底）。
                                          // GDI 五件套的回收由 `_gdi_guard` 在函数末尾的 Drop 负责（覆盖全部退出路径）。
}

/// 注册窗口类（进程内一次）。★ 背景刷必须是**实刷**：NULL 刷不擦底，拉伸时旧像素
/// 残留 + 控件默认灰（COLOR_3DFACE）透出来。
fn register_class() {
    static REG: std::sync::Once = std::sync::Once::new();
    REG.call_once(|| {
        let cname = wide("CurSeen.AiChat");
        let bg = unsafe { CreateSolidBrush(crate::platform::win_util::colorref(CHAT_BG)) };
        // `..Default::default()` 照抄 sticky.rs 的注册写法（hInstance 等零值即可）。
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: WNDCLASS_STYLES(CS_HREDRAW.0 | CS_VREDRAW.0),
            lpfnWndProc: Some(chat_proc),
            // ★ 实刷背景：拉伸时 WM_ERASEBKGND 真擦底，旧像素不再残留。
            //   （0.62 里该字段是裸 HBRUSH，非 Option——NULL 即 HBRUSH::default()。）
            hbrBackground: bg,
            lpszClassName: PCWSTR(cname.as_ptr()),
            ..Default::default()
        };
        unsafe {
            let _ = RegisterClassExW(&wc);
        }
    });
}

/// 建四个子控件（对话区 / 清除 / 发送 / 输入框），统一雅黑 14（=面板字号口径）。
///
/// ★ 两个 EDIT 都**不带 WS_BORDER**：系统边框是凹陷 3D 斜面（2026-09-30 用户反馈
/// 「丑」），平面黑框由主窗 `WM_PAINT` 统一外画（见 [`draw_flat_frames`]）。
fn create_controls(hwnd: HWND) {
    let edit_cls = wide("EDIT");
    let btn_cls = wide("BUTTON");
    let clear_w = wide("清除");
    let send_w = wide("发送");
    let save_w = wide("保存…");
    let copy_w = wide("复制");
    let display_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_LEFT) as u32,
    );
    // BS_OWNERDRAW：按钮自绘（WM_DRAWITEM 分支），系统立体阴影被去掉；黑框 +
    // 悬停变色由自绘与子类过程配合实现。
    let btn_style = WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | (BS_OWNERDRAW as u32));
    let edit_style = WINDOW_STYLE(
        WS_CHILD.0
            | WS_VISIBLE.0
            | WS_VSCROLL.0
            | WS_TABSTOP.0
            | (ES_MULTILINE | ES_WANTRETURN | ES_AUTOVSCROLL | ES_LEFT) as u32,
    );
    let (font, _, _, _, _) = {
        let g = chat_gdi();
        (g.font, g.white, g.frame, g.hover, g.pressed)
    };
    unsafe {
        let made: Vec<HWND> = [
            CreateWindowExW(
                Default::default(),
                PCWSTR(edit_cls.as_ptr()),
                PCWSTR::null(),
                display_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_DISPLAY as usize as *mut _)),
                None,
                None,
            ),
            CreateWindowExW(
                Default::default(),
                PCWSTR(btn_cls.as_ptr()),
                PCWSTR(clear_w.as_ptr()),
                btn_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_BTN_CLEAR as usize as *mut _)),
                None,
                None,
            ),
            CreateWindowExW(
                Default::default(),
                PCWSTR(btn_cls.as_ptr()),
                PCWSTR(send_w.as_ptr()),
                btn_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_BTN_SEND as usize as *mut _)),
                None,
                None,
            ),
            CreateWindowExW(
                Default::default(),
                PCWSTR(edit_cls.as_ptr()),
                PCWSTR::null(),
                edit_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_EDIT as usize as *mut _)),
                None,
                None,
            ),
            CreateWindowExW(
                Default::default(),
                PCWSTR(btn_cls.as_ptr()),
                PCWSTR(save_w.as_ptr()),
                btn_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_BTN_SAVE as usize as *mut _)),
                None,
                None,
            ),
            CreateWindowExW(
                Default::default(),
                PCWSTR(btn_cls.as_ptr()),
                PCWSTR(copy_w.as_ptr()),
                btn_style,
                0,
                0,
                0,
                0,
                Some(hwnd),
                Some(HMENU(CI_BTN_COPY as usize as *mut _)),
                None,
                None,
            ),
        ]
        .into_iter()
        .filter_map(|r| r.ok())
        .collect();
        for w in &made {
            let _ = SendMessageW(*w, WM_SETFONT, Some(WPARAM(font as usize)), Some(LPARAM(1)));
        }
        // 对话区上限 2M 字符（AI 长回复不会被默认上限掐断）。
        if let Ok(disp) = GetDlgItem(Some(hwnd), CI_DISPLAY) {
            let _ = SendMessageW(
                disp,
                0x00C5, // EM_SETLIMITTEXT
                Some(WPARAM(2_000_000)),
                Some(LPARAM(0)),
            );
        }
        // 子类化：输入 EDIT 截 Ctrl+Enter / Esc；四个按钮记 hover（ownerdraw 无
        // hover 通知）。原过程都存 GWLP_USERDATA。
        for id in [CI_EDIT, CI_BTN_CLEAR, CI_BTN_SEND, CI_BTN_SAVE, CI_BTN_COPY] {
            if let Ok(c) = GetDlgItem(Some(hwnd), id) {
                let proc = if id == CI_EDIT {
                    chat_edit_proc
                } else {
                    chat_btn_proc
                };
                let prev = SetWindowLongPtrW(c, GWLP_WNDPROC, proc as *const () as usize as isize);
                SetWindowLongPtrW(c, GWLP_USERDATA, prev);
            }
        }
    }
}

/// 依真实客户区排布：对话区填顶部，按钮条居中带，输入框压底。
fn layout_chat(hwnd: HWND) {
    unsafe {
        let mut rc = RECT::default();
        if GetClientRect(hwnd, &mut rc).is_err() {
            return;
        }
        let (cw, ch) = (rc.right, rc.bottom);
        // 四个按钮：左侧「保存…」「复制」（作用对象 = AI 回复），右侧「清除」「发送」。
        let min_w = BTN_W * 4 + PAD * 2 + BAND_GAP * 3;
        if cw < min_w || ch < BTN_H + INPUT_H + PAD * 2 + BAND_GAP * 2 {
            return;
        }
        let disp_h = ch - PAD * 2 - BTN_H - INPUT_H - BAND_GAP * 2;
        let band_y = PAD + disp_h + BAND_GAP;
        let input_y = band_y + BTN_H + BAND_GAP;
        if let Ok(disp) = GetDlgItem(Some(hwnd), CI_DISPLAY) {
            let _ = MoveWindow(disp, PAD, PAD, cw - PAD * 2, disp_h, true);
        }
        if let Ok(clear) = GetDlgItem(Some(hwnd), CI_BTN_CLEAR) {
            let _ = MoveWindow(
                clear,
                cw - PAD - BTN_W * 2 - BAND_GAP,
                band_y,
                BTN_W,
                BTN_H,
                true,
            );
        }
        if let Ok(send) = GetDlgItem(Some(hwnd), CI_BTN_SEND) {
            let _ = MoveWindow(send, cw - PAD - BTN_W, band_y, BTN_W, BTN_H, true);
        }
        if let Ok(save) = GetDlgItem(Some(hwnd), CI_BTN_SAVE) {
            let _ = MoveWindow(save, PAD, band_y, BTN_W, BTN_H, true);
        }
        if let Ok(copy) = GetDlgItem(Some(hwnd), CI_BTN_COPY) {
            let _ = MoveWindow(copy, PAD + BTN_W + BAND_GAP, band_y, BTN_W, BTN_H, true);
        }
        if let Ok(edit) = GetDlgItem(Some(hwnd), CI_EDIT) {
            let _ = MoveWindow(edit, PAD, input_y, cw - PAD * 2, INPUT_H, true);
        }
    }
}

/// 主窗过程。消息号沿用 win_util 原生框的数字风格（0x0111=WM_COMMAND 等）。
extern "system" fn chat_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            // WM_SIZE：用户拖拽缩放 → 重排 + 重画框线 + 滚动条按需重算。
            0x0005 => {
                layout_chat(hwnd);
                update_scrollbars(hwnd);
                let _ = InvalidateRect(Some(hwnd), None, true);
                LRESULT(0)
            }
            // WM_PAINT：擦底之后给两个 EDIT 外画 1px 纯黑平面框（子控件无系统边框）。
            0x000F => {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                if !dc.is_invalid() {
                    draw_flat_frames(hwnd, dc);
                    let _ = EndPaint(hwnd, &ps);
                }
                LRESULT(0)
            }
            // WM_COMMAND：清除 / 发送按钮；输入框内容变化 → 滚动条按需重算。
            0x0111 => {
                let id = (wparam.0 & 0xffff) as i32;
                let note = (wparam.0 >> 16) as i32;
                if id == CI_BTN_CLEAR {
                    chat_clear(hwnd);
                } else if id == CI_BTN_SEND {
                    chat_send(hwnd);
                } else if id == CI_BTN_SAVE {
                    chat_save_reply(hwnd);
                } else if id == CI_BTN_COPY {
                    chat_copy_reply(hwnd);
                } else if id == CI_EDIT && note == 0x0300 {
                    // EN_CHANGE：输入内容变化 → 滚动条按需出现/隐藏。
                    if let Ok(edit) = GetDlgItem(Some(hwnd), CI_EDIT) {
                        update_vscroll(edit);
                    }
                }
                LRESULT(0)
            }
            // WM_DRAWITEM：BS_OWNERDRAW 按钮自绘——平面底 + 1px 纯黑描边 + 居中文字；
            // 悬停浅灰、按下深灰（悬停状态由 chat_btn_proc 维护）。
            WM_DRAWITEM => {
                let dis = &*(lparam.0 as *const windows::Win32::UI::Controls::DRAWITEMSTRUCT);
                let id = dis.CtlID as i32;
                let label = match id {
                    CI_BTN_CLEAR => "清除",
                    CI_BTN_SEND => "发送",
                    CI_BTN_SAVE => "保存…",
                    CI_BTN_COPY => "复制",
                    _ => "",
                };
                if !label.is_empty() {
                    let g = chat_gdi();
                    let dc = dis.hDC;
                    let pressed =
                        (dis.itemState.0 & windows::Win32::UI::Controls::ODS_SELECTED.0) != 0;
                    let hovered = HOVER.with(|h| h.borrow().contains(&id));
                    let fill = if pressed {
                        g.pressed
                    } else if hovered {
                        g.hover
                    } else {
                        g.white
                    };
                    let _ = FillRect(dc, &dis.rcItem, HBRUSH(fill as *mut _));
                    let _ = FrameRect(dc, &dis.rcItem, HBRUSH(g.frame as *mut _));
                    let mut buf = wide(label);
                    // ★ 测宽/绘制都掐掉尾部 NUL：DT_CALCRECT 会把 NUL 的字宽也量进
                    //   去（雅黑 14px 下 ≈9px），文字整体左偏——2026-09-30 探针实测
                    //   「左22/右31」就是这么来的。
                    let n = label.encode_utf16().count();
                    let _ = SetBkMode(dc, TRANSPARENT);
                    let _ = SetTextColor(dc, crate::platform::win_util::colorref(CHAT_TEXT));
                    let old = SelectObject(dc, HGDIOBJ(g.font as *mut _));
                    // ★ 手动居中而不是 DT_CENTER|DT_VCENTER：CJK 字体（雅黑）的「行盒」
                    //   含大量上下内边距，DT_VCENTER 按**行盒**居中，字形视觉偏高——
                    //   用户报「要水平垂直都居中」正是它。先用 DT_CALCRECT 量出文字
                    //   实际包围盒，再按包围盒几何居中，对任何字体都稳。
                    let mut mrc = dis.rcItem;
                    let _ = DrawTextW(
                        dc,
                        &mut buf[..n],
                        &mut mrc,
                        DRAW_TEXT_FORMAT(DT_CALCRECT.0 | DT_SINGLELINE.0),
                    );
                    let (tw, th) = (mrc.right - mrc.left, mrc.bottom - mrc.top);
                    let (bw, bh) = (
                        dis.rcItem.right - dis.rcItem.left,
                        dis.rcItem.bottom - dis.rcItem.top,
                    );
                    let mut rc = dis.rcItem;
                    rc.left += ((bw - tw) / 2).max(0);
                    rc.top += ((bh - th) / 2).max(0);
                    rc.right = rc.left + tw;
                    rc.bottom = rc.top + th;
                    let _ = DrawTextW(
                        dc,
                        &mut buf[..n],
                        &mut rc,
                        // DT_LEFT/DT_TOP = 0（默认对齐），画在手动定位的 rc 左上即可。
                        DRAW_TEXT_FORMAT(DT_SINGLELINE.0 | DT_NOCLIP.0),
                    );
                    SelectObject(dc, old);
                }
                LRESULT(1)
            }
            // 子类过程投递的发送（Ctrl+Enter）。
            WM_APP_CHAT_SEND => {
                chat_send(hwnd);
                LRESULT(0)
            }
            // worker 线程回传结果（wparam = Box<(status, body)> 所有权）。
            WM_APP_CHAT_RESULT => {
                chat_result(hwnd, wparam);
                LRESULT(0)
            }
            // WM_TIMER：标题栏读秒。
            0x0113 => {
                if wparam.0 == CHAT_TIMER {
                    chat_tick(hwnd);
                }
                LRESULT(0)
            }
            // 0x0133=WM_CTLCOLOREDIT、0x0138=WM_CTLCOLORSTATIC：接管子控件配色
            //（只读 EDIT 走 STATIC 分支）。
            // ★★ 必须 **OPAQUE**（sticky 的 TRANSPARENT 在这里会出大事）：EDIT 打字 /
            //   退格 / 滚动后只**部分重绘**受影响的行，OPAQUE 模式下每行文字自带
            //   「字底矩形」把旧字形擦掉；TRANSPARENT 不擦 → 删字后旧字残留、滚动后
            //   新旧行重叠（2026-09-30 用户报的就是这两条）。SetBkColor 给白底即可。
            0x0133 | 0x0138 => {
                let dc = HDC(wparam.0 as *mut _);
                let g = chat_gdi();
                let _ = SetBkColor(dc, crate::platform::win_util::colorref(CHAT_BG));
                let _ = SetTextColor(dc, crate::platform::win_util::colorref(CHAT_TEXT));
                let _ = SetBkMode(dc, OPAQUE);
                LRESULT(g.white)
            }
            // WM_KEYDOWN：Esc 关窗（主窗拿到焦点的场合）。
            0x0100 if wparam.0 == VK_ESCAPE.0 as usize => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            // WM_CLOSE：标题栏 X / Alt+F4。
            0x0010 => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            // WM_DESTROY：静默（泵靠 IsWindow 判活结尾）。
            0x0002 => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

/// 给对话区 / 输入框外画 1px 纯黑平面框（控件自身无系统边框）。
///
/// 框画在控件矩形**外侧 1px 环**上：`FrameRect` 描的是传入矩形的边界，把矩形
/// 从控件矩形向四周扩 1px，黑线正好落在控件与父窗背景的交界，不占控件客户区。
fn draw_flat_frames(hwnd: HWND, dc: HDC) {
    let g = chat_gdi();
    for id in [CI_DISPLAY, CI_EDIT] {
        unsafe {
            let Ok(c) = GetDlgItem(Some(hwnd), id) else {
                continue;
            };
            if !IsWindow(Some(c)).as_bool() {
                continue;
            }
            let mut rc = RECT::default();
            if GetClientRect(c, &mut rc).is_err() {
                continue;
            }
            // 控件客户区 → 父窗客户区坐标（两点）。
            let mut pts = [
                POINT {
                    x: rc.left,
                    y: rc.top,
                },
                POINT {
                    x: rc.right,
                    y: rc.bottom,
                },
            ];
            MapWindowPoints(Some(c), Some(hwnd), &mut pts);
            let frame = RECT {
                left: pts[0].x - 1,
                top: pts[0].y - 1,
                right: pts[1].x + 1,
                bottom: pts[1].y + 1,
            };
            let _ = FrameRect(dc, &frame, HBRUSH(g.frame as *mut _));
        }
    }
}

/// 滚动条按需出现：行数 × 行高 > 客户区高才显示竖向滚动条，否则隐藏。
///
/// EDIT 挂 `WS_VSCROLL` 后滚动条常驻；`ShowScrollBar(false)` 能在保留样式位的
/// 前提下把它藏掉——内容超窗时再放出来（2026-09-30 用户口径「只有超出窗口时
/// 才出现」）。
fn update_vscroll(h: HWND) {
    unsafe {
        let dc = GetDC(Some(h));
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(dc, &mut tm);
        let _ = ReleaseDC(Some(h), dc);
        let mut rc = RECT::default();
        if GetClientRect(h, &mut rc).is_err() {
            return;
        }
        let line_h = tm.tmHeight.max(1);
        let visible = (((rc.bottom - rc.top) / line_h).max(1)) as usize;
        let lines = SendMessageW(
            h,
            0x00BA, /* EM_GETLINECOUNT */
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        )
        .0
        .max(0) as usize;
        let _ = windows::Win32::UI::Controls::ShowScrollBar(
            h,
            windows::Win32::UI::WindowsAndMessaging::SB_VERT,
            lines > visible,
        );
    }
}

/// 两个 EDIT 的滚动条一起按需重算（WM_SIZE 用）。
fn update_scrollbars(hwnd: HWND) {
    for id in [CI_DISPLAY, CI_EDIT] {
        unsafe {
            if let Ok(c) = GetDlgItem(Some(hwnd), id) {
                update_vscroll(c);
            }
        }
    }
}

/// 按钮（ownerdraw）子类过程：只记 hover（WM_MOUSEMOVE 进 / WM_MOUSELEAVE 出），
/// 状态进 [`HOVER`] 供 WM_DRAWITEM 取用，其余全回原过程。
extern "system" fn chat_btn_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let id = GetDlgCtrlID(hwnd);
        match msg {
            WM_MOUSEMOVE => {
                // WM_MOUSEMOVE：从非悬停进入悬停 → 记状态 + 请求 WM_MOUSELEAVE + 重画。
                let entered = HOVER.with(|h| {
                    let mut b = h.borrow_mut();
                    if b.contains(&id) {
                        false
                    } else {
                        b.push(id);
                        true
                    }
                });
                if entered {
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    let _ = TrackMouseEvent(&mut tme);
                    let _ = InvalidateRect(Some(hwnd), None, true);
                }
            }
            // ★ 常量必须用 `WM_MOUSELEAVE`(0x02A3)：曾手写 0x02A2，那是
            //   `WM_NCMOUSELEAVE`（非客户区），永远收不到 → 悬停变色后不恢复。
            WM_MOUSELEAVE => {
                // WM_MOUSELEAVE：移出 → 清状态 + 重画（回到白底黑框）。
                HOVER.with(|h| h.borrow_mut().retain(|&x| x != id));
                let _ = InvalidateRect(Some(hwnd), None, true);
            }
            _ => {}
        }
        let prev = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if prev == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let f: RawWndProc = std::mem::transmute(prev as usize as *const ());
        f(hwnd, msg, wparam, lparam)
    }
}

/// 输入 EDIT 的子类过程：只截 Ctrl+Enter（发送）与 Esc（关窗），其余回原过程。
extern "system" fn chat_edit_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        // ★ GetKeyState 在 windows 0.62 里收 i32（VIRTUAL_KEY 的 .0 是 u16，要转）。
        if msg == 0x0100 && wparam.0 == VK_RETURN.0 as usize && GetKeyState(VK_CONTROL.0 as i32) < 0
        {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = PostMessageW(Some(parent), WM_APP_CHAT_SEND, WPARAM(0), LPARAM(0));
            }
            return LRESULT(0);
        }
        if msg == 0x0100 && wparam.0 == VK_ESCAPE.0 as usize {
            if let Ok(parent) = GetParent(hwnd) {
                let _ = DestroyWindow(parent);
            }
            return LRESULT(0);
        }
        let prev = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if prev == 0 {
            return DefWindowProcW(hwnd, msg, wparam, lparam);
        }
        let f: RawWndProc = std::mem::transmute(prev as usize as *const ());
        f(hwnd, msg, wparam, lparam)
    }
}

/// 发送：取输入 → trim → 上屏（带发送时刻）→ 历史入账 → 起读秒 → worker 发 HTTP。
fn chat_send(hwnd: HWND) {
    let text = read_ctrl_text(hwnd, CI_EDIT);
    // ★ trim 是「两个空行」修复的另一半：用户按 Enter 再发送时正文自带尾换行，
    // 上屏 / 入历史都掐掉（块间空行由 append_message 统一保证）。
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    // URL / 模型为空直接人话报错，不发请求（Lua 侧已兜默认值，这里是最后防线——
    // 2026-09-29 实测：地址被删空保存后 ureq 对空 URI 报 "http: empty string"，
    // 用户会误以为是 key 的问题）。
    let empty_cfg = CHAT.with(|c| {
        c.borrow()
            .as_ref()
            .map(|st| st.url.trim().is_empty() || st.model.trim().is_empty())
            .unwrap_or(true)
    });
    if empty_cfg {
        append_message(
            hwnd,
            CI_DISPLAY,
            "（未发送）API 地址 / 模型为空——请在面板「脚本」页展开「AI 写脚本」卡片填好这两栏再问。",
        );
        return;
    }
    // 忙态检查 + 历史入账 + 组请求体（一把锁内完成，忙时直接忽略）。
    let ready = CHAT.with(|c| {
        let mut b = c.borrow_mut();
        match b.as_mut() {
            Some(st) if !st.busy => {
                st.busy = true;
                st.history.push(("user".to_string(), text.clone()));
                trim_history(&mut st.history);
                Some((
                    st.url.clone(),
                    st.key.clone(),
                    build_body(&st.model, &st.sys, &st.history),
                ))
            }
            _ => None,
        }
    });
    let Some((url, key, body)) = ready else {
        return;
    };

    let hm = now_hms();
    append_message(hwnd, CI_DISPLAY, &format!("── {hm} 我 ──\n{text}"));
    unsafe {
        if let Ok(edit) = GetDlgItem(Some(hwnd), CI_EDIT) {
            let blank = wide("");
            let _ = SetWindowTextW(edit, PCWSTR(blank.as_ptr()));
            // ★ WM_SETTEXT 不触发 EN_CHANGE（那只在用户输入时来），程序侧清空要
            // 自己重算滚动条，否则发完消息输入框空着还挂着滚动条。
            update_vscroll(edit);
        }
    }
    // AI 块不占位：回复到达时才整块上屏（块头带到达时刻 `── HH:MM:SS AI ──`）——
    // 占位行拿不到到达时刻，硬留会破坏块头。等待反馈 = 标题读秒。
    // ★ **不禁用发送按钮**（2026-09-30）：`EnableWindow(false)` 后按钮收不到任何
    //   鼠标消息 → 用户报「清除悬停有效果、发送没有」。忙时重复点击本来就由
    //   [`chat_send` 的 busy 守卫] 静默忽略，视觉状态保持一致即可。
    unsafe {
        let _ = SetTimer(Some(hwnd), CHAT_TIMER, 1000, None);
        let t = wide("AI 写脚本 (0s)");
        let _ = SetWindowTextW(hwnd, PCWSTR(t.as_ptr()));
    }

    // worker：只做 HTTP 与回传，不碰任何 UI 状态。
    let hwnd_isize = hwnd.0 as isize;
    let spawned = std::thread::Builder::new()
        .name("ai-chat-http".into())
        .spawn(move || {
            let headers = vec![
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), format!("Bearer {key}")),
            ];
            let (status, resp) = crate::platform::http::post_json(&url, &headers, &body);
            let ptr = Box::into_raw(Box::new((status, resp)));
            let target = HWND(hwnd_isize as *mut _);
            // 窗已关 → PostMessage 失败，兜住内存不漏。
            if unsafe {
                PostMessageW(
                    Some(target),
                    WM_APP_CHAT_RESULT,
                    WPARAM(ptr as usize),
                    LPARAM(0),
                )
            }
            .is_err()
            {
                drop(unsafe { Box::from_raw(ptr) });
            }
        });
    if spawned.is_err() {
        // 线程起不来（极端）：当场解除忙态，给条看得见的错误。
        chat_unbusy(hwnd);
        append_message(hwnd, CI_DISPLAY, "（无法启动请求线程）");
    }
}

/// 收 worker 结果：解除忙态、停读秒、恢复标题，回复/错误上屏（带到达时刻）并入账。
fn chat_result(hwnd: HWND, wparam: WPARAM) {
    let ptr = wparam.0 as *mut (i64, String);
    if ptr.is_null() {
        return;
    }
    let (status, body) = *unsafe { Box::from_raw(ptr) };
    chat_unbusy(hwnd);
    let hm = now_hms();
    match extract_reply(status, &body) {
        Ok(reply) => {
            CHAT.with(|c| {
                let mut b = c.borrow_mut();
                if let Some(st) = b.as_mut() {
                    st.history.push(("assistant".to_string(), reply.clone()));
                    trim_history(&mut st.history);
                }
            });
            // reply.trim()：模型回复常自带前后空行——掐头去尾，块间空行交给
            // append_message 统一保证（2026-09-29 起的口径，2026-09-30 加固）。
            append_message(
                hwnd,
                CI_DISPLAY,
                &format!("── {hm} AI ──\n{}", reply.trim()),
            );
        }
        Err(e) => {
            append_message(hwnd, CI_DISPLAY, &format!("── {hm} AI ──\n请求失败：{e}"));
        }
    }
}

/// 标题栏读秒 +1（`AI 写脚本 (Ns)`）。
fn chat_tick(hwnd: HWND) {
    let n = CHAT.with(|c| {
        let mut b = c.borrow_mut();
        b.as_mut().map(|st| {
            st.ticks += 1;
            st.ticks
        })
    });
    if let Some(n) = n {
        let t = wide(&format!("AI 写脚本 ({n}s)"));
        unsafe {
            let _ = SetWindowTextW(hwnd, PCWSTR(t.as_ptr()));
        }
    }
}

/// 清除：对话区 + 历史（等待回复时忽略——回复会追加，清了会造成历史错位）。
fn chat_clear(hwnd: HWND) {
    let busy = CHAT.with(|c| c.borrow().as_ref().map(|s| s.busy).unwrap_or(false));
    if busy {
        return;
    }
    CHAT.with(|c| {
        let mut b = c.borrow_mut();
        if let Some(st) = b.as_mut() {
            st.history.clear();
        }
    });
    unsafe {
        if let Ok(disp) = GetDlgItem(Some(hwnd), CI_DISPLAY) {
            let blank = wide("");
            let _ = SetWindowTextW(disp, PCWSTR(blank.as_ptr()));
            update_vscroll(disp);
        }
    }
    // 清空后重新亮欢迎段——它是窗口说明，不算对话内容。
    append_message(hwnd, CI_DISPLAY, WELCOME);
}

/// 复制回复：对话区有选区 → 复制选区；否则复制最后一条 AI 回复。成功静默（剪贴板
/// 历史面板可见），无可复制内容时在对话区给提示。
fn chat_copy_reply(hwnd: HWND) {
    let Some(text) = reply_target(hwnd) else {
        append_message(hwnd, CI_DISPLAY, "（还没有可复制的 AI 回复）");
        return;
    };
    crate::platform::clipboard_history::copy_text(&text);
    append_message(hwnd, CI_DISPLAY, "（已复制到剪贴板）");
}

/// 保存回复：另存为对话框落盘。目录 / 文件名按回复里点名的 `assets/...` 路径预填
/// （AI 按欢迎语约定会「说明保存位置」），没有点名时落到 `assets/lua/` + 首个代码
/// 围栏的语言推断扩展名。取消对话框 = 什么都不发生。
fn chat_save_reply(hwnd: HWND) {
    let Some(text) = reply_target(hwnd) else {
        append_message(hwnd, CI_DISPLAY, "（还没有可保存的 AI 回复）");
        return;
    };
    let (dir, name, def_ext) = save_target(&text);
    let Some(d) = dir else {
        return;
    };
    let _ = std::fs::create_dir_all(&d);
    let Some(path) =
        crate::platform::file_dialog::save_text(&def_ext, &name, Some(&d), Some(hwnd.0 as isize))
    else {
        return; // 用户取消 / 对话框失败
    };
    match std::fs::write(&path, text.as_str()) {
        Ok(()) => append_message(
            hwnd,
            CI_DISPLAY,
            &format!("（已保存到 {}）", path.display()),
        ),
        Err(e) => append_message(hwnd, CI_DISPLAY, &format!("（保存失败：{e}）")),
    }
}

/// 保存 / 复制的作用对象：对话区当前选区（UTF-16 偏移）优先，否则最后一条 AI 回复。
/// 两者皆无 → None（调用方提示）。
fn reply_target(hwnd: HWND) -> Option<String> {
    unsafe {
        if let Ok(w) = GetDlgItem(Some(hwnd), CI_DISPLAY) {
            let (mut start, mut end): (u32, u32) = (0, 0);
            let _ = SendMessageW(
                w,
                0x00B0, /* EM_GETSEL */
                Some(WPARAM(&mut start as *mut u32 as usize)),
                Some(LPARAM(&mut end as *mut u32 as isize)),
            );
            if end > start {
                let full = read_ctrl_text(hwnd, CI_DISPLAY);
                return Some(utf16_slice(&full, start as usize, end as usize));
            }
        }
    }
    CHAT.with(|c| c.borrow().as_ref().and_then(|st| last_reply(&st.history)))
}

/// 最后一条 AI 回复（trim 掉前后空白；与上屏口径一致）。
fn last_reply(history: &[(String, String)]) -> Option<String> {
    history
        .iter()
        .rev()
        .find(|(role, _)| role == "assistant")
        .map(|(_, content)| content.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 按 UTF-16 单位切字符串（Win32 `EM_GETSEL` 的位置语义；越界截断到实际长度）。
fn utf16_slice(s: &str, start: usize, end: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    let a = start.min(units.len());
    let b = end.min(units.len()).max(a);
    String::from_utf16_lossy(&units[a..b])
}

/// 允许落盘的扩展名（AI 会产出的文本类产物；可执行/二进制一律不收）。
const SAVE_EXTS: [&str; 6] = ["lua", "json", "md", "txt", "html", "ini"];

/// 从回复里提取它**点名**的保存路径（`assets/...` 且以允许的扩展名结尾）。
///
/// 做法：定位每个 `assets`，从那里取**最长路径安全字符游程**（ASCII 字母数字 +
/// `_ - . / \`，正反斜杠都算路径符），遇第一个路径外字符（中英文标点、星号、
/// 空格…）即停；归一化反斜杠后必须以 `assets/` 打头——天然免疫前后文标点
/// （含全角逗号）与 Markdown 强调。找不到 → None。
fn suggested_rel_path(reply: &str) -> Option<String> {
    let mut search_from = 0;
    while let Some(pos) = reply[search_from..].find("assets") {
        let abs = search_from + pos;
        let rest = &reply[abs..];
        let end = rest
            .find(|c: char| {
                !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '\\'))
            })
            .unwrap_or(rest.len());
        let norm = rest[..end].replace('\\', "/");
        let lower = norm.to_ascii_lowercase();
        if norm.starts_with("assets/")
            && SAVE_EXTS.iter().any(|e| lower.ends_with(&format!(".{e}")))
        {
            return Some(norm);
        }
        // 这个 assets 不是保存位置（如 assets/lua/README），继续找下一处。
        search_from = abs + "assets".len();
    }
    None
}

/// 把 `assets/<feature>/<sub...>/<file>.<ext>` 拆成 (feature, sub, file)。
/// `assets/` 后至少要有一段（`assets/foo.lua` 不收——它没有落在任何功能目录里，
/// 八成是 AI 随口一提而非保存位置）；中间子目录可有可无（`assets/lua/foo.lua` 合法）。
fn split_asset_rel(rel: &str) -> Option<(String, String, String)> {
    let rest = rel.strip_prefix("assets/")?;
    let mut parts = rest.split('/');
    let feature = parts.next()?.to_string();
    let tail: Vec<&str> = parts.collect();
    if feature.is_empty() || tail.is_empty() {
        return None;
    }
    let file = tail[tail.len() - 1].to_string();
    if file.is_empty() {
        return None;
    }
    let sub = tail[..tail.len() - 1].join("/");
    Some((feature, sub, file))
}

/// 文件名 → (去扩展名的 stem, 扩展名)；没有扩展名时按 txt 兜底。
fn split_stem_ext(file: &str) -> (String, String) {
    match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => {
            (stem.to_string(), ext.to_ascii_lowercase())
        }
        _ => (file.to_string(), "txt".to_string()),
    }
}

/// 首个代码围栏（```）的语言 → 扩展名；没有围栏 / 未知语言 → txt（2026-10-08 用户定：
/// 默认要 .txt，不要 .md——普通说明文字用户拿记事本打开才是常态）。
fn fenced_ext(reply: &str) -> &str {
    for line in reply.lines() {
        let t = line.trim();
        if let Some(info) = t.strip_prefix("```") {
            let lang = info.split_whitespace().next().unwrap_or("");
            return match lang.to_ascii_lowercase().as_str() {
                "lua" => "lua",
                "json" => "json",
                "html" | "htm" => "html",
                "ini" => "ini",
                "md" | "markdown" => "md",
                _ => "txt",
            };
        }
    }
    "txt"
}

/// 「保存回复」的目标三元组：(初始目录, 预填文件名（不带扩展名）, 默认扩展名)。
/// 优先级：回复点名的 `assets/<feature>/<sub>/<file>` > `assets/lua/` + 围栏语言 > txt。
fn save_target(reply: &str) -> (Option<std::path::PathBuf>, String, String) {
    if let Some(rel) = suggested_rel_path(reply) {
        if let Some((feature, sub, file)) = split_asset_rel(&rel) {
            let (stem, ext) = split_stem_ext(&file);
            let base = crate::core::paths::asset_dir(&feature);
            let dir = if sub.is_empty() { base } else { base.join(sub) };
            return (Some(dir), stem, ext);
        }
    }
    (
        Some(crate::core::paths::asset_dir("lua")),
        "回复".to_string(),
        fenced_ext(reply).to_string(),
    )
}

/// 解除忙态：停读秒、恢复标题（按钮不再禁用，无需恢复）。
fn chat_unbusy(hwnd: HWND) {
    CHAT.with(|c| {
        let mut b = c.borrow_mut();
        if let Some(st) = b.as_mut() {
            st.busy = false;
            st.ticks = 0;
        }
    });
    unsafe {
        let _ = KillTimer(Some(hwnd), CHAT_TIMER);
        let t = wide("AI 写脚本");
        let _ = SetWindowTextW(hwnd, PCWSTR(t.as_ptr()));
    }
}

/// 读子控件全文（UTF-16 → String，与 win_util::read_edit_text 同一套）。
fn read_ctrl_text(hwnd: HWND, id: i32) -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    unsafe {
        let Ok(w) = GetDlgItem(Some(hwnd), id) else {
            return String::new();
        };
        let len = GetWindowTextLengthW(w).max(0) as usize;
        if len == 0 {
            return String::new();
        }
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(w, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// 换行归一化为 EDIT 认的 `\r\n`（★ 多行 EDIT 只认 \r\n）。
fn to_crlf(s: &str) -> String {
    s.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', "\r\n")
}

/// 尾部换行占多少 UTF-16 unit（`\r`/`\n` 各算一个；`\r\n` 两个 unit 一起剥）。
fn trailing_break_units(units: &[u16]) -> usize {
    let mut k = 0;
    while k < units.len() {
        let c = units[units.len() - 1 - k];
        if c == 0x0D || c == 0x0A {
            k += 1;
        } else {
            break;
        }
    }
    k
}

/// 往只读对话区追加一个消息块，**块与块之间恒为恰好一个空行**。
///
/// 做法（2026-09-30 加固）：先把既有文本尾部所有换行剥干净（不管它们是历史
/// 遗留还是上一块自带），非空则补两个 `\r\n`（= 一个空行），再写块正文（不带
/// 尾换行）。这样无论用户输入 / AI 回复 / 上一块尾部是什么样，结果都确定：
/// `内容 / 空行 / 内容`。
fn append_message(hwnd: HWND, id: i32, body: &str) {
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};
    const EM_SETSEL: u32 = 0x00B1;
    const EM_REPLACESEL: u32 = 0x00C2;
    unsafe {
        let Ok(w) = GetDlgItem(Some(hwnd), id) else {
            return;
        };
        let len = GetWindowTextLengthW(w).max(0) as usize;
        let mut buf = vec![0u16; len + 1];
        let n = GetWindowTextW(w, &mut buf).max(0) as usize;
        let strip = trailing_break_units(&buf[..n.min(len)]);
        // 只读框暂时放开再写（EM_REPLACESEL 对只读框不保证生效，稳妥起见显式开关）。
        let _ = SendMessageW(
            w,
            0x00CF, /* EM_SETREADONLY */
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
        if strip > 0 {
            let start = n - strip;
            let _ = SendMessageW(
                w,
                EM_SETSEL,
                Some(WPARAM(start)),
                Some(LPARAM(start as isize)),
            );
            let _ = SendMessageW(w, EM_REPLACESEL, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let base = n - strip;
        let sep = if base == 0 { "" } else { "\r\n\r\n" };
        let s: Vec<u16> = format!("{sep}{}", to_crlf(body))
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let _ = SendMessageW(
            w,
            EM_SETSEL,
            Some(WPARAM(base)),
            Some(LPARAM(base as isize)),
        );
        let _ = SendMessageW(
            w,
            EM_REPLACESEL,
            Some(WPARAM(0)),
            Some(LPARAM(s.as_ptr() as isize)),
        );
        let _ = SendMessageW(
            w,
            0x00CF, /* EM_SETREADONLY */
            Some(WPARAM(1)),
            Some(LPARAM(0)),
        );
        // 滚到底 + 滚动条按需重算。
        let end = GetWindowTextLengthW(w).max(0) as usize;
        let _ = SendMessageW(w, EM_SETSEL, Some(WPARAM(end)), Some(LPARAM(end as isize)));
        let _ = SendMessageW(
            w,
            0x00B7, /* EM_SCROLLCARET */
            Some(WPARAM(0)),
            Some(LPARAM(0)),
        );
        update_vscroll(w);
    }
}

/// 组 `/chat/completions` 请求体：system 打头 + 多轮历史原序。
fn build_body(model: &str, sys: &str, history: &[(String, String)]) -> String {
    let mut msgs = vec![serde_json::json!({ "role": "system", "content": sys })];
    for (role, content) in history {
        msgs.push(serde_json::json!({ "role": role, "content": content }));
    }
    serde_json::to_string(&serde_json::json!({ "model": model, "messages": msgs }))
        .unwrap_or_default()
}

/// 从响应里取回复文本。状态非 200 / 解析失败 / 结构不对 → Err（带可读信息）。
fn extract_reply(status: i64, body: &str) -> Result<String, String> {
    if status != 200 {
        return Err(format!("HTTP {status}：{}", truncate_tail(body, 300)));
    }
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("响应解析失败：{e}"))?;
    let content = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .ok_or_else(|| "响应里没有回复内容（choices[0].message.content 缺失）".to_string())?;
    Ok(content.to_string())
}

/// 截尾保头（错误 body 可能巨大；只留前 `max` 字符 + 省略号）。
fn truncate_tail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_leave_message_is_the_client_area_variant() {
        // ★ 回归锁：悬停变色后**不恢复**的真因是这里曾手写 0x02A2 —— 那是
        //   `WM_NCMOUSELEAVE`（非客户区离开），按钮永远收不到，状态清不掉。
        //   钉住数值，谁再把常量改回去都会红。
        assert_eq!(WM_MOUSELEAVE, 0x02A3);
        assert_ne!(WM_MOUSELEAVE, 0x02A2, "0x02A2 是 WM_NCMOUSELEAVE");
        assert_eq!(WM_MOUSEMOVE, 0x0200);
    }

    #[test]
    fn build_body_orders_system_user_assistant() {
        let hist = vec![
            ("user".to_string(), "问题".to_string()),
            ("assistant".to_string(), "回答".to_string()),
            ("user".to_string(), "追问".to_string()),
        ];
        let v: serde_json::Value =
            serde_json::from_str(&build_body("glm-4-flash", "你是助手", &hist)).unwrap();
        assert_eq!(v["model"], "glm-4-flash");
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 4, "system + 3 轮历史");
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "你是助手");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "问题");
        assert_eq!(msgs[2]["role"], "assistant");
        assert_eq!(msgs[3]["content"], "追问");
    }

    #[test]
    fn extract_reply_pulls_choices_content() {
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"你好"}}]}"#;
        assert_eq!(extract_reply(200, body).unwrap(), "你好");
    }

    #[test]
    fn extract_reply_reports_bad_status_with_snippet() {
        let e = extract_reply(401, r#"{"error":"bad key"}"#).unwrap_err();
        assert!(e.contains("401"), "{e}");
        assert!(e.contains("bad key"), "{e}");
    }

    #[test]
    fn extract_reply_survives_garbage_body() {
        assert!(extract_reply(200, "not json").is_err());
        // 200 但结构不对（没有 choices）也要报解析失败，不能给出空回复。
        assert!(extract_reply(200, r#"{"ok":1}"#).is_err());
    }

    #[test]
    fn truncate_tail_keeps_head_and_marks() {
        assert_eq!(truncate_tail("abcdef", 4), "abcd…");
        assert_eq!(truncate_tail("abc", 10), "abc");
    }

    /// 通道顺序回归锁（2026-09-29）：[`win_util::colorref`] 收 `R,G,B,A`；写成
    /// `A,R,G,B` 会让正文近黑变亮红——症状是「颜色怪」而非报错，必须钉死在数字上
    /// （COLORREF = `0x00BBGGRR`，故 RGB(45,52,54) = `0x36342D`）。
    #[test]
    fn chat_colors_map_to_expected_colorrefs() {
        let cr = crate::platform::win_util::colorref;
        assert_eq!(cr(CHAT_TEXT).0, 0x0036_342D, "TEXT 应为 RGB(45,52,54)");
        assert_eq!(cr(CHAT_FRAME).0, 0x0000_0000, "FRAME 应为纯黑");
        assert_eq!(cr(CHAT_BG).0, 0x00FF_FFFF, "BG 应为白");
        assert_eq!(cr(CHAT_HOVER).0, 0x00F0_ECE8, "HOVER 应为 RGB(232,236,240)");
        assert_eq!(
            cr(CHAT_PRESSED).0,
            0x00E4_DED8,
            "PRESSED 应为 RGB(216,222,228)"
        );
    }

    /// 块间空行判据（2026-09-30）：剥尾换行 + 补两个换行的拼装结果必须恒为
    /// 「一个空行」——上一块带任意尾换行都不能叠出第二个空行。
    #[test]
    fn append_separator_is_exactly_one_blank_line() {
        // 既有文本尾部 0/1/2/4 个换行（\r\n 混 \n）→ 剥完后都只剩一个空行。
        for tail in ["", "\n", "\r\n", "\r\n\r\n", "\r\n\r\n\r\n\r\n"] {
            let existing = format!("上文{tail}");
            let units: Vec<u16> = existing.encode_utf16().collect();
            let strip = trailing_break_units(&units);
            let base = units.len() - strip;
            let joined = if base == 0 {
                "块".to_string()
            } else {
                format!("{}\r\n\r\n块", String::from_utf16_lossy(&units[..base]))
            };
            let lines: Vec<&str> = joined.split("\r\n").collect();
            assert_eq!(
                lines,
                vec!["上文", "", "块"],
                "tail={tail:?} 时块间必须恰为一个空行"
            );
        }
        // 空对话：首块前不补空行。
        assert_eq!(trailing_break_units(&[]), 0);
    }

    /// 尾部换行计数的边界：\r\n 两个 unit、混排、不含换行、全换行。
    #[test]
    fn trailing_break_units_counts_utf16() {
        let u = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(trailing_break_units(&u("abc")), 0);
        assert_eq!(trailing_break_units(&u("abc\r\n")), 2);
        assert_eq!(trailing_break_units(&u("abc\n")), 1);
        assert_eq!(trailing_break_units(&u("abc\r\n\r\n\n")), 5);
        assert_eq!(trailing_break_units(&u("\r\n")), 2);
        assert_eq!(trailing_break_units(&u("中\n")), 1, "中文后跟换行");
    }

    /// 换行归一化：\r\n 不重复转换，裸 \n 转 \r\n，裸 \r 也归一。
    #[test]
    fn to_crlf_normalizes_all_line_endings() {
        assert_eq!(to_crlf("a\r\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\nb"), "a\r\nb");
        assert_eq!(to_crlf("a\rb"), "a\r\nb");
        assert_eq!(to_crlf("a\r\n\nb"), "a\r\n\r\nb");
    }

    // ── 保存 / 复制回复（2026-10-08）─────────────────────────────────

    /// UTF-16 切片：ASCII / BMP 中文 / 星形面（代理对）各来一刀；越界截断。
    #[test]
    fn utf16_slices_by_units() {
        assert_eq!(utf16_slice("hello", 1, 3), "el");
        assert_eq!(utf16_slice("中文abc", 1, 4), "文ab");
        // 😀 = 2 个 UTF-16 unit：位置 1..3 恰好取到完整代理对。
        assert_eq!(utf16_slice("a😀b", 1, 3), "😀");
        assert_eq!(utf16_slice("ab", 0, 99), "ab", "越界截断");
        assert_eq!(utf16_slice("ab", 5, 9), "", "起点也越界 → 空");
    }

    /// 取最后一条 AI 回复：跳过中间的 user 轮，trim 前后空白；没有 → None。
    #[test]
    fn last_reply_takes_latest_assistant() {
        let h = vec![
            ("user".to_string(), "q1".into()),
            ("assistant".to_string(), "  a1 \n".into()),
            ("user".to_string(), "q2".into()),
            ("assistant".to_string(), "a2".into()),
        ];
        assert_eq!(
            last_reply(&h).as_deref(),
            Some("a2"),
            "取最后一条 assistant"
        );
        let empty: Vec<(String, String)> = Vec::new();
        assert_eq!(last_reply(&empty), None);
        let only_user = vec![("user".to_string(), "q".into())];
        assert_eq!(last_reply(&only_user), None);
    }

    /// 路径建议：能从回复里捞出点名的 assets/ 路径（反斜杠归一），捞不出 → None。
    #[test]
    fn suggested_rel_path_scans_reply() {
        assert_eq!(
            suggested_rel_path(
                "好的，完整内容如下，保存到 assets/lua/MouseTrail/my_fx.lua 即可。\n```lua"
            ),
            Some("assets/lua/MouseTrail/my_fx.lua".to_string())
        );
        assert_eq!(
            suggested_rel_path("存 `assets\\VirtualKeyboards\\skin.json`"),
            Some("assets/VirtualKeyboards/skin.json".to_string()),
            "反斜杠归一化"
        );
        assert_eq!(
            suggested_rel_path("存到 <assets/lua/scripts/mini.html> 这个位置"),
            Some("assets/lua/scripts/mini.html".to_string()),
            "尖括号包裹也能切出来"
        );
        assert_eq!(
            suggested_rel_path("目标：**assets/lua/ClickFx/ripple.lua**，内容如下"),
            Some("assets/lua/ClickFx/ripple.lua".to_string()),
            "Markdown 加粗的星号只掐头去尾"
        );
        // 拒绝：非 assets 路径 / 无扩展名目录 / 不允许的扩展名。
        assert_eq!(suggested_rel_path("放到 C:\\tmp\\x.lua"), None);
        assert_eq!(suggested_rel_path("看 assets/lua/README"), None);
        assert_eq!(suggested_rel_path("别下 assets/x.exe"), None);
    }

    /// assets 相对路径拆解：功能段必需、子目录可选、裸 `assets/x.ext` 拒收。
    #[test]
    fn split_asset_rel_splits_feature_sub_file() {
        assert_eq!(
            split_asset_rel("assets/lua/MouseTrail/foo.lua"),
            Some(("lua".into(), "MouseTrail".into(), "foo.lua".into()))
        );
        assert_eq!(
            split_asset_rel("assets/VirtualKeyboards/skin.json"),
            Some(("VirtualKeyboards".into(), String::new(), "skin.json".into()))
        );
        assert_eq!(split_asset_rel("assets/foo.lua"), None, "无功能目录 → 拒收");
        assert_eq!(split_asset_rel("assets/"), None);
        assert_eq!(split_asset_rel("lua/foo.lua"), None);
    }

    /// stem / 扩展名拆分：大小写归一、无扩展名 txt 兜底、点开头文件名不误拆。
    #[test]
    fn split_stem_ext_handles_edge_cases() {
        assert_eq!(
            split_stem_ext("fx.LUA"),
            ("fx".to_string(), "lua".to_string())
        );
        assert_eq!(
            split_stem_ext("a.b.lua"),
            ("a.b".to_string(), "lua".to_string())
        );
        assert_eq!(
            split_stem_ext("noext"),
            ("noext".to_string(), "txt".to_string())
        );
        assert_eq!(
            split_stem_ext(".hidden"),
            (".hidden".to_string(), "txt".to_string())
        );
    }

    /// 围栏语言 → 扩展名：首个围栏生效，大小写不敏感；无围栏 / 未知语言 → txt 兜底，
    /// ```md 显式声明时才给 md（2026-10-08 用户定：默认要 txt）。
    #[test]
    fn fenced_ext_maps_language() {
        assert_eq!(fenced_ext("说明\n```lua\ncode\n```"), "lua");
        assert_eq!(fenced_ext("```JSON\n{}\n```"), "json");
        assert_eq!(
            fenced_ext("```lua some note\nx"),
            "lua",
            "info 串只取首个 token"
        );
        assert_eq!(fenced_ext("纯文本回复"), "txt");
        assert_eq!(fenced_ext("```\ncode\n```"), "txt", "空 info → txt");
        assert_eq!(fenced_ext("```md\n# 标题\n```"), "md", "显式 md 才给 md");
        assert_eq!(fenced_ext("```python\nx\n```"), "txt", "未知语言 → txt");
    }

    /// 保存目标三元组：点名路径优先（目录/名字/扩展名全按它）；
    /// 没点名 → assets/lua + 围栏语言；连围栏都没有 → txt。
    #[test]
    fn save_target_prefers_named_path_then_fence() {
        let (dir, name, ext) = save_target("保存到 assets/lua/MouseTrail/xx.lua");
        assert!(dir.unwrap().ends_with("assets\\lua\\MouseTrail"));
        assert_eq!((name.as_str(), ext.as_str()), ("xx", "lua"));

        let (dir, name, ext) = save_target("这是 JSON 方案：\n```json\n{}\n```");
        assert!(dir.unwrap().ends_with("assets\\lua"));
        assert_eq!((name.as_str(), ext.as_str()), ("回复", "json"));

        let (_, name, ext) = save_target("就是一段说明文字");
        assert_eq!((name.as_str(), ext.as_str()), ("回复", "txt"));
    }
}
