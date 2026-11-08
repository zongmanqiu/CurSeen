//! 剪贴板图像格式覆盖面探针（`clip_png_dibv5`）—— 2026-11-09
//!
//! ## 它回答一个问题
//!
//! 用户报「RStudio 里对图片右键 `Copy Image`，能粘到别处，但**不进 CurSeen 的剪贴板
//! 历史**，别的剪贴板工具都收得到」。RStudio Desktop 是 Electron 应用，它的复制走
//! Chromium 的 `ClipboardWin::WriteBitmap`，而那个函数**只写两种格式**：
//!
//! ```text
//! 1) ClipboardFormatType::PngType()      → 注册名 "PNG"（PNG 编码字节）
//! 2) ClipboardFormatType::BitmapType()   → CF_DIBV5（BITMAPV5HEADER + 32bpp）
//! ```
//!
//! **一个 `CF_DIB` 都不写。** 旧实现只问 `CF_DIB`(8)，于是这类来源的图片连"要不要读"
//! 都判成否，静默跳过（连重试都不触发）；别的剪贴板工具认 DIBV5/PNG，所以从外部看不
//! 出问题。同一条路径也盖住了 Chrome / Edge 的「复制图片」。
//!
//! ## 判据（自红设计）
//!
//! 探针自己**按 Chromium 的写法**把图放上剪贴板，再让 `get_image()` 去读：
//!
//! | 用例 | 剪贴板上只有 | 期望 |
//! |---|---|---|
//! | `png_only` | `PNG` | 读到，像素逐字节一致（含 α） |
//! | `dibv5_only` | `CF_DIBV5`（V5 头 + α 掩码 + 非零 α） | 读到，像素一致，α 保留 |
//! | `dib_only` | `CF_DIB`（40 字节头，α 位全 0） | 读到，**α 强制 255**（老行为不许回退） |
//! | `text_only` | `CF_UNICODETEXT` | 读不到（别把文本当图） |
//!
//! 撤掉本轮修复重跑，前两格必然红（`None`）——这就是它的自红证明。
//!
//! ```text
//! python build/probe.py run clip_png_dibv5 -- --nocapture
//! ```
//!
//! ⚠ 本探针会**覆盖当前剪贴板**（跑完留在最后写入的内容上），别在有东西要粘的时候跑。
//!
//! probe-group: fast

#![cfg(windows)]
#![allow(non_snake_case)]

use windows::core::PCWSTR;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GHND};

use curseen_ui::platform::clipboard::get_image;
use curseen_ui::platform::screenshot::{encode_png, Shot};

const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const CF_UNICODETEXT: u32 = 13;

/// 样本：4×3，每个像素都不同（R/B 互换写错、行序翻转写错都会当场对不上）。
/// α 前 6 个像素非零、后 6 个为 0——只测"有没有 α"的判据会被这半边钉住。
fn sample_bgra() -> Vec<u8> {
    let mut v = Vec::with_capacity(4 * 3 * 4);
    for y in 0..3u8 {
        for x in 0..4u8 {
            let a = if y * 4 + x < 6 { 0x80 + x } else { 0 };
            v.extend_from_slice(&[0x10 * (x + 1), 0x20 * (y + 1), 0x90, a]);
        }
    }
    v
}

/// 往全局内存塞一段字节（`SetClipboardData` 之后所有权归系统，**不许自己 GlobalFree**）。
unsafe fn hglobal(bytes: &[u8]) -> Option<HGLOBAL> {
    let hg = GlobalAlloc(GHND, bytes.len()).ok()?;
    if hg.is_invalid() {
        return None;
    }
    let p = GlobalLock(hg);
    if p.is_null() {
        return None;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
    let _ = GlobalUnlock(hg);
    Some(hg)
}

fn register(name: &str) -> u32 {
    let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
}

/// 打印当前剪贴板上的全部格式。
///
/// 为什么要打：本机装着多个剪贴板工具（用户自述），它们**会**在我们写完之后
/// 重写剪贴板（观察到的症状：明明只写了文本，`get_image()` 却能读出图）。判据要能
/// 区分"我们的代码把文本当图"和"剪贴板被第三方换掉了"——只看断言分不清。
fn dump_formats(tag: &str) {
    use windows::Win32::System::DataExchange::{EnumClipboardFormats, GetClipboardFormatNameW};
    unsafe {
        if OpenClipboard(None).is_err() {
            println!("[{tag}] 枚举失败：剪贴板被占用");
            return;
        }
        let mut f = 0u32;
        let mut names = Vec::new();
        loop {
            f = EnumClipboardFormats(f);
            if f == 0 {
                break;
            }
            let mut buf = [0u16; 128];
            let n = GetClipboardFormatNameW(f, &mut buf);
            let name = if n > 0 {
                String::from_utf16_lossy(&buf[..n as usize])
            } else {
                format!("#{f}")
            };
            names.push(name);
        }
        let _ = CloseClipboard();
        println!("[{tag}] 剪贴板格式: {names:?}");
    }
}

/// 图像格式是否"真的"在剪贴板上（与库内 `image_available` 同判据，探针自查前置条件）。
fn image_present() -> bool {
    let png = register("PNG");
    let mime = register("image/png");
    unsafe {
        [CF_DIB, CF_DIBV5, png, mime]
            .into_iter()
            .any(|f| f != 0 && windows::Win32::System::DataExchange::IsClipboardFormatAvailable(f).is_ok())
    }
}

/// 清空剪贴板 → 写 `fmt` → 关闭。`fmt == 0` 表示只清空。
unsafe fn put(fmt: u32, hg: Option<HGLOBAL>) {
    assert!(OpenClipboard(None).is_ok(), "OpenClipboard 失败");
    let _ = EmptyClipboard();
    if let Some(h) = hg {
        assert!(
            SetClipboardData(fmt, Some(windows::Win32::Foundation::HANDLE(h.0))).is_ok(),
            "SetClipboardData(fmt={fmt}) 失败"
        );
    }
    let _ = CloseClipboard();
}

/// Chromium 的 `BitmapType()` 实体：BITMAPV5HEADER(124) + 32bpp + BI_BITFIELDS + α 掩码。
/// 行序自下而上（DIB 的约定），像素 BGRA 原样。
unsafe fn make_dibv5(pixels: &[u8], w: i32, h: i32) -> Option<HGLOBAL> {
    let px_len = (w as usize) * (h as usize) * 4;
    assert_eq!(pixels.len(), px_len, "样本次寸与宽高不符");
    let mut buf = vec![0_u8; 124 + px_len];
    let u = buf.as_mut_ptr().cast::<u32>();
    u.write(124); // bV5Size
    u.add(1).write(w as u32); // bV5Width
    u.add(2).write(h as u32); // bV5Height（正 = 自下而上）
    u.add(3).write((32 << 16) | 1); // bV5BitCount<<16 | bV5Planes
    u.add(4).write(3); // BI_BITFIELDS
    u.add(5).write(px_len as u32); // bV5SizeImage
    u.add(10).write(0x00FF_0000); // bV5RedMask
    u.add(11).write(0x0000_FF00); // bV5GreenMask
    u.add(12).write(0x0000_00FF); // bV5BlueMask
    u.add(13).write(0xFF00_0000); // bV5AlphaMask
    u.add(14).write(0x7352_4742); // bV5CSType = LCS_sRGB ('sRGB')
    // 自下而上：DIB 第一行 = 源图最后一行。
    for row in 0..h as usize {
        let src = (h as usize - 1 - row) * (w as usize) * 4;
        buf[124 + row * (w as usize) * 4..124 + (row + 1) * (w as usize) * 4]
            .copy_from_slice(&pixels[src..src + (w as usize) * 4]);
    }
    hglobal(&buf)
}

/// 老路数：BITMAPINFOHEADER(40) + 32bpp + BI_RGB。α 位**故意填 0**（截图工具就是这样），
/// 读回必须变成 255，否则历史里每张截图都会是透明的。
unsafe fn make_dib40(pixels: &[u8], w: i32, h: i32) -> Option<HGLOBAL> {
    let px_len = (w as usize) * (h as usize) * 4;
    let mut buf = vec![0_u8; 40 + px_len];
    let u = buf.as_mut_ptr().cast::<u32>();
    u.write(40);
    u.add(1).write(w as u32);
    u.add(2).write(h as u32);
    u.add(3).write((32 << 16) | 1);
    u.add(4).write(0);
    for row in 0..h as usize {
        let src = (h as usize - 1 - row) * (w as usize) * 4;
        let dst = 40 + row * (w as usize) * 4;
        for i in 0..(w as usize) * 4 {
            // 只清 α（每 4 字节的第 4 个），RGB 原样。
            buf[dst + i] = if i % 4 == 3 { 0 } else { pixels[src + i] };
        }
    }
    hglobal(&buf)
}

fn check(name: &str, want_alpha: bool) {
    let src = sample_bgra();
    let got = get_image().unwrap_or_else(|| panic!("[{name}] 读不到图像 —— 格式没被认出"));
    assert_eq!((got.w, got.h), (4, 3), "[{name}] 宽高不对");
    for (i, px) in got.bgra.chunks_exact(4).enumerate() {
        let w = &src[i * 4..i * 4 + 4];
        assert_eq!(&px[..3], &w[..3], "[{name}] 第 {i} 个像素的 BGR 不对");
        let want_a = if want_alpha { w[3] } else { 255 };
        assert_eq!(px[3], want_a, "[{name}] 第 {i} 个像素的 α 不对");
    }
    println!("[{name}] ok  {}x{}", got.w, got.h);
}

/// 四个用例写在**一个** `#[test]` 里：它们抢同一份系统剪贴板，拆成多个函数就依赖
/// libtest 的执行顺序（并行时必炸），而"顺序对了"这件事没法用断言保证。
/// 收尾在同一函数里清空剪贴板，不给后续探针留脏数据
/// （`pin_zoom_e2e_probe` 就被剪贴板残留坑过一次）。
#[test]
#[ignore = "真机探针：会覆盖系统剪贴板，按 probe.py run clip_png_dibv5 手动跑"]
fn 剪贴板按_chromium_的两种格式都能读出图() {
    let src = sample_bgra();

    // ① 只有注册名 "PNG"（Chromium 写的第一档）——旧实现直接判"不是图"。
    let png = encode_png(&Shot {
        w: 4,
        h: 3,
        bgra: src.clone(),
    })
    .expect("PNG 编码");
    let fmt = register("PNG");
    unsafe {
        put(fmt, hglobal(&png));
    }
    // ★ 前置条件自查：必须**真的**只有 PNG（否则这一格证明不了新通路——旧代码只认
    //   CF_DIB，若板上同时有 CF_DIB，旧代码也能读，差分就失效了）。
    unsafe {
        assert!(
            !windows::Win32::System::DataExchange::IsClipboardFormatAvailable(CF_DIB).is_ok(),
            "[PNG] 前置条件不成立：板上还有 CF_DIB"
        );
        assert!(
            !windows::Win32::System::DataExchange::IsClipboardFormatAvailable(CF_DIBV5).is_ok(),
            "[PNG] 前置条件不成立：板上还有 CF_DIBV5"
        );
    }
    check("PNG", true);

    // ② 只有 CF_DIBV5（Chromium 写的第二档，V5 头 + α 掩码）。
    unsafe {
        put(CF_DIBV5, make_dibv5(&src, 4, 3));
    }
    unsafe {
        assert!(
            !windows::Win32::System::DataExchange::IsClipboardFormatAvailable(fmt).is_ok(),
            "[DIBV5] 前置条件不成立：板上还有 PNG"
        );
    }
    // ★ 诊断（不参与判据）：系统会给 CF_DIBV5 **合成**一个"可用的 CF_DIB"
    //   （`IsClipboardFormatAvailable(8)` 因此为真），但真正去 `GetClipboardData(8)`
    //   时才看得出有没有数据——**实测两次运行结果不同**（false / true），说明合成
    //   不稳定、不能当数据源。这行就是那条坑的现场记录。
    unsafe {
        let avail = windows::Win32::System::DataExchange::IsClipboardFormatAvailable(CF_DIB).is_ok();
        let got = OpenClipboard(None).is_ok()
            && {
                let r = windows::Win32::System::DataExchange::GetClipboardData(CF_DIB).is_ok();
                let _ = CloseClipboard();
                r
            };
        println!("[DIBV5] 诊断：IsAvailable(CF_DIB)={avail} GetClipboardData(CF_DIB)={got}");
    }
    check("DIBV5", true);

    // ③ 老路数 CF_DIB：α 位全 0，必须刷成 255（这一格是"别把老行为改坏"的护栏）。
    unsafe {
        put(CF_DIB, make_dib40(&src, 4, 3));
    }
    check("DIB", false);

    // ④ 纯文本不该被当成图。
    //
    // ★ 先自查前置条件：本机有别的剪贴板工具会在我们写完之后重写剪贴板。若此刻
    //   图像格式**确实**在板上（`image_present()` 为真），那 `get_image()` 读出图
    //   就是对的——红的是环境，不是代码。这种情况打印格式表并跳过，避免假红。
    let text: Vec<u16> = "hello".encode_utf16().chain(std::iter::once(0)).collect();
    let bytes: Vec<u8> = text.iter().flat_map(|u| u.to_le_bytes()).collect();
    unsafe {
        put(CF_UNICODETEXT, hglobal(&bytes));
    }
    dump_formats("text_only");
    if image_present() {
        println!("[text_only] skipped：图像格式被第三方重新放上剪贴板，本格无效");
    } else {
        assert!(get_image().is_none(), "[text_only] 纯文本不该被当成图");
        println!("[text_only] ok");
    }

    unsafe { put(0, None) };
    println!("[cleanup] 剪贴板已清空");
}


/// 现场诊断（不做断言）：读**当前**剪贴板图像，打印尺寸与前几个像素。
///
/// 用法：在目标程序里复制图片后立刻 `python build/probe.py run clip_png_dibv5`。
/// 2026-11-09 用它确认"RStudio 真实写入的那份剪贴板"能否被 `get_image()` 读出——
/// 探针自己造的 DIBV5 和 Chromium 造的不一定同形（实测 Chromium 用 BI_RGB 而非
/// BI_BITFIELDS，且额外带 HTML Format / CF_BITMAP 合成项）。
#[test]
#[ignore = "现场诊断：读当前剪贴板，需先手动复制一张图"]
fn 现场读当前剪贴板() {
    match get_image() {
        Some(s) => {
            let px = s.bgra.len() / 4;
            let mut a_min = 255u8;
            let mut a_max = 0u8;
            for p in s.bgra.chunks_exact(4) {
                a_min = a_min.min(p[3]);
                a_max = a_max.max(p[3]);
            }
            println!(
                "[now] 读到 {w}x{h} 像素={px} 首像素={:02x?} α∈[{a_min},{a_max}]",
                &s.bgra[..4.min(s.bgra.len())],
                w = s.w,
                h = s.h
            );
        }
        None => println!("[now] get_image() = None（读不到）"),
    }
}
