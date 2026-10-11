//! 崩溃兜底：把**未处理的结构化异常**（SEH）落盘。
//!
//! 与 [`crate::core::logger::install_panic_hook`] 的分工是**两条不相交的通道**：
//!
//! | 通道 | 覆盖的崩溃 | 现有手段 |
//! |---|---|---|
//! | Rust `panic!` | 数组越界、`unwrap`、字符边界、断言…… | panic hook（已有，实测能工作） |
//! | 结构化异常（SEH） | 访问冲突 `0xC0000005`、栈溢出 `0xC00000FD`、除零 `0xC0000094`、非法指令 | **本模块** |
//!
//! 为什么两条都得装：`panic = "abort"` 下 Rust 的 `abort()` 走 `__fastfail`，**绕过 SEH**，
//! 所以 SEH 兜底抓不到 panic；反过来 SEH 异常也**不经过** Rust 的 panic 通道，panic hook
//! 一声不响。只装一条，另一条通道上的崩溃就仍然是"程序毫无征兆地没了"。
//!
//! 落盘：`<程序目录>/log/curseen-crash-YYYYMMDD-HHMMSS.log`，内含异常码、异常地址、
//! 出错线程、栈回溯地址、exe 构建指纹，以及**当天日志的尾部** —— 崩溃现场最值钱的信息是
//! "崩之前程序正在做什么"，而这一条只有日志能回答。
//!
//! 实现上刻意**不分配堆**：文本走静态缓冲 + `core::fmt::Write`，只有崩溃文件路径做一次
//! `PathBuf::join`。崩溃时堆往往已不可信（访问冲突的常见成因就是野指针写坏了堆），
//! 对齐 `.ex/pecia/core/CrashReport.cpp` 的做法（那里用 C 静态缓冲）。
//!
//! 分析时注意：`strip = true` 的发布版**没有符号**，栈里只有地址。定位靠三样东西 ——
//! "当天日志尾部"（回答"崩在哪一步"）、本文件登记的 exe 构建指纹（大小 / 链接时间）、
//! 以及异常码本身（访问冲突 / 栈溢出 / 除零已能区分大半类问题）。`md5` **不适合**当
//! "同一份产物"的判据：2026-09-18 实测证明源码一字不改重编它也会变（PE 时间戳 +
//! 链接器 GUID 每次构建都不同）。

use std::io::{Read, Seek, SeekFrom};
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};

// 注：这两个 API 在 windows crate 里挂在 `Win32_System_Kernel` feature 下（不是
// `Win32_System_Diagnostics_Debug`），见 Cargo.toml 的注释。
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
};

/// 崩溃文本缓冲容量（静态，崩溃时不再向堆要内存）。
const CAP: usize = 32 * 1024;
/// 日志尾部最多读多少字节（再从中取最后 [`TAIL_LINES`] 行）。
const TAIL_BYTES: usize = 8 * 1024;
/// 日志尾部保留多少行。
const TAIL_LINES: usize = 40;

/// 提示框只弹一次：钩子线程反复崩时不刷屏。
static NOTIFIED: AtomicBool = AtomicBool::new(false);

// ── 出错线程的身份 ────────────────────────────────────────────────────────
//
// 发布版栈全是 <unknown>，"崩在哪条线程"只能看到裸 ID（如 19064）——而 ID 谁也
// 对不上号（2026-10-11 崩溃实测：用户只开了截图贴图+剪贴板，仍要在五六条候选
// 线程里猜）。SEH filter 就跑在**出错线程上**，`std::thread::current().name()`
// 直接就是它的 Builder 名字——所有后台线程都已用 `Builder::new().name(...)` 命名
// （无名裸线程已清零），所以这里免费拿到身份，无需登记表。
fn current_thread_label() -> String {
    match std::thread::current().name() {
        Some(n) => format!("（{}）", n),
        None => String::new(),
    }
}

/// 崩溃文本缓冲。
///
/// 用 `UnsafeCell` + 手写 `unsafe impl Sync`，而不是 `Mutex` 或 `SyncUnsafeCell`：
/// 前者崩溃路径上不能再依赖锁（持有锁的线程可能正是崩掉的那个），后者在 1.94 仍是
/// unstable（`sync_unsafe_cell`）。
struct CrashBuf(core::cell::UnsafeCell<[u8; CAP]>);

// SAFETY: 只有本模块的 `Sink::write_str` 会取 `&mut`，而它只在 `handler` 里被调用；
// 系统分发未处理异常时只在**一个**线程上跑这个 filter，且 handler 不派生线程、不递归
// 回到同一条路径（它不做任何可能再次触发异常的事）。
unsafe impl Sync for CrashBuf {}

static TEXT: CrashBuf = CrashBuf(core::cell::UnsafeCell::new([0u8; CAP]));

/// 往静态缓冲里追加格式化文本的接收器（无堆分配）。
struct Sink {
    len: usize,
}

impl Sink {
    fn new() -> Self {
        Self { len: 0 }
    }

    fn as_bytes(&self) -> &[u8] {
        // 走 `from_raw_parts` 而不是 `&(*ptr)[..len]`：后者会触发
        // `dangerous_implicit_autorefs`（对裸指针的解引用隐式 autoref），
        // 而这里的裸指针访问本来就是有意的，显式写出来反而更清楚。
        //
        // SAFETY: `TEXT` 是静态的，指针在进程存活期内恒定有效；`len` 恒由 `write_str`
        // 在容量内推进，故 `[0, len)` 始终落在缓冲区内。
        unsafe { core::slice::from_raw_parts(TEXT.0.get() as *const u8, self.len) }
    }
}

impl core::fmt::Write for Sink {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // SAFETY: 见 `CrashBuf` 的 `unsafe impl Sync` 说明 —— 这是这条路径上唯一的可变借出；
        // 切片的长度恒为 `CAP`，写入区间由下面的 `min(CAP - len)` 保证不越界。
        let buf = unsafe { core::slice::from_raw_parts_mut(TEXT.0.get() as *mut u8, CAP) };
        let room = CAP.saturating_sub(self.len);
        let n = s.len().min(room);
        buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

/// 安装 SEH 兜底。**应尽早调用**（`main` 里紧挨着 panic hook），早于任何窗口 / 配置装配，
/// 这样启动阶段的异常也能留证。
pub fn install() {
    // SAFETY: 只注册一个进程级 filter；handler 全程不 panic、不分配堆、必定有返回值。
    unsafe {
        let _ = SetUnhandledExceptionFilter(Some(handler));
    }
}

/// 未处理异常的顶层 filter。
///
/// 约定：**必定返回** `EXCEPTION_EXECUTE_HANDLER`，且自身绝不 panic —— 崩溃处理里再崩一次
/// 就真的什么都不剩了。因此下面每一步都只做"尽力而为"的忽略失败，不 `unwrap`。
unsafe extern "system" fn handler(info: *const EXCEPTION_POINTERS) -> i32 {
    // ★ 第一件事：把系统指针还回去。激光笔这类特效换的是**系统级光标位图**，而这条路
    //   径（访问冲突 / 栈溢出）不经过 Rust 的 panic 通道、也来不及跑 `Drop` —— 不在这里
    //   恢复，用户崩完之后整台机器就没有鼠标指针了。幂等且不分配堆，安全。
    crate::platform::hide_cursor::ensure_shown();

    let mut sink = Sink::new();

    // 异常记录理论上非空；这里仍做防御，因为崩溃路径上"再炸一次"的代价最大。
    let (code, addr, thread_id) = if info.is_null() {
        (0i32, std::ptr::null_mut::<core::ffi::c_void>(), 0u32)
    } else {
        let rec = (*info).ExceptionRecord;
        if rec.is_null() {
            (0, std::ptr::null_mut(), 0)
        } else {
            (
                (*rec).ExceptionCode.0,
                (*rec).ExceptionAddress,
                windows::Win32::System::Threading::GetCurrentThreadId(),
            )
        }
    };

    let now = chrono::Local::now();
    let _ = core::fmt::Write::write_fmt(
        &mut sink,
        format_args!(
            "=== CurSeen 崩溃报告（未处理的结构化异常）===\n\
             时间        {}\n\
             异常码      0x{:08X}{}\n\
             异常地址    {:p}\n\
             出错线程    {}{}\n",
            now.format("%Y-%m-%d %H:%M:%S%.3f"),
            code as u32,
            describe(code),
            addr,
            thread_id,
            current_thread_label()
        ),
    );

    // ── exe 构建指纹（大小 / 链接时间）──
    // 不记 md5：源码一字不改重编它也会变（PE 时间戳 + 链接器 GUID），只配当"这次构建的
    // 产物指纹"，不配当"同一份产物"的判据。大小 + 修改时间才是稳定可比的。
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(md) = std::fs::metadata(&exe) {
            let mtime = md
                .modified()
                .ok()
                .map(|t| {
                    let d: chrono::DateTime<chrono::Local> = t.into();
                    d.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|| "（未知）".to_string());
            let _ = core::fmt::Write::write_fmt(
                &mut sink,
                format_args!(
                    "exe         {} ({} 字节, 链接时间 {})\n",
                    exe.display(),
                    md.len(),
                    mtime
                ),
            );
        }
    }

    // ── 模块基址（exe 的加载地址）──
    //
    // 异常地址是含 ASLR 基址的绝对地址；没有基址它就无法换算成 exe 内的 RVA，
    // 事后拿着同版本 exe/PDB 也对不上号（2026-10-11 崩溃实测教训：报告里只有
    // `异常地址 0x7ff6…`，基址没记，那次崩溃就永远停在"知道地址、对不了行"）。
    // 有了基址：`RVA = 异常地址 − 基址`，配同一次构建的 PDB 即可精确定位。
    let base = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }
        .ok()
        .map(|h| h.0 as usize);
    let _ = core::fmt::Write::write_fmt(
        &mut sink,
        format_args!(
            "exe 基址    {}\n",
            match base {
                Some(b) => format!("0x{b:016X}"),
                None => "（未知）".to_string(),
            }
        ),
    );

    // ── 栈回溯 ──
    //
    // 用 `std::backtrace` 而**不是** `RtlCaptureStackBackTrace`：后者在本项目实测给出的是
    // 一串**栈地址**（相邻帧恒差 8 字节、且不落在任何模块的代码段），正常上下文与异常上下文
    // 都一样 —— 拿去定位等于误导。`std::backtrace` 能借系统 dll 的导出表给出帧名，实测输出里的
    // `CallWindowProcW` / `SendMessageW` / `KiUserCallbackDispatcher` 正是判断"崩在 UI 线程处理
    // 窗口消息途中"的关键线索（2026-09-18 那次闪退就是这样读出来的）。
    //
    // 自己的代码帧仍是 `<unknown>`：发布版 `strip = true` 把符号剥了，要精确到行得配同版本提交。
    // 代价：`force_capture` 会分配内存并可能加载 dbghelp —— 崩溃路径上可接受（反正要退出），
    // 且失败时它退化为纯地址而不是 panic。
    //
    // ★ 帧名要**打折看**：无符号时解析器取"离该地址最近的导出符号"，于是会出现 `strncpy` /
    //   `_chkstk` / `__ImageBase` 这种明显张冠李戴的名字（探针实测，见 §19）。真正可信的是
    //   **链条形状**：`KiUserExceptionDispatcher`（异常从系统进来）→ … → `main` / `invoke_main`
    //   / `BaseThreadInitThunk`（进程启动链），以及系统 dll 的真导出名（`CallWindowProcW`、
    //   `SendMessageW` 这类 —— 它们正是判断"崩在窗口消息处理途中"的依据）。
    let _ = core::fmt::Write::write_fmt(
        &mut sink,
        format_args!(
            "\n--- 栈回溯（自己的帧显示 <unknown>：发布版无符号，需配同版本提交解析）---\n{}",
            std::backtrace::Backtrace::force_capture()
        ),
    );

    // ── 当天日志尾部：崩溃现场"之前发生了什么" ──
    append_log_tail(&mut sink);

    // ── Minidump 落盘 ──
    //
    // 文本报告 + 无符号栈只能看"链条形状"；要**精确到行**必须拿 dump 配同一次构建的
    // PDB（`temp/target/release/curseen.pdb`，勿删）。dump 里带着异常上下文（rip/rsp、
    // 出错线程全栈），WinDbg/cdb 一发就能符号化。堆分配在这里可接受（同 backtrace 的
    // 取舍：反正要退出）。
    let dump_note = write_minidump(&now, info, thread_id);
    let _ = core::fmt::Write::write_fmt(&mut sink, format_args!("{dump_note}\n"));
    prune_old_dumps();

    let _ = core::fmt::Write::write_fmt(
        &mut sink,
        format_args!(
            "\n（提示）把本文件与同目录 log/curseen-*.log 一起提交即可定位。\n\
             栈帧名取自「离该地址最近的导出符号」，可能张冠李戴（发布版 `strip = true` 无符号）；\n\
             优先看异常码与链条形状；**同名 .dmp + 同次构建的 curseen.pdb** 可精确到行。\n"
        ),
    );

    let path = write_report(&now, sink.as_bytes());
    notify(&path, code, thread_id);

    // EXCEPTION_EXECUTE_HANDLER = 1：交给系统按默认流程处置（进程退出）。
    1
}

/// 异常码 → 人话（只覆盖常见的几个，其余留空）。
fn describe(code: i32) -> &'static str {
    match code as u32 {
        0xC000_0005 => "（访问冲突：读写无效地址 —— 通常是野指针 / 空指针 / 已释放对象）",
        0xC000_00FD => "（栈溢出：深递归或超大栈数组）",
        0xC000_0094 => "（整数除零）",
        0xC000_0095 => "（整数溢出）",
        0xC000_001D => "（非法指令）",
        0xC000_008C => "（数组越界 / 缓冲区安全检查失败）",
        0xC000_0135 => "（未处理的自定义异常）",
        0x8000_0003 => "（断点 / 主动触发）",
        _ => "",
    }
}

/// 把崩溃报告写到 `<程序目录>/log/curseen-crash-<时间戳>.log`，返回路径。
fn write_report(now: &chrono::DateTime<chrono::Local>, body: &[u8]) -> String {
    let path = crate::core::paths::log_dir()
        .join(format!("curseen-crash-{}.log", now.format("%Y%m%d-%H%M%S")));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, body);
    path.display().to_string()
}

/// 追加当天日志的尾部若干行（尽力而为）。
fn append_log_tail(sink: &mut Sink) {
    let path = crate::core::paths::log_dir().join(format!(
        "curseen-{}.log",
        chrono::Local::now().format("%Y%m%d")
    ));
    // ★ 先补一行回执到日志，再读它。
    //
    // 日志是"独立线程 + BufWriter"，崩溃时最后几行往往还在缓冲里没落盘；探针实测更极端：
    // 程序启动后不到一秒就崩时，日志文件**还不存在**（第一条消息仍排在队列里）。
    // 补这一行既保证文件存在，又给尾部一个明确的终点 —— 读到它就说明"它的上一行就是崩溃前
    // 最后一条已落盘日志"。真实场景（跑了几分钟才崩）文件早已存在，这行只是让尾部更清楚。
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = std::io::Write::write_all(
            &mut f,
            format!(
                "[{}] [FATAL] 进程因未处理异常终止（详见同目录 curseen-crash-*.log）\n",
                chrono::Local::now().format("%H:%M:%S%.3f")
            )
            .as_bytes(),
        );
        let _ = std::io::Write::flush(&mut f);
    }
    let Ok(mut f) = std::fs::File::open(&path) else {
        let _ = core::fmt::Write::write_fmt(
            sink,
            format_args!("\n--- 日志尾部（{} 打不开，跳过）---\n", path.display()),
        );
        return;
    };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = size.saturating_sub(TAIL_BYTES as u64);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return;
    }
    let mut buf = Vec::with_capacity(TAIL_BYTES);
    if f.read_to_end(&mut buf).is_err() {
        return;
    }
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    let _ = core::fmt::Write::write_fmt(
        sink,
        format_args!(
            "\n--- 当天日志尾部（{}，最后 {} 行）---\n",
            path.display(),
            lines.len() - start
        ),
    );
    for l in &lines[start..] {
        let _ = core::fmt::Write::write_fmt(sink, format_args!("{}\n", l));
    }
}

/// 把崩溃现场写成 Minidump（`<程序目录>/log/curseen-crash-<时间戳>.dmp`）。
///
/// 返回写进文本报告的一行说明（成功=路径，失败=原因）。尽力而为：任何一步失败都不
/// 影响文本报告落盘。`ClientPointers = FALSE`：异常上下文就在本进程地址空间里。
fn write_minidump(
    now: &chrono::DateTime<chrono::Local>,
    info: *const EXCEPTION_POINTERS,
    thread_id: u32,
) -> String {
    let path = crate::core::paths::log_dir()
        .join(format!("curseen-crash-{}.dmp", now.format("%Y%m%d-%H%M%S")));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(file) = std::fs::File::create(&path) else {
        return "minidump   （创建 .dmp 文件失败，只有文本报告）".to_string();
    };
    // dump 文件句柄借 `File` 的 RAII：MiniDumpWriteDump 同步写完才返回，之后 drop 无妨。
    let handle = HANDLE(file.as_raw_handle());
    // info 非空才有异常上下文可带；`Option<结构体>` 不是 FSI 安全布局，故用裸指针表达缺席。
    let exc = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: thread_id,
        ExceptionPointers: info as *mut EXCEPTION_POINTERS,
        ClientPointers: windows::core::BOOL(0),
    };
    let exc_param = if info.is_null() {
        None
    } else {
        Some(std::ptr::from_ref(&exc))
    };
    // SAFETY: 句柄有效、参数结构存活到调用结束；dbghelp 内部分配堆，崩溃路径上可接受。
    // windows 0.62 的包装把 BOOL 成功翻成 Result；失败在报告里降级为一行说明。
    let ok = unsafe {
        MiniDumpWriteDump(
            windows::Win32::System::Threading::GetCurrentProcess(),
            std::process::id(),
            handle,
            MINIDUMP_TYPE(0), // MiniDumpNormal：线程栈足够符号化，体积小
            exc_param,
            None,
            None,
        )
    };
    if ok.is_ok() {
        format!(
            "minidump   {}（配同次构建的 curseen.pdb 可精确到行）",
            path.display()
        )
    } else {
        "minidump   （MiniDumpWriteDump 失败，只有文本报告）".to_string()
    }
}

/// 保留最近 [`KEEP_DUMPS`] 份 `.dmp`，删更旧的。对齐 Pecia 的 `pruneOldDumps`：
/// dump 一份几十 KB~几 MB，无上限堆积会在便携目录里悄悄长个。当前这份刚写出、
/// mtime 最新，必在保留之列。尽力而为，失败不响。
const KEEP_DUMPS: usize = 3;

fn prune_old_dumps() {
    let dir = crate::core::paths::log_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut files: Vec<(std::path::PathBuf, std::time::SystemTime)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "dmp"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((e.path(), m.modified().ok()?))
        })
        .collect();
    files.sort_by_key(|&(_, t)| std::cmp::Reverse(t)); // 新的在前
    for (p, _) in files.into_iter().skip(KEEP_DUMPS) {
        let _ = std::fs::remove_file(p);
    }
}

/// 弹一次提示框，告诉用户崩溃文件在哪 —— 把"毫无征兆"变成"有征兆"。
fn notify(path: &str, code: i32, thread_id: u32) {
    if NOTIFIED.swap(true, Ordering::SeqCst) {
        return;
    }
    // 无人值守时跳过弹框：`examples/crash_probe.rs` 要能自动跑完并检查产物，
    // 一个模态框会把它永久挂住。与 `logger.rs` 的"钩子回调内禁止调用日志"同一类考虑
    // —— 崩溃路径本身也要能被测试驱动。
    if std::env::var_os("CURSEEN_CRASH_NO_DIALOG").is_some() {
        return;
    }
    let msg = format!(
        "CurSeen 遇到未处理的异常，即将退出。\n\n\
         异常码: 0x{:08X}{}\n\
         出错线程: {}\n\n\
         崩溃报告已保存到:\n{}\n\
         请把该文件与同目录的 log\\curseen-*.log 一起发给开发者。",
        code as u32,
        describe(code),
        thread_id,
        path
    );
    let mut wide: Vec<u16> = msg.encode_utf16().collect();
    wide.push(0);
    let mut title: Vec<u16> = "CurSeen - 崩溃报告".encode_utf16().collect();
    title.push(0);
    // SAFETY: 两个缓冲区都以 NUL 结尾且活到调用结束；属主窗口为空 → 无主对话框。
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::MessageBoxW(
            None,
            windows::core::PCWSTR(wide.as_ptr()),
            windows::core::PCWSTR(title.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::MB_OK
                | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR
                | windows::Win32::UI::WindowsAndMessaging::MB_TASKMODAL,
        );
    }
}
