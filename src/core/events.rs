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
    /// 用户点了「检查更新」：由 UI 线程从配置取两条清单地址与当前源，再起后台线程。
    UpdateCheck,
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
    /// 脚本请宿主开一个模态窗（`ext:result` / `ext:input`），见 [`ScriptDialogSpec`]。
    ///
    /// 为什么必须绕这一圈：windui 的窗**只能在 UI 线程开**，而脚本每次触发都在自己的
    /// 工作线程上跑（`scripting::ext_runner`）。旧做法是工作线程直接弹原生框、靠框自带的
    /// 嵌套消息泵阻塞（`platform::win_util::text_result_box`）——那套控件的滚动条和按钮
    /// 与主窗口完全不同（用户 2026-10-27 指出）。换成宿主窗后，跨线程只能送**纯数据 +
    /// 回复通道**，于是这个窗必然是模态同步的：工作线程 `recv` 等用户点按钮。
    ScriptDialog(std::sync::Arc<ScriptDialogSpec>),
    /// 脚本要把**它当初来自的那张贴图**换成另一张图（`ext:set_image`），见 [`ScriptImageSpec`]。
    ///
    /// 为什么也绕这一圈：像素可以跨线程递（`RgbaBitmap` 是 `Send`），但**贴图窗的画布不行**
    /// （`RefCell<PinCanvas>` 不是 `Sync`，而换图要顺带重建渲染对象、改窗口尺寸、踢重排——
    /// 全都必须在 UI 线程做）。所以这里同样是"纯数据 + 一次性回复"。
    ScriptImage(std::sync::Arc<ScriptImageSpec>),
    /// 屏幕中央的一句短反馈（`ext_runner` 的「正在运行：X」/「X 还在跑，这次忽略」）。
    ///
    /// 为什么也要一条消息：这句话是**脚本的工作线程**想说的，而浮字画在 UI 线程的覆盖层里
    /// （`Runtime::action_notify`）。与 [`Self::GifSelChanged`] 同一条纪律——能由我们自己
    /// 决定时刻的界面变化，主动推一条消息；park 态下没有它就只能等下一次用户输入，
    /// 而那正好是"按了热键屏幕一点反应都没有"的病。
    ///
    /// ★ 只用 `String`、没有回复通道：**它是反馈，不是业务结果**。送不到（总线没装 /
    ///   应用正在退出）不算失败，也不该让脚本因此改变行为。
    Notice(String),
    /// 在线脚本库的用户动作（刷新清单 / 安装 / 卸载，m36）。
    StoreAction(StoreAction),
    /// 在线脚本库的进展（后台线程 → UI 线程，m36）。
    Store(StoreEvent),
}

/// 一次脚本回写：把 `hwnd` 那张贴图的像素换成 `bitmap`。
///
/// ★ 语义与右键「旋转 / 镜像」**同一条缝**（`runtime::layer::pin::transform_pin`）：
///   只换**视图**，不动 `OpenPin.content` 与贴图环里那份**出厂原图**（「恢复原图」与
///   「上一个/下一个」的源），也不写盘。想回原样：贴图右键「恢复原图」（m16）。
pub struct ScriptImageSpec {
    /// 目标贴图窗的句柄（= 脚本上下文里的 `ctx.hwnd`）。`0` = 不是从贴图触发的 ⇒ 直接失败。
    pub hwnd: isize,
    /// 新像素。
    pub bitmap: std::sync::Arc<super::image::RgbaBitmap>,
    /// 一次性回复：`Ok(())` = 已换；`Err(中文原因)` = 找不到那张窗 / 重建渲染对象失败。
    reply: Mutex<Option<std::sync::mpsc::Sender<Result<(), String>>>>,
}

impl ScriptImageSpec {
    /// 构造一次回写请求，交出回复端。
    pub fn new(
        hwnd: isize,
        bitmap: std::sync::Arc<super::image::RgbaBitmap>,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Result<(), String>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            std::sync::Arc::new(Self {
                hwnd,
                bitmap,
                reply: Mutex::new(Some(tx)),
            }),
            rx,
        )
    }

    /// 交回结果。**只回一次**（`take`）：先回的那个算数，后来的空操作。
    pub fn reply(&self, outcome: Result<(), String>) {
        if let Some(tx) = self.reply.lock().unwrap().take() {
            let _ = tx.send(outcome);
        }
    }
}

impl std::fmt::Debug for ScriptImageSpec {
    /// 像素不进 `Debug`，只记句柄与尺寸。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptImageSpec")
            .field("hwnd", &self.hwnd)
            .field("w", &self.bitmap.w)
            .field("h", &self.bitmap.h)
            .finish()
    }
}

/// 一个脚本要宿主弹的模态窗（`ext:result` = 只读正文，`ext:input` = 可编辑）。
///
/// ★ **形状是刻意的**：只有"标题 + 正文/初值 + 一次性回复"，没有控件树、没有回调。
///   跨线程送不了 Lua 函数（`Runtime` 非 `Send`，窗只能在 UI 线程开），所以窗内的事件
///   全部在 Rust 侧闭合，值确定后一次性回给工作线程。这样既不需要"signal + 回调进 Lua"
///   那座桥，也没有"脚本已退出但窗还开着"的生命周期问题。常驻 / 自定义界面不属于这里，
///   那类需求走外部程序（见 `.plan/m08-capability-route-and-host-dialog.md`）。
pub struct ScriptDialogSpec {
    /// 显示在窗内标题带上的文字（脚本给的，如「图片体检」）。
    ///
    /// ★ **不是**窗口标题：`TopmostKeeper::guard_aux` 是按标题找窗的，动态标题守不住。
    ///   窗口标题恒为 `ui::script_dialog` 里的两个静态常量之一。
    pub title: String,
    /// 只读结果窗的正文。宿主按行切第一处分隔符自动排成两列（切不开的整行显示）。
    pub body: String,
    /// `true` = 可编辑输入窗（预填 [`Self::initial`]）；`false` = 只读结果窗。
    pub editable: bool,
    /// 输入窗的预填值。
    pub initial: String,
    /// 落点参照：**触发它的那个窗口的句柄**（贴图右键带来；面板里点「运行」传 0）。
    ///
    /// 为什么带句柄而不是坐标：frameless 的宿主窗要"落在手边"（跟标注输入窗同一条理由——
    /// 双屏合计 3840 宽时屏幕中心离用户刚点的那一下可能差一整个屏幕）。由 UI 侧问一次
    /// `platform::win_util::window_rect_hwnd` 拿矩形，工作线程不碰几何。
    pub owner_hwnd: isize,
    /// 一次性的回复出口：`Some(文本)` = 点了「确定」，`None` = 取消 / 关窗 / 结果窗被关掉。
    ///
    /// 用 `Mutex<Option<_>>` + `take`：**只回一次**。按钮先回，随后的关窗回调再想回就是
    /// 空操作；反过来也一样。没人回而窗彻底销毁时，这里的 `Sender` 随 `Arc` 一起掉，
    /// 工作线程的 `recv` 返回 `Err` ⇒ 按"取消"收尾，**不会挂死**。
    reply: Mutex<Option<std::sync::mpsc::Sender<Option<String>>>>,
}

impl std::fmt::Debug for ScriptDialogSpec {
    /// 手写信封：`Sender` 不进 `Debug`，正文只打长度（结果框里可能是整段 OCR 文本）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptDialogSpec")
            .field("title", &self.title)
            .field("body_len", &self.body.len())
            .field("editable", &self.editable)
            .finish()
    }
}

impl ScriptDialogSpec {
    /// 只读结果窗（`ext:result`、脚本返回的字符串）。构造时一并交出回复端。
    pub fn result(
        title: impl Into<String>,
        body: impl Into<String>,
        owner_hwnd: isize,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Option<String>>,
    ) {
        Self::build(title.into(), body.into(), false, String::new(), owner_hwnd)
    }

    /// 可编辑输入窗（`ext:input`）。
    pub fn input(
        title: impl Into<String>,
        initial: impl Into<String>,
        owner_hwnd: isize,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Option<String>>,
    ) {
        let initial = initial.into();
        Self::build(title.into(), String::new(), true, initial, owner_hwnd)
    }

    fn build(
        title: String,
        body: String,
        editable: bool,
        initial: String,
        owner_hwnd: isize,
    ) -> (
        std::sync::Arc<Self>,
        std::sync::mpsc::Receiver<Option<String>>,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            std::sync::Arc::new(Self {
                title,
                body,
                editable,
                initial,
                owner_hwnd,
                reply: Mutex::new(Some(tx)),
            }),
            rx,
        )
    }

    /// 把结果送回工作线程。幂等：第二次及以后是空操作（按钮 + 关窗会各调一次）。
    pub fn reply(&self, value: Option<String>) {
        if let Ok(mut slot) = self.reply.lock() {
            if let Some(tx) = slot.take() {
                // 接收端已掉线（脚本线程已经退出）就放弃——这里不能 panic，
                // 也不能把值留给下一个窗。
                let _ = tx.send(value);
            }
        }
    }

    /// 是否已经回过人（`on_close_request` 用它决定要不要按"取消"补一次）。
    pub fn replied(&self) -> bool {
        self.reply
            .lock()
            .map(|slot| slot.is_none())
            .unwrap_or(false)
    }
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
    /// 有新版本，且拿到了下载地址 / 校验值 / HISTORY 说明。
    Available {
        latest: String,
        url: String,
        sha256: String,
        notes: String,
    },
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

/// 在线脚本库的一次用户动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreAction {
    /// 拉取官方清单（`source` 为选中的镜像序号，失败时先试另一镜像、再回退缓存）。
    RefreshCatalog { source: usize },
    /// 安装或更新某个包（按 id；`source` 决定先试哪个镜像）。
    Install { id: String, source: usize },
    /// 卸载某个包（只删安装记录里登记的文件）。
    Uninstall(String),
}

/// 清单里的一枚包 + "本机装没装"的合并视图（纯数据，可跨线程）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePackRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub size: u64,
    /// 本机已装的版本（`None` = 没装）。
    pub installed_version: Option<String>,
    /// 包的 `min_host` 高于当前宿主：列出但**不许装**（按钮置灰并说明原因）。
    pub too_new: bool,
    /// 包内文件数（**只有已装的知道**：来自安装记录；未装 = 0 = 未知）。
    /// 详情窗用来回答"这个包会往 assets/lua 放多少文件"。
    pub files: u64,
    /// 当前清单里有哪些镜像地址（bit i = `store::SOURCE_LABELS[i]` 有地址）。
    /// 选了某个来源、而这一项没有该来源的地址 ⇒ 行尾按钮置灰（与 RuPix 同一判据）。
    pub source_mask: u8,
    /// 作者 / 许可证 / 发行时间（来自清单，详情窗显示）。
    pub author: String,
    pub license: String,
    pub released: String,
    /// 需要的 Lua API 级别（与宿主 `store::LUA_API` 比较）。
    pub api: u32,
    /// 安装后体积（未压缩）与脚本文件数（清单侧）。
    pub unpacked_size: u64,
    pub file_count: u64,
}

/// 在线脚本库的进展事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreEvent {
    /// 开始拉清单。
    CatalogStarted,
    /// 清单就绪（`note` 非空 = 走的是缓存 / 有需要说明的情况）。
    CatalogReady {
        rows: Vec<StorePackRow>,
        note: String,
    },
    /// 清单拉取失败（没有可用缓存时）。
    CatalogFailed { error: String },
    /// 某个包开始下载（`total == 0` = 服务端未给长度）。
    PackProgress {
        id: String,
        received: u64,
        total: u64,
    },
    /// 安装/更新完成。
    Installed {
        id: String,
        name: String,
        version: String,
        added: u64,
        updated: u64,
        removed: u64,
    },
    /// 安装/更新失败。
    InstallFailed { id: String, error: String },
    /// 卸载完成（`files` = 删掉的文件数）。
    Uninstalled { id: String, files: u64 },
    /// 卸载失败。
    UninstallFailed { id: String, error: String },
}
