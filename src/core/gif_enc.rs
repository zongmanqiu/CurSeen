//! GIF 录制侧：调色板量化 + 伽马正确降采样 + 帧封装。
//!
//! 解码在 [`crate::core::gif`]（悬浮标「图片」动画用），两者互不相关。
//!
//! # 为什么复用 `gif` crate 的编码器，而不是自己写 LZW
//!
//! 一开始的直觉是"自己写更轻"，写完才发现方向错了 —— **`gif` crate 本来就已经在依赖里**
//! （悬浮标要解 GIF 动图）。它的编码器走 `weezl` 的 LZW，那部分代码无论我们写不写都在
//! 编译单元里。自己再手写一份 LZW 的净收益 = 0 字节体积 + 一份需要长期维护、且极难门禁
//! 的码位逻辑（GIF 的 LZW 有个著名坑：码长增长点是**编码器早于解码器**一步，不吐 Clear
//! 码的解码器会溢出）。所以这里只写"这个 crate 不提供、而我们真正需要"的部分：
//!
//! 1. **调色板**：`gif` 要求你给全局色表，怎么从画面选色是它不管的。屏幕录制内容
//!    颜色高度重复，这里有个关键优化 —— 颜色数 ≤256 时**直接用精确色**（等于无损）。
//! 2. **降采样**：录制要出 720p/480p 这类档位，且必须在**线性光**里平均（见下）。
//! 3. **量化缓存**：2M 像素逐点搜 256 色的最近邻是每帧 5 亿次距离计算，来不及。
//!
//! 这三件都是**纯函数**，于是不必开 GPU、不必录一段真屏幕，就能把正确性钉在门禁上。

use std::io::{self, BufWriter};
use std::path::Path;

/// GIF 调色板上限（格式决定：全局色表最大 256 色）。
pub const MAX_COLORS: usize = 256;

/// 帧延时的下限（厘秒，1cs = 10ms）。
///
/// GIF 原生分辨率就是 10ms，但**浏览器与多数看图器把 <2cs 当作"未指定"按 10cs(100ms) 播**，
/// 于是设 1cs 会得到反直觉的"放慢十倍"。要 10ms/帧就得接受这个小瑕疵，故统一抬到 2cs
/// —— 上限即 50fps，GIF 录制远够用。
pub const MIN_DELAY_CS: u16 = 2;

/// 首帧"不同颜色数"不超过这个值时，走**真色路径**（见 [`Palette::from_hist`]）。
///
/// # 为什么不是一个很小的数
///
/// 画面色位只有 ~217 个，但真实内容的颜色数落在 218…4096 之间**很常见**
/// （带抗锯齿文字的界面、终端、图表）。这些画面若一律退回"桶均值"，每个颜色都会被
/// 抹到 5 位格子的均值上（±4）；而真色是逐像素精确的。抬到 4096 之后，
/// 这类画面拿到的仍是**真实像素色**，只是被中位切分合并到 217 色。
///
/// 代价只有一次带早退的扫描：照片在第 ~4097 种颜色处就退出（只扫几千像素）；
/// 真正扫完整帧的只有"颜色本来就少"的画面，那正是它该付的成本。
const EXACT_CAP: usize = 4096;

/// 覆盖层特效（拖尾 / 点击效果 / 光标）的**保色底**：写进全局色表最前面的一组颜色。
///
/// # 为什么必须有
///
/// 调色板只从**首帧**选色，而拖尾是**之后才出现**的 —— 首帧那一刻鼠标多半静止，画面里
/// 根本没有那个颜色，于是黄色拖尾被量化到最近的灰。用户看到的就是
/// 「黄色的鼠标拖尾，在 GIF 里变成灰色」（2026-09-20 报告）。
///
/// # 为什么是"色相环"而不是"从那几个设置里取色"
///
/// 拖尾有 `rainbow` 模式，它会产生**任意色相** —— 没有有限个"要保的颜色"可枚举；而每个
/// 拖尾脚本各带一个 `fixed_color`，还会随用户增删脚本而变。钉死一圈色相则与配置无关，
/// 也不必把设置从 `Runtime` 一路搬进 [`crate::platform::recorder::RecOptions`]。
///
/// # 为什么是三档而不是一档
///
/// 拖尾是**半透明**的，DXGI 抓到的是它与桌面混合后的颜色：
///   · 压在深色背景上 → 明度被拉低（≈V 0.5）
///   · 压在浅色背景上 → 饱和度被拉低（≈S 0.5）
/// 只给"满饱和满明度"的一圈，上面两种情况都会量化偏色。所以色相环取三档：
/// 满饱和亮色 / 半明度 / 半饱和（pastel）。
///
/// 代价是 39 个色位（约占全局色表的 15%），换来"任何纯色特效都不会退化成灰"。
/// 不用固定色立方（6×6×6 = 216 色）是因为那会把画面本身的精确色挤到只剩 40 槽，
/// 而"颜色数 ≤256 时精确无损"正是本编码器对代码 / PPT / 演示画面的主要优势。
pub fn accent_colors() -> Vec<[u8; 3]> {
    /// 色相环的档数。12 档 = 每 30° 一档，**正好含 60°**，于是纯黄 `#FFFF00` 是精确命中。
    const HUES: u32 = 12;
    let mut out: Vec<[u8; 3]> = Vec::with_capacity(HUES as usize * 3 + 3);
    for (s, v) in [(1.0f32, 1.0f32), (1.0, 0.5), (0.5, 1.0)] {
        for step in 0..HUES {
            out.push(hsv(step as f32 * 360.0 / HUES as f32, s, v));
        }
    }
    out.push([255, 255, 255]);
    out.push([0, 0, 0]);
    out.push([128, 128, 128]);
    out
}

/// HSV → RGB（`h` 单位度，`s`/`v` ∈ [0,1]）。色相环只是常量表，不为它引第三方库。
fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

// ─────────────────────────── 调色板 ───────────────────────────

/// 一张 GIF 的全局调色板。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    colors: Vec<[u8; 3]>,
    /// 色表位深（1..=8）。色表字节数 = 3 × 2^bits（`gif` crate 要求长度是 2 的幂）。
    bits: u8,
}

impl Palette {
    /// 由一组颜色建调色板（去重、截断到 [`MAX_COLORS`]）。空输入返回 `None`。
    pub fn from_colors<'a, I: IntoIterator<Item = &'a [u8; 3]>>(colors: I) -> Option<Self> {
        let mut uniq: Vec<[u8; 3]> = Vec::new();
        for c in colors {
            if uniq.len() >= MAX_COLORS {
                break;
            }
            if !uniq.contains(c) {
                uniq.push(*c);
            }
        }
        if uniq.is_empty() {
            return None;
        }
        let bits = bits_for(uniq.len());
        Some(Self { colors: uniq, bits })
    }

    /// 调色板的**唯一入口**（录制 / 单帧 / 带保色底，全走这条）。
    ///
    /// * `hist` —— 可能**跨多帧累积**过的全像素直方图。录制时它攒的是"预热窗口"里的
    ///   所有帧（见 [`crate::platform::recorder`] 的 `PALETTE_WARMUP_FRAMES`）。
    /// * `first_frame` —— 用来做**真色**判定：这一帧的不同颜色数 ≤ [`EXACT_CAP`] 时，
    ///   直接用画面里**真实存在**的颜色建表（≤ 预算色数 ⇒ 等于**无损**）。
    /// * `accents` —— 保色底（[`accent_colors`]），先占位。
    ///
    /// # 三条路径，各有各的道理
    ///
    /// **① 真色路径**（首帧颜色数 ≤ [`EXACT_CAP`]）：色位给画面里真有的颜色。
    /// 装得下就完全精确（代码 / PPT / 演示画面等于无损）；装不下就在**真色**上做中位
    /// 切分。它比"桶均值"准一档 —— 桶均值只是一格 8×8×8 里的平均色。
    /// 剩下的色位再从直方图按**出现次数**补最显眼的，这一步同时解决了
    /// 「开始之后才出现的颜色」：拖尾、弹窗、光标都在这里进来。
    ///
    /// **② 丰富路径**（壁纸 / 照片 / 视频）：加权中位切分，色位按实际**面积**分配。
    /// 此时首帧没有特殊地位 —— 只看整段预热窗口统计出来的颜色分布。
    ///
    /// ★ 顺序很重要：保色底**先占位**，否则画面把槽吃干之后它们一个也进不来。
    pub fn from_hist(
        first_frame: Option<&[u8]>,
        hist: &Hist,
        max_colors: usize,
        accents: &[[u8; 3]],
    ) -> Option<Self> {
        let max_colors = max_colors.clamp(2, MAX_COLORS);
        if hist.is_empty() {
            return None;
        }
        let mut all: Vec<[u8; 3]> = Vec::with_capacity(max_colors);
        for c in accents {
            if all.len() >= max_colors {
                break;
            }
            if !all.contains(c) {
                all.push(*c);
            }
        }
        // 至少给画面留 2 个色位（与 `from_rgb` 的钳位口径一致）。
        let budget = max_colors.saturating_sub(all.len()).max(2);
        let occupied = hist.occupied();

        // ① 真色路径。
        if let Some(mut exact) = first_frame.and_then(|f| unique_colors_exact(f, EXACT_CAP)) {
            exact.sort_unstable();
            // ★ 加权（2026-10-08 用户报「GIF 录 PPT 背景颜色对不上」的次因）：
            //   颜色数超预算时按**像素数**加权切分。等权会让占屏 90% 的背景色和一条
            //   抗锯齿边平起平坐，被切进同一个盒子后平均掉几个灰阶；加权后背景色
            //   靠体量自成一盒，代表色就是它自己。
            let base: Vec<[u8; 3]> = if exact.len() <= budget {
                exact.iter().map(|(c, _)| *c).collect()
            } else {
                median_cut_weighted(&exact, budget)
            };
            // 已经被选中的颜色"占住"的桶不再重复喂进来：桶均值与它肉眼等价，
            // 让它进来只会白占一个色位。
            let mut claimed: std::collections::HashSet<usize> =
                base.iter().map(|c| bin_index(c[0], c[1], c[2])).collect();
            for c in base {
                if all.len() >= max_colors {
                    break;
                }
                if !all.contains(&c) {
                    all.push(c);
                }
            }
            let mut rest = occupied;
            rest.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
            for (c, _) in rest {
                if all.len() >= max_colors {
                    break;
                }
                if !claimed.insert(bin_index(c[0], c[1], c[2])) || all.contains(&c) {
                    continue;
                }
                all.push(c);
            }
            return Self::from_colors(all.iter());
        }

        // ② 丰富路径。
        for c in median_cut_weighted(&occupied, budget) {
            if all.len() >= max_colors {
                break;
            }
            if !all.contains(&c) {
                all.push(c);
            }
        }
        Self::from_colors(all.iter())
    }

    /// 从**一帧** RGB 字节流（长度 3n）取一张调色板。
    ///
    /// 颜色数 ≤ `max_colors` 时返回**精确色**（对 PPT / 代码 / 演示这类画面等于无损）；
    /// 否则走全像素直方图 + 加权中位切分（见 [`Self::from_hist`]）。
    pub fn from_rgb(rgb: &[u8], max_colors: usize) -> Option<Self> {
        let mut hist = Hist::new();
        hist.add_rgb(rgb);
        Self::from_hist(Some(rgb), &hist, max_colors, &[])
    }

    /// 与 [`Self::from_rgb`] 相同，但先钉住一组**保色底**（[`accent_colors`]），
    /// 画面只拿剩下的色位。
    pub fn from_rgb_with_accents(
        rgb: &[u8],
        max_colors: usize,
        accents: &[[u8; 3]],
    ) -> Option<Self> {
        let mut hist = Hist::new();
        hist.add_rgb(rgb);
        Self::from_hist(Some(rgb), &hist, max_colors, accents)
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    /// 由**一份现成的色表字节**（长度 3 × 2^bits）建调色板。
    ///
    /// ★ 与 [`Self::from_colors`] 的关键区别：**去重与截断一律不做**。GIF 的每个像素存的
    ///   是**色表下标**，掉一个表项或换一次顺序都会让整张图颜色错位。
    ///   裁剪路径（[`crate::core::gif_trim`]）拿的是别人写好的色表，必须原样搬回去，
    ///   所以走这一条；`from_colors` 只适合"我们自己从画面里选色"的录制路径。
    pub fn from_table(bytes: &[u8]) -> Option<Self> {
        let n = bytes.len() / 3;
        // 色表长度必须是 2 的幂（GIF 格式规定，`gif` crate 也按这个校验）。
        if n == 0 || n > MAX_COLORS || (n & (n - 1)) != 0 || bytes.len() != n * 3 {
            return None;
        }
        let colors: Vec<[u8; 3]> = bytes.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        Some(Self {
            colors,
            bits: bits_for(n),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }

    /// 追加**一个专用槽位**并返回它的下标 —— 给"透明下标"留位。
    ///
    /// # 为什么必须新开一槽，而不是复用源文件声明过的那个透明下标
    ///
    /// GIF 的透明是**逐帧**语义：同一个下标可以在这一帧算透明、在另一帧当实色画。裁剪要写的
    /// 是"整段合成后的画面"，如果直接复用源里的透明下标，遇到"另一帧拿它画过实色"就会在产物
    /// 里挖出洞，而且是**看不出来的那种**（只有那几个像素变透明）。
    ///
    /// 新槽的颜色从不参与渲染（用到它的像素永远是透明的），所以取什么色都无所谓 —— 这里
    /// 复制 0 号色，保证**只改色表长度、不改任何既有下标**，既有像素的映射一个都不动。
    ///
    /// 色表已是 [`MAX_COLORS`] 时返回 `None`（没法再扩），调用方需退让（见 `rewrite_to`）。
    pub fn with_extra_slot(&self) -> Option<(Self, u8)> {
        if self.colors.is_empty() || self.colors.len() >= MAX_COLORS {
            return None;
        }
        let mut colors = self.colors.clone();
        let idx = colors.len() as u8;
        colors.push(colors[0]);
        let bits = bits_for(colors.len());
        Some((Self { colors, bits }, idx))
    }

    pub fn bits(&self) -> u8 {
        self.bits
    }

    pub fn colors(&self) -> &[[u8; 3]] {
        &self.colors
    }

    /// `gif::Encoder` 要的全局色表：3 × 2^bits 字节，尾部补 0。
    pub fn table_bytes(&self) -> Vec<u8> {
        let want = 1usize << self.bits;
        let mut out = Vec::with_capacity(want * 3);
        for c in &self.colors {
            out.extend_from_slice(c);
        }
        out.resize(want * 3, 0);
        out
    }

    /// 最近色，在 **OKLab** 里比距离（理由见 [`oklab`]）。平局取下标小的 → 结果确定。
    ///
    /// ★ 这是**冷路径 / 测试**用的便捷入口：每次调用都要把整张色表转一遍（256 次）。
    ///   逐像素的热点路径必须走 [`Quantizer`] —— 它在 `new` 里把色表转好一次。
    pub fn nearest(&self, r: u8, g: u8, b: u8) -> u8 {
        let lab: Vec<[f32; 3]> = self
            .colors
            .iter()
            .map(|c| oklab(c[0], c[1], c[2]))
            .collect();
        nearest_lab(&lab, r, g, b)
    }
}

/// 装得下 `n` 种颜色的最小位深（1..=8）。
fn bits_for(n: usize) -> u8 {
    let mut bits = 1u8;
    while (1usize << bits) < n && bits < 8 {
        bits += 1;
    }
    bits
}

// ─────────────────────── 颜色直方图（全像素统计） ───────────────────────

/// 15 位颜色直方图（每通道 5 bit，共 32768 个桶）。
///
/// # ★★ 为什么不能"隔 N 个像素抽一个"（2026-09-20 修掉的真 bug）
///
/// 抽样的步长只有一种算法：`stride = n / cap`。而它与画面**宽度**未必互素。实拍参数：
/// 1920×1080、`cap = 256 × 64 = 16384` ⇒ `stride = 126`，而 `gcd(126, 1920) = 6`
/// ⇒ 被采到的**列**只剩 `x ≡ 0 (mod 6)`，**5/6 的列从未进入色表**。
/// 一条细竖带（拖尾、边框、进度条）正好落在 `x ≡ 4` 那一列时，它的颜色在调色板里
/// **根本不存在** —— 用户看到的是「颜色时对时不对，取决于它在屏幕的哪个位置」，
/// 而且缺陷随**选区宽度**漂移（换显示器、换框选尺寸又变），几乎无法复现。
/// 全像素统计没有这个问题，代价只是每像素一次加法。
///
/// # 为什么同时记"和"
///
/// 桶只是 5 位的粗格子，桶里所有像素的**真实均值**才是要喂给中位切分的颜色。
/// 只记计数的话代表色只能退化成格子中心（±4 的误差），细渐变会被切成阶梯。
pub struct Hist {
    /// 每个桶 = `[计数, R 和, G 和, B 和]`。
    bins: Vec<[u32; 4]>,
    total: u64,
}

impl Default for Hist {
    fn default() -> Self {
        Self::new()
    }
}

impl Hist {
    pub fn new() -> Self {
        Self {
            bins: vec![[0; 4]; 1 << 15],
            total: 0,
        }
    }

    /// 把整帧 RGB（3n 字节）统计进来。**不做任何抽点**。
    pub fn add_rgb(&mut self, rgb: &[u8]) {
        for px in rgb.chunks_exact(3) {
            let b = &mut self.bins[bin_index(px[0], px[1], px[2])];
            b[0] += 1;
            b[1] += px[0] as u32;
            b[2] += px[1] as u32;
            b[3] += px[2] as u32;
            self.total += 1;
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// 占用桶的（桶内真实均值, 像素数）。均值比桶中心准得多 —— 细渐变靠它保住。
    pub fn occupied(&self) -> Vec<([u8; 3], u32)> {
        let mut out = Vec::new();
        for b in &self.bins {
            let n = b[0];
            if n == 0 {
                continue;
            }
            let half = n / 2;
            out.push((
                [
                    ((b[1] + half) / n) as u8,
                    ((b[2] + half) / n) as u8,
                    ((b[3] + half) / n) as u8,
                ],
                n,
            ));
        }
        out
    }
}

/// 每通道 5 bit 的桶下标（与 [`Quantizer`] 的缓存同构）。
#[inline]
fn bin_index(r: u8, g: u8, b: u8) -> usize {
    (((r as usize) >> 3) << 10) | (((g as usize) >> 3) << 5) | ((b as usize) >> 3)
}

/// 全像素扫描，收集**精确**颜色及其像素数；一旦超过 `max` 种立刻放弃（返回 `None`）。
///
/// 有早退：照片 / 视频 / 壁纸在头几千个像素就撞上第 `max+1` 种颜色，几乎零成本。
/// 只有"本来就 ≤256 色"的画面（代码 / PPT / 演示）才会扫完整帧 —— 那种画面下集合极小、
/// 每次查表常数时间，1080p 一帧几十毫秒，而**整段录制只付一次**。
/// 换来的是这类画面**精确无损**（见 [`Palette::from_hist`] 的①）。
/// 像素数一并返回：颜色数超预算时喂给加权中位切分，保住占屏大头的背景色。
fn unique_colors_exact(rgb: &[u8], max: usize) -> Option<Vec<([u8; 3], u32)>> {
    use std::collections::HashMap;
    let mut seen: HashMap<u32, ([u8; 3], u32)> = HashMap::with_capacity(64);
    for px in rgb.chunks_exact(3) {
        let key = ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | px[2] as u32;
        match seen.get_mut(&key) {
            Some(e) => e.1 += 1,
            None => {
                if seen.len() > max {
                    return None;
                }
                seen.insert(key, ([px[0], px[1], px[2]], 1));
            }
        }
    }
    Some(seen.into_values().collect())
}

/// 加权中位切分：输入是（代表色, 权重），盒子按**权重中位**切开，代表色取加权均值。
///
/// # 为什么必须加权
///
/// 等权切分让"只出现几个像素的颜色"和"占满半屏的颜色"**平起平坐**：于是大片渐变被
/// 糊掉，而噪声 / 抗锯齿边角反而占走色位。按像素数加权之后，色位按用户**实际看到的
/// 面积**分配 —— 这是画质好坏的关键一步。
fn median_cut_weighted(entries: &[([u8; 3], u32)], max: usize) -> Vec<[u8; 3]> {
    if entries.is_empty() || max == 0 {
        return Vec::new();
    }
    struct Bx {
        idx: Vec<u32>,
        w: u64,
    }
    let mut boxes = vec![Bx {
        idx: (0..entries.len() as u32).collect(),
        w: entries.iter().map(|e| e.1 as u64).sum(),
    }];
    while boxes.len() < max {
        let mut best: Option<(usize, usize, u16)> = None;
        for (bi, b) in boxes.iter().enumerate() {
            if b.idx.len() < 2 {
                continue;
            }
            for ch in 0..3 {
                let (mut lo, mut hi) = (255u8, 0u8);
                for &i in &b.idx {
                    let c = entries[i as usize].0[ch];
                    lo = lo.min(c);
                    hi = hi.max(c);
                }
                let range = (hi - lo) as u16;
                let better = match best {
                    None => true,
                    Some((_, _, br)) => range > br,
                };
                if better {
                    best = Some((bi, ch, range));
                }
            }
        }
        let Some((bi, ch, range)) = best else { break };
        if range == 0 {
            break; // 所有盒子都退化成单色，再切没有意义
        }
        let mut b = boxes.swap_remove(bi);
        b.idx.sort_unstable_by_key(|&i| entries[i as usize].0[ch]);
        // 按**权重中位**切：从前往后累加像素数，累到一半就下刀。
        let half = b.w / 2;
        let mut acc = 0u64;
        let mut cut = 0usize;
        for (n, &i) in b.idx.iter().enumerate() {
            acc += entries[i as usize].1 as u64;
            if acc >= half {
                cut = n + 1;
                break;
            }
        }
        let cut = cut.clamp(1, b.idx.len() - 1);
        let right = b.idx.split_off(cut);
        let wl: u64 = b.idx.iter().map(|&i| entries[i as usize].1 as u64).sum();
        let wr: u64 = right.iter().map(|&i| entries[i as usize].1 as u64).sum();
        boxes.push(Bx { idx: b.idx, w: wl });
        boxes.push(Bx { idx: right, w: wr });
    }
    boxes
        .iter()
        .filter(|b| !b.idx.is_empty())
        .map(|b| {
            let mut sum = [0f64; 3];
            let mut w = 0f64;
            for &i in &b.idx {
                let (c, n) = entries[i as usize];
                let n = n as f64;
                for ch in 0..3 {
                    sum[ch] += c[ch] as f64 * n;
                }
                w += n;
            }
            let w = w.max(1.0);
            [
                (sum[0] / w).round() as u8,
                (sum[1] / w).round() as u8,
                (sum[2] / w).round() as u8,
            ]
        })
        .collect()
}

// ─────────────────────── 感知色差（OKLab） ───────────────────────

/// sRGB 字节 → OKLab（Ottosson 2020 的拟合式）。
///
/// # 为什么不能直接在 sRGB 里比欧氏距离
///
/// 三条，都不是"看起来更高级"：
///  · sRGB 字节值**不是感知均匀的** —— 同样 30 级的差，暗部一眼能看出、亮部几乎看不出；
///  · 直接对 RGB 求欧氏距离把三通道**等权**，于是"亮度差很多、色相相近"经常赢过
///    "亮度差不多、色相偏了"。前者把渐变的暗部糊平、后者正是用户抱怨的
///    「颜色都不太对」；
///  · 它还会让**抗锯齿边缘**（拖尾那圈 1px）跳到"与背景同亮度、但色相完全不同"的
///    色表项上 —— 用户看到的就是「纯色飘带多了一圈边框」。
///
/// 热点路径不重复转换：调色板那 256 个色的 OKLab 在 [`Quantizer::new`] 里算一次，
/// 像素侧则由 15 位缓存兜住（见 [`Quantizer`]）。
pub fn oklab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let lut = srgb_to_linear_lut();
    let (r, g, b) = (lut[r as usize], lut[g as usize], lut[b as usize]);
    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// sRGB 字节 → 线性光（0..1）。256 项查表，把 `powf` 从热点路径里拿掉。
fn srgb_to_linear_lut() -> &'static [f32; 256] {
    static LUT: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = [0f32; 256];
        for (i, v) in t.iter_mut().enumerate() {
            let s = i as f32 / 255.0;
            *v = if s <= 0.040_45 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            };
        }
        t
    })
}

/// 在**已转好**的 OKLab 调色板里找最近色。平局取下标小的 → 结果确定。
pub fn nearest_lab(lab: &[[f32; 3]], r: u8, g: u8, b: u8) -> u8 {
    let p = oklab(r, g, b);
    let mut best = 0usize;
    let mut best_d = f32::MAX;
    for (i, q) in lab.iter().enumerate() {
        let dl = p[0] - q[0];
        let da = p[1] - q[1];
        let db = p[2] - q[2];
        let d = dl * dl + da * da + db * db;
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best as u8
}

// ─────────────────────────── 量化 ───────────────────────────

/// RGB → 调色板下标，带 15-bit 直方缓存 + 预转好的 OKLab 色表 + **精确色快路径**。
///
/// 无缓存时每像素要在 ≤256 色里线性搜一遍：1080p 每帧约 5 亿次距离计算，来不及。
/// 缓存键把每通道压到 5 bit（32³ = 32768 格），首帧建满后命中率极高。
///
/// ★ 缓存查的是**桶中心**的颜色，不是"落进该桶的第一个像素"的颜色：桶内两个像素
///   真实的最近色可能不同（5 bit = 每通道 ±4 的模糊）。按桶中心查，映射就**与像素
///   到达顺序无关**，误差也最小。
///
/// ★★ 精确色快路径（2026-10-08 用户报「GIF 录 PPT 背景颜色对不上」）：像素颜色
///   **恰好就在调色板里**时直接用它，不过桶。桶中心查找在浅色区会把 248..255 一格的
///   中心 252 判给**白色保色底**而不是画面自己的 248 —— 整片 #F8F8F8 的背景被弹成
///   纯白（离线推演 + 判据 `a_color_present_in_the_palette_is_mapped_to_itself`
///   双重钉住）。精确路径建出的调色板里大量颜色本就是画面真色，这一步让
///   「PPT / 代码 / 演示画面无损」的承诺在**量化端**也成立。查表是 1024 槽开放寻址
///   （≤256 项，负载 ≤25%），每像素约几次比较，不进热点瓶颈。
pub struct Quantizer {
    palette: Palette,
    cache: Vec<u16>,
    /// 色表的 OKLab 预转换（理由见 [`oklab`]）。热路径每次查色都要用它，
    /// 所以只转一次 —— 不能每次调用都重算 256 遍。
    lab: Vec<[f32; 3]>,
    /// 24 位精确色键 → 调色板下标 的开放寻址表。`u32::MAX` = 空槽。
    exact_keys: Vec<u32>,
    exact_vals: Vec<u8>,
}

/// 精确色表的槽数。≥ 4 × 256，负载率 ≤ 25%，线性探测几乎一步命中。
const EXACT_SLOTS: usize = 1024;

#[inline]
fn exact_slot(key: u32) -> usize {
    (key.wrapping_mul(0x9E37_79B1) >> 12) as usize & (EXACT_SLOTS - 1)
}

const CACHE_EMPTY: u16 = u16::MAX;

impl Quantizer {
    pub fn new(palette: Palette) -> Self {
        let lab: Vec<[f32; 3]> = palette
            .colors()
            .iter()
            .map(|c| oklab(c[0], c[1], c[2]))
            .collect();
        let mut exact_keys = vec![u32::MAX; EXACT_SLOTS];
        let mut exact_vals = vec![0u8; EXACT_SLOTS];
        for (i, c) in palette.colors().iter().enumerate() {
            let key = exact_key(c[0], c[1], c[2]);
            let mut s = exact_slot(key);
            while exact_keys[s] != u32::MAX && exact_keys[s] != key {
                s = (s + 1) & (EXACT_SLOTS - 1);
            }
            if exact_keys[s] == u32::MAX {
                exact_keys[s] = key;
                exact_vals[s] = i as u8;
            }
        }
        Self {
            palette,
            cache: vec![CACHE_EMPTY; 32768],
            lab,
            exact_keys,
            exact_vals,
        }
    }

    pub fn palette(&self) -> &Palette {
        &self.palette
    }

    /// 把 RGB 流（3n 字节）量化成 n 个下标，**追加**写进 `out`。
    pub fn map_into(&mut self, rgb: &[u8], out: &mut Vec<u8>) {
        out.reserve(rgb.len() / 3);
        for px in rgb.chunks_exact(3) {
            // 快路径：颜色本就在色表里 → 精确下标（见 struct 文档）。
            let key = exact_key(px[0], px[1], px[2]);
            let mut s = exact_slot(key);
            let mut hit = None;
            while self.exact_keys[s] != u32::MAX {
                if self.exact_keys[s] == key {
                    hit = Some(self.exact_vals[s]);
                    break;
                }
                s = (s + 1) & (EXACT_SLOTS - 1);
            }
            if let Some(idx) = hit {
                out.push(idx);
                continue;
            }
            // 慢路径：按 5 位桶中心找最近色（与到达顺序无关，见 struct 文档）。
            let bkey = bin_index(px[0], px[1], px[2]);
            let mut idx = self.cache[bkey];
            if idx == CACHE_EMPTY {
                idx = nearest_lab(&self.lab, center(px[0]), center(px[1]), center(px[2])) as u16;
                self.cache[bkey] = idx;
            }
            out.push(idx as u8);
        }
    }
}

/// 量化缓存与精确色表共用的 24 位颜色键。
#[inline]
fn exact_key(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// 5 位桶的**几何中心**（±4 以内最不坏的代表点）。
#[inline]
fn center(v: u8) -> u8 {
    let c = (((v as u16) >> 3) << 3) + 4;
    if c > 255 {
        255
    } else {
        c as u8
    }
}

// ─────────────────────────── 降采样 ───────────────────────────

/// BGRA（自上而下，紧密排布）→ RGB（目标尺寸），区域平均。
///
/// **在线性光里平均**：直接对 sRGB 字节求平均会让暗部偏亮、亮部偏暗（屏幕录制里表现为
/// 细字发灰、色块边缘发脏）。这里查表把 sRGB 折到线性、平均、再查表编回 sRGB ——
/// 两次 8bit 查表，比逐像素 `powf` 便宜几个数量级。
pub fn downscale_bgra_to_rgb(src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32) -> Vec<u8> {
    let dw = dw.max(1);
    let dh = dh.max(1);
    let mut out = Vec::with_capacity((dw * dh * 3) as usize);
    if sw == 0 || sh == 0 || src.len() < (sw as usize * sh as usize * 4) {
        out.resize((dw * dh * 3) as usize, 0);
        return out;
    }
    if dw == sw && dh == sh {
        for px in src.chunks_exact(4) {
            out.push(px[2]);
            out.push(px[1]);
            out.push(px[0]);
        }
        return out;
    }
    let (lut_in, lut_out) = srgb_luts();
    for dy in 0..dh {
        let y0 = (dy as u64 * sh as u64 / dh as u64) as u32;
        let y1 = (((dy + 1) as u64 * sh as u64 / dh as u64) as u32)
            .max(y0 + 1)
            .min(sh);
        for dx in 0..dw {
            let x0 = (dx as u64 * sw as u64 / dw as u64) as u32;
            let x1 = (((dx + 1) as u64 * sw as u64 / dw as u64) as u32)
                .max(x0 + 1)
                .min(sw);
            let mut acc = [0u32; 3];
            let mut cnt = 0u32;
            for y in y0..y1 {
                let row = y as usize * sw as usize * 4;
                for x in x0..x1 {
                    let p = row + x as usize * 4;
                    // BGRA：蓝在最低字节。
                    acc[0] += lut_in[src[p + 2] as usize];
                    acc[1] += lut_in[src[p + 1] as usize];
                    acc[2] += lut_in[src[p] as usize];
                    cnt += 1;
                }
            }
            let cnt = cnt.max(1);
            for ch in 0..3 {
                out.push(lut_out[(acc[ch] / cnt) as usize]);
            }
        }
    }
    out
}

/// (sRGB 字节 → 线性 0..4096, 线性 0..4096 → sRGB 字节) 两张查表。
fn srgb_luts() -> (Vec<u32>, Vec<u8>) {
    let mut to_lin = Vec::with_capacity(256);
    for i in 0..256u32 {
        let s = i as f64 / 255.0;
        let l = if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        };
        to_lin.push((l * 4096.0).round() as u32);
    }
    let mut to_srgb = Vec::with_capacity(4097);
    for i in 0..=4096u32 {
        let l = i as f64 / 4096.0;
        let s = if l <= 0.0031308 {
            l * 12.92
        } else {
            1.055 * l.powf(1.0 / 2.4) - 0.055
        };
        to_srgb.push((s * 255.0).round().clamp(0.0, 255.0) as u8);
    }
    (to_lin, to_srgb)
}

/// 按"长边不超过 `max_long`"算目标尺寸（保持宽高比、取偶、**绝不上采样**）。
///
/// 纯函数 → 门禁能直接钉住"720p 档在 1080p 屏上产出 1280×720"这类契约。
pub fn fit_within(src_w: u32, src_h: u32, max_long: u32) -> (u32, u32) {
    let even = |v: u32| (v.max(2)) & !1;
    if src_w == 0 || src_h == 0 || max_long == 0 {
        return (even(src_w), even(src_h));
    }
    let long = src_w.max(src_h);
    if long <= max_long {
        return (even(src_w), even(src_h));
    }
    let scale = max_long as f64 / long as f64;
    (
        even((src_w as f64 * scale).round() as u32),
        even((src_h as f64 * scale).round() as u32),
    )
}

// ─────────────────────────── 写入器 ───────────────────────────

/// 两帧之间的变化包围盒（下标空间），`None` = **一个像素都没变**。
///
/// 屏幕录制的内容在两帧之间绝大多数面积是一样的（PPT 一页要停好几秒、打字只动一行、
/// 光标在一个按钮附近晃），把整幅 `w×h` 都塞进 LZW 是纯粹的浪费。这里只求出变化的最小
/// 外接矩形，交给 GIF 的**子矩形帧**去写（`Frame::left/top/width/height`）。
///
/// 两条实现上的讲究：
///
/// 1. **先比整行**（`a == b` 走 memcmp）——静止画面下这行比较几乎是全部成本，比逐像素
///    快一个数量级；
/// 2. 行内**只从两端往中间找不同**，不必扫完全行：包围盒只关心最左和最右的差异列。
///
/// 返回 `(x, y, w, h)`；无变化返回 `None`。
pub fn diff_rect(prev: &[u8], cur: &[u8], w: usize, h: usize) -> Option<(u16, u16, u16, u16)> {
    if w == 0 || h == 0 || prev.len() < w * h || cur.len() < w * h {
        // ★ 输入不完整时**保守地当成"整幅都变了"**（而不是"没变"）。
        //   `None` 的语义是"这一帧可以整个丢掉"——门禁函数在拿不准的时候必须往
        //   "多写一点"那边倒，绝不能把调用方引向丢帧。
        return Some((
            0,
            0,
            w.min(u16::MAX as usize) as u16,
            h.min(u16::MAX as usize) as u16,
        ));
    }
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0usize;
    let mut max_y = 0usize;
    for y in 0..h {
        let s = y * w;
        let a = &prev[s..s + w];
        let b = &cur[s..s + w];
        if a == b {
            continue;
        }
        if y < min_y {
            min_y = y;
        }
        max_y = y;
        // 从两端往中间夹：min_x 只要最小的那个不同列，max_x 只要最大的那个。
        let mut lo = 0usize;
        while lo < w && a[lo] == b[lo] {
            lo += 1;
        }
        let mut hi = w - 1;
        while hi > lo && a[hi] == b[hi] {
            hi -= 1;
        }
        if lo < min_x {
            min_x = lo;
        }
        if hi > max_x {
            max_x = hi;
        }
    }
    if min_y >= h {
        return None;
    }
    // 尺寸用 u16 是 `gif::Frame` 的字段类型；GIF 画布本身也限 65535，这里不会溢出。
    Some((
        min_x.min(u16::MAX as usize) as u16,
        min_y.min(u16::MAX as usize) as u16,
        (max_x - min_x + 1).min(u16::MAX as usize) as u16,
        (max_y - min_y + 1).min(u16::MAX as usize) as u16,
    ))
}

/// 变化面积超过整幅的这个比例时，干脆写整幅。
///
/// 理由不在字节数（这时候两者差不多），而在**行的连续性**：子矩形帧的每一行都是
/// `width` 字节的短行，LZW 的滑窗在行尾被截断，字典命中率比整幅低。面积已经很接近
/// 整幅时，整幅帧反而更小、且少一次逐行拷贝。
const FULL_FRAME_RATIO_NUM: u32 = 4;
const FULL_FRAME_RATIO_DEN: u32 = 5;

/// 待写帧：**延迟一帧**再落盘，好把"这段时间画面没变"并入它的延时。
///
/// 为什么要拖一帧：GIF 的延时是**写在帧头里**的，已经写出去的帧改不了。所以"画面静止
/// 1.5 秒"这件事只有两条出路 —— 要么真的再写一帧静止画面（浪费一次 LZW），要么把它
/// 加在**上一帧**的延时上。后者要求上一帧还没写出去，于是就必须延迟一帧。
struct Pending {
    /// 该帧的**完整**下标（`w*h`）。它是下一帧求包围盒的基准。
    indices: Vec<u8>,
    /// 相对上一帧的变化矩形；首帧是整幅。
    rect: (u16, u16, u16, u16),
    /// 自它成为待写帧以来累积的显示时长（厘秒）。
    delay_cs: u32,
}

/// 逐帧写 GIF89a。内部用 `gif` crate 的编码器（见模块头注释）。
pub struct GifWriter {
    enc: gif::Encoder<BufWriter<std::fs::File>>,
    w: u16,
    h: u16,
    frames: u32,
    /// 见 [`Pending`]。`None` = 还没收到任何一帧。
    pending: Option<Pending>,
}

impl GifWriter {
    /// 建文件、写头与全局色表；`loop_forever` 决定要不要循环扩展。
    pub fn create(
        path: &Path,
        w: u32,
        h: u32,
        palette: &Palette,
        loop_forever: bool,
    ) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = std::fs::File::create(path)?;
        let w = w.clamp(1, 65535) as u16;
        let h = h.clamp(1, 65535) as u16;
        let table = palette.table_bytes();
        let mut enc = gif::Encoder::new(BufWriter::new(file), w, h, &table)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?;
        if loop_forever {
            enc.set_repeat(gif::Repeat::Infinite)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}")))?;
        }
        Ok(Self {
            enc,
            w,
            h,
            frames: 0,
            pending: None,
        })
    }

    pub fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }

    /// **写进文件的**帧数（待写帧也算上，它马上就会落盘）。
    ///
    /// ★ 不是"采集到多少帧"：走 [`Self::write_frame_delta`] 时，与上一帧逐像素相同的帧
    ///   不会进文件（时长并进上一帧的延时）。所以这个数会**小于**采集帧数，那正是差量
    ///   在起作用 —— 用它作进度反馈时别拿它当 fps 的分子。
    pub fn frames(&self) -> u32 {
        self.frames + u32::from(self.pending.is_some())
    }

    /// 写一帧（整幅、不透明、不交错）。`indices` 长度须等于 `w*h`。
    pub fn write_frame(&mut self, indices: &[u8], delay_cs: u16) -> io::Result<()> {
        let need = self.w as usize * self.h as usize;
        if indices.len() != need {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GIF 帧下标数应为 {need}，实得 {}", indices.len()),
            ));
        }
        let rect = (0, 0, self.w, self.h);
        self.emit(indices, rect, delay_cs.max(MIN_DELAY_CS))
    }

    /// 写一帧 —— **带帧间差量与静止合并**。`dt_cs` 是距上一帧采集到的真实间隔（厘秒）。
    ///
    /// 三种情形：
    ///
    /// - **首帧**：整幅写出（没有基准可比）；
    /// - **与上一帧逐像素相同**：**不写帧**，把 `dt_cs` 记在待写帧上（静止画面因此零成本）；
    /// - **有变化**：只写变化包围盒那一片，同时把上一帧落盘（顺带把静止时长并进它的延时）。
    ///
    /// 相机用的"采集→编码"循环每 tick 都会调它，所以**不做任何分配**以外的重活：
    /// 求包围盒是一次 memcmp 密集的扫描，比它省下的 LZW 便宜得多。
    ///
    /// ★ 只在**我们自己的全局色表 + `Dispose::Keep`** 的前提下成立：子矩形帧的语义就是
    ///   "把这块覆盖到当前画布上"，所以它天然要求每一帧都能拿到"上一帧的完整画面"——
    ///   本函数内部用 `Pending::indices` 一直留着那份完整画面，正是为此。
    pub fn write_frame_delta(&mut self, indices: &[u8], dt_cs: u16) -> io::Result<()> {
        let need = self.w as usize * self.h as usize;
        if indices.len() != need {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GIF 帧下标数应为 {need}，实得 {}", indices.len()),
            ));
        }
        let dt = dt_cs as u32;
        // 直接 take 走 owned 值：既免去 as_mut/take 两次借用，也无需 expect 兜底
        // （else 分支已处理 None），守住生产禁 panic 红线。
        let Some(mut pf) = self.pending.take() else {
            // 首帧：整幅，延时从 0 起算（下一帧到来时才会补上区间）。
            self.pending = Some(Pending {
                indices: indices.to_vec(),
                rect: (0, 0, self.w, self.h),
                delay_cs: 0,
            });
            return Ok(());
        };
        if pf.indices == indices {
            // 画面一个像素都没动：这一帧不进文件，时间记在待写帧身上。
            pf.delay_cs = pf.delay_cs.saturating_add(dt);
            self.pending = Some(pf);
            return Ok(());
        }
        // 有变化 → 上一帧落盘（延时 = 它在屏幕上真正待了多久），本帧成为新的待写帧。
        pf.delay_cs = pf.delay_cs.saturating_add(dt);
        let (prev_idx, prev_rect, prev_delay) = (pf.indices, pf.rect, pf.delay_cs);
        self.emit(&prev_idx, prev_rect, clamp_delay(prev_delay))?;
        let rect = match diff_rect(&prev_idx, indices, self.w as usize, self.h as usize) {
            Some(r) => r,
            // 理论上到不了（上面刚比过不相等），真到了就退化成整幅，宁可大一点也不出错帧。
            None => (0, 0, self.w, self.h),
        };
        let rect = if self.is_mostly_full(rect) {
            (0, 0, self.w, self.h)
        } else {
            rect
        };
        self.pending = Some(Pending {
            indices: indices.to_vec(),
            rect,
            delay_cs: 0,
        });
        Ok(())
    }

    /// 矩形面积是否已接近整幅（见 [`FULL_FRAME_RATIO_NUM`]）。
    fn is_mostly_full(&self, rect: (u16, u16, u16, u16)) -> bool {
        let area = rect.2 as u32 * rect.3 as u32;
        let full = self.w as u32 * self.h as u32;
        area * FULL_FRAME_RATIO_DEN >= full * FULL_FRAME_RATIO_NUM
    }

    /// **底层入口**：直接写一片下标。`rect` 相对整幅，`delay_cs` 是这一帧的**显示时长**。
    ///
    /// 调用方要自己负责全部正确性（写哪些帧、矩形对不对、延时多久）。存在的唯一理由是
    /// 裁剪路径（[`crate::core::gif_trim::rewrite_to`]）—— 它有自己的"待写帧"簿记（要跳过被
    /// 删掉的区间），套不进 [`Self::write_frame_delta`] 那条"逐次给相邻间隔"的契约。
    ///
    /// 面积已接近整幅的矩形会被归一成整幅（理由见 [`Self::is_mostly_full`]）。
    pub fn write_raw(
        &mut self,
        full: &[u8],
        rect: (u16, u16, u16, u16),
        delay_cs: u16,
    ) -> io::Result<()> {
        self.write_raw_alpha(full, rect, delay_cs, None)
    }

    /// [`Self::write_raw`] 的**带透明下标**版本：这一帧里所有 `== transparent` 的下标
    /// 都会被标成"透明"（`gif::Frame::transparent`）。
    ///
    /// 为什么透明下标要**由调用方给一个新槽**（而不是复用源文件声明的那个）：GIF 的透明是
    /// **逐帧**语义 —— 同一个下标可以在这一帧是透明的、在另一帧是实色。复用就会在产物里挖出
    /// 洞。<transparent> 由 [`crate::core::gif_trim::rewrite_to`] 用
    /// [`Palette::with_extra_slot`] 申请，专槽专用，因此**不会撞上任何画面内容**。
    pub fn write_raw_alpha(
        &mut self,
        full: &[u8],
        rect: (u16, u16, u16, u16),
        delay_cs: u16,
        transparent: Option<u8>,
    ) -> io::Result<()> {
        let need = self.w as usize * self.h as usize;
        if full.len() != need {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("GIF 帧下标数应为 {need}，实得 {}", full.len()),
            ));
        }
        let rect = if self.is_mostly_full(rect) {
            (0, 0, self.w, self.h)
        } else {
            rect
        };
        self.emit_alpha(full, rect, delay_cs, transparent)
    }

    /// 把一片下标真正交给 `gif` 编码器（**不声明透明**）。`rect` 是相对整幅的矩形。
    fn emit(&mut self, full: &[u8], rect: (u16, u16, u16, u16), delay_cs: u16) -> io::Result<()> {
        self.emit_alpha(full, rect, delay_cs, None)
    }

    /// [`Self::emit`] 的带透明下标版本。
    fn emit_alpha(
        &mut self,
        full: &[u8],
        rect: (u16, u16, u16, u16),
        delay_cs: u16,
        transparent: Option<u8>,
    ) -> io::Result<()> {
        let (fw, fh) = (self.w as usize, self.h as usize);
        let (left, top, rw, rh) = (
            rect.0 as usize,
            rect.1 as usize,
            rect.2 as usize,
            rect.3 as usize,
        );
        let is_full = left == 0 && top == 0 && rw == fw && rh == fh;
        let mut frame = if is_full {
            gif::Frame::from_indexed_pixels(self.w, self.h, full[..fw * fh].to_vec(), transparent)
        } else {
            // 子矩形帧：逐行把这片挖出来（行内是连续的，所以是一堆 memcpy，不是逐像素）。
            let mut buf = Vec::with_capacity(rw * rh);
            for row in top..(top + rh).min(fh) {
                let s = row * fw + left;
                buf.extend_from_slice(&full[s..(s + rw).min(fw * fh)]);
            }
            // 上面可能因越界而短一截；补齐到 rw*rh，否则 crate 内部的 assert 会炸。
            buf.resize(rw * rh, 0);
            gif::Frame {
                left: left as u16,
                top: top as u16,
                width: rw as u16,
                height: rh as u16,
                buffer: buf.into(),
                transparent,
                ..Default::default()
            }
        };
        frame.delay = delay_cs.max(MIN_DELAY_CS);
        frame.dispose = gif::DisposalMethod::Keep;
        self.enc
            .write_frame(&frame)
            .map_err(|e| io::Error::other(format!("{e}")))?;
        self.frames += 1;
        Ok(())
    }

    /// 收尾落盘（写 NETSCAPE/终结符并 flush）。待写帧在这里补上。
    pub fn finish(mut self) -> io::Result<()> {
        if let Some(p) = self.pending.take() {
            // 尾帧的延时 = 它出现之后到停止为止的时长；一次都没变过就退化成最小延时。
            self.emit(&p.indices, p.rect, clamp_delay(p.delay_cs))?;
        }
        let mut inner = self
            .enc
            .into_inner()
            .map_err(|e| io::Error::other(format!("GIF 收尾失败: {e:?}")))?;
        use std::io::Write as _;
        inner.flush()
    }
}

/// 累积的厘秒数 → GIF 延时字段。下限是看图器约定，上限是 u16。
///
/// 对外可见是因为裁剪路径（[`crate::core::gif_trim`]）自己攒时长、自己落盘，
/// 它必须走**同一套**夹取 —— 两边各写一次，迟早会出现"录制的产物合规、裁剪的不合规"。
pub fn clamp_delay(cs: u32) -> u16 {
    cs.clamp(MIN_DELAY_CS as u32, u16::MAX as u32) as u16
}

// ─────────────────────── 门禁（纯函数 + 独立解码器） ───────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("curseen_gifenc_test");
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn palette_uses_exact_colors_when_few() {
        // 3 个纯色 → 调色板就该是这 3 个，量化误差 0。
        let mut rgb = Vec::new();
        for _ in 0..100 {
            rgb.extend_from_slice(&[255, 0, 0]);
            rgb.extend_from_slice(&[0, 255, 0]);
            rgb.extend_from_slice(&[0, 0, 255]);
        }
        let p = Palette::from_rgb(&rgb, MAX_COLORS).expect("非空");
        assert_eq!(p.len(), 3, "三个纯色就该只出三色");
        for c in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
            assert!(p.colors().contains(&c), "缺精确色 {c:?}");
        }
        let mut q = Quantizer::new(p);
        let mut idx = Vec::new();
        q.map_into(&rgb, &mut idx);
        let colors = q.palette().colors();
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let got = colors[idx[i] as usize];
            assert_eq!(&got[..], px, "量化把纯色改掉了（第 {i} 个像素）");
        }
    }

    #[test]
    fn palette_median_cut_stays_within_limit() {
        // 4096 个渐变色的画面：必须收敛到 ≤256 色，且平均误差有限。
        let mut rgb = Vec::with_capacity(4096 * 3);
        for i in 0..4096u32 {
            rgb.push((i % 256) as u8);
            rgb.push(((i / 16) % 256) as u8);
            rgb.push(((i / 256) % 256) as u8);
        }
        let p = Palette::from_rgb(&rgb, MAX_COLORS).expect("非空");
        assert!(p.len() <= MAX_COLORS, "色数超限：{}", p.len());
        assert!(p.len() > 8, "渐变画面不该只剩 {} 色", p.len());
        let mut q = Quantizer::new(p);
        let mut idx = Vec::new();
        q.map_into(&rgb, &mut idx);
        let colors = q.palette().colors();
        let mut err = 0f64;
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let c = colors[idx[i] as usize];
            for ch in 0..3 {
                err += (c[ch] as f64 - px[ch] as f64).abs();
            }
        }
        let mean = err / (rgb.len() / 3) as f64 / 3.0;
        assert!(mean < 12.0, "平均通道误差 {mean:.2} 偏大");
    }

    #[test]
    fn palette_bits_and_table_size_follow_color_count() {
        let p2 = Palette::from_colors([[0u8, 0, 0], [1, 1, 1]].iter()).expect("有颜色");
        assert_eq!(p2.bits(), 1, "2 色只需 1 位");
        assert_eq!(p2.table_bytes().len(), 2 * 3, "色表长度须为 3×2^bits");
        let p300 = Palette::from_rgb(
            &(0..300u32)
                .flat_map(|i| {
                    [
                        (i % 256) as u8,
                        ((i / 2) % 256) as u8,
                        ((i / 3) % 256) as u8,
                    ]
                })
                .collect::<Vec<u8>>(),
            MAX_COLORS,
        )
        .expect("非空");
        assert!(p300.len() <= MAX_COLORS);
        assert_eq!(p300.table_bytes().len(), 256 * 3, "满色表必须是 768 字节");
        assert_eq!(p300.bits(), 8);
    }

    /// 给透明下标"另开一槽"：既有下标一个都不能动，色表长度要仍然合法（3×2^bits）。
    ///
    /// 这条 API 的整个价值就在"既有下标不动"上 —— 动了的话裁剪产物里所有像素的颜色都会
    /// 平移，而那是最难查的一类错（图还能看，叫不出哪里不对）。
    #[test]
    fn reserving_a_transparent_slot_never_moves_the_existing_indices() {
        let p = Palette::from_colors([[10u8, 20, 30], [40, 50, 60], [70, 80, 90]].iter())
            .expect("有颜色");
        // 只比**真实颜色**那一段：`table_bytes` 会把色表补到 2^bits，那次补齐的填充字节
        // （这里是 3 色补到 4 色）本来就会被新槽顶掉，拿它当判据是错的。
        let before = p.table_bytes()[..p.len() * 3].to_vec();
        let (q, t) = p.with_extra_slot().expect("3 色还能扩");
        assert_eq!(t as usize, p.len(), "新槽必须落在原色表**之后**");
        assert_eq!(
            &q.table_bytes()[..before.len()],
            &before[..],
            "既有色表被改动了 —— 产物所有像素都会偏色"
        );
        assert_eq!(q.table_bytes().len() % 3, 0);
        assert!(
            q.table_bytes().len() / 3 == 4,
            "3 色 + 1 槽 = 4，正好补满 2 位"
        );
        assert_eq!(q.bits(), 2);

        // 满色表开不出来：必须**明确**返回 None（调用方据此退化成"全实心写"）。
        let full = Palette::from_rgb(
            &(0..(MAX_COLORS as u32))
                .flat_map(|i| [i as u8, (i / 2) as u8, (i / 3) as u8])
                .collect::<Vec<u8>>(),
            MAX_COLORS,
        )
        .expect("非空");
        assert_eq!(full.len(), MAX_COLORS);
        assert!(
            full.with_extra_slot().is_none(),
            "色表已满时不能悄悄扩 —— 那会写出越界下标"
        );
    }

    #[test]
    fn downscale_preserves_solid_color_and_size() {
        let (sw, sh) = (16u32, 16u32);
        let mut src = Vec::new();
        for _ in 0..(sw * sh) {
            src.extend_from_slice(&[10, 20, 200, 255]); // BGRA → RGB (200,20,10)
        }
        let out = downscale_bgra_to_rgb(&src, sw, sh, 4, 4);
        assert_eq!(out.len(), 4 * 4 * 3);
        for px in out.chunks_exact(3) {
            assert_eq!(px, [200, 20, 10], "纯色降采样不该变色");
        }
    }

    #[test]
    fn downscale_is_gamma_aware() {
        // 黑白棋盘降到 1×1：线性光平均 = 0.5 线性 → sRGB ≈ 188。
        // 若退化成在 sRGB 空间直接平均会得到 127 —— 这条门禁钉的就是这个差别。
        let (sw, sh) = (2u32, 2u32);
        let mut src = Vec::new();
        for i in 0..4 {
            let v = if i % 2 == 0 { 0u8 } else { 255 };
            src.extend_from_slice(&[v, v, v, 255]);
        }
        let out = downscale_bgra_to_rgb(&src, sw, sh, 1, 1);
        let got = out[0];
        assert!(
            (181..=196).contains(&got),
            "伽马正确平均应落在 181..=196，实得 {got}（≈127 即退化成 sRGB 空间平均）"
        );
    }

    #[test]
    fn downscale_same_size_is_just_channel_swap() {
        let src = vec![1u8, 2, 3, 255, 4, 5, 6, 255];
        let out = downscale_bgra_to_rgb(&src, 2, 1, 2, 1);
        assert_eq!(out, vec![3, 2, 1, 6, 5, 4]);
    }

    #[test]
    fn downscale_short_input_is_zeroed_not_panicking() {
        let out = downscale_bgra_to_rgb(&[1, 2, 3, 4], 8, 8, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 3);
        assert!(
            out.iter().all(|&v| v == 0),
            "输入不足应返回全 0，而不是读越界"
        );
    }

    #[test]
    fn fit_within_never_upscales_and_keeps_even() {
        assert_eq!(fit_within(1920, 1080, 1280), (1280, 720));
        assert_eq!(fit_within(1920, 1080, 1920), (1920, 1080));
        assert_eq!(fit_within(1280, 720, 1920), (1280, 720), "不该上采样");
        assert_eq!(fit_within(2560, 1440, 1920), (1920, 1080));
        assert_eq!(fit_within(1080, 1920, 1280), (720, 1280), "竖屏按长边算");
        let (w, h) = fit_within(1921, 1081, 1920);
        assert_eq!((w % 2, h % 2), (0, 0), "编码器要偶数边");
    }

    /// 端到端：写一段双色动画，再用**独立的解码器**（`gif` crate 的 Reader）读回来。
    ///
    /// 这比"断言文件非空"强得多 —— 它验的是下标真的按帧落进去了。
    #[test]
    fn written_gif_round_trips_through_the_decoder() {
        let path = tmp_path("roundtrip.gif");
        let _ = std::fs::remove_file(&path);
        let pal = Palette::from_colors([[0u8, 0, 0], [255, 255, 255]].iter()).expect("有颜色");
        let (w, h) = (6u32, 4u32);
        let f1: Vec<u8> = (0..(w * h) as u8).map(|i| i % 2).collect();
        let f2: Vec<u8> = (0..(w * h) as u8)
            .map(|i| if i < 12 { 1 } else { 0 })
            .collect();
        {
            let mut gw = GifWriter::create(&path, w, h, &pal, true).expect("建文件");
            gw.write_frame(&f1, 5).expect("写帧1");
            gw.write_frame(&f2, 7).expect("写帧2");
            assert_eq!(gw.frames(), 2);
            gw.finish().expect("收尾");
        }

        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let file = std::fs::File::open(&path).expect("打开");
        let mut dec = opts.read_info(file).expect("解析");
        assert_eq!((dec.width(), dec.height()), (w as u16, h as u16));
        // 全局色表：黑在前、白在后（from_colors 保留传入顺序）
        let table = dec.global_palette().expect("有全局色表").to_vec();
        assert_eq!(&table[..6], &[0, 0, 0, 255, 255, 255]);

        let got1 = dec.read_next_frame().expect("读帧1").expect("有帧1");
        assert_eq!(got1.buffer.as_ref(), &f1[..], "第 1 帧下标对不上");
        assert_eq!(got1.delay, 5);
        let got2 = dec.read_next_frame().expect("读帧2").expect("有帧2");
        assert_eq!(got2.buffer.as_ref(), &f2[..], "第 2 帧下标对不上");
        assert_eq!(got2.delay, 7);
        assert!(dec.read_next_frame().expect("读完").is_none(), "应只有两帧");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn non_looping_gif_has_no_netscape_extension() {
        let path = tmp_path("once.gif");
        let _ = std::fs::remove_file(&path);
        let pal = Palette::from_colors([[9u8, 9, 9]].iter()).expect("有颜色");
        {
            let mut gw = GifWriter::create(&path, 2, 2, &pal, false).expect("建文件");
            gw.write_frame(&[0u8; 4], 30).expect("写帧");
            gw.finish().expect("收尾");
        }
        let bytes = std::fs::read(&path).expect("读回");
        assert_eq!(&bytes[..6], b"GIF89a");
        assert!(
            !bytes.windows(11).any(|w| w == b"NETSCAPE2.0"),
            "不循环时不该写循环扩展"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn tiny_delay_is_clamped_to_viewer_minimum() {
        let path = tmp_path("fast.gif");
        let _ = std::fs::remove_file(&path);
        let pal = Palette::from_colors([[1u8, 2, 3]].iter()).expect("有颜色");
        {
            let mut gw = GifWriter::create(&path, 2, 2, &pal, true).expect("建文件");
            gw.write_frame(&[0u8; 4], 1).expect("写帧");
            gw.finish().expect("收尾");
        }
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let file = std::fs::File::open(&path).expect("打开");
        let mut dec = opts.read_info(file).expect("解析");
        let f = dec.read_next_frame().expect("读").expect("有帧");
        assert_eq!(
            f.delay, MIN_DELAY_CS,
            "1cs 会被看图器当 100ms 播，必须抬到 2cs"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_index_count_is_rejected_at_the_boundary() {
        let path = tmp_path("bad.gif");
        let pal = Palette::from_colors([[1u8, 2, 3]].iter()).expect("有颜色");
        let mut gw = GifWriter::create(&path, 8, 8, &pal, false).expect("建文件");
        assert!(
            gw.write_frame(&[0u8; 4], 3).is_err(),
            "下标数不足必须返回错误（否则会撞 crate 内部的 assert）"
        );
        let _ = std::fs::remove_file(&path);
    }

    // ─────────────────────── 帧间差量 ───────────────────────

    /// 包围盒必须是**最小**的，且"完全一样"要能被识别成 `None`。
    #[test]
    fn diff_rect_is_minimal_and_none_when_identical() {
        let (w, h) = (10usize, 5usize);
        let a = vec![7u8; w * h];
        assert_eq!(diff_rect(&a, &a, w, h), None, "逐像素相同 → 没有变化矩形");

        let mut b = a.clone();
        b[w + 4] = 9;
        assert_eq!(diff_rect(&a, &b, w, h), Some((4, 1, 1, 1)), "单像素 → 1×1");

        let mut c = a.clone();
        c[2 * w + 3] = 1;
        c[3 * w + 6] = 1;
        assert_eq!(
            diff_rect(&a, &c, w, h),
            Some((3, 2, 4, 2)),
            "两行上各一个像素 → 包住它们的最小矩形"
        );

        let mut d = a.clone();
        d[0] = 1;
        d[4 * w + 9] = 2;
        assert_eq!(diff_rect(&a, &d, w, h), Some((0, 0, 10, 5)), "对角 → 整幅");
    }

    /// 输入长度不足时**宁多写、不丢帧**（`None` 的语义是"可以整帧丢掉"，绝不能误报）。
    #[test]
    fn diff_rect_fails_safe_on_short_input() {
        assert_eq!(
            diff_rect(&[0u8; 4], &[1u8; 4], 4, 4),
            Some((0, 0, 4, 4)),
            "输入不完整应保守地报「整幅都变了」，而不是「没变」"
        );
    }

    /// 端到端：差量写过一串"静止 + 局部变化"的帧，再用**独立解码器**逐帧重放，
    /// 重放出的画布序列必须与原始画面序列逐字节一致。
    ///
    /// 这条门禁是差量功能的地基 —— 子矩形帧一旦算错位置，画面上会出现"残影/错位块"，
    /// 而那种 bug 靠肉眼看产物截图极难发现（帧与帧之间只差几个小块）。
    #[test]
    fn delta_frames_rebuild_the_same_canvas_and_merge_static_time() {
        let path = tmp_path("delta.gif");
        let _ = std::fs::remove_file(&path);
        let pal = Palette::from_colors([[0u8, 0, 0], [255, 255, 255], [255, 0, 0]].iter())
            .expect("有颜色");
        let (w, h) = (8u32, 6u32);
        let n = (w * h) as usize;

        let f1 = vec![0u8; n];
        let mut f2 = f1.clone();
        f2[2 * 8 + 3] = 1; // 只动一个像素
        let f3 = f2.clone(); // 与 f2 逐像素相同 → 必须被合并掉
        let mut f4 = f2.clone();
        f4[5 * 8 + 7] = 2;
        let expect = [f1.clone(), f2.clone(), f4.clone()];

        {
            let mut gw = GifWriter::create(&path, w, h, &pal, true).expect("建文件");
            gw.write_frame_delta(&f1, 2).expect("帧1");
            gw.write_frame_delta(&f2, 2).expect("帧2");
            gw.write_frame_delta(&f3, 2).expect("帧3（静止）");
            gw.write_frame_delta(&f4, 2).expect("帧4");
            assert_eq!(
                gw.frames(),
                3,
                "4 次采集里有 1 次是静止帧，写进文件的只该有 3 帧"
            );
            gw.finish().expect("收尾");
        }

        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let file = std::fs::File::open(&path).expect("打开");
        let mut dec = opts.read_info(file).expect("解析");
        let (fw, fh) = (w as usize, h as usize);
        let mut canvas = vec![0u8; n];
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let mut delays: Vec<u16> = Vec::new();
        while let Some(f) = dec.read_next_frame().expect("读帧") {
            let (fwidth, fheight) = (f.width as usize, f.height as usize);
            assert_eq!(
                f.buffer.len(),
                fwidth * fheight,
                "子矩形帧的数据量必须等于它自报的 width×height"
            );
            for row in 0..fheight {
                let sy = f.top as usize + row;
                if sy >= fh {
                    break;
                }
                for col in 0..fwidth {
                    let sx = f.left as usize + col;
                    if sx >= fw {
                        break;
                    }
                    canvas[sy * fw + sx] = f.buffer[row * fwidth + col];
                }
            }
            seen.push(canvas.clone());
            delays.push(f.delay);
        }

        assert_eq!(
            seen.len(),
            3,
            "静止帧必须被合并，只剩 3 帧（实得 {}）",
            seen.len()
        );
        assert_eq!(seen, expect, "逐帧重放出的画布与原画面不一致");
        assert!(
            delays.iter().all(|&d| d >= MIN_DELAY_CS),
            "延时不得低于看图器下限"
        );
        assert_eq!(
            delays[1], 4,
            "静止那一帧的时长要落到**它之前**那一帧上（2+2），实得 {}",
            delays[1]
        );
        // 首帧必须是整幅：它是后续所有子矩形帧的基准，缺一块就永远补不回来。
        let mut opts2 = gif::DecodeOptions::new();
        opts2.set_color_output(gif::ColorOutput::Indexed);
        let f2file = std::fs::File::open(&path).expect("打开");
        let mut d2 = opts2.read_info(f2file).expect("解析");
        let first = d2.read_next_frame().expect("读").expect("有首帧");
        assert_eq!(
            (first.left, first.top, first.width, first.height),
            (0u16, 0u16, w as u16, h as u16),
            "首帧必须是整幅"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// 保色底必须真的含"纯色"：拖尾的默认固定色就是纯黄 `#FFFF00`，色相环少了 60° 这一档
    /// 就等于没修。
    #[test]
    fn accent_colors_pin_the_pure_primaries_and_secondaries() {
        let a = accent_colors();
        for c in [
            [255u8, 255, 0], // 黄（拖尾默认固定色）
            [255, 0, 0],     // 红
            [0, 255, 0],     // 绿
            [0, 255, 255],   // 青
            [0, 0, 255],     // 蓝
            [255, 0, 255],   // 品红
        ] {
            assert!(a.contains(&c), "保色底缺少 {c:?}");
        }
    }

    /// **复现用户报的现象并证明修复**：首帧只有桌面灰阶（鼠标静止 ⇒ 画面里没有拖尾色），
    /// 不带保色底时黄色被量化成灰；带上保色底后黄色必须还在。
    ///
    /// 对照组断言"退化成灰"是刻意的 —— 它把 bug 本身钉进门禁，将来谁把保色底去掉，
    /// 失败信息会直接说明"这正是用户看到的灰色拖尾"。
    #[test]
    fn accents_keep_the_yellow_trail_from_turning_gray() {
        // 首帧：200 级灰阶（≈一张没有彩色内容的桌面）。
        let mut rgb = Vec::with_capacity(64 * 64 * 3);
        for i in 0..(64 * 64usize) {
            let g = (i % 200) as u8;
            rgb.extend_from_slice(&[g, g, g]);
        }

        let plain = Palette::from_rgb(&rgb, MAX_COLORS).expect("对照组调色板");
        let acc = Palette::from_rgb_with_accents(&rgb, MAX_COLORS, &accent_colors())
            .expect("带保色底的调色板");

        // 对照组：最近色是灰 —— 这就是"黄色拖尾在 GIF 里变成灰色"。
        let bad = plain.colors()[plain.nearest(255, 255, 0) as usize];
        assert!(
            bad[0] == bad[1] && bad[1] == bad[2],
            "对照组本该退化成灰（现象没复现，本用例失去意义）：{bad:?}"
        );

        // 修复组：黄色必须基本原样。
        let got = acc.colors()[acc.nearest(255, 255, 0) as usize];
        let d = (got[0] as i32 - 255).abs() + (got[1] as i32 - 255).abs() + got[2] as i32;
        assert!(d <= 24, "黄色被量化偏了：{got:?}（三通道偏差和 {d}）");

        // 保色底不得把画面本身的精确色挤掉：灰阶仍要逐级存在。
        assert!(
            acc.len() > accent_colors().len(),
            "画面颜色一个都没进来，预算算错了"
        );
        assert!(
            acc.colors().contains(&[100, 100, 100]),
            "画面里的灰被挤掉了（保色底占了太多槽）"
        );
    }

    /// **端到端**：首帧里没有黄色（模拟"鼠标静止、拖尾还没出现"），后续帧才冒出黄色拖尾 ——
    /// 用**独立解码器**译回来的拖尾必须仍是黄色。
    ///
    /// 上面那条保色底单测只盯调色板；这条盯**整条链**（建表 → 量化 → 差量写 → 解码），
    /// 才等于用户真正看到的那张图。没有保色底时这里会译出一块灰。
    #[test]
    fn a_yellow_trail_appearing_later_survives_the_round_trip() {
        let path = tmp_path("yellow_trail.gif");
        let _ = std::fs::remove_file(&path);
        let (w, h) = (48u32, 32u32);
        let (wi, hi) = (w as usize, h as usize);
        let n = wi * hi;

        // 三帧：第 0 帧是纯灰阶桌面；第 1、2 帧在 (20..28, 10..18) 出现一块纯黄。
        let mut frames: Vec<Vec<u8>> = Vec::new();
        for k in 0..3usize {
            let mut f = vec![0u8; n * 3];
            for i in 0..n {
                let g = ((i * 5) % 180) as u8;
                f[i * 3] = g;
                f[i * 3 + 1] = g;
                f[i * 3 + 2] = g;
            }
            if k > 0 {
                for y in 10..18usize {
                    for x in 20..28usize {
                        let p = (y * wi + x) * 3;
                        f[p] = 255;
                        f[p + 1] = 255;
                        f[p + 2] = 0;
                    }
                }
            }
            frames.push(f);
        }

        // 调色板按**首帧**建（与录制一致），但带保色底 —— 这就是修复点。
        let pal = Palette::from_rgb_with_accents(&frames[0], MAX_COLORS, &accent_colors())
            .expect("调色板");
        let mut q = Quantizer::new(pal.clone());
        let mut idx: Vec<u8> = Vec::new();
        {
            let mut gw = GifWriter::create(&path, w, h, &pal, true).expect("建文件");
            for f in &frames {
                // ★ 必须清空：`map_into` 是**追加**语义（`run_gif` 里同样先 clear）。
                idx.clear();
                q.map_into(f, &mut idx);
                gw.write_frame_delta(&idx, 5).expect("写帧");
            }
            gw.finish().expect("收尾");
        }

        // 用 RGBA 输出解码（解码器自己查色表），再按 left/top 贴回画布。
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::RGBA);
        let file = std::fs::File::open(&path).expect("打开");
        let mut dec = opts.read_info(file).expect("解析");
        let mut canvas = vec![0u8; n * 4];
        let mut last: Option<Vec<u8>> = None;
        while let Some(f) = dec.read_next_frame().expect("读帧") {
            let (fw, fh) = (f.width as usize, f.height as usize);
            for row in 0..fh {
                let sy = f.top as usize + row;
                if sy >= hi {
                    break;
                }
                for col in 0..fw {
                    let sx = f.left as usize + col;
                    if sx >= wi {
                        break;
                    }
                    let src = (row * fw + col) * 4;
                    let dst = (sy * wi + sx) * 4;
                    canvas[dst..dst + 4].copy_from_slice(&f.buffer[src..src + 4]);
                }
            }
            last = Some(canvas.clone());
        }
        let last = last.expect("至少一帧");

        // 末帧的拖尾像素必须是黄的（三通道偏差和 ≤ 60）。
        let p = (14 * wi + 24) * 4;
        let got = (last[p], last[p + 1], last[p + 2]);
        let d = (got.0 as i32 - 255).abs() + (got.1 as i32 - 255).abs() + got.2 as i32;
        assert!(d <= 60, "拖尾译回来不是黄色：{got:?}（偏差和 {d}）");

        // 末帧的桌面灰阶也必须还在（保色底没把画面挤掉）。
        let p2 = (2 * wi + 2) * 4;
        let (r, g, b) = (last[p2], last[p2 + 1], last[p2 + 2]);
        assert!(r == g && g == b, "灰阶被染上了颜色：{:?}", (r, g, b));
        let _ = std::fs::remove_file(&path);
    }

    /// 只有一帧、且那一帧之后画面一直没动：产物应是"单帧 + 它的总停留时长"。
    #[test]
    fn a_single_static_recording_collapses_to_one_frame() {
        let path = tmp_path("static.gif");
        let _ = std::fs::remove_file(&path);
        let pal = Palette::from_colors([[3u8, 3, 3]].iter()).expect("有颜色");
        let f = vec![0u8; 4];
        {
            let mut gw = GifWriter::create(&path, 2, 2, &pal, true).expect("建文件");
            for _ in 0..10 {
                gw.write_frame_delta(&f, 5).expect("写");
            }
            gw.finish().expect("收尾");
        }
        let mut opts = gif::DecodeOptions::new();
        opts.set_color_output(gif::ColorOutput::Indexed);
        let file = std::fs::File::open(&path).expect("打开");
        let mut dec = opts.read_info(file).expect("解析");
        let one = dec.read_next_frame().expect("读").expect("有帧");
        assert_eq!(one.delay, 45, "9 个静止区间（5cs×9）应并进首帧的延时");
        assert!(
            dec.read_next_frame().expect("读完").is_none(),
            "静止部分不该真的再写帧"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// **差量的体积账**：同一串"小块在动"的画面，走差量写出的文件必须**显著小于**整幅写。
    ///
    /// 把"性能"从口号变成数字 —— 差量一旦因回归退化成整幅（比如 `diff_rect` 恒返回整幅），
    /// 这里会直接失败，而不是等用户发现产物悄悄变大才回头查。
    #[test]
    fn delta_frames_shrink_the_file_on_a_small_moving_region() {
        let (w, h) = (480u32, 270u32);
        let (wi, hi) = (w as usize, h as usize);
        let n = wi * hi;

        // 30 帧：一块 60×40 的白块每帧右移 4px，其余是纯黑背景。
        // 变化包围盒 ≈ 64×40 ≈ 2560 像素 / 整幅 129 600 像素 ⇒ 理论上限约 1/50。
        let mut frames: Vec<Vec<u8>> = Vec::new();
        for k in 0..30usize {
            let mut f = vec![0u8; n];
            let bx = 10 + k * 4;
            for y in 100..140usize {
                for x in bx..(bx + 60).min(wi) {
                    f[y * wi + x] = 1;
                }
            }
            frames.push(f);
        }
        let pal = Palette::from_colors([[0u8, 0, 0], [255, 255, 255]].iter()).expect("有颜色");

        let full_path = tmp_path("size_full.gif");
        let delta_path = tmp_path("size_delta.gif");
        let _ = std::fs::remove_file(&full_path);
        let _ = std::fs::remove_file(&delta_path);

        {
            let mut gw = GifWriter::create(&full_path, w, h, &pal, true).expect("建文件");
            for f in &frames {
                gw.write_frame(f, 5).expect("写整幅");
            }
            gw.finish().expect("收尾");
        }
        {
            let mut gw = GifWriter::create(&delta_path, w, h, &pal, true).expect("建文件");
            for f in &frames {
                gw.write_frame_delta(f, 5).expect("写差量");
            }
            assert_eq!(gw.frames(), 30, "每帧都在动，不该有帧被合并");
            gw.finish().expect("收尾");
        }

        let full = std::fs::metadata(&full_path).expect("stat full").len();
        let delta = std::fs::metadata(&delta_path).expect("stat delta").len();
        eprintln!(
            "[体积账] 480×270 × 30 帧，移动块 60×40：整幅 {full} B → 差量 {delta} B（{:.1}x）",
            full as f64 / delta as f64
        );
        assert!(
            delta * 3 < full,
            "差量应把体积压到整幅的 1/3 以下：整幅 {full} B vs 差量 {delta} B \
             （比值 {}%）",
            delta * 100 / full
        );
        let _ = std::fs::remove_file(&full_path);
        let _ = std::fs::remove_file(&delta_path);
    }

    /// ★★ 用户症状直测（2026-10-08「GIF 录 PPT 背景颜色对不上」）：
    /// **画面里真实存在、且已在调色板里的颜色，量化后必须原样**。
    ///
    /// 桶中心查找的缺陷：5 位桶把 248..255 归成一格，格中心 252 离**保色底的白 255**
    /// 比离画面自己的精确色 248 更近 —— 于是整片 #F8F8F8 的 PPT 背景被弹成纯白，
    /// 放回 PPT 一比就是「背景色不一样」。
    #[test]
    fn a_color_present_in_the_palette_is_mapped_to_itself() {
        // 首帧：大片 #F8F8F8 背景 + 少量黑色文字（颜色数 ≤ 预算 → 走精确色路径）。
        let mut rgb = Vec::new();
        for i in 0..(64 * 64usize) {
            if i % 97 == 0 {
                rgb.extend_from_slice(&[10, 10, 10]); // 文字
            } else {
                rgb.extend_from_slice(&[248, 248, 248]); // 背景
            }
        }
        let pal =
            Palette::from_rgb_with_accents(&rgb, MAX_COLORS, &accent_colors()).expect("调色板");
        assert!(
            pal.colors().contains(&[248, 248, 248]),
            "精确色路径应把背景原色收进色表（否则是另一档问题）"
        );

        let mut q = Quantizer::new(pal);
        let mut idx = Vec::new();
        q.map_into(&rgb, &mut idx);
        let colors = q.palette().colors();
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let got = colors[idx[i] as usize];
            assert_eq!(&got[..], px, "第 {i} 个像素 {px:?} 被量化成了 {got:?}");
        }
    }

    /// 同一类缺陷的**带色偏**版本：米白背景 #FBF6EC（三通道不同）同样不许被
    /// 弹到白色或其它保色底上。桶中心 (252, 248, 244) 最近的常是白。
    #[test]
    fn tinted_near_white_background_keeps_its_tint() {
        let mut rgb = Vec::new();
        for i in 0..(64 * 64usize) {
            if i % 89 == 0 {
                rgb.extend_from_slice(&[40, 40, 40]);
            } else {
                rgb.extend_from_slice(&[251, 246, 236]); // 米白
            }
        }
        let pal =
            Palette::from_rgb_with_accents(&rgb, MAX_COLORS, &accent_colors()).expect("调色板");
        assert!(pal.colors().contains(&[251, 246, 236]));
        let mut q = Quantizer::new(pal);
        let mut idx = Vec::new();
        q.map_into(&rgb, &mut idx);
        let colors = q.palette().colors();
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let got = colors[idx[i] as usize];
            assert_eq!(&got[..], px, "第 {i} 个像素 {px:?} 被量化成了 {got:?}");
        }
    }

    // ─────────── 画质：采样、覆盖面、色差度量（2026-09-20 用户报"画质差"后补） ───────────

    /// 两个颜色之间**最大通道差**。
    fn max_channel_err(a: [u8; 3], b: [u8; 3]) -> i32 {
        (0..3)
            .map(|c| (a[c] as i32 - b[c] as i32).abs())
            .max()
            .unwrap_or(0)
    }

    /// sRGB 两个颜色的 OKLab 距离（×100，便于读表）。
    fn de100(a: [u8; 3], b: [u8; 3]) -> f32 {
        let p = oklab(a[0], a[1], a[2]);
        let q = oklab(b[0], b[1], b[2]);
        (((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()) * 100.0
    }

    /// **对照组**：2026-09-20 之前用的"隔 N 像素抽样"。
    ///
    /// 留在这里是有意的 —— 把 bug 本身钉住，证明下面那条门禁测的确实是它
    /// （同 `accents_keep_the_yellow_trail_from_turning_gray` 的做法）。
    /// 逻辑与旧 `sample_rgb` 逐字一致：`cap = max*64`，`stride = n / cap`。
    fn old_strided_sample(rgb: &[u8], max_colors: usize) -> Vec<[u8; 3]> {
        let n = rgb.len() / 3;
        if n == 0 {
            return Vec::new();
        }
        let cap = (max_colors * 64).max(1024);
        let stride = if n > cap { n / cap } else { 1 };
        let mut out = Vec::with_capacity(cap.min(n));
        let mut i = 0;
        while i < n {
            let p = i * 3;
            out.push([rgb[p], rgb[p + 1], rgb[p + 2]]);
            i += stride.max(1);
        }
        out
    }

    /// **对照组**：旧的 RGB 欧氏距离最近色。
    fn old_rgb_nearest(pal: &Palette, r: u8, g: u8, b: u8) -> u8 {
        let mut best = 0usize;
        let mut best_d = u32::MAX;
        for (i, c) in pal.colors().iter().enumerate() {
            let dr = c[0] as i32 - r as i32;
            let dg = c[1] as i32 - g as i32;
            let db = c[2] as i32 - b as i32;
            let d = (dr * dr + dg * dg + db * db) as u32;
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best as u8
    }

    /// 抽样混叠：**有些列从未进入色表**，于是"颜色对不对"取决于画面在屏幕哪一侧。
    ///
    /// 实拍参数（1920×1080、256 色）：`stride = 126`、`gcd(126, 1920) = 6`
    /// ⇒ 只有 `x ≡ 0 (mod 6)` 的列被采到。这里把高度缩到 32 让 `stride = 3`
    /// （`gcd(3, 1920) = 3`），复现同一个缺陷：只画在 `x ≡ 1 (mod 3)` 列上的颜色，
    /// 旧抽样**一次都没碰到**。
    #[test]
    fn full_pixel_histogram_sees_columns_that_strided_sampling_missed() {
        const W: usize = 1920;
        const H: usize = 32;
        let bg = [40u8, 44, 52];
        // 故意挑一个**远离保色底**的颜色（不是纯色相，也不接近黑白灰）。
        let band = [70u8, 205, 140];
        let mut rgb = vec![0u8; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let c = if x % 3 == 1 { band } else { bg };
                let p = (y * W + x) * 3;
                rgb[p..p + 3].copy_from_slice(&c);
            }
        }

        // 对照组：旧抽样一次都没采到那条带 → 色表里根本没有它。
        let old = Palette::from_colors(old_strided_sample(&rgb, MAX_COLORS).iter())
            .expect("对照组调色板");
        assert!(
            !old.colors().contains(&band),
            "对照组本该漏掉这条带（这正是要钉住的 bug）"
        );
        let got_old = old.colors()[old.nearest(band[0], band[1], band[2]) as usize];
        let err_old = max_channel_err(got_old, band);
        assert!(err_old >= 30, "对照组应明显偏色，实得 {got_old:?}");

        // 修复组：全像素直方图 → 精确命中。
        let new = Palette::from_rgb(&rgb, MAX_COLORS).expect("修复组调色板");
        let got_new = new.colors()[new.nearest(band[0], band[1], band[2]) as usize];
        assert_eq!(got_new, band, "全像素统计应精确命中，实得 {got_new:?}");
        eprintln!(
            "[抽样混叠] 只出现在 x≡1 (mod 3) 列上的 {band:?}：旧抽样 → {got_old:?}（差 {err_old}）\
             ；全像素直方图 → {got_new:?}（差 0）"
        );
    }

    /// 造一帧"桌面 + 半透明飘带"。`cx = None` 表示这一帧还没有飘带。
    ///
    /// 还原用户的实际画面：飘带配成**纯色**（`#FFFF00`）+ `opacity 0.5`（`ribbon.lua`
    /// 里 `a = opacity * 255`），但半透明的东西压在背景上合成出来的**是一族颜色**；
    /// 两侧再各留 1px 覆盖率 50% 的抗锯齿边 —— 那圈 1px 就是用户看到的"边框"。
    fn desktop_frame(w: usize, h: usize, cx: Option<i64>) -> Vec<u8> {
        let mut rgb = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                // 背景：一条暖-冷渐变，模拟壁纸/窗口内容。
                let bg = [
                    (60 + x * 120 / w.max(1)) as u8,
                    (70 + y * 90 / h.max(1)) as u8,
                    (110 + (x + y) * 60 / (w + h).max(1)) as u8,
                ];
                let p = (y * w + x) * 3;
                let c = match cx {
                    None => bg,
                    Some(cx) => {
                        let d = (x as i64 - cx).abs();
                        // 带宽 15px（d ≤ 7 实心），d == 8 那圈是抗锯齿边。
                        let cov: u32 = match d {
                            0..=7 => 255,
                            8 => 128,
                            _ => 0,
                        };
                        let a = (255.0 * 0.5 * (cov as f32 / 255.0)).round() as u32;
                        [
                            (bg[0] as u32 * (255 - a) / 255 + 255 * a / 255) as u8,
                            (bg[1] as u32 * (255 - a) / 255 + 255 * a / 255) as u8,
                            (bg[2] as u32 * (255 - a) / 255) as u8,
                        ]
                    }
                };
                rgb[p..p + 3].copy_from_slice(&c);
            }
        }
        rgb
    }

    /// 量化后逐像素与"屏幕上真实的颜色"比，返回（飘带芯最大通道差, 飘带边最大通道差）。
    fn ribbon_err(pal: &Palette, frame: &[u8], w: usize, h: usize, cx: i64) -> (i32, i32) {
        let mut q = Quantizer::new(pal.clone());
        let mut idx = Vec::new();
        q.map_into(frame, &mut idx);
        let colors = pal.colors();
        let (mut core, mut edge) = (0i32, 0i32);
        for y in 0..h {
            for x in 0..w {
                let d = (x as i64 - cx).abs();
                if d > 8 {
                    continue;
                }
                let p = (y * w + x) * 3;
                let got = colors[idx[y * w + x] as usize];
                let e = (0..3)
                    .map(|c| (got[c] as i32 - frame[p + c] as i32).abs())
                    .max()
                    .unwrap_or(0);
                if d <= 7 {
                    core = core.max(e);
                } else {
                    edge = edge.max(e);
                }
            }
        }
        (core, edge)
    }

    /// ★★ 用户症状直测：**首帧里没有的纯色飘带，量化后不该变色、更不该多出一圈边框**。
    ///
    /// 对照组 = "只看首帧选色"（2026-09-20 之前的行为）；修复组 = 预热窗口的全像素直方图。
    /// 之所以首帧里没有飘带是**常态**而不是巧合：录制前的倒计时里鼠标基本不动，
    /// 等录制开始，`fade_duration` 3 秒的拖尾刚好褪干净。
    #[test]
    fn a_ribbon_missing_from_the_first_frame_keeps_its_color_and_gains_no_outline() {
        const W: usize = 320;
        const H: usize = 200;
        // 第 0 帧纯桌面；之后飘带从左边走进来（同一次录制里"之后才出现"的颜色）。
        let frames: Vec<Vec<u8>> = (0..9)
            .map(|k| {
                desktop_frame(
                    W,
                    H,
                    if k == 0 {
                        None
                    } else {
                        Some(60 + k as i64 * 8)
                    },
                )
            })
            .collect();
        let cx = 60 + 8 * 8;
        let accents = accent_colors();

        // 对照组：只看首帧（旧行为）。
        let old =
            Palette::from_rgb_with_accents(&frames[0], MAX_COLORS, &accents).expect("对照组调色板");
        // 修复组：预热窗口（9 帧）的全像素直方图 + 保色底。
        let mut hist = Hist::new();
        for f in &frames {
            hist.add_rgb(f);
        }
        let new = Palette::from_hist(Some(&frames[0]), &hist, MAX_COLORS, &accents)
            .expect("修复组调色板");

        let last = &frames[8];
        let (core_old, edge_old) = ribbon_err(&old, last, W, H, cx);
        let (core_new, edge_new) = ribbon_err(&new, last, W, H, cx);
        eprintln!(
            "[飘带] 最大通道差 —— 芯：只看首帧 {core_old} → 预热直方图 {core_new}；\
             抗锯齿边（用户说的\"边框\"）：{edge_old} → {edge_new}"
        );

        // 对照组：飘带的颜色在色表里根本不存在 ⇒ 芯与边都明显偏色。
        assert!(
            core_old >= 12,
            "对照组本该偏色（把 bug 钉住），实得 {core_old}"
        );
        assert!(
            edge_old >= 20,
            "对照组的抗锯齿边也该明显偏色，实得 {edge_old}"
        );
        // 修复组：芯与边都落到"肉眼分不出"的量级。
        assert!(
            core_new <= 10,
            "修复后飘带芯应基本无色差，实得 {core_new}（对照 {core_old}）"
        );
        assert!(
            edge_new * 2 < edge_old,
            "修复后抗锯齿边必须比对照组准一半以上：{edge_new} vs {edge_old}"
        );
        assert!(
            edge_new <= 10,
            "修复后\"边框\"应消失（通道差 ≤10），实得 {edge_new}"
        );
    }

    /// OKLab 色差比 RGB 欧氏距离更准（对照组 = 旧度量）。
    ///
    /// 读法：两边共用**同一张色表**，唯一差别是"哪个色表项离这个像素更近"。
    /// 指标取 OKLab 里的欧氏距离（标准 ΔE_OK 的近似），对两边都公平。
    ///
    /// 为什么把色表压到 **16 色**而不是用满色表：色表密的时候两种度量几乎总是一致
    /// （实测 256 色下分歧为 0 —— 那种情况下挑哪个都行）；色表越疏，"挑哪个"越决定
    /// 画质，也就是这个度量真正起作用的地方。先断言"确实有分歧"，免得门禁退化成
    /// 空断言。
    #[test]
    fn oklab_distance_is_more_accurate_than_plain_rgb_distance() {
        const W: usize = 256;
        const H: usize = 160;
        // 斜向双渐变：颜色数远超色表容量，逼出"必须近似"的场景。
        let mut rgb = vec![0u8; W * H * 3];
        for y in 0..H {
            for x in 0..W {
                let p = (y * W + x) * 3;
                rgb[p] = (x * 255 / (W - 1)) as u8;
                rgb[p + 1] = (y * 255 / (H - 1)) as u8;
                rgb[p + 2] = ((x + y) * 255 / (W + H - 2)) as u8;
            }
        }
        let pal = Palette::from_rgb(&rgb, 16).expect("调色板");
        assert_eq!(pal.len(), 16, "该用例需要一张 16 色表");
        let mut q = Quantizer::new(pal.clone());
        let mut idx = Vec::new();
        q.map_into(&rgb, &mut idx);
        let colors = pal.colors();

        let (mut sum_old, mut sum_new, mut worst_old, mut worst_new) = (0f64, 0f64, 0f32, 0f32);
        let mut disagree = 0usize;
        for (i, px) in rgb.chunks_exact(3).enumerate() {
            let want = [px[0], px[1], px[2]];
            let ia = old_rgb_nearest(&pal, px[0], px[1], px[2]) as usize;
            let ib = idx[i] as usize;
            if ia != ib {
                disagree += 1;
            }
            let d_old = de100(colors[ia], want);
            let d_new = de100(colors[ib], want);
            sum_old += d_old as f64;
            sum_new += d_new as f64;
            worst_old = worst_old.max(d_old);
            worst_new = worst_new.max(d_new);
        }
        let n = (W * H) as f64;
        eprintln!(
            "[色差] 渐变 {W}×{H} / 16 色表：两种度量在 {disagree}/{n:.0} 个像素上选择不同；\
             平均 ΔE×100 —— RGB 距离 {:.2} → OKLab {:.2}；最差 {:.1} → {:.1}",
            sum_old / n,
            sum_new / n,
            worst_old,
            worst_new
        );
        assert!(
            disagree > 0,
            "两种度量本该在这张图上分歧，否则这条门禁测不出东西"
        );
        assert!(
            sum_new < sum_old,
            "OKLab 最近色必须让感知色差更小：OKLab {:.2} vs RGB {:.2}",
            sum_new / n,
            sum_old / n
        );
    }

    /// 产出 `temp/gif_quality/ribbon_before_after.png` 供人眼评审（**不做断言**）。
    ///
    /// ```text
    /// cargo test --release --lib dump_ribbon_quality_png -- --ignored --nocapture
    /// ```
    ///
    /// 三条横带，**内容与位置完全相同、都放大 4 倍**，唯一差别是"用哪张色表量化"：
    ///   ① 屏幕上真实的样子（还没量化）
    ///   ② 修复前：只看首帧选色
    ///   ③ 修复后：预热窗口的全像素直方图
    /// 带芯偏色与那一圈假边框，放大 4 倍后肉眼直接可辨 —— 真机截图这条路被 UIPI 断了
    /// （见 `examples/render_probe.rs` 头部），但"量化前后差多少"本来就不需要真机。
    #[test]
    #[ignore = "只产出 PNG 供人看，不进回归"]
    fn dump_ribbon_quality_png() {
        const W: usize = 320;
        const H: usize = 200;
        const Z: usize = 4; // 放大倍数
        const CW: usize = 80;
        const CH: usize = 60;
        let frames: Vec<Vec<u8>> = (0..9)
            .map(|k| {
                desktop_frame(
                    W,
                    H,
                    if k == 0 {
                        None
                    } else {
                        Some(60 + k as i64 * 8)
                    },
                )
            })
            .collect();
        let cx = 60 + 8 * 8;
        let accents = accent_colors();

        let old = Palette::from_rgb_with_accents(&frames[0], MAX_COLORS, &accents).expect("对照组");
        let mut hist = Hist::new();
        for f in &frames {
            hist.add_rgb(f);
        }
        let new =
            Palette::from_hist(Some(&frames[0]), &hist, MAX_COLORS, &accents).expect("修复组");

        let last = &frames[8];
        let mut q_old = Quantizer::new(old);
        let mut q_new = Quantizer::new(new);
        let (mut i_old, mut i_new) = (Vec::new(), Vec::new());
        q_old.map_into(last, &mut i_old);
        q_new.map_into(last, &mut i_new);
        let c_old = q_old.palette().colors().to_vec();
        let c_new = q_new.palette().colors().to_vec();

        let x0 = (cx as usize).saturating_sub(CW / 2);
        let y0 = (H - CH) / 2;
        let crop = |idx: Option<&Vec<u8>>, colors: &[[u8; 3]]| -> Vec<[u8; 3]> {
            let mut out = Vec::with_capacity(CW * CH);
            for y in y0..y0 + CH {
                for x in x0..x0 + CW {
                    let p = (y * W + x) * 3;
                    out.push(match idx {
                        None => [last[p], last[p + 1], last[p + 2]],
                        Some(i) => colors[i[y * W + x] as usize],
                    });
                }
            }
            out
        };
        let strips = [
            crop(None, &[]),
            crop(Some(&i_old), &c_old),
            crop(Some(&i_new), &c_new),
        ];

        let mut px: Vec<u8> = Vec::with_capacity(CW * Z * 3 * (CH * Z * 3 + 4));
        for (si, s) in strips.iter().enumerate() {
            if si > 0 {
                // 两条带之间压一条 2px 深色分隔线，免得三张糊成一张。
                for _ in 0..(CW * Z * 2) {
                    px.extend_from_slice(&[0, 0, 0, 255]);
                }
            }
            for y in 0..CH {
                for _ in 0..Z {
                    for x in 0..CW {
                        let c = s[y * CW + x];
                        for _ in 0..Z {
                            px.extend_from_slice(&[c[0], c[1], c[2], 255]);
                        }
                    }
                }
            }
        }

        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("temp")
            .join("gif_quality");
        std::fs::create_dir_all(&dir).expect("建目录");
        let out = dir.join("ribbon_before_after.png");
        let file = std::fs::File::create(&out).expect("建 PNG");
        let (w, h) = ((CW * Z) as u32, (CH * Z * 3 + 4) as u32);
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().expect("PNG 头");
        wr.write_image_data(&px).expect("PNG 数据");
        wr.finish().expect("PNG 收尾");
        println!(
            "画质对比 PNG（① 真实 ② 修复前 ③ 修复后）: {}",
            out.display()
        );
    }
}
