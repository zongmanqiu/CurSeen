//! probe-group: fast
//! 便签「调透明度时，到底是真透明还是只是变灰」的真机探针（`#[ignore]`，要真实桌面会话）。
//!
//! # 它要回答的问题
//!
//! 用户报：「便签透明度改变时不对劲。并不是真的透明，而是有点灰色。」
//! 开发者在自己机器上看不出问题。两边说法要**用像素**裁决，不能靠印象。
//!
//! # 判据
//!
//! 把便签挪到屏幕上**两块底色不同且各自足够纯**的地方，各截一次屏，量「标题栏」与
//! 「正文」两条带的众数色 `P`（底色 `B` 取自**同一张截图**里便签四周那一圈）：
//!
//! | 真实情况 | 换地方后 `P` | `P` 与 `a*C + (1-a)*B` |
//! |---|---|---|
//! | **真半透明** | **跟着底色变** | 吻合（容差 8/255） |
//! | **不透明（只是颜色发灰）** | **逐字节不变** | 不吻合（实测恒等于层实色 `C`） |
//!
//! 决定性的一条：**"换了底色像素不变" 就是"根本没透"**，与 alpha 设成多少无关。
//!
//! # 底色怎么量（这一段是探针的全部难点，踩过三次坑，别改回去）
//!
//! 底色必须和便签**同一张截图**、取自便签**四周那一圈**（`RING`），并且要求那一圈的
//! **众数占比 ≥ 70%**——只有底色够纯，"圈里的颜色"才等于"便签底下的颜色"。
//! 屏幕上纯色块不多，所以先扫 `CANDIDATES` 这些候选位置，挑两块"最纯且颜色差最大"的。
//!
//! 试过并**否决**的三种做法（别再捡）：
//!   · 先把便签挪开量一次底色、再挪回来量一次：**两次之间屏幕内容会变**，实测拿到过
//!     [208,213,211] 这种跟真实底色无关的数，于是算出"标题栏没透"的假结论。
//!   · 自建纯色背板窗：普通窗压在所有已开窗口下面；做成 topmost 也压不住**别的最顶层窗**
//!     （实测：全屏撒 5 个点，只有 1 个点是背板色，其余是常驻置顶程序的界面）。
//!   · 直接刷桌面画布（`GetDC(NULL)` + `FillRect`）：**慢一帧**，这次截到的是上一次的颜色。
//!
//! 跑法（**先退掉常驻的 curseen.exe**）：`python build/probe.py run note_translucency_probe`

use std::collections::HashMap;
use std::time::{Duration, Instant};

use curseen_ui::platform::sticky::{layered_alpha, NoteCfg, StickyManager, BODY_CLASS, CLASS_NAME};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
    SelectObject, CAPTUREBLT, DIB_RGB_COLORS, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetSystemMetrics, GetWindow, GetWindowRect,
    MoveWindow, PeekMessageW, TranslateMessage, GW_OWNER, MSG, PM_REMOVE, SM_CXSCREEN, SM_CYSCREEN,
};

const VIEW_W: i32 = 360;
const VIEW_H: i32 = 260;
/// 便签四周多截这么宽的一圈，用来取底色。
const RING: i32 = 44;
/// 候选取景位置（扫描用，挑两块最纯且颜色差最大的）。
const CANDIDATES: [(i32, i32); 8] = [
    (160, 120),
    (620, 120),
    (1080, 120),
    (1500, 120),
    (160, 480),
    (620, 480),
    (1080, 480),
    (1500, 480),
];
/// 底色纯度门槛（达不到 = 这块地方是花屏，判据不可信）。
const MIN_PURITY: u32 = 70;

// ── 抓屏 ────────────────────────────────────────────────────────────────

struct Shot {
    w: i32,
    h: i32,
    rgb: Vec<u8>, // 每像素 3 字节（R,G,B），行序自上而下
}

/// 把屏幕上 `(x,y,w,h)` 取回来。`CAPTUREBLT` 必带：否则分层窗（便签就是）截不到。
fn grab(x: i32, y: i32, w: i32, h: i32) -> Shot {
    use windows::Win32::Graphics::Gdi::{BITMAPINFO, BITMAPINFOHEADER, BI_RGB};
    unsafe {
        let screen: HDC = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // 负 = 自上而下
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = CreateDIBSection(Some(screen), &bi, DIB_RGB_COLORS, &mut bits, None, 0)
            .expect("CreateDIBSection 失败");
        let old = SelectObject(mem, HGDIOBJ(bmp.0));
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), x, y, SRCCOPY | CAPTUREBLT);
        // ★ 先把像素拷出来再删位图：`bits` 就是这张 DIB 的内存。
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        if ok.is_ok() && !bits.is_null() {
            let raw = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
            for px in raw.chunks_exact(4) {
                rgb.push(px[2]); // DIB 是 BGRA
                rgb.push(px[1]);
                rgb.push(px[0]);
            }
        }
        let _ = SelectObject(mem, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        assert!(ok.is_ok(), "BitBlt 失败");
        Shot { w, h, rgb }
    }
}

impl Shot {
    fn at(&self, x: i32, y: i32) -> [u8; 3] {
        let i = ((y * self.w + x) * 3) as usize;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    /// 区域的众数色、命中数、总数。
    fn mode_pct(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> ([u8; 3], u32, u32) {
        let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
        let mut total = 0u32;
        for y in y0.max(0)..y1.min(self.h) {
            for x in x0.max(0)..x1.min(self.w) {
                *counts.entry(self.at(x, y)).or_default() += 1;
                total += 1;
            }
        }
        let (c, n) = counts.into_iter().max_by_key(|(_, n)| *n).unwrap_or_default();
        (c, n, total)
    }
}

// ── 便签 ────────────────────────────────────────────────────────────────

unsafe extern "system" fn enum_cb(h: HWND, l: LPARAM) -> BOOL {
    let out = &mut *(l.0 as *mut Vec<(HWND, String)>);
    let mut buf = [0u16; 64];
    let n = GetClassNameW(h, &mut buf);
    let name = String::from_utf16_lossy(&buf[..n as usize]);
    if name == CLASS_NAME || name == BODY_CLASS {
        out.push((h, name));
    }
    BOOL(1)
}

fn find_pair() -> Option<(HWND, HWND)> {
    let mut found: Vec<(HWND, String)> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut found as *mut _ as isize));
    }
    for (body, cls) in &found {
        if cls != BODY_CLASS {
            continue;
        }
        let owner = unsafe { GetWindow(*body, GW_OWNER) }.unwrap_or(HWND(std::ptr::null_mut()));
        if !owner.0.is_null() && found.iter().any(|(f, c)| f.0 == owner.0 && c == CLASS_NAME) {
            return Some((owner, *body));
        }
    }
    None
}

fn rect_of(h: HWND) -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = GetWindowRect(h, &mut r);
    }
    r
}

fn pump(ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let mut msg = MSG::default();
        unsafe {
            if PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// 一个取景点的全部数据：位置、底色、底色纯度、标题栏实测、正文实测。
struct Sample {
    pos: (i32, i32),
    bg: [u8; 3],
    purity: u32,
    title: [u8; 3],
    body: [u8; 3],
}

/// 把便签挪到 `(vx,vy)` 截一次屏：**底色与便签像素取自同一帧**。
fn sample_at(frame: HWND, body: HWND, vx: i32, vy: i32) -> Sample {
    unsafe {
        let _ = MoveWindow(frame, vx, vy, VIEW_W, VIEW_H, true);
    }
    pump(320);
    let fr = rect_of(frame);
    let br = rect_of(body);
    let shot = grab(vx - RING, vy - RING, VIEW_W + 2 * RING, VIEW_H + 2 * RING);
    // 便签在截图里的左上角（挪动未必精确到像素，按实际 rect 校正）。
    let nx = fr.left - (vx - RING);
    let ny = fr.top - (vy - RING);

    // ① 底色 = 那一圈的众数色，纯度 = 它的占比。
    let mut counts: HashMap<[u8; 3], u32> = HashMap::new();
    let mut ring_total = 0u32;
    for y in 0..shot.h {
        for x in 0..shot.w {
            if x >= nx && x < nx + VIEW_W && y >= ny && y < ny + VIEW_H {
                continue; // 便签本体，不算底色
            }
            *counts.entry(shot.at(x, y)).or_default() += 1;
            ring_total += 1;
        }
    }
    let (bg, bg_n) = counts.into_iter().max_by_key(|(_, n)| *n).unwrap_or_default();
    let purity = bg_n * 100 / ring_total.max(1);

    // ② 两条取样带（截图坐标 = 窗内偏移 + 便签在截图里的左上角）。
    let dy = br.top - fr.top;
    let (title, _, _) = shot.mode_pct(nx + 8, ny + 3, nx + VIEW_W - 8, ny + dy.max(4) - 2);
    let (bod, _, _) = shot.mode_pct(
        nx + 8,
        ny + dy + 4,
        nx + VIEW_W - 8,
        ny + (br.bottom - fr.top).min(VIEW_H) - 4,
    );
    Sample {
        pos: (vx, vy),
        bg,
        purity,
        title,
        body: bod,
    }
}

/// 理论合成值：`a*C + (1-a)*B`。
fn expect(color: [u8; 4], total: u32, bg: [u8; 3]) -> [u8; 3] {
    let a = layered_alpha(total, color[3]) as f32 / 255.0;
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = (color[i] as f32 * a + bg[i] as f32 * (1.0 - a)).round() as u8;
    }
    out
}

fn dist(a: [u8; 3], b: [u8; 3]) -> i32 {
    (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).max().unwrap()
}

fn screen_ok() -> bool {
    unsafe {
        GetSystemMetrics(SM_CXSCREEN) >= 1500 + VIEW_W + RING
            && GetSystemMetrics(SM_CYSCREEN) >= 480 + VIEW_H + RING
    }
}

/// 扫所有候选位置，返回"够纯"的那些（按纯度降序）。
fn scan(frame: HWND, body: HWND) -> Vec<Sample> {
    let mut out = Vec::new();
    for (vx, vy) in CANDIDATES {
        let s = sample_at(frame, body, vx, vy);
        println!(
            "    候选 ({vx},{vy})：底色 {:?} 纯度 {}% → 标题栏 {:?} 正文 {:?}",
            s.bg, s.purity, s.title, s.body
        );
        if s.purity >= MIN_PURITY {
            out.push(s);
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.purity));
    out
}

/// ★ 核心判据：**换底色，像素必须跟着变**。不变 = 根本没透。
#[test]
#[ignore]
fn lowering_opacity_really_lets_the_backdrop_through() {
    assert!(screen_ok(), "屏幕放不下候选取景框，改小 CANDIDATES 再跑");
    let base = NoteCfg::default();

    for total in [100u32, 40u32] {
        let cfg = NoteCfg {
            alpha: total,
            ..base.clone()
        };
        let mut mgr = StickyManager::new();
        mgr.ensure(&cfg);
        mgr.spawn(&cfg);
        let deadline = Instant::now() + Duration::from_secs(5);
        let (frame, body) = loop {
            if let Some(p) = find_pair() {
                break p;
            }
            if Instant::now() > deadline {
                panic!("5 秒内没建出便签窗（桌面会话不可用？）");
            }
            pump(20);
        };
        println!("\n=== 总不透明度 {total}%：扫取景位置 ===");
        let pure = scan(frame, body);
        drop(mgr);
        pump(120);

        // ⓪ 前提：底色要够纯，而且**和便签自己的颜色差得够远**——
        //   两者越接近，"透没透"在像素上越看不出来（判据就失效了）。
        let sa = pure.first().unwrap_or_else(|| {
            panic!(
                "没有一个够纯的取景位置（纯度门槛 {MIN_PURITY}%）——桌面太花了，\
                 关掉几个窗口或换个纯色壁纸再跑"
            )
        });
        assert!(
            dist(sa.bg, [cfg.bg[0], cfg.bg[1], cfg.bg[2]]) >= 60
                && dist(sa.bg, [cfg.edit[0], cfg.edit[1], cfg.edit[2]]) >= 60,
            "底色 {:?} 和便签自己的颜色（背景 {:?} / 编辑区 {:?}）太接近 —— 判不出透没透，\
             换个位置再跑",
            sa.bg,
            &cfg.bg[..3],
            &cfg.edit[..3]
        );
        // 有第二块颜色不同的就顺带做差分（不是必需的，有了更硬）。
        let sb = pure.iter().find(|s| dist(s.bg, sa.bg) >= 60);

        let a_title = layered_alpha(total, cfg.bg[3]);
        let a_body = layered_alpha(total, cfg.edit[3]);
        println!("\n=== 总不透明度 {total}%（层 alpha：标题栏 {a_title}，正文 {a_body}）===");
        println!(
            "  取景 A {:?}：底色 {:?}（纯度 {}%）→ 标题栏 {:?}（理论 {:?}） 正文 {:?}（理论 {:?}）",
            sa.pos,
            sa.bg,
            sa.purity,
            sa.title,
            expect(cfg.bg, total, sa.bg),
            sa.body,
            expect(cfg.edit, total, sa.bg)
        );
        if let Some(sb) = sb {
            println!(
                "  取景 B {:?}：底色 {:?}（纯度 {}%）→ 标题栏 {:?}（理论 {:?}） 正文 {:?}（理论 {:?}）",
                sb.pos,
                sb.bg,
                sb.purity,
                sb.title,
                expect(cfg.bg, total, sb.bg),
                sb.body,
                expect(cfg.edit, total, sb.bg)
            );
            println!(
                "  ★ 换底色后像素差：标题栏 {} 正文 {}（底色本身差 {}）",
                dist(sa.title, sb.title),
                dist(sa.body, sb.body),
                dist(sa.bg, sb.bg)
            );
        }

        // ① ★ 决定性：实测必须等于 `a*C + (1-a)*B`。**不透明的话实测会恒等于 C**，
        //    两者相差 `(1-a)*|B-C|`——底色挑得离便签色够远时，这是个一眼能看出的差距。
        let want_t = expect(cfg.bg, total, sa.bg);
        let want_b = expect(cfg.edit, total, sa.bg);
        let opaque_gap_b = dist([cfg.edit[0], cfg.edit[1], cfg.edit[2]], want_b);
        println!(
            "  判据分辨力：若不透明，实测与理论会差 {}（正文）/ {}（标题栏）",
            opaque_gap_b,
            dist([cfg.bg[0], cfg.bg[1], cfg.bg[2]], want_t)
        );
        assert!(
            dist(sa.body, want_b) <= 8,
            "★ 正文实测 {:?} 不等于理论合成值 {:?}（差 {}）。若它是不透明的实色，\
             应该等于 {:?}——到底是哪一种，看这两个数谁更接近",
            sa.body,
            want_b,
            dist(sa.body, want_b),
            &cfg.edit[..3]
        );
        assert!(
            dist(sa.title, want_t) <= 8,
            "★ 标题栏实测 {:?} 不等于理论合成值 {:?}（差 {}）。若它是不透明的实色，\
             应该等于 {:?}",
            sa.title,
            want_t,
            dist(sa.title, want_t),
            &cfg.bg[..3]
        );

        // ② 有第二块底色就顺带做差分（更硬的一条）。
        if let Some(sb) = sb {
            let leak = 255 - layered_alpha(total, cfg.edit[3]) as i32;
            let min_gap = (dist(sa.bg, sb.bg) * leak / 255).max(2) - 2;
            assert!(
                dist(sa.body, sb.body) >= min_gap && dist(sa.title, sb.title) >= min_gap,
                "★ 换底色后像素几乎不变（正文差 {}、标题栏差 {}，至少该有 {}）——根本没透",
                dist(sa.body, sb.body),
                dist(sa.title, sb.title),
                min_gap
            );
        }
    }
}

/// 探针对账：连抓两次同一块区域必须一致（否则上面的数都是噪声）。
#[test]
#[ignore]
fn grab_is_deterministic() {
    let a = grab(700, 700, 64, 64);
    let b = grab(700, 700, 64, 64);
    let mut same = 0;
    for i in 0..a.rgb.len() {
        if a.rgb[i] == b.rgb[i] {
            same += 1;
        }
    }
    let pct = same * 100 / a.rgb.len().max(1);
    println!("连抓两次一致率 {pct}%（样点 {:?} / {:?}）", a.at(32, 32), b.at(32, 32));
    assert!(pct >= 95, "抓屏不稳定（一致率 {pct}%）——上面那些数不能信");
}
