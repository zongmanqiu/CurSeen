//! GIF 裁剪：把录下来的动图**掐头去尾、抠掉中间不要的段**，重写成一份 GIF。
//!
//! # 为什么不是"解码成帧列表 → 挑一挑 → 重新编码"
//!
//! 最直观的写法是把整段解码成一堆 RGBA 帧（`Vec<Vec<u8>>`）再挑，但那笔内存很吓人：
//! 720p、15fps、10 秒 = 150 帧 × 3.7 MB ≈ **550 MB**，用户框大一点就直接把程序顶爆。
//!
//! 这里走的是**流式重放**：解码器只维护**一张画布**，每解出一帧就把它合成到画布上，
//! 该留就顺手写进输出、该丢就跳过。峰值内存 = 2 张画布（当前 + 待写帧的副本），
//! 720p 约 1.8 MB，**与时长无关**。
//!
//! # 为什么"跳过一段"不影响画面正确性
//!
//! 我们的产物是 `Dispose::Keep` + **子矩形帧**：每一帧的语义是"把这块覆盖到当前画布上"。
//! 所以把中间某段丢掉之后，下一帧只要写出"相对**上一个被保留的帧**的变化矩形"，重放
//! 出来的画布就与源里那一帧完全一致 —— 被跳过的那几帧从来没被画出来，也就不需要"擦掉"。
//! 这一步的代价只是**接缝处的矩形可能偏大**（跨过了被删区间里所有的变化），不影响正确性。
//!
//! # 为什么输出是"不透明、拍平"的
//!
//! 源文件里可能有透明像素、`Background`/`Previous` 处置。解码时按 GIF 语义把它们**合成**
//! 到画布上（见 [`FrameReader::next_frame`]），写出去的就是合成后的实心画面。视觉结果与
//! 原文件逐帧一致，代价是失去了"哪些像素本来就是透明"的信息 —— 对屏幕录制而言那本来
//! 就是没有的东西。
//!
//! # 不支持的输入
//!
//! **带局部色表的帧**：我们只搬一份全局色表，逐帧局部色表要原样重排就必须保序保长，
//! 那等于把 GIF 的色表管理整套搬过来。这类文件（多来自老式动画制作工具）会被明确拒绝，
//! 而不是悄悄换个色表把颜色搞错。
//!
//! # 输出落点：**另存为**，源文件只读（2026-09-20 用户口径）
//!
//! [`rewrite_to`] 把结果写到调用方给的 `dst`，`src` 全程只被读。所以这个模块不需要"备份 /
//! 留档"的概念 —— 原片天然还在。落点等于原片时直接报错（[`same_file`]），那是手滑而不是
//! 意图。写盘策略是"先写同目录临时文件、成功了再 `rename` 上去"，落点上永远只见完整产物。

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use super::gif_enc::{self, GifWriter, Palette};

/// 帧延时的兜底（厘秒）。
///
/// 有些工具会写 0 延时的帧，而看图器/浏览器把 <2cs 当"未指定"按 100ms 播（见
/// `gif_enc::MIN_DELAY_CS`）。我们自己生成的文件不会有，但裁剪是吃**任意** GIF 的入口。
fn delay_of(raw: u16) -> u16 {
    raw.max(gif_enc::MIN_DELAY_CS)
}

/// 按 `Background` 处置清空一块矩形（画成"透明"，在索引空间里就是 0 号项）。
///
/// ★ 索引空间里的 0 号项**只是**"透明"的表示，真源是旁边那条**不透明掩码**。两者必须一起改，
///   否则"被清掉"的像素在掩码里还留着 true，产物就会把它们当实色写出去（见 [`blit`]）。
fn clear_rect(canvas: &mut [u8], opaque: &mut [bool], w: u32, h: u32, r: (u32, u32, u32, u32)) {
    let (l, t, rw, rh) = r;
    for y in t..(t + rh).min(h) {
        let s = (y * w) as usize + l as usize;
        let e = s + rw.min(w.saturating_sub(l)) as usize;
        if e <= canvas.len() {
            canvas[s..e].fill(0);
        }
        if e <= opaque.len() {
            opaque[s..e].fill(false);
        }
    }
}

/// 把一帧的下标贴到画布上。`transparent` 里的下标跳过（保留画布原值）。
///
/// 同时**点亮不透明掩码**：贴上去了说明"这里真的画了东西"。跳过的（透明）像素掩码保持原样
/// —— 这正是 GIF 的语义（透明 = 让底下的东西透出来），也是 [`rewrite_to`] 能忠实回写透明
/// 背景的唯一依据。
#[allow(clippy::too_many_arguments)]
fn blit(
    canvas: &mut [u8],
    opaque: &mut [bool],
    cw: u32,
    ch: u32,
    l: u32,
    t: u32,
    buf: &[u8],
    fw: u32,
    fh: u32,
    transparent: Option<u8>,
) {
    for row in 0..fh {
        let cy = t + row;
        if cy >= ch {
            break;
        }
        let src_row = (row * fw) as usize;
        let dst_row = (cy * cw + l) as usize;
        for col in 0..fw {
            let cx = l + col;
            if cx >= cw {
                break;
            }
            let si = src_row + col as usize;
            let Some(&v) = buf.get(si) else { continue };
            if Some(v) == transparent {
                continue;
            }
            let di = dst_row + col as usize;
            if let Some(slot) = canvas.get_mut(di) {
                *slot = v;
            }
            if let Some(m) = opaque.get_mut(di) {
                *m = true;
            }
        }
    }
}

/// 索引画布 → RGBA（预览用）。**透明像素给 alpha = 0。**
///
/// ★ 透明必须一路传到预览：否则用户看的是"0 号色的实底"、存出来的却是透明的 —— 预览与
///   产物不一致是最难查的一类问题（用户看到的根本不是他会拿到的图）。alpha=0 落到深色预览
///   底上，读起来就是"这里什么都没有"，与看图器的棋盘格同义。
fn to_rgba(canvas: &[u8], palette: &[u8], opaque: &[bool]) -> Vec<u8> {
    let n = palette.len() / 3;
    let mut out = Vec::with_capacity(canvas.len() * 4);
    for (i, &idx) in canvas.iter().enumerate() {
        let k = (idx as usize).min(n.saturating_sub(1)) * 3;
        let (r, g, b) = if palette.len() >= k + 3 {
            (palette[k], palette[k + 1], palette[k + 2])
        } else {
            (0, 0, 0)
        };
        let a = if opaque.get(i).copied().unwrap_or(true) {
            255
        } else {
            0
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

/// 顺序解码器：**带一张画布**，支持"推进一帧"和"跳到第 k 帧"。
///
/// 单调向右的访问是增量的（预览滑杆从左往右拖时几乎不花时间）；往回跳则重新打开文件。
pub struct FrameReader {
    path: PathBuf,
    src: gif::Decoder<BufReader<File>>,
    w: u32,
    h: u32,
    palette: Vec<u8>,
    canvas: Vec<u8>,
    /// 像素"真的被画过"的掩码 —— **透明与否的真源**。
    ///
    /// 为什么不能只看索引：索引画布是零初始化的，而 0 号项既可能是"透明"也可能是"
    /// 某个真的黑/白像素"，光看下标分不出来。掩码才是事实。
    opaque: Vec<bool>,
    /// 源文件声明过的透明下标（取第一次见到的那个）。`None` = 这份 GIF 从不声明透明
    /// ⇒ 一个像素都不该被写成透明（我们自己的录制产物就是这一类）。
    tidx: Option<u8>,
    delays: Vec<u16>,
    /// 已经合成到第几帧（= 已消费的帧数）。`canvas` 是第 `cur - 1` 帧之后的画面。
    cur: usize,
    done: bool,
    /// 上一帧的处置：下一帧合成**之前**要先按它处理画布。
    pending_dispose: gif::DisposalMethod,
    last_rect: (u32, u32, u32, u32),
    /// `Previous` 处置要用的"覆盖前"快照（画面与掩码必须**同时**回滚）。
    snapshot: Option<(Vec<u8>, Vec<bool>)>,
}

impl FrameReader {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|e| format!("打开 {} 失败：{e}", path.display()))?;
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let dec = opts
            .read_info(BufReader::new(file))
            .map_err(|e| format!("{} 不是有效的 GIF：{e}", path.display()))?;
        let (w, h) = (dec.width() as u32, dec.height() as u32);
        if w == 0 || h == 0 {
            return Err("GIF 画布尺寸为 0".to_string());
        }
        let palette = dec.global_palette().map(|p| p.to_vec()).unwrap_or_default();
        if palette.is_empty() || palette.len() % 3 != 0 {
            return Err("GIF 没有可用的全局色表（本工具的裁剪不处理逐帧局部色表）".to_string());
        }
        Ok(Self {
            path: path.to_path_buf(),
            src: dec,
            w,
            h,
            palette,
            canvas: vec![0u8; (w * h) as usize],
            // ★ 初始画布 = **全透明**（与看图器/浏览器一致：GIF 的"逻辑屏幕背景色"在实际
            //   渲染里不铺底，第一帧没画到的地方就是透的）。
            opaque: vec![false; (w * h) as usize],
            tidx: None,
            delays: Vec::new(),
            cur: 0,
            done: false,
            pending_dispose: gif::DisposalMethod::Keep,
            last_rect: (0, 0, 0, 0),
            snapshot: None,
        })
    }

    pub fn width(&self) -> u32 {
        self.w
    }

    pub fn height(&self) -> u32 {
        self.h
    }

    pub fn palette(&self) -> &[u8] {
        &self.palette
    }

    /// 已经解出来的帧数。
    pub fn decoded(&self) -> usize {
        self.cur
    }

    /// 当前画布（= 第 `decoded() - 1` 帧合成之后的完整画面，索引空间）。
    pub fn canvas(&self) -> &[u8] {
        &self.canvas
    }

    /// 与 [`Self::canvas`] 一一对应的**不透明掩码**（`true` = 这个像素真的被画过）。
    pub fn opaque_mask(&self) -> &[bool] {
        &self.opaque
    }

    /// 源文件声明过的透明下标（`None` = 从不声明透明）。
    pub fn transparent_index(&self) -> Option<u8> {
        self.tidx
    }

    /// 各帧延时（只到已解码的帧为止）。
    pub fn delays(&self) -> &[u16] {
        &self.delays
    }

    /// 推进一帧；返回它的帧号，`None` = 到头了。
    pub fn next_frame(&mut self) -> Result<Option<usize>, String> {
        if self.done {
            return Ok(None);
        }
        // ① 上一帧的处置先落实（GIF 的处置是**下一帧显示前**生效的）。
        match self.pending_dispose {
            gif::DisposalMethod::Background => clear_rect(
                &mut self.canvas,
                &mut self.opaque,
                self.w,
                self.h,
                self.last_rect,
            ),
            gif::DisposalMethod::Previous => {
                // 画面与掩码必须**同时**回滚：只回滚画面的话，"这块曾经被画过"这个事实会
                // 留在掩码里，产物就把它当实色写出去（该透的地方不透）。
                if let Some((c, m)) = &self.snapshot {
                    self.canvas.copy_from_slice(c);
                    self.opaque.copy_from_slice(m);
                }
            }
            _ => {}
        }

        // ② 读一帧并合成。`f` 借着 `self.src`，这段里只碰其它字段（借用是分字段的）。
        let (l, t, fw, fh, delay, dispose) = {
            let f = match self.src.read_next_frame() {
                Ok(Some(f)) => f,
                Ok(None) => {
                    self.done = true;
                    return Ok(None);
                }
                Err(e) => return Err(format!("解码第 {} 帧失败：{e}", self.cur + 1)),
            };
            if f.palette.is_some() {
                return Err("源文件里有带局部色表的帧，裁剪无法保真".to_string());
            }
            // 记下源文件声明过的透明下标（只要见过一次就够：它只用来回答"这份 GIF 到底有没有
            // 透明"这个问题；真正写进产物的那个透明下标由 `rewrite_to` 另开新槽，见其文档）。
            if self.tidx.is_none() {
                self.tidx = f.transparent;
            }
            // `Previous` 要的是"这一帧盖上去之前"的画面，所以快照必须在 blit 之前。
            if f.dispose == gif::DisposalMethod::Previous {
                self.snapshot = Some((self.canvas.clone(), self.opaque.clone()));
            } else {
                self.snapshot = None;
            }
            blit(
                &mut self.canvas,
                &mut self.opaque,
                self.w,
                self.h,
                f.left as u32,
                f.top as u32,
                &f.buffer,
                f.width as u32,
                f.height as u32,
                f.transparent,
            );
            (
                f.left as u32,
                f.top as u32,
                f.width as u32,
                f.height as u32,
                delay_of(f.delay),
                f.dispose,
            )
        };

        self.pending_dispose = dispose;
        self.last_rect = (l, t, fw, fh);
        self.delays.push(delay);
        self.cur += 1;
        Ok(Some(self.cur - 1))
    }

    /// 走到底，返回总帧数（幂等）。
    pub fn frame_count(&mut self) -> Result<usize, String> {
        while self.next_frame()?.is_some() {}
        Ok(self.cur)
    }

    /// 合成到第 `k` 帧，返回该帧的 RGBA 预览（**透明像素 alpha = 0**，见 [`to_rgba`]）。
    pub fn frame_rgba_at(&mut self, k: usize) -> Result<(u32, u32, Vec<u8>), String> {
        if k < self.cur {
            let p = self.path.clone();
            *self = Self::open(&p)?;
        }
        while self.cur <= k {
            if self.next_frame()?.is_none() {
                return Err(format!("帧号 {k} 越界（共 {} 帧）", self.cur));
            }
        }
        Ok((
            self.w,
            self.h,
            to_rgba(&self.canvas, &self.palette, &self.opaque),
        ))
    }
}

// ─────────────────────────── 保留区间 ───────────────────────────

/// 规范化一组区间：丢掉空段、按起点排序、合并重叠/相邻段。
///
/// 所有对外接口都先过这一道，于是下游可以放心假设"升序、互不相交、非空"。
pub fn normalize(ranges: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut v: Vec<(usize, usize)> = ranges.iter().copied().filter(|(a, b)| b > a).collect();
    v.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(v.len());
    for (a, b) in v {
        match out.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => out.push((a, b)),
        }
    }
    out
}

/// 「删掉这几段」→「保留这几段」。裁剪界面只维护"要删的区间"，这里是唯一的换算点。
pub fn complement(total: usize, removed: &[(usize, usize)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut cur = 0usize;
    for (a, b) in normalize(removed) {
        let a = a.min(total);
        let b = b.min(total);
        if a > cur {
            out.push((cur, a));
        }
        cur = cur.max(b);
    }
    if cur < total {
        out.push((cur, total));
    }
    out.retain(|(a, b)| b > a);
    out
}

/// 一组保留区间的（帧数，总时长厘秒）—— 给界面上的实时摘要用。
pub fn plan_summary(delays: &[u16], keep: &[(usize, usize)]) -> (usize, u32) {
    let mut frames = 0usize;
    let mut cs = 0u32;
    for (a, b) in normalize(keep) {
        // `get` 切片：a 越界时整段为空（与旧 `a..b.min(len)` 的不 panic 语义一致）。
        for &d in delays.get(a..b.min(delays.len())).into_iter().flatten() {
            frames += 1;
            cs = cs.saturating_add(u32::from(d));
        }
    }
    (frames, cs)
}

// ─────────────────────────── 重写 ───────────────────────────

/// 一次裁剪的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrimReport {
    /// 写进新文件的帧数。
    pub frames: usize,
    /// 新文件的时长（厘秒）。
    pub duration_cs: u32,
    /// 新文件的字节数。
    pub bytes: u64,
    /// 实际落点。用户可能在「另存为」对话框里改了名或换了目录，日志与浮字都该说真话。
    pub dst: PathBuf,
}

/// 按保留区间重写 `src`，结果**落到 `dst`** —— 源文件全程只读，绝不被改动。
///
/// # 为什么是"另存为"而不是"原地替换"（2026-09-20 用户口径）
///
/// 原片是**录制产物**，是一次性的既成事实；裁剪是**编辑**，编辑的结果该是一件新东西。
/// 旧实现（原地替换 + 把原片归档到 `temp/gif_trim/`）有两个说不清的地方：
/// 一是"留档放哪、留多久"没有答案，二是用户根本没法一次裁出两个版本（掐头一版、去尾一版）。
/// 改成另存为之后，原片天然还在，`temp/gif_trim/` 那套归档也就不需要了。
///
/// ★ 与 `src` 相同的落点会被**拒绝**（[`same_file`]）：那是用户手滑，不是意图。
///   对话框的"重名确认"不能替这一条把关 —— 它只问"要不要覆盖"，而"覆盖正在编辑的原片"
///   无论怎么确认都是错的。
///
/// # 透明（第三方 GIF 才有的形态）
///
/// 贴纸 / 表情包靠"透明下标"让底下的东西透出来，而**我们自己的录制产物从不带透明** ——
/// 所以这条路径是「随时裁盘上任意 GIF」通用化之后才第一次碰到的输入。
///
/// 做法：`FrameReader` 除了索引画布，还维护一条**不透明掩码**（哪些像素真的被画过）；
/// 写出去时把"从没被画过"的像素换成**新开的那个专用透明下标**（[`Palette::with_extra_slot`]）。
/// 不复用源文件声明的透明下标，因为 GIF 的透明是**逐帧**语义（同一个下标可能在这一帧算透明、
/// 在另一帧当实色画），复用就会在产物里挖出看不见的洞。
///
/// ★ 色表已满 [`gif_enc::MAX_COLORS`]（256 色）时开不出新槽，此时退回"全实心写" ——
///   与修复前一致。这是**已知退化点**：256 色的源同时又要透明，信息上本来就得牺牲一个色位。
pub fn rewrite_to(src: &Path, keep: &[(usize, usize)], dst: &Path) -> Result<TrimReport, String> {
    let keep = normalize(keep);
    if keep.is_empty() {
        return Err("没有保留任何帧".to_string());
    }
    if same_file(src, dst) {
        return Err("落点就是原文件，请另存为新的文件名".to_string());
    }
    if let Some(dir) = dst.parent() {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            return Err(format!("目标目录不存在：{}", dir.display()));
        }
    }
    let mut rd = FrameReader::open(src)?;
    let base = Palette::from_table(rd.palette())
        .ok_or_else(|| "色表长度非法（应为 3×2^n）".to_string())?;
    // 透明下标：**另开一槽**，不复用源文件声明的那个（理由见 `write_raw_alpha` 与
    // `Palette::with_extra_slot`：透明是逐帧语义，同一个下标可能既被声明透明、又被当实色画）。
    // 开不出来（色表已满 `MAX_COLORS`）时退回"全实心写" —— 与修复前的行为一致。
    let (pal, alpha_idx) = match base.with_extra_slot() {
        Some((p, t)) => (p, Some(t)),
        None => (base, None),
    };
    let (w, h) = (rd.width(), rd.height());
    let (wu, hu) = (w as usize, h as usize);

    // 把"合成画面"翻译成**可以写出去的下标**：从没被画过的像素写成透明下标。
    //
    // ★ 这一步是整个透明修复的落点。索引画布上那些"从没被画过"的像素在 viewers 眼里是
    //   **透明的**（GIF 的逻辑屏幕背景色不铺底），而索引空间里它们与"真的用了 0 号色"没法
    //   区分 —— 所以判据是掩码，不是下标。
    let out_buf = |canvas: &[u8], mask: &[bool]| -> Vec<u8> {
        let mut v = canvas.to_vec();
        if let Some(t) = alpha_idx {
            for (i, m) in mask.iter().enumerate() {
                if !*m && i < v.len() {
                    v[i] = t;
                }
            }
        }
        v
    };

    // 临时文件放在**落点**同目录：`rename` 只在同卷内才是原子的搬迁（跨卷会退化成
    // 复制 + 删除，中途失败就留下半截文件）。写成功了才搬过去，落点上永远只见完整产物。
    let tmp = tmp_sibling(dst);
    let _ = std::fs::remove_file(&tmp);
    let mut writer =
        GifWriter::create(&tmp, w, h, &pal, true).map_err(|e| format!("创建临时文件失败：{e}"))?;

    let mut seg = 0usize;
    // 待写帧：`(完整画面, 变化矩形, 显示时长厘秒)`。
    type PendingFrame = (Vec<u8>, (u16, u16, u16, u16), u32);
    let mut pending: Option<PendingFrame> = None;
    // 已经写出去（或正待写）的那一帧的完整画面 = 输出流里当前"画布"的样子。
    let mut rendered: Option<Vec<u8>> = None;
    let mut frames = 0usize;
    let mut duration_cs = 0u32;
    let full_rect = (0u16, 0u16, w as u16, h as u16);

    let result = (|| -> Result<(), String> {
        while let Some(i) = rd.next_frame()? {
            while seg < keep.len() && i >= keep[seg].1 {
                seg += 1;
            }
            if seg >= keep.len() {
                break; // 后面全是不要的，不必再解
            }
            if i < keep[seg].0 {
                continue; // 落在被删的区间里
            }
            let d = rd.delays()[i] as u32;
            duration_cs = duration_cs.saturating_add(d);

            // ★ 比较/求差/落盘**一律用翻译后的缓冲**：同一个"从没被画过"的像素在原始画布上
            //   可能是任意残留索引，拿它比会得出"变了/没变"的错误结论。
            let cur = out_buf(rd.canvas(), rd.opaque_mask());

            // 与待写帧逐像素相同 → 不写帧，把时长并到它身上（静止画面零成本）。
            if let Some((pc, _, pd)) = pending.as_mut() {
                if pc.as_slice() == cur.as_slice() {
                    *pd = pd.saturating_add(d);
                    continue;
                }
            }
            // 有变化 → 上一帧落盘。
            if let Some((pc, prect, pd)) = pending.take() {
                writer
                    .write_raw_alpha(&pc, prect, gif_enc::clamp_delay(pd), alpha_idx)
                    .map_err(|e| format!("写帧失败：{e}"))?;
                frames += 1;
            }
            // 相对**上一个被保留的帧**求变化矩形（跨过被删区间时矩形会偏大，但正确）。
            let rect = match &rendered {
                Some(prev) => gif_enc::diff_rect(prev, &cur, wu, hu).unwrap_or(full_rect),
                None => full_rect,
            };
            rendered = Some(cur.clone());
            pending = Some((cur, rect, d));
        }
        if let Some((pc, prect, pd)) = pending.take() {
            writer
                .write_raw_alpha(&pc, prect, gif_enc::clamp_delay(pd), alpha_idx)
                .map_err(|e| format!("写帧失败：{e}"))?;
            frames += 1;
        }
        Ok(())
    })();

    if let Err(e) = result {
        drop(writer);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if frames == 0 {
        drop(writer);
        let _ = std::fs::remove_file(&tmp);
        return Err("裁剪后一帧都不剩".to_string());
    }
    writer.finish().map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("GIF 收尾失败：{e}")
    })?;
    let bytes = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);

    // `fs::rename` 在 Windows 上走 `MoveFileExW(.., MOVEFILE_REPLACE_EXISTING)`，可直接覆盖。
    // 覆盖的是**用户自己在对话框里挑的那个文件**（他已经被告知重名了），不是原片。
    if let Err(e) = std::fs::rename(&tmp, dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("写入目标文件失败：{e}"));
    }
    Ok(TrimReport {
        frames,
        duration_cs,
        bytes,
        dst: dst.to_path_buf(),
    })
}

/// 同目录的临时文件名（同卷才能 `rename` 覆盖）。
fn tmp_sibling(dst: &Path) -> PathBuf {
    let mut name = dst
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(".curseen-tmp");
    dst.with_file_name(name)
}

/// 两个路径是否指向同一个文件。
///
/// 优先 `canonicalize`：Windows 上它会把短名（8.3）、`\\?\` 前缀、大小写、`..` 段全部归一，
/// 是唯一可靠的判等。但它要求文件**已存在** —— 用户挑了个新名字时落点当然不存在，此时退化
/// 成字符串比较（分隔符统一成 `\`、转小写，因为 Windows 路径大小写不敏感）。
///
/// 退化分支挡不住"同一个文件用相对路径 vs 绝对路径表达"这种极端输入，但两个调用方给的
/// 都是绝对路径（录制器给的是我们亲手拼的绝对路径，对话框回填的也是绝对路径），够用。
fn same_file(a: &Path, b: &Path) -> bool {
    if let (Ok(x), Ok(y)) = (a.canonicalize(), b.canonicalize()) {
        return x == y;
    }
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    norm(a) == norm(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("curseen_giftrim_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("建临时目录");
        d
    }

    /// 造一段测试素材：`n` 帧，每帧把 `frames[i]` 的那一片画上去（与录制产物同构：
    /// 全局色表 + `Dispose::Keep` + 子矩形帧 + 首帧整幅）。
    fn write_source(path: &Path, w: u16, h: u16, frames: &[(Vec<u8>, u16)]) {
        use gif::Frame;
        let pal: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0, 255];
        let file = std::fs::File::create(path).expect("建文件");
        let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w, h, &pal).expect("编码器");
        enc.set_repeat(gif::Repeat::Infinite).expect("循环");
        for (buf, delay) in frames {
            let f = Frame {
                width: w,
                height: h,
                buffer: buf.clone().into(),
                delay: *delay,
                dispose: gif::DisposalMethod::Keep,
                ..Default::default()
            };
            enc.write_frame(&f).expect("写帧");
        }
    }

    /// 两种纯色、8×4 的三帧素材：帧0 全黑、帧1 上半白、帧2 全白。
    fn three_frame_source(dir: &Path) -> PathBuf {
        let (w, h) = (8u16, 4u16);
        let n = (w * h) as usize;
        let f0 = vec![0u8; n];
        let mut f1 = vec![0u8; n];
        for v in f1.iter_mut().take(n / 2) {
            *v = 1;
        }
        let f2 = vec![1u8; n];
        let p = dir.join("src.gif");
        write_source(
            &p,
            w,
            h,
            &[(f0.clone(), 5), (f1.clone(), 5), (f2.clone(), 5)],
        );
        p
    }

    /// 解码整份 GIF 成"逐帧画布"，用来比对裁剪前后的画面。
    fn replay(path: &Path) -> (u32, u32, Vec<Vec<u8>>, Vec<u16>) {
        let mut rd = FrameReader::open(path).expect("打开");
        let n = rd.frame_count().expect("数帧");
        let (w, h) = (rd.width(), rd.height());
        let delays = rd.delays().to_vec();
        let mut out = Vec::new();
        for k in 0..n {
            let (_, _, rgba) = rd.frame_rgba_at(k).expect("取帧");
            // 取 R 通道当"颜色标签"（素材只用了黑/白两种）。
            out.push(rgba.chunks_exact(4).map(|p| p[0]).collect::<Vec<u8>>());
        }
        (w, h, out, delays)
    }

    #[test]
    fn normalize_sorts_merges_and_drops_empty() {
        assert_eq!(normalize(&[]), vec![]);
        assert_eq!(normalize(&[(5, 5)]), vec![], "空段要丢掉");
        assert_eq!(
            normalize(&[(4, 6), (0, 2)]),
            vec![(0, 2), (4, 6)],
            "要按起点排序"
        );
        assert_eq!(normalize(&[(0, 5), (3, 8)]), vec![(0, 8)], "重叠要合并");
        assert_eq!(normalize(&[(0, 5), (5, 9)]), vec![(0, 9)], "相邻要合并");
    }

    #[test]
    fn complement_is_the_exact_inverse_of_removal() {
        assert_eq!(complement(10, &[]), vec![(0, 10)]);
        assert_eq!(complement(10, &[(3, 6)]), vec![(0, 3), (6, 10)], "抠中间");
        assert_eq!(complement(10, &[(0, 4)]), vec![(4, 10)], "去头");
        assert_eq!(complement(10, &[(7, 10)]), vec![(0, 7)], "去尾");
        assert_eq!(complement(10, &[(0, 10)]), vec![], "全删光");
        assert_eq!(
            complement(10, &[(2, 4), (4, 6)]),
            vec![(0, 2), (6, 10)],
            "接壤的两段合并后仍要对"
        );
        // 越界的区间要被夹进 [0, total]，不能把保留段算出负数或倒序。
        assert_eq!(complement(5, &[(0, 99)]), vec![]);
        assert_eq!(complement(5, &[(99, 200)]), vec![(0, 5)]);
    }

    #[test]
    fn plan_summary_counts_kept_frames_and_time() {
        let delays = [5u16, 7, 3, 10];
        assert_eq!(plan_summary(&delays, &[(0, 4)]), (4, 25), "全留");
        assert_eq!(plan_summary(&delays, &[(0, 2)]), (2, 12), "前两帧");
        assert_eq!(plan_summary(&delays, &[(1, 3)]), (2, 10), "中间两帧");
        assert_eq!(plan_summary(&delays, &[]), (0, 0));
    }

    /// 目录里遗留的 `.curseen-tmp` 残渣（任何一条失败路径都不该留下它）。
    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("读目录")
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".curseen-tmp"))
            .collect()
    }

    /// 掐头：去掉首帧后，剩下的画面必须与源里对应的帧**逐像素一致**。
    ///
    /// 这是整条裁剪链路的地基 —— 跳帧之后如果"接缝处矩形"算错，画面上会留下上一段
    /// 的残影，而那种错很难用肉眼在成品里发现。
    ///
    /// ★ 2026-09-20 起裁剪是**另存为**：断言因此要落在**目标文件**上，并且顺手把
    ///   "源文件逐字节不变"一起钉住 —— 那正是"不顶替原片"这条需求的机器判据。
    #[test]
    fn trimming_the_head_preserves_the_remaining_frames() {
        let dir = tmp_dir("head");
        let src = three_frame_source(&dir);
        let dst = dir.join("out.gif");
        let (_, _, before, _) = replay(&src);
        assert_eq!(before.len(), 3);
        let src_before = std::fs::read(&src).expect("读原件");

        let rep = rewrite_to(&src, &[(1, 3)], &dst).expect("裁剪应成功");
        assert_eq!(rep.frames, 2, "去掉首帧后应剩两帧");
        assert_eq!(rep.duration_cs, 10, "5+5");
        assert_eq!(rep.dst, dst, "报告里的落点应是真实落点");
        assert!(rep.bytes > 0);

        let (w, h, after, _) = replay(&dst);
        assert_eq!((w, h), (8, 4), "画布尺寸不能变");
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], before[1], "第 1 帧画面错位");
        assert_eq!(after[1], before[2], "第 2 帧画面错位");
        assert_eq!(
            std::fs::read(&src).expect("读回原件"),
            src_before,
            "源文件被改动了 —— 另存为的前提就是它只读"
        );
        assert!(leftovers(&dir).is_empty(), "留下了临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 抠中间：删掉第 1 帧后，剩下两帧的画面仍要分别对上源里的第 0、第 2 帧。
    #[test]
    fn trimming_a_middle_segment_preserves_both_sides() {
        let dir = tmp_dir("middle");
        let src = three_frame_source(&dir);
        let dst = dir.join("out.gif");
        let (_, _, before, _) = replay(&src);

        let rep = rewrite_to(&src, &complement(3, &[(1, 2)]), &dst).expect("裁剪应成功");
        assert_eq!(rep.frames, 2);
        assert_eq!(rep.duration_cs, 10);

        let (_, _, after, _) = replay(&dst);
        assert_eq!(after.len(), 2);
        assert_eq!(after[0], before[0], "前半段画面变了");
        assert_eq!(after[1], before[2], "后半段画面变了");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 只留一帧：产物必须是单帧，且延时等于那一帧的时长（不是 0，也不能是整段）。
    #[test]
    fn keeping_a_single_frame_yields_a_single_frame_gif() {
        let dir = tmp_dir("single");
        let src = three_frame_source(&dir);
        let dst = dir.join("out.gif");
        let rep = rewrite_to(&src, &[(2, 3)], &dst).expect("裁剪应成功");
        assert_eq!(rep.frames, 1);
        assert_eq!(rep.duration_cs, 5);

        let (_, _, after, delays) = replay(&dst);
        assert_eq!(after.len(), 1);
        assert_eq!(delays, vec![5]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 删光 / 空区间必须是**错误**，而且源文件不能被改动或删掉、也不该留下临时文件。
    #[test]
    fn refusing_to_write_an_empty_gif_leaves_the_source_untouched() {
        let dir = tmp_dir("empty");
        let src = three_frame_source(&dir);
        let dst = dir.join("out.gif");
        let before = std::fs::read(&src).expect("读原件");
        assert!(rewrite_to(&src, &[], &dst).is_err(), "空保留区间应报错");
        assert!(
            rewrite_to(&src, &complement(3, &[(0, 3)]), &dst).is_err(),
            "全删应报错"
        );
        assert_eq!(std::fs::read(&src).expect("读回"), before, "源文件被动过了");
        assert!(!dst.exists(), "报错了却还是生成了目标文件");
        assert!(leftovers(&dir).is_empty(), "失败后残留了临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 全留 = 幂等：画面与帧数都不该变。
    #[test]
    fn keeping_everything_is_a_no_op_on_content() {
        let dir = tmp_dir("all");
        let src = three_frame_source(&dir);
        let dst = dir.join("out.gif");
        let (_, _, before, delays_before) = replay(&src);
        rewrite_to(&src, &[(0, 3)], &dst).expect("裁剪应成功");
        let (_, _, after, delays_after) = replay(&dst);
        assert_eq!(after, before, "全留不该改变任何一帧");
        assert_eq!(delays_after, delays_before, "全留不该改变延时");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─────────────────── 「另存为」的硬约束 ───────────────────

    /// **落点绝不能是原片本身** —— 这条不靠对话框的"重名确认"，靠这里挡死。
    ///
    /// 用户手滑把落点选成原片时，唯一正确的结果是拒绝；对话框只能问"要不要覆盖"，
    /// 而"覆盖正在编辑的原片"无论怎么确认都是错的。
    #[test]
    fn the_source_file_is_never_a_valid_destination() {
        let dir = tmp_dir("same");
        let src = three_frame_source(&dir);
        let before = std::fs::read(&src).expect("读原件");

        // ① 一模一样的路径。
        let e = rewrite_to(&src, &[(0, 2)], &src).expect_err("落点 = 原片时必须报错");
        assert!(e.contains("原文件"), "错误信息要能看懂，实得 {e}");

        // ② 同一个文件换一种写法：大小写 + 分隔符（Windows 路径大小写不敏感）。
        //    这条挡的是"对话框回填的路径与原路径字面不同、其实是同一个文件"。
        let shouty = PathBuf::from(src.to_string_lossy().replace('/', "\\").to_uppercase());
        if shouty != src {
            assert!(
                rewrite_to(&src, &[(0, 2)], &shouty).is_err(),
                "大小写不同的同一文件也要挡住：{}",
                shouty.display()
            );
        }

        assert_eq!(std::fs::read(&src).expect("读回"), before, "原片被动过了");
        assert!(leftovers(&dir).is_empty(), "被拒绝的调用不该留下临时文件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 落点可以在**任意目录**（含不存在的父目录 → 明确的错误，而不是含混的 IO 失败）。
    #[test]
    fn the_output_lands_where_asked() {
        let dir = tmp_dir("dst");
        let src = three_frame_source(&dir);
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).expect("建子目录");
        // 非 ASCII 文件名也要能写 —— 用户会这么起名（「已裁剪」）。
        let dst = sub.join("裁剪后.gif");

        let rep = rewrite_to(&src, &[(0, 2)], &dst).expect("裁剪应成功");
        assert_eq!(rep.dst, dst);
        assert!(dst.exists(), "目标文件没生成");
        assert!(rep.bytes > 0);
        // 临时文件必须与落点同目录（同卷才能原子搬移），且已被搬走。
        assert!(leftovers(&dir).is_empty(), "原目录不该有临时文件");
        assert!(leftovers(&sub).is_empty(), "落点目录留下了临时文件");

        // 父目录不存在：给一句人话，别让用户去猜 IO 错误码。
        let missing = dir.join("nope").join("x.gif");
        let e = rewrite_to(&src, &[(0, 2)], &missing).expect_err("目录不存在应报错");
        assert!(e.contains("目录不存在"), "错误信息要能看懂，实得 {e}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ─────────────────── 透明（第三方 GIF 才有的形态） ───────────────────

    /// 造一份**带透明背景**的素材 —— 贴纸 / 表情包的常见形态：透明下标 = 0，
    /// 画面只有中间一块用 1 号色（第 2 帧在别处再点一块 2 号色，免得两帧完全相同被静帧合并）。
    fn transparent_source(dir: &Path) -> PathBuf {
        use gif::Frame;
        let (w, h) = (8u16, 4u16);
        let pal: Vec<u8> = vec![0, 0, 0, 255, 255, 255, 255, 0, 0];
        let base = |extra: Option<(usize, usize, u8)>| {
            let mut b = vec![0u8; (w * h) as usize];
            for y in 1..3usize {
                for x in 2..6usize {
                    b[y * w as usize + x] = 1;
                }
            }
            if let Some((x, y, v)) = extra {
                b[y * w as usize + x] = v;
            }
            b
        };
        let p = dir.join("transparent.gif");
        let file = std::fs::File::create(&p).expect("建文件");
        let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w, h, &pal).expect("编码器");
        enc.set_repeat(gif::Repeat::Infinite).expect("循环");
        for buf in [base(None), base(Some((1, 1, 2)))] {
            let f = Frame {
                width: w,
                height: h,
                buffer: buf.into(),
                delay: 5,
                dispose: gif::DisposalMethod::Keep,
                transparent: Some(0),
                ..Default::default()
            };
            enc.write_frame(&f).expect("写帧");
        }
        p
    }

    /// **独立**解码一份 GIF 并按 GIF 语义合成，返回末帧的"不透明掩码"。
    ///
    /// ★ 特意**不复用** `FrameReader`：判据不能建在被判对象上（否则"两边一起错"会一起绿）。
    /// 这里只用 `gif` crate 的 RGBA 输出（透明像素给 alpha=0）+ 自己 20 行的合成：
    /// 透明像素不覆盖、`Background` 处置清成透明、`Previous` 回滚快照。
    fn composite_opaque_mask(path: &Path) -> (u32, u32, Vec<bool>) {
        let file = std::fs::File::open(path).expect("打开产物");
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let mut dec = opts
            .read_info(std::io::BufReader::new(file))
            .expect("解码产物");
        let (w, h) = (dec.width() as u32, dec.height() as u32);
        let n = (w * h) as usize;
        let mut opaque = vec![false; n];
        let mut saved = vec![false; n];
        let (mut dsp, mut rect) = (gif::DisposalMethod::Keep, (0u32, 0u32, 0u32, 0u32));
        while let Ok(Some(f)) = dec.read_next_frame() {
            match dsp {
                gif::DisposalMethod::Background => clear_rect_u32(&mut opaque, w, h, rect),
                gif::DisposalMethod::Previous => opaque.copy_from_slice(&saved),
                _ => {}
            }
            if f.dispose == gif::DisposalMethod::Previous {
                saved.copy_from_slice(&opaque);
            }
            for row in 0..f.height as usize {
                for col in 0..f.width as usize {
                    let x = f.left as usize + col;
                    let y = f.top as usize + row;
                    if x >= w as usize || y >= h as usize {
                        continue;
                    }
                    // RGBA 输出里透明像素 alpha = 0。
                    if f.buffer[(row * f.width as usize + col) * 4 + 3] != 0 {
                        opaque[y * w as usize + x] = true;
                    }
                }
            }
            dsp = f.dispose;
            rect = (f.left as u32, f.top as u32, f.width as u32, f.height as u32);
        }
        (w, h, opaque)
    }

    /// 掩码版的清矩形（测试内自备，不借实现里的那个）。
    fn clear_rect_u32(mask: &mut [bool], w: u32, h: u32, r: (u32, u32, u32, u32)) {
        for y in r.1..(r.1 + r.3).min(h) {
            for x in r.0..(r.0 + r.2).min(w) {
                mask[(y * w + x) as usize] = false;
            }
        }
    }

    /// **带透明背景的 GIF，裁完必须还是带透明背景的。**
    ///
    /// 这是「随时裁盘上任意 GIF」之后才可能碰到的输入：我们自己的录制产物**从不带
    /// 透明**，而贴纸 / 表情包几乎都靠"透明下标"让底下的东西透出来。画布若只是纯索引画布、
    /// 不记透明，那些像素就会被当成 0 号色**实心写出去** —— 用户拿到一张实底图，而预览时
    /// 完全看不出来（预览本来就把透明画成 0 号色）。
    #[test]
    fn a_transparent_background_survives_the_trim() {
        let dir = tmp_dir("alpha");
        let src = transparent_source(&dir);
        let dst = dir.join("out.gif");
        rewrite_to(&src, &[(0, 2)], &dst).expect("另存为");

        let (w, _h, mask) = composite_opaque_mask(&dst);
        let at = |x: usize, y: usize| mask[y * w as usize + x];
        assert!(!at(0, 0), "左上角该仍是**透明**的（产物把它写实了）");
        assert!(!at(7, 3), "右下角该仍是透明的");
        assert!(at(3, 1), "中间那块该是实心的");
        assert!(at(1, 1), "第 2 帧补画的那个像素该是实的");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
