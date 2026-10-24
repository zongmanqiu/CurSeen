//! 长截图拼接核心（**纯函数**，无 Win32 / windui 依赖，可离线单测）。
//!
//! 四条核心手法（各家长截图工具沉淀的通行经验）：
//! - **行哈希粗筛**：先在候选偏移上做行哈希比对，把逐像素
//!   精配的范围从几百行缩到个位候选；
//! - **纹理加权精配**（差值 × 纹理权重两因子）：
//!   纯色行权重打 0.2 折 —— 纯色页面 / 循环列表里"哪都能对上"的多解问题靠它压住；
//!   但加权平均会被"少数错配 + 大量匹配"稀释（粘性工具栏中途弹出时整段错配近乎免费），
//!   所以另立一条**错配率硬判据**：采样行里差得多的占比超限即否决；
//! - **粘性头部**：两个来源——
//!   ① 帧顶与上一帧连续不变的行（滚动中直接可测）；② 匹配失败时，若最佳候选的错配
//!   全部集中在**顶部连续带**且带高 ≥ `header_min_rows`（工具栏中途弹出），把带高
//!   采纳为头部后重试一次。头部只增不减（对齐数学允许，见下）；
//! - **帧闸门**：整帧哈希相同直接拒收 ——
//!   抓屏可能快于 DWM 合成，重复位图会浪费匹配预算并弄丢重叠桥。
//!
//! 模型是**只追加**的：画布从第一帧开始，向下滚 d 行就追加 d 行；向上滚回已捕获
//! 区域报 [`PushOutcome::NoNewContent`]（不裁剪 —— 回滚修整是二期）。
//!
//! 对齐数学（`header` 为当前认定的头部行数，画布第 i 行 = 页面第 i 行）：
//! ```text
//! 帧视口第 r 行 = 页面第 scroll+r 行；画布末行恒等于上一帧视口底，
//! 故 H = scroll_prev + 帧高。向下滚 d 行：
//!   重叠 L = 帧高 - header - d
//!   帧 [header, header+L) 应等于 画布 [H-L, H)
//!   （帧 header+j = 页面 scroll+header+j，画布 H-L+j = 页面 scroll_prev+d+header+j，
//!     而 scroll = scroll_prev + d ⇒ 两边同页行 ✓，对 header 的任何历史都成立）
//! 停顿/回滚（scroll ≤ scroll_prev）：
//!   帧内容整体落在画布内，按视口顶的画布偏移 o = scroll 搜索（≤ H-帧高）。
//! ```

use crate::core::image::RgbaBitmap;

/// 拼接参数。细节见各字段文档。
#[derive(Debug, Clone, Copy)]
pub struct StitchParams {
    /// 头部（粘性元素）最大行数占比（只认成片的固定带，不认零散静止行）。
    pub header_max_ratio: f32,
    /// 单帧最多判定为"新增"的行数占比上限：至少留 15% 的重叠做验证，
    /// 否则一次快甩就可能整帧无证据地拼进去。
    pub max_new_ratio: f32,
    /// 粗筛通过的候选里最多精配几个（按粗筛得分排序取前几）。
    pub refine_candidates: usize,
    /// 加权平均每像素差低于该值视为匹配成功（0..=255 尺度）。
    pub accept_avg_diff: f32,
    /// 采样行里"差得多"（> `accept_avg_diff`）的占比上限（错配率硬判据）。
    pub max_mismatch_frac: f32,
    /// 行内水平梯度低于该值 = 低纹理行（权重打 [`Self::flat_weight`] 折）。
    pub texture_low: f32,
    /// 低纹理行的权重（同类做法是低纹理块直接跳过；这里不跳过只降权，保留弱证据）。
    pub flat_weight: f32,
    /// 头部判定的行差阈值（与上一帧同位行平均差低于它 = "没变"）。
    pub header_static_diff: f32,
    /// 头部至少连续多少行才认定（太短当噪声）。
    pub header_min_rows: usize,
    /// 粗筛/精配采样的行距（每 N 行比一次）。
    pub row_step: usize,
    /// 粗筛要求的最小命中率（采样行里哈希相等的占比）。
    pub coarse_hit_ratio: f32,
    /// 回滚搜索的画布深度（视口高的倍数）：向上滚超过这么多行就当 Ambiguous。
    pub back_search_frames: usize,
}

impl Default for StitchParams {
    fn default() -> Self {
        Self {
            header_max_ratio: 0.30,
            max_new_ratio: 0.85,
            refine_candidates: 3,
            accept_avg_diff: 10.0,
            max_mismatch_frac: 0.12,
            texture_low: 3.0,
            flat_weight: 0.2,
            header_static_diff: 4.0,
            header_min_rows: 4,
            row_step: 4,
            coarse_hit_ratio: 0.6,
            back_search_frames: 2,
        }
    }
}

/// 一帧喂进来的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushOutcome {
    /// 追加了 `new_rows` 行新内容（第一帧返回整帧高）。
    Appended { new_rows: usize },
    /// 与上一帧逐字节相同（帧闸门：页面没动，或抓屏快于 DWM 刷新）。
    NoMotion,
    /// 内容已在画布内（停顿 / 回滚到已捕获区域）。画布不变。
    NoNewContent,
    /// 找不到可信重叠（快甩跳过太多 / 内容不连续）。本帧跳过，画布不变。
    Ambiguous,
}

/// 一次评估的结论：加权平均差 + 逐采样行的匹配位（供错配率与顶部错配带分析）。
struct Eval {
    avg: f32,
    /// (采样行号, 是否匹配)。行号相对"帧侧起始行"。
    samples: Vec<(usize, bool)>,
}

impl Eval {
    fn mismatch_frac(&self) -> f32 {
        if self.samples.is_empty() {
            return 1.0;
        }
        let bad = self.samples.iter().filter(|(_, m)| !m).count();
        bad as f32 / self.samples.len() as f32
    }

    /// 错配是否构成"顶部连续带"（其余采样行全匹配）。返回带高（行）。
    ///
    /// "只认成片固定带"的判据：零散错配（弹窗压在内容中间）
    /// 不构成带，不会误把内容当头部。
    fn top_band(&self, step: usize) -> Option<usize> {
        let mut band = 0usize;
        for (row, m) in &self.samples {
            if *m {
                break;
            }
            band = *row + step; // 带高推进到该采样行的行距边界
        }
        // 全部错配不算带（那是"根本没对上"，不是"顶部有固定条"）。
        let all_bad = self.samples.iter().all(|(_, m)| !m);
        if all_bad || self.samples.is_empty() {
            return None;
        }
        Some(band)
    }
}

/// 长截图拼接器。画布与抓屏帧同布局（`Shot.bgra`：BGRA 行优先），
/// 逐字节比较对通道序不敏感；出口经 [`Self::take_bitmap`] 转 RGBA。
pub struct ScrollStitcher {
    params: StitchParams,
    w: usize,
    frame_h: usize,
    /// 拼接画布（BGRA），行优先，长度恒为 `w * canvas_h * 4`。
    canvas: Vec<u8>,
    canvas_h: usize,
    /// 已认定的粘性头部行数（只增不减，见模块头"对齐数学"）。
    header_rows: usize,
    /// 上一帧原始字节（头部检测的对照）。
    last_frame: Vec<u8>,
    /// 上一帧整帧哈希（帧闸门）。
    last_hash: u64,
    /// 连续 Ambiguous 计数（调用方可据此报警）。
    pub ambiguous_streak: u32,
}

fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

impl ScrollStitcher {
    /// 建一个拼接器。`w`/`frame_h` 是抓屏帧的尺寸（像素）。
    pub fn new(w: u32, frame_h: u32) -> Self {
        Self::with_params(w, frame_h, StitchParams::default())
    }

    pub fn with_params(w: u32, frame_h: u32, params: StitchParams) -> Self {
        Self {
            params,
            w: w as usize,
            frame_h: frame_h as usize,
            canvas: Vec::new(),
            canvas_h: 0,
            header_rows: 0,
            last_frame: Vec::new(),
            last_hash: 0,
            ambiguous_streak: 0,
        }
    }

    pub fn canvas_h(&self) -> usize {
        self.canvas_h
    }

    pub fn header_rows(&self) -> usize {
        self.header_rows
    }

    /// 拼接器是否已吃过第一帧（画布已播种）。
    pub fn seeded(&self) -> bool {
        self.canvas_h > 0
    }

    /// 取走拼接结果（BGRA 画布）。调用后拼接器回到未播种态。
    pub fn take_canvas(&mut self) -> (usize, usize, Vec<u8>) {
        let out = (self.w, self.canvas_h, std::mem::take(&mut self.canvas));
        self.canvas_h = 0;
        out
    }

    /// 取走画布并转成 [`RgbaBitmap`]（BGRA→RGBA）。画布空 / 尺寸非法返回 `None`。
    pub fn take_bitmap(&mut self) -> Option<RgbaBitmap> {
        if self.canvas_h == 0 {
            return None;
        }
        let (w, h, bgra) = self.take_canvas();
        let mut rgba = Vec::with_capacity(w * h * 4);
        for px in bgra.chunks_exact(4) {
            rgba.push(px[2]);
            rgba.push(px[1]);
            rgba.push(px[0]);
            rgba.push(px[3]);
        }
        RgbaBitmap::new(w as u32, h as u32, rgba)
    }

    /// 喂一帧（BGRA 字节，长度必须 = `w * frame_h * 4`）。
    pub fn push(&mut self, frame: &[u8]) -> PushOutcome {
        let stride = self.w * 4;
        if frame.len() != stride * self.frame_h {
            return PushOutcome::Ambiguous; // 尺寸不符：拒绝而非 panic
        }
        let hash = fnv64(frame);
        if self.canvas.is_empty() {
            // 第一帧：整帧播种。
            self.canvas = frame.to_vec();
            self.canvas_h = self.frame_h;
            self.commit_frame(frame, hash);
            return PushOutcome::Appended {
                new_rows: self.frame_h,
            };
        }
        if hash == self.last_hash {
            return PushOutcome::NoMotion;
        }
        let p = self.params;
        // 头部检测（对上一帧）：帧顶连续"没变"的行。只在本帧做候选值，
        // 真正采纳（sticky 增长）要等匹配成功 —— 见模块头的对齐数学。
        let k = self.static_top_rows(frame);
        let header = if k >= p.header_min_rows {
            self.header_rows.max(k)
        } else {
            self.header_rows
        };
        let body_h = self.frame_h.saturating_sub(header);
        if body_h < p.header_min_rows * 2 {
            self.commit_frame(frame, hash);
            return PushOutcome::Ambiguous; // 头部吃掉整帧：没法匹配
        }

        // ── ① 向下滚：候选 d ∈ [1, max_d] ──
        let min_overlap = (body_h / 8).max(16).min(body_h);
        let max_d = body_h.saturating_sub(min_overlap) as i64;
        let mut cands: Vec<(i64, f32)> = Vec::new();
        for d in 1..=max_d {
            let hits = self.coarse_forward(frame, header, d);
            if hits >= p.coarse_hit_ratio {
                cands.push((d, hits));
            }
        }
        // 得分降序、同分取更小的 d（重叠更多 = 证据更多）。
        cands.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        cands.truncate(p.refine_candidates);
        for (d, _) in &cands {
            let e = self.eval_forward(frame, header, *d);
            if self.eval_pass(&e) {
                self.append_rows(frame, header, *d);
                self.adopt_header(k);
                self.commit_frame(frame, hash);
                self.ambiguous_streak = 0;
                return PushOutcome::Appended {
                    new_rows: *d as usize,
                };
            }
        }

        // ── ② 停顿 / 回滚：帧内容整体已在画布内（视口顶的画布偏移 o = scroll）──
        if let Some(o) = self.search_contained(frame, header) {
            let _ = o;
            self.adopt_header(k);
            self.commit_frame(frame, hash);
            self.ambiguous_streak = 0;
            return PushOutcome::NoNewContent;
        }

        // ── ③ 顶部错配带恢复：粘性工具栏**中途弹出**时，①② 都会因顶部整段错配而
        //      失败（加权平均救不了"结构性错配"，见模块头）。取错配集中在顶部的
        //      那次评估，把带高采纳为头部，重试一次。
        if header < self.max_header_rows() {
            let mut best: Option<(Eval, i64)> = None;
            for (d, _) in &cands {
                let e = self.eval_forward(frame, 0, *d);
                if e.top_band(p.row_step).is_some()
                    && best.as_ref().is_none_or(|(b, _)| e.avg < b.avg)
                {
                    best = Some((e, *d));
                }
            }
            if let Some((e, _)) = best {
                if let Some(band) = e.top_band(p.row_step) {
                    if band >= p.header_min_rows && band > self.header_rows {
                        let new_header = band.min(self.max_header_rows()).max(self.header_rows);
                        let body = self.frame_h - new_header;
                        if body >= p.header_min_rows * 2 {
                            let min_ov = (body / 8).max(16).min(body);
                            let max_d2 = (body.saturating_sub(min_ov)) as i64;
                            for d in 1..=max_d2 {
                                // 与主路径同纪律：先粗筛（行哈希）再精配 —— 不设门的话
                                // 低对比内容里 d±1 的错位候选能靠加权平均混进来。
                                if self.coarse_forward(frame, new_header, d) < p.coarse_hit_ratio {
                                    continue;
                                }
                                let e2 = self.eval_forward(frame, new_header, d);
                                if self.eval_pass(&e2) {
                                    self.header_rows = new_header; // 中途弹出的头部，此刻才采纳
                                    self.append_rows(frame, new_header, d);
                                    self.commit_frame(frame, hash);
                                    self.ambiguous_streak = 0;
                                    return PushOutcome::Appended {
                                        new_rows: d as usize,
                                    };
                                }
                            }
                        }
                    }
                }
            }
        }

        self.ambiguous_streak += 1;
        self.commit_frame(frame, hash);
        PushOutcome::Ambiguous
    }

    fn max_header_rows(&self) -> usize {
        ((self.frame_h as f32 * self.params.header_max_ratio) as usize)
            .min(self.frame_h.saturating_sub(1))
    }

    /// 头部采纳：只有匹配成功（有运动证据）时才允许 sticky 增长。
    fn adopt_header(&mut self, k: usize) {
        if k >= self.params.header_min_rows && k > self.header_rows {
            self.header_rows = k.min(self.max_header_rows());
        }
    }

    /// 本帧与上一帧帧顶连续"没变"的行数（上限 [`StitchParams::header_max_ratio`]）。
    fn static_top_rows(&self, frame: &[u8]) -> usize {
        let stride = self.w * 4;
        let cap = self.max_header_rows();
        let mut k = 0;
        while k < cap {
            let a = &frame[k * stride..(k + 1) * stride];
            let b = &self.last_frame[k * stride..(k + 1) * stride];
            if avg_row_diff(a, b) >= self.params.header_static_diff {
                break;
            }
            k += 1;
        }
        k
    }

    /// 精配是否通过：加权平均差达标 **且** 错配率不超限（防"少数结构性错配被
    /// 低权重稀释"，见模块头）。
    fn eval_pass(&self, e: &Eval) -> bool {
        e.avg < self.params.accept_avg_diff && e.mismatch_frac() <= self.params.max_mismatch_frac
    }

    /// 向下滚候选 `d` 的粗筛命中率：采样行哈希相等的占比。
    fn coarse_forward(&self, frame: &[u8], header: usize, d: i64) -> f32 {
        let stride = self.w * 4;
        let body_h = self.frame_h - header;
        let l = (body_h as i64 - d) as usize; // 重叠行数
        if l == 0 {
            return 0.0;
        }
        let step = self.params.row_step;
        let (mut hits, mut total) = (0usize, 0usize);
        let mut j = 0;
        while j < l {
            let f_row = &frame[(header + j) * stride..(header + j + 1) * stride];
            let c_row = &self.canvas
                [(self.canvas_h - l + j) * stride..(self.canvas_h - l + j + 1) * stride];
            if fnv64(f_row) == fnv64(c_row) {
                hits += 1;
            }
            total += 1;
            j += step;
        }
        if total == 0 {
            return 0.0;
        }
        hits as f32 / total as f32
    }

    /// 向下滚候选 `d` 的精配：重叠区逐行纹理加权 SAD + 逐采样行匹配位。
    fn eval_forward(&self, frame: &[u8], header: usize, d: i64) -> Eval {
        let stride = self.w * 4;
        let body_h = self.frame_h - header;
        let l = (body_h as i64 - d) as usize;
        if l == 0 {
            return Eval {
                avg: f32::INFINITY,
                samples: Vec::new(),
            };
        }
        let step = self.params.row_step.max(2);
        let (mut acc, mut wsum) = (0.0f32, 0.0f32);
        let mut samples = Vec::new();
        let mut j = 0;
        while j < l {
            let f_row = &frame[(header + j) * stride..(header + j + 1) * stride];
            let c_row = &self.canvas
                [(self.canvas_h - l + j) * stride..(self.canvas_h - l + j + 1) * stride];
            let diff = avg_row_diff(f_row, c_row);
            let w = row_texture(f_row, self.w);
            let weight = if (w as f32) < self.params.texture_low {
                self.params.flat_weight
            } else {
                1.0
            };
            acc += weight * diff;
            wsum += weight;
            samples.push((j, diff < self.params.accept_avg_diff));
            j += step;
        }
        let avg = if wsum <= 0.0 {
            f32::INFINITY
        } else {
            acc / wsum
        };
        Eval { avg, samples }
    }

    /// 停顿 / 回滚搜索：在画布尾部 `back_search_frames` 个视口高的范围里找
    /// "帧内容整体落点"。命中（错配率达标）返回视口顶偏移。
    fn search_contained(&self, frame: &[u8], header: usize) -> Option<usize> {
        let stride = self.w * 4;
        let body_h = self.frame_h - header;
        if body_h < self.params.header_min_rows * 2 || self.canvas_h < self.frame_h {
            return None;
        }
        // 视口顶 o = scroll ≤ scroll_prev = H - frame_h；向下滚时 o > H - frame_h，
        // 天然与 ① 的参数空间不相交。
        let o_min = self
            .canvas_h
            .saturating_sub(self.frame_h + self.frame_h * self.params.back_search_frames);
        let o_max = self.canvas_h - self.frame_h;
        let mut best: Option<(usize, f32)> = None; // (o, avg)
        for o in o_min..=o_max {
            // 粗筛：帧顶 body 行哈希 == 画布 o+header 行哈希 才值得整帧评估。
            let f0 = &frame[header * stride..(header + 1) * stride];
            let c0 = &self.canvas[(o + header) * stride..(o + header + 1) * stride];
            if fnv64(f0) != fnv64(c0) {
                continue;
            }
            let e = self.eval_contained(frame, header, o);
            if self.eval_pass(&e) && best.as_ref().is_none_or(|(_, a)| e.avg < *a) {
                best = Some((o, e.avg));
            }
        }
        best.map(|(o, _)| o)
    }

    /// 画布偏移 `o`（视口顶）的精配。
    fn eval_contained(&self, frame: &[u8], header: usize, o: usize) -> Eval {
        let stride = self.w * 4;
        let body_h = self.frame_h - header;
        let step = self.params.row_step.max(2);
        let (mut acc, mut wsum) = (0.0f32, 0.0f32);
        let mut samples = Vec::new();
        let mut j = 0;
        while j < body_h {
            let f_row = &frame[(header + j) * stride..(header + j + 1) * stride];
            let c_row = &self.canvas[(o + header + j) * stride..(o + header + j + 1) * stride];
            let diff = avg_row_diff(f_row, c_row);
            let w = row_texture(f_row, self.w);
            let weight = if (w as f32) < self.params.texture_low {
                self.params.flat_weight
            } else {
                1.0
            };
            acc += weight * diff;
            wsum += weight;
            samples.push((j, diff < self.params.accept_avg_diff));
            j += step;
        }
        let avg = if wsum <= 0.0 {
            f32::INFINITY
        } else {
            acc / wsum
        };
        Eval { avg, samples }
    }

    /// 把候选 `d` 的新行追加进画布：帧 [header+L, frame_h)。
    fn append_rows(&mut self, frame: &[u8], header: usize, d: i64) {
        let stride = self.w * 4;
        let body_h = self.frame_h - header;
        let l = (body_h as i64 - d) as usize;
        let from = (header + l) * stride;
        self.canvas.extend_from_slice(&frame[from..]);
        self.canvas_h += d as usize;
    }

    fn commit_frame(&mut self, frame: &[u8], hash: u64) {
        self.last_frame.clear();
        self.last_frame.extend_from_slice(frame);
        self.last_hash = hash;
    }
}

/// 一行的"纹理"：采样像素与左邻的水平梯度均值（0..=255）。纯色行 ≈ 0。
fn row_texture(row: &[u8], _w: usize) -> u32 {
    let step = 4 * 4; // 每 4 个像素采一个
    let mut acc = 0u64;
    let mut n = 0u64;
    let mut i = 4; // 从第 2 个采样点开始（有左邻可比）
    while i + 4 <= row.len() {
        // BGR 三通道差的绝对值之和（A 不参与）。各自升 u64 再加，理由同 `avg_row_diff`。
        let d = (row[i].abs_diff(row[i - 4]) as u64)
            + (row[i + 1].abs_diff(row[i - 3]) as u64)
            + (row[i + 2].abs_diff(row[i - 2]) as u64);
        acc += d;
        n += 1;
        i += step;
    }
    if n == 0 {
        return 0;
    }
    (acc / n) as u32
}

/// 两行的平均每像素差（0..=255，BGR 三通道均值）。
fn avg_row_diff(a: &[u8], b: &[u8]) -> f32 {
    let step = 4 * 4;
    let mut acc = 0u64;
    let mut n = 0u64;
    let mut i = 0;
    while i + 4 <= a.len().min(b.len()) {
        // 先各自升 u64 再相加：三通道差之和最大 765，u8 直加在 debug 下必溢出 panic
        // （release 静默回绕，均值也被污染）。
        let d = (a[i].abs_diff(b[i]) as u64)
            + (a[i + 1].abs_diff(b[i + 1]) as u64)
            + (a[i + 2].abs_diff(b[i + 2]) as u64);
        acc += d;
        n += 1;
        i += step;
    }
    if n == 0 {
        return 0.0;
    }
    (acc / n / 3) as f32
}

/// BGRA 帧转置：`(w, h)` 的图像宽高互换（像素连同字节原样搬，通道序不变）。
///
/// **宽截图的全部算法复用就在这一步**：行拼接器只会"沿行方向长"，把每一帧转置后喂进去，
/// "向右滚"就变成了"向下滚"——行哈希、纹理加权、粘性头部（左侧固定栏转置后成了顶栏）、
/// 画布偏移搜索全部原样成立；拼完再转置一次即得横向长图。`transpose∘transpose = id`，
/// 往返无损（逐字节搬运，无插值）。
///
/// 返回 `(new_w, new_h, data)`，`new_w = h`、`new_h = w`。
pub fn transpose_bgra(w: i32, h: i32, bgra: Vec<u8>) -> (i32, i32, Vec<u8>) {
    if w <= 0 || h <= 0 {
        return (w, h, bgra);
    }
    let w = w as usize;
    let h = h as usize;
    let mut out = vec![0u8; bgra.len()];
    // 源像素 (x=ny, y=nx) 在 `src[(nx*w+ny)*4]`；转置后新图宽 = h，落在
    // `out[(ny*h+nx)*4]`。对源做跨行列访问——缓存不友好但每像素只 4 字节，
    // 3 MB 一帧的量级实测无感；换成分块会引入 8 种边角特例，不值得。
    for nx in 0..h {
        for ny in 0..w {
            let src = (nx * w + ny) * 4;
            let dst = (ny * h + nx) * 4;
            out[dst..dst + 4].copy_from_slice(&bgra[src..src + 4]);
        }
    }
    (h as i32, w as i32, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 60;
    const FH: u32 = 40;

    /// 转置往返无损（宽截图的算法复用前提）：`transpose∘transpose = id`，且一次转置的
    /// 尺寸/内容映射逐像素正确（每像素用可区分的 (x,y) 编码，任何映射错误都会现形）。
    #[test]
    fn transpose_round_trips_losslessly() {
        let (w, h) = (5i32, 3i32);
        let src: Vec<u8> = (0..(w * h * 4)).map(|i| (i % 251) as u8).collect();
        let (tw, th, trans) = transpose_bgra(w, h, src.clone());
        assert_eq!((tw, th), (h, w), "宽高互换");
        assert_eq!(trans.len(), src.len());
        // 逐像素对账：src(x,y) 四字节 == trans(y,x) 四字节。
        for y in 0..h as usize {
            for x in 0..w as usize {
                let s = &src[(y * w as usize + x) * 4..][..4];
                let t = &trans[(x * h as usize + y) * 4..][..4];
                assert_eq!(s, t, "像素 ({x},{y}) 转置错位");
            }
        }
        let (bw, bh, back) = transpose_bgra(tw, th, trans);
        assert_eq!((bw, bh, back), (w, h, src), "二次转置必须还原");
        // 退化输入：非正尺寸原样返回（管线层不该遇到，但拒绝做蠢事）。
        assert_eq!(transpose_bgra(0, 5, vec![]), (0, 5, vec![]));
    }

    /// 造一帧：顶部 `header` 行纯色（粘性头部），下方是"页面第 page 行"的
    /// 渐变内容（每行不同 → 行哈希可区分）。`page_top` = 视口顶对应的页面行号。
    fn frame(page_top: i32, header: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity((W * FH * 4) as usize);
        for r in 0..FH as usize {
            for c in 0..W {
                if r < header {
                    let px = [30u8, 90, 200, 255]; // 头部纯色（BGRA）
                    v.extend_from_slice(&px);
                } else {
                    let page_row = page_top + r as i32;
                    // 相邻页面行的差必须远超 accept_avg_diff（10）：否则错位一行的
                    // 候选也能过精配（测试曾用系数 7/13/5，平均行差 8.3，被 d±1 钻空子）。
                    let b = (page_row * 40 + c as i32 * 3) as u8;
                    let g = (page_row * 61 + 11) as u8;
                    let rr = (page_row * 25 + c as i32) as u8;
                    v.extend_from_slice(&[b, g, rr, 255]);
                }
            }
        }
        v
    }

    /// 页面 `page_row` 行的像素（供画布内容断言）。
    fn page_px(page_row: i32, c: i32) -> [u8; 4] {
        [
            (page_row * 40 + c * 3) as u8,
            (page_row * 61 + 11) as u8,
            (page_row * 25 + c) as u8,
            255,
        ]
    }

    #[test]
    fn seed_and_stepwise_scroll_produce_exact_canvas() {
        let mut s = ScrollStitcher::new(W, FH);
        assert_eq!(
            s.push(&frame(0, 0)),
            PushOutcome::Appended {
                new_rows: FH as usize
            }
        );
        // 每步下滚 7 行：第 n 帧的视口顶 = 7n。
        for n in 1..=6usize {
            let out = s.push(&frame((7 * n) as i32, 0));
            assert_eq!(out, PushOutcome::Appended { new_rows: 7 }, "第 {n} 步");
        }
        assert_eq!(s.canvas_h(), FH as usize + 6 * 7, "画布 = 首帧 + Σd");
        // 画布内容逐行等于页面。
        let (w, h, bgra) = s.take_canvas();
        assert_eq!((w, h), (W as usize, FH as usize + 42));
        for r in 0..h {
            for c in [0usize, 30, 59] {
                let px = &bgra[(r * w + c) * 4..][..4];
                assert_eq!(
                    px,
                    page_px(r as i32, c as i32),
                    "画布第 {r} 行第 {c} 列应等于页面"
                );
            }
        }
    }

    #[test]
    fn take_bitmap_converts_bgra_to_rgba() {
        let mut s = ScrollStitcher::new(1, 2);
        let mut f = vec![0u8; 8];
        // BGRA: B=1 G=2 R=3 A=4 → RGBA 应为 3,2,1,4。
        f.copy_from_slice(&[1, 2, 3, 4, 1, 2, 3, 4]);
        s.push(&f);
        let bmp = s.take_bitmap().expect("画布应能转位图");
        assert_eq!((bmp.w, bmp.h), (1, 2));
        assert_eq!(bmp.rgba, vec![3, 2, 1, 4, 3, 2, 1, 4]);
    }

    #[test]
    fn identical_frame_is_rejected_by_the_gate() {
        let mut s = ScrollStitcher::new(W, FH);
        let f = frame(0, 0);
        assert_eq!(
            s.push(&f),
            PushOutcome::Appended {
                new_rows: FH as usize
            }
        );
        assert_eq!(s.push(&f), PushOutcome::NoMotion, "同帧 = 帧闸门拒收");
        assert_eq!(s.canvas_h(), FH as usize, "画布不涨");
    }

    #[test]
    fn scroll_back_into_captured_region_reports_no_new_content() {
        let mut s = ScrollStitcher::new(W, FH);
        s.push(&frame(0, 0));
        s.push(&frame(20, 0));
        let h = s.canvas_h();
        // 回滚 8 行：视口顶 = 12，内容完全在已捕获范围内。
        assert_eq!(s.push(&frame(12, 0)), PushOutcome::NoNewContent);
        assert_eq!(s.canvas_h(), h, "回滚不改画布");
        // 停在原地（视口顶不变）也是 NoNewContent。
        assert_eq!(s.push(&frame(12, 0)), PushOutcome::NoMotion);
        assert_eq!(s.push(&frame(13, 0)), PushOutcome::NoNewContent);
    }

    #[test]
    fn sticky_header_is_detected_and_excluded() {
        let mut s = ScrollStitcher::new(W, FH);
        s.push(&frame(0, 8)); // 首帧带 8 行粘性头部
                              // 顶部 8 行恒为纯色 → 头部检测应在第二步就吃掉它。
        let out = s.push(&frame(6, 8));
        assert_eq!(out, PushOutcome::Appended { new_rows: 6 });
        assert_eq!(s.header_rows(), 8, "头部行数应被检出");
        assert_eq!(s.canvas_h(), FH as usize + 6);
        // 继续滚：对齐不因头部漂移。
        s.push(&frame(14, 8));
        assert_eq!(s.canvas_h(), FH as usize + 6 + 8);
    }

    #[test]
    fn pure_color_body_with_unique_footer_still_matches() {
        // 页面主体全同色（多解陷阱），页面第 30 行起是唯一纹理 —— 错误的 d 会在
        // 纹理区对不上，正确的 d 被纹理行锁定；纯色行经纹理降权后不再投票。
        let mk = |page_top: i32| -> Vec<u8> {
            let mut v = Vec::with_capacity((W * FH * 4) as usize);
            for r in 0..FH as usize {
                for c in 0..W as i32 {
                    let page_row = page_top + r as i32;
                    if page_row >= 30 {
                        let b = (page_row * 17 + c * 5) as u8;
                        v.extend_from_slice(&[b, 250, 250, 255]);
                    } else {
                        v.extend_from_slice(&[250, 250, 250, 255]);
                    }
                }
            }
            v
        };
        let mut s = ScrollStitcher::new(W, FH);
        s.push(&mk(0));
        let out = s.push(&mk(9));
        assert_eq!(
            out,
            PushOutcome::Appended { new_rows: 9 },
            "纯色主体 + 纹理 footer 应锁定正确 d"
        );
        assert_eq!(s.canvas_h(), FH as usize + 9);
    }

    #[test]
    fn ambiguous_frame_is_skipped_without_corrupting_the_canvas() {
        let mut s = ScrollStitcher::new(W, FH);
        s.push(&frame(0, 0));
        let h = s.canvas_h();
        // 甩动 200 行：超出 max_d（body 85% = 34 行），找不到重叠。
        let out = s.push(&frame(200, 0));
        assert_eq!(out, PushOutcome::Ambiguous);
        assert_eq!(s.canvas_h(), h, "画布原样保留");
        assert_eq!(s.ambiguous_streak, 1);
        assert_eq!(s.push(&frame(201, 0)), PushOutcome::Ambiguous);
        assert_eq!(s.ambiguous_streak, 2);
    }

    #[test]
    fn mid_session_toolbar_gets_learned_as_header() {
        // 工具栏**中途弹出**：前两帧无头部，第三帧顶部 6 行变纯色工具条。
        // 常规匹配因顶部整段错配失败 → 顶部错配带恢复路径把它学成头部。
        let mut s = ScrollStitcher::new(W, FH);
        s.push(&frame(0, 0));
        s.push(&frame(10, 0));
        assert_eq!(s.canvas_h(), FH as usize + 10);
        let out = s.push(&frame(16, 6));
        assert_eq!(
            out,
            PushOutcome::Appended { new_rows: 6 },
            "工具栏帧应照常追加"
        );
        assert!(
            s.header_rows() >= 6,
            "工具栏应被学成头部（实际 {}）",
            s.header_rows()
        );
        assert_eq!(s.canvas_h(), FH as usize + 16);
        // 后续帧对齐不受影响。
        let out = s.push(&frame(26, 6));
        assert_eq!(out, PushOutcome::Appended { new_rows: 10 });
        assert_eq!(s.canvas_h(), FH as usize + 26);
    }

    #[test]
    fn wrong_sized_frame_is_rejected_not_panic() {
        let mut s = ScrollStitcher::new(W, FH);
        assert_eq!(s.push(&[0u8; 10]), PushOutcome::Ambiguous);
        assert!(!s.seeded());
    }
}
