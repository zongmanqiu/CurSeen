//! 放大镜面板 → 录制管线的画面共享（「录制合成」通道）。
//!
//! # 为什么需要（2026-09-28 用户报告）
//!
//! 画中画 / 全屏放大窗自带 `WDA_EXCLUDEFROMCAPTURE`（防「无限套娃」，见
//! `pip_window` 模块头）。这个标志是**系统级**的：DXGI 桌面复制同样遵守，
//! 于是录屏里放大窗被整体抠掉——视频里看到的是窗口底下的画面，「没有任何
//! 放大效果」。
//!
//! # 为什么不录制期间动态关掉排除标志
//!
//! `SetWindowDisplayAffinity` 是**窗口级**开关，没有「只对某一次捕获生效」的
//! 粒度。录制期间关掉它，放大镜自己的源图抓取（GDI `BitBlt`，30 FPS）也会
//! 抓到自己 → 「无限套娃」回归（被实测钉死过的问题，见 `magnifier::pip`
//! 模块头）。因此排除标志保持常开，改为**录制管线主动来取画面**：
//!
//! ```text
//! PipWindow::present ──publish 面板位图──▶ 共享内存 ──▶ Session::pump 每帧贴进采集纹理
//!    （屏幕上照常显示）                                    （光标画在面板之上，与屏幕层序一致）
//! ```
//!
//! 面板像素本来就是「干净源图拉伸后的结果」（源图走排除通道，永不套娃），
//! 把它贴进录制帧 = 视频里得到与屏幕一致的放大画面。GIF 与 MP4 两条通路
//! 共用 `Session::pump`，一处合成两边生效。
//!
//! # ★ 为什么走命名共享内存（2026-10-07）
//!
//! 本通道最早是进程内静态槽，2026-10-07 录制链在子进程载体上时合成静默失效过
//! （发布端在主进程、消费端在子进程，静态槽跨不了进程），于是改为**命名文件映射**
//! （`Local\CurSeenPipShare\*`，按会话隔离），发布/消费两端映射同一块内存交换
//! 位图；同步用 seqlock（写两段递增计数，读端前后一致才收）。录制链收回进程内
//! 线程后映射照用（行为一致，测试与生产同一条路径）；映射建不出来时退回进程内
//! 注册表。
//!
//! # 开关与成本
//!
//! 发布要拷整块面板位图（全屏放大时可达整屏大小）。[`set_enabled`] 只在录制
//! 会话存活期间为真（会话 `open` 开、`drop` 关）——不录制时 `present` 一字节
//! 都不拷，槽也保持空。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_ALL_ACCESS,
    MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
};

use crate::platform::pip_window::{FULLSCREEN_WINDOW_TITLE, PIP_WINDOW_TITLE};

/// 一帧可合成的放大镜面板画面。
pub struct PipFrame {
    /// 面板左上角在**虚拟桌面**中的坐标（物理像素）。
    pub x: i32,
    pub y: i32,
    /// 面板尺寸（物理像素）。
    pub w: i32,
    pub h: i32,
    /// 面板像素：top-down 32bpp BGRA，行距 = `w * 4`（与 `PipWindow` 的 DIB 同构）。
    pub bgra: Vec<u8>,
}

/// 消费端按这份固定名单找映射：放大窗就这两把（主进程建窗时以标题为键发布）。
const KNOWN_TITLES: [&str; 2] = [PIP_WINDOW_TITLE, FULLSCREEN_WINDOW_TITLE];

/// 共享内存命名（★ 无 `Local\` 前缀、无反斜杠子路径——映射名前缀后**不允许**
/// 再出现反斜杠，`OpenFileMappingW` 直接 `ERROR_PATH_NOT_FOUND`，2026-10-07
/// 实测；无前缀 = 默认会话私有）。
const NS: &str = "curseen-pipshare-";
const CTL_NAME: &str = "curseen-pipshare-ctl";

/// 槽映射头魔数（"CPSS"）。对不上 = 不是我们的页（别解引用）。
const MAGIC: u32 = 0x4353_5053;
const VERSION: u32 = 1;
/// 头区大小（对齐到 64B，像素数据从这里开始）。
const HDR: usize = 64;
/// 头字段偏移（`seq` 之后的元数据每次发布重写）。
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_SEQ: usize = 8;
const OFF_X: usize = 12;
const OFF_Y: usize = 16;
const OFF_W: usize = 20;
const OFF_H: usize = 24;
const OFF_LEN: usize = 28;
/// 控制页：单个 u32 旗标（1 = 录制会话存活）。
const CTL_OFF: usize = 0;

// ───────────────────────── 录制会话开关 ─────────────────────────

/// 进程内镜像：共享内存不可用时的退路（`set_enabled` 双写、`is_enabled` 先读共享页）。
fn local_enabled() -> &'static AtomicBool {
    static ON: OnceLock<AtomicBool> = OnceLock::new();
    ON.get_or_init(|| AtomicBool::new(false))
}

/// 控制页句柄缓存：首个调用者建，其余打开。
fn ctl_page() -> Option<&'static SharedPage> {
    static CTL: OnceLock<Option<SharedPage>> = OnceLock::new();
    CTL.get_or_init(|| {
        SharedPage::acquire(CTL_NAME, 4, true)
            .map_err(|e| log::debug!("capture_share 控制页不可用（退进程内旗标）: {e}"))
            .ok()
    })
    .as_ref()
}

/// 录制会话开关：会话 `open` 开、`drop` 关（两个进程谁开谁关都写同一页）。
///
/// 关的同时清空所有槽——录制结束后面板内容不再被任何线程消费，留着只会
/// 让下一次录制的第一帧贴出一张过期画面。
pub fn set_enabled(on: bool) {
    local_enabled().store(on, Ordering::Relaxed);
    if let Some(ctl) = ctl_page() {
        ctl.store_u32(CTL_OFF, u32::from(on));
    }
    if !on {
        if let Ok(mut m) = shared_slots().lock() {
            for s in m.values_mut() {
                s.invalidate();
            }
            m.clear() // 槽容量跟面板尺寸走，下轮录制按新尺寸重建
        }
        if let Ok(mut m) = fallback_slots().lock() {
            for g in m.values_mut() {
                *g = None;
            }
        }
        if let Ok(mut c) = reader_cache().lock() {
            c.clear(); // 别把上一场录制的好帧带进下一场
        }
    }
}

/// 发布通道当前是否打开。共享控制页优先（跨进程旗标在那一页上）。
pub fn is_enabled() -> bool {
    if let Some(ctl) = ctl_page() {
        return ctl.load_u32(CTL_OFF) != 0;
    }
    local_enabled().load(Ordering::Relaxed)
}

// ───────────────────────── 发布端（主进程） ─────────────────────────

/// 发布一帧面板画面。面板没变时每帧内容也会变（实时取景器），直接整块重写。
///
/// 共享内存不可用时退进程内注册表（进程内线程载体照常工作）。
pub fn publish(title: &'static str, f: PipFrame) {
    let ok = try_publish_shared(title, &f);
    if !ok {
        let mut g = fallback_slots().lock().expect("capture_share 注册表锁");
        g.insert(title, Some(Arc::new(f)));
    }
}

/// 走共享内存发布。`false` = 建槽失败（调用方退进程内注册表）。
fn try_publish_shared(title: &str, f: &PipFrame) -> bool {
    let Ok(mut m) = shared_slots().lock() else {
        return false;
    };
    // 容量不够（面板变大）→ 丢弃旧映射按新尺寸重建；够则复用。
    let entry = m.remove(title);
    let s = match entry {
        Some(s) if s.data_bytes() >= f.bgra.len() => s,
        other => {
            drop(other);
            match SharedPage::acquire(&slot_name(title), f.bgra.len(), true) {
                Ok(s) => s,
                Err(e) => {
                    log::debug!("capture_share 建槽失败（退进程内注册表）: {e}");
                    return false;
                }
            }
        }
    };
    s.write_frame(f);
    m.insert(title.to_string(), s);
    true
}

/// 面板隐藏时清掉该窗的已发布画面（seq 两段递增地作废，消费端当帧读到"没有"）。
///
/// 没建过槽（本会话从未发布）= 没东西可清，静默返回。
pub fn clear(title: &str) {
    if let Ok(mut m) = shared_slots().lock() {
        if let Some(s) = m.get_mut(title) {
            s.invalidate();
        }
    }
    if let Ok(mut g) = fallback_slots().lock() {
        if let Some(slot) = g.get_mut(title) {
            *slot = None; // 退路注册表同样清
        }
    }
}

// ───────────────────────── 消费端（录制进程） ─────────────────────────

/// 每标题"上一帧好帧"缓存：seqlock 撕裂的轮次复用它，产物不再闪"原屏幕"。
/// `Empty`（面板隐藏/从未发布）时清掉，避免隐藏后残留画面被一直贴。
fn reader_cache() -> &'static Mutex<HashMap<String, Arc<PipFrame>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<PipFrame>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 录制线程取当前所有面板帧（seqlock 撕裂的轮次复用上一帧好帧，见
/// [`reader_cache`]）。
pub fn snapshot() -> Vec<Arc<PipFrame>> {
    if !is_enabled() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for title in KNOWN_TITLES {
        match read_shared(title) {
            SlotRead::Ready(f) => {
                let f = Arc::new(f);
                if let Ok(mut c) = reader_cache().lock() {
                    c.insert(title.to_string(), Arc::clone(&f));
                }
                out.push(f);
            }
            SlotRead::Torn => {
                // 写端正在写整屏位图（全屏放大 8MB/帧）：复用上一帧好帧，
                // 而不是让这一帧录制出没有面板的裸桌面。
                if let Ok(c) = reader_cache().lock() {
                    if let Some(f) = c.get(title) {
                        out.push(Arc::clone(f));
                    }
                }
            }
            SlotRead::Empty => {
                if let Ok(mut c) = reader_cache().lock() {
                    c.remove(title);
                }
            }
        }
    }
    // 退路：共享内存不可用时的进程内注册表。
    if let Ok(map) = fallback_slots().lock() {
        for g in map.values() {
            if let Some(f) = g.as_ref() {
                out.push(Arc::clone(f));
            }
        }
    }
    out
}

/// 从共享槽读一次的结果。撕裂（写端正在写/读到一半被改写）与"没有画面"
/// 必须区分：撕裂时消费端应复用上一帧好帧（全屏放大整屏 8MB 重写，写窗口
/// 毫秒级，30fps 消费者撞上的概率不小——每撞一次产物就闪一帧"原屏幕"），
/// 而面板隐藏/从未发布才是真的"这一帧没有"。
pub enum SlotRead {
    /// 拿到完整一帧。
    Ready(PipFrame),
    /// seqlock 撕裂：本轮没读到，但槽里显然有画面——复用上一帧。
    Torn,
    /// 槽里确实没有画面（从未发布 / 已被 invalidate）。
    Empty,
}

/// 从共享槽读一帧（打开失败 → Empty；撕裂 → Torn；详见 [`SlotRead`]）。
///
/// 句柄无论成败都留在缓存里（下帧重试；视图按创建方整块大小映射，越界由
/// 写端的 `byte_len` 校验兜住）。打开失败**必须留痕**：跨进程通道静默失效过
/// 一次（2026-10-07 子进程读到空槽），没有日志现场无从查起。
fn read_shared(title: &str) -> SlotRead {
    let readers: &Mutex<HashMap<String, SharedPage>> = reader_slots();
    let mut map = match readers.lock() {
        Ok(m) => m,
        Err(_) => return SlotRead::Empty,
    };
    let name = slot_name(title);
    let page = match map.remove(&name) {
        Some(p) => p,
        None => match SharedPage::acquire(&name, 0, false) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("capture_share 打开槽 {name} 失败: {e}");
                return SlotRead::Empty;
            }
        },
    };
    let r = page.read_frame();
    map.insert(name, page);
    r
}

// ───────────────────────── 共享页原语 ─────────────────────────

fn slot_name(title: &str) -> String {
    // 标题含非 ASCII，映射名用 FNV-1a 哈希（稳定、无非法字符）。
    let mut h: u32 = 0x811C_9DC5;
    for b in title.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{NS}s{h:08x}")
}

fn shared_slots() -> &'static Mutex<HashMap<String, SharedPage>> {
    static M: OnceLock<Mutex<HashMap<String, SharedPage>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 消费端只读句柄缓存（槽按需打开）。
fn reader_slots() -> &'static Mutex<HashMap<String, SharedPage>> {
    static M: OnceLock<Mutex<HashMap<String, SharedPage>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 进程内退路注册表（共享内存建不出来时才用）。
fn fallback_slots() -> &'static Mutex<HashMap<&'static str, Option<Arc<PipFrame>>>> {
    type FallbackMap = HashMap<&'static str, Option<Arc<PipFrame>>>;
    static M: OnceLock<Mutex<FallbackMap>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 一块命名文件映射 + 视图。
///
/// `acquire(name, bytes, create)`：`create=true` 时建（已存在则复用）；
/// `bytes=0` 表示以只读打开并采纳创建方的整块大小（消费端）。
struct SharedPage {
    map: HANDLE,
    view: *mut u8,
    bytes: usize,
}

// 映射与视图是进程级资源（不绑定线程），锁保护下跨线程使用安全。
unsafe impl Send for SharedPage {}
unsafe impl Sync for SharedPage {}

impl SharedPage {
    /// 打开或创建。`Err` = Win32 失败（调用方走退路）。
    fn acquire(name: &str, bytes: usize, create: bool) -> Result<Self, String> {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let (map, view_bytes) = if create {
                // 高位 0：≤2GB 用低参即可（面板最大也就整屏 BGRA 级别）。
                let map = CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    None,
                    PAGE_READWRITE,
                    0,
                    (HDR + bytes) as u32,
                    PCWSTR(wide.as_ptr()),
                )
                .map_err(|e| format!("CreateFileMappingW({name}): {e}"))?;
                (map, HDR + bytes)
            } else {
                let map = OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, PCWSTR(wide.as_ptr()))
                    .map_err(|e| format!("OpenFileMappingW({name}): {e}"))?;
                (map, 0) // 0 = 采纳创建方大小
            };
            let addr = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if addr.Value.is_null() {
                let _ = CloseHandle(map);
                return Err(format!("MapViewOfFile({name}) 失败"));
            }
            let view_bytes = if create {
                view_bytes
            } else {
                // 只读打开：VirtualQuery 太重，直接读头里的容量不够用——映射视图
                // 大小本就是创建方的整块大小，越界由 `byte_len` 校验兜住。
                0
            };
            Ok(Self {
                map,
                view: addr.Value.cast(),
                bytes: view_bytes,
            })
        }
    }

    /// 数据区容量（字节）。消费端打开的页返回 0（未知，靠 `byte_len` 校验）。
    fn data_bytes(&self) -> usize {
        self.bytes.saturating_sub(HDR)
    }

    fn store_u32(&self, off: usize, v: u32) {
        unsafe { AtomicU32::from_ptr(self.view.add(off).cast()) }.store(v, Ordering::Release);
    }

    fn load_u32(&self, off: usize) -> u32 {
        unsafe { AtomicU32::from_ptr(self.view.add(off).cast()) }.load(Ordering::Acquire)
    }

    /// 写一帧（seqlock 写端：seq +1 → 写像素与元数据 → seq +1）。
    fn write_frame(&self, f: &PipFrame) {
        let need = f.w as usize * f.h as usize * 4;
        if f.w <= 0 || f.h <= 0 || f.bgra.len() < need || self.data_bytes() < need {
            return;
        }
        unsafe {
            let seq = AtomicU32::from_ptr(self.view.add(OFF_SEQ).cast());
            seq.fetch_add(1, Ordering::AcqRel); // → 奇数 = 写入中
            std::ptr::copy_nonoverlapping(f.bgra.as_ptr(), self.view.add(HDR), need);
            // 元数据写在像素之后：读端拿旧元数据配新像素时尺寸恒一致。
            let set = |off: usize, v: u32| {
                (self.view.add(off)).cast::<u32>().write_volatile(v);
            };
            let seti = |off: usize, v: i32| set(off, v as u32);
            seti(OFF_X, f.x);
            seti(OFF_Y, f.y);
            seti(OFF_W, f.w);
            seti(OFF_H, f.h);
            set(OFF_LEN, need as u32);
            set(OFF_MAGIC, MAGIC);
            set(OFF_VERSION, VERSION);
            seq.fetch_add(1, Ordering::Release); // → 偶数 = 稳定
        }
    }

    /// 读一帧（seqlock 读端：前后 seq 一致且为偶数才收）。
    ///
    /// 撕裂（写中 / 读期间被改写）→ [`SlotRead::Torn`]；从未发布 / 已作废 →
    /// [`SlotRead::Empty`]；各头异常分支一次性留痕（跨进程通道静默失效过，
    /// 见模块头）。
    fn read_frame(&self) -> SlotRead {
        unsafe {
            let seq = AtomicU32::from_ptr(self.view.add(OFF_SEQ).cast());
            let get = |off: usize| (self.view.add(off)).cast::<u32>().read_volatile();
            let s1 = seq.load(Ordering::Acquire);
            if s1 == 0 {
                once_diag("读端：写端从未发布过（seq=0）");
                return SlotRead::Empty;
            }
            if s1 % 2 != 0 {
                return SlotRead::Torn; // 正在写
            }
            if get(OFF_MAGIC) != MAGIC || get(OFF_VERSION) != VERSION {
                once_diag(&format!(
                    "读端：头不匹配 magic={:x} version={}",
                    get(OFF_MAGIC),
                    get(OFF_VERSION)
                ));
                return SlotRead::Empty;
            }
            let len = get(OFF_LEN) as usize;
            let w = get(OFF_W) as i32;
            let h = get(OFF_H) as i32;
            if w <= 0 || h <= 0 || len != w as usize * h as usize * 4 {
                if len == 0 {
                    return SlotRead::Empty; // invalidate 过：面板隐藏
                }
                once_diag(&format!("读端：尺寸异常 w={w} h={h} len={len}"));
                return SlotRead::Empty;
            }
            let mut bgra = vec![0u8; len];
            std::ptr::copy_nonoverlapping(self.view.add(HDR), bgra.as_mut_ptr(), len);
            if seq.load(Ordering::Acquire) != s1 {
                return SlotRead::Torn; // 读期间被改写
            }
            SlotRead::Ready(PipFrame {
                x: get(OFF_X) as i32,
                y: get(OFF_Y) as i32,
                w,
                h,
                bgra,
            })
        }
    }

    /// 清空内容（`set_enabled(false)` 用）：seq 两段递增包一次"删数据"。
    fn invalidate(&self) {
        unsafe {
            let seq = AtomicU32::from_ptr(self.view.add(OFF_SEQ).cast());
            seq.fetch_add(1, Ordering::AcqRel);
            (self.view.add(OFF_LEN)).cast::<u32>().write_volatile(0);
            seq.fetch_add(1, Ordering::Release);
        }
    }
}

/// 诊断留痕（每条只打一次，防 30FPS 刷屏）。
fn once_diag(msg: &str) {
    static SEEN: OnceLock<Mutex<std::collections::HashSet<u64>>> = OnceLock::new();
    let seen = SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()));
    let key = {
        let mut h: u64 = 1469598103934665603;
        for b in msg.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(1099511628211);
        }
        h
    };
    if seen.lock().map(|mut s| s.insert(key)).unwrap_or(false) {
        log::warn!("capture_share {msg}");
    }
}

impl Drop for SharedPage {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view.cast(),
            });
            let _ = CloseHandle(self.map);
        }
    }
}

// ───────────────────────── 纯几何（不变） ─────────────────────────

/// 面板矩形（虚拟桌面）→ 采集纹理（本输出、已裁剪）坐标，并与纹理边界求交。
///
/// `panel`/`origin`/`tex` 分别是面板矩形 `(x, y, w, h)`、输出原点 `(x, y)` 与
/// 纹理尺寸 `(w, h)`。返回 [`BlitRect`]：目标落在纹理上，源落在面板位图内，两者
/// 同宽高（1:1 直拷，无缩放——缩放由录制管线后续的输出缩放统一做）。与纹理无
/// 交集（面板在另一块屏上 / 完全在裁剪区外）→ `None`。
///
/// 纯函数（不碰 Win32），可单测。
pub struct BlitRect {
    /// 目标（纹理坐标）与尺寸。
    pub dst_x: i32,
    pub dst_y: i32,
    /// 源（面板位图内偏移）。
    pub src_x: i32,
    pub src_y: i32,
    /// 交区尺寸（目标与源同值）。
    pub w: i32,
    pub h: i32,
}

pub fn pip_blit_rect(
    panel: (i32, i32, i32, i32),
    origin: (i32, i32),
    tex: (u32, u32),
) -> Option<BlitRect> {
    let (fx, fy, fw, fh) = panel;
    if fw <= 0 || fh <= 0 {
        return None;
    }
    let tx = fx - origin.0;
    let ty = fy - origin.1;
    let left = tx.max(0);
    let top = ty.max(0);
    let right = (tx + fw).min(tex.0 as i32);
    let bottom = (ty + fh).min(tex.1 as i32);
    if right <= left || bottom <= top {
        return None;
    }
    // 源偏移 = 交区在面板位图内的位置（面板被纹理边界裁掉的部分不拷）。
    Some(BlitRect {
        dst_x: left,
        dst_y: top,
        src_x: left - tx,
        src_y: top - ty,
        w: right - left,
        h: bottom - top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blit_rect_maps_panel_to_texture_origin() {
        // 面板 (1100,200) 在 origin=(1000,100) 的输出上 → 纹理 (100,100)，整块在内。
        let r = pip_blit_rect((1100, 200, 400, 300), (1000, 100), (1920, 1080)).unwrap();
        assert_eq!(
            (r.dst_x, r.dst_y, r.src_x, r.src_y, r.w, r.h),
            (100, 100, 0, 0, 400, 300)
        );
    }

    #[test]
    fn blit_rect_clips_to_texture_bounds() {
        // 右、下同时越出纹理：tx=1800、ty=800，交区右缘裁到 1920、下缘裁到 1080。
        let r = pip_blit_rect((2800, 900, 400, 300), (1000, 100), (1920, 1080)).unwrap();
        assert_eq!(
            (r.dst_x, r.dst_y, r.src_x, r.src_y, r.w, r.h),
            (1800, 800, 0, 0, 120, 280)
        );
        // 左上在纹理外（负方向）：交区从纹理 (0,0) 起，源偏移 = 被裁掉的部分。
        let r = pip_blit_rect((900, 50, 400, 300), (1000, 100), (1920, 1080)).unwrap();
        assert_eq!(
            (r.dst_x, r.dst_y, r.src_x, r.src_y, r.w, r.h),
            (0, 0, 100, 50, 300, 250)
        );
    }

    #[test]
    fn blit_rect_skips_disjoint_panel() {
        // 面板在另一块屏（完全在纹理右外）→ None。
        assert!(pip_blit_rect((3000, 200, 400, 300), (1000, 100), (1920, 1080)).is_none());
        // 退化尺寸 → None。
        assert!(pip_blit_rect((1100, 200, 0, 300), (1000, 100), (1920, 1080)).is_none());
    }

    #[test]
    fn publish_roundtrip_and_disable_clears() {
        set_enabled(true);
        assert!(is_enabled());
        // 没发布过 → 不出帧。
        assert!(snapshot().is_empty());
        // 发布一帧（用真实放大窗标题，消费端按名单找它）→ 快照可见。
        publish(
            PIP_WINDOW_TITLE,
            PipFrame {
                x: 1,
                y: 2,
                w: 4,
                h: 4,
                bgra: vec![7u8; 64],
            },
        );
        let snap = snapshot();
        assert_eq!(snap.len(), 1, "应恰好读到画中画一帧，实际 {}", snap.len());
        assert_eq!((snap[0].x, snap[0].y, snap[0].w, snap[0].h), (1, 2, 4, 4));
        assert!(snap[0].bgra.iter().all(|&b| b == 7));
        // 面板变大 → 重建槽后仍可读（容量跟随新尺寸）。
        publish(
            PIP_WINDOW_TITLE,
            PipFrame {
                x: 3,
                y: 4,
                w: 8,
                h: 8,
                bgra: vec![9u8; 256],
            },
        );
        let snap = snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!((snap[0].w, snap[0].h), (8, 8));
        // 关开关 → 槽清空 + 快照恒空。
        set_enabled(false);
        assert!(!is_enabled());
        assert!(snapshot().is_empty());
    }
}
