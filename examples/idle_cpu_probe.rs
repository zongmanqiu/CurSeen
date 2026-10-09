//! 空载 CPU 归因探针：回答「park 态空载那 ~1% CPU 到底花在哪」。
//!
//! # 为什么要有它
//!
//! park 态不出帧（`runtime::frame` 的帧门），但进程里仍有两路常驻活动：
//!
//! 1. **250ms 对账**（`runtime::overlay.rs` 的 `PANEL_POLL`）：windui 的 `on_interval`
//!    落在主窗 `SetTimer` 上（`app/mod.rs:791`），即 park 态下 WM_TIMER 照样每秒醒
//!    4 次，每次跑 `sync_ui` + z 序守护 + GIF 条几何对账；
//! 2. **低级鼠标钩子线程**（`platform/input/hook.rs`）：系统每一次鼠标移动都会进
//!    本进程的回调（喂 `HOOK_TX`，由帧循环 drain）。
//!
//! 外部观察（任务管理器 ~1%）分不出这两路的占比。本探针做**差分实验**：
//!
//! | 阶段 | 注入 | 归因 |
//! |---|---|---|
//! | A 静默 | 无 | 250ms 对账 + 其它常驻线程的地板开销 |
//! | B 鼠标 125Hz | SendInput 相对移动 | A + 钩子链路的低频档 |
//! | C 鼠标 ~1kHz | SendInput 突发注入 | B 的 8 倍事件率——钩子开销是否线性放大 |
//!
//! C 与 A 的差 = 钩子链路；A 本身 = 定时器 + 杂项。另外用 `GetThreadTimes` 把
//! 进程 CPU 拆到**线程**：占用窗口消息循环的那个 TID 就是 UI/对账线程，一眼定位主犯。
//!
//! # 跑法
//!
//! ```text
//! cargo run --release --example idle_cpu_probe             # 自动：先 attach 已运行实例，找不到才拉起
//! cargo run --release --example idle_cpu_probe -- attach   # 只测已运行实例（绝不杀）
//! cargo run --release --example idle_cpu_probe -- spawn    # 自己拉起 temp/target/release/curseen.exe（跑完杀掉）
//! ```
//!
//! ⚠ 阶段 B/C 会用 SendInput 晃动真实光标 ±1px 共 ~40 秒；阶段 A 需要你别碰鼠标。
//!   测的是整进程（含全部线程），attach 用户真实实例的数据最有代表性（真实配置、真 park）。
//!
//! # 2026-10-01 实测（本机 12 逻辑核，attach 用户真实实例）
//!
//! | 阶段 | 第 1 轮 | 第 2 轮 |
//! |---|---|---|
//! | A 静默空载 | 0.43% / 0.26% | 0.03% / 0.09% |
//! | B 鼠标 125Hz | 0.19% | 0.20% |
//! | C 鼠标 1kHz | 0.22% | 0.10% |
//!
//! 结论（全机口径，单核值 ×12）：
//!
//! 1. **空载地板极低**：0.03–0.09%（= 单核 0.4–1.1%）。用户在任务管理器看到的
//!    「空载跳 1%」是瞬时尖峰（对账/托盘/输入处理的刷新瞬间），不是持续负载。
//! 2. **钩子链路不是热点**：125Hz→1kHz 事件率 ×8，进程 CPU 不升反平（0.1–0.2%）——
//!    每次回调的 enqueue 开销远小于分辨率噪声。
//! 3. **剩余地板** = park 态仍在跑的周期对账（30Hz `MOUSE_POLL` + 250ms `PANEL_POLL`
//!    的 WM_TIMER）+ 杂项线程。就 0.0x% 的量级，不值得为它改架构；真要抠，方向是
//!    park 态拉长 `MOUSE_POLL` 周期或改事件驱动。
//! 4. **线程级归因对本机不可用**：用户实例是提权运行，非提权探针 `OpenThread`
//!    恒 0x80070005（拒绝访问）——以管理员身份跑本探针即可解锁按线程拆账。
//!    另注：探针**不能**按窗口标题找实例（park 态窗口可能已销毁，会误判"没在跑"
//!    而去拉第二个实例，被单实例互斥弹开 → 测到空壳全程 0.00%，首轮即中招），
//!    必须按 exe 名走进程快照（`find_instance`）。

use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, Thread32First, Thread32Next,
    PROCESSENTRY32W, TH32CS_SNAPPROCESS, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
use windows::Win32::System::Threading::{
    CreateProcessW, GetProcessTimes, GetThreadTimes, OpenProcess, OpenThread,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, STARTUPINFOW, THREAD_QUERY_INFORMATION,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_MOUSE, MOUSEEVENTF_MOVE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};

/// FILETIME → 100ns 单位的 u64。
fn ft64(ft: &windows::Win32::Foundation::FILETIME) -> u64 {
    ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64
}

/// 进程 + 按线程的 CPU 时间快照（单位 100ns）。
struct CpuSnap {
    proc_cpu: u64,
    threads: Vec<(u32, u64)>, // (tid, kernel+user)
}

fn snapshot(pid: u32) -> CpuSnap {
    let dbg = std::env::var("IDLE_PROBE_DEBUG").is_ok();
    // 进程级
    let mut proc_cpu = 0u64;
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            let (mut c, mut e, mut k, mut u) = core::mem::zeroed();
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok() {
                proc_cpu = ft64(&k) + ft64(&u);
            }
            let _ = CloseHandle(h);
        }
    }
    // 线程级：ToolHelp 快照按 owner pid 过滤（TH32CS_SNAPTHREAD 必须传 0 拿全系统，
    // 自己过滤；传 pid 反而拿不到线程）。
    let mut threads = Vec::new();
    let mut n_matching = 0usize;
    unsafe {
        let snap = match CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
            Ok(s) => s,
            Err(e) => {
                if dbg {
                    eprintln!("[dbg] SNAPTHREAD 失败: {e}");
                }
                return CpuSnap { proc_cpu, threads };
            }
        };
        let mut te = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        if Thread32First(snap, &mut te).is_ok() {
            loop {
                if te.th32OwnerProcessID == pid {
                    n_matching += 1;
                    match OpenThread(THREAD_QUERY_INFORMATION, false, te.th32ThreadID) {
                        Ok(th) => {
                            let (mut c, mut e, mut k, mut u) = core::mem::zeroed();
                            if GetThreadTimes(th, &mut c, &mut e, &mut k, &mut u).is_ok() {
                                threads.push((te.th32ThreadID, ft64(&k) + ft64(&u)));
                            } else if dbg {
                                eprintln!("[dbg] tid={} GetThreadTimes 失败", te.th32ThreadID);
                            }
                            let _ = CloseHandle(th);
                        }
                        Err(e) => {
                            if dbg && n_matching <= 3 {
                                eprintln!(
                                    "[dbg] tid={} OpenThread 失败: {e}（目标提权而探针未提权时会如此）",
                                    te.th32ThreadID
                                );
                            }
                        }
                    }
                }
                if Thread32Next(snap, &mut te).is_err() {
                    break;
                }
            }
        } else if dbg {
            eprintln!("[dbg] Thread32First 失败");
        }
        let _ = CloseHandle(snap);
    }
    if dbg {
        eprintln!(
            "[dbg] pid={pid} 匹配线程 {n_matching} 个，取到时间 {} 个",
            threads.len()
        );
    }
    CpuSnap { proc_cpu, threads }
}

/// 逻辑核数（CPU% 分母：任务管理器的百分比 = 全部逻辑核）。
fn ncores() -> f64 {
    unsafe {
        let mut si = SYSTEM_INFO::default();
        GetSystemInfo(&mut si);
        si.dwNumberOfProcessors as f64
    }
}

/// 按 exe 名找已运行实例（`curseen.exe`）。
///
/// ★ 不能按窗口标题找：park 态覆盖层窗口可能已销毁、托盘窗无标题，标题法会漏判
///   → 探针误拉第二个实例 → 被单实例互斥弹开退出 → 测了个空壳（首轮实测即中招，
///   全程 0.00%）。进程快照按 exe 名找才是权威。
fn find_instance() -> Option<u32> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut pe = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = None;
        if Process32FirstW(snap, &mut pe).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &pe.szExeFile[..pe.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                );
                if name.eq_ignore_ascii_case("curseen.exe") {
                    found = Some(pe.th32ProcessID);
                    break;
                }
                if Process32NextW(snap, &mut pe).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// 进程是否还活着且未退出（GetExitCodeProcess STILL_ACTIVE = 259）。
fn alive(pid: u32) -> bool {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        use windows::Win32::System::Threading::GetExitCodeProcess;
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok() && code == 259;
        let _ = CloseHandle(h);
        ok
    }
}

/// 找出（attach 模式）该 pid 的全部 `CurSeen*` 窗口 TID——首次 find_instance 只拿到
/// 第一个命中的窗口，这里补齐，供线程归因标注。
fn window_tids(pid: u32) -> Vec<u32> {
    let mut tids: Vec<u32> = Vec::new();
    unsafe extern "system" fn cb(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<u32>) };
        let mut wpid = 0u32;
        let tid = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut wpid)) };
        if wpid != 0 {
            let mut buf = [0u16; 128];
            let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
            if n > 0 {
                let title = String::from_utf16_lossy(&buf[..n as usize]);
                if title.starts_with("CurSeen") {
                    out.push(tid);
                }
            }
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut tids as *mut _ as isize));
    }
    let _ = pid;
    tids
}

/// 拉起 `temp/target/release/curseen.exe`，返回 (pid, 是否成功)。
fn spawn_instance() -> Option<u32> {
    let exe = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../temp/target/release/curseen.exe"
    );
    if !std::path::Path::new(exe).exists() {
        eprintln!("找不到 {exe}——先 `cargo build --release` 或直接用 attach 模式");
        return None;
    }
    let mut cmd: Vec<u16> = exe.encode_utf16().chain(std::iter::once(0)).collect();
    let mut si = STARTUPINFOW::default();
    si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut pi = PROCESS_INFORMATION::default();
    let ok = unsafe {
        CreateProcessW(
            PWSTR::null(),
            Some(PWSTR(cmd.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &si,
            &mut pi,
        )
    };
    if ok.is_err() {
        eprintln!("CreateProcessW 失败: {:?}", ok.unwrap_err());
        return None;
    }
    unsafe {
        let _ = CloseHandle(pi.hThread);
    }
    Some(pi.dwProcessId)
}

/// 关掉自己拉起的实例：先 WM_CLOSE（托盘程序常拦关闭，等 3 秒），不行再强杀。
fn kill_instance(pid: u32) {
    unsafe extern "system" fn cb(hwnd: HWND, l: LPARAM) -> windows::core::BOOL {
        let pid = l.0 as u32;
        let mut wpid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut wpid)) };
        if wpid == pid {
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        windows::core::BOOL(1)
    }
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(pid as isize));
    }
    std::thread::sleep(Duration::from_secs(3));
    unsafe {
        if let Ok(h) = OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            false,
            pid,
        ) {
            let mut code = 0u32;
            // 还活着才杀（GetExitCodeProcess STILL_ACTIVE = 259）。
            use windows::Win32::System::Threading::GetExitCodeProcess;
            if GetExitCodeProcess(h, &mut code).is_ok() && code == 259 {
                let _ = windows::Win32::System::Threading::TerminateProcess(h, 1);
            }
            let _ = CloseHandle(h);
        }
    }
}

/// 一段采样窗口：`inject` 为 None = 静默；Some(hz, burst) = 按 hz 事件率注入鼠标移动。
///
/// 返回 (窗口秒数, 进程 CPU%, 线程增量)。
fn sample_window(
    pid: u32,
    secs: u64,
    inject: Option<f64>,
    win_tids: &[u32],
    ncore: f64,
) -> (f64, f64, Vec<(u32, f64)>) {
    let t0 = Instant::now();
    let s0 = snapshot(pid);
    let deadline = t0 + Duration::from_secs(secs);
    // 注入节奏：系统 Sleep 粒度 ~15.6ms，改用"每 16ms 一拍、每拍 burst 个"凑频率：
    // 125Hz → 每拍 2 个（2/16ms = 125Hz）；1000Hz → 每拍 16 个（16/16ms = 1kHz 名义值，
    // 实际因 Sleep 过冲略低 ~5%，不影响归因量级）。
    let per_tick = match inject {
        Some(hz) => (hz * 0.016).round().max(1.0) as usize,
        None => 0,
    };
    let mut flip = 0i32;
    while Instant::now() < deadline {
        for _ in 0..per_tick {
            flip = -flip;
            let mi = windows::Win32::UI::Input::KeyboardAndMouse::MOUSEINPUT {
                dx: flip,
                dy: 0,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE,
                time: 0,
                dwExtraInfo: 0,
            };
            let inp = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: windows::Win32::UI::Input::KeyboardAndMouse::INPUT_0 { mi },
            };
            unsafe {
                SendInput(&[inp], std::mem::size_of::<INPUT>() as i32);
            }
        }
        std::thread::sleep(Duration::from_millis(if per_tick > 0 { 16 } else { 100 }));
    }
    let wall = t0.elapsed().as_secs_f64();
    let s1 = snapshot(pid);
    let dcpu = s1.proc_cpu.saturating_sub(s0.proc_cpu) as f64 / 10_000_000.0; // 100ns → 秒
    let pct = dcpu / wall / ncore * 100.0;
    // 线程增量，按 CPU 降序
    let mut tdelta: Vec<(u32, f64)> = s1
        .threads
        .iter()
        .filter_map(|(tid, t1)| {
            let t0v = s0.threads.iter().find(|(t, _)| t == tid)?.1;
            let d = (t1.saturating_sub(t0v)) as f64 / 10_000_000.0;
            if d > 0.0005 {
                Some((*tid, d / wall / ncore * 100.0))
            } else {
                None
            }
        })
        .collect();
    tdelta.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let _ = win_tids;
    (wall, pct, tdelta)
}

fn report(label: &str, wall: f64, pct: f64, threads: &[(u32, f64)], win_tids: &[u32]) {
    println!(
        "  {label:<14} {wall:>5.1}s   进程 CPU = {pct:>5.2}% （单核算 {sc:>5.2}%）",
        sc = pct * ncores()
    );
    for (i, (tid, p)) in threads.iter().take(4).enumerate() {
        let tag = if win_tids.contains(tid) {
            " ← UI/对账线程(持有窗口)"
        } else {
            ""
        };
        println!("      线程#{i} tid={tid:<6} {p:>5.2}%{tag}");
    }
    if threads.is_empty() {
        println!("      ⚠ 无任何线程 CPU 增量——若每个阶段都如此，被测进程多半已退出（数据无效）");
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "auto".into());
    println!("空载 CPU 归因探针 · 模式 `{mode}`");
    println!("(口径：整进程 CPU%，分母 = 全部逻辑核；单核算 = 乘核数)\n");

    let ncore = ncores();
    println!("逻辑核数 {ncore}");

    // 自检模式：拿自己的线程验证枚举代码本身是否好使（2 秒即回）。
    if mode == "self" {
        let me = std::process::id();
        let s0 = snapshot(me);
        let t0 = Instant::now();
        let mut x = 0u64;
        while t0.elapsed() < Duration::from_secs(2) {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
        }
        std::hint::black_box(x);
        let s1 = snapshot(me);
        println!(
            "自检 pid={me}：线程 {} 个；2s 忙转期间取到增量的线程 {} 个",
            s1.threads.len(),
            s1.threads
                .iter()
                .filter(|(tid, t1)| {
                    t1 > &s0
                        .threads
                        .iter()
                        .find(|(t, _)| t == tid)
                        .map(|(_, v)| *v)
                        .unwrap_or(0)
                })
                .count()
        );
        return;
    }

    // ── 定位实例 ──
    let mut spawned = false;
    let pid = match find_instance() {
        Some(pid) if mode != "spawn" => {
            if !alive(pid) {
                eprintln!("pid={pid} 已退出。");
                return;
            }
            println!("attach 已运行实例 pid={pid}（不会动它）\n");
            pid
        }
        _ => {
            if mode == "attach" {
                eprintln!("没找到运行中的 curseen.exe（按进程快照找，不是窗口标题）。");
                return;
            }
            let pid = spawn_instance().unwrap_or_else(|| {
                eprintln!("拉起失败。");
                std::process::exit(1);
            });
            println!("已拉起临时实例 pid={pid}，测完自动关掉");
            spawned = true;
            std::thread::sleep(Duration::from_secs(6)); // 等初始化 + 进 park
                                                        // ★ 存活自检：单实例互斥会让第二个实例秒退（首轮实测即中招，测了个空壳）。
            if !alive(pid) {
                eprintln!(
                    "拉起的实例已退出——大概率已有实例在跑（单实例互斥弹开），改用 attach 模式。"
                );
                return;
            }
            println!("存活自检通过\n");
            pid
        }
    };
    let win_tids = window_tids(pid);

    // ── 阶段 A：静默空载 ──
    println!("── 阶段 A：静默空载（别碰鼠标，20s）──");
    for i in 1..=2 {
        let (wall, pct, th) = sample_window(pid, 10, None, &win_tids, ncore);
        report(&format!("A{i}"), wall, pct, &th, &win_tids);
    }

    // ── 阶段 B：鼠标 125Hz ──
    println!("── 阶段 B：注入鼠标移动 ~125Hz（光标会 ±1px 晃动，20s）──");
    let (wall, pct, th) = sample_window(pid, 20, Some(125.0), &win_tids, ncore);
    report("B 125Hz", wall, pct, &th, &win_tids);

    // ── 阶段 C：鼠标 ~1kHz ──
    println!("── 阶段 C：注入鼠标移动 ~1kHz（同上，20s）──");
    let (wall, pct, th) = sample_window(pid, 20, Some(1000.0), &win_tids, ncore);
    report("C 1kHz", wall, pct, &th, &win_tids);

    println!("\n── 判读 ──");
    println!("· A 的进程 CPU% = 250ms 对账 + 其它常驻的地板（对账归因看 UI/对账线程那一行）");
    println!("· B−A、C−B = 钩子链路的增量；若随事件率线性放大，说明钩子回调是热点");
    println!("· 若 A ≈ 0 而 B/C 才上去：空载 1% 只在你手放鼠标上时出现，属正常输入税");

    if spawned {
        kill_instance(pid);
        println!("\n（临时实例已关闭）");
    }
}
