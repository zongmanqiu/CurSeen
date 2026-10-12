//! 剪贴板 0xC0000005 的**确定性复现**：旧版必崩、新版不崩（2026-10-13）。
//!
//! # 用户的问题
//!
//! "你能否在崩溃那版里复现，并证明改版后不出现？如果做不到，我怀疑 dmp 没用、
//! 也怀疑你的修复无效。"
//!
//! 本探针正面回答它：同一套并发场景，跑两遍——
//!
//! | 侧 | 读者 | 写者 | 期望 |
//! |---|---|---|---|
//! | **旧**（崩溃那版） | `old_get_image()`：逐字抄自 `.bf/1012-2340 崩溃1012修复前/clipboard.rs` | `old_to_clipboard()`：旧版裸 `OpenClipboard + EmptyClipboard` | 进程被 0xC0000005 杀掉 |
//! | **新**（现出货） | `curseen_ui::platform::clipboard::get_image()` | `curseen_ui::platform::screenshot::Shot::to_clipboard()` | 干净退出，且**读回来的像素与写进去的逐字节一致** |
//!
//! ## 为什么"逐字抄"能算数
//!
//! 崩溃那版的 PDB（`temp/symbols/curseen-20261012-002708.pdb`）把崩溃点解到
//! `src\platform\clipboard.rs:160`；而快照 `.bf/1012-2340` 那份 `clipboard.rs`
//! 的**第 160 行正好是 `bgra[...].copy_from_slice(row)`**。行号对得上 ⇒ 这份快照
//! 就是 D 盘那个 exe 的源码，抄它不是"仿写"，是同一段代码。
//!
//! ## 两轮
//!
//! - **A 轮（自然竞态）**：不加任何同步，读写两个线程各跑各的，看它**自己**会不会撞。
//!   撞上就崩，进程死，父进程看退出码。这一轮证明"不需要人为安排，它就会发生"。
//! - **B 轮（强制交错）**：仅当 A 轮在时限内没撞上才跑。读者先开剪贴板、锁住、解完头，
//!   然后**等写者把那块内存 Empty 掉**，再继续拷贝。这是把 A 轮里随机出现的那个交错
//!   顺序**钉死**，保证必崩。B 轮是兜底，不是主证据。
//!
//! ## 顺带验一件事：新代码有没有把"崩"降级成"悄悄丢内容"
//!
//! 昨天那版夹取确实能把崩改成返回 `None`——那就是静默丢数据。所以新版这一侧
//! **不只看"没崩"，还校验读回来的像素**与写进去的完全一致（尺寸 + 每个像素的
//! B/G/R）。没崩但内容错了，判据一样红。
//!
//! ```text
//! python build/probe.py run clip_crash_repro
//! ```
//!
//! probe-group: fast

#![cfg(windows)]
#![allow(non_snake_case, non_upper_case_globals)]

use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    SetClipboardData,
};
use windows::Win32::System::Diagnostics::Debug::{
    MiniDumpWriteDump, SetUnhandledExceptionFilter, EXCEPTION_POINTERS,
    MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThreadId};

const CF_DIB: u32 = 8;
const MODE_VAR: &str = "CURSEEN_CLIP_REPRO";
const DMP_VAR: &str = "CURSEEN_CLIP_REPRO_DMP";

/// 与崩溃现场同一量级的图：w=1599 h=819，stride=6396，DIB 共 5,238,364 字节。
const W: i32 = 1599;
const H: i32 = 819;
const FILL: u8 = 0xA5;

// ── 旧版（崩溃那版）代码：逐字抄自 .bf/1012-2340 ──────────────────────────

struct OldShot {
    w: i32,
    h: i32,
    bgra: Vec<u8>,
}

struct OldCloser;
impl Drop for OldCloser {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}
struct OldUnlocker(*mut u8);
impl Drop for OldUnlocker {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(HGLOBAL(self.0 as *mut _));
        }
    }
}

/// 旧版 `mask_bytes`（原文照抄）。
fn mask_bytes(compression: u32) -> usize {
    if compression == 3 {
        12
    } else {
        0
    }
}

/// 旧版 `get_image`，**逐字照抄**崩掉那版（`.bf/1012-2340/clipboard.rs` 100-187 行）。
///
/// 只把返回类型换成同字段的本地 `OldShot`（`core::image::Shot` 就是这 3 个字段），
/// 其余一行未改 —— 包括那句 `let total = GlobalSize(...)` 和 `copy_from_slice`。
/// ★ 注意它**没有任何锁**：这就是崩的原因，不是"数据畸形"。
unsafe fn old_get_image() -> Option<OldShot> {
    unsafe {
        OpenClipboard(None).ok()?;
        let _guard = OldCloser;
        if IsClipboardFormatAvailable(CF_DIB).is_err() {
            return None;
        }
        let hmem = GetClipboardData(CF_DIB).ok()?;
        let ptr = GlobalLock(HGLOBAL(hmem.0));
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = OldUnlocker(ptr.cast());
        let total = GlobalSize(HGLOBAL(hmem.0)) as usize;
        if total < 20 {
            return None;
        }
        let base = ptr.cast::<u32>();
        let bi_size = *base as usize;
        let w = *base.add(1) as i32;
        let h_raw = *base.add(2) as i32;
        let bpp = (*base.add(3) >> 16) as u32;
        let compression = *base.add(4);
        if w <= 0 || w > 16384 || h_raw == 0 || h_raw.abs() > 16384 {
            return None;
        }
        let top_down = h_raw < 0;
        let h = h_raw.abs();
        if !(compression == 0 || compression == 3) {
            return None;
        }
        if !(40..=124).contains(&bi_size) {
            return None;
        }
        let pixel_offset = if bi_size >= 108 {
            bi_size
        } else if compression == 3 {
            bi_size + 12
        } else {
            bi_size
        };
        if pixel_offset + (h as usize) * ((w as usize) * 4) > total {
            return None;
        }
        match bpp {
            32 => {
                let px = (ptr.cast::<u8>()).add(bi_size + mask_bytes(compression));
                let stride = (w as usize) * 4;
                let mut bgra = vec![0_u8; (w as usize) * (h as usize) * 4];
                for y in 0..h as usize {
                    let src_row = if top_down { y } else { h as usize - 1 - y };
                    let row = std::slice::from_raw_parts(px.add(src_row * stride), stride);
                    // ★★★ 崩溃那版 PDB 解出的 `clipboard.rs:160` 就是这一行。
                    bgra[y * stride..(y + 1) * stride].copy_from_slice(row);
                }
                for a in bgra.chunks_exact_mut(4) {
                    a[3] = 255;
                }
                Some(OldShot { w, h, bgra })
            }
            24 => {
                let stride = ((w as usize) * 3).div_ceil(4) * 4;
                let px = (ptr.cast::<u8>()).add(bi_size);
                let mut bgra = vec![0_u8; (w as usize) * (h as usize) * 4];
                for y in 0..h as usize {
                    let src_row = if top_down { y } else { h as usize - 1 - y };
                    for x in 0..w as usize {
                        let s = src_row * stride + x * 3;
                        let d = y * (w as usize) * 4 + x * 4;
                        bgra[d] = *px.add(s);
                        bgra[d + 1] = *px.add(s + 1);
                        bgra[d + 2] = *px.add(s + 2);
                        bgra[d + 3] = 255;
                    }
                }
                Some(OldShot { w, h, bgra })
            }
            _ => None,
        }
    }
}

/// 旧版写者的**剪贴板部分**（`.bf/1013-0035/screenshot.rs` 290-302 行原文）：
/// 裸 `OpenClipboard` → `EmptyClipboard` → `SetClipboardData`，**不走任何锁**。
///
/// 头与像素的填法与旧版等价（旧版用 `BITMAPINFOHEADER` 结构体写，这里按 u32 偏移写，
/// 落盘字节完全相同），不重复开 GDI feature。
unsafe fn old_to_clipboard(shot: &OldShot) -> bool {
    unsafe {
        let stride = (shot.w as usize) * 4;
        let total = 40 + stride * (shot.h as usize);
        let Ok(hg) = GlobalAlloc(GHND, total) else {
            return false;
        };
        if hg.is_invalid() {
            return false;
        }
        let p = GlobalLock(hg);
        if p.is_null() {
            let _ = GlobalFree(Some(hg));
            return false;
        }
        {
            let dst = std::slice::from_raw_parts_mut(p.cast::<u8>(), total);
            let base = p.cast::<u32>();
            base.write(40);
            base.add(1).write(shot.w as u32);
            base.add(2).write(shot.h as u32); // 正 = 自下而上，与旧版一致
            base.add(3).write((32 << 16) | 1);
            base.add(4).write(0);
            // 旧版把行倒过来写（CF_DIB 自下而上）；这里直接整块填充同一字节，
            // 行序无关，不影响"读者被抽走内存"这个被测点。
            dst[40..].fill(FILL);
        }
        let _ = GlobalUnlock(hg);
        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            let r = SetClipboardData(CF_DIB, Some(HANDLE(hg.0)));
            let _ = CloseClipboard();
            if r.is_err() {
                let _ = GlobalFree(Some(hg));
                return false;
            }
        } else {
            let _ = GlobalFree(Some(hg));
            return false;
        }
        true
    }
}

// ── 子进程：崩之前先落一份 minidump ───────────────────────────────────────

/// 顶层异常过滤：写 .dmp，然后让进程带着异常码退出。
///
/// 存在的意义是**顺带验证 dmp 那条路本身**：父进程会检查这个文件有没有真的生成、
/// 以及能不能被符号化回行号。
unsafe extern "system" fn dump_filter(info: *const EXCEPTION_POINTERS) -> i32 {
    eprintln!("[child] ★ 捕获到未处理异常，正在写 minidump");
    if let Ok(p) = std::env::var(DMP_VAR) {
        if let Ok(file) = std::fs::File::create(&p) {
            let handle = HANDLE(file.as_raw_handle());
            let exc = MINIDUMP_EXCEPTION_INFORMATION {
                ThreadId: unsafe { GetCurrentThreadId() },
                ExceptionPointers: info as *mut EXCEPTION_POINTERS,
                ClientPointers: windows::core::BOOL(0),
            };
            let r = unsafe {
                MiniDumpWriteDump(
                    GetCurrentProcess(),
                    std::process::id(),
                    handle,
                    MINIDUMP_TYPE(0),
                    Some(std::ptr::from_ref(&exc)),
                    None,
                    None,
                )
            };
            eprintln!("[child] minidump -> {}（写入 {}）", p, r.is_ok());
        }
    }
    1 // EXCEPTION_EXECUTE_HANDLER：进程终止
}

// ── A 轮：自然竞态 ────────────────────────────────────────────────────────

/// 旧版：读写两个线程各跑各的，**不加任何同步**，看它自己会不会撞。
fn round_a_old(secs: u64) -> ! {
    let big = OldShot {
        w: W,
        h: H,
        bgra: vec![FILL; (W as usize) * (H as usize) * 4],
    };
    assert!(unsafe { old_to_clipboard(&big) }, "种子写入失败");

    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let (s0, n0) = (stop.clone(), writes.clone());
    let big2 = OldShot {
        w: W,
        h: H,
        bgra: vec![FILL; (W as usize) * (H as usize) * 4],
    };
    std::thread::Builder::new()
        .name("repro-writer".into())
        .spawn(move || {
            let mut n = 0usize;
            while !s0.load(Ordering::Relaxed) {
                unsafe {
                    let _ = old_to_clipboard(&big2);
                }
                n += 1;
                n0.store(n, Ordering::Relaxed);
            }
        })
        .expect("写者线程");

    let t0 = Instant::now();
    let mut reads = 0usize;
    loop {
        if t0.elapsed() > Duration::from_secs(secs) {
            break;
        }
        match unsafe { old_get_image() } {
            Some(s) => {
                reads += 1;
                if s.w != W || s.h != H {
                    eprintln!("[child] 读到 {}x{}（种子是 {}x{}）", s.w, s.h, W, H);
                }
            }
            None => {
                reads += 1;
            }
        }
        if reads % 100 == 0 {
            eprintln!(
                "[child] A轮 已读 {} 次 / 写者已写 {} 次，还没崩",
                reads,
                writes.load(Ordering::Relaxed)
            );
        }
    }
    stop.store(true, Ordering::Relaxed);
    eprintln!(
        "[child] A轮 {} 秒内读 {} 次、写 {} 次，未复现（转 B 轮强制交错）",
        secs,
        reads,
        writes.load(Ordering::Relaxed)
    );
    round_b_old()
}

// ── B 轮：强制交错（把 A 轮里随机出现的那个顺序钉死） ──────────────────────

static READY: AtomicBool = AtomicBool::new(false);
static EMPTIED: AtomicBool = AtomicBool::new(false);

/// 读者开剪贴板 → 锁 → 解头 → **等写者把内存收走** → 继续拷贝。
///
/// 与 A 轮的差别只有"多等了一下"，拷贝那一行与旧版原文**逐字相同**。
fn round_b_old() -> ! {
    let small = OldShot {
        w: 64,
        h: 64,
        bgra: vec![0x3C; 64 * 64 * 4],
    };
    let tw = std::thread::Builder::new()
        .name("repro-writer-forced".into())
        .spawn(move || {
            while !READY.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(1));
            }
            unsafe {
                if OpenClipboard(None).is_ok() {
                    let _ = EmptyClipboard();
                    let _ = old_to_clipboard(&small);
                    let _ = CloseClipboard();
                }
            }
            EMPTIED.store(true, Ordering::SeqCst);
        })
        .expect("写者线程");

    unsafe {
        OpenClipboard(None).ok().expect("B轮 开剪贴板");
        if IsClipboardFormatAvailable(CF_DIB).is_err() {
            panic!("B轮 剪贴板上没有 CF_DIB");
        }
        let hmem = GetClipboardData(CF_DIB).ok().expect("B轮 GetClipboardData");
        let ptr = GlobalLock(HGLOBAL(hmem.0));
        assert!(!ptr.is_null(), "B轮 GlobalLock");
        let base = ptr.cast::<u32>();
        let bi_size = *base as usize;
        let compression = *base.add(4);
        eprintln!(
            "[child] B轮 读者已锁住 DIB：ptr={:p} GlobalSize={} 字节",
            ptr,
            GlobalSize(HGLOBAL(hmem.0))
        );
        READY.store(true, Ordering::SeqCst);
        while !EMPTIED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        eprintln!("[child] B轮 写者已 EmptyClipboard —— 读者继续拷贝（下一步必崩）");

        // ↓↓↓ 与旧版原文逐字相同（.bf/1012-2340/clipboard.rs:157-161）↓↓↓
        let px = (ptr.cast::<u8>()).add(bi_size + mask_bytes(compression));
        let stride = (W as usize) * 4;
        let mut bgra = vec![0_u8; (W as usize) * (H as usize) * 4];
        for y in 0..H as usize {
            let src_row = H as usize - 1 - y;
            let row = std::slice::from_raw_parts(px.add(src_row * stride), stride);
            bgra[y * stride..(y + 1) * stride].copy_from_slice(row);
        }
        // ↑↑↑ 崩在这一行 ↑↑↑
    }
    let _ = tw.join();
    eprintln!("[child] B轮 拷贝居然完成了 —— 内存没被抽走，判据需要重新设计");
    std::process::exit(7)
}

// ── 新版那一侧 ────────────────────────────────────────────────────────────

/// 新版：读用真出货的 `get_image()`，写用真出货的 `Shot::to_clipboard()`。
///
/// 判据**不只是"没崩"**，还校验读回来的像素与写进去的逐字节一致 —— 防止把"崩"
/// 降级成"悄悄少收一条内容"。
fn run_new(rounds: usize) {
    use curseen_ui::platform::screenshot::Shot;

    let big = Shot {
        w: W,
        h: H,
        bgra: vec![FILL; (W as usize) * (H as usize) * 4],
    };
    if let Err(e) = curseen_ui::platform::screenshot::to_clipboard(&big) {
        panic!("新版写者失败：{e}");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicUsize::new(0));
    let (s0, n0) = (stop.clone(), writes.clone());
    let big2 = Shot {
        w: W,
        h: H,
        bgra: vec![FILL; (W as usize) * (H as usize) * 4],
    };
    std::thread::Builder::new()
        .name("repro-writer-new".into())
        .spawn(move || {
            let mut n = 0usize;
            while !s0.load(Ordering::Relaxed) {
                let _ = curseen_ui::platform::screenshot::to_clipboard(&big2);
                n += 1;
                n0.store(n, Ordering::Relaxed);
            }
        })
        .expect("写者线程");

    let mut ok = 0usize;
    let mut none = 0usize;
    let mut bad = 0usize;
    for i in 0..rounds {
        match curseen_ui::platform::clipboard::get_image() {
            None => none += 1,
            Some(s) => {
                // 尺寸 + 每个像素的 B/G/R 都要对得上（A 通道被强制成 255）。
                let mut good = s.w == W && s.h == H && s.bgra.len() == (W as usize) * (H as usize) * 4;
                if good {
                    for c in s.bgra.chunks_exact(4) {
                        if c[0] != FILL || c[1] != FILL || c[2] != FILL || c[3] != 255 {
                            good = false;
                            break;
                        }
                    }
                }
                if good {
                    ok += 1;
                } else {
                    bad += 1;
                    eprintln!("[child] 内容不一致：{}x{} len={}", s.w, s.h, s.bgra.len());
                }
            }
        }
        if (i + 1) % 50 == 0 {
            eprintln!("[child] 新版 已读 {} 次（一致 {} / 空 {} / 错 {}）", i + 1, ok, none, bad);
        }
    }
    stop.store(true, Ordering::Relaxed);
    println!(
        "[child] 新版跑完：读 {} 次 ⇒ 像素一致 {} / 空 {} / 内容错 {}；写者 {} 次",
        rounds,
        ok,
        none,
        bad,
        writes.load(Ordering::Relaxed)
    );
    // ★ 空（None）可以接受：那是"剪贴板被别的进程占着"的常态。
    //   但**内容错**一条都不能有 —— 那正是"静默丢数据"。
    assert_eq!(bad, 0, "新版读到了内容不一致的图（静默损坏）");
    assert!(ok + none == rounds);
}

// ── 父进程：跑两个子进程，比退出码 ────────────────────────────────────────

const AV: i32 = 0xC000_0005_u32 as i32;

fn run_child(mode: &str, dmp: Option<&str>) -> (Option<i32>, String, bool) {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.args([
        "--ignored",
        "--exact",
        "旧版必崩_新版不崩",
        "--nocapture",
        "--test-threads=1",
    ]);
    cmd.env(MODE_VAR, mode);
    if let Some(p) = dmp {
        let _ = std::fs::remove_file(p);
        cmd.env(DMP_VAR, p);
    }
    let out = cmd.output().expect("子进程");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let dmp_ok = dmp.map(|p| std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)).unwrap_or(false);
    (out.status.code(), text, dmp_ok)
}

#[test]
#[ignore]
fn 旧版必崩_新版不崩() {
    match std::env::var(MODE_VAR).ok().as_deref() {
        Some("old") => {
            unsafe {
                let _ = SetUnhandledExceptionFilter(Some(dump_filter));
            }
            round_a_old(12);
        }
        Some("new") => run_new(200),
        Some(x) => panic!("未知模式 {x}"),
        None => {
            // 父进程
            let dmp = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("temp")
                .join("clip_repro_old.dmp");
            let dmp_s = dmp.to_string_lossy().to_string();

            println!("\n===== A：旧版（崩溃那版源码逐字抄）=====");
            let (code_old, text_old, dmp_ok) = run_child("old", Some(&dmp_s));
            println!("{}", tail(&text_old, 14));
            println!("退出码 = {:?}（0xC0000005 = 访问冲突）", code_old);
            println!("minidump 已生成 = {}", dmp_ok);

            println!("\n===== B：新版（现出货代码）=====");
            let (code_new, text_new, _) = run_child("new", None);
            println!("{}", tail(&text_new, 14));
            println!("退出码 = {:?}", code_new);

            println!("\n===== 结论 =====");
            let reproduced = matches!(code_old, Some(AV)) || dmp_ok;
            println!(
                "旧版崩溃复现   = {}（退出码 {:?}，dmp {}）",
                if reproduced { "是" } else { "否" },
                code_old,
                if dmp_ok { "已落盘" } else { "缺失" }
            );
            println!(
                "新版未崩       = {}（退出码 {:?}）",
                if code_new == Some(0) { "是" } else { "否" },
                code_new
            );
            assert!(reproduced, "旧版没有复现崩溃 —— 说明我对根因的理解是错的");
            assert_eq!(code_new, Some(0), "新版崩了或判据失败");
        }
    }
}

fn tail(s: &str, n: usize) -> String {
    let v: Vec<&str> = s.lines().collect();
    let start = v.len().saturating_sub(n);
    v[start..].join("\n")
}
