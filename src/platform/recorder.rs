//! 屏幕录制：DXGI Desktop Duplication 采集 + Media Foundation H264/AAC 编码（可走硬件）。
//!
//! # 为什么是这几个
//!
//! | 环节 | 选择 | 理由 |
//! |---|---|---|
//! | 采集 | **DXGI Desktop Duplication** | 只在屏幕真的变了才给帧（`WAIT_TIMEOUT`），静止时零负载；帧本身就是 D3D 纹理，可以直接进 GPU 流水线，不必每帧拷回内存 |
//! | 转码 | **VideoProcessorMFT**（系统自带） | BGRA → NV12 的活儿交给 GPU，CPU 只发两个指令；**缩放也在这里做**（见「分辨率档」） |
//! | 编码 | **IMFSinkWriter + H264 + AAC** | 系统自带编码器（Intel QSV / NVIDIA NVENC / AMD VCE 都会走 `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`），不需要打包 ffmpeg，二进制不涨 |
//! | 音频 | **WASAPI**（麦克风 + loopback） | 系统原生 API，采集麦克风和系统声音，互不干扰 |
//!
//! 三者叠加：录屏期间主线程完全不参与，采集-转换-编码全在**一条后台线程**里串行跑
//! （省掉跨线程传 COM 对象的 `Send` 麻烦，也少一次队列拷贝）。
//!
//! # 一次录制 = 一个 [`Session`] + 一个 sink
//!
//! 2026-09-18 起，采集部分抽成 [`Session`]（设备 / 复制会话 / 采集纹理 / 光标合成 /
//! 取帧节拍），**MP4 与 GIF 两条通路共用它**。这不是为省几行代码，而是因为
//! DXGI Desktop Duplication **对每个显示器有并发会话限制**：同一输出上后建的会话会把
//! 先前的挤掉。GIF 与录屏各建一份 duplication，就会出现"开一个另一个瞎掉"的偶发故障
//! （`docs/DESIGN.md` §3.1 记的就是这个坑）。共用 Session 之后，两件事天然互斥。
//!
//! # 分辨率档（720p / 1080p）
//!
//! 采集纹理**永远是原生分辨率**（光标要在原始像素上合成，缩完再画会糊），缩放交给
//! VideoProcessorMFT 的输出类型 —— 它本来就是个缩放器，改一个 `FRAME_SIZE` 就走 GPU，
//! CPU 零成本。于是"720p 档"在 4K 屏上也是免费降下来的。
//!
//! # GIF
//!
//! GIF 不走 Media Foundation（不需要编码器、不需要音频），而是采集 → 读回 → CPU 量化
//! → `gif` crate 编码（见 [`crate::core::gif_enc`] 模块头，那里解释了为什么复用
//! 已有的 `gif` crate 而不是自己写 LZW）。**与录屏共用同一个 [`Session`]**，所以
//! "帧率 / 分辨率 / 录制鼠标"这三个开关对两者语义一致。
//!
//! # 已知边界
//!
//! - **按显示器录制**，不是"虚拟桌面整块"：Desktop Duplication 是 per-output 的 API，
//!   要拼多屏得为每块屏各建一份 duplication 再合成，代价远超收益。
//! - **音频**：WASAPI loopback 采集系统声音，WASAPI eCapture 采集麦克风，
//!   两路混音后编码为 AAC。麦克风支持基于 RNNoise 的神经网络降噪（弱 / 中 / 强三档）。
//! - **GIF 无音频**：格式就不支持，界面上的音频项对它无效。

use std::sync::atomic::{AtomicBool, AtomicI8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::{IUnknown, Interface, Result as WinResult};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{
    StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, SRCCOPY,
};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetCursorPos, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON,
};

use crate::core::error::{CurSeenError, Result};
use crate::core::gif_enc::{self, GifWriter, Palette, Quantizer};
use crate::platform::capture_share;

/// 录制区域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RecordArea {
    /// 主显示器。
    Primary,
    /// 录制开始时鼠标所在的显示器。
    Cursor,
}

/// 产物类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RecKind {
    /// H264 + AAC 的 mp4（走 Media Foundation）。
    Mp4,
    /// 无声 GIF 动图（走 [`crate::core::gif_enc`]）。
    Gif,
}

/// 硬件编码策略。
///
/// `Auto` = 打开 `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`，**并在建 sink 失败时自动退回
/// 软件编码器**；`Off` = 始终用系统软件编码器。
///
/// 为什么要留 `Off`：老显卡的硬件编码器画质往往不如系统软件编码器（wcap 的 README
/// 也建议老 GPU 显式关掉硬件编码），而"自动"在驱动坏了的情况下会让人以为录屏坏了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Hardware {
    Auto,
    Off,
}

/// 一次录制的参数。
#[derive(Debug, Clone)]
pub struct RecOptions {
    pub kind: RecKind,
    /// 输出文件（`.mp4` 或 `.gif`）。
    pub path: std::path::PathBuf,
    pub area: RecordArea,
    /// 只录这一块（**虚拟桌面物理像素**；副屏在主屏左侧时 x 可为负）。
    ///
    /// `None` = 整块显示器（旧行为，MP4 走这条）。
    /// `Some` = 桌面复制抓到的帧**先裁到这一块**再缩放/编码 —— 于是"录哪一块"由选框
    /// 说了算，而不是只能录整屏。GIF 的选框录制走这条。
    ///
    /// ★ 顺序必须是**先裁后缩**：先缩再裁的话，裁剪坐标要按缩放比反算，舍入误差让边缘
    /// 差一两个像素，而且框外的缩放算力全白费。
    ///
    /// ★ 裁剪在**采集纹理**上完成（见 `Session::open`），所以下游的缩放 / 光标合成 /
    /// 封装器一行都不用改 —— 它们看到的"原生尺寸"就是裁完的尺寸。
    pub crop: Option<crate::core::geometry::ScreenRect>,
    pub fps: u32,
    /// 目标码率（kbps）。仅 MP4 用。
    ///
    /// ★ `None` = **自动**：按 `core::config::auto_bitrate_kbps()` 用**最终输出尺寸** ×
    ///   帧率算。不能在调用方算 —— 自动档关心的是"编码器实际要写多少像素"，而那只在
    ///   桌面复制建好、拿到 `ModeDesc` 并做完缩放之后才知道（见 `Session::open`）。
    pub bitrate_kbps: Option<u32>,
    /// 输出**长边**上限（像素）。`0` = 原始分辨率（只保证偶数边）。MP4 与 GIF 共用。
    pub max_long_edge: u32,
    /// 硬件编码策略。仅 MP4 用。
    pub hardware: Hardware,
    pub show_cursor: bool,
    /// 是否采集麦克风声音。GIF 忽略。
    pub capture_mic: bool,
    /// 是否采集系统声音（扬声器 / 桌面音频）。GIF 忽略。
    pub capture_speaker: bool,
    /// 麦克风降噪档位（`off`/`weak`/`medium`/`strong`），透传给 WASAPI 采集线程。
    /// 噪声门已在 2026-09-19 删除，强度合并进本字段。
    pub mic_denoise: String,
    /// 指定麦克风设备（友好名，空串=系统默认）。
    pub mic_device: String,
    /// 指定系统声音设备（友好名，空串=系统默认）。
    pub speaker_device: String,
    /// GIF 是否无限循环。MP4 忽略。
    pub gif_loop: bool,
}

/// 录制线程与 UI 之间的共享状态。
///
/// ★★ 热字段**全部是原子量，UI 读取永不加锁**（2026-10-06）。
///
/// 旧写法是 `Arc<Mutex<RecStats>>`：UI 每帧轮询 `frames()` / `out_size()` /
/// `elapsed_ms()` 都要和录制线程抢同一把锁。录制线程在锁内只写几个字段、MF 调用都在锁外，
/// 所以"楔死时锁不会被带走"是靠**纪律**维持的——只要哪天有人在锁内加一句会阻塞的调用，
/// "软件被录屏拖死"就会以最隐蔽的方式复活（平时测不出，只在特定时机触发，
/// 和 0930 那次"看不出规律"的失败是同一种形态）。
///
/// 现在：除 `error` 外的读取路径完全无锁；`error` 用独立的小锁且 `try_lock` 回退
/// （拿不到就当"暂无"），因为它是"取一次就走"的冷路径。
/// 于是"UI 永不被录制侧阻塞"从纪律变成**结构**。
struct RecShared {
    /// 时间基准（`started_ms` / `elapsed_ms` 的零点）。`Instant` 是 `Copy`，无需加锁。
    base: Instant,
    /// 采集侧已经取到的帧数（**不管有没有写进文件**）。
    frames: AtomicU64,
    /// 编码侧真的写进文件的帧数（≤ `frames`，差就是被有界队列丢掉的）。
    written: AtomicU64,
    /// 因为编码队列满而丢掉的帧数。
    dropped: AtomicU64,
    /// 编码线程在收尾时限内没退出（楔住）。见 [`Self::wedged`] 的说明。
    wedge: AtomicBool,
    out_w: AtomicU32,
    out_h: AtomicU32,
    /// -1 = 还没走到建 sink；0 = 未启用硬件；1 = 已启用。
    hardware: AtomicI8,
    /// 正式开录的时刻（相对 `base` 的毫秒）。`u64::MAX` = 还没开始。
    started_ms: AtomicU64,
    /// 致命错误（置上后录制已结束）。冷路径，独占一把小锁。
    error: Mutex<Option<String>>,
}

impl RecShared {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            frames: AtomicU64::new(0),
            written: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            wedge: AtomicBool::new(false),
            out_w: AtomicU32::new(0),
            out_h: AtomicU32::new(0),
            hardware: AtomicI8::new(-1),
            started_ms: AtomicU64::new(u64::MAX),
            error: Mutex::new(None),
        }
    }
    fn set_frames(&self, n: u64) {
        self.frames.store(n, Ordering::Relaxed);
    }
    fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
    /// 编码侧写完一帧就 +1（采集/编码解耦后，`frames` 与它不再相等）。
    fn add_written(&self, n: u64) {
        self.written.fetch_add(n, Ordering::Relaxed);
    }
    fn written(&self) -> u64 {
        self.written.load(Ordering::Relaxed)
    }
    /// 队列满了、这一帧没送进编码器。
    fn add_dropped(&self, n: u64) {
        self.dropped.fetch_add(n, Ordering::Relaxed);
    }
    fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
    /// 编码线程在限时内没退出。
    ///
    /// ★ 为什么它不能只看"采集线程退没退"：解耦之后采集线程**自己不编码**，它把帧
    ///   交给编码线程就立刻返回；真正会楔在 `WriteSample`/`Finalize` 上的是**编码线程**。
    ///   若只看采集线程，楔住会被误报成"正常收尾"（产物此时根本没 `moov`）。
    fn set_wedged(&self) {
        self.wedge.store(true, Ordering::Relaxed);
    }
    fn wedged(&self) -> bool {
        self.wedge.load(Ordering::Relaxed)
    }
    fn set_out_size(&self, w: u32, h: u32) {
        self.out_w.store(w, Ordering::Relaxed);
        self.out_h.store(h, Ordering::Relaxed);
    }
    fn out_size(&self) -> (u32, u32) {
        (
            self.out_w.load(Ordering::Relaxed),
            self.out_h.load(Ordering::Relaxed),
        )
    }
    fn set_hardware(&self, used: bool) {
        self.hardware
            .store(if used { 1 } else { 0 }, Ordering::Relaxed);
    }
    /// MP4：本次是否允许了硬件编码（`None` = 还没走到建 sink）。
    fn hardware_allowed(&self) -> Option<bool> {
        match self.hardware.load(Ordering::Relaxed) {
            1 => Some(true),
            0 => Some(false),
            _ => None,
        }
    }
    fn mark_started(&self, at: Instant) {
        let ms = at.saturating_duration_since(self.base).as_millis() as u64;
        self.started_ms.store(ms, Ordering::Relaxed);
    }
    /// 已录制毫秒数（`0` = 还没正式开录）。
    fn elapsed_ms(&self) -> u64 {
        let s = self.started_ms.load(Ordering::Relaxed);
        if s == u64::MAX {
            return 0;
        }
        self.base.elapsed().as_millis() as u64 - s
    }
    fn set_error(&self, msg: String) {
        if let Ok(mut e) = self.error.lock() {
            *e = Some(msg);
        }
    }
    /// 取走一次错误（取过即无）。★ `try_lock`：拿不到就当"暂无"，绝不阻塞 UI。
    fn take_error(&self) -> Option<String> {
        self.error.try_lock().ok().and_then(|mut e| e.take())
    }
}

/// 一次录制的收尾结论。
///
/// ★★ 把"有没有产物"与"过程出没出错"**分开**。旧写法是一个 `Result<u64>`，两者只能二选一，
/// 实现里选了"文件非空就算成功"—— 于是 2026-09-19 16:35 那次：录到第 8 秒崩
/// （`0x887A0005`），留下一个 3.5 MB、**没有 `moov` 盒**、任何播放器都打不开的 mp4，
/// 界面却照样弹"录制完成"。用户以为录到了，实际上什么都没录到。
///
/// 现在：产物可用就报 `bytes = Some(n)`，同时把 `error` 交出去，让界面能说清"这段是中断的"。
#[derive(Debug, Clone)]
pub struct FinishOutcome {
    /// 产物字节数；`None` = 没有产出（文件不存在或为空）。
    pub bytes: Option<u64>,
    /// 录制线程中途报的错；`None` = 全程无错。
    pub error: Option<String>,
    /// 录制线程在收尾时限内**没有退出**（楔住了）。
    ///
    /// ★ 它的后果不只是"这段没了"：楔住的线程仍占着桌面复制（**进程级单例**），
    ///   在它退出之前新的一段根本建不起来（`DuplicateOutput` 直接失败）。
    ///   UI 必须据此如实告知"需重启软件才能继续录"，而不是让用户按下快捷键之后
    ///   得到一次莫名其妙的失败。2026-10-06（对应"软件绝不因录屏卡死"那条原则）。
    pub wedged: bool,
}

impl FinishOutcome {
    /// 完整录完：有产物且过程中一次错都没出。
    pub fn is_clean(&self) -> bool {
        self.bytes.is_some() && self.error.is_none() && !self.wedged
    }
}

/// 录制线程所处的阶段（看门狗停摆报告与用户提示用，2026-10-06 自新版合并回旧内核）。
///
/// 阶段名是**用户可见**的（停摆时提示「录制卡住（X）」），措辞要能让用户原样贴回来定位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecStage {
    /// 还没进主循环（建会话 / 建 sink / 音频起播）。
    Startup,
    /// 桌面复制：等/取桌面帧 + 拷纹理 + `ReleaseFrame`。
    Acquire,
    /// 画面转换 BGRA→NV12。
    Convert,
    /// 写视频 `WriteSample`。
    Encode,
    /// 写音频。
    Audio,
    /// 收尾 `Finalize`。
    Finalize,
    /// 尾部路径 ①：停音频采集、等采集线程退出。
    AudioStop,
    /// 尾部路径 ②：补写音频尾部样本（之后进 Finalize）。
    Tail,
    /// 采集侧把当前画面**读回系统内存**（采集/编码解耦后过界的那一步）。
    Readback,
    /// 已结束（正常退出或出错返回后）。
    Done,
}

impl RecStage {
    /// 阶段 → 整数（阶段存在原子里程碑里，只能放整数）。
    pub(crate) fn to_u32(self) -> u32 {
        match self {
            Self::Startup => 0,
            Self::Acquire => 1,
            Self::Convert => 2,
            Self::Encode => 3,
            Self::Audio => 4,
            Self::Finalize => 5,
            Self::Done => 6,
            Self::AudioStop => 7,
            Self::Tail => 8,
            Self::Readback => 9,
        }
    }
    pub(crate) fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Acquire,
            2 => Self::Convert,
            3 => Self::Encode,
            4 => Self::Audio,
            5 => Self::Finalize,
            6 => Self::Done,
            7 => Self::AudioStop,
            8 => Self::Tail,
            9 => Self::Readback,
            _ => Self::Startup,
        }
    }
    /// 完整阶段名（日志用）。
    pub fn name(self) -> &'static str {
        match self {
            Self::Startup => "建会话/建 sink",
            Self::Acquire => "桌面采集 AcquireNextFrame",
            Self::Convert => "画面转换 BGRA→NV12",
            Self::Encode => "写视频 WriteSample",
            Self::Audio => "写音频",
            Self::Finalize => "收尾 Finalize",
            Self::AudioStop => "停音频采集",
            Self::Tail => "音频尾部写入",
            Self::Readback => "像素回读",
            Self::Done => "已结束",
        }
    }
    /// 短阶段名（浮字提示用，越短越好）。
    pub fn short(self) -> &'static str {
        match self {
            Self::Startup => "启动",
            Self::Acquire => "桌面采集",
            Self::Convert => "画面转换",
            Self::Encode => "写视频",
            Self::Audio => "写音频",
            Self::Finalize => "收尾",
            Self::AudioStop => "停采",
            Self::Tail => "音尾",
            Self::Readback => "回读",
            Self::Done => "已结束",
        }
    }
}

/// 录制心跳与阶段的原子里程碑（看门狗的数据源）。
///
/// `stamp_ms` = 「起点起第几毫秒」，录制线程**每帧**都会碰它；UI 侧用
/// `stall_ms()`（现在 − stamp）判断"多久没推进了"。
struct RecProgress {
    stage: std::sync::atomic::AtomicU32,
    stamp_ms: std::sync::atomic::AtomicU64,
    /// ★ **编码线程**的独立阶段（2026-10-06 采集/编码解耦后必需）。
    ///
    /// 两条线程共用 `stage` 会互相覆盖：采集侧每 33ms 就 touch 一次，编码器楔在
    ///   `WriteSample` 上时报出来的却是采集侧最后 touch 的那个阶段 —— 停摆提示会
    ///   **指错地方**（实测 `encode` 楔住时报成「画面转换 BGRA→NV12」）。
    ///   编码侧单独记一份，谁卡了就读谁的。
    enc_stage: std::sync::atomic::AtomicU32,
    /// 线程起点：`stamp_ms` 与 `stall_ms` 的共同时间基准。
    base: Instant,
}

impl RecProgress {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            stage: std::sync::atomic::AtomicU32::new(RecStage::Startup.to_u32()),
            stamp_ms: std::sync::atomic::AtomicU64::new(0),
            enc_stage: std::sync::atomic::AtomicU32::new(RecStage::Startup.to_u32()),
            base: Instant::now(),
        })
    }
    /// 采集侧的里程碑。
    fn touch(&self, stage: RecStage) {
        self.stage
            .store(stage.to_u32(), std::sync::atomic::Ordering::Relaxed);
        self.stamp_ms.store(
            self.base.elapsed().as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }
    /// 编码侧的里程碑：**只写 `enc_stage`**，不碰采集侧那份（否则又互相覆盖）。
    fn touch_enc(&self, stage: RecStage) {
        self.enc_stage
            .store(stage.to_u32(), std::sync::atomic::Ordering::Relaxed);
    }
    fn read(&self) -> (RecStage, u64) {
        (
            RecStage::from_u32(self.stage.load(std::sync::atomic::Ordering::Relaxed)),
            self.stamp_ms.load(std::sync::atomic::Ordering::Relaxed),
        )
    }
    /// 编码侧当前阶段（停摆提示在"编码停摆"时读它，见 `runtime/frame.rs`）。
    fn read_enc(&self) -> RecStage {
        RecStage::from_u32(self.enc_stage.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// 单文件方案：产物就是名义路径本身（新分段方案的 `.parts` 归位逻辑不存在）。
pub fn product_path(final_path: &std::path::Path) -> std::path::PathBuf {
    final_path.to_path_buf()
}

/// 一个正在进行的录制。
///
/// 录制本体 = 进程内的一条采集线程 + 一条编码线程（2026-10-07 用户拍板：不再出进程，
/// 省掉 1.79MB 的重复链接份 —— 那份里的代码主程序本来就有，子进程只是"再链接一遍"）。
/// 代价要如实记住：录制线程楔住后**杀不掉**，桌面复制单例被它占着 ⇒ 只能重启软件
/// （`wedged` 结论 + `runtime::record` 的 `rec_wedged` 就是这条路的如实上报）。
pub struct Recorder {
    stop: Arc<AtomicBool>,
    stats: Arc<RecShared>,
    progress: Arc<RecProgress>,
    join: Option<std::thread::JoinHandle<()>>,
    path: std::path::PathBuf,
    kind: RecKind,
}

impl Recorder {
    /// 起录制：一条进程内录制线程。
    pub fn start(opts: RecOptions) -> Result<Self> {
        let path = opts.path.clone();
        let kind = opts.kind;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(RecShared::new());
        let progress = RecProgress::new();
        let t_stop = stop.clone();
        let t_stats = stats.clone();
        let t_progress = progress.clone();
        let join = std::thread::Builder::new()
            .name("curseen-record".into())
            .spawn(move || record_thread(opts, t_stop, t_stats, t_progress))
            .map_err(|e| CurSeenError::Unsupported(leak(&format!("起录制线程失败: {e}"))))?;
        Ok(Self {
            stop,
            stats,
            progress,
            join: Some(join),
            path,
            kind,
        })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn kind(&self) -> RecKind {
        self.kind
    }

    /// MP4：本次是否允许了硬件编码（`None` = 还没走到建 sink，或这是 GIF）。
    ///
    /// 「允许了」≠「每个变换都跑在 GPU 上」—— MF 的自动回退是"建 sink 失败就换软件编码器"，
    /// 建成功了 `true` 就意味着硬件变换被启用了。仅用于日志与事后核对。
    pub fn hardware_allowed(&self) -> Option<bool> {
        self.stats.hardware_allowed()
    }

    /// 采集侧已经取到的帧数（无锁）。
    pub fn frames(&self) -> u64 {
        self.stats.frames()
    }

    /// 编码侧真的写进文件的帧数（无锁）。
    ///
    /// ★ 采集/编码解耦之后它**不等于** [`Self::frames`]：差就是"取到了但没写进去"
    ///   （被有界队列丢掉的，或者还在队列里）。判据要区分"采集卡了"和"编码卡了"
    ///   就必须看它 —— 见 `tests/probes/rec_wedge.rs`。
    pub fn written(&self) -> u64 {
        self.stats.written()
    }

    /// 因编码队列满而丢弃的帧数（无锁）。正常录制应为 0；它涨 = 编码跟不上采集。
    pub fn dropped(&self) -> u64 {
        self.stats.dropped()
    }

    /// **采集侧**当前阶段与已取帧数（看门狗停摆报告用）。
    pub fn progress(&self) -> (RecStage, u64) {
        let (stage, _) = self.progress.read();
        (stage, self.frames())
    }

    /// **编码侧**当前阶段（2026-10-06 解耦后新增）。
    ///
    /// 两条线程共用一份阶段会互相覆盖 —— 采集侧每 33ms 就 touch 一次，于是编码器
    ///   楔在 `WriteSample` 上时报出来的却是采集侧那个阶段，停摆提示**指错地方**。
    ///   判据/提示在"编码停摆"时必须读这一份。
    pub fn enc_progress(&self) -> RecStage {
        self.progress.read_enc()
    }

    /// 距上次心跳推进了多少毫秒（录制线程每帧都会碰心跳，持续增长 = 楔住）。
    pub fn stall_ms(&self) -> u64 {
        let now = self.progress.base.elapsed().as_millis() as u64;
        let stamp = self
            .progress
            .stamp_ms
            .load(std::sync::atomic::Ordering::Relaxed);
        now.saturating_sub(stamp)
    }

    /// 已录制毫秒数（无锁）。
    pub fn elapsed_ms(&self) -> u64 {
        self.stats.elapsed_ms()
    }

    /// 实际输出尺寸（`0×0` = 还没建好，无锁）。
    pub fn out_size(&self) -> (u32, u32) {
        self.stats.out_size()
    }

    /// 取走一次错误信息（取过即无）。★ 不阻塞（`try_lock`）。
    pub fn take_error(&self) -> Option<String> {
        self.stats.take_error()
    }

    /// 请求停止。不阻塞（收尾由 [`Self::finish`] 等）。
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// 停止并等待线程收尾（写完 mp4 索引 / GIF 终结符）。
    pub fn finish(mut self) -> FinishOutcome {
        self.finalize_inner(15)
    }

    /// 后台收尾（2026-10-06 自新版合并回旧内核）：UI 线程只发停止位，收尾在
    /// 后台线程限时进行，`on_done` 在后台被调用一次（含超时弃子的结论）。
    ///
    /// # Why（为什么必须有它）
    /// 收尾可能被编码器（`WriteSample` / `Finalize`）卡住——即使有关流 + 限时 join
    /// 两道保险，"UI 线程同步等收尾"这件事本身就意味着最坏情况界面冻结 15 秒。
    /// `finish_detached` 把这 15 秒也挪到后台：快捷键立刻恢复，结论照常回 UI
    /// （`runtime::record` 的 `REC_FINISH_NOTICES` 槽位）。
    pub fn finish_detached(
        self,
        timeout: std::time::Duration,
        on_done: impl FnOnce(FinishOutcome) + Send + 'static,
    ) {
        let mut rec = self;
        rec.request_stop();
        // `join` 留在 rec 里：后台线程 finalize_inner 里限时等待；Drop 在收尾后发生
        // （彼时 join 已被取走，Drop 里的 join 成为纯停止位写入，绝不阻塞）。
        let spawned = std::thread::Builder::new()
            .name("curseen-rec-finish".into())
            .spawn(move || {
                let outcome = rec.finalize_inner((timeout.as_secs()).max(1));
                on_done(outcome);
            });
        if spawned.is_err() {
            // 起不出收尾线程（线程资源枯竭，极端情况）：self 已随被丢弃的闭包释放
            // （Drop 里的限时 join 在调用线程跑，最多 secs 秒）。录制线程本体在停止位
            // 之后**自己**完成尾部写入 + `Finalize`（都在 `record_thread` 内），
            // 产物不丢；只是没人等它、回调发不出去 —— 如实记日志。
            log::warn!("录制收尾线程起不来，产物将由录制线程自行落盘");
        }
    }

    /// 收尾本体：限时等录制结束 → 汇总产物与错误。
    ///
    /// ★ 限时等待（2026-10-06）：录制侧只要楔在 `WriteSample`/`Finalize`，无限期等待
    ///   就会把调用方一起拖死 —— 而"卡死"和"产物损坏"其实是同一条链（用户只能强杀，
    ///   强杀 = moov 没写 = 打不开）。所以到点必须放弃等待并**如实按中断上报**。
    ///
    /// ★★ 放弃等待后那条线程**还活着**（进程内线程杀不掉，桌面复制单例还被它占着）
    ///   —— 这是进程内录制的已知代价（2026-10-07 用户拍板接受），`wedged` 如实上报。
    fn finalize_inner(&mut self, secs: u64) -> FinishOutcome {
        self.request_stop();
        let exited = match self.join.take() {
            Some(j) => join_bounded(j, secs),
            None => true,
        };
        let wedged = if exited { self.stats.wedged() } else { true };
        let error = if !exited {
            // ★ 两条线程各报各的：收尾超时时**采集线程常常是在等编码线程**（它卡在
            //   `EncHandle::finish` 的限时 join 上），只报采集阶段会指向"卡在采集"，
            //   而真正卡住的往往是编码器。
            let (stage, frames) = {
                let (s, _) = self.progress.read();
                (s, self.stats.frames())
            };
            let msg = format!(
                "收尾超时（>{secs}s 未退出，采集「{}」/ 编码「{}」，帧 {frames}）；产物可能缺索引",
                stage.name(),
                RecStage::from_u32(self.progress.read_enc().to_u32()).name()
            );
            log::warn!("录制{msg}");
            Some(msg)
        } else {
            // ★ 采集/编码解耦之后，"采集线程退出了"不再等于"收完了"：真正会楔在
            //   `WriteSample`/`Finalize` 上的是**编码线程**（wedged 由 stats 报告）。
            self.stats.take_error()
        };
        let bytes = std::fs::metadata(&self.path)
            .ok()
            .map(|m| m.len())
            .filter(|n| *n > 0);
        if let (Some(msg), Some(_)) = (error.as_ref(), bytes) {
            // ★ "有产物 + 中途出错" = 断在中间。产物现在仍可播放（`SinkFinisher` 兜住了
            //   `Finalize`），但**绝不能**再当"录完了"报给用户 —— 这条 warn 是事后核对的
            //   唯一线索，配合界面上的"录制中断"提示。
            log::warn!("录制中断（{msg}），已收尾产物：{}", self.path.display());
        }
        FinishOutcome {
            bytes,
            error,
            wedged,
        }
    }
}

/// 等一个线程退出，但**最多等 `secs` 秒**（2026-10-06）。
///
/// Rust 的 `JoinHandle` 没有带超时的 join：借一个看护线程代等，`recv_timeout`
/// 到点还没退出就放弃等待（看护线程留在后台继续收尸，录制线程的资源最终仍会被
/// 它释放；看护线程自身随录制线程退出而结束，不泄漏活线程）。
/// 返回 `true` = 正常退出；`false` = 超时放弃（已打 warn）。
fn join_bounded(join: std::thread::JoinHandle<()>, secs: u64) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let watcher = std::thread::Builder::new()
        .name("rec-finish-watch".into())
        .spawn(move || {
            let _ = join.join();
            let _ = tx.send(());
        });
    match watcher {
        Ok(_) => match rx.recv_timeout(Duration::from_secs(secs)) {
            Ok(()) => true,
            Err(_) => {
                log::warn!("录制线程收尾超过 {secs}s，转后台继续（界面不再等待）");
                false
            }
        },
        // 看护线程建不出来（极端情况）：句柄已随闭包被丢弃 = 转为分离线程，不阻塞。
        Err(_) => true,
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(j) = self.join.take() {
            // ★ 同 `finish`：Drop（常发生在应用退出路径）也不许被楔死的收尾拖死。
            join_bounded(j, 15);
        }
    }
}

/// 后台录制线程的全部内容。
///
/// MP4 才需要 Media Foundation；GIF 完全不需要，于是这里**不为 GIF 初始化 MF** ——
/// 少一个"MF 初始化失败就录不了 GIF"的无谓失败点。
fn record_thread(
    opts: RecOptions,
    stop: Arc<AtomicBool>,
    stats: Arc<RecShared>,
    progress: Arc<RecProgress>,
) {
    if opts.kind == RecKind::Gif {
        if let Err(e) = unsafe { run_gif(&opts, &stop, &stats) } {
            set_error(&stats, e);
        }
        return;
    }

    let mut com = false;
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() {
            com = true;
        }
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
            set_error(&stats, format!("Media Foundation 初始化失败: {e}"));
            return;
        }
    }
    let outcome = unsafe { run_mp4(&opts, &stop, &stats, &progress) };
    if let Err(e) = outcome {
        set_error(&stats, e);
    }
    unsafe {
        let _ = MFShutdown();
        if com {
            CoUninitialize();
        }
    }
}

fn set_error(stats: &RecShared, msg: String) {
    log::error!("录屏失败: {msg}");
    stats.set_error(msg);
}

/// 故障注入：走到指定阶段就**永久阻塞**（`CURSEEN_REC_WEDGE_AT`）。
///
/// ★ 与 `CURSEEN_REC_FAULT_AFTER`（注入"错误返回"）是一对：那条验证"出错也要能收尾成
///   可播文件"，这条验证 **"楔住也不能拖死软件"** —— 2026-10-06 定下的原则性要求
///   （软件绝不因录屏卡死）。判据在 `tests/probes/rec_wedge.rs`：
///   收尾必须限时返回、结论必须带 `wedged`、UI 侧时间必须远小于"无限等待"。
///
/// `tick` 下限是 10：先正常录十来帧再楔，这样"楔之前是健康的"也有证据。
fn maybe_wedge(want: Option<&str>, stage: &str, tick: i64) {
    let Some(want) = want else { return };
    if tick < 10 || !want.eq_ignore_ascii_case(stage) {
        return;
    }
    log::warn!("故障注入：在「{stage}」阶段楔住（tick={tick}），不再退出");
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// ─────────────────────────── 采集会话（MP4 / GIF 共用） ───────────────────────────

/// 一次采集会话：D3D 设备 + 桌面复制 + 采集纹理 + 光标合成。
///
/// `pump` 之后 `acq` 里**永远是"当前画面"**：首帧由 GDI 兜底铺进去，之后每来一帧 DXGI
/// 的就盖上去，一直没变化就一直重复它 —— 这正是"只录变化"想要的效果。
struct Session {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output1: IDXGIOutput1,
    dupl: IDXGIOutputDuplication,
    /// 采集纹理：**原生分辨率** BGRA，带 `GDI_COMPATIBLE`（光标要用 GDI 画上去）。
    acq: ID3D11Texture2D,
    /// 原生尺寸（采集纹理尺寸）。**裁了就是裁完的尺寸**（见 `RecOptions::crop`）。
    src_w: u32,
    src_h: u32,
    /// 裁剪偏移（相对这块显示器左上角的物理像素）。`None` = 整块显示器。
    crop: Option<(u32, u32)>,
    /// 输出尺寸（缩放后；等于原生尺寸时就是没缩）。
    out_w: u32,
    out_h: u32,
    /// 该输出在虚拟桌面里的左上角（画光标要减掉它）。
    origin: POINT,
    show_cursor: bool,
    /// **无光标底图**：fresh 帧时从 `acq` 拷来（桌面复制给的是不含光标的干净画面），
    /// 面板合成（同为 GDI 通道）也画在这张上。
    clean: ID3D11Texture2D,
    /// **合成目标**：每帧从 `clean` 拷来 + GDI 画光标，回读读它。
    ///
    /// ★ 2026-10-09 实锤：以前 GDI 直接画在 `acq`（桌面复制的活跃纹理）上，
    ///   桌面持续更新时约 1/3 的帧光标凭空消失（产物 I-beam 闪烁 93/272 帧，
    ///   实屏正常）。根因就是 GDI 写入与复制纹理的 GPU 生命周期竞争——
    ///   解耦后 GDI 只碰我们自己的 `work`，与复制彻底无关。
    work: ID3D11Texture2D,
    /// 诊断：上一次 `pump` 里 `AcquireNextFrame` 的耗时（微秒）。
    t_acquire_us: u64,
    /// 诊断：上一次 `pump` 里 `CopyResource` 的耗时（微秒）。
    t_copy_us: u64,
    /// 诊断：上一次 `pump` 里 GDI 光标合成的耗时（微秒）。
    t_cursor_us: u64,
    // ★ 本会话**不碰 Media Foundation**（2026-10-06 采集/编码解耦后）：桌面复制、
    //   光标合成、回读都在这台设备上，而 MF（转换器 / SinkWriter）在**另一台独立
    //   D3D 设备**上的编码线程里跑（见 `run_mp4`）。两边只以系统内存字节过界 ——
    //   绝不共享 `ID3D11DeviceContext`（单线程语义，2026-10-05 实测第 1259s 互等死锁）。
}

impl Session {
    unsafe fn open(opts: &RecOptions) -> std::result::Result<Self, String> {
        // ── 1. D3D11 设备 ──
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            windows::Win32::Foundation::HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|e| format!("创建 D3D11 设备失败: {e}"))?;
        let device = device.ok_or_else(|| "创建 D3D11 设备失败（无返回）".to_string())?;
        let context = context.ok_or_else(|| "创建 D3D11 上下文失败".to_string())?;

        // ★ 这里**不建** D3D 设备管理器了：采集侧不碰 MF，管理器归编码线程（见
        //   `create_d3d_and_mgr`，编码线程用它绑自己的那台设备）。

        // ── 2. 选显示器 + 建 Desktop Duplication ──
        let dxgi_device: IDXGIDevice = device
            .cast()
            .map_err(|e| format!("取 IDXGIDevice 失败: {e}"))?;
        let adapter: IDXGIAdapter = dxgi_device
            .GetAdapter()
            .map_err(|e| format!("取 IDXGIAdapter 失败: {e}"))?;

        let cursor = current_cursor_pos();
        let target = pick_output(&adapter, opts.area, cursor, opts.crop)?;
        let output_desc = target
            .GetDesc()
            .map_err(|e| format!("取显示器描述失败: {e}"))?;
        let output1: IDXGIOutput1 = target
            .cast()
            .map_err(|e| format!("取 IDXGIOutput1 失败: {e}"))?;
        let mut dupl: Option<IDXGIOutputDuplication> = None;
        duplicate_with_retry(&output1, &device, &mut dupl)?;
        let dupl = dupl.ok_or_else(|| "建立桌面复制失败".to_string())?;
        let desc = dupl.GetDesc();
        let full_w = desc.ModeDesc.Width.max(2);
        let full_h = desc.ModeDesc.Height.max(2);
        // ── 2b. 选框 → 相对这块显示器左上角的裁剪矩形 ──
        //
        // ★ 必须**夹进这块显示器**：选框拖到跨屏时，超出部分在这块屏上没有像素可拷，
        //   `CopySubresourceRegion` 会直接失败。取交集而不是报错——"跨屏那半没录上"
        //   比"整段录不出来"好解释得多。
        //
        // ★ 夹完退化成 0（选框整块落在另一块屏上）→ 退回整屏：宁可多录也别产出 0 字节。
        let dc = output_desc.DesktopCoordinates;
        let (crop, src_w, src_h) = match opts.crop {
            Some(c) => {
                let x0 = (c.x - dc.left).clamp(0, full_w as i32);
                let y0 = (c.y - dc.top).clamp(0, full_h as i32);
                let x1 = (c.x + c.w - dc.left).clamp(0, full_w as i32);
                let y1 = (c.y + c.h - dc.top).clamp(0, full_h as i32);
                if x1 - x0 < 2 || y1 - y0 < 2 {
                    (None, full_w, full_h)
                } else {
                    (
                        Some((x0 as u32, y0 as u32)),
                        (x1 - x0) as u32,
                        (y1 - y0) as u32,
                    )
                }
            }
            None => (None, full_w, full_h),
        };
        let (out_w, out_h) = output_size(src_w, src_h, opts.max_long_edge);

        // ── 3. 采集纹理 ──
        // 带 GDI_COMPATIBLE：鼠标指针要用 GDI 的 DrawIconEx 画上去
        // （DXGI 桌面复制本身不含光标，这是最省事的合成办法）。
        let acq = create_texture(
            &device,
            src_w,
            src_h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
        )?;

        // ★ 首帧兜底（实测踩过）：DXGI 只在"屏幕有变化"时才给帧。用户按下录屏后如果一动
        //   不动地看一段静态内容，到他动第一下之前一帧都收不到，产物会是个 0 字节的空文件。
        //   所以先用 GDI 抓一张铺进采集纹理当初始内容。
        //
        // ★ 裁了就抓**裁剪后那块**（`origin` 已含裁剪偏移）：抓整屏再裁也一样，但 GDI
        //   白抓了框外的像素，而首帧兜底是在 UI 线程之外抢时间做的。
        let have_frame = prime_with_gdi(
            &context,
            &acq,
            &windows::Win32::Foundation::RECT {
                left: dc.left + crop.map(|(x, _)| x as i32).unwrap_or(0),
                top: dc.top + crop.map(|(_, y)| y as i32).unwrap_or(0),
                right: dc.left + crop.map(|(x, _)| x as i32).unwrap_or(0) + src_w as i32,
                bottom: dc.top + crop.map(|(_, y)| y as i32).unwrap_or(0) + src_h as i32,
            },
            src_w,
            src_h,
        );
        // ★ 首帧兜底失败（极少：奇数宽显示器、或 GDI 抓取瞬时失败）→ 把采集纹理清成黑底，
        //   仍按节奏出帧。绝不走「一直没变化就没首帧 → 整段录不出」的老路：DXGI 只在屏幕
        //   真变化时给帧，若首帧没兜住、且用户录的是静态内容，每一 tick 都会因为没画面被
        //   跳过，最后帧数远少于 tick 数、实际帧率对不上设定（实测 10 设 → 约 6 成片）。
        if !have_frame {
            clear_acq_black(&device, &context, &acq);
        }

        // ── 3b. clean / work 双纹理（光标闪烁修复，2026-10-09）──
        // clean：无光标底图；work：GDI 合成目标。见字段注释。
        // ★★ clean 也必须带 GDI_COMPATIBLE（2026-10-09 补）：放大镜/画中画面板的
        //    合成（composite_pip_frames→blit_pip_frame）就画在 clean 上，走
        //    IDXGISurface1::GetDC —— 该调用要求目标纹理带此标志，缺了每帧报
        //    DXGI_ERROR_INVALID_CALL（日志 1654 次 "pip blit 失败"），面板从此
        //    录不进视频。show_cursor=false 时 read_tex 读的正是 clean，只给
        //    work 加标志覆盖不了这条分支。
        let clean = create_texture(
            &device,
            src_w,
            src_h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
        )?;
        let work = create_texture(
            &device,
            src_w,
            src_h,
            DXGI_FORMAT_B8G8R8A8_UNORM,
            (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
            D3D11_RESOURCE_MISC_GDI_COMPATIBLE.0 as u32,
        )?;
        // 首帧兜底内容同步进 clean（此后 fresh 帧会持续覆盖它）。
        context.CopyResource(&clean, &acq);

        let s = Self {
            device,
            context,
            output1,
            dupl,
            acq,
            src_w,
            src_h,
            crop,
            out_w,
            out_h,
            origin: POINT {
                // ★ 裁了就把裁剪偏移加进来：`draw_cursor` 用「光标屏幕坐标 − origin」定位，
                //   而纹理现在是**裁剪后**的画面，原点就是选框左上角。
                x: dc.left + crop.map(|(x, _)| x as i32).unwrap_or(0),
                y: dc.top + crop.map(|(_, y)| y as i32).unwrap_or(0),
            },
            show_cursor: opts.show_cursor,
            clean,
            work,
            t_acquire_us: 0,
            t_copy_us: 0,
            t_cursor_us: 0,
        };
        // 录制会话存活期间打开面板发布通道（GIF 与 MP4 共用此处，见 `capture_share`）。
        capture_share::set_enabled(true);
        Ok(s)
    }

    /// 回读该读哪张：显示光标时是 `work`（带光标合成），否则 `clean`（底图，
    /// 已含面板合成）。**不再是 `acq`** —— 那是桌面复制的活跃纹理，GDI 不碰它。
    fn read_tex(&self) -> ID3D11Texture2D {
        if self.show_cursor {
            self.work.clone()
        } else {
            self.clean.clone()
        }
    }

    /// 取一帧。返回"这次是否真的拿到了新帧"。
    ///
    /// 无论有没有新帧，`acq` 里都是可用的当前画面（见类型注释）。
    unsafe fn pump(&mut self, timeout_ms: u32) -> std::result::Result<bool, String> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res: Option<IDXGIResource> = None;
        let t_acq = Instant::now();
        let acquired = self.dupl.AcquireNextFrame(timeout_ms, &mut info, &mut res);
        self.t_acquire_us = t_acq.elapsed().as_micros() as u64;
        // 拷贝单独计时：`AcquireNextFrame` 里也包含"等桌面变化"的超时，
        // 混在一起就分不清"等帧"和"搬像素"各占多少。
        let t_copy = Instant::now();
        let fresh = match acquired {
            Ok(()) => {
                if let Some(r) = res.as_ref() {
                    let src: ID3D11Texture2D =
                        r.cast().map_err(|e| format!("帧转纹理失败: {e}"))?;
                    let dst: ID3D11Resource =
                        self.acq.cast().map_err(|e| format!("纹理转型失败: {e}"))?;
                    let from: ID3D11Resource =
                        src.cast().map_err(|e| format!("纹理转型失败: {e}"))?;
                    match self.crop {
                        // 整屏：整资源拷贝（老路径）。
                        None => self.context.CopyResource(Some(&dst), Some(&from)),
                        // ★ 选框：只搬框内那块。源盒用**源纹理**坐标系（整块显示器），
                        //   目标落在 (0,0)，于是 `acq` 天生就是裁完的画面 —— 下游的缩放、
                        //   光标合成、封装器一行都不用知道"裁剪"这件事。
                        Some((cx, cy)) => self.context.CopySubresourceRegion(
                            Some(&dst),
                            0,
                            0,
                            0,
                            0,
                            Some(&from),
                            0,
                            Some(&D3D11_BOX {
                                left: cx,
                                top: cy,
                                front: 0,
                                right: cx + self.src_w,
                                bottom: cy + self.src_h,
                                back: 1,
                            }),
                        ),
                    }
                    true
                } else {
                    false
                }
            }
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => false,
            Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                // 显示器重排 / 显卡驱动重置 / 分辨率变化会让 duplication 失效：重建即可。
                // 重建不了就体面收尾——已经写进文件的那部分仍在盘上。
                log::warn!("桌面复制失效，尝试重建");
                let mut new_dup: Option<IDXGIOutputDuplication> = None;
                duplicate_with_retry(&self.output1, &self.device, &mut new_dup)?;
                match new_dup {
                    Some(d) => self.dupl = d,
                    None => return Err("桌面复制失效且无法重建".to_string()),
                }
                false
            }
            Err(e) => return Err(format!("AcquireNextFrame 失败: {e:?}")),
        };
        self.t_copy_us = t_copy.elapsed().as_micros() as u64;
        if fresh {
            // ★ 只有真的拿到帧才 ReleaseFrame：超时那次没有帧可放（放了会返回
            //   DXGI_INVALID_CALL，下一次 Acquire 直接卡死）。
            let _ = self.dupl.ReleaseFrame();
            // 新桌面画面（不含光标）拷进 clean 底图。之后的 GDI 合成全部只碰
            // clean / work，与桌面复制的活跃纹理再无瓜葛。
            unsafe { self.context.CopyResource(&self.clean, &self.acq) };
        }
        // ★ 放大镜面板合成（见 `capture_share` 模块头）：面板窗对捕获系统级排除
        //   （防套娃），视频里这块画面由管线自己贴——面板窗发布最新位图到共享槽，
        //   这里取来贴进**底图**。贴在光标**之前**：屏幕上光标悬在放大窗上方，
        //   视频里保持同一层序。超时帧也照贴（面板 30FPS 自更新，不等桌面变化）。
        //   ★ 同为 GDI 通道，必须画在自己的纹理上（闪烁修复，同 draw_cursor）。
        composite_pip_frames(&self.clean, self.origin, self.src_w, self.src_h);
        if self.show_cursor {
            // ★ 每帧从 clean 底图拷到 work 再画光标：GDI 绝不直接写 acq。
            //   （旧实现 GDI 直写 acq，桌面持续更新时 ~1/3 帧光标丢失——
            //   实测 TIM 里 I-beam 闪烁 93/272 帧，2026-10-09。）
            unsafe { self.context.CopyResource(&self.work, &self.clean) };
            // 失败不致命：少个光标也比整段录不出来好。
            let t_cur = Instant::now();
            let _ = draw_cursor(&self.work, self.origin);
            self.t_cursor_us = t_cur.elapsed().as_micros() as u64;
        } else {
            self.t_cursor_us = 0;
        }
        Ok(fresh)
    }

    fn out_size(&self) -> (u32, u32) {
        (self.out_w, self.out_h)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // 录制会话结束（含任何错误路径的提前 return）：关掉面板发布通道并清空
        // 槽，别让过期面板画面留到下一次录制（见 `capture_share::set_enabled`）。
        capture_share::set_enabled(false);
    }
}

/// 输出尺寸决策：`max_long_edge == 0` 表示"原始分辨率"（只把奇数边向下取偶）。
///
/// H264 与 GIF 都要求偶数边长；纯函数，于是没有 GPU 也能钉住"720p 档产出 1280×720"。
fn output_size(src_w: u32, src_h: u32, max_long_edge: u32) -> (u32, u32) {
    if max_long_edge == 0 {
        (src_w & !1, src_h & !1)
    } else {
        gif_enc::fit_within(src_w, src_h, max_long_edge)
    }
}
// ─────────────────────────── MP4 通路 ───────────────────────────

/// 采集线程交给编码线程的一批数据。
///
/// ★ 只装**系统内存字节**：编码线程用的是另一台 D3D 设备，GPU 纹理跨不过去（见
///   [`EncConfig`] 上的解耦说明）。这是"解耦"的唯一过界形式。
enum EncMsg {
    /// 一帧 **BGRA**（原生尺寸，紧密排布；缩放留给编码侧的转换器）。
    Frame { ts: i64, dur: i64, bgra: Vec<u8> },
    /// 一批 PCM（48kHz 立体声 i16）。
    Audio { ts: i64, pcm: Vec<i16> },
}

/// 有界队列的容量（帧的份数）。
///
/// ★ 它同时就是**允许积压的上限**：满了就丢帧（有界队列 + 丢帧的标准形状）。
///   1080p 一帧 BGRA 约 8MB ⇒ 峰值约 48MB，与"录屏时内存别失控"相容；
///   调大只是把"编码慢"从"丢帧"换成"延迟越来越高"，并不让编码变快。
const ENC_QUEUE_CAP: usize = 6;

/// 采集侧最多等编码线程收尾多少秒（超过就如实报"楔住"，绝无限等待）。
const ENC_FINISH_SECS: u64 = 12;

/// 编码线程轮询通道的间隔。
///
/// ★ 不能一直阻塞在 `recv()` 上：采集线程**卡死**（楔在 `AcquireNextFrame`）时它既
///   不会关通道、也不会再送帧 —— 阻塞等的话编码线程会陪着一起等下去，而 `moov`
///   只有 `Finalize` 才写 ⇒ 卡死 = 内容全丢。轮询让编码线程有机会**自己收尾**。
const ENC_POLL: Duration = Duration::from_millis(500);

/// 停止位已置、且这么久没收到新帧 ⇒ 认定采集侧不动了，编码侧自行 `Finalize`。
///
/// ★ 太短会在"采集侧只是慢"的时候误收尾（那会丢掉后面的帧）；1.5s 相当于 45 帧
///   的静默，正常录制绝无可能。
const ENC_IDLE_GRACE: Duration = Duration::from_millis(1500);

/// 建编码侧（转换器 + SinkWriter）需要的参数：都是 `Copy`/自有数据，好整体搬进线程。
struct EncConfig {
    path: std::path::PathBuf,
    fps: u32,
    bitrate_kbps: Option<u32>,
    /// 送入转换器的**原生**尺寸（采集侧回读的就是这个尺寸）。
    src_w: u32,
    src_h: u32,
    /// 编码器的输出尺寸（缩放后的结论）。
    out_w: u32,
    out_h: u32,
    has_audio: bool,
    hardware: Hardware,
    /// UI 侧的停止位：编码线程据此判断"采集侧是不是该停了 / 卡了"。
    stop: Arc<AtomicBool>,
    /// 故障注入（见 `maybe_wedge`）。
    wedge_at: Option<String>,
}

/// 编码线程的句柄。**任何退出路径（含 `?` 提前返回）都会限时收尾**。
///
/// # Why（为什么它必须是个类型）
/// 采集线程与编码线程分离之后，"录完了"不等于"文件写完了"：`moov` 是编码线程
/// `Finalize` 才写的。若采集线程出错直接返回、不等编码线程，进程可能在 `moov`
/// 落盘之前就退出，产物又变成"有几 MB、放不了"的幌子 —— 2026-09-19 那次事故的
/// 同款形态。所以收尾挂在 `Drop` 上：出错路径也跑，且**限时**（见
/// [`join_bounded`]），楔住时置 `wedged` 而不是把调用方拖死。
struct EncHandle {
    tx: Option<std::sync::mpsc::SyncSender<EncMsg>>,
    join: Option<std::thread::JoinHandle<()>>,
    /// 编码线程的结论（它自己写，采集侧读）。
    result: Arc<Mutex<Option<std::result::Result<(), String>>>>,
    stats: Arc<RecShared>,
    progress: Arc<RecProgress>,
}

impl EncHandle {
    /// 投一条消息。**队列满就退还**（`EncGone::Full` 里带着原消息），绝不阻塞采集节奏。
    fn send(&self, msg: EncMsg) -> std::result::Result<(), EncGone> {
        let Some(tx) = self.tx.as_ref() else {
            return Err(EncGone::Closed);
        };
        tx.try_send(msg).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(m) => EncGone::Full(m),
            std::sync::mpsc::TrySendError::Disconnected(_) => EncGone::Closed,
        })
    }
    /// 关通道并限时等编码线程收尾。返回编码侧的结论。
    fn finish(mut self) -> std::result::Result<(), String> {
        // 先关通道：`rx` 上的 `for` 循环才会结束，编码线程才会 `Finalize`。
        self.tx = None;
        let Some(join) = self.join.take() else {
            return Ok(());
        };
        if !join_bounded(join, ENC_FINISH_SECS) {
            // ★ 读**编码侧**那份阶段：共用一份的话这里会报出采集侧最后 touch 的
            //   阶段（`Readback`），指不到真正卡住的 `WriteSample` / `Finalize`。
            let stage = self.progress.read_enc();
            self.stats.set_wedged();
            return Err(format!(
                "编码线程收尾超时（>{ENC_FINISH_SECS}s 未退出，停在「{}」，已写入 {} 帧），产物可能缺索引",
                stage.name(),
                self.stats.written()
            ));
        }
        match self.result.lock().ok().and_then(|mut g| g.take()) {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(e),
            None => Err("编码线程提前退出，未给出结论（详见日志）".to_string()),
        }
    }
}

impl Drop for EncHandle {
    fn drop(&mut self) {
        // ★ `finish()` 走过之后 `join` 已是 `None`，这里什么也不做；
        //   只在**提前返回**的路径上兜底（照样限时，绝不无限等）。
        self.tx = None;
        if let Some(j) = self.join.take() {
            if !join_bounded(j, ENC_FINISH_SECS) {
                self.stats.set_wedged();
                log::warn!("编码线程收尾超过 {ENC_FINISH_SECS}s（提前返回路径），产物可能缺索引");
            }
        }
    }
}

/// 编码队列的两种"没送进去"。
enum EncGone {
    /// 队列满：**原消息退还**（帧可以直接丢，音频必须攒着重发）。
    Full(EncMsg),
    /// 编码线程已经不在了（出错退出 / panic）：采集侧该收摊了。
    Closed,
}

/// 录制主循环（MP4）。任何一步失败都返回错误串，交给 `record_thread` 收尾。
///
/// # ★ 采集 / 编码解耦（2026-10-06，A 方案）
/// 采集（DXGI 桌面复制 + 光标 + 回读）留在**本线程**；MF（BGRA→NV12 转换器 + SinkWriter）
/// 整体搬到**编码线程**、并配**另一台 D3D 设备**。两边只以系统内存字节过界。
///   · `ID3D11DeviceContext` 是单线程语义：采集与 MF 共用一台设备会在驱动里互等，
///     2026-10-05 实测第 1259s 必楔一次，楔住后桌面复制（进程级单例）被占死。
///   · 跨设备共享纹理走不通（共享句柄解决"跨设备拷贝"，不解决"MF 能读"）。
///   · 编码慢/楔住时，采集侧继续按节奏取帧、越界就丢 —— 卡顿降级为"掉几帧"，
///     而不是"整条链一起停"。专业录播软件同款形状（有界环形缓冲 +
///     拿不到槽立即丢 + 写盘独立线程）。
unsafe fn run_mp4(
    opts: &RecOptions,
    stop: &Arc<AtomicBool>,
    stats: &Arc<RecShared>,
    progress: &Arc<RecProgress>,
) -> std::result::Result<(), String> {
    let mut sess = Session::open(opts)?;
    let (out_w, out_h) = sess.out_size();
    stats.set_out_size(out_w, out_h);

    // ── 采集侧只留一张中转纹理：把当前画面读回系统内存过界给编码线程 ──
    let staging = create_staging_texture(&sess.device, sess.src_w, sess.src_h)?;

    // `CURSEEN_REC_WEDGE_AT=<阶段>` → 走到该阶段就**永久阻塞**，模拟 MF/驱动把录制
    // 线程楔死（真实现场无法按需复现）。与"注入错误返回"互补：那条验证"出错也要能
    // 收尾成可播文件"，这条验证**"楔住也不能拖死软件"**——2026-10-06 定的原则性要求。
    // 判据见 `tests/probes/rec_wedge.rs`。不设这个变量时只在循环里多一次字符串比较。
    // ★ 采集侧认 `acquire`，编码侧认 `encode` / `finalize`（见 `encode_worker`）。
    let wedge_at: Option<String> = std::env::var("CURSEEN_REC_WEDGE_AT").ok();

    // ── 起编码线程（MF 全在它上面，独立 D3D 设备）──
    let (tx, rx) = std::sync::mpsc::sync_channel::<EncMsg>(ENC_QUEUE_CAP);
    let enc_result = Arc::new(Mutex::new(None::<std::result::Result<(), String>>));
    let cfg = EncConfig {
        path: opts.path.clone(),
        fps: opts.fps,
        bitrate_kbps: opts.bitrate_kbps,
        src_w: sess.src_w,
        src_h: sess.src_h,
        out_w,
        out_h,
        has_audio: opts.capture_mic || opts.capture_speaker,
        hardware: opts.hardware,
        stop: stop.clone(),
        wedge_at: wedge_at.clone(),
    };
    // ★ 线程要 `move` 走 `cfg`，而下面还要读它的尺寸/音频开关：先留一份标量。
    let has_audio = cfg.has_audio;
    let t_stats = stats.clone();
    let t_progress = progress.clone();
    let t_result = enc_result.clone();
    let enc_join = std::thread::Builder::new()
        .name("curseen-rec-encode".into())
        .spawn(move || {
            let r = unsafe { encode_worker(&cfg, &rx, &t_stats, &t_progress) };
            // ★ 结论要**既**进槽（给采集侧汇总）**又**进日志：采集侧在"编码线程已
            //   退出"这条路上拿不到它的话，现场就只剩一句没有原因的"中止"。
            if let Err(ref e) = r {
                log::error!("编码线程失败: {e}");
            }
            if let Ok(mut g) = t_result.lock() {
                *g = Some(r);
            }
        })
        .map_err(|e| format!("起编码线程失败: {e}"))?;
    let enc = EncHandle {
        tx: Some(tx),
        join: Some(enc_join),
        result: enc_result,
        stats: stats.clone(),
        progress: progress.clone(),
    };

    // ── 音频采集（WASAPI）──
    //   仍在采集线程：drain 很便宜，样本同样以字节过界（音频**不丢**，见主循环）。
    let audio_capture = if has_audio {
        crate::platform::audio_capture::AudioCapture::start(
            opts.capture_mic,
            opts.capture_speaker,
            &opts.mic_denoise,
            &opts.mic_device,
            &opts.speaker_device,
        )
    } else {
        None
    };

    // ── 主循环 ──
    let fps = opts.fps.clamp(5, 60);
    let frame_dur = Duration::from_nanos(1_000_000_000 / fps as u64);
    // 一份 100ns 为单位的帧时长（MF 的时间基准）：只有**首帧**用它，之后一律按
    // 真实间隔算，见下面 `dur` 的注释。
    let dur_100ns = (10_000_000i64 / fps as i64).max(1);
    // 处理一帧要多久的估计（毫秒）。`AcquireNextFrame` 的超时必须**预留**出这段时间：
    // 屏幕静止时它会占满整个超时才返回，若超时就等于帧长，处理再花掉 7ms，
    // 一帧就成了 40ms —— 实测 3 秒只录到 75 帧而不是 90，视频时长被压成 2.5 秒。
    let mut reserve_ms = 8.0f32;

    // ── 仅测试用的故障注入 ──
    // `CURSEEN_REC_FAULT_AFTER=N` → 第 N 帧之后故意报一次错，模拟"录到一半硬件抽风"
    // （真实现场是 `0x887A0005`，无法按需复现）。用来验证 **中途失败也要把 mp4 收尾成
    // 可播放文件** 这条保证，见 `tests/abort_record_smoke.rs`。
    // 不设这个变量时零影响：只在循环外读一次，循环里只多一次 `Option<u64>` 比较。
    let fault_after: Option<u64> = std::env::var("CURSEEN_REC_FAULT_AFTER")
        .ok()
        .and_then(|v| v.parse().ok());
    // 空音频流兜底（2026-10-06）：loopback 在系统没放声音时**一个包都不产**，麦克风
    // 又没开 ⇒ 音频流整段 0 样本 ⇒ `Finalize` 撞一个 ~10 秒的内部等待（实测
    // 9.9~10.5s，且**只在"当时恰好没声音"时出现**——"保存时卡一下"的不确定性来源）。
    // 按视频时间轴把缺量补成 0 样本：流永不为空，静音本来就是 0，语义无损。
    // `CURSEEN_REC_NO_SILENCE=1` 仅诊断二分用（关掉填充复现旧行为）。
    let silence_fill_off = std::env::var("CURSEEN_REC_NO_SILENCE").is_ok();
    // 音频流时间线：下一批样本的起始时刻（100ns，会话内，与视频 ts 同基准）。
    // ★ **内容驱动**——每批写完就推进"这批的时长"。曾经用视频帧 ts 给每批打戳，
    //   但批里内容是"自上次 drain 攒到的任意长度"，与帧长对不齐 ⇒ 时间线频繁
    //   缺口/重叠，解码出来就是沙沙的电流声（2026-10-06 参考音实测：连续单文件
    //   录制里 5s 标记间隔两次缩成 4.86s = 140ms 内容被吞）。
    let mut audio_ts_next: i64 = 0;
    // 攒着没发出去的音频（队列满时留到下一批，**音频绝不丢**：丢了就是时间线缺口，
    // 解码出来正是 2026-10-06 修掉的那片电流声）。
    let mut pending_pcm: Vec<i16> = Vec::new();
    // 编码线程是否已经不在了（出错退出 / panic）。置上后采集侧立刻收摊。
    let mut enc_gone = false;

    let mut tick = 0i64;
    let mut last_ts: Option<i64> = None;
    // 分段耗时（微秒，累计）。★ **不挂在 `CURSEEN_REC_TRACE` 后面**：提权副本由
    // `ShellExecuteW(runas)` 拉起，不一定继承环境变量 —— 实测把 trace 打开也拿不到
    // 一行输出，偏偏排查"帧率只有 5fps"这类问题时最需要它。每帧几次 `Instant::now()`
    // 的开销可忽略，所以默认就记，只保留"每 30 帧一条日志"的节流。
    let mut acc_acq = 0u128;
    let mut acc_copy = 0u128;
    let mut acc_cursor = 0u128;
    // `acc_conv`：采集侧这一步现在是**回读**（像素过界），不再是 BGRA→NV12 转换
    // （转换随 MF 一起搬到编码线程了）。
    let mut acc_conv = 0u128;
    let mut acc_audio = 0u128;
    let start = Instant::now();
    stats.mark_started(start);

    while !stop.load(Ordering::Relaxed) {
        let deadline = frame_deadline(start, frame_dur, tick);
        let remaining = deadline.saturating_duration_since(Instant::now());
        let reserve = Duration::from_secs_f32((reserve_ms / 1000.0).max(0.0));
        let timeout = remaining.saturating_sub(reserve).as_millis().min(500) as u32;
        progress.touch(RecStage::Acquire);
        maybe_wedge(wedge_at.as_deref(), "acquire", tick);
        sess.pump(timeout)?;
        acc_acq += sess.t_acquire_us as u128;
        acc_copy += sess.t_copy_us as u128;
        acc_cursor += sess.t_cursor_us as u128;

        // ★ 每 tick 恒写一帧：`acq` 里永远是「上一帧」。这样成片帧率 = 设定值、
        //   时长分毫不差，不再因「静态期跳过写帧」而掉帧（10 设 → 约 6）。
        let processed = Instant::now();

        // 采集纹理 → 转换器 → NV12 纹理 → SinkWriter
        //
        // ★ 时间戳取**挂钟**而不是 `帧序号 × 帧长`：编码一帧的实际耗时会抖动（鼠标
        //   光标合成、GPU 忙…），按序号算的话少录的帧会把视频"压紧"——实测 3 秒只
        //   录到 73 帧，视频就只剩 2.43 秒，放出来快了 23%。挂钟时间戳下，掉帧只是
        //   掉帧，时长仍然分毫不差。
        let ts = (Instant::now() - start).as_nanos() as i64 / 100;
        // 本帧的"显示时长" = 到下一帧之间的真实间隔，不是固定的 1/fps。
        // 固定值会让总时长 = 帧数 × 1/fps：掉帧时视频就被压紧（实测 3 秒录出 2.5 秒）。
        let dur = match last_ts {
            Some(prev) => (ts - prev).max(1),
            None => dur_100ns,
        };
        last_ts = Some(ts);
        progress.touch(RecStage::Readback);
        let t_conv = Instant::now();
        // 把当前画面读回系统内存 —— 这是采集侧与编码侧之间**唯一**的过界形式。
        let bgra = readback_bgra(
            &sess.context,
            &staging,
            sess.read_tex(),
            sess.src_w,
            sess.src_h,
        )?;
        acc_conv += t_conv.elapsed().as_micros();
        match enc.send(EncMsg::Frame { ts, dur, bgra }) {
            Ok(()) => {}
            Err(EncGone::Full(_)) => {
                // ★ 编码跟不上就丢这一帧，采集侧继续按节奏跑 —— 卡顿降级为"掉几帧"，
                //   而不是"整条链一起停"。时间戳是挂钟算的，丢帧只让下一帧的 `dur`
                //   变长，总时长分毫不差。
                stats.add_dropped(1);
            }
            Err(EncGone::Closed) => {
                enc_gone = true;
                break;
            }
        }

        // ── 音频：drain 出 PCM 后过界给编码线程（**绝不丢**）──
        //   丢一帧画面只是掉帧；丢一段音频就是时间线上的空洞，解码出来正是
        //   2026-10-06 修掉的那片电流声。所以发不出去就攒到下一批一起发。
        if let Some(ref ac) = audio_capture {
            progress.touch(RecStage::Audio);
            let t_aud = Instant::now();
            pending_pcm.extend(take_audio_batch(ac, ts, audio_ts_next, silence_fill_off));
            if !pending_pcm.is_empty() {
                let n = pending_pcm.len();
                let pcm = std::mem::take(&mut pending_pcm);
                match enc.send(EncMsg::Audio {
                    ts: audio_ts_next,
                    pcm,
                }) {
                    Ok(()) => audio_ts_next += n as i64 * 10_000_000 / 96_000,
                    Err(EncGone::Full(EncMsg::Audio { pcm: back, .. })) => {
                        // 攒着：下一批连同新样本一起发（`ts` 仍是这批的起点）。
                        pending_pcm = back;
                    }
                    _ => {
                        enc_gone = true;
                        break;
                    }
                }
            }
            acc_audio += t_aud.elapsed().as_micros();
        }

        tick += 1;
        // 故障注入：模拟"录到一半硬件抽风"，验证 `SinkFinisher` 的收尾保证。
        if fault_after == Some(tick as u64) {
            return Err("测试注入的中断（CURSEEN_REC_FAULT_AFTER）".to_string());
        }
        stats.set_frames(tick as u64);
        if tick % 30 == 0 {
            let n = tick as f64;
            let avg = |x: u128| x as f64 / n / 1000.0;
            // ★ 采集侧的"编码"耗时已经不存在了（MF 在编码线程）；换成**队列丢弃
            //   帧数** —— 这才是解耦之后"编码跟不跟得上"的唯一现场指标。
            log::info!(
                "rec trace: tick={tick} 实到 {:.1}fps | 每帧 ms：采集 {:.2} 拷贝 {:.2} 光标 {:.2} 回读 {:.2} 音频 {:.2} | 已写 {} 丢 {}",
                n / start.elapsed().as_secs_f64(),
                avg(acc_acq),
                avg(acc_copy),
                avg(acc_cursor),
                avg(acc_conv),
                avg(acc_audio),
                stats.written(),
                stats.dropped(),
            );
        }
        // 用本帧实际耗时更新"预留"估计（EMA + 1ms 余量），下一帧的超时据此缩短。
        reserve_ms = reserve_ms * 0.8 + (processed.elapsed().as_secs_f32() * 1000.0) * 0.2 + 1.0;

        // ★ 截止时刻由「起点 + 帧序号 × 帧长」算，不用累加：`AcquireNextFrame` 与
        //   编码本身都要花时间，累加会把这点开销一帧一帧攒成可见的掉帧（实测 3 秒
        //   只录到 76 帧而不是 90）。绝对时刻天然吸收抖动；落后太多时（系统卡过一下）
        //   `sleep_until` 立即返回，不会去追补几百帧。
        sleep_until(frame_deadline(start, frame_dur, tick));
    }

    // 停止音频采集，等待线程退出；最后一批样本同样过界给编码线程。
    if let Some(ac) = audio_capture {
        progress.touch(RecStage::AudioStop);
        ac.stop();
        // 给采集线程一点时间收尾（WASAPI 的 Stop 是同步的，但线程退出需要时间）。
        std::thread::sleep(Duration::from_millis(50));
        progress.touch(RecStage::Tail);
        let ts = (Instant::now() - start).as_nanos() as i64 / 100;
        pending_pcm.extend(take_audio_batch(&ac, ts, audio_ts_next, silence_fill_off));
    }
    // ★ 尾部音频要**重试**着发出去：此刻编码线程还在消化队列里的剩帧，一次
    //   `try_send` 几乎必然撞 `Full`；只试一次的话最后几百毫秒的声音就没了。
    //   重试上限 20 × 20ms = 400ms，超过就放弃（收尾不能被它拖住）。
    if !enc_gone && !pending_pcm.is_empty() {
        for _ in 0..20 {
            let pcm = std::mem::take(&mut pending_pcm);
            match enc.send(EncMsg::Audio {
                ts: audio_ts_next,
                pcm,
            }) {
                Ok(()) => break,
                Err(EncGone::Full(EncMsg::Audio { pcm: back, .. })) => pending_pcm = back,
                _ => break,
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        if !pending_pcm.is_empty() {
            log::warn!(
                "最后 {} 个音频样本没送进编码队列（收尾超时放弃），末尾约 {:.0}ms 无声",
                pending_pcm.len(),
                pending_pcm.len() as f64 / 96.0
            );
        }
    }

    // 采集侧从这里起就是"等编码收尾"：阶段如实切到 `Finalize`，否则停摆提示会停在
    // 最后一次采集动作（`Readback`）上，看着像"卡在采集"，其实卡的是编码器。
    progress.touch(RecStage::Finalize);
    if enc_gone {
        // ★ 编码线程先没的：把**它**的原因带出去（"编码线程已退出"本身没信息量）。
        let why = enc
            .result
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            .and_then(|r| r.err());
        return Err(
            why.unwrap_or_else(|| "编码线程已退出（未给出原因，可能被 panic 带走）".to_string())
        );
    }
    // ★ 从这里起"录完了"≠"文件写完了"：`moov` 是编码线程 `Finalize` 才写的。
    //   `finish()` 关通道 → 限时等编码线程收尾 → 带回它的结论（楔住就如实标）。
    enc.finish()
}

/// 编码线程：BGRA→NV12 转换 + H264/AAC 写入 MP4 + `Finalize`。
///
/// ★ 跑在**自己的 D3D 设备**上（见 [`run_mp4`] 的解耦说明）：与采集侧**零共享**
///   `ID3D11DeviceContext`。于是所有会楔住的 MF 调用（`ProcessInput` /
///   `ProcessOutput` / `WriteSample` / `Finalize`）都只发生在这条线程上 ——
///   "编码器卡住"最多卡住这一条线程：采集侧照常按节奏取帧（队列满了就丢帧），
///   UI 侧由 [`join_bounded`] 限时兜住（见 [`EncHandle::finish`]）。
///
/// ★ 通道关闭（`rx` 无消息且发送端已 drop）即代表"该收摊了"：排空缓冲里的剩帧，
///   然后 `Finalize` 写 `moov`。
unsafe fn encode_worker(
    cfg: &EncConfig,
    rx: &std::sync::mpsc::Receiver<EncMsg>,
    stats: &Arc<RecShared>,
    progress: &Arc<RecProgress>,
) -> std::result::Result<(), String> {
    // COM 是**按线程**初始化的：编码线程要自己进 MTA（MF 与 CoCreateInstance 都要）。
    let mut com = false;
    if CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() {
        com = true;
    }

    // ── 本线程自己的 D3D 设备 + 设备管理器 ──
    let (device, context, mgr) =
        create_d3d_and_mgr().map_err(|e| format!("建编码侧 D3D 设备失败: {e}"))?;
    // ── 输入纹理（原生尺寸 BGRA）──
    // ★★ 采集侧过界来的是**字节**，而 XVP 配了 D3D 设备之后**只吃 DXGI 样本**
    //    （实测：`Input Sample has no DXGI buffer (passing a software sample to an XVP
    //    configured for hardware is unsupported) 0x80004002`）。所以字节要在**编码侧
    //    这台设备**上重新长成纹理，再包成 DXGI 样本送进去 —— 转换器的配置（输入
    //    DXGI BGRA、输出 NV12）与改动前**完全一致**，只有纹理的归属设备换了。
    let in_tex = create_texture(
        &device,
        cfg.src_w,
        cfg.src_h,
        DXGI_FORMAT_B8G8R8A8_UNORM,
        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        0,
    )?;
    // ── NV12 纹理（输出尺寸）：MFT 不自带输出样本时用它（见下面 `mft_provides_out`）──
    let nv12 = create_texture(
        &device,
        cfg.out_w,
        cfg.out_h,
        DXGI_FORMAT_NV12,
        (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        0,
    )?;

    // ── BGRA → NV12 转换器（系统自带的 Video Processor MFT，走 GPU，顺带缩放）──
    //    输入用**原生尺寸**、输出用**目标尺寸** → 缩放就在这一步免费完成。
    let converter: IMFTransform =
        CoCreateInstance(&CLSID_VideoProcessorMFT, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("创建 VideoProcessorMFT 失败: {e}"))?;
    set_video_type(
        &converter,
        true,
        MFVideoFormat_ARGB32,
        cfg.src_w,
        cfg.src_h,
        cfg.src_w * 4,
    )
    .map_err(|e| format!("设置转换器输入类型失败: {e}"))?;
    set_video_type(
        &converter,
        false,
        MFVideoFormat_NV12,
        cfg.out_w,
        cfg.out_h,
        cfg.out_w,
    )
    .map_err(|e| format!("设置转换器输出类型失败: {e}"))?;
    // ★★ 把**编码侧自己的** D3D 设备交给转换器（不交就会退回 CPU 转换 ~10ms/帧，
    //    并偶发 `0x887A0005` 中止录制）。位置：必须在媒体类型设好之后。
    converter
        .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, mgr.as_raw() as usize)
        .map_err(|e| format!("给转换器设置 D3D 设备失败: {e}"))?;
    converter
        .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
        .map_err(|e| format!("转换器 Flush 失败: {e}"))?;

    // ★★★ 设了设备管理器之后 `MFT_OUTPUT_STREAM_PROVIDES_SAMPLES` 会置上（实测 flags
    //    0x0 → 0x100），此时**输出样本必须由 MFT 自己分配**：继续自己塞一个样本会恒得
    //    `Output Sample is Invalid (0x80070057)`。这里查一次，主循环按结论分支。
    //    矩阵对照见 `tests/mft_probe.rs`。
    let mft_provides_out = converter
        .GetOutputStreamInfo(0)
        .map(|i| i.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0)
        .unwrap_or(false);

    // ── SinkWriter（H264 + AAC → MP4）──
    let bitrate_kbps = cfg
        .bitrate_kbps
        .unwrap_or_else(|| crate::core::config::auto_bitrate_kbps(cfg.out_w, cfg.out_h, cfg.fps));
    let (sink, used_hw) = create_sink_writer(
        &cfg.path,
        cfg.fps,
        bitrate_kbps,
        cfg.out_w,
        cfg.out_h,
        cfg.has_audio,
        cfg.hardware,
        &mgr,
    )?;
    stats.set_hardware(used_hw);
    // ★★ 从这个点起，无论后面哪一步 `?` 提前返回，mp4 都会被补上 `moov` 盒。
    let mut finisher = SinkFinisher(Some(sink.clone()));
    log::info!(
        "MP4 编码器：{}（策略 {}，{}×{} @{}fps，码率 {}）",
        if used_hw {
            "已启用硬件编码"
        } else {
            "软件编码"
        },
        match cfg.hardware {
            Hardware::Auto => "自动",
            Hardware::Off => "关闭",
        },
        cfg.out_w,
        cfg.out_h,
        cfg.fps,
        match cfg.bitrate_kbps {
            Some(k) => format!("{k} kbps（固定）"),
            None => format!("{bitrate_kbps} kbps（自动）"),
        }
    );

    let mut written = 0i64;
    let mut last_msg = Instant::now();
    loop {
        // ★ 见 `ENC_POLL`：轮询而不是阻塞 —— 采集侧卡死时这是唯一能保住 `moov` 的路。
        let msg = match rx.recv_timeout(ENC_POLL) {
            Ok(m) => {
                last_msg = Instant::now();
                m
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if cfg.stop.load(Ordering::Relaxed) && last_msg.elapsed() >= ENC_IDLE_GRACE {
                    // ★ 自救：停止位已置、又静默了这么久 ⇒ 采集侧卡住了（它既不关
                    //   通道也不送帧）。这里自己 `Finalize`，卡死之前录到的那一段
                    //   就还是个能播的文件 —— "录屏出错也不能丢内容"这条底线的
                    //   最后一道保险。
                    log::warn!(
                        "采集侧已停止且 {:.1}s 无新帧（可能卡住了），编码侧自行收尾（已写 {written} 帧）",
                        last_msg.elapsed().as_secs_f64()
                    );
                    break;
                }
                continue;
            }
            // 通道关闭 = 采集侧正常收摊，排空缓冲后收尾。
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match msg {
            EncMsg::Frame { ts, dur, bgra } => {
                progress.touch_enc(RecStage::Convert);
                // 采集侧回读的 BGRA 字节 → 编码侧纹理 → DXGI 样本（XVP 只吃后者）。
                upload_bgra(&context, &in_tex, &bgra, cfg.src_w, cfg.src_h)?;
                let in_sample = sample_from_texture(&in_tex)?;
                in_sample
                    .SetSampleTime(ts)
                    .map_err(|e| format!("SetSampleTime 失败: {e}"))?;
                in_sample
                    .SetSampleDuration(dur)
                    .map_err(|e| format!("SetSampleDuration 失败: {e}"))?;
                converter
                    .ProcessInput(0, &in_sample, 0)
                    .map_err(|e| format!("ProcessInput 失败: {e}"))?;
                let mut buf = MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: if mft_provides_out {
                        std::mem::ManuallyDrop::new(None)
                    } else {
                        std::mem::ManuallyDrop::new(Some(sample_from_texture(&nv12)?))
                    },
                    dwStatus: 0,
                    pEvents: std::mem::ManuallyDrop::new(None),
                };
                let mut status = 0u32;
                let mut pending: Option<IMFSample> = None;
                match converter.ProcessOutput(0, std::slice::from_mut(&mut buf), &mut status) {
                    Ok(()) => {
                        if let Some(s) = std::mem::ManuallyDrop::take(&mut buf.pSample) {
                            let _ = s.SetSampleTime(ts);
                            let _ = s.SetSampleDuration(dur);
                            pending = Some(s);
                        }
                        let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        // 转换器偶尔要先吃两帧才吐一帧：不算错，下一帧继续。
                        let _ = std::mem::ManuallyDrop::take(&mut buf.pSample);
                        let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                    }
                    Err(e) => {
                        let _ = std::mem::ManuallyDrop::take(&mut buf.pSample);
                        let _ = std::mem::ManuallyDrop::take(&mut buf.pEvents);
                        return Err(format!("ProcessOutput 失败: {e}"));
                    }
                }
                if let Some(s) = pending {
                    progress.touch_enc(RecStage::Encode);
                    maybe_wedge(cfg.wedge_at.as_deref(), "encode", written);
                    sink.WriteSample(0, &s)
                        .map_err(|e| format!("WriteSample 失败: {e}"))?;
                    written += 1;
                    stats.add_written(1);
                }
            }
            EncMsg::Audio { ts, pcm } => {
                progress.touch_enc(RecStage::Audio);
                write_audio_samples(&sink, &pcm, ts, 1)?;
            }
        }
    }

    progress.touch_enc(RecStage::Finalize);
    maybe_wedge(cfg.wedge_at.as_deref(), "finalize", i64::MAX);
    sink.Finalize().map_err(|e| format!("收尾 MP4 失败: {e}"))?;
    // ★ 已经收尾过了，告诉守卫别再收一次（重复 `Finalize` 会报错刷日志）。
    finisher.0 = None;
    log::info!(
        "编码线程收尾完成：写入 {written} 帧（采集侧因队列满丢弃 {} 帧）",
        stats.dropped()
    );
    if com {
        CoUninitialize();
    }
    Ok(())
}

/// 建一台**独立**的 D3D11 设备 + 绑定它的 DXGI 设备管理器（编码侧专用）。
///
/// ★ 为什么编码器不能复用采集那台设备：`ID3D11DeviceContext` 是单线程语义，两条
///   线程（采集 / MF）在同一台设备上并发会在驱动里互等 —— 2026-10-05 实测第 1259s
///   必楔一次，且楔住后桌面复制（进程级单例）被占死，后续录制全废。
unsafe fn create_d3d_and_mgr(
) -> std::result::Result<(ID3D11Device, ID3D11DeviceContext, IMFDXGIDeviceManager), String> {
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    D3D11CreateDevice(
        None,
        D3D_DRIVER_TYPE_HARDWARE,
        windows::Win32::Foundation::HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(&levels),
        D3D11_SDK_VERSION,
        Some(&mut device),
        None,
        Some(&mut context),
    )
    .map_err(|e| format!("创建 D3D11 设备失败: {e}"))?;
    let device = device.ok_or_else(|| "创建 D3D11 设备失败（无返回）".to_string())?;
    let context = context.ok_or_else(|| "创建 D3D11 上下文失败".to_string())?;
    let mut token = 0u32;
    let mut mgr: Option<IMFDXGIDeviceManager> = None;
    MFCreateDXGIDeviceManager(&mut token, &mut mgr)
        .map_err(|e| format!("创建 D3D 设备管理器失败: {e}"))?;
    let mgr = mgr.ok_or_else(|| "创建 D3D 设备管理器失败（无返回）".to_string())?;
    let unk: IUnknown = device
        .cast()
        .map_err(|e| format!("D3D 设备转 IUnknown 失败: {e}"))?;
    mgr.ResetDevice(&unk, token)
        .map_err(|e| format!("把 D3D 设备交给设备管理器失败: {e}"))?;
    Ok((device, context, mgr))
}

/// 把一段紧密排布的 BGRA 字节**上传**到一张纹理（`UpdateSubresource`）。
///
/// ★ 采集/编码解耦之后帧以字节过界，而 XVP 只吃 DXGI 样本，所以字节要在编码侧
///   这台设备上重新长成纹理（见 `encode_worker` 里 `in_tex` 的说明）。
unsafe fn upload_bgra(
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    bgra: &[u8],
    w: u32,
    h: u32,
) -> std::result::Result<(), String> {
    let need = w as usize * h as usize * 4;
    if bgra.len() < need {
        return Err(format!(
            "上传帧数据不足：需要 {need} 字节，实际 {}",
            bgra.len()
        ));
    }
    let dst: ID3D11Resource = tex.cast().map_err(|e| format!("纹理转型失败: {e}"))?;
    context.UpdateSubresource(Some(&dst), 0, None, bgra.as_ptr().cast(), w * 4, 0);
    Ok(())
}

/// `IMFSinkWriter` 的收尾守卫：**出错也要 `Finalize`**。
///
/// ★★ mp4 的索引（`moov`）是 `Finalize` 时才写的。中途失败若直接 `return Err`，产物就
/// 只有 `ftyp` + `mdat` —— 有几 MB、看着像录到了，实际任何播放器都打不开（实测
/// 2026-09-19 16:35 那次：录到第 8 秒出 `0x887A0005`，留下 3,518,527 字节的无 `moov` 文件，
/// 而界面报了"录制完成"）。`Finalize` 会把已写入的采样索引补上，于是"崩之前录到的那几秒"
/// 是一段能播的短片。配合 [`FinishOutcome::error`] 如实告知用户"这段是中断的"。
struct SinkFinisher(Option<IMFSinkWriter>);

impl Drop for SinkFinisher {
    fn drop(&mut self) {
        if let Some(s) = self.0.take() {
            if let Err(e) = unsafe { s.Finalize() } {
                log::warn!("收尾 MP4 失败（已录部分可能不可播放）: {e}");
            }
        }
    }
}

/// 断流多久才值得补静音（200ms，100ns 单位）。
///
/// ★ **小于它的缺口一律不补，等真实样本自然续上**。录制循环每帧 drain 一次，
///   而采集线程是按自己的节奏往缓冲攒包的——短缺口只说明"这帧恰好没取到"，
///   真实样本下一帧就到。这时补静音等于**把连续音频拦腰插进一段 0**，每个插点
///   都是一次波形跳变（电流声）；时间线是内容驱动的，短缺口只会让音轨整体
///   相对视频滞后几毫秒，听不出来。loopback 断流超过 200ms 只可能是真没声音
///   （loopback 有声音时每 ~10ms 一个包）。
const SILENCE_FILL_MIN_HNS: i64 = 2_000_000;
/// 单次补静音的 i16 上限（1s 的量，48kHz × 2ch ⇒ 96000/秒）：异常情况下不至于
/// 一把造出天文数字。
const SILENCE_FILL_CAP_I16: usize = 96_000 * 2;

/// 取一批待写音频（2026-10-06 重写，查"电流声"时定稿）。
///
/// 真实样本优先原样返回；取空且断流超过 [`SILENCE_FILL_MIN_HNS`] 时，把缺量补成
/// 静音（单次封顶 1s）。返回值可能为空——短缺口不补，见阈值常量的说明。
///
/// # Why（为什么值得这 40 行）
/// 音频流整段 0 样本时，MF 的 MP4 sink 在 `Finalize` 里会等一个 ~10 秒的内部
/// 超时（180 分钟压测实测 57 次收尾全部 9.9~10.5s；有声音流动时毫秒级）。
/// 这个等待落在**保存阶段**且只在"当时系统恰好没声音"时出现——旧版"不确定性
/// 失败"里"保存时卡死"的那个不确定因素就是它。
fn take_audio_batch(
    ac: &crate::platform::audio_capture::AudioCapture,
    ts: i64,
    audio_ts_next: i64,
    off: bool,
) -> Vec<i16> {
    let taken = ac.take_samples();
    if !taken.is_empty() || off {
        return taken;
    }
    let deficit = ts - audio_ts_next;
    if deficit <= SILENCE_FILL_MIN_HNS {
        return Vec::new();
    }
    // 100ns → i16 个数：deficit/1e7 × 96000 ⇒ deficit × 96 / 10_000。
    let n = ((deficit.min(10_000_000) as usize) * 96 / 10_000).min(SILENCE_FILL_CAP_I16);
    vec![0i16; n]
}

/// 把一段 16-bit 立体声 PCM 打包成 `IMFSample` 写进音频流（stream `stream`）。
unsafe fn write_audio_samples(
    sink: &IMFSinkWriter,
    samples: &[i16],
    ts: i64,
    stream: u32,
) -> std::result::Result<(), String> {
    if samples.is_empty() {
        return Ok(());
    }
    let byte_count = samples.len() * 2;
    let buf: IMFMediaBuffer =
        MFCreateMemoryBuffer(byte_count as u32).map_err(|e| format!("创建音频缓冲区失败: {e}"))?;
    {
        let mut data = std::ptr::null_mut();
        let mut max_len = 0u32;
        buf.Lock(&mut data, Some(&mut max_len), None)
            .map_err(|e| format!("锁定音频缓冲区失败: {e}"))?;
        let dst = std::slice::from_raw_parts_mut(data as *mut i16, samples.len());
        dst.copy_from_slice(samples);
        buf.Unlock()
            .map_err(|e| format!("解锁音频缓冲区失败: {e}"))?;
        buf.SetCurrentLength(byte_count as u32).ok();
    }
    let audio_sample: IMFSample =
        MFCreateSample().map_err(|e| format!("创建音频 Sample 失败: {e}"))?;
    audio_sample
        .AddBuffer(&buf)
        .map_err(|e| format!("音频 AddBuffer 失败: {e}"))?;
    // 音频时间戳与视频对齐（同一挂钟起点）。
    let _ = audio_sample.SetSampleTime(ts);
    // 音频时长 = 采样数 / (采样率 × 声道数) × 10^7（100ns 单位）。
    let audio_dur = (samples.len() as i64 * 10_000_000) / (48000 * 2);
    let _ = audio_sample.SetSampleDuration(audio_dur.max(1));
    sink.WriteSample(stream, &audio_sample)
        .map_err(|e| format!("音频 WriteSample 失败: {e}"))?;
    Ok(())
}

// ─────────────────────────── GIF 通路 ───────────────────────────

/// 定调色板前要**先看多少帧**。
///
/// # ★★ 为什么不能只看首帧（2026-09-20 用户报告「颜色都不太对 + 纯色飘带多了边框」）
///
/// GIF 的全局色表写在**文件头**，落笔之后整段录制都无法更改。所以"用哪一帧选色"
/// 就等于"这段录制的颜色上限"。而取首帧恰恰是最糟的选择：
///
///  · 录 GIF 前的倒计时里鼠标基本不动，等录制真正开始，拖尾（`fade_duration` 默认
///    3 秒）**刚好褪干净** —— 首帧里没有拖尾；
///  · 用户一动鼠标飘带才出现，此时色表早已定稿。飘带是**半透明**的，它压在不同
///    背景上会合成出一整族颜色；这族颜色在色表里**一个都没有**，于是只能落到最近的
///    保色底上 → 色相偏了（"颜色都不太对"）；
///  · 更难看的是**抗锯齿边缘**：飘带轮廓那 1px 是它与背景的过渡色，同样不在色表里，
///    被量化到某个"亮度相近但色相不同"的槽上 → 一条纯色飘带**凭空多出一圈边框**。
///
/// 攒够一段再定表，这些"开始之后才出现"的颜色就是**精确色**。
const PALETTE_WARMUP_FRAMES: usize = 12;

/// 预热期缓存 RGB 的字节上限（按帧数封顶之外再按内存封顶）。
///
/// 预热期要留住原始 RGB（还没有色表，没法先量化），按 `宽 × 高 × 3` 字节/帧计。
/// 1280×720 十二帧约 33MB；大选区（1600×900）会提前停在第 8 帧。写完即释放，
/// 且只发生在录制头一秒，与"录制中 +100MB 是编码器会话固定成本"是两笔账。
const PALETTE_WARMUP_MAX_BYTES: usize = 36 * 1024 * 1024;

/// 把预热窗口攒下的帧**一次性**定表 + 补写，返回可继续追加的写入器与量化器。
///
/// 补写用的是**当时**量到的帧间隔（`warm` 里存着），不是重算 —— 否则整段录制的
/// 时间轴会整体前移一个预热窗口。
fn flush_warmup(
    path: &std::path::Path,
    out_w: u32,
    out_h: u32,
    fps: u32,
    hist: &gif_enc::Hist,
    accents: &[[u8; 3]],
    warm: &mut Vec<(Vec<u8>, u16)>,
) -> std::result::Result<(GifWriter, Quantizer), String> {
    let first = warm.first().map(|f| f.0.as_slice());
    let pal = Palette::from_hist(first, hist, gif_enc::MAX_COLORS, accents)
        .ok_or_else(|| "GIF 调色板建立失败（预热窗口里没有取到任何颜色）".to_string())?;
    log::info!(
        "GIF 录制 {}×{} @{}fps：预热 {} 帧（{} 色，含 {} 色保色底）后定表",
        out_w,
        out_h,
        fps,
        warm.len(),
        pal.len(),
        accents.len()
    );
    // ★ 无限循环**恒开**（用户口径 2026-09-20）：不再是可选项。
    //   GIF 的用途（演示操作、表情包）默认就该循环播放，"播一遍就停在最后一帧"
    //   几乎没人想要。`RecOptions::gif_loop` 只为旧配置兼容保留，不再被读取。
    let mut w = GifWriter::create(path, out_w, out_h, &pal, true)
        .map_err(|e| format!("创建 GIF 失败: {e}"))?;
    let mut q = Quantizer::new(pal);
    let mut idx: Vec<u8> = Vec::new();
    for (rgb, dt) in warm.drain(..) {
        idx.clear();
        q.map_into(&rgb, &mut idx);
        // ★ 走**帧间差量**那条路（见 `GifWriter::write_frame_delta`）：画面没动的帧不写，
        //   变化只写包围盒。屏幕录制里绝大多数时间是"某一小块在动"，这一步同时省了
        //   字节和 LZW 的工作量；静止画面直接零成本。
        w.write_frame_delta(&idx, dt)
            .map_err(|e| format!("写 GIF 帧失败: {e}"))?;
    }
    Ok((w, q))
}

/// GIF 录制主循环：采集 → 读回 → 降采样 → 量化 → GIF。
///
/// 与 MP4 共享 [`Session`]（同一套采集/缩放/光标语义），只是把"进 GPU 编码器"换成
/// "回 CPU 做调色板量化"。
unsafe fn run_gif(
    opts: &RecOptions,
    stop: &AtomicBool,
    stats: &RecShared,
) -> std::result::Result<(), String> {
    let mut sess = Session::open(opts)?;
    let (out_w, out_h) = sess.out_size();
    stats.set_out_size(out_w, out_h);
    // 读回用的中转纹理（CPU 可映射）。
    let staging = create_staging_texture(&sess.device, sess.src_w, sess.src_h)?;

    // 档位由配置层（`GIF_FPS_LABELS`）定，这里只挡住"手改配置改出来的非法值"。
    // ★ 上限是 50 不是 30：GIF 延时的单位是 10ms，50 帧正好是 20ms 一帧，
    //   再高就会落进"浏览器把 <20ms 当 100ms 播"的坑（见 `core::gif_enc` 模块头）。
    let fps = opts.fps.clamp(1, 50);
    let frame_dur = Duration::from_nanos(1_000_000_000 / fps as u64);
    let mut reserve_ms = 12.0f32;

    let start = Instant::now();
    stats.mark_started(start);

    let mut writer: Option<GifWriter> = None;
    let mut quant: Option<Quantizer> = None;
    let mut indices: Vec<u8> = Vec::new();
    let mut prev_cs: i64 = 0;
    let mut tick = 0i64;
    // 保色底（`gif_enc::accent_colors`）是常量表，整段录制算一次就够。
    let accents = gif_enc::accent_colors();
    // 预热窗口：色表写进文件头之后就再也改不了，所以先攒够画面再定表。
    // 用户报告的"飘带偏色 + 纯色飘带多出一圈边框"就出在这里，见 `PALETTE_WARMUP_FRAMES`。
    let mut warm: Vec<(Vec<u8>, u16)> = Vec::new();
    let mut warm_bytes = 0usize;
    let mut hist = gif_enc::Hist::new();

    while !stop.load(Ordering::Relaxed) {
        let deadline = frame_deadline(start, frame_dur, tick);
        let remaining = deadline.saturating_duration_since(Instant::now());
        let reserve = Duration::from_secs_f32((reserve_ms / 1000.0).max(0.0));
        let timeout = remaining.saturating_sub(reserve).as_millis().min(500) as u32;
        sess.pump(timeout)?;

        let processed = Instant::now();
        let bgra = readback_bgra(
            &sess.context,
            &staging,
            sess.read_tex(),
            sess.src_w,
            sess.src_h,
        )?;
        let rgb = gif_enc::downscale_bgra_to_rgb(&bgra, sess.src_w, sess.src_h, out_w, out_h);

        // 延时按**真实间隔**给（GIF 的延时单位是 10ms，只能取整；用差分让累积误差不漂）。
        let now_cs = (Instant::now() - start).as_millis() as i64 / 10;
        let delay_cs = (now_cs - prev_cs).clamp(0, u16::MAX as i64) as u16;
        prev_cs = now_cs;

        if quant.is_none() {
            // ── 预热期：本帧先攒起来，一帧都不写 ─────────────────────────
            // ★ 直方图收的是**全像素**（见 `gif_enc::Hist`）：抽点统计会让色表看不见
            //   某些列上的颜色，于是"颜色对不对"取决于画面落在屏幕哪一侧。
            hist.add_rgb(&rgb);
            warm_bytes += rgb.len();
            warm.push((rgb, delay_cs));
            let enough =
                warm.len() >= PALETTE_WARMUP_FRAMES || warm_bytes >= PALETTE_WARMUP_MAX_BYTES;
            if enough {
                let (w, q) =
                    flush_warmup(&opts.path, out_w, out_h, fps, &hist, &accents, &mut warm)?;
                warm_bytes = 0;
                // 直方图用完即弃：之后每帧直接量化，留着它只是白占 512KB。
                hist = gif_enc::Hist::new();
                quant = Some(q);
                writer = Some(w);
            }
        } else {
            let q = match quant.as_mut() {
                Some(q) => q,
                None => return Err("GIF 量化器缺失".to_string()),
            };
            indices.clear();
            q.map_into(&rgb, &mut indices);
            if let Some(w) = writer.as_mut() {
                // ★ 走**帧间差量**那条路（见 `GifWriter::write_frame_delta`）：画面没动的帧不写，
                //   变化只写包围盒。屏幕录制里绝大多数时间是"某一小块在动"，这一步同时省了
                //   字节和 LZW 的工作量；静止画面直接零成本。
                w.write_frame_delta(&indices, delay_cs)
                    .map_err(|e| format!("写 GIF 帧失败: {e}"))?;
            }
        }

        tick += 1;
        stats.set_frames(tick as u64);
        reserve_ms = reserve_ms * 0.8 + (processed.elapsed().as_secs_f32() * 1000.0) * 0.2 + 1.0;
        sleep_until(frame_deadline(start, frame_dur, tick));
    }

    // 预热期还没攒满就被停掉（按得太快）：攒下的那些帧一样要定表写出去，不能白录。
    if quant.is_none() && !warm.is_empty() {
        let (w, _) = flush_warmup(&opts.path, out_w, out_h, fps, &hist, &accents, &mut warm)?;
        writer = Some(w);
    }

    match writer {
        Some(w) => {
            let written = w.frames();
            w.finish().map_err(|e| format!("GIF 收尾失败: {e}"))?;
            // ★ 采集帧数与写入帧数**分开报**：走差量之后两者不再相等（静止帧不进文件），
            //   差值本身就是"这段录屏有多静"的量化指标 —— 排查"产物太大"时，
            //   这一行是第一手判据（差得少 = 画面一直在动，省不下来）。
            log::info!("GIF 录制完成，采集 {tick} 帧 → 写入 {written} 帧");
            Ok(())
        }
        // 一帧都没录到（起手就被停）：写一个 1×1 的空 GIF，免得留下 0 字节文件让
        // `finish` 报"空文件"这种误导性的原因。
        None => {
            let pal = Palette::from_colors([[0u8, 0, 0]].iter())
                .ok_or_else(|| "GIF 兜底调色板失败".to_string())?;
            let mut w = GifWriter::create(&opts.path, 2, 2, &pal, true)
                .map_err(|e| format!("创建 GIF 失败: {e}"))?;
            let _ = w.write_frame(&[0u8; 4], 100);
            w.finish().map_err(|e| format!("GIF 收尾失败: {e}"))?;
            Ok(())
        }
    }
}

// ─────────────────────────── 通用辅助 ───────────────────────────

/// 第 `tick` 帧应该送出的绝对时刻。
fn frame_deadline(start: Instant, frame_dur: Duration, tick: i64) -> Instant {
    start + frame_dur * tick.max(0) as u32
}

/// 用 GDI 抓一张当前画面铺进采集纹理，作为"第一帧"。
///
/// 返回是否成功。失败不致命：顶多是"屏幕真的静止时没有首帧"，DXGI 的第一帧一到就补上了。
unsafe fn prime_with_gdi(
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
    r: &windows::Win32::Foundation::RECT,
    out_w: u32,
    out_h: u32,
) -> bool {
    let rect = crate::platform::display::ScreenRect {
        x: r.left,
        y: r.top,
        w: r.right - r.left,
        h: r.bottom - r.top,
    };
    // 尺寸必须逐像素对上：DXGI 那边把奇数边长向下取了偶，GDI 抓的是原始尺寸。
    if rect.w != out_w as i32 || rect.h != out_h as i32 {
        return false;
    }
    let Ok(shot) = crate::platform::screenshot::grab(rect) else {
        return false;
    };
    let dst: ID3D11Resource = match tex.cast() {
        Ok(r) => r,
        Err(_) => return false,
    };
    context.UpdateSubresource(Some(&dst), 0, None, shot.bgra.as_ptr().cast(), out_w * 4, 0);
    true
}

/// 把采集纹理清成不透明黑底（首帧兜底失败时用，避免写进未初始化内容）。
unsafe fn clear_acq_black(
    device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    tex: &ID3D11Texture2D,
) {
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    if device
        .CreateRenderTargetView(tex, None, Some(&mut rtv))
        .is_ok()
    {
        if let Some(v) = rtv {
            context.ClearRenderTargetView(&v, &[0.0_f32, 0.0, 0.0, 1.0]);
        }
    }
}

/// 把一张纹理的像素读回内存（BGRA，紧密排布，行距已压掉）。
///
/// `staging` 必须是同尺寸的 `USAGE_STAGING` + `CPU_ACCESS_READ` 纹理，跨帧复用。
unsafe fn readback_bgra(
    context: &ID3D11DeviceContext,
    staging: &ID3D11Texture2D,
    src: ID3D11Texture2D,
    w: u32,
    h: u32,
) -> std::result::Result<Vec<u8>, String> {
    let dst: ID3D11Resource = staging
        .cast()
        .map_err(|e| format!("staging 转型失败: {e}"))?;
    let from: ID3D11Resource = src.cast().map_err(|e| format!("源纹理转型失败: {e}"))?;
    context.CopyResource(Some(&dst), Some(&from));
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    context
        .Map(&dst, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|e| format!("Map 读回失败: {e}"))?;
    let pitch = mapped.RowPitch as usize;
    let row = w as usize * 4;
    let mut out = Vec::with_capacity(row * h as usize);
    {
        let all = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * h as usize);
        for y in 0..h as usize {
            let s = y * pitch;
            out.extend_from_slice(&all[s..s + row]);
        }
    }
    context.Unmap(&dst, 0);
    Ok(out)
}

/// 睡到 `t`。
///
/// ★ 不能用裸 `thread::sleep`：Windows 默认定时器粒度约 **15.6ms**，`sleep(5ms)` 实际会
///   睡到下一个时钟滴答。实测每帧为此多花 7ms —— 30fps 的 33ms 预算变成 40ms，3 秒只
///   录到 75 帧，而 MP4 的时长是按「帧数 ÷ 声明帧率」算的，于是 3 秒录出 2.5 秒、
///   画面整体快了 20%。
///
/// 所以最后 2ms 改成忙等（`spin_loop`），把这 15ms 的量化误差压到微秒级。
/// 只在最后不到 2ms 里转空循环，CPU 代价可以忽略。
fn sleep_until(t: Instant) {
    loop {
        let now = Instant::now();
        if now >= t {
            return;
        }
        let left = t - now;
        if left > Duration::from_millis(2) {
            std::thread::sleep(left - Duration::from_millis(2));
        } else {
            std::hint::spin_loop();
        }
    }
}

/// 重建退避三段带：前段密集快试（瞬时模式切换几十 ms 就过），后段放宽等系统过渡完。
/// 总窗口 ≈7.25s —— 覆盖慢速过渡（改分辨率/驱动重置 1~2s），又不至于楔在重建上太久。
/// （分级退避的通行形状：带内计数、逐段放宽；参数为适配我们
/// "重建失败就体面收尾"的语义收紧了总时长。）
const DUP_RETRY_BANDS: [(&str, u32, u32); 3] =
    [("快试", 25, 10), ("过渡", 100, 20), ("慢等", 500, 10)];

/// 第 `attempt` 次重试前应睡多久（attempt 从 0 起；越界取最后一段值）。
fn dup_retry_sleep(attempt: u32) -> Duration {
    let mut rest = attempt;
    for &(_, ms, count) in &DUP_RETRY_BANDS {
        if rest < count {
            return Duration::from_millis(ms as u64);
        }
        rest -= count;
    }
    Duration::from_millis(
        DUP_RETRY_BANDS
            .last()
            .map(|&(_, ms, _)| ms as u64)
            .unwrap_or(50),
    )
}

/// 建桌面复制（失败重试若干次：分辨率切换 / 全屏游戏切出时 D3D 需要一点时间）。
unsafe fn duplicate_with_retry(
    output1: &IDXGIOutput1,
    device: &ID3D11Device,
    out: &mut Option<IDXGIOutputDuplication>,
) -> std::result::Result<(), String> {
    let total: u32 = DUP_RETRY_BANDS.iter().map(|&(_, _, c)| c).sum();
    let total_ms: u64 = (0..total)
        .map(|a| dup_retry_sleep(a).as_millis() as u64)
        .sum();
    let mut last = String::new();
    for attempt in 0..total {
        match output1.DuplicateOutput(device) {
            Ok(d) => {
                *out = Some(d);
                return Ok(());
            }
            Err(e) => {
                last = format!("{e}");
                std::thread::sleep(dup_retry_sleep(attempt));
            }
        }
    }
    Err(format!(
        "建立桌面复制失败（重试 {total} 次 / 退避窗口 {total_ms}ms）: {last}"
    ))
}

unsafe fn create_texture(
    device: &ID3D11Device,
    w: u32,
    h: u32,
    format: DXGI_FORMAT,
    bind: u32,
    misc: u32,
) -> std::result::Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind,
        CPUAccessFlags: 0,
        MiscFlags: misc,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut tex))
        .map_err(|e| format!("创建纹理失败: {e}"))?;
    tex.ok_or_else(|| "创建纹理失败（无返回）".to_string())
}

/// 建一张可被 CPU 映射的中转纹理（GIF 读回用）。
unsafe fn create_staging_texture(
    device: &ID3D11Device,
    w: u32,
    h: u32,
) -> std::result::Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    device
        .CreateTexture2D(&desc, None, Some(&mut tex))
        .map_err(|e| format!("创建中转纹理失败: {e}"))?;
    tex.ok_or_else(|| "创建中转纹理失败（无返回）".to_string())
}

/// 给转换器设置输入 / 输出媒体类型。
unsafe fn set_video_type(
    converter: &IMFTransform,
    input: bool,
    subtype: windows::core::GUID,
    w: u32,
    h: u32,
    stride: u32,
) -> WinResult<()> {
    let t: IMFMediaType = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | (h as u64))?;
    t.SetUINT32(&MF_MT_DEFAULT_STRIDE, stride)?;
    if input {
        converter.SetInputType(0, &t, 0)
    } else {
        converter.SetOutputType(0, &t, 0)
    }
}

/// 枚举系统里的**硬件** H264 编码器，返回第一个的友好名。
///
/// 这是"自动识别 GPU"的实现：`MFT_ENUM_FLAG_HARDWARE` 只列硬件 MFT（Intel QSV /
/// NVIDIA NVENC / AMD VCE 注册的都是这一类），软件编码器（Microsoft H264 Encoder）
/// 不在其中。返回 `None` = 本机没有硬件编码器（或是驱动没注册），此时硬件加速选项
/// 必然退回 CPU。
///
/// 不需要 COM 初始化（`MFTEnumEx` 与 `CoTaskMemFree` 都是裸 API）。
///
/// `MFStartup`/`MFShutdown` 是**保险**：MSDN 没写枚举 MFT 必须先起平台，但官方示例都先起。
/// 这一对是引用计数式的（+1 / -1），与正在录制的那条链路各算各的、不会互相踢掉。
/// 本函数只在打开控制面板时跑一次，代价可以忽略。
pub fn hardware_encoder_name() -> Option<String> {
    unsafe {
        let started = MFStartup(MF_VERSION, MFSTARTUP_LITE).is_ok();
        let name = hardware_encoder_name_inner();
        if started {
            let _ = MFShutdown();
        }
        name
    }
}

unsafe fn hardware_encoder_name_inner() -> Option<String> {
    let out_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    // ★ 类型是 `IMFActivate***` 里的"二级指针"那一级：`Option<IMFActivate>` 是裸指针
    //   宽度，所以 `*mut Option<IMFActivate>` ≡ `IMFActivate**`（数组首地址），
    //   传 `&mut arr` 正好对上包装里的 `*mut *mut Option<IMFActivate>`。
    //   上一版把 `arr` 写成三级指针又传 `arr` 本身，编译器拦下了——那是把 null 当出参，
    //   `MFTEnumEx` 会往 null 写数组首地址。
    let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE,
        None,
        Some(&out_type),
        &mut arr,
        &mut count,
    )
    .ok()?;
    if arr.is_null() {
        return None;
    }
    let mut name = None;
    for i in 0..count as usize {
        let slot = arr.add(i);
        // ★ 用 `read` 而不是 `*slot`：`Option<IMFActivate>` 的 Copy 语义会多一次
        //   AddRef，之后数组又被整块释放、没人 Release，引用计数就漏了。
        let act: Option<IMFActivate> = std::ptr::read(slot);
        if act.is_none() {
            continue;
        }
        if name.is_none() {
            if let Some(a) = act.as_ref() {
                // 缓冲式 `GetString`：容量 256 个 u16，够任何驱动名（超出会返回
                // `E_NOT_SUFFICIENT_BUFFER`，这里视作拿不到名字，不影响"有硬件编码器"的判断）。
                let mut buf = [0u16; 256];
                if a.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, None)
                    .is_ok()
                {
                    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                    let s = String::from_utf16_lossy(&buf[..end]);
                    if !s.is_empty() {
                        name = Some(s);
                    }
                }
            }
        }
        drop(act);
    }
    CoTaskMemFree(Some(arr as *const core::ffi::c_void));
    name
}

/// 建 MP4 SinkWriter：输出 H264 + AAC，输入 NV12 + PCM。
///
/// 返回 `(sink, 是否真的用了硬件编码)`。
///
/// `Hardware::Auto` 下的**自动回退**：先带 `MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS`
/// 建；建不出来（老显卡、驱动没装编码器、被虚拟化环境屏蔽）就原样再来一次但关掉它。
/// 这样"硬件加速=自动"永远不会比"关"更糟 —— 最坏也只是退成软件编码。
///
/// ★★ 2026-10-09 修"谎报"：`ENABLE_HARDWARE_TRANSFORMS` 只是**允许** MF 用硬件 MFT，
///    本机没注册硬件编码器时 MF 会**静默**选软件 MFT，而旧版把"允许了"当"用了"返回
///    —— 回落路径也返回 true，收尾日志/UI 全在撒谎。现在开工前先 `MFTEnumEx`
///    （`MFT_ENUM_FLAG_HARDWARE`）查注册表里有没有硬件 H264 编码器：没有就直接走
///    软件路径，不空跑一次注定失败的尝试；有才允许硬件。返回值 = 真实结论。
// ★ clippy::too_many_arguments：Media Foundation 建编码器的参数全集（路径/帧率/码率/
//   宽高/有无音频/硬件档/设备管理器）；两个函数保持镜像签名，自动回退时逐参透传。
#[allow(clippy::too_many_arguments)]
unsafe fn create_sink_writer(
    path: &std::path::Path,
    fps: u32,
    bitrate_kbps: u32,
    w: u32,
    h: u32,
    has_audio: bool,
    hardware: Hardware,
    mgr: &IMFDXGIDeviceManager,
) -> std::result::Result<(IMFSinkWriter, bool), String> {
    let allow_hw = hardware == Hardware::Auto;
    let want_hw = allow_hw && hardware_encoder_name_inner().is_some();
    if allow_hw && !want_hw {
        log::info!("未发现硬件 H264 编码器，直接使用软件编码");
    }
    match create_sink_writer_inner(path, fps, bitrate_kbps, w, h, has_audio, want_hw, mgr) {
        Ok(s) => Ok((s, want_hw)),
        Err(e) if want_hw => {
            log::warn!("硬件编码不可用，回退软件编码（原因：{e}）");
            let s = create_sink_writer_inner(path, fps, bitrate_kbps, w, h, has_audio, false, mgr)?;
            Ok((s, false))
        }
        Err(e) => Err(e),
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_sink_writer_inner(
    path: &std::path::Path,
    fps: u32,
    bitrate_kbps: u32,
    w: u32,
    h: u32,
    has_audio: bool,
    allow_hw: bool,
    mgr: &IMFDXGIDeviceManager,
) -> std::result::Result<IMFSinkWriter, String> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 0).map_err(|e| format!("MFCreateAttributes: {e}"))?;
    let attrs = attrs.ok_or_else(|| "MFCreateAttributes 无返回".to_string())?;
    if allow_hw {
        attrs
            .SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1)
            .map_err(|e| format!("启用硬件编码失败: {e}"))?;
    }
    // ★★ 让它和转换器共用本会话的 D3D 设备。必须**在 `MFCreateSinkWriterFromURL` 之前**
    //    填进属性 —— 编码器 MFT 是随 SinkWriter 一起装配的，建完再设就没用了。
    //    不设的话编码器也自建设备，而我们要它吃的是转换器（在**我们**设备上）产出的
    //    DXGI 样本：跨设备又是一次未定义行为，正是 `0x887A0005` 的另一半来源。
    attrs
        .SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, mgr)
        .map_err(|e| format!("给编码器设置 D3D 设备失败: {e}"))?;

    // ★★ 关掉 SinkWriter 的默认**实时节流** + 开低延迟（2026-10-06，修"不确定性
    //    录制失败"）。默认节流是为直播转码设计的：编码器 MFT 忙起来会**反压/拒收**
    //    样本——实测两种表现：产物样本被大量丢弃（打不开/时长缩水），或
    //    `WriteSample` 把录制线程楔死（→ 停止时 UI 跟着卡死 → 强杀 → moov 没写
    //    → 产物打不开）。录屏是"能丢帧也绝不能楔死"的场景，方向与直播相反。
    attrs
        .SetUINT32(&MF_SINK_WRITER_DISABLE_THROTTLING, 1)
        .map_err(|e| format!("关闭节流失败: {e}"))?;
    attrs
        .SetUINT32(&MF_LOW_LATENCY, 1)
        .map_err(|e| format!("设置低延迟失败: {e}"))?;

    // ── 视频流（stream 0）──
    let out_type: IMFMediaType =
        MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    out_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|_| "设置 MAJOR_TYPE 失败".to_string())?;
    out_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
        .map_err(|_| "设置输出 SUBTYPE 失败".to_string())?;
    out_type
        .SetUINT32(&MF_MT_AVG_BITRATE, bitrate_kbps * 1000)
        .map_err(|_| "设置码率失败".to_string())?;
    out_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
        .map_err(|_| "设置帧率失败".to_string())?;
    out_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | (h as u64))
        .map_err(|_| "设置尺寸失败".to_string())?;
    out_type
        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
        .map_err(|_| "设置宽高比失败".to_string())?;
    out_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        .map_err(|_| "设置扫描模式失败".to_string())?;

    let hpath = windows::core::HSTRING::from(path.to_string_lossy().as_ref());
    let sink: IMFSinkWriter = MFCreateSinkWriterFromURL(&hpath, None, Some(&attrs))
        .map_err(|e| format!("创建 SinkWriter 失败: {e}"))?;
    let vstream = sink
        .AddStream(&out_type)
        .map_err(|e| format!("AddStream 失败: {e}"))?;

    let in_type: IMFMediaType =
        MFCreateMediaType().map_err(|e| format!("MFCreateMediaType: {e}"))?;
    in_type
        .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
        .map_err(|_| "设置输入 MAJOR_TYPE 失败".to_string())?;
    in_type
        .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
        .map_err(|_| "设置输入 SUBTYPE 失败".to_string())?;
    in_type
        .SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | (h as u64))
        .map_err(|_| "设置输入尺寸失败".to_string())?;
    in_type
        .SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
        .map_err(|_| "设置输入帧率失败".to_string())?;
    in_type
        .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)
        .map_err(|_| "设置输入宽高比失败".to_string())?;
    in_type
        .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
        .map_err(|_| "设置输入扫描模式失败".to_string())?;
    in_type
        .SetUINT32(&MF_MT_DEFAULT_STRIDE, w)
        .map_err(|_| "设置输入 stride 失败".to_string())?;
    sink.SetInputMediaType(vstream, &in_type, None)
        .map_err(|e| format!("SetInputMediaType 失败: {e}"))?;

    // ── 音频流（stream 1，可选）──
    //    AAC 编码：128kbps / 48kHz / 立体声。
    if has_audio {
        let audio_out: IMFMediaType =
            MFCreateMediaType().map_err(|e| format!("创建音频输出类型失败: {e}"))?;
        audio_out
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|_| "音频 MAJOR_TYPE 失败".to_string())?;
        audio_out
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
            .map_err(|_| "音频 SUBTYPE 失败".to_string())?;
        audio_out
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 16000)
            .map_err(|_| "音频码率失败".to_string())?;
        audio_out
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
            .map_err(|_| "音频采样率失败".to_string())?;
        audio_out
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)
            .map_err(|_| "音频声道数失败".to_string())?;
        audio_out
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|_| "音频位深失败".to_string())?;
        let astream = sink
            .AddStream(&audio_out)
            .map_err(|e| format!("音频 AddStream 失败: {e}"))?;

        let audio_in: IMFMediaType =
            MFCreateMediaType().map_err(|e| format!("创建音频输入类型失败: {e}"))?;
        audio_in
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|_| "音频输入 MAJOR_TYPE 失败".to_string())?;
        audio_in
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
            .map_err(|_| "音频输入 SUBTYPE 失败".to_string())?;
        audio_in
            .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48000)
            .map_err(|_| "音频输入采样率失败".to_string())?;
        audio_in
            .SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)
            .map_err(|_| "音频输入声道数失败".to_string())?;
        audio_in
            .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
            .map_err(|_| "音频输入位深失败".to_string())?;
        audio_in
            .SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)
            .map_err(|_| "音频输入对齐失败".to_string())?;
        audio_in
            .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 48000 * 4)
            .map_err(|_| "音频输入字节率失败".to_string())?;
        sink.SetInputMediaType(astream, &audio_in, None)
            .map_err(|e| format!("音频 SetInputMediaType 失败: {e}"))?;
    }

    sink.BeginWriting()
        .map_err(|e| format!("BeginWriting 失败: {e}"))?;
    Ok(sink)
}

/// 用一张纹理包出一个 `IMFSample`（不拷像素，只是引用）。
unsafe fn sample_from_texture(tex: &ID3D11Texture2D) -> std::result::Result<IMFSample, String> {
    let surface: IDXGISurface = tex
        .cast()
        .map_err(|e| format!("纹理转 IDXGISurface 失败: {e}"))?;
    let buf = MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &surface, 0, false)
        .map_err(|e| format!("MFCreateDXGISurfaceBuffer 失败: {e}"))?;
    let sample: IMFSample = MFCreateSample().map_err(|e| format!("MFCreateSample 失败: {e}"))?;
    sample
        .AddBuffer(&buf)
        .map_err(|e| format!("AddBuffer 失败: {e}"))?;
    Ok(sample)
}

/// 把共享槽里的放大镜面板贴进采集纹理（GDI，与 [`draw_cursor`] 同一条 GetDC 通道）。
///
/// 槽空（没开放大镜 / 面板还没出过画 / 录制已收尾）时零开销。单帧失败不致命：
/// 少贴一帧，下一帧还会再来。
unsafe fn composite_pip_frames(tex: &ID3D11Texture2D, origin: POINT, tex_w: u32, tex_h: u32) {
    for f in &capture_share::snapshot() {
        if let Err(e) = blit_pip_frame(tex, f, origin, tex_w, tex_h) {
            log::warn!("pip blit 失败: {e}");
        }
    }
}

/// 贴一帧放大镜面板：面板矩形（虚拟桌面）→ 本输出纹理坐标，1:1 直拷。
///
/// 失败不致命（`Err` 被调用方忽略）。
unsafe fn blit_pip_frame(
    tex: &ID3D11Texture2D,
    f: &capture_share::PipFrame,
    origin: POINT,
    tex_w: u32,
    tex_h: u32,
) -> WinResult<()> {
    let Some(r) =
        capture_share::pip_blit_rect((f.x, f.y, f.w, f.h), (origin.x, origin.y), (tex_w, tex_h))
    else {
        return Ok(()); // 面板在别的屏上 / 完全落在裁剪区外
    };
    if f.bgra.len() < f.w as usize * f.h as usize * 4 {
        return Ok(()); // 防御：不完整帧不贴
    }
    let surface: IDXGISurface1 = tex.cast()?;
    let hdc = surface.GetDC(false)?;
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: f.w,
            // 负值 = top-down，与面板 DIB 的行序一致（`Panel::create` 同款约定）。
            biHeight: -f.h,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let rows = StretchDIBits(
        hdc,
        r.dst_x,
        r.dst_y,
        r.w,
        r.h,
        r.src_x,
        r.src_y,
        r.w,
        r.h,
        Some(f.bgra.as_ptr().cast()),
        &bi,
        DIB_RGB_COLORS,
        SRCCOPY,
    );
    let _ = surface.ReleaseDC(None);
    // ★ GDI 的失败大多是返回 0 行而不报错——这条 warn 是合成静默失效时的唯一线索
    //   （2026-10-07 排查"录屏里没有放大画面"时钉下的教训）。
    if rows == 0 {
        log::warn!(
            "pip blit StretchDIBits 画了 0 行（目标 {}x{} @ {},{}）",
            r.w,
            r.h,
            r.dst_x,
            r.dst_y
        );
    }
    Ok(())
}

/// 把鼠标指针画到纹理上（GDI）。
///
/// DXGI 桌面复制产出的是"没有光标的那张图"——光标是 DWM 合成时才叠上去的。要录进去
/// 只能自己画：纹理带 `GDI_COMPATIBLE` 就能取到 HDC，`DrawIconEx` 一步到位。
///
/// 失败不致命（`Err` 被忽略）：少个光标也比整段录不出来好。
/// `origin` 为该输出在虚拟桌面中的左上角（物理像素）。`DrawIconEx` 把光标画到"本输出纹理"
/// 上，而 `ci.ptScreenPos` 是**全虚拟桌面**的绝对坐标，故要减去 `origin` 偏移——否则非主屏
/// （位于主屏左/上方，其 `DesktopCoordinates.left/top` 为负）上光标会被画到纹理之外。
unsafe fn draw_cursor(tex: &ID3D11Texture2D, origin: POINT) -> WinResult<()> {
    let mut ci = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    GetCursorInfo(&mut ci)?;
    // ★ 按位测而不是等值：Windows 10+ 还有 CURSOR_SUPPRESSDISPLAYCAPTURE（0x2）
    //   可能与 SHOWING（0x1）同置，等值比较会把光标整帧跳过（防御性修复）。
    if ci.flags.0 & CURSOR_SHOWING.0 == 0 || ci.hCursor.0.is_null() {
        return Ok(());
    }
    let surface: IDXGISurface1 = tex.cast()?;
    let hdc = surface.GetDC(false)?;
    let ok = DrawIconEx(
        hdc,
        ci.ptScreenPos.x - origin.x,
        ci.ptScreenPos.y - origin.y,
        HICON(ci.hCursor.0),
        0,
        0,
        0,
        None,
        DI_NORMAL,
    );
    let _ = surface.ReleaseDC(None);
    ok
}

/// 当前鼠标位置（物理像素）。取不到返回 (0,0)。
fn current_cursor_pos() -> POINT {
    let mut p = POINT::default();
    unsafe {
        if GetCursorPos(std::ptr::addr_of_mut!(p)).is_err() {
            p = POINT { x: 0, y: 0 };
        }
    }
    p
}

/// 按区域策略挑一块显示器。
unsafe fn pick_output(
    adapter: &IDXGIAdapter,
    area: RecordArea,
    cursor: POINT,
    crop: Option<crate::core::geometry::ScreenRect>,
) -> std::result::Result<IDXGIOutput, String> {
    // ★ 有选框就按**选框中心**挑显示器，不再看 `area`：框在哪块屏上就录哪块屏。
    //   照 `RecordArea::Primary` 挑会在双屏上录错那一块（用户框了副屏、产物却是主屏）。
    let probe = crop.map(|c| POINT {
        x: c.x + c.w / 2,
        y: c.y + c.h / 2,
    });
    let mut i = 0u32;
    let mut best: Option<IDXGIOutput> = None;
    while let Ok(o) = adapter.EnumOutputs(i) {
        if best.is_none() {
            best = Some(o.clone());
        }
        if let Some(p) = probe {
            if output_contains(&o, p) {
                return Ok(o);
            }
        } else if area == RecordArea::Cursor && output_contains(&o, cursor) {
            return Ok(o);
        }
        i += 1;
        if i > 16 {
            break;
        }
    }
    best.ok_or_else(|| "没有找到任何显示器".to_string())
}

unsafe fn output_contains(output: &IDXGIOutput, p: POINT) -> bool {
    let Ok(d) = output.GetDesc() else {
        return false;
    };
    let r = d.DesktopCoordinates;
    p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom
}

/// 生成默认文件名：`录制_20260916_021453.mp4`。
pub fn default_file_name() -> String {
    let now = chrono::Local::now();
    format!("录制_{}.mp4", now.format("%Y%m%d_%H%M%S"))
}

/// GIF 默认文件名：`动图_20260916_021453.gif`。
pub fn gif_file_name() -> String {
    let now = chrono::Local::now();
    format!("动图_{}.gif", now.format("%Y%m%d_%H%M%S"))
}

/// 截图默认文件名：`截图_20260916_021453.png`。
pub fn shot_file_name() -> String {
    let now = chrono::Local::now();
    format!("截图_{}.png", now.format("%Y%m%d_%H%M%S"))
}

/// 把动态错误串漏成 `&'static str`（`CurSeenError::Unsupported` 只吃静态串）。
fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

// ─────────────────────── 门禁 ───────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// 分辨率档 → 输出尺寸，是**纯函数**，可以在没有 GPU 的机器上钉住。
    #[test]
    fn resolution_tier_maps_to_expected_output_size() {
        // 0 = 原始（只取偶）
        assert_eq!(output_size(1920, 1080, 0), (1920, 1080));
        assert_eq!(output_size(1921, 1081, 0), (1920, 1080));
        // 1080p 档：4K 屏降一半
        assert_eq!(output_size(3840, 2160, 1920), (1920, 1080));
        // 720p 档
        assert_eq!(output_size(1920, 1080, 1280), (1280, 720));
        // 已经比档位小 → 不上采样
        assert_eq!(output_size(1280, 720, 1920), (1280, 720));
    }

    /// 重建退避的三段带映射：断言必须带量（25/100/500ms 与各段边界）。
    #[test]
    fn dup_retry_sleep_follows_three_bands() {
        let ms = |a: u32| dup_retry_sleep(a).as_millis() as u32;
        // 快试带：attempt 0..10 → 25ms
        assert_eq!(ms(0), 25);
        assert_eq!(ms(9), 25);
        // 过渡带：attempt 10..30 → 100ms
        assert_eq!(ms(10), 100);
        assert_eq!(ms(29), 100);
        // 慢等带：attempt 30..40 → 500ms
        assert_eq!(ms(30), 500);
        assert_eq!(ms(39), 500);
        // 越界（不会发生，但函数必须单调不炸）→ 收敛到最后一段
        assert_eq!(ms(40), 500);
        assert_eq!(ms(400), 500);
        // 总窗口 = 25×10 + 100×20 + 500×10 = 7250ms
        let total: u64 = (0..40u32)
            .map(|a| dup_retry_sleep(a).as_millis() as u64)
            .sum();
        assert_eq!(total, 7250);
    }
}
