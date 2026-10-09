//! 系统常驻内存归因探针：把"录一次 + 裁一次 + 保存之后内存为什么回不去"拆成逐动作的数字。
//!
//! # 为什么要有它
//!
//! 本项目的**自有**内存都能按照 accounts 一段段说清（`examples/mem_probe.rs` 管那一摊），
//! 但有一类账对不上：录过一次之后任务管理器停在 40MB 久久不下，而回收列表里明明
//! 什么都不剩了。剩下的只能落在**系统组件**的头上——那些 DLL 一旦被加载就不会卸载，
//! 它们的**私有**数据页照样算在你的头上：
//!
//! | 动作 | 拉起的模块 |
//! |---|---|
//! | `GetSaveFileNameW` / `GetOpenFileNameW` | `comdlg32` → `ExplorerFrame` / `shell32` / `propsys` + 本机全部 shell 扩展 |
//! | `IFileOpenDialog` | 同上（同一套 COM 接口，Vista+ 分支） |
//! | `MFStartup` | `mfplat` / `mf` / `mfcore` + H.264 编码器 MFT |
//! | `D3D11CreateDevice` | `d3d11` / `dxgi` + **厂商 UMD**（NVIDIA `nvwgf2umx` 等，通常最肥） |
//!
//! 静态推理只能走到"它们会不会卸载"，走不到"各自多少 MB"——那取决于本机显卡驱动与装了
//! 什么 shell 扩展。本探针把这两件事都量出来。
//!
//! # 三个关键口径
//!
//! - **动作后 vs `EmptyWorkingSet` 之后**：前者就是用户在任务管理器看到的数字；二者的差
//!   = 这一刀能砍掉的（我们的确已释放、只是还挂在工作集里的页）；**后者与基线的差 =
//!   永久抬升**，这才是真正要归因的对象。
//! - **pws（私有工作集）vs commit（私有提交）**：commit 反映堆的真实占用。commit 降了而
//!   pws 没降 = 只差一次 `EmptyWorkingSet`；commit 不降 = 真的没释放。
//! - **模块差集**：`EnumProcessModules` 前后取差，直接点名这次动作带进来了谁。
//!
//! # 跑法
//!
//! ```text
//! cargo run --release --example mem_attrib -- base       # 什么都不做（基线）
//! cargo run --release --example mem_attrib -- com        # COM 单元 opp initialize / 反初始化
//! cargo run --release --example mem_attrib -- dlg        # 原生「另存为」对话框（自动取消）
//! cargo run --release --example mem_attrib -- folder     # IFileOpenDialog 选目录（自动取消）
//! cargo run --release --example mem_attrib -- mf         # MFStartup / MFShutdown
//! cargo run --release --example mem_attrib -- d3d        # D3D11 设备一开一关
//! cargo run --release --example mem_attrib -- d3d-wait   # 同上，之后每 10 秒采样一次看自然回落
//! cargo run --release --example mem_attrib -- all        # 复刻「录制 + 裁剪保存」整条链
//! ```
//!
//! 对话框档**无需人工干预**：探针起一条线程，3 秒后找到自己那个 `#32770` 并 `WM_CLOSE`
//! （等价于用户点取消）。
//!
//! ⚠ 对话框会在屏幕上闪一下；跑之前别开着重要窗口。

use std::collections::HashSet;
use std::time::Duration;

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{HMODULE, HWND, LPARAM, WPARAM};
use windows::Win32::System::ProcessStatus::{
    EnumProcessModules, GetModuleFileNameExW, GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    PROCESS_MEMORY_COUNTERS_EX2,
};
use windows::Win32::System::Threading::GetCurrentProcess;

const MB: f64 = 1024.0 * 1024.0;

/// 两个口径：`pws` = 私有工作集（任务管理器那一列），`commit` = 私有提交（堆的真实占用）。
#[derive(Clone, Copy)]
struct Snap {
    pws: usize,
    commit: usize,
}

fn snap() -> Snap {
    unsafe {
        let mut m = PROCESS_MEMORY_COUNTERS_EX2 {
            // ★ `cb` 就是版本号：填 EX2 的大小，系统才会填 `PrivateWorkingSetSize`。
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
            ..Default::default()
        };
        let p = (&mut m as *mut PROCESS_MEMORY_COUNTERS_EX2).cast::<PROCESS_MEMORY_COUNTERS>();
        if GetProcessMemoryInfo(GetCurrentProcess(), p, m.cb).is_err() {
            return Snap { pws: 0, commit: 0 };
        }
        Snap {
            pws: m.PrivateWorkingSetSize,
            commit: m.PrivateUsage,
        }
    }
}

/// 已加载模块的**基名**集合（忽略路径，便于做前后差集）。
fn modules() -> HashSet<String> {
    let mut out = HashSet::new();
    unsafe {
        let me = GetCurrentProcess();
        let mut mods = [HMODULE::default(); 1024];
        let mut need = 0u32;
        if EnumProcessModules(
            me,
            mods.as_mut_ptr(),
            std::mem::size_of_val(&mods) as u32,
            &mut need,
        )
        .is_err()
        {
            return out;
        }
        let n = (need as usize / std::mem::size_of::<HMODULE>()).min(mods.len());
        for m in &mods[..n] {
            let mut buf = [0u16; 260];
            let len = GetModuleFileNameExW(Some(me), Some(*m), &mut buf) as usize;
            if len == 0 {
                continue;
            }
            let full = String::from_utf16_lossy(&buf[..len]);
            let base = full.rsplit(['\\', '/']).next().unwrap_or(&full);
            out.insert(base.to_ascii_lowercase());
        }
    }
    out
}

fn trim() {
    unsafe {
        let r = windows::Win32::System::ProcessStatus::EmptyWorkingSet(GetCurrentProcess());
        if r.is_err() {
            println!("  !! EmptyWorkingSet 失败: {}", r.unwrap_err());
        }
    }
}

/// 三段式归因：动作前 → 动作后（用户看到的数）→ 裁剪后（永久抬升）。
///
/// `trim_after` = false 用于 d3d-wait 这类要自己控制节奏的场景。
fn stage(label: &str, base: Snap, base_mods: &HashSet<String>, trim_after: bool) -> Snap {
    let after = snap();
    let residual = if trim_after {
        trim();
        snap()
    } else {
        after
    };
    println!(
        "  {label:<10} 未裁 {pws:>7.2} MB / {cm:>7.2} MB   裁后 {rpws:>7.2} MB / {rcm:>7.2} MB   \
         ↑永久抬高 {up:>6.2} MB",
        label = label,
        pws = after.pws as f64 / MB,
        cm = after.commit as f64 / MB,
        rpws = residual.pws as f64 / MB,
        rcm = residual.commit as f64 / MB,
        up = residual.pws.saturating_sub(base.pws) as f64 / MB,
    );
    let new = modules();
    let mut added: Vec<String> = new.difference(base_mods).cloned().collect();
    added.sort();
    if !added.is_empty() {
        println!("     新增模块({}): {}", added.len(), added.join(", "));
    }
    residual
}

// ────────────────────────────── 各动作 ──────────────────────────────

/// COM 单元：只为测"初始化/反初始化这一对"本身的代价。
fn do_com(apartment: bool) {
    use windows::Win32::System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED,
    };
    let which = if apartment {
        COINIT_APARTMENTTHREADED
    } else {
        COINIT_MULTITHREADED
    };
    unsafe {
        if CoInitializeEx(None, which).is_ok() {
            CoUninitialize();
        }
    }
}

/// 3 秒后把自己弹出的通用对话框关掉（等价于用户点「取消」）。
///
/// `EnumWindows` 回调里按**进程**过滤：通用对话框的类名 `#32770` 是全系统共用的
/// （任何程序的模态窗都是它），不按 PID 筛会把别人的窗户一起关了。
fn arm_dialog_auto_close() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        unsafe extern "system" fn one(hwnd: HWND, _l: LPARAM) -> BOOL {
            use windows::Win32::System::Threading::GetCurrentProcessId;
            use windows::Win32::UI::WindowsAndMessaging::{
                GetClassNameW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
            };
            unsafe {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let mut cls = [0u16; 64];
                let n = GetClassNameW(hwnd, &mut cls);
                if pid == GetCurrentProcessId()
                    && String::from_utf16_lossy(&cls[..n as usize]) == "#32770"
                {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                    return BOOL(0); // 找到即可，停
                }
            }
            BOOL(1)
        }
        let _ =
            unsafe { windows::Win32::UI::WindowsAndMessaging::EnumWindows(Some(one), LPARAM(0)) };
    });
}

/// 原生「另存为」对话框（走 `GetSaveFileNameW`，与 MP4 裁剪「保存」同一条路）。
fn do_save_dialog() {
    use windows::core::PWSTR;
    use windows::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT,
        OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let mut buf = vec![0u16; 4096];
    let name: Vec<u16> = "mem_attrib_probe.mp4".encode_utf16().collect();
    buf[..name.len()].copy_from_slice(&name);
    let filter: Vec<u16> = "MP4 视频\0*.mp4\0".encode_utf16().collect();
    let ext: Vec<u16> = "mp4".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR::from_raw(filter.as_ptr()),
        lpstrFile: PWSTR::from_raw(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrDefExt: PCWSTR::from_raw(ext.as_ptr()),
        Flags: OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    arm_dialog_auto_close();
    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    if !ok.as_bool() {
        let code = unsafe { CommDlgExtendedError() };
        if code.0 != 0 {
            println!("     (对话框未建成 CommDlgExtendedError=0x{:04X})", code.0);
        } else {
            println!("     (已自动取消)");
        }
    }
}

/// Vista+ 的「选目录」对话框（`IFileOpenDialog` + `FOS_PICKFOLDERS`）。
fn do_folder_dialog() {
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, FOS_PICKFOLDERS};
    safe_com(|| unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
        dlg.SetOptions(FOS_PICKFOLDERS | dlg.GetOptions().ok()?)
            .ok()?;
        arm_dialog_auto_close();
        // 取消是预期结果：这里只关心 shell 全家桶有没有被拉起来。
        let _ = dlg.Show(None);
        Some(())
    });
}

/// 带 COM 配对的小 helper：自己初始化成功才反初始化（跟进 file_dialog.rs 的 `ComGuard`）。
fn safe_com<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let out = f();
    if ok {
        unsafe { CoUninitialize() };
    }
    out
}

/// 媒体基金会平台：`MFStartup` / `MFShutdown`（录制与 MP4 裁剪都从这里起步）。
fn do_mf() {
    use windows::Win32::Media::MediaFoundation::{MFShutdown, MFStartup, MFSTARTUP_LITE};
    unsafe {
        if MFStartup(
            windows::Win32::Media::MediaFoundation::MF_SDK_VERSION,
            MFSTARTUP_LITE,
        )
        .is_ok()
        {
            let _ = MFShutdown();
        }
    }
}

/// D3D11 设备 + DXGI 资源：与 `Session::open` 同一份参数（硬件适配器 + BGRA）。
fn do_d3d() {
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
    };
    unsafe {
        let mut dev: Option<ID3D11Device> = None;
        let levels = [
            D3D_FEATURE_LEVEL_11_1,
            D3D_FEATURE_LEVEL_11_0,
            D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_10_0,
        ];
        let r = D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut dev),
            None,
            None,
        );
        if let Err(e) = r {
            println!("     (D3D11CreateDevice 失败: {e}——本机无硬件 D3D11，本档数据无效)");
            return;
        }
        drop(dev); // 与 Session::drop 同款：设备就此释放，DLL 会不会走是另一件事
    }
}

fn main() {
    let scene = std::env::args().nth(1).unwrap_or_else(|| "all".into());
    println!("内存归因探针 · 场景 `{scene}`");
    println!("(口径：私有工作集 MB / 私有提交 MB；`↑永久抬高` = 裁剪后仍高于基线的部分)\n");

    // 先什么都别做，等 ABI 稳头 à partir
    trim();
    std::thread::sleep(Duration::from_millis(300));
    let base = snap();
    let base_mods = modules();
    println!(
        "基线        {:.2} MB / {:>7.2} MB   模块数 {}",
        base.pws as f64 / MB,
        base.commit as f64 / MB,
        base_mods.len()
    );

    match scene.as_str() {
        "base" => {
            println!("（什么都不做：这就是干净进程的代价）");
        }
        "com" => {
            do_com(false);
            stage("COM MTA", base, &base_mods, true);
        }
        "dlg" => {
            do_save_dialog();
            stage("另存为对话框", base, &base_mods, true);
        }
        "folder" => {
            do_folder_dialog();
            stage("选目录对话框", base, &base_mods, true);
        }
        "mf" => {
            do_mf();
            stage("MF", base, &base_mods, true);
        }
        "d3d" => {
            do_d3d();
            stage("D3D11", base, &base_mods, true);
        }
        "d3d-wait" => {
            // ★ 验证"过了约 1 分钟才回落"是不是 UMD 空闲卸载：每一轮 sample 都**不裁**，
            //   只看操作系统自己会不会把页/库收回去。
            do_d3d();
            let mut cur = stage("D3D11 刚释放", base, &base_mods, false);
            for i in 1..=8 {
                std::thread::sleep(Duration::from_secs(10));
                let s = snap();
                println!(
                    "  T+{:<3}s     未裁 {pws:>7.2} MB / {cm:>7.2} MB   （较上轮 {d:+.2} MB）",
                    i * 10,
                    pws = s.pws as f64 / MB,
                    cm = s.commit as f64 / MB,
                    d = s.pws as f64 / MB - cur.pws as f64 / MB,
                );
                cur = s;
            }
            let mut after_trim = modules();
            after_trim.retain(|m| !base_mods.contains(m));
            trim();
            let t = snap();
            println!(
                "  最终裁剪后  {:.2} MB / {:>7.2} MB   ↑永久抬高 {:.2} MB",
                t.pws as f64 / MB,
                t.commit as f64 / MB,
                t.pws.saturating_sub(base.pws) as f64 / MB
            );
        }
        "all" => {
            // 复刻用户那条链：录制（COM + MF + D3D11）→ 停录 → 裁剪窗（MF + 预览解码）
            // → 保存对话框。累计到最后一并裁 —— 这才是"任务管理器看到的 40MB"。
            do_com(false);
            stage("① COM", base, &base_mods, false);
            do_mf();
            stage("② +MF", base, &base_mods, false);
            do_d3d();
            stage("③ +D3D11", base, &base_mods, false);
            do_mf(); // 裁剪窗自己的 MfGuard（又一次 MFStartup）
            stage("④ 裁剪:MF", base, &base_mods, false);
            do_save_dialog();
            stage("⑤ 保存对话框", base, &base_mods, false);
            println!("\n  以上全部动作都做完以后，一次性整理工作集：");
            trim();
            let t = snap();
            println!(
                "  全链裁剪后  {:.2} MB / {:>7.2} MB   ↑永久抬高 {:.2} MB",
                t.pws as f64 / MB,
                t.commit as f64 / MB,
                t.pws.saturating_sub(base.pws) as f64 / MB
            );
            let new = modules();
            let mut added: Vec<String> = new.difference(&base_mods).cloned().collect();
            added.sort();
            println!("  全链新增模块({}): {}", added.len(), added.join(", "));
        }
        other => {
            println!(
                "未知场景 `{other}`。可用：base / com / dlg / folder / mf / d3d / d3d-wait / all"
            );
        }
    }
}
