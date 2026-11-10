//! 系统剪贴板读取（文本 / 图像）——贴图功能的输入源。
//!
//! 只读不写：写入已有 `screenshot::to_clipboard`（CF_DIB）。读取走标准剪贴板 API：
//! 文本取 `CF_UNICODETEXT`（13）；图像按**来源习惯**依次试三种格式（见 [`get_image`]）：
//! `PNG`（注册名）→ `CF_DIBV5`（17）→ `CF_DIB`（8，BITMAPINFOHEADER + 像素）。
//!
//! ★ 为什么不是"只认 CF_DIB"：Chromium 系（Chrome / Edge / Electron，**含 RStudio
//!   Desktop**）复制图片时只写 `PNG` + `CF_DIBV5` 两种，一个 `CF_DIB` 都不写
//!   （Chromium `ClipboardWin::WriteBitmap`：先 `PngType()` 再 `BitmapType()`，后者在
//!   Windows 上就是 DIBV5）。旧实现只问 `CF_DIB` ⇒ 这类来源的图片一条都收不进来，
//!   而别的剪贴板工具都认 DIBV5/PNG，所以从外部看不出问题（2026-11-09 用户报
//!   "RStudio 复制图片不进历史"）。详见《踩坑记录》§27。
//!
//!   ⚠ 顺带一个必须知道的坑：板上有 `CF_DIBV5` 时，`IsClipboardFormatAvailable(CF_DIB)`
//!   会**合成**出一个"可用"的答复，但 `GetClipboardData(CF_DIB)` 能不能真拿到数据并不
//!   保证（探针实测两种结果都出现过）。所以判"有没有图"不能靠单一格式的可用性，
//!   取数据也不能指望"可用"就等于"取得出来"。
//!
//! 所有函数失败一律返回 `None`（剪贴板被别的进程占用、格式不符、空剪贴板都是常态），
//! 调用方（贴图快捷键 / 剪贴板历史）据此回落到各自的兜底。
//!
//! 剪贴板格式常量沿用 `screenshot.rs` 的本地钉住惯例（为一个常量开一整个
//! `Win32_System_Ole` feature 不值）：CF_DIB=8、CF_UNICODETEXT=13、CF_DIBV5=17。

use core::ffi::c_void;
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW,
};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    GlobalLock, GlobalSize, GlobalUnlock, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT,
};

use super::screenshot::Shot;

const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;
const CF_UNICODETEXT: u32 = 13;

/// 剪贴板**内容序列号**（系统维护，每次内容变化 +1）。
///
/// 用它决定"要不要去读剪贴板"：只读一个 u32，比读一次图文内容便宜几个数量级，
/// 因此可以挂在既有的 250ms 对账上而不付任何代价。序列号没变就直接跳过。
///
/// ★ 这是"最多支持 pin 最近 10 个内容"能名副其实的前提：贴图快捷键只在**按键那一瞬**
/// 采样剪贴板，两次按键之间复制的内容全都会丢。有了序列号轮询，任何一次复制都会
/// 在 250ms 内被收进贴图环。
pub fn sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// ★★★ 剪贴板**进程级互斥锁**——2026-10-13 根因修复的核心。
///
/// Windows 对**跨进程**的 `OpenClipboard` 提供互斥，但对**同一进程内**的并发打开
/// **不提供**。实测（`tests/probes/clip_race.rs` 第 1 问）：线程 A 举着不关，线程 B
/// 照样 `OpenClipboard(None)` 成功。于是**我们自己**的写者（5 个 `to_clipboard`
/// 调用点，其中 2 个就在剪贴板模块自己的面板线程上）可以在读者（`clip-thumb` 线程）
/// 持有 CF_DIB 指针做逐行 `memcpy` 的中途 `EmptyClipboard()`，把那块内存整块抽走：
///
/// ```text
/// [第2问] GlobalSize 声称 5,238,364；页表可读 清空前 5,238,720 → 清空后 0
/// ```
///
/// 这就是 2026-10-12 23:10 那次 0xC0000005 的成因。同一现场还有一条反证
/// （探针第 3 问）：**没有写者干扰时 `GlobalSize` 与页表完全一致**——所以
/// "外部畸形数据"的说法不成立，别再往那个方向修。
///
/// ★ 这是 `std::sync::Mutex`，**不可重入**：持锁期间不得再调任何会拿它的函数
/// （`get_text` / `get_image` / `to_clipboard` / `write_files` / html 那两处）。
pub(crate) fn clip_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// 拿剪贴板锁。★ **中毒不传播**：上一个持锁线程 panic 之后锁会中毒，这里直接
/// `into_inner()` 恢复——剪贴板是共享资源，一个线程挂掉不该让整个模块的剪贴板
/// 功能永久失效（那正是"崩溃后程序变僵尸"的同类症状）。
pub(crate) fn lock_clip() -> MutexGuard<'static, ()> {
    match clip_lock().lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

/// 拿锁 → 开剪贴板 → 执行 `f` → 关剪贴板 → 放锁。
///
/// 返回 `None` = 剪贴板开不了（被别的进程占用，常态）。**关剪贴板发生在放锁之前**，
/// 保证"持有剪贴板"这件事始终被锁覆盖。
///
/// ★ 全项目**只有** [`get_text`] / [`get_image`] / [`crate::platform::screenshot::to_clipboard`]
///   等少数几处该碰剪贴板，且一律经这里或 [`lock_clip`]；`build/hygiene_scan.py`
///   会拦住任何绕过门禁的裸 `OpenClipboard`。
pub(crate) fn with_clipboard<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    let _guard = lock_clip();
    unsafe {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let _closer = Closer;
        f()
    }
}

/// RAII：离开作用域时关剪贴板。
struct Closer;

impl Drop for Closer {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// RAII：离开作用域时 GlobalUnlock。
struct Unlocker(*mut u8);

impl Drop for Unlocker {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(HGLOBAL(self.0 as *mut _));
        }
    }
}

/// 读取剪贴板文本（CF_UNICODETEXT）——**原样**，不做任何修剪。
///
/// 与 [`get_text`] 的差别只在末尾：那个版本是“给人看”的（去尾部空白/首尾空白，
/// 空串归 None）；本版本是“给板子看”的——捕获重写上板（把板上内容原样搬走再补
/// 文件引用）用它，保证写回去的文本与来源逐字符一致，不引入我们自己的篡改。
pub fn get_text_raw() -> Option<String> {
    with_clipboard(|| unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT).ok()?;
        let ptr = GlobalLock(HGLOBAL(h.0));
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = Unlocker(ptr.cast());
        // 与 get_text 同一道闸：GlobalSize 不可信，用页表夹住再扫，扫出分配区之外
        // 就是 0xC0000005（2026-10-11 崩溃排查防御项）。
        let total = readable_bytes(ptr.cast(), GlobalSize(HGLOBAL(h.0)) as usize)?;
        let max_chars = total / 2;
        let mut len = 0_usize;
        let u16_ptr = ptr.cast::<u16>();
        while len < max_chars && *u16_ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(u16_ptr, len);
        Some(String::from_utf16_lossy(slice))
    })
}

/// 读取剪贴板文本（CF_UNICODETEXT）。
pub fn get_text() -> Option<String> {
    with_clipboard(|| unsafe {
        if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
            return None;
        }
        let h = GetClipboardData(CF_UNICODETEXT).ok()?;
        let ptr = GlobalLock(HGLOBAL(h.0));
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = Unlocker(ptr.cast());
        // 到双零结尾的 UTF-16 长度。★ 以 GlobalSize 为上界：剪贴板数据的结尾**由
        // 来源进程负责放 NUL**，畸形来源（或不规范的自定义写入方）不给终止符时，
        // 无界扫描会一路读出分配区之外——正好就是 0xC0000005 读越界（2026-10-11
        // 崩溃排查时定的防御项）。越界就按"到此为止"截断，交给 lossy 转换兜底。
        // ★ 与图像同一道闸：`GlobalSize` 报的长度不可信（见 [`readable_bytes`]），
        //   用它当上界扫描字符串同样会扫出分配区外 → 0xC0000005。
        let total = readable_bytes(ptr.cast(), GlobalSize(HGLOBAL(h.0)) as usize)?;
        let max_chars = total / 2;
        let mut len = 0_usize;
        let u16_ptr = ptr.cast::<u16>();
        while len < max_chars && *u16_ptr.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(u16_ptr, len);
        let text = String::from_utf16_lossy(slice);
        let text = text.trim_end_matches(['\r', '\n', ' ']).trim().to_string();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

/// 剪贴板上是否有我们**认得的**图像格式（供调用方决定"要不要去读一次 / 要不要重试"）。
///
/// 判据与 [`get_image`] 一一对应——两处必须同源，否则会出现"说有图、读不出来"
/// 的空转（旧版就是只认 `CF_DIB`，Chromium 系写 DIBV5+PNG 时判据为假、整条逻辑
/// 静默跳过，连重试都不触发）。
pub(crate) fn image_available() -> bool {
    unsafe {
        [CF_DIB, CF_DIBV5, png_format(), png_mime_format()]
            .into_iter()
            .any(|f| f != 0 && IsClipboardFormatAvailable(f).is_ok())
    }
}

/// 注册名 → 剪贴板格式 id（名字大小写不敏感）。
///
/// ★ `Vec<u16>` 必须先绑到 `let`：直接把临时量取址传进去，分号处即 drop，
///   `RegisterClipboardFormatW` 拿到的是已经回收的内存（同 §24 那条坑）。
fn register_format(name: &str) -> u32 {
    let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
}

/// Chromium / Firefox 复制图片时写的 PNG 格式（注册名 `PNG`）。
///
/// ★ 捕获重写上板（`clipboard_history::rewrite_image_board`）也要用同一个格式号：
/// 注册是系统级幂等的（同名回同号），这里作为进程内唯一注册点。
pub(crate) fn png_format() -> u32 {
    static F: OnceLock<u32> = OnceLock::new();
    *F.get_or_init(|| register_format("PNG"))
}

/// 少数应用按 MIME 名注册（`image/png`）——一并认下，代价只是一个 u32。
pub(crate) fn png_mime_format() -> u32 {
    static F: OnceLock<u32> = OnceLock::new();
    *F.get_or_init(|| register_format("image/png"))
}

/// 读取剪贴板图像：`PNG` → `CF_DIBV5` → `CF_DIB`，按这个顺序取第一个解得开的。
///
/// 三档的取舍：
/// - **`PNG`**：无损、α 就是内容本身（没有"DIB 那一套 α 到底算不算数"的歧义），
///   Chromium 系把图片放上剪贴板时**总是**写它，所以这是最该优先的一档。
/// - **`CF_DIBV5`**：BITMAPV5HEADER（124 字节）+ 32bpp + α 掩码。α 保留与否见
///   [`read_dib`] 的判定——**声明了掩码且真有非零 α 才算数**。
/// - **`CF_DIB`**：截图工具 / 画图等老路数，α 一律按 255 处理（见下）。
///
/// ★★★ 2026-10-12 23:10 那次 0xC0000005 的真正成因**不是**"外部畸形数据"，而是
///   **我们自己的写者与读者撞车**：Windows 对同进程内的并发 `OpenClipboard`
///   不提供互斥，于是 `EmptyClipboard()` 能在逐行 `memcpy` 的中途把那块内存抽走
///   （探针 `tests/probes/clip_race.rs` 实测：页表可读 5,238,720 → 0）。根治是
///   [`clip_lock`] 这把进程级锁 —— 本函数已走 [`with_clipboard`]，写者进不来。
///
///   下面两道闸是**纵深防御**（防御别的进程/延迟渲染等尚未穷尽的路径），不是主防线：
///   ① [`readable_bytes`] 用页表把"声称的长度"夹成真值；② 逐行走 [`read_into`]
///   （`ReadProcessMemory` 读自己），读不动就返回 `None`，**永远不会崩**。
///   ⚠ 别再把它当成"剪贴板数据不可信"的证据去夹别的地方——无写者时 `GlobalSize`
///   不会超额声称（同一探针第 3 问：声称 5,238,364 ⊂ 页表可读 5,238,720），
///   夹了只会平白丢内容。
pub fn get_image() -> Option<Shot> {
    with_clipboard(|| unsafe {
        if let Some(s) = read_png() {
            return Some(s);
        }
        if let Some(s) = read_dib(CF_DIBV5) {
            return Some(s);
        }
        read_dib(CF_DIB)
    })
}

/// 读 `PNG` / `image/png` 格式的字节并解码。拿不到或解不开都返回 `None`。
unsafe fn read_png() -> Option<Shot> {
    for fmt in [png_format(), png_mime_format()] {
        if fmt == 0 || IsClipboardFormatAvailable(fmt).is_err() {
            continue;
        }
        let Ok(hmem) = GetClipboardData(fmt) else {
            continue;
        };
        let ptr = GlobalLock(HGLOBAL(hmem.0));
        if ptr.is_null() {
            continue;
        }
        let _guard_mem = Unlocker(ptr.cast());
        // 先拷成自己的 `Vec` 再解：解码器要 `&[u8]`，而这块内存归别的进程所有，
        // 解码不能吊着一把随时可能被 `EmptyClipboard()` 抽走的句柄（§23）。
        let Some(total) = readable_bytes(ptr.cast(), GlobalSize(HGLOBAL(hmem.0)) as usize) else {
            continue;
        };
        if total < 8 {
            continue;
        }
        let mut buf = vec![0_u8; total];
        if !read_into(ptr.cast(), &mut buf) {
            continue;
        }
        if let Some(s) = super::screenshot::decode_png_bytes(&buf) {
            return Some(s);
        }
    }
    None
}

/// 读一个 DIB 句柄（`CF_DIB` 或 `CF_DIBV5`）并解码成自上而下 BGRA。
///
/// 支持 32bpp（BI_RGB / BI_BITFIELDS）与 24bpp；自下而上或 `biHeight<0` 的自上而下
/// 都处理。
///
/// ★ α 的处理分三种情况（照抄 WIC 那条经验：DIB 的 α **默认不可信**）：
///   ① `CF_DIB`：一律刷成 255。截图工具写 32bpp 时 α 位全 0（"未定义"），
///      当真就会整张透明 —— 与 GDI 抓屏同款陷阱。
///   ② `CF_DIBV5` 且 V4/V5 头的 `bV5AlphaMask` 非 0 **且**像素里真有非零 α：保留。
///      Chromium 复制带透明背景的图就走这一档。
///   ③ `CF_DIBV5` 但像素 α 全 0 / 掩码为 0：同样按 255 处理（等价于①）。
unsafe fn read_dib(fmt: u32) -> Option<Shot> {
    if IsClipboardFormatAvailable(fmt).is_err() {
        return None;
    }
    let hmem = GetClipboardData(fmt).ok()?;
    let ptr = GlobalLock(HGLOBAL(hmem.0));
    if ptr.is_null() {
        return None;
    }
    let _guard_mem = Unlocker(ptr.cast());
    // BITMAPINFOHEADER：biSize(4) biWidth(4) biHeight(4, 有符号) planes+bpp(4) compression(4)
    // ★ 先取"真正能读多少"并保证装得下 20 字节的头字段，再解头——畸形 DIB（数据区比头还小）
    //   直接拒绝，否则下面 5 个 u32 的读取本身就是越界（2026-10-11 崩溃排查防御项）。
    let total = readable_bytes(ptr.cast(), GlobalSize(HGLOBAL(hmem.0)) as usize)?;
    if total < 20 {
        return None;
    }
    let base = ptr.cast::<u32>();
    let bi_size = *base as usize;
    let w = *base.add(1) as i32;
    let h_raw = *base.add(2) as i32;
    let bpp = (*base.add(3) >> 16) as u32;
    let compression = *base.add(4);
    // ★ `unsigned_abs`：i32::MIN 的 `abs()` 在发布版自己就是负数，`as usize` 会变成
    //   一个天文数字，上面那道"≤16384"的门会被它绕过去。
    if w <= 0 || w > 16384 || h_raw == 0 || h_raw.unsigned_abs() > 16384 {
        return None;
    }
    let top_down = h_raw < 0;
    let h = h_raw.unsigned_abs() as usize;
    // BI_RGB(0) 与 BI_BITFIELDS(3) 都按无压缩像素走；其余压缩一律不认。
    if !(compression == 0 || compression == 3) {
        return None;
    }
    if !(40..=124).contains(&bi_size) {
        return None;
    }
    // ★ DIBV5 的 α 掩码 = `bV5AlphaMask`，BITMAPV4/V5 头里第 52 字节（u32 索引 13）。
    //   两道前置：`bi_size >= 108`（短头没有这个字段，第 52 字节是像素本身）与
    //   `total >= 56`（掩码占第 52..56 字节；头声称 124 字节、实际只给了 20 字节的
    //   畸形数据不能读出去）。
    let alpha_mask = if bi_size >= 108 && total >= 56 {
        *base.add(13)
    } else {
        0
    };
    // ★★ 像素起点：边界检查与下面的取址**必须共用这一个数**（见 [`pixel_offset`]）。
    let pixel_offset = pixel_offset(bi_size, compression);
    // 像素区不可超出可读长度，畸形头会越界读（UB）。
    if pixel_offset + h * ((w as usize) * 4) > total {
        return None;
    }
    match bpp {
        32 => {
            // 像素紧跟在头（+ BI_BITFIELDS 的 3 个 DWORD 掩码）之后，32bpp 天然 4 字节对齐。
            let px = (ptr.cast::<u8>()).add(pixel_offset);
            let stride = (w as usize) * 4;
            let mut bgra = vec![0_u8; stride * h];
            for y in 0..h {
                let src_row = if top_down { y } else { h - 1 - y };
                // 逐行直读到翻转后的位置：读不动（未提交/已被回收）就整张放弃。
                if !read_into(
                    px.add(src_row * stride),
                    &mut bgra[y * stride..(y + 1) * stride],
                ) {
                    return None;
                }
            }
            // α 是否当真：见函数头 ②/③ —— 声明了掩码**且**真有非零 α 才保留。
            let keep_alpha = honor_alpha(
                fmt == CF_DIBV5,
                alpha_mask,
                bgra.chunks_exact(4).any(|p| p[3] != 0),
            );
            if !keep_alpha {
                for a in bgra.chunks_exact_mut(4) {
                    a[3] = 255;
                }
            }
            Some(Shot {
                w,
                h: h as i32,
                bgra,
            })
        }
        24 => {
            let stride = ((w as usize) * 3).div_ceil(4) * 4;
            let px = (ptr.cast::<u8>()).add(pixel_offset);
            // ★ 只读每行的 `w*3` 个真实像素字节：行末的对齐填充**最后一行的**有些
            //   来源根本没写（DIB 允许），读它会白失败一次。
            let need = (w as usize) * 3;
            let mut bgra = vec![0_u8; (w as usize) * h * 4];
            let mut row = vec![0_u8; need];
            for y in 0..h {
                let src_row = if top_down { y } else { h - 1 - y };
                if !read_into(px.add(src_row * stride), &mut row) {
                    return None;
                }
                for x in 0..w as usize {
                    let s = x * 3;
                    let d = y * (w as usize) * 4 + x * 4;
                    bgra[d] = row[s];
                    bgra[d + 1] = row[s + 1];
                    bgra[d + 2] = row[s + 2];
                    bgra[d + 3] = 255;
                }
            }
            Some(Shot {
                w,
                h: h as i32,
                bgra,
            })
        }
        _ => None,
    }
}

/// 本进程伪句柄（-1）：`ReadProcessMemory` 用它读自己，权限恒够、也不用关。
/// （windows crate 没给 `GetCurrentProcess`，只有 `GetCurrentProcessId`。）
const SELF_PROCESS: HANDLE = HANDLE(usize::MAX as *mut c_void);

/// 从**可能根本不可读**的地址拷 `dst.len()` 字节；读不到就 `false`，绝不崩。
///
/// ★★ 走 `ReadProcessMemory`（读自己）而不是 `copy_from_slice`：后者就是 memcpy，
///   撞上未提交页/已回收的页 = 0xC0000005，进程当场死（2026-10-12 就是这么死的）；
///   前者只是返回 FALSE。剪贴板 DIB 是别的进程塞进来的数据，"头里声明的尺寸"和
///   "实际能读的内存"不一致时，这是唯一的软着陆方式。
unsafe fn read_into(src: *const u8, dst: &mut [u8]) -> bool {
    let mut read = 0_usize;
    ReadProcessMemory(
        SELF_PROCESS,
        src as *const c_void,
        dst.as_mut_ptr() as *mut c_void,
        dst.len(),
        Some(&mut read),
    )
    .is_ok()
        && read == dst.len()
}

/// `ptr` 之后**真正能读**的字节数（不超过 `claimed`）。
///
/// ★★★ `GlobalSize` 报的只是句柄里记的那个数，**不是**"这块内存有多大"的保证：
///   2026-10-12 那次崩溃里它报 5,238,364，而 `ptr` 之后连续提交的只有 3,262,000 ——
///   逐行拷到第 510 行时撞上未提交页。`VirtualQuery` 给的是页表的实话（从 `ptr` 起
///   同状态同保护属性的那一段还剩多少），拿它把声称值夹住：越界就变成一次 `None`，
///   而不是把隔壁堆内存当成像素拷进缩略图。
unsafe fn readable_bytes(ptr: *const u8, claimed: usize) -> Option<usize> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let n = VirtualQuery(
        Some(ptr as *const c_void),
        &mut info,
        core::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
    );
    if n == 0 || info.State != MEM_COMMIT {
        return None;
    }
    let base = info.BaseAddress as usize;
    if base > ptr as usize {
        return None;
    }
    Some(claimed.min(base + info.RegionSize - ptr as usize))
}

/// DIB 的 α 是否当真（纯函数，规则见 [`read_dib`] 函数头 ②/③）。
///
/// 三条缺一不可：
/// - `fmt_is_v5`：只有 `CF_DIBV5` 这一档才有"α 是内容"的约定。`CF_DIB` 里 32bpp 的
///   第 4 字节多数来源根本不填（截图工具填 0）——当真整张图就变全透明，这是本项目
///   反复踩过的坑。
/// - `alpha_mask != 0`：头里必须真的声明了 α 通道（`bV5AlphaMask`）。
/// - `any_nonzero`：像素里真有非零 α。全 0 只可能是"来源没填"，按不透明处理。
fn honor_alpha(fmt_is_v5: bool, alpha_mask: u32, any_nonzero: bool) -> bool {
    fmt_is_v5 && alpha_mask != 0 && any_nonzero
}

/// 像素区相对 DIB 起点的字节偏移。
///
/// ★★ 边界检查与取址**必须共用这一个返回值**：老代码检查用 `pixel_offset`、取址却写
///   `bi_size + mask_bytes(compression)`，两者在「BITMAPV4/V5 头(≥108) + BI_BITFIELDS」
///   上差 12 字节 —— 既让整张图错 12 字节的零头，又让末尾 12 字节的读脱离检查保护。
/// ★ BITMAPV4/V5 已把掩码收进头内，不能再 +12（否则整张图错 12 字节）。
fn pixel_offset(bi_size: usize, compression: u32) -> usize {
    // BI_BITFIELDS：3 个 DWORD 颜色掩码紧跟 BITMAPINFOHEADER（部分来源写 4 个，
    // 按标准的 3 个算——多出来的那一个属于像素区，与 32bpp 通路的解法一致）。
    if bi_size < 108 && compression == 3 {
        bi_size + 12
    } else {
        bi_size
    }
}

#[cfg(test)]
mod tests {
    use super::honor_alpha;
    use super::pixel_offset;

    /// `read_into` 的两条命脉：① 正常内存要真拷对；② **不可读的地址要返回 false 而不是崩**
    /// （后者就是 2026-10-12 那次 0xC0000005 的软着陆点）。
    #[test]
    fn 软读取拷得对且读不动时不崩() {
        use super::read_into;
        let src = vec![7_u8, 6, 5, 4, 3];
        let mut dst = vec![0_u8; 5];
        assert!(unsafe { read_into(src.as_ptr(), &mut dst) });
        assert_eq!(dst, src);
        // 空指针是最典型的"读不动"：必须安静地 false，绝不能把测试进程带崩。
        assert!(!unsafe { read_into(std::ptr::null(), &mut dst) });
    }

    /// 像素起点：只有「BITMAPINFOHEADER 系(头 < 108) + BI_BITFIELDS」才让出 12 字节掩码。
    #[test]
    fn 掩码只在短头加比特域时占位() {
        assert_eq!(pixel_offset(40, 0), 40);
        assert_eq!(pixel_offset(40, 3), 52);
        assert_eq!(pixel_offset(108, 0), 108);
        // ★ 崩过的地方：V4/V5 头 + BI_BITFIELDS 不能再 +12（掩码已在头内）。
        assert_eq!(pixel_offset(108, 3), 108);
        assert_eq!(pixel_offset(124, 3), 124);
    }

    /// α 的三条政策逐格钉住（见 [`honor_alpha`]）。
    ///
    /// 第一行是**最贵的那一格**：`CF_DIB` 的 32bpp 来源（截图工具）α 位通常全 0，
    /// 一旦当真，剪贴板历史里每张截图都会变成全透明——比"不显示"严重得多。
    #[test]
    fn 只有_dibv5_声明掩码并真有非零透明度才保留() {
        const MASK: u32 = 0xFF00_0000;
        // CF_DIB：哪怕掩码位非 0、像素里有 α，也一律按不透明。
        assert!(!honor_alpha(false, MASK, true));
        // CF_DIBV5 且头里声明了 α 掩码、像素真有非零 α：保留（Chromium 系这条路）。
        assert!(honor_alpha(true, MASK, true));
        // 声明了掩码但 α 全 0（来源没填）：按不透明。
        assert!(!honor_alpha(true, MASK, false));
        // V5 头没写 α 掩码：按不透明。
        assert!(!honor_alpha(true, 0, true));
    }

    /// ★★★ 剪贴板门禁**真的**串行化（2026-10-13 根因修复的回归判据）。
    ///
    /// 形态：A 举着锁不放 ⇒ B 的 `with_clipboard` 必须**进不来**；A 放了 ⇒ B 立刻进得去。
    /// 这是确定性的（不依赖时序运气），去掉 [`clip_lock`] 就会红。
    ///
    /// 为什么它必须是判据而不是"看起来对"：2026-10-12 那次崩溃的根因就是
    /// **没人串行化** —— Windows 对同进程内的并发 `OpenClipboard` 不提供互斥
    /// （探针 `tests/probes/clip_race.rs` 第 1 问实测两个线程能同时打开成功），
    /// 于是写者的 `EmptyClipboard()` 能在读者 memcpy 的中途抽走那块内存。
    #[test]
    fn 剪贴板门禁串行化() {
        use super::{lock_clip, with_clipboard};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let entered = Arc::new(AtomicBool::new(false));
        let holder = lock_clip();
        let e = entered.clone();
        let t = std::thread::Builder::new()
            .name("gate-probe".into())
            .spawn(move || {
                let _ = with_clipboard(|| {
                    e.store(true, Ordering::SeqCst);
                    Some(())
                });
            })
            .expect("起线程");
        std::thread::sleep(std::time::Duration::from_millis(150));
        // A 还举着 ⇒ B 一步都没进来。
        assert!(
            !entered.load(Ordering::SeqCst),
            "门禁失效：A 持锁期间 B 竟然进来了"
        );
        drop(holder);
        t.join().expect("join");
        // 放了锁 B 就该进去（说明 B 不是因为别的原因卡住）。
        assert!(
            entered.load(Ordering::SeqCst),
            "释放之后 B 仍然进不去 —— 门禁把人锁死了"
        );
    }

    /// 门禁中毒后**仍可用**：一个持锁线程 panic 不该让剪贴板功能永久失效
    /// （"崩溃之后整个模块废掉"是 2026-10-12 僵尸进程的同类症状）。
    #[test]
    fn 门禁中毒后仍可用() {
        use super::lock_clip;
        let r = std::thread::Builder::new()
            .name("poison-probe".into())
            .spawn(|| {
                let _g = lock_clip();
                panic!("故意毒化");
            })
            .expect("起线程");
        assert!(r.join().is_err(), "这个线程本就该 panic");
        // 中毒之后仍能拿到锁：不能因为一次 panic 就让后续所有剪贴板访问失败。
        let _g = lock_clip();
    }
}
