//! 剪贴板并发门禁探针（2026-10-13）。
//!
//! # 为什么要有这个探针
//!
//! 2026-10-12 23:10 的 0xC0000005 崩在 `clipboard.rs::get_image` 的逐行 `memcpy`：
//! DIB 头声明 `w=1599 h=819`（需 5,238,364 字节），但 `ptr` 之后**连续可读的只有约
//! 3.2 MB**，拷到第 510 行撞上未提交页。当时我把它归因为"外部畸形数据"，拿
//! `GlobalSize` 当上界去夹 —— 结果同一版当晚又崩一次，说明**那个前提是错的**。
//!
//! `GlobalSize` 恰好等于"头 40 + 510 行"这个事实说明：内存是**在拷贝过程中被截断的**，
//! 不是生产者少分配了。能截断它的只有 `EmptyClipboard()`。于是问题变成两问：
//!
//! 1. 同一进程的两个线程，能不能**同时** `OpenClipboard(None)` 成功？
//!    能 ⇒ 我们自己的写者（`screenshot::to_clipboard`，5 个调用点里有 2 个就在剪贴板
//!    模块自己的面板线程上）可以在 `clip-thumb` 线程持有指针 memcpy 的中途把那块内存
//!    释放掉。这就是"程序内部顺序错了"。
//! 2. 写者清空之后，读者手里那个指针的**可读长度**会不会缩水？
//!    缩水 ⇒ 与崩溃现场同形，第 1 问的后果坐实。
//!
//! ★ 全程只 `VirtualQuery` 查询，**绝不**按头里声明的长度去解引用 —— 要证明的是
//!   "内存被截断"，不是再制造一次崩溃。探针自己因此不会崩。
//!
//! # 实测结论（2026-10-13 00:27，修复前）
//!
//! ```text
//! [第1问] A 打开=true  B 在 A 未关时打开=true  ⇒ 本进程内并发打开 不被互斥
//! [第2问] GlobalSize 声称 5,238,364；页表可读 清空前 5,238,720 → 清空后 0
//! [基线]  GlobalSize=5,238,364 页表可读=5,238,720 ⇒ 声称值可信
//! ```
//!
//! 三条合起来把"外部畸形数据"这个方向**彻底排除**（第 3 问：无写者时声称值从没说谎），
//! 并坐实根因是我们自己的写者抽走了读者手里的内存。
//!
//! # 修复与判据
//!
//! 根治在 `platform::clipboard::clip_lock`（进程级互斥锁），所有剪贴板访问经
//! `with_clipboard` / `lock_clip`；`build/hygiene_scan.py` 拦住任何绕过门禁的裸
//! `OpenClipboard`。**确定性回归判据**是 `clipboard.rs` 里的两个单元测试
//! （`剪贴板门禁串行化` / `门禁中毒后仍可用`）——本探针只负责取证，不进 `cargo test`。
//!
//! ```text
//! python build/probe.py run clip_race
//! ```
//!
//! probe-group: fast

#![cfg(windows)]
#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, VirtualQuery, GHND,
    MEMORY_BASIC_INFORMATION, MEM_COMMIT,
};

const CF_DIB: u32 = 8;

/// 头里声明的宽高 → 一块合法的 CF_DIB（自下而上、32bpp、BI_RGB）。
///
/// 只填头 + 按 `stride*h` 分配；像素区留零即可（本探针不校验像素内容）。
unsafe fn make_dib(w: i32, h: i32) -> Option<HGLOBAL> {
    let stride = (w as usize) * 4;
    let total = 40 + stride * (h as usize);
    let hg = GlobalAlloc(GHND, total).ok()?;
    if hg.is_invalid() {
        return None;
    }
    let p = GlobalLock(hg);
    if p.is_null() {
        return None;
    }
    let base = p.cast::<u32>();
    base.write(40); // biSize
    base.add(1).write(w as u32); // biWidth
    base.add(2).write(h as u32); // biHeight（正 = 自下而上）
    base.add(3).write((32 << 16) | 1); // biBitCount<<16 | biPlanes
    base.add(4).write(0); // BI_RGB
    let _ = GlobalUnlock(hg);
    Some(hg)
}

/// `ptr` 之后**连续可读**的字节数（逐段走页表，遇到未提交/不可读就停）。
///
/// 这是崩溃现场那个"实际只有 3.2 MB"的同一个量：不靠 `GlobalSize` 的声称值。
unsafe fn readable_from(ptr: *const u8) -> usize {
    let mut cur = ptr as usize;
    let mut total = 0usize;
    for _ in 0..64 {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let n = VirtualQuery(
            Some(cur as *const c_void),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        );
        if n == 0 || info.State != MEM_COMMIT {
            break;
        }
        // PAGE_NOACCESS=0x01 / PAGE_GUARD=0x100：看着提交但读不得。
        if info.Protect.0 & 0x01 != 0 || info.Protect.0 & 0x100 != 0 {
            break;
        }
        let base = info.BaseAddress as usize;
        if base > cur {
            break;
        }
        let end = base + info.RegionSize;
        total += end - cur;
        cur = end;
        if total > 64 << 20 {
            break;
        }
    }
    total
}

fn wait_flag(f: &AtomicBool, limit: Duration) -> bool {
    let t0 = Instant::now();
    while !f.load(Ordering::SeqCst) {
        if t0.elapsed() > limit {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

/// 第 1 问：同一进程两个线程能否同时 `OpenClipboard(None)` 成功。
///
/// A 打开后**一直不关**，B 在此期间尝试打开。两者都成功 ⇒ 系统对本进程内的并发
/// 打开**不提供互斥**，我们自己的写者就能在读者拷贝中途 `EmptyClipboard()`。
#[test]
#[ignore]
fn 两线程能否同时打开剪贴板() {
    let a_open = Arc::new(AtomicBool::new(false));
    let a_ready = Arc::new(AtomicBool::new(false));
    let b_open = Arc::new(AtomicBool::new(false));
    let b_done = Arc::new(AtomicBool::new(false));

    let (a1, ar) = (a_open.clone(), a_ready.clone());
    let ta = std::thread::Builder::new()
        .name("probe-clip-A".into())
        .spawn(move || unsafe {
            let ok = OpenClipboard(None).is_ok();
            a1.store(ok, Ordering::SeqCst);
            ar.store(true, Ordering::SeqCst);
            // 举着不放，给 B 一个明确的并发窗口。
            std::thread::sleep(Duration::from_millis(300));
            if ok {
                let _ = CloseClipboard();
            }
        })
        .expect("线程 A");

    let (b1, b2, br) = (b_open.clone(), b_done.clone(), a_ready.clone());
    let tb = std::thread::Builder::new()
        .name("probe-clip-B".into())
        .spawn(move || unsafe {
            if !wait_flag(&br, Duration::from_secs(2)) {
                return;
            }
            let ok = OpenClipboard(None).is_ok();
            b1.store(ok, Ordering::SeqCst);
            if ok {
                let _ = CloseClipboard();
            }
            b2.store(true, Ordering::SeqCst);
        })
        .expect("线程 B");

    ta.join().expect("join A");
    tb.join().expect("join B");

    println!(
        "\n[第1问] A 打开={}  B 在 A 未关时打开={}\n        ⇒ 本进程内并发打开 {}互斥",
        a_open.load(Ordering::SeqCst),
        b_open.load(Ordering::SeqCst),
        if a_open.load(Ordering::SeqCst) && b_open.load(Ordering::SeqCst) {
            "不被"
        } else {
            "被"
        }
    );
    // 这是**取证**探针：结论打出来，断言只钉"两个线程都真的跑到了"，
    // 免得某天线程起不来时探针静默假绿。
    assert!(a_ready.load(Ordering::SeqCst) && b_done.load(Ordering::SeqCst));
}

/// 第 2 问：写者在读者持有指针期间 `EmptyClipboard()`，读者手里的块会不会缩水。
///
/// 形态必须与崩溃现场一致：`GlobalSize` 仍报原值，但页表给的可读长度掉了。
#[test]
#[ignore]
fn 写者清空会让读者手中的指针缩水() {
    let before = Arc::new(AtomicUsize::new(0));
    let after = Arc::new(AtomicUsize::new(0));
    let claimed = Arc::new(AtomicUsize::new(0));
    let ready = Arc::new(AtomicBool::new(false));
    let b_open = Arc::new(AtomicBool::new(false));
    let b_done = Arc::new(AtomicBool::new(false));

    // 与崩溃现场同一量级的图：1599×819，stride 6396。
    let (w, h) = (1599i32, 819i32);

    let (c0, c1, c2, c3, c4, c5) = (
        before.clone(),
        after.clone(),
        claimed.clone(),
        ready.clone(),
        b_done.clone(),
        b_open.clone(),
    );
    let ta = std::thread::Builder::new()
        .name("probe-clip-reader".into())
        .spawn(move || unsafe {
            if OpenClipboard(None).is_err() {
                return;
            }
            let hg = match make_dib(w, h) {
                Some(g) => g,
                None => {
                    let _ = CloseClipboard();
                    return;
                }
            };
            if SetClipboardData(CF_DIB, Some(HANDLE(hg.0))).is_err() {
                let _ = GlobalFree(Some(hg));
                let _ = CloseClipboard();
                return;
            }
            // 交出所有权后经剪贴板取回，与 get_image 同一条路。
            let h2 = GetClipboardData(CF_DIB).ok();
            let ptr = match h2 {
                Some(x) => GlobalLock(HGLOBAL(x.0)).cast::<u8>(),
                None => {
                    let _ = CloseClipboard();
                    return;
                }
            };
            if ptr.is_null() {
                let _ = CloseClipboard();
                return;
            }
            c2.store(GlobalSize(HGLOBAL(h2.unwrap().0)) as usize, Ordering::SeqCst);
            c0.store(readable_from(ptr), Ordering::SeqCst);
            c3.store(true, Ordering::SeqCst);

            let _ = wait_flag(&c4, Duration::from_secs(2));
            // ★ 只读查询，不解引用 —— 探针自己不能崩。
            c1.store(readable_from(ptr), Ordering::SeqCst);

            let _ = GlobalUnlock(HGLOBAL(h2.unwrap().0));
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
            let _ = c5;
        })
        .expect("读者线程");

    let (d0, d1, d2) = (b_open.clone(), b_done.clone(), ready.clone());
    let tb = std::thread::Builder::new()
        .name("probe-clip-writer".into())
        .spawn(move || unsafe {
            if !wait_flag(&d2, Duration::from_secs(2)) {
                d1.store(true, Ordering::SeqCst);
                return;
            }
            let ok = OpenClipboard(None).is_ok();
            d0.store(ok, Ordering::SeqCst);
            if ok {
                // ★ 就是这一句：把读者手里那块内存的所有权收回。
                let _ = EmptyClipboard();
                if let Some(small) = make_dib(64, 64) {
                    if SetClipboardData(CF_DIB, Some(HANDLE(small.0))).is_err() {
                        let _ = GlobalFree(Some(small));
                    }
                }
                let _ = CloseClipboard();
            }
            d1.store(true, Ordering::SeqCst);
        })
        .expect("写者线程");

    ta.join().expect("join 读者");
    tb.join().expect("join 写者");

    let b0 = before.load(Ordering::SeqCst);
    let b1 = after.load(Ordering::SeqCst);
    println!(
        "\n[第2问] 写者打开={}\n        GlobalSize 声称 = {} 字节\n        页表可读: 清空前 {} → 清空后 {}\n        ⇒ {}",
        b_open.load(Ordering::SeqCst),
        claimed.load(Ordering::SeqCst),
        b0,
        b1,
        if b1 < b0 {
            format!("缩水 {} 字节（与崩溃现场同形：拷贝中途内存被截断）", b0 - b1)
        } else {
            "未缩水".to_string()
        }
    );
    assert!(b0 > 0, "读者没拿到块，探针没跑到点上");
}

/// 顺带记一笔：`GlobalSize` 与页表可读长度平时是否一致（没有写者干扰时）。
///
/// 若两者**本来就**不一致，那"畸形外部数据"的说法才站得住；一致则彻底排除。
#[test]
#[ignore]
fn 无写者时GlobalSize与页表一致() {
    unsafe {
        if OpenClipboard(None).is_err() {
            println!("打不开剪贴板，跳过");
            return;
        }
        let hg = match make_dib(1599, 819) {
            Some(g) => g,
            None => {
                let _ = CloseClipboard();
                return;
            }
        };
        if SetClipboardData(CF_DIB, Some(HANDLE(hg.0))).is_err() {
            let _ = GlobalFree(Some(hg));
            let _ = CloseClipboard();
            return;
        }
        let h2 = GetClipboardData(CF_DIB).ok();
        if let Some(x) = h2 {
            let ptr = GlobalLock(HGLOBAL(x.0)).cast::<u8>();
            if !ptr.is_null() {
                let g = GlobalSize(HGLOBAL(x.0)) as usize;
                let r = readable_from(ptr);
                println!(
                    "\n[基线] GlobalSize={} 页表可读={} 差值={} ⇒ {}",
                    g,
                    r,
                    g - r,
                    if r >= g { "声称值可信" } else { "声称值虚高" }
                );
                let _ = GlobalUnlock(HGLOBAL(x.0));
            }
        }
        let _ = EmptyClipboard();
        let _ = CloseClipboard();
    }
}
