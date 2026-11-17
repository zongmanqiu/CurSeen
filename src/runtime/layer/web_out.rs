//! 浏览器源输出（m83）：把虚拟键盘推给直播软件的「浏览器源」。
//!
//! ## 为什么是它
//!
//! 用户的目标是「桌面上完全看不见键盘，但直播画面里有」。窗口做不到（屏外/隐藏/最小化/
//! `WDA_EXCLUDEFROMCAPTURE` 四条全部真机判废）；虚拟摄像头这条，直播姬的摄像头列表
//! **不认 DShow 设备**（2026-11-17 实测）。而直播姬/OBS 都带 CEF 的**浏览器源**：它在后台
//! 离屏渲染，桌面上不出窗口，而且**支持透明**（不需要色键）。
//!
//! ## 形状
//!
//! 进程内起一个只监听 `127.0.0.1` 的 HTTP 服务：
//!
//! - `GET /`        → 内置页面（canvas + `fetch` 流）
//! - `GET /frames`  → 先发 8 字节头（`u32 w` / `u32 h`，小端），随后是定长原始 RGBA 帧
//!   （straight alpha：`DibTarget` 出来是预乘的，[`publish_from`] 统一反预乘一次）。
//!
//! **不做 JPEG/PNG 编码**：本地回环带宽白送（742×198×4 ≈ 588 KB/帧 @30fps ≈ 17.6 MB/s），
//! 换来的是零编解码开销 + 像素级透明，不引入任何新依赖。
//!
//! ## 帧从哪来
//!
//! 不在这里画键帽：`live_draw_list` 由覆盖层的帧引擎（`frame.rs::render`）算好，本模块只在
//! 「直播帧脏标记被排空」的那一拍把它渲成位图（与合成窗同一条 `translate_offset` 路径）。
//! 静止时不出帧——与「静止零功耗」同一条口径。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::runtime::Runtime;

const PAGE: &str = include_str!("web_out.html");

struct Frame {
    w: u32,
    h: u32,
    /// 定长 `w*h*4`，straight-alpha RGBA。
    data: Vec<u8>,
}

struct Inner {
    frame: Option<Arc<Frame>>,
    seq: u64,
    closed: bool,
}

/// 最新一帧的共享槽：渲染线程 `publish`，HTTP 线程 `wait_new`。
pub(crate) struct FrameHub {
    inner: Mutex<Inner>,
    cv: Condvar,
}

impl FrameHub {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                frame: None,
                seq: 0,
                closed: true,
            }),
            cv: Condvar::new(),
        })
    }

    fn publish(&self, w: u32, h: u32, data: Vec<u8>) {
        if data.len() != (w as usize) * (h as usize) * 4 {
            return;
        }
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.frame = Some(Arc::new(Frame { w, h, data }));
        g.seq = g.seq.wrapping_add(1);
        g.closed = false;
        self.cv.notify_all();
    }

    /// 停掉输出：等着的连接会收到 `closed` 并断开（页面的重连循环会一直等，直到重新开）。
    fn clear(&self) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.frame = None;
        g.seq = g.seq.wrapping_add(1);
        g.closed = true;
        self.cv.notify_all();
    }

    pub(crate) fn has_frame(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame
            .is_some()
    }

    fn is_closed(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    /// 等一帧比 `last` 新的；超时返回 `None`（调用方继续等，别把页面踢掉）。
    fn wait_new(&self, last: u64, timeout: Duration) -> Option<(u64, Arc<Frame>)> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let deadline = Instant::now() + timeout;
        loop {
            if g.seq != last {
                return g.frame.clone().map(|f| (g.seq, f));
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            g = self
                .cv
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

/// 运行中的本地服务。`Drop` = 停 accept + 叫醒 + 收线程。
pub(crate) struct Server {
    /// **优先**端口（= 配置里的值）：对账拿它比，看不懂回退后的实际端口。
    desired: u16,
    /// 实际绑定成功的端口（首选被占时顺延）。
    port: u16,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

/// 首选端口被占时向后顺延的个数（含首选共 10 个候选）。
///
/// ★ 普适性：别人的电脑上 8765 很可能早被别的程序占着（开发工具、其它直播脚本……）。
///   一个固定端口 + 无声失败 = 新用户第一次用就是空白画面。顺延后把**实际**地址复制给
///   用户，功能就自愈了。
const PORT_FALLBACK_TRIES: u16 = 10;

impl Server {
    pub(crate) fn start(desired: u16, hub: Arc<FrameHub>) -> std::io::Result<Server> {
        // 首选 → 顺延。只绑定 127.0.0.1；端口全部被占才报错（上层 3 秒后重试）。
        let mut listener = None;
        let mut actual = desired;
        for off in 0..PORT_FALLBACK_TRIES {
            let p = desired.saturating_add(off);
            if p == 0 {
                continue;
            }
            match TcpListener::bind(("127.0.0.1", p)) {
                Ok(l) => {
                    actual = p;
                    listener = Some(l);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
                Err(e) => return Err(e),
            }
        }
        let listener = listener.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!("127.0.0.1:{desired} 起连续 {PORT_FALLBACK_TRIES} 个端口都被占用"),
            )
        })?;
        if actual != desired {
            log::warn!(
                "浏览器源：端口 {desired} 被占用，改用 {actual}（面板「复制地址」给的是后者）"
            );
        }
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let join = std::thread::Builder::new()
            .name("curseen-web-out".into())
            .spawn(move || accept_loop(listener, hub, st))?;
        Ok(Server {
            desired,
            port: actual,
            stop,
            join: Some(join),
        })
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// 拿配置里的**优先**端口对账：回退后的实际端口不参与"是否要重启"的判断。
    pub(crate) fn desired(&self) -> u16 {
        self.desired
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // 叫醒阻塞在 accept 上的线程（非阻塞轮询下这只是一次普通连接，随后自己退出）。
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

fn accept_loop(listener: TcpListener, hub: Arc<FrameHub>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((sock, _)) => {
                let hub = hub.clone();
                std::thread::spawn(move || handle(sock, hub));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => break,
        }
    }
}

fn handle(mut sock: TcpStream, hub: Arc<FrameHub>) {
    let _ = sock.set_nodelay(true);
    let mut buf = [0u8; 4096];
    let mut n = 0usize;
    // 只读到请求头结束：GET 请求没有 body。
    while n < buf.len() {
        match sock.read(&mut buf[n..]) {
            Ok(0) => return,
            Ok(k) => {
                n += k;
                if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => return,
        }
    }
    let head = String::from_utf8_lossy(&buf[..n]);
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .unwrap_or("/");

    if path.starts_with("/frames") {
        serve_frames(sock, hub);
    } else if path == "/" || path.starts_with("/?") {
        let mut head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
             Cache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            PAGE.len()
        )
        .into_bytes();
        head.extend_from_slice(PAGE.as_bytes());
        let _ = sock.write_all(&head);
    } else {
        let _ = sock
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
}

fn serve_frames(mut sock: TcpStream, hub: Arc<FrameHub>) {
    let mut last = 0u64;
    let mut size: Option<(u32, u32)> = None;
    loop {
        let Some((seq, frame)) = hub.wait_new(last, Duration::from_millis(400)) else {
            // 超时：直播页还开着但键盘静止（正常）。关掉输出时要主动断开。
            if hub.is_closed() {
                return;
            }
            // 还没出过帧（刚开）→ 继续等；出过帧 → 继续挂着等下一帧。
            continue;
        };
        if size.is_none() {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                        Cache-Control: no-store\r\nConnection: close\r\n\
                        Transfer-Encoding: chunked\r\n\r\n";
            let hdr = [(frame.w).to_le_bytes(), (frame.h).to_le_bytes()].concat();
            if sock.write_all(head.as_bytes()).is_err() || write_chunk(&mut sock, &hdr).is_err() {
                return;
            }
            size = Some((frame.w, frame.h));
        } else if size != Some((frame.w, frame.h)) {
            // 键盘换了方案/缩放：尺寸变了。断开让页面重连，用新尺寸重新开流
            // （浏览器 canvas 的尺寸只在连接建立时定一次）。
            return;
        }
        if write_chunk(&mut sock, &frame.data).is_err() {
            return;
        }
        last = seq;
    }
}

fn write_chunk(sock: &mut TcpStream, data: &[u8]) -> std::io::Result<()> {
    sock.write_all(format!("{:X}\r\n", data.len()).as_bytes())?;
    sock.write_all(data)?;
    sock.write_all(b"\r\n")
}

/// 把预乘 RGBA 反成 straight RGBA（浏览器 `ImageData` 要后者）。
fn unpremultiply(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else if a < 255 {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// 输出倍率：2×（物理像素）——直播画面里往往要放大 2 倍以上，1× 放大会糊。
/// 代价：缓冲区 4 倍（742×198 → 1484×396 ≈ 2.35 MB/帧），本机回环扛得住。
const OUT_SCALE: f32 = 2.0;

/// 当前键盘该输出多大的帧（像素）。
///
/// ★ 唯一来源：键帽包围盒 × [`OUT_SCALE`]——换方案、改键帽尺寸都会得到不同的值。
///   这条尺寸同时是**面板上「来源尺寸」那一行**给的数字（直播软件里浏览器源的宽高照填，
///   键盘正好铺满），所以发布路径与展示路径必须同源，不许各算一遍。
pub(crate) fn output_size(
    vk: &crate::modules::virtual_keyboard::VirtualKeyboardModule,
    cfg: &crate::core::config::VirtualKeyboardSettings,
) -> Option<(u32, u32)> {
    let (_, _, bw, bh) = vk.layout_world_bounds(cfg)?;
    Some(frame_px(bw, bh))
}

/// 包围盒（世界物理像素）→ 帧像素。发布与展示共用这一条换算。
fn frame_px(bw: f32, bh: f32) -> (u32, u32) {
    (
        (bw * OUT_SCALE).round().max(1.0) as u32,
        (bh * OUT_SCALE).round().max(1.0) as u32,
    )
}

/// 把一份 `DrawList` 按「世界物理坐标 − 包围盒原点」渲进 RGBA 缓冲，输出 2×。
///
/// ★ 倍率有**两个来源**，必须同时给对（B16/B18 两次真机各踩一半）：
///   - 图元（键帽）：画布 `make_canvas(.., OUT_SCALE)`；
///   - **文字**：引擎 `set_scale(OUT_SCALE)`——画布的 scale 不作用到文字，漏了这一行
///     就是"键帽放大了、字还是原大小"，字还钉在键帽左上角，看起来像整体偏移
///     （2026-11-17 真机第二次反馈）。`translate_offset` 保持 1:1，坐标都交给这两处。
///   - 少给 = 缩在角落/字不放大；两边都乘 = 4×。
fn render_into(
    list: &crate::core::draw::DrawList,
    engine: &mut windui::text::DWriteEngine,
    buf: &mut [u8],
    w: u32,
    h: u32,
    bx: f32,
    by: f32,
) {
    use windui::render::{DibTarget, RenderTarget};
    let mut target = DibTarget::new(buf, w, h);
    use windui::text::TextEngine as _;
    engine.set_scale(OUT_SCALE);
    let mut canvas = target.make_canvas(engine, OUT_SCALE);
    crate::runtime::render::translate_offset(list, &mut *canvas, 1.0, -bx, -by);
}

/// 把当前 `live_draw_list` 渲成带 alpha 的位图并发布。
///
/// ★ 尺寸/原点与合成窗同一来源（`vk.layout_world_bounds`），只是这次底色不画——
///   键帽以外 alpha=0，直播软件里透出场景背景，不需要色键。
///
/// 返回 `Some((w,h))` 当且仅当**帧尺寸刚刚变过**（调用方拿它回灌面板的「来源尺寸」；
/// 尺寸没变时不重复刷信号）。
pub(crate) fn publish_from(rt: &Runtime) -> Option<(u32, u32)> {
    let Some((bx, by, bw, bh)) = rt.vk.layout_world_bounds(&rt.config.virtual_keyboard) else {
        rt.web_frames.clear();
        return None;
    };
    let (w, h) = frame_px(bw, bh);
    let mut buf = vec![0u8; (w as usize) * (h as usize) * 4];
    {
        let mut slot = rt.web_text.borrow_mut();
        let engine = slot.get_or_insert_with(windui::text::DWriteEngine::new);
        render_into(&rt.live_draw_list, engine, &mut buf, w, h, bx, by);
    }
    unpremultiply(&mut buf);
    let changed = rt.web_last_size.get() != (w, h);
    if changed {
        rt.web_last_size.set((w, h));
        log::info!("浏览器源：发布 {}x{} 帧", w, h);
    }
    rt.web_frames.publish(w, h, buf);
    changed.then_some((w, h))
}

/// 开/关/换端口对账（挂在覆盖层的 33ms 轮询上，与直播窗同一个节拍）。
///
/// 绑定失败（端口被占）不是致命的：记一次日志，3 秒后再试；期间不影响其它功能。
pub(crate) fn sync(rt: &std::rc::Rc<std::cell::RefCell<Runtime>>) {
    let (want, port) = {
        let r = rt.borrow();
        (r.config.live.browser_active(), r.config.live.browser_port)
    };
    let mut r = rt.borrow_mut();
    if let Some(srv) = r.web_server.as_ref() {
        if want && srv.desired() == port {
            return;
        }
        r.web_server = None; // Drop：停 accept、断流
        r.web_frames.clear();
        let url = r.config.live.browser_url();
        r.panel_set_live_url(url);
    }
    if !want {
        return;
    }
    let now = Instant::now();
    if let Some(next) = r.web_retry_at {
        if now < next {
            return;
        }
    }
    match Server::start(port, r.web_frames.clone()) {
        Ok(srv) => {
            let url = format!("http://127.0.0.1:{}/", srv.port());
            log::info!("浏览器源：已监听 {url}");
            r.panel_set_live_url(url);
            r.web_server = Some(srv);
            r.web_retry_at = None;
        }
        Err(e) => {
            log::warn!("浏览器源：端口 {} 绑定失败：{}（3 秒后重试）", port, e);
            r.web_retry_at = Some(now + Duration::from_secs(3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{Config, LiveMode};

    /// ★ 回归（B16，2026-11-17 真机）：推给浏览器的帧里，键帽必须**铺满整帧**。
    ///
    /// 真机现象是"键盘只占画面左上角一小块、其余透明"——根因是帧按 2× 分配、绘制却按
    /// 1× 落笔（倍率只给了一边）。这条用例量的是**内容包围盒占帧的比例**：1× 落笔时
    /// 宽高各只有一半（≈0.5），两边都乘又会画到帧外。判据取 0.95，跑得动也留得下
    /// 键帽自带的 1–2 像素余量。
    #[test]
    fn published_frame_is_filled_by_the_keyboard() {
        let mut rt = Runtime::new(Config::default());
        rt.config.virtual_keyboard.enabled = true;
        rt.config.live.mode = LiveMode::Browser;
        rt.frame(); // 解析方案（否则没有可画的内容，判据会假绿）
        rt.render();
        publish_from(&rt);
        let (_, f) = rt
            .web_frames
            .wait_new(0, Duration::from_millis(1))
            .expect("发布后应立刻能取到帧");
        let (w, h) = (f.w as usize, f.h as usize);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        for y in 0..h {
            for x in 0..w {
                if f.data[(y * w + x) * 4 + 3] != 0 {
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        let cov_w = (x1 + 1 - x0) as f32 / w as f32;
        let cov_h = (y1 + 1 - y0) as f32 / h as f32;
        assert!(
            cov_w > 0.95 && cov_h > 0.95,
            "键帽必须铺满整帧，实际覆盖率 {cov_w:.2}×{cov_h:.2}（宽×高）——\
             倍率只给画布或只给 View 一处，两边都乘/都不乘会把键盘缩在角落"
        );
    }

    /// ★ 换方案尺寸必须跟着变（B17 用户需求：款式很多，窗口/源要按键盘实际大小走）。
    /// 这条同时钉住"面板里的「来源尺寸」与发布路径同源"：两者都走 [`output_size`]。
    #[test]
    fn output_size_follows_the_keyboard_layout() {
        let mut rt = Runtime::new(Config::default());
        rt.config.virtual_keyboard.enabled = true;
        let a = output_size(&rt.vk, &rt.config.virtual_keyboard).expect("ansi_104 应有包围盒");
        rt.config.virtual_keyboard.layout = "ansi_60".into();
        let b = output_size(&rt.vk, &rt.config.virtual_keyboard).expect("ansi_60 应有包围盒");
        assert_ne!(a, b, "另一种键盘必须算出另一种输出尺寸：{a:?} vs {b:?}");
        // 尺寸不是随手写的常量：帧 = 包围盒 × 2（面板照着填宽高才能铺满）。
        let (_, _, bw, bh) = rt
            .vk
            .layout_world_bounds(&rt.config.virtual_keyboard)
            .unwrap();
        assert_eq!(b, ((bw * 2.0).round() as u32, (bh * 2.0).round() as u32));
    }

    /// ★ 回归（B18，2026-11-17 真机第二次）：文字必须和键帽一起放大到 2×。
    ///
    /// 真机现象是"键帽放大了、字还是原大小，字钉在左上角，看起来整体偏移"——画布的
    /// scale 不作用于文字，文字倍率在 `DWriteEngine::set_scale` 上，漏了那一行。
    /// 判据用**自校准比例**：同一段文字走生产路径（2×）与本地 1× 对照，墨迹高度之比
    /// 必须≈2；漏 `set_scale` 时该比值≈1（字大小没变），一眼能红。
    #[test]
    fn text_glyphs_scale_with_the_frame() {
        use crate::core::color::Argb;
        use crate::core::draw::DrawList;
        let mut list = DrawList::new();
        list.text(0.0, 0.0, "Hg", 40.0, Argb::new(255, 255, 255, 255), None);

        let (w, h) = (320u32, 160u32);
        let mut two_x = vec![0u8; (w * h * 4) as usize];
        let mut eng = windui::text::DWriteEngine::new();
        render_into(&list, &mut eng, &mut two_x, w, h, 0.0, 0.0);
        let h2 = ink_height(&two_x, w, h);

        // 1× 对照：与修复前的写法等价（画布 1×、引擎 1×）。
        let mut one_x = vec![0u8; (w * h * 4) as usize];
        {
            use windui::render::{DibTarget, RenderTarget};
            let mut eng1 = windui::text::DWriteEngine::new();
            let mut target = DibTarget::new(&mut one_x, w, h);
            let mut canvas = target.make_canvas(&mut eng1, 1.0);
            crate::runtime::render::translate_offset(&list, &mut *canvas, 1.0, 0.0, 0.0);
        }
        let h1 = ink_height(&one_x, w, h);
        assert!(
            h1 > 0 && h2 > 0,
            "两侧都要有墨迹（否则判据假绿）：1×={h1} 2×={h2}"
        );
        let ratio = h2 as f32 / h1 as f32;
        assert!(
            (1.7..=2.3).contains(&ratio),
            "文字必须随 2× 画布一起放大，实测墨迹高度比 {ratio:.2}（1×={h1}, 2×={h2}）"
        );
    }

    fn ink_height(buf: &[u8], w: u32, h: u32) -> u32 {
        let (mut y0, mut y1) = (u32::MAX, 0u32);
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                if buf[((y * w + x) * 4 + 3) as usize] > 8 {
                    any = true;
                    y0 = y0.min(y);
                    y1 = y1.max(y);
                }
            }
        }
        if any {
            y1 - y0 + 1
        } else {
            0
        }
    }

    /// 反预乘：alpha=0 → RGB 归零；alpha=255 → 原样；半透明 → 按 a 还原。
    /// ★ 为什么必须测：`DibTarget` 出的是预乘值，直接当 `ImageData` 喂给浏览器会让
    ///   半透明区域整体发暗（"键帽边缘一圈黑边"），而这种缺陷只有对着像素才看得出。
    #[test]
    fn unpremultiply_restores_straight_alpha() {
        let mut px = vec![0, 0, 0, 0, 10, 20, 30, 255, 128, 64, 32, 128];
        unpremultiply(&mut px);
        assert_eq!(&px[0..4], &[0, 0, 0, 0], "全透明像素 RGB 必须归零");
        assert_eq!(&px[4..8], &[10, 20, 30, 255], "不透明像素不许动");
        // 预乘 128/255 的 (128,64,32) 还原 = (255,128,64)（(c*255 + a/2)/a）
        assert_eq!(&px[8..12], &[255, 128, 64, 128]);
    }

    /// ★ 普适性（B19）：首选端口被别的程序占着时必须自动顺延，而不是"页面一直空白"。
    /// 判据同时钉住"对账看 desired、连接看实际端口"这两半。
    #[test]
    fn server_falls_back_when_port_is_taken() {
        let hold = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let busy = hold.local_addr().unwrap().port();
        let srv = Server::start(busy, FrameHub::new()).expect("应顺延到下一个端口");
        assert_eq!(srv.desired(), busy, "对账仍看首选端口");
        assert_ne!(srv.port(), busy, "首选被占必须换一个");
        assert!(
            std::net::TcpStream::connect(("127.0.0.1", srv.port())).is_ok(),
            "顺延后的端口必须真能连上"
        );
    }

    /// 槽语义：发布后可立刻等到；尺寸不符的帧被拒；clear 让等待方看到 closed。
    #[test]
    fn hub_publishes_frame_and_closes() {
        let hub = FrameHub::new();
        assert!(!hub.has_frame());
        hub.publish(2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(hub.has_frame());
        let (seq, f) = hub
            .wait_new(0, Duration::from_millis(1))
            .expect("应立刻拿到帧");
        assert!(seq > 0);
        assert_eq!((f.w, f.h, f.data.len()), (2, 1, 8));
        // 长度与尺寸不自洽的帧不进槽（守住"定长帧"这个协议前提）。
        hub.publish(4, 4, vec![0; 8]);
        assert!(
            hub.wait_new(seq, Duration::from_millis(1)).is_none(),
            "尺寸与实际长度不符的帧必须被拒（槽里仍是上一帧）"
        );
        hub.clear();
        assert!(hub.is_closed());
        assert!(!hub.has_frame());
    }
}
