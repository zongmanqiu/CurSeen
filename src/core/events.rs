//! 输入事件契约。捕获层产出、UI 层消费，两侧都不依赖对方。

use std::collections::HashMap;
use std::sync::Mutex;

/// 物理键的稳定标识。不用 UI 框架的 `Key` 枚举 ——那个是控件层的键语义
/// （带 Tab/方向键等抽象），而捕捉层需要 104/144 键的物理身份
/// （区分左右 Ctrl、区分小键盘与编辑键区）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    /// 字母数字与符号键（已按 Shift 修正到实际字符）
    Char(char),
    F(u8),
    Esc,
    Tab,
    CapsLock,
    Backspace,
    Enter,
    Space,
    /// 鼠标左键。只作"显示"用（虚拟键盘 WASD 板等覆盖层），不参与快捷键注册
    /// （`to_vk` 对它返回 `None`），也永远不会从键盘钩子产生 —— 由鼠标钩子的
    /// `MouseDown/MouseUp` 事件直接携带 `MouseButton`，消费方自行归一到这两个变体。
    MouseLeft,
    /// 鼠标右键。语义同 [`KeyCode::MouseLeft`]。
    MouseRight,
    /// 鼠标中键。语义同 [`KeyCode::MouseLeft`]。
    MouseMiddle,
    /// 鼠标侧键 1（浏览器后退键）。语义同 [`KeyCode::MouseLeft`]。
    MouseX1,
    /// 鼠标侧键 2（浏览器前进键）。语义同 [`KeyCode::MouseLeft`]。
    MouseX2,
    /// 滚轮上滚。滚轮是**脉冲**事件（没有对应的"抬起"）——消费方只记一次按下
    /// 时刻，走既有的保持/淡出时间线。由鼠标钩子的 `MouseWheel`（纵向、delta>0）
    /// 归一而来。
    WheelUp,
    /// 滚轮下滚。语义同 [`KeyCode::WheelUp`]。
    WheelDown,
    LShift,
    RShift,
    LCtrl,
    RCtrl,
    LAlt,
    RAlt,
    LWin,
    RWin,
    Menu,
    PrtSc,
    ScrollLock,
    Pause,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    NumLock,
    NumDiv,
    NumMul,
    NumSub,
    NumAdd,
    NumDot,
    NumEnter,
    Num(u8),
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MediaNext,
    MediaPrev,
    MediaStop,
    MediaPlay,
    BrowserBack,
    BrowserForward,
    BrowserRefresh,
    BrowserHome,
    /// 其余 OEM 键（原样保留 VK 码）
    Oem(u8),
    /// 未映射（原样保留 VK 码，便于日志排查）
    Unknown(u16),
}

impl KeyCode {
    /// 规范显示名（弹窗用）：如 "Ctrl" / "Shift" / "A" / "F5" / "Num?" / "Up"
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Char(c) => match c {
                ' ' => "Space",
                // 非 ASCII 字符（IME / 死键 / 法德西键盘）不在 128 项静态表里，
                // 直接按 `c as usize` 索引会越界 panic；走 non_ascii_char_name 的缓存
                // 取稳定 &'static str（同种字符只泄漏一条）。
                _ if (c as usize) < BOXED_CHARS.len() => BOXED_CHARS[c as usize],
                _ => non_ascii_char_name(c),
            },
            Self::F(n) => match n {
                1 => "F1",
                2 => "F2",
                3 => "F3",
                4 => "F4",
                5 => "F5",
                6 => "F6",
                7 => "F7",
                8 => "F8",
                9 => "F9",
                10 => "F10",
                11 => "F11",
                12 => "F12",
                _ => "F?",
            },
            Self::Esc => "Esc",
            Self::Tab => "Tab",
            Self::CapsLock => "CapsLock",
            Self::Backspace => "Backspace",
            Self::Enter => "Enter",
            Self::Space => "Space",
            Self::MouseLeft => "LMB",
            Self::MouseRight => "RMB",
            Self::MouseMiddle => "MMB",
            Self::MouseX1 => "X1",
            Self::MouseX2 => "X2",
            Self::WheelUp => "WheelUp",
            Self::WheelDown => "WheelDown",
            Self::LShift => "LShift",
            Self::RShift => "RShift",
            Self::LCtrl => "LCtrl",
            Self::RCtrl => "RCtrl",
            Self::LAlt => "LAlt",
            Self::RAlt => "RAlt",
            Self::LWin => "LWin",
            Self::RWin => "RWin",
            Self::Menu => "Menu",
            Self::PrtSc => "PrtSc",
            Self::ScrollLock => "ScrollLock",
            Self::Pause => "Pause",
            Self::Insert => "Insert",
            Self::Delete => "Delete",
            Self::Home => "Home",
            Self::End => "End",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Up => "Up",
            Self::Down => "Down",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::NumLock => "NumLock",
            Self::NumDiv => "Num/",
            Self::NumMul => "Num*",
            Self::NumSub => "Num-",
            Self::NumAdd => "Num+",
            Self::NumDot => "Num.",
            Self::NumEnter => "NumEnter",
            Self::Num(_n) => "Num?",
            Self::VolumeUp => "Vol+",
            Self::VolumeDown => "Vol-",
            Self::VolumeMute => "Mute",
            Self::MediaNext => "Next",
            Self::MediaPrev => "Prev",
            Self::MediaStop => "Stop",
            Self::MediaPlay => "Play",
            Self::BrowserBack => "Back",
            Self::BrowserForward => "Fwd",
            Self::BrowserRefresh => "Refresh",
            Self::BrowserHome => "Home",
            Self::Oem(_) => "Oem",
            Self::Unknown(_) => "?",
        }
    }

    /// 键的类别（决定弹窗着色与显示过滤）
    pub fn class(self) -> KeyClass {
        match self {
            Self::Char(_) => KeyClass::Character,
            Self::F(_)
            | Self::LShift
            | Self::RShift
            | Self::LCtrl
            | Self::RCtrl
            | Self::LAlt
            | Self::RAlt
            | Self::LWin
            | Self::RWin
            | Self::Menu => KeyClass::Function,
            _ => KeyClass::Special,
        }
    }
}

/// 字符键显示名静态表：空格单独处理，其余直接取字符。
/// '0'-'9' = "0"-"9", 'a'-'z' = "A"-"Z", 其余符号原样。
static BOXED_CHARS: [&str; 128] = build_char_table();

const fn build_char_table() -> [&'static str; 128] {
    let mut table: [&str; 128] = [""; 128];
    // 数字 0-9
    let mut i = 0;
    while i < 10 {
        // '0' = 48
        table[48 + i] = match i {
            0 => "0",
            1 => "1",
            2 => "2",
            3 => "3",
            4 => "4",
            5 => "5",
            6 => "6",
            7 => "7",
            8 => "8",
            _ => "9",
        };
        i += 1;
    }
    // 字母 a-z → 大写显示
    let mut c = b'a';
    while c <= b'z' {
        let idx = c as usize;
        // 'a' → "A", etc.
        table[idx] = match c {
            b'a' => "A",
            b'b' => "B",
            b'c' => "C",
            b'd' => "D",
            b'e' => "E",
            b'f' => "F",
            b'g' => "G",
            b'h' => "H",
            b'i' => "I",
            b'j' => "J",
            b'k' => "K",
            b'l' => "L",
            b'm' => "M",
            b'n' => "N",
            b'o' => "O",
            b'p' => "P",
            b'q' => "Q",
            b'r' => "R",
            b's' => "S",
            b't' => "T",
            b'u' => "U",
            b'v' => "V",
            b'w' => "W",
            b'x' => "X",
            b'y' => "Y",
            _ => "Z",
        };
        c += 1;
    }
    // 字母 A-Z：Shift+字母的 KeyDown 会产生大写字符（vk.rs 的 Shift 修正），此前
    // 表里没填——display_name 返回空串，击键回显在 Shift+字母时显示空标签，
    // 击键音效的分类脚本也拿不到键名（2026-09-21）。小写表已就位，直接镜像。
    let mut c = b'A';
    while c <= b'Z' {
        table[c as usize] = table[(c + 32) as usize];
        c += 1;
    }
    // 常见符号
    table[b' ' as usize] = "Space";
    table[b'`' as usize] = "`";
    table[b'-' as usize] = "-";
    table[b'=' as usize] = "=";
    table[b'[' as usize] = "[";
    table[b']' as usize] = "]";
    table[b'\\' as usize] = "\\";
    table[b';' as usize] = ";";
    table[b'\'' as usize] = "'";
    table[b',' as usize] = ",";
    table[b'.' as usize] = ".";
    table[b'/' as usize] = "/";
    table
}

/// 非 ASCII 字符（法/德/西键盘、IME、死键）无法进 128 项静态表，而 `display_name`
/// 必须返回 `&'static str`：用一个小缓存，每个**不同**的非 ASCII 字符泄漏一条 `&'static str`，
/// 数量受限于用户实际按过的字符种类，不会无界增长（键是 `char` 不是串，同种字符只漏一条）。
fn non_ascii_char_name(c: char) -> &'static str {
    static CACHE: Mutex<Option<HashMap<char, &'static str>>> = Mutex::new(None);
    match CACHE.lock() {
        Ok(mut guard) => {
            let map = guard.get_or_insert_with(HashMap::new);
            if let Some(s) = map.get(&c) {
                return s;
            }
            let leaked: &'static str = Box::leak(c.to_string().into_boxed_str());
            map.insert(c, leaked);
            leaked
        }
        // 锁中毒（理论上不会发生）：退回安全占位，绝不 panic。
        Err(_) => "?",
    }
}

/// 键类别。对齐 Python keymap 的 (type, is_fun) 二元组。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyClass {
    /// 字符键（字母/数字/符号）——display_mode = chars_only 时显示
    Character,
    /// 功能键与修饰键（红色显示）——is_fun = true
    Function,
    /// 编辑/导航/小键盘/媒体键（白色/正文色）
    Special,
}

/// 鼠标按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// 修饰键状态。
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

/// 捕获层产出的统一事件。
#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    /// 键按下。`repeated` 为 true 表示是长按自动重复（系统连发）。
    KeyDown {
        key: KeyCode,
        mods: Modifiers,
        repeated: bool,
        time_ms: u64,
    },
    KeyUp {
        key: KeyCode,
        mods: Modifiers,
        time_ms: u64,
    },
    /// 鼠标移动。坐标为**虚拟桌面物理像素**（多显示器可能为负）。
    MouseMove { x: i32, y: i32, time_ms: u64 },
    MouseDown {
        button: MouseButton,
        x: i32,
        y: i32,
        time_ms: u64,
    },
    MouseUp {
        button: MouseButton,
        x: i32,
        y: i32,
        time_ms: u64,
    },
    MouseWheel {
        delta: i32,
        /// 是否横向倾斜滚轮（WM_MOUSEHWHEEL）。只有击键回显消费它（滚轮左/右），
        /// 倍率缩放只认纵向。
        horizontal: bool,
        x: i32,
        y: i32,
        time_ms: u64,
    },
}

/// 从后台线程 / 托盘发往 UI 线程的**意图**。
///
/// # 为什么放在 `core`
///
/// 这是一份**消息契约**：托盘、更新线程是生产者，`Runtime` 是消费者，两侧都不该依赖
/// 对方所在的层。放进 `core::events` 之后 `update` 子系统可以直接用它而不必绕道 `app`，
/// 分层图因此保持无环。
///
/// `Runtime` 是 `Rc<RefCell<_>>`，不是 `Send`——后台线程（输入钩子桥接、更新线程）碰不到它，
/// 托盘回调也不该碰（回调运行时平台层正持有窗口状态的 `&mut`，重入会形成别名 UB）。
/// 两边都只能把"想做什么"经 `App::channel` 送到 UI 线程执行。
#[derive(Debug)]
pub enum UiMsg {
    /// 打开控制面板；已开着则把它激活到前台（由 `single("panel")` 保证）。
    OpenPanel,
    /// 输入钩子捕获到的一个事件。
    Input(InputEvent),
    /// 更新流程的一步进展（检查 / 下载）。 [`crate::update::UpdateEvent`] 是事件载荷，
    /// 定义在 `update` 子系统内部以保持该子系统自洽；它是纯数据，`Clone + Send`。
    Update(UpdateEvent),
    /// GIF 选框状态变了（框好 / 收起）。
    ///
    /// # 为什么必须是一条消息，而不是"等下一次自然唤醒"
    ///
    /// 选框是在**区域选择窗**里确认的（`layer::region` 的 `confirm` 直接调
    /// `Runtime::confirm_snap`），那一刻覆盖层往往正处在 **park 态**——功能全关时它不为
    /// 任何输入出帧（见 `Runtime::input_needs_frame`）。park 态下 `paint()` 只渲进草稿、
    /// 不上屏，于是：
    ///
    /// - 边框画不出来 → 用户看到的症状是"框完屏幕一点反馈都没有"；
    /// - 控件条只挂在通道回调上 → 没有消息就永远不开窗 → "要再按一次快捷键才出按钮"。
    ///
    /// 所以状态一变就**主动推一条消息**，由 UI 线程解除 park 并立即重绘。这与
    /// `take_pin_request` / `take_rec_toast` 是同一条纪律：**能由我们自己决定时刻的
    /// 界面变化，不该去等下一次用户输入**。
    GifSelChanged,
    /// 该打开 GIF 裁剪窗了（控件条上的「裁剪」按下）。
    ///
    /// 与 [`Self::GifSelChanged`] 同一个理由：按下按钮的那一刻是**控件条窗口**在处理
    /// 事件，而开新窗必须在 UI 线程的通道回调里做；没有这条消息就只能等下一次自然
    /// 唤醒（250ms 对账），按钮的手感会明显发黏。
    GifTrimRequested,
    /// 截图**编辑态**已进入（`Ctrl` + 截图快捷键 → 框选确认）。
    ///
    /// 与 [`Self::GifSelChanged`] 逐字同一条理由：确认框选那一刻是**区域选择窗**在处理
    /// 事件，而覆盖层往往正处在 **park 态**（用户没开任何需要出帧的功能）。park 态下
    /// `paint()` 只渲进草稿、不上屏，于是"框完屏幕一点反馈都没有"——底图、选区外压暗、
    /// 跟随选区的工具栏全都画不出来。状态一变就主动推一条消息，由 UI 线程解除 park 并
    /// 立即重绘，而不是等下一次用户输入或 250ms 对账。
    ShotEditChanged,
    /// 长截图会话状态变了（框选确认开始采集 / 收工交付）。
    ///
    /// 与 [`Self::GifSelChanged`] 同一条理由：选区确认发生在**区域选择窗**里
    /// （`Runtime::confirm_snap` 的提前返回），那一刻覆盖层往往正 park 着——
    /// 不主动推消息的话，紫色的采集边框要等下一次自然唤醒（250ms 对账）才出现。
    /// 收工不需要这条消息（250ms 对账的 `poll_scroll` 兜底 + `Notify` 自己续帧），
    /// 但开始那一拍必须由它踢出来。
    ScrollChanged,
    /// 有一张图已经备好，可以弹原生「另存为」了（截图 / 屏幕标注的保存按钮）。
    ///
    /// # 为什么必须绕一圈消息
    ///
    /// 原生保存对话框是**模态**的，自带消息泵、会阻塞到用户松手；而它只能在 **UI 线程**、
    /// 且必须在"事件分发完全返回之后"调用（`ctx.defer_blocking`）——否则就是拿着
    /// `&mut Runtime` 借着重入自己的窗口过程（见 `platform::file_dialog` 的模块头）。
    /// 产出这张图的地方是 `Runtime::frame()` 里的离屏合成（正借着 `&mut Runtime`），
    /// 所以它只能**登记 + 发这条消息**，由 UI 线程的通道回调去弹框。
    ///
    /// 与 [`Self::GifTrimRequested`] 是同一个套路：跨线程的"请 UI 线程开一个模态东西"。
    SaveAsReady,
    /// `temp/`（exe 同级那个）的占用统计回来了。
    ///
    /// # 为什么又是一条消息
    ///
    /// 与 [`Self::Update`] 同一条纪律：递归统计目录是几十毫秒量级的磁盘活，只能在后台
    /// 线程跑；而 `Signal` 是 `!Send`，后台线程连"顺手 `set` 一下那个数字"都做不到。
    /// 于是只能把结果当**纯数据**送回来，由 UI 线程写进信号
    /// （`Runtime::apply_temp_usage` → `ui::temp_usage::TempUsageState::apply`）。
    TempUsage(super::temp_usage::TempUsage),
}

/// 更新流程的一条后台事件。
///
/// 与 [`InputEvent`] 同属"后台线程产出、UI 线程消费"的**消息契约**，故一并放在 `core`
/// （对齐 Pecia 把 `PipeProtocol.h` 放进 `core` 的做法）。这样 `update` 子系统可以只依赖
/// `core`，分层图无环；事件的**解释**（怎么改 UI 状态）仍在 UI 线程，见
/// `Runtime::apply_update_event`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateEvent {
    /// 已开始检查（把状态行切成"正在检查…"）。
    CheckStarted,
    /// 已是最新版本。
    UpToDate { latest: String },
    /// 有新版本，且拿到了下载地址。
    Available { latest: String, url: String },
    /// 检查失败（网络 / 解析 / 没有可下载的包）。
    CheckFailed { error: String },
    /// 下载进度（`total == 0` 表示服务端未给长度）。
    DownloadProgress { received: u64, total: u64 },
    /// 下载完成，zip 已落在本地。
    Downloaded { path: std::path::PathBuf },
    /// 下载失败。
    DownloadFailed { error: String },
    /// 替换脚本已启动，本进程马上就退出。
    Launching,
}
