//! CF_HTML（"HTML Format"）富文本剪贴板保真 —— 捕获时原样存、粘贴时原样回。
//!
//! # Why（为什么做它）
//!
//! 浏览器 / Office 复制富文本时，剪贴板上同时有 `CF_UNICODETEXT`（纯文本兜底）与
//! 注册格式 **"HTML Format"**（CF_HTML：一段带字节偏移头的 HTML）。旧实现只读纯文本，
//! 历史里存的是"剥掉格式后的骨架"，粘贴回浏览器/Word 丢失全部排版（颜色/加粗/表格/
//! 链接）——2026-10-07 立项补齐。
//!
//! # 方案：原样字节，零解析
//!
//! CF_HTML 头里的 `StartHTML/EndHTML` 是**字节偏移**，各家实现质量参差（偏移写错的
//! 不少见）。我们不解释它：捕获时把 "HTML Format" 的全局内存**原样拷贝**落盘
//! （`texts/<内容哈希>.html`），条目只留路径；粘贴时把这串字节**原样** `SetClipboardData`
//! 回同名注册格式。读端是目标程序，它认得自家格式 —— 我们做纯搬运工，永不失真。
//!
//! - 条目内存零负担（大 HTML 不进内存，只在盘上）；
//! - 超过 [`MAX_HTML_BYTES`] 不存（粘贴回落纯文本，功能不丢）；
//! - 清理随条目走（淘汰 / 单删都删 `.html`，「清空」整目录删自动覆盖）。

use std::sync::OnceLock;

use windows::core::PCWSTR;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GHND};

/// 与 `clipboard.rs` / `clipboard_history.rs` 同款本地钉住（CF_UNICODETEXT=13，
/// 为一个常量开一整个 `Win32_System_Ole` feature 不值）。
const CF_UNICODETEXT: u32 = 13;

const HTML_FMT_NAME: &str = "HTML Format";
const SELF_SHIELD_NAME: &str = "CurSeen.ClipboardSelf";

/// 单条 CF_HTML 落盘上限（字节）。Office 带内联图片的富文本可达数十 MB，
/// 超限说明它本质是"一张贴着文字的图"，保真收益趋零 —— 不存，粘贴回落纯文本。
pub const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;

/// "HTML Format" 注册格式号（进程内恒定，懒注册一次）。
pub fn html_format() -> u32 {
    static FMT: OnceLock<u32> = OnceLock::new();
    *FMT.get_or_init(|| {
        let wide: Vec<u16> = HTML_FMT_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) }
    })
}

/// RAII：离开作用域时 GlobalUnlock。
struct Unlocker(HGLOBAL);
impl Drop for Unlocker {
    fn drop(&mut self) {
        unsafe {
            let _ = GlobalUnlock(self.0);
        }
    }
}

/// "HTML Format" 现在可用吗（不打开剪贴板的廉价探测，捕获过滤用）。
pub fn available() -> bool {
    unsafe { IsClipboardFormatAvailable(html_format()).is_ok() }
}

/// 读取 "HTML Format" 的**原始字节**（含 CF_HTML 头）。失败一律 `None`
/// （不可用 / 被占用 / 锁不上都是常态，调用方回落纯文本）。
pub fn read() -> Option<Vec<u8>> {
    // ★ 走进程级门禁：与 `clip-thumb` 的读、截图模块的写共用同一把锁（2026-10-13）。
    crate::platform::clipboard::with_clipboard(|| unsafe {
        let fmt = html_format();
        if fmt == 0 || IsClipboardFormatAvailable(fmt).is_err() {
            return None;
        }
        let h = GetClipboardData(fmt).ok()?;
        let hg = HGLOBAL(h.0);
        let ptr = GlobalLock(hg);
        if ptr.is_null() {
            return None;
        }
        let _guard_mem = Unlocker(hg);
        let size = GlobalSize(hg) as usize;
        if size == 0 {
            return None;
        }
        let raw = std::slice::from_raw_parts(ptr.cast::<u8>(), size);
        // ★ 全局块按分配粒度向上取整，尾部可能有一串 0 填充 —— 它不属于内容，
        //   存下来粘贴回也无害，但会让"同内容再复制"的哈希对不上。截到双 NUL 后。
        let end = raw
            .windows(2)
            .position(|w| w == [0, 0])
            .map(|p| p + 2)
            .unwrap_or(size);
        Some(raw[..end].to_vec())
    })
}

/// 头部最小合理性：真 CF_HTML 以 `Version:` 起头且带 `StartHTML:` 偏移声明。
/// 只做安检（拦"同名格式的垃圾"），**不解析偏移** —— 我们不解释内容。
pub fn plausible(blob: &[u8]) -> bool {
    blob.len() > 32 && blob.starts_with(b"Version:") && blob.windows(10).any(|w| w == b"StartHTML:")
}

/// 写一条命名格式（空 DWORD）标记"这是 CurSeen 自己写回的"，捕获端见之即跳过。
/// ★ 调用前提：剪贴板**已打开**（与 `clipboard_history::set_self_shield` 同纪律）。
unsafe fn set_self_shield_opened() {
    let wide: Vec<u16> = SELF_SHIELD_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let fmt = RegisterClipboardFormatW(PCWSTR(wide.as_ptr()));
    if fmt == 0 {
        return;
    }
    let hg = match GlobalAlloc(GHND, 4) {
        Ok(h) => h,
        Err(_) => return,
    };
    let p = GlobalLock(hg);
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    *(p.cast::<u32>()) = 0;
    let _ = GlobalUnlock(hg);
    let _ = SetClipboardData(fmt, Some(HANDLE(hg.0)));
}

/// 写回：纯文本（CF_UNICODETEXT 兜底）+ 可选富文本（"HTML Format"）。
///
/// ★ 文本必须写：只写 HTML 的剪贴板在目标程序不支持 HTML 时（老编辑器 / 终端）
/// 粘出来是空 —— 双格式是保真的前提，不是锦上添花。写完置自写屏蔽，本次写回
/// 触发的更新不会被自己的历史收录。
///
/// # Safety
/// 操作全局剪贴板；须在非 UI 阻塞场景调用（与旧 `write_text` 同一纪律）。
pub unsafe fn write_text_and_html(text: &str, html: Option<&[u8]>) {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let hg = match GlobalAlloc(GHND, utf16.len() * 2) {
        Ok(h) => h,
        Err(_) => return,
    };
    let p = GlobalLock(hg);
    if p.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    // SAFETY: `p` 是 `GlobalLock(hg)` 成功返回的非空指针，可写 `GlobalAlloc(GHND,
    // utf16.len() * 2)` 个字节，正好等于源的长度（u16 = 2 字节）；源是本地 `Vec`，
    // 目的是全局堆块，**不重叠**。写入后立刻 `GlobalUnlock`。
    std::ptr::copy_nonoverlapping(utf16.as_ptr(), p.cast::<u16>(), utf16.len());
    let _ = GlobalUnlock(hg);

    // HTML 一份独立全局块：SetClipboardData 交出所有权，失败要自己回收。
    let mut html_hg: Option<HGLOBAL> = None;
    if let Some(blob) = html {
        if let Ok(h) = GlobalAlloc(GHND, blob.len()) {
            let hp = GlobalLock(h);
            if hp.is_null() {
                let _ = GlobalFree(Some(h));
            } else {
                // SAFETY: `hp` 是 `GlobalLock(h)` 成功返回的非空指针，块大小正是
                // `GlobalAlloc(GHND, blob.len())`，与源长度逐字节相等；源是调用方的
                // 切片、目的是全局堆块，**不重叠**；写完立刻 `GlobalUnlock`。
                std::ptr::copy_nonoverlapping(blob.as_ptr(), hp.cast::<u8>(), blob.len());
                let _ = GlobalUnlock(h);
                html_hg = Some(h);
            }
        }
    }

    // ★ 走进程级门禁（2026-10-13）：`EmptyClipboard()` 会释放别人正在读的那块内存。
    let opened = crate::platform::clipboard::with_clipboard(|| unsafe {
        let _ = EmptyClipboard();
        let _ = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(hg.0)));
        if let Some(h) = html_hg.take() {
            // 写失败（格式被拒等）自己回收；写成功所有权归剪贴板，不许再碰。
            if SetClipboardData(html_format(), Some(HANDLE(h.0))).is_err() {
                let _ = GlobalFree(Some(h));
            }
        }
        set_self_shield_opened();
        Some(())
    })
    .is_some();
    if !opened {
        let _ = GlobalFree(Some(hg));
        if let Some(h) = html_hg {
            let _ = GlobalFree(Some(h));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 合法头（浏览器同款骨架）必须通过。
    #[test]
    fn plausible_accepts_real_header() {
        let blob = b"Version:0.9\r\nStartHTML:0000000105\r\nEndHTML:0000000192\r\n\
StartFragment:0000000141\r\nEndFragment:0000000178\r\n<html><body><!--StartFragment-->\
<b>hi</b><!--EndFragment--></body></html>";
        assert!(
            plausible(blob),
            "浏览器同款 CF_HTML 头应被判定为合理（{} 字节）",
            blob.len()
        );
    }

    /// 垃圾 / 过短 / 缺偏移声明必须被拦。
    #[test]
    fn plausible_rejects_garbage() {
        assert!(!plausible(b"hello"), "无头纯文本不该混进富文本通道");
        assert!(
            !plausible(b"Version:0.9"),
            "只有 Version 没有 StartHTML 也该拦（11 字节）"
        );
        assert!(!plausible(&[]), "空字节串该拦");
    }

    /// 端到端：写"文本+HTML"→ 读回 HTML 字节应与写入一致（剪贴板被占则跳过，不算红）。
    #[test]
    fn html_roundtrips_through_clipboard() {
        let text = "加粗正文";
        let html = test_blob("<b>加粗正文</b>");
        unsafe { write_text_and_html(text, Some(&html)) };
        match read() {
            Some(b) => assert_eq!(b, html, "写进的 CF_HTML 字节必须原样读回（{text}）"),
            // 剪贴板被别的进程握着（CI / 杀软监听）是常态：跳过不判红。
            None => println!("剪贴板不可用，端到端用例跳过（不算失败）"),
        }
    }

    /// 构造一份头格式与真实 CF_HTML 一致的测试 blob（本用例只验往返，不验偏移语义）。
    fn test_blob(fragment: &str) -> Vec<u8> {
        let body =
            format!("<html><body><!--StartFragment-->{fragment}<!--EndFragment--></body></html>");
        let header_len = 93; // 各偏移字段定宽后的头长
        let start_html = header_len;
        let end_html = header_len + body.len();
        format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\n\
StartFragment:{start_html:010}\r\nEndFragment:{end_html:010}\r\n{body}"
        )
        .into_bytes()
    }
}
