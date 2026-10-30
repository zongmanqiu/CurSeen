//! 位图与 PNG 解码（含「取最大正方形」裁剪这个**可选**工具）。
//!
//! core 层只认**非预乘 RGBA8**这一种像素货币：解码方（`platform::image_loader`）产出它，
//! 渲染层（`runtime::render`）再转成后端要的格式。统一货币是为了让算法层与渲染层都只需要
//! 对付一种布局，不必各自记一遍预乘与否。
//!
//! PNG/GIF **解码**入口目前唯一的消费者是悬浮标的「图片」类型：PNG 由 [`load_png`]
//! **原样**解码（保持宽高比，2026-09-22 起不再裁正方形），GIF 由 `core::gif` 逐帧解码；
//! 两者都按「缩放」倍数整体缩放后把中心对准鼠标（见 `scripting::api_draw` 的 `ctx:image`）。
//! [`Shot`]/[`RgbaBitmap`] 本身则是截图/标注链（`core::backdrop`）的像素底座。

use std::io::BufReader;
use std::path::Path;

use super::geometry::Rect;

/// 抓取到的一帧（BGRA，行自上而下，行内紧密排列）。
///
/// 与 [`RgbaBitmap`] 的区别是**通道顺序**（BGRA vs RGBA）：`Shot` 对齐 GDI `BitBlt`
/// 的直接产出，`RgbaBitmap` 是本项目的统一像素货币（见模块文档）。转换很廉价，但二者
/// 不可混用。
///
/// 之所以放在 `core`：它是纯数据（`Vec<u8>` + 尺寸），而 `core::backdrop` 要用它做
/// "帧 ⇄ 位图"的唯一转换点。真正在它后面调 Win32 的是 `platform::screenshot::grab`，
/// 那里保留了同名重导出，既有 `platform::screenshot::Shot` 的引用不必改。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shot {
    pub w: i32,
    pub h: i32,
    pub bgra: Vec<u8>,
}

impl Shot {
    /// 转成 RGBA（`Vec<u8>`），PNG 编码与剪贴板都要这个顺序。
    pub fn to_rgba(&self) -> Vec<u8> {
        let n = (self.w.max(0) * self.h.max(0) * 4) as usize;
        let mut out = Vec::with_capacity(n);
        for px in self.bgra.chunks_exact(4) {
            out.push(px[2]);
            out.push(px[1]);
            out.push(px[0]);
            out.push(px[3]);
        }
        out
    }

    /// [`Self::to_rgba`] 的消费版：就地换 R/B，复用原缓冲（α 原样保留）。
    ///
    /// 截图确认链每张图都要转一次：借用版要**再分配**一份 `w*h*4` 并逐字节 push，
    /// 1920×1080 峰值多 8.3 MB；换成原地换序后整条「帧→位图」从 median 12.1 ms 降到
    /// 3.3 ms（`backdrop::shot_to_bitmap_owned`，n=200）。还要保留原帧的调用方
    /// （目前只剩 `Backdrop::from_shot` 的借用签名）继续用借用版。
    pub fn into_rgba(mut self) -> Vec<u8> {
        for px in self.bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        self.bgra
    }

    /// 是否有像素。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    /// 裁出子区域（区域截图的确认步）。`x/y` 为相对本帧左上角的物理像素偏移，
    /// 允许为负（选区从冻结帧边缘拖出时钳到帧内）；宽高 < 1 时返回 `None`。
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> Option<Shot> {
        if w <= 0 || h <= 0 || self.w <= 0 || self.h <= 0 {
            return None;
        }
        // 钳到帧内
        let x0 = x.clamp(0, self.w);
        let y0 = y.clamp(0, self.h);
        let w = w.min(self.w - x0);
        let h = h.min(self.h - y0);
        if w <= 0 || h <= 0 {
            return None;
        }
        let stride = (self.w as usize) * 4;
        let mut bgra = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for row in 0..h as usize {
            let src = (y0 as usize + row) * stride + (x0 as usize) * 4;
            bgra.extend_from_slice(&self.bgra[src..src + (w as usize) * 4]);
        }
        Some(Shot { w, h, bgra })
    }
}

/// 贴窗裁剪前要从每边收进多少像素（见 [`RgbaBitmap::frame_trim`]）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTrim {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl FrameTrim {
    /// 四边都没多（绝大多数截图都是这一种）。
    pub fn is_empty(&self) -> bool {
        self.left == 0 && self.right == 0 && self.top == 0 && self.bottom == 0
    }

    /// 收进来的总像素数（日志用）。
    pub fn total(&self) -> u32 {
        self.left + self.right + self.top + self.bottom
    }
}

/// 每边最多收进几像素。
///
/// 真机实测（`build/temp/screenshot` 里用户的智能选区产物）见到的规模就是 **1–2px**：
/// 1px = 半透明边框外侧多出来的一列，2px = 窗口自身留的透明缩放边。
/// 再大就不该由这条启发式来猜了（超大偏差说明推荐框本身错了，压着收会把窗户吃掉）。
pub const FRAME_TRIM_MAX: u32 = 2;

/// "相邻两个像素是同一片颜色"的容差（每通道）。
///
/// 取 2 的依据：桌面壁纸/窗口内容的渐变在**相邻**像素上远小于它；而真边界上那 1px 是
/// DWM 的半透明边框，它与外侧桌面至少差 `0.6 × |边框 − 桌面|`（深色桌面上实测 40+）。
const CONTINUATION_TOL: i32 = 2;

/// 矩形的一条边（[`RgbaBitmap::frame_trim`] 的入参）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// 某一环与外圈之外那一行的比较结果（[`RgbaBitmap::ring_vs_outside`]）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ring {
    /// 与外圈之外基本一致 —— 这一环不属于窗户，可以收。
    Same,
    /// 与外圈之外不同 —— 到边界了，从外面数到这里为止。
    Differs,
    /// 取不到样本（贴帧边界 / 尺寸不够）—— 判不出来，不动。
    Unknown,
}

/// 两个 RGB 是不是"同一片颜色"（每通道差 ≤ [`CONTINUATION_TOL`]）。
fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= CONTINUATION_TOL)
}

/// 非预乘 RGBA8 位图。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaBitmap {
    /// 宽（像素）。
    pub w: u32,
    /// 高（像素）。
    pub h: u32,
    /// 行优先、非预乘 RGBA8，长度恒为 `w * h * 4`。
    pub rgba: Vec<u8>,
}

impl RgbaBitmap {
    /// 由尺寸与像素构造。长度不符返回 `None`（拒绝而不是 panic：图片来自用户磁盘，
    /// 解码器与尺寸表不一致时应当只是"这张图用不了"，不能拖垮整个进程）。
    pub fn new(w: u32, h: u32, rgba: Vec<u8>) -> Option<Self> {
        let need = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
        if w == 0 || h == 0 || rgba.len() != need {
            return None;
        }
        Some(Self { w, h, rgba })
    }

    /// 是否没有像素。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// 是否已是正方形。
    #[inline]
    pub fn is_square(&self) -> bool {
        self.w == self.h
    }

    /// 从**左上角**起向右下角裁出能取到的最大正方形：边长 `s = min(w, h)`，
    /// 即取原图的 `(0, 0, s, s)`。
    ///
    /// 原图本就是正方形时返回自身的一份廉价引用标记——调用方靠 [`Self::is_square`]
    /// 先判一次即可避免拷贝。
    pub fn crop_square_top_left(&self) -> Self {
        if self.is_empty() {
            return self.clone();
        }
        let s = self.w.min(self.h);
        if s == self.w && s == self.h {
            return self.clone();
        }
        let mut out = Vec::with_capacity((s as usize) * (s as usize) * 4);
        let row_bytes = (self.w as usize) * 4;
        for y in 0..s as usize {
            let start = y * row_bytes;
            out.extend_from_slice(&self.rgba[start..start + (s as usize) * 4]);
        }
        Self {
            w: s,
            h: s,
            rgba: out,
        }
    }

    /// 裁出子矩形（**像素坐标，相对本图左上角**，可为负）。截图区域选择的确认步用它：
    /// 选区是从整张冻结帧上裁的，而冻结帧就是这块位图。
    ///
    /// 钳到图内（拖出边缘时收到边界），宽高 < 1 时返回 `None`。与
    /// [`crate::platform::screenshot::Shot::crop`] 同款语义——那份是给 BGRA 帧用的，
    /// 这份是给非预乘 RGBA 位图用的，两条链各自不用做一次颜色序转换。
    pub fn crop(&self, x: i32, y: i32, w: i32, h: i32) -> Option<Self> {
        if w <= 0 || h <= 0 || self.is_empty() {
            return None;
        }
        let x0 = x.clamp(0, self.w as i32);
        let y0 = y.clamp(0, self.h as i32);
        let w = w.min(self.w as i32 - x0);
        let h = h.min(self.h as i32 - y0);
        if w <= 0 || h <= 0 {
            return None;
        }
        let stride = (self.w as usize) * 4;
        let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for row in 0..h as usize {
            let start = (y0 as usize + row) * stride + (x0 as usize) * 4;
            rgba.extend_from_slice(&self.rgba[start..start + (w as usize) * 4]);
        }
        Some(Self {
            w: w as u32,
            h: h as u32,
            rgba,
        })
    }

    /// 贴着窗口外框的裁剪：裁出 `(x, y, w, h)`，并把**最外圈 1px** 从"混了窗口外颜色的
    /// 半透明边框"**反解回边框本色**。
    ///
    /// # 两趟，顺序不能反
    ///
    /// ① [`Self::frame_trim`]：把"外圈与外圈之外连成一片"的那些边收进来 —— 那一两列
    ///    根本不属于窗户（用户第三次反馈的"依然会截取到边框线外"）。
    /// ② 反解**收完之后**的最外圈：那时候它才是窗户的边框线。
    ///
    /// # 为什么要反解
    ///
    /// DWM 给每个非最大化窗口画一圈 **40% 不透明**的边框线，它就落在
    /// `DWMWA_EXTENDED_FRAME_BOUNDS` 的最外圈。抓屏拿到的是它与背景**合成后**的结果
    /// （`alpha` 恒 255，GDI 不给半透明）：
    ///
    /// ```text
    /// obs = (1 - alpha) * bg + alpha * border
    /// ```
    ///
    /// 于是"照外框裁"的产物边缘必然混进 `(1 - alpha)` = 60% 的桌面色 —— 用户放大看就是
    /// "边框线外的颜色被截进来了"。而**内缩 1px 躲开它**又会把边框线整条裁掉
    /// （用户同样不满意，因为窗口没有第二条边框）。
    ///
    /// 剩下唯一自洽的做法：**区域照旧（含边框线），只把颜色反解成本色**。
    ///
    /// # 反解要用到"窗口外侧那 1px"
    ///
    /// `bg` 只能从**外侧紧邻的桌面像素**取 —— 它不在裁剪结果里，所以这里是从**本帧**
    /// （整张冻结帧）上按世界坐标直接取。哪条边贴着帧边界（窗口顶到屏幕边）就拿不到
    /// `bg`，那条边**原样保留**（退化，不猜）。
    ///
    /// ★★ 2026-09-28 定案：**反解已整体删除，像素一律原样保留。**
    ///
    /// 此前这里会按 `obs = a·边框 + (1-a)·bg` 把最外圈 1px 反解成"不透明本色"，
    /// 其中 `bg` 取自边框外侧 1px 的**可见**像素。这个模型先天站不住：边框后面的
    /// 真实背景本来就被边框盖住、**不可见**，外侧 1px 常常是 DWM 阴影、相邻窗口或
    /// 完全另一片内容——bg 一错，反解就产出屏幕上根本不存在的颜色（用户先后报过
    /// "灰色边框变黑色""边框依然有问题"）。成熟截图工具的通行做法：都只用
    /// `DwmGetWindowAttribute(EXTENDED_FRAME_BOUNDS)` 定窗框、像素原样保留——
    /// 半透明边框在屏上是什么样，截出来就是什么样。我们也走这条路。
    ///
    /// `alpha` 参数保留（调用方继续传 `FRAME_BORDER_ALPHA`），仅作签名兼容、不再使用。
    pub fn crop_frame_opaque(&self, x: i32, y: i32, w: i32, h: i32, _alpha: f32) -> Option<Self> {
        // ★ 先把"不属于窗户的那一两列"收掉，再裁（见 [`Self::frame_trim`]）。
        let t = self.frame_trim(x, y, w, h);
        if !t.is_empty() {
            log::info!(
                "贴窗裁剪：外圈不属于窗户，四边收进 L{} T{} R{} B{}",
                t.left,
                t.top,
                t.right,
                t.bottom
            );
        }
        let (x, y, w, h) = (
            x + t.left as i32,
            y + t.top as i32,
            w - (t.left + t.right) as i32,
            h - (t.top + t.bottom) as i32,
        );
        self.crop(x, y, w, h)
    }

    /// 智能贴窗裁剪前先问一句：**这一刀的四条边，真的落在窗户边界上吗？**
    /// # 判据
    ///
    /// 只有一条 —— **外圈与外圈之外是不是同一片颜色**（"连成一片"）：
    ///   · 真边界上那 1px 是 DWM 的**半透明**边框，它与外侧的桌面必然不同（深色桌面上
    ///     实测差 40+）；
    ///   · 若外圈与外圈之外几乎一模一样（差 ≤ [`CONTINUATION_TOL`]），说明这条边落在
    ///     窗户的**内部或外部** —— 这一圈根本不属于窗户，该收进来。
    ///
    /// # 为什么要这条
    ///
    /// 用户 2026-09-23 第三次报同一件事（*"依然会截取到边框线外…我截图并放大观察，发现
    /// 边框线外的颜色依然被截取了。重复多次验证，依然如此。"*）。拿他真机产物的**逐像素**
    /// 数据反查（`build/temp/screenshot/CurSeen_2026-09-23_03-25-30.png`，704×608）：
    ///
    /// ```text
    /// 上边 第 0 行  : (118,118,118) 整行     ← 反解后的边框线，正确
    /// 下边 第 607 行: (116,118,118) 整行     ← 正确
    /// 左边 第 0 列  : (78,78,85)/(85,70,70)/(117,81,37)… 随 y 变   ← 不是边框
    /// 右边 倒数两列 : (76,76,76) 恒定                            ← 不是边框
    /// ```
    ///
    /// 关键在**反解的幂等性**：`unmix(obs, bg)` 在 `obs == bg` 时返回 `obs` 本身。
    /// 那两列为 `(117,81,37)` 之类的值时反解结果**必须**等于边框本色，实测却不是
    /// （`(117,81,37)` 连"0.6×背景 + 0.4×边框"都拼不出来 —— 蓝通道解出 −17）。
    /// 结论：**左右两条边的外圈不是窗户的边框**，`EXT` 那一刀在水平方向宽了（左 1px、
    /// 右 2px），裁进来的是窗外的东西。
    ///
    /// # ★ 保守的一侧
    ///
    /// 上限 [`FRAME_TRIM_MAX`] 之内**一次边界都没数到**（每一环都与外圈之外连成一片）
    /// → **一点都不动**。纯色图、最大化窗口压在纯色桌面上这两类情形会落进这里 ——
    /// 无从判断边界在哪，少做比乱削安全。
    ///
    /// 每边的判定都要求 **90% 以上**的像素一致：圆角、别的窗口压住一段边这类"只有一段
    /// 不成立"的情况会被算成"边界"（同样是保守的那一侧）。
    pub fn frame_trim(&self, x: i32, y: i32, w: i32, h: i32) -> FrameTrim {
        let mut t = FrameTrim::default();
        let (x0, y0) = (x, y);
        // 4：收到只剩 4px 还判不出边界，说明这一刀本来就没落在窗户上，停手更安全。
        if w > 4 {
            t.left = self.trim_side(x0, y0, w, h, Side::Left);
            t.right = self.trim_side(x0, y0, w, h, Side::Right);
        }
        if h > 4 {
            t.top = self.trim_side(x0, y0, w, h, Side::Top);
            t.bottom = self.trim_side(x0, y0, w, h, Side::Bottom);
        }
        // 四边收进之后必须还剩得下东西（收到 < 4px 就整条作废，宁可原样）。
        if (w as u32) < t.left + t.right + 4 || (h as u32) < t.top + t.bottom + 4 {
            return FrameTrim::default();
        }
        t
    }

    /// 一条边该收进几像素（0 = 不动）。
    ///
    /// 从外往里数环：**与外圈之外连成一片**的环都算"多出来的"，数到第一个**与外圈之外
    /// 不同**的环（= 真正的边界）就是该收的像素数。
    ///
    /// ★ 上限 [`FRAME_TRIM_MAX`] 之内**一次边界都没数到** → 收 0。这是为了"整张纯色图"
    /// 这类情形：每一环都与外圈相同，无从判断边界在哪，就一点都不动（宁可少做）。
    /// 拿不到外侧像素（边贴着帧边界，例如窗口顶到屏幕边）→ 同样不动。
    fn trim_side(&self, x0: i32, y0: i32, w: i32, h: i32, side: Side) -> u32 {
        for k in 0..=FRAME_TRIM_MAX as i32 {
            match self.ring_vs_outside(x0, y0, w, h, side, k) {
                // 边界找到了：外面前 k 环都是多余的。
                Ring::Differs => return k as u32,
                Ring::Same => {}
                // 取不到样本（贴帧边界 / 尺寸不够）：不动。
                Ring::Unknown => return 0,
            }
        }
        0
    }

    /// 第 `k` 环（从选区外圈往里数，`k = 0` 就是选区最外那一圈）与外圈**之外**那一行的关系。
    fn ring_vs_outside(&self, x0: i32, y0: i32, w: i32, h: i32, side: Side, k: i32) -> Ring {
        let n = match side {
            Side::Left | Side::Right => h,
            Side::Top | Side::Bottom => w,
        };
        if n <= 0 {
            return Ring::Unknown;
        }
        let (mut same, mut total) = (0i32, 0i32);
        for i in 0..n {
            let (r, o) = match side {
                Side::Left => ((x0 + k, y0 + i), (x0 - 1, y0 + i)),
                Side::Right => ((x0 + w - 1 - k, y0 + i), (x0 + w, y0 + i)),
                Side::Top => ((x0 + i, y0 + k), (x0 + i, y0 - 1)),
                Side::Bottom => ((x0 + i, y0 + h - 1 - k), (x0 + i, y0 + h)),
            };
            let (Some(r), Some(o)) = (self.rgb_at(r.0, r.1), self.rgb_at(o.0, o.1)) else {
                return Ring::Unknown;
            };
            total += 1;
            if near(r, o) {
                same += 1;
            }
        }
        // 90% 以上"连成一片"才算同一片：圆角/被别的窗口压住一段边这类只有一段不成立的
        // 情况，会被算成"边界"（保守的那一侧）。
        if total > 0 && same * 10 >= total * 9 {
            Ring::Same
        } else {
            Ring::Differs
        }
    }

    /// `(x, y)` 处的 RGB（越界返回 `None`）。给 [`Self::frame_trim`] 的环比对取样用。
    fn rgb_at(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return None;
        }
        let i = ((y as usize) * (self.w as usize) + (x as usize)) * 4;
        Some([self.rgba[i], self.rgba[i + 1], self.rgba[i + 2]])
    }

    /// 裁出子矩形并转成 **BGRA** 帧（[`Shot`]）。
    ///
    /// 与 [`Self::crop`] 再换一次通道等价，但**一趟做完**：区域选择窗的放大镜每移动一个
    /// 像素就要从冻结帧取一小块喂给 `platform::pip_window`（那块缓冲是 BGRA 内存序），
    /// 中间再落一份 RGBA 副本纯属浪费。
    ///
    /// 钳法与"宽高 < 1 返回 `None`"与 [`Self::crop`] 逐字一致——两处不同步的表现是
    /// "放大镜在屏幕边缘缺一块"。
    pub fn crop_to_shot(&self, x: i32, y: i32, w: i32, h: i32) -> Option<Shot> {
        if w <= 0 || h <= 0 || self.is_empty() {
            return None;
        }
        let x0 = x.clamp(0, self.w as i32);
        let y0 = y.clamp(0, self.h as i32);
        let w = w.min(self.w as i32 - x0);
        let h = h.min(self.h as i32 - y0);
        if w <= 0 || h <= 0 {
            return None;
        }
        let stride = (self.w as usize) * 4;
        let row_bytes = (w as usize) * 4;
        let mut bgra = Vec::with_capacity(row_bytes * h as usize);
        for row in 0..h as usize {
            let start = (y0 as usize + row) * stride + (x0 as usize) * 4;
            for px in self.rgba[start..start + row_bytes].chunks_exact(4) {
                bgra.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        Some(Shot { w, h, bgra })
    }

    /// 把这块位图铺进目标矩形时的**源区域**（当前即整块；裁剪已在 [`Self::crop_square_top_left`]
    /// 完成）。留这个函数是为了让"取图规则"只有一个出处，渲染层不必自己猜。
    #[inline]
    pub fn source_rect(&self) -> Rect {
        Rect::new(0.0, 0.0, self.w as f32, self.h as f32)
    }

    /// 顺时针旋转 90°：宽高互换，内容整体右转。
    ///
    /// 贴图右键菜单「时间针90度」用。纯像素搬运，通道逐个拷贝（张量和滤波都不做，
    /// 90° 是刚性的、不会丢信息）。返回新位图，调用方决定替换 / 另存。
    pub fn rotate_cw90(&self) -> RgbaBitmap {
        if self.is_empty() {
            return self.clone();
        }
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0_u8; w * h * 4];
        // 顺时针 90°：源 (x, y) → 目标 (W-1-y, x)（列/行互换成行/列）。
        // 于是目标 (x', y') = 源 (x = y', y = W-1-x')。
        for sy in 0..h {
            for sx in 0..w {
                let dx = h - 1 - sy; // 目标列
                let dy = sx; // 目标行
                let s = (sy * w + sx) * 4;
                let d = (dy * h + dx) * 4;
                out[d..d + 4].copy_from_slice(&self.rgba[s..s + 4]);
            }
        }
        RgbaBitmap {
            w: self.h,
            h: self.w,
            rgba: out,
        }
    }

    /// 水平镜像（左右翻转）：`(x, y)` ← `(W-1-x, y)`。尺寸不变。
    pub fn flip_h(&self) -> RgbaBitmap {
        if self.is_empty() {
            return self.clone();
        }
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0_u8; w * h * 4];
        for y in 0..h {
            let row = &self.rgba[y * w * 4..(y + 1) * w * 4];
            for x in 0..w {
                let s = x * 4;
                let d = (w - 1 - x) * 4;
                out[y * w * 4 + d..y * w * 4 + d + 4].copy_from_slice(&row[s..s + 4]);
            }
        }
        RgbaBitmap {
            w: self.w,
            h: self.h,
            rgba: out,
        }
    }

    /// 垂直镜像（上下翻转）：`(x, y)` ← `(x, H-1-y)`。尺寸不变。
    pub fn flip_v(&self) -> RgbaBitmap {
        if self.is_empty() {
            return self.clone();
        }
        let (w, h) = (self.w as usize, self.h as usize);
        let mut out = vec![0_u8; w * h * 4];
        for y in 0..h {
            let srow = &self.rgba[(h - 1 - y) * w * 4..(h - y) * w * 4];
            out[y * w * 4..(y + 1) * w * 4].copy_from_slice(srow);
        }
        RgbaBitmap {
            w: self.w,
            h: self.h,
            rgba: out,
        }
    }

    /// 从整图里取一小块，按整数倍放大（最近邻）。
    ///
    /// 返回的位图边长 = `src 边长 × zoom`，超界处补黑（贴屏幕边时不留空洞）。
    ///
    /// ★ 2026-09-17 起**没有调用方**：它原本只服务截图区域窗的放大镜，而放大镜已按用户
    /// 要求整块删除（"有一个带红色边框的小矩形…要删除"）。保留为纯图像工具——无平台调用、
    /// 无副作用，将来若要再做"放大取色"之类的功能可直接复用（与
    /// [`crate::platform::file_dialog::open_folder`] 同一种处置）。
    pub fn magnify(&self, cx: i32, cy: i32, half_w: i32, half_h: i32, zoom: i32) -> Option<Self> {
        if self.is_empty() || half_w <= 0 || half_h <= 0 || zoom <= 0 {
            return None;
        }
        let (ow, oh) = (half_w * 2, half_h * 2);
        let mut rgba = vec![0_u8; (ow * oh * 4) as usize];
        for oy in 0..oh {
            let sy = cy - half_h + oy / zoom;
            if sy < 0 || sy >= self.h as i32 {
                continue;
            }
            for ox in 0..ow {
                let sx = cx - half_w + ox / zoom;
                if sx < 0 || sx >= self.w as i32 {
                    continue;
                }
                let s = ((sy as usize) * (self.w as usize) + sx as usize) * 4;
                let d = ((oy as usize) * (ow as usize) + ox as usize) * 4;
                rgba[d..d + 4].copy_from_slice(&self.rgba[s..s + 4]);
            }
        }
        Self::new(ow as u32, oh as u32, rgba)
    }
}

/// 圆形桌面镜片的取样方式。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LensMode {
    /// 放大：`zoom` 是放大倍数（1.0 = 原样）。
    Magnify { zoom: f32 },
    /// 逐通道求差：`|源 − color|`，对应 CSS 的 `mix-blend-mode: difference`。
    Difference { color: (u8, u8, u8) },
}

/// 从一张抓屏里取出**圆形**镜片：缩放/反色 + 1px 羽化蒙版。
///
/// 返回边长 `2×radius` 的非预乘 RGBA 位图，圆心 = 源图中心；圆外 alpha = 0。
/// 采样用双线性（放大时比最近邻少一层锯齿），源坐标钳到边缘（贴屏边不会出现黑环）。
///
/// ★ 为什么放在 core：这是纯像素运算（输入 [`Shot`]、输出 [`RgbaBitmap`]，无平台调用），
///   与 [`RgbaBitmap::magnify`] 同一层；`scripting` 只负责"抓哪块"和"画到哪"。
pub fn lens_circle(shot: &Shot, radius: u32, mode: LensMode) -> Option<RgbaBitmap> {
    if shot.is_empty() || radius == 0 {
        return None;
    }
    let r = radius as i32;
    let size = r * 2;
    let (sw, sh) = (shot.w, shot.h);
    if sw < 1 || sh < 1 {
        return None;
    }
    let zoom = match mode {
        LensMode::Magnify { zoom } => {
            if !zoom.is_finite() || zoom <= 0.0 {
                1.0
            } else {
                zoom
            }
        }
        LensMode::Difference { .. } => 1.0,
    };
    let diff_color = match mode {
        LensMode::Difference { color } => Some(color),
        LensMode::Magnify { .. } => None,
    };

    let mut rgba = vec![0_u8; (size * size * 4) as usize];
    let src_cx = sw as f32 / 2.0;
    let src_cy = sh as f32 / 2.0;
    let r_f = r as f32;

    for oy in 0..size {
        let dy = oy as f32 + 0.5 - r_f;
        for ox in 0..size {
            let dx = ox as f32 + 0.5 - r_f;
            let dist = (dx * dx + dy * dy).sqrt();
            // 边缘 1px 羽化：圆外完全透明，圆内完全保留，避免硬锯齿。
            let cover = (r_f - dist + 0.5).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let sx = src_cx + dx / zoom;
            let sy = src_cy + dy / zoom;
            let (mut cr, mut cg, mut cb) = sample_bilinear(shot, sx, sy);
            if let Some((tr, tg, tb)) = diff_color {
                cr = cr.abs_diff(tr);
                cg = cg.abs_diff(tg);
                cb = cb.abs_diff(tb);
            }
            let d = ((oy as usize) * (size as usize) + ox as usize) * 4;
            rgba[d] = cr;
            rgba[d + 1] = cg;
            rgba[d + 2] = cb;
            rgba[d + 3] = (cover * 255.0).round() as u8;
        }
    }
    RgbaBitmap::new(size as u32, size as u32, rgba)
}

/// 双线性取样（源坐标为浮点；越界钳到边缘）。
fn sample_bilinear(shot: &Shot, x: f32, y: f32) -> (u8, u8, u8) {
    let max_x = (shot.w - 1) as f32;
    let max_y = (shot.h - 1) as f32;
    let x = x.clamp(0.0, max_x);
    let y = y.clamp(0.0, max_y);
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = (x0 + 1).min(shot.w - 1);
    let y1 = (y0 + 1).min(shot.h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let px = |xx: i32, yy: i32| -> (f32, f32, f32) {
        let i = ((yy as usize) * (shot.w as usize) + xx as usize) * 4;
        (
            shot.bgra[i + 2] as f32,
            shot.bgra[i + 1] as f32,
            shot.bgra[i] as f32,
        )
    };
    let (r00, g00, b00) = px(x0, y0);
    let (r10, g10, b10) = px(x1, y0);
    let (r01, g01, b01) = px(x0, y1);
    let (r11, g11, b11) = px(x1, y1);
    let mix = |a: f32, b: f32, c: f32, d: f32| -> u8 {
        let top = a + (b - a) * fx;
        let bottom = c + (d - c) * fx;
        (top + (bottom - top) * fy).round().clamp(0.0, 255.0) as u8
    };
    (
        mix(r00, r10, r01, r11),
        mix(g00, g10, g01, g11),
        mix(b00, b10, b01, b11),
    )
}

/// 从本地 PNG 文件解码，得到悬浮标「图片」样式要用的位图（**保持原始宽高**）。
///
/// 走 `png` crate（core 层不碰 windui）。★ 色型鲁棒性：用户随手存的 PNG 什么色型都有——
/// 实测一张 1-bit 调色板 + tRNS 的 134×134 图标被旧实现（只认 RGBA/RGB）静默拒收，表现为
/// "选了图但完全看不见"。现在用 `normalize_to_color8` 把调色板/低位深灰度/16-bit 全部
/// 展开/降位成 8-bit，再统一映射到 RGBA。任何失败都写警告日志（不再静默）。
///
/// ★ 不裁正方形（2026-09-22 改）：早期版本裁"左上角最大正方形"，非方图会被切掉一条边。
/// 现在原样返回 `w×h`，宽高比由绘制端保证——脚本只给一个「缩放」倍数，宿主按
/// `w*scale × h*scale` 铺开、中心对准鼠标。
pub fn load_png(path: &Path) -> Option<RgbaBitmap> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = match decoder.read_info() {
        Ok(r) => r,
        Err(e) => {
            log::warn!("悬浮标图片 {} 不是有效 PNG：{e}", path.display());
            return None;
        }
    };
    // png 0.18 的 `read_info` 只返回 `Reader`（旧版是 `(Info, Reader)` 元组）。`info()` 借用
    // `reader`，故先把需要的字段拷出来，再调 `next_frame` 取像素，避免借用冲突。
    let (w, h) = {
        let info = reader.info();
        (info.width, info.height)
    };
    // `output_buffer_size()` 返回 `Option<usize>`（尺寸未知时 None），取不到就放弃这张图。
    let Some(buf_size) = reader.output_buffer_size() else {
        log::warn!("悬浮标图片 {} 尺寸无法确定", path.display());
        return None;
    };
    let mut buf = vec![0u8; buf_size];
    let output = match reader.next_frame(&mut buf) {
        Ok(o) => o,
        Err(e) => {
            log::warn!("悬浮标图片 {} 解码失败：{e}", path.display());
            return None;
        }
    };
    let raw = &buf[..output.buffer_size()];

    // 归一化后可能的输出色型：RGBA8 / RGB8 / 灰度8 / 灰度+A8。统一成非预乘 RGBA8。
    let px = w as usize * h as usize;
    let rgba = match output.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => {
            let mut out = Vec::with_capacity(px * 4);
            for p in raw.chunks_exact(3) {
                out.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
            out
        }
        png::ColorType::Grayscale => {
            let mut out = Vec::with_capacity(px * 4);
            for &g in raw {
                out.extend_from_slice(&[g, g, g, 255]);
            }
            out
        }
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity(px * 4);
            for p in raw.chunks_exact(2) {
                out.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
            out
        }
        other => {
            log::warn!(
                "悬浮标图片 {} 归一化后仍是暂不支持的色型 {other:?}",
                path.display()
            );
            return None;
        }
    };
    if rgba.len() != px * 4 {
        log::warn!(
            "悬浮标图片 {} 像素长度不符：{} != {}x{}x4",
            path.display(),
            rgba.len(),
            w,
            h
        );
        return None;
    }
    // 原样返回：宽高比交给绘制端（见本函数文档）。
    RgbaBitmap::new(w, h, rgba)
}

/// 马赛克打码：从 `bmp`（覆盖 `screen` 那块虚拟桌面）里取出世界矩形 `rect` 的像素，
/// 按 `block` 见方的块做**平均色**填充，返回打码后的补丁位图与**钳进画面后的世界矩形**。
///
/// - `rect` 与位图坐标通过 `screen` 换算（世界 = 虚拟桌面物理像素，位图左上角 =
///   `screen` 左上角）；`rect` 超出画面的部分被钳掉 —— 补丁只覆盖真实存在的像素，
///   不给"画面外"编造内容。
/// - 块平均（而非抽点采样）：抽点会让细线/文字整块消失或残留半截，平均色才是用户
///   预期里"看不清但知道那里有东西"的马赛克。
/// - `block < 1` 视为 1（等于原样拷贝）。
pub fn mosaic_patch(
    bmp: &RgbaBitmap,
    rect: crate::core::geometry::Rect,
    screen: crate::platform::display::ScreenRect,
    block: f32,
) -> Option<(RgbaBitmap, crate::core::geometry::Rect)> {
    use crate::core::geometry::Rect;

    // 世界矩形 → 位图像素矩形（钳进画面）
    let fx0 = (rect.x.round() as i32).max(screen.x);
    let fy0 = (rect.y.round() as i32).max(screen.y);
    let fx1 = (rect.x.round() as i32 + rect.w.round() as i32).min(screen.x + screen.w);
    let fy1 = (rect.y.round() as i32 + rect.h.round() as i32).min(screen.y + screen.h);
    if fx1 <= fx0 || fy1 <= fy0 {
        return None;
    }
    let ox = (fx0 - screen.x) as u32;
    let oy = (fy0 - screen.y) as u32;
    let bw = (fx1 - fx0) as u32;
    let bh = (fy1 - fy0) as u32;
    if ox >= bmp.w || oy >= bmp.h {
        return None;
    }
    let bw = bw.min(bmp.w - ox);
    let bh = bh.min(bmp.h - oy);
    if bw == 0 || bh == 0 {
        return None;
    }

    let block = block.round().max(1.0) as i32;
    // ★★ 块网格**锚在源图原点**（2026-09-28 三返工）：马赛克画笔是"一码接一码"落
    //    下去的，两码的矩形各自独立。若每码都从自己的左上角起排块，同一块背景在
    //    两码里会被拆成不同的两半 —— 接缝处颜色对不上，肉眼就是"碎格没对齐"。
    //    锚到源图原点后，无论从哪儿落码，块边界在世界坐标里始终落在同一条线上。
    let px0 = ox as i32;
    let py0 = oy as i32;
    let px1 = px0 + bw as i32;
    let py1 = py0 + bh as i32;

    let mut out = vec![0_u8; (bw * bh * 4) as usize];
    let src = &bmp.rgba;
    let avg_block = |sx0: i32, sy0: i32, sx1: i32, sy1: i32| -> [u8; 4] {
        let (mut sr, mut sg, mut sb, mut sa) = (0_u64, 0_u64, 0_u64, 0_u64);
        for yy in sy0..sy1 {
            let row = yy as usize * bmp.w as usize;
            for xx in sx0..sx1 {
                let i = (row + xx as usize) * 4;
                sr += src[i] as u64;
                sg += src[i + 1] as u64;
                sb += src[i + 2] as u64;
                sa += src[i + 3] as u64;
            }
        }
        let n = ((sx1 - sx0) * (sy1 - sy0)) as u64;
        // n > 0：块范围恒非空（调用处已保证 sx1>sx0 / sy1>sy0）。
        [
            (sr / n) as u8,
            (sg / n) as u8,
            (sb / n) as u8,
            (sa / n) as u8,
        ]
    };

    // 遍历与补丁相交的**世界块**：块颜色 = 整块（钳进源图）的平均 —— 只平均相交
    // 部分会让相邻两码对同一块算出不同颜色，接缝立现。
    let mut cy = (py0 / block) * block;
    while cy < py1 {
        let sy1 = (cy + block).min(bmp.h as i32);
        if sy1 > cy {
            let mut cx = (px0 / block) * block;
            while cx < px1 {
                let sx1 = (cx + block).min(bmp.w as i32);
                if sx1 > cx {
                    let c = avg_block(cx, cy, sx1, sy1);
                    // 块 ∩ 补丁 的那一片写进补丁
                    let wx0 = cx.max(px0);
                    let wx1 = sx1.min(px1);
                    let wy0 = cy.max(py0);
                    let wy1 = sy1.min(py1);
                    for yy in wy0..wy1 {
                        let row = (yy - py0) as usize * bw as usize;
                        for xx in wx0..wx1 {
                            let o = (row + (xx - px0) as usize) * 4;
                            out[o] = c[0];
                            out[o + 1] = c[1];
                            out[o + 2] = c[2];
                            out[o + 3] = c[3];
                        }
                    }
                }
                cx += block;
            }
        }
        cy += block;
    }
    let dest = Rect {
        x: (screen.x + px0) as f32,
        y: (screen.y + py0) as f32,
        w: bw as f32,
        h: bh as f32,
    };
    RgbaBitmap::new(bw, bh, out).map(|p| (p, dest))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 消费版换序必须与借用版逐字节相同，且**不碰 α**（α 由 `shot_to_bitmap_owned` 补）。
    #[test]
    fn into_rgba_matches_to_rgba() {
        let shot = Shot {
            w: 2,
            h: 1,
            bgra: vec![0x11, 0x22, 0x33, 0x7F, 0x44, 0x55, 0x66, 0x00],
        };
        let want = shot.to_rgba();
        assert_eq!(want, vec![0x33, 0x22, 0x11, 0x7F, 0x66, 0x55, 0x44, 0x00]);
        assert_eq!(shot.clone().into_rgba(), want);
    }

    /// 马赛克打码：块平均 + 世界矩形钳进画面 + 贴边半块。
    #[test]
    fn mosaic_patch_averages_blocks_and_clamps_to_screen() {
        // 8×2 的"桌面"，screen 原点 (0,0)。左半 (0..4) 全黑，右半 (4..8) 全白。
        let mut rgba = vec![0_u8; 8 * 2 * 4];
        for y in 0..2usize {
            for x in 0..8usize {
                let i = (y * 8 + x) * 4;
                let v = if x < 4 { 0_u8 } else { 255_u8 };
                rgba[i] = v;
                rgba[i + 1] = v;
                rgba[i + 2] = v;
                rgba[i + 3] = 255;
            }
        }
        let bmp = RgbaBitmap::new(8, 2, rgba).expect("位图");
        let screen = crate::platform::display::ScreenRect::new(0, 0, 8, 2);

        // 打码整幅，块宽 4：左块平均 (0,0,0)，右块 (255,255,255)
        let (patch, dest) = mosaic_patch(
            &bmp,
            crate::core::geometry::Rect::new(0.0, 0.0, 8.0, 2.0),
            screen,
            4.0,
        )
        .expect("应出补丁");
        assert_eq!((dest.x, dest.y, dest.w, dest.h), (0.0, 0.0, 8.0, 2.0));
        assert_eq!((patch.w, patch.h), (8, 2));
        assert_eq!(patch.rgba[0..3], [0, 0, 0], "左块应为黑");
        // (x=4, y=1) 的像素：i = (y*w + x)*4 = (8+4)*4
        let mid = (8 + 4) * 4;
        assert_eq!(patch.rgba[mid..mid + 3], [255, 255, 255], "右块应为白");

        // 世界矩形超出画面右缘：钳到画面，补丁只覆盖存在的那部分
        let (patch, dest) = mosaic_patch(
            &bmp,
            crate::core::geometry::Rect::new(6.0, 0.0, 8.0, 2.0),
            screen,
            2.0,
        )
        .expect("应出补丁");
        assert_eq!((dest.x, dest.w), (6.0, 2.0), "右侧越界部分应被钳掉");
        assert_eq!((patch.w, patch.h), (2, 2));

        // 完全在画面外：不给补丁
        assert!(
            mosaic_patch(
                &bmp,
                crate::core::geometry::Rect::new(100.0, 100.0, 8.0, 2.0),
                screen,
                4.0
            )
            .is_none(),
            "画面外不该编造像素"
        );
    }

    /// ★★ 贴框裁剪：反解已删，**最外圈 1px 原样保留**（2026-09-28 定案）。
    ///
    /// 判据两边都钉：
    ///   ① `crop_frame_opaque` 裁出的最外圈 == 屏幕上的观测值 `(47,200,47)`——
    ///      半透明边框在屏上什么样，截出来就什么样；
    ///   ② 尺寸不变、内部客户区一像素不动。
    /// 数值来自真机实测（`platform::window_probe::FRAME_BORDER_ALPHA`）：
    /// `obs = 0.6*bg + 0.4*118`，绿底 `(0,255,0)` 上得 `(47,200,47)`。
    #[test]
    fn crop_frame_opaque_keeps_the_translucent_border_verbatim() {
        // 6×5 的"桌面"，中间 4×3 是窗口：最外圈 = 混色边框，内部 = 白客户区。
        let obs: [u8; 3] = [47, 200, 47]; // 0.6*bg + 0.4*border
        let mut rgba = vec![0_u8; 6 * 5 * 4];
        for y in 0..5usize {
            for x in 0..6usize {
                let i = (y * 6 + x) * 4;
                let in_win = (1..5).contains(&x) && (1..4).contains(&y);
                let on_edge = in_win && (x == 1 || x == 4 || y == 1 || y == 3);
                let c = if !in_win {
                    [0, 255, 0]
                } else if on_edge {
                    obs
                } else {
                    [255, 255, 255]
                };
                rgba[i..i + 3].copy_from_slice(&c);
                rgba[i + 3] = 255;
            }
        }
        let src = RgbaBitmap::new(6, 5, rgba).expect("位图");
        let at = |b: &RgbaBitmap, x: usize, y: usize| -> [u8; 3] {
            let i = (y * (b.w as usize) + x) * 4;
            [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2]]
        };

        // ① 最外圈（含四角）逐像素 = 观测值：不做任何反解。
        let out = src
            .crop_frame_opaque(1, 1, 4, 3, 0.4)
            .expect("贴框裁剪应成功");
        assert_eq!((out.w, out.h), (4, 3), "尺寸不许变（边框线仍在产物里）");
        for (x, y) in [
            (0usize, 0usize),
            (1, 0),
            (2, 0),
            (3, 0),
            (0, 1),
            (3, 1),
            (0, 2),
            (1, 2),
            (2, 2),
            (3, 2),
        ] {
            assert_eq!(at(&out, x, y), obs, "({x},{y}) 应原样保留观测值");
        }
        // ② 内部像素一个都不许动。
        assert_eq!(at(&out, 1, 1), [255, 255, 255], "客户区必须原样");
        assert_eq!(at(&out, 2, 1), [255, 255, 255], "客户区必须原样");
    }

    /// 贴着帧边界裁时那一侧**拿不到背景**：不许猜，原样保留（退化而非算飞）。
    #[test]
    fn crop_frame_opaque_leaves_edges_without_a_background_sample() {
        let mut rgba = vec![0_u8; 4 * 4 * 4];
        for px in rgba.chunks_exact_mut(4) {
            px[0] = 47;
            px[1] = 200;
            px[2] = 47;
            px[3] = 255;
        }
        let src = RgbaBitmap::new(4, 4, rgba).expect("位图");
        // 裁整张：四条边都在帧边界上，一条都取不到 bg → 全部原样。
        let out = src.crop_frame_opaque(0, 0, 4, 4, 0.4).expect("裁得出来");
        assert_eq!(
            out.rgba, src.rgba,
            "拿不到外侧像素时必须原样返回，不许凭空反解"
        );
    }

    /// alpha 越界（0 或 1，或负数）时反解没有意义 —— 原样返回，绝不除零。
    #[test]
    fn crop_frame_opaque_ignores_degenerate_alpha() {
        let src = RgbaBitmap::new(3, 3, vec![9_u8; 3 * 3 * 4]).expect("位图");
        for a in [0.0, 1.0, -1.0, 2.0] {
            let out = src.crop_frame_opaque(0, 0, 3, 3, a).expect("裁得出来");
            assert_eq!(out.rgba, src.rgba, "alpha={a} 时应原样返回");
        }
    }

    /// ★★ 贴窗裁剪的"外圈收进"（2026-09-23 用户第三次报"依然会截取到边框线外"）。
    ///
    /// 造一张 12×10 的桌面 + 一个 5×4 的窗户（`x 3..7`、`y 3..6`，最外圈是混色边框）。
    /// 把**故意宽出来的那一刀**喂进去：左边多 1px、右边多 2px —— 正是用户真机产物里
    /// 量出来的形状（见 [`RgbaBitmap::frame_trim`] 文档里的逐像素表）。
    ///
    /// 判据三条一起看：
    ///   ① 左 1 右 2，上下不动；
    ///   ② **对照组**：正正好好的那一刀必须**一像素都不收**（否则就是"每次都乱削"的假绿）；
    ///   ③ 裁剪结果的尺寸回到窗户本身，且最外圈是边框本色。
    #[test]
    fn frame_trim_removes_the_ring_that_continues_outside() {
        let src = desktop_with_window();
        let want = FrameTrim {
            left: 1,
            right: 2,
            top: 0,
            bottom: 0,
        };
        assert_eq!(
            src.frame_trim(2, 3, 8, 4),
            want,
            "该收的边要收够、不该收的不许动"
        );

        // ② 对照组：完全贴合窗户的那一刀，四边都不收。
        assert_eq!(
            src.frame_trim(3, 3, 5, 4),
            FrameTrim::default(),
            "已经贴边的那一刀不许再收（收了就是'边框线被吃掉'）"
        );

        // ③ 端到端：收完之后尺寸 = 窗户尺寸，最外圈 = 边框**观测值**（反解已删，
        //    屏上什么样就什么样；反解不改像素，所以就是夹具画的那圈混色）。
        let out = src
            .crop_frame_opaque(2, 3, 8, 4, 0.4)
            .expect("贴框裁剪应成功");
        assert_eq!((out.w, out.h), (5, 4), "收进之后应当正好是窗户大小");
        for (x, y) in [(0usize, 0usize), (4, 0), (0, 3), (4, 3), (2, 0)] {
            assert_eq!(at3(&out, x, y), [53, 59, 65], "({x},{y}) 应是边框观测值");
        }
    }

    /// ★ 收进的**上限**与"判不出来就不动"：整张纯色图里随便切一刀，一像素都不许动。
    ///
    /// 少了这条，"纯色图被削掉两边"这种更难解释的 bug 会从这里进来。
    #[test]
    fn frame_trim_leaves_uniform_frames_alone() {
        let src = RgbaBitmap::new(20, 20, vec![77_u8; 20 * 20 * 4]).expect("位图");
        assert_eq!(src.frame_trim(5, 5, 10, 10), FrameTrim::default());
    }

    /// 贴着**帧边界**的那条边拿不到"外圈之外"的样本 → 不动（退化，不猜）。
    ///
    /// 对应真机：窗户顶到屏幕边（`x = 0`）时，左边根本没有"窗外像素"可比。
    #[test]
    fn frame_trim_gives_up_on_edges_at_the_frame_border() {
        let src = desktop_with_window();
        let t = src.frame_trim(0, 0, 12, 10);
        assert_eq!((t.left, t.top), (0, 0), "贴帧边界的边必须原样保留");
    }

    /// 造一张 12×10 的帧：底色桌面 `(10,20,30)`，`x 3..=7`、`y 3..=6` 是一个窗户 ——
    /// 最外圈是**被桌面冲淡后的**边框、内部是白客户区。
    ///
    /// ★ 边框色必须**物理自洽**：`obs = 0.6*bg + 0.4*118`（`alpha = 0.4`、本色 `118`），
    /// 算出来 `(53,59,65)`。第一版随手写了 `(47,200,47)`，反解出来是 `(103,255,73)` ——
    /// 用例自己都不知道自己在断言什么。**凡是喂给反解的样本，都要能反解回本色。**
    fn desktop_with_window() -> RgbaBitmap {
        let (fw, fh) = (12usize, 10usize);
        let bg = [10i32, 20, 30];
        let border: [u8; 3] = [
            ((bg[0] * 6 + 118 * 4) / 10) as u8,
            ((bg[1] * 6 + 118 * 4) / 10) as u8,
            ((bg[2] * 6 + 118 * 4) / 10) as u8,
        ];
        let mut rgba = vec![0_u8; fw * fh * 4];
        for y in 0..fh {
            for x in 0..fw {
                let i = (y * fw + x) * 4;
                let in_win = (3..8).contains(&x) && (3..7).contains(&y);
                let on_edge = in_win && (x == 3 || x == 7 || y == 3 || y == 6);
                let c: [u8; 3] = if !in_win {
                    [bg[0] as u8, bg[1] as u8, bg[2] as u8]
                } else if on_edge {
                    border
                } else {
                    [255, 255, 255]
                };
                rgba[i..i + 3].copy_from_slice(&c);
                rgba[i + 3] = 255;
            }
        }
        RgbaBitmap::new(fw as u32, fh as u32, rgba).expect("位图")
    }

    /// 取一位图上的 RGB。
    fn at3(b: &RgbaBitmap, x: usize, y: usize) -> [u8; 3] {
        let i = (y * (b.w as usize) + x) * 4;
        [b.rgba[i], b.rgba[i + 1], b.rgba[i + 2]]
    }

    /// ★ `crop_to_shot` 必须与 `crop` + 换序**逐像素一致**——放大镜取样的正确性全靠它。
    ///
    /// 判据取非平凡样本（偏移不为 0、宽高不等），且比对全部四个通道：`grid` 把 x 写进 R、
    /// y 写进 G，于是"忘了换序"和"换反了"两种错都会当场红（只比一个通道时它们可能撞对）。
    #[test]
    fn crop_to_shot_matches_crop_with_channels_swapped() {
        let src = grid(20, 12);
        let shot = src.crop_to_shot(3, 2, 5, 4).expect("合法裁剪");
        assert_eq!((shot.w, shot.h), (5, 4));

        let cropped = src.crop(3, 2, 5, 4).expect("同一块应当裁得出来");
        for row in 0..4usize {
            for col in 0..5usize {
                let i = (row * 5 + col) * 4;
                assert_eq!(
                    &shot.bgra[i..i + 4],
                    &[
                        cropped.rgba[i + 2],
                        cropped.rgba[i + 1],
                        cropped.rgba[i],
                        cropped.rgba[i + 3],
                    ],
                    "({col},{row}) 处 BGRA 与 RGBA 对不上"
                );
            }
        }
        // 首像素就是源图的 (3,2)：R 存 x、G 存 y ⇒ BGRA = [B=0, G=2, R=3, A=255]
        assert_eq!(&shot.bgra[0..4], &[0, 2, 3, 255]);
    }

    /// 越界与退化输入与 `crop` 同款处置（钳到图内 / 返回 `None`），不许 panic。
    #[test]
    fn crop_to_shot_clamps_like_crop() {
        let src = grid(8, 8);
        let s = src.crop_to_shot(6, 6, 10, 10).expect("越界应被钳到图内");
        assert_eq!((s.w, s.h), (2, 2));
        assert!(
            src.crop_to_shot(-5, -5, 3, 3).is_some(),
            "负偏移钳到原点仍可裁"
        );
        assert!(src.crop_to_shot(0, 0, 0, 4).is_none(), "宽 0 应拒绝");
        assert!(
            RgbaBitmap {
                w: 0,
                h: 0,
                rgba: Vec::new()
            }
            .crop_to_shot(0, 0, 4, 4)
            .is_none(),
            "空图应拒绝"
        );
    }

    /// 构造一张 w×h 的位图，像素值 = 行号 * 100 + 列号（放进 R 通道），便于定位来源。
    fn grid(w: u32, h: u32) -> RgbaBitmap {
        let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
        for y in 0..h {
            for x in 0..w {
                rgba.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        RgbaBitmap::new(w, h, rgba).expect("尺寸与像素长度自洽")
    }

    fn red_channel(b: &RgbaBitmap, x: u32, y: u32) -> u8 {
        let i = ((y * b.w + x) * 4) as usize;
        b.rgba[i]
    }

    #[test]
    fn new_rejects_length_mismatch() {
        assert!(RgbaBitmap::new(2, 2, vec![0; 16]).is_some());
        assert!(RgbaBitmap::new(2, 2, vec![0; 15]).is_none());
        assert!(RgbaBitmap::new(0, 2, vec![]).is_none());
    }

    /// 宽 > 高：裁掉**右边**，高度不变。
    #[test]
    fn wide_image_keeps_the_left_square() {
        let src = grid(5, 3);
        let c = src.crop_square_top_left();
        assert_eq!((c.w, c.h), (3, 3), "应取 min(5,3)=3 的正方形");
        // 左上角像素仍是原图左上角（裁剪从 (0,0) 起算）。
        assert_eq!(red_channel(&c, 0, 0), 0);
        assert_eq!(red_channel(&c, 2, 2), 2, "第三列应是原图第三列");
        assert_eq!(
            c.rgba.len(),
            (3 * 3 * 4) as usize,
            "裁剪后像素长度必须重新自洽"
        );
    }

    /// 高 > 宽：裁掉**下边**，宽度不变。
    #[test]
    fn tall_image_keeps_the_top_square() {
        let src = grid(3, 5);
        let c = src.crop_square_top_left();
        assert_eq!((c.w, c.h), (3, 3));
        assert_eq!(red_channel(&c, 0, 0), 0);
        assert_eq!(red_channel(&c, 2, 2), 2);
    }

    #[test]
    fn square_image_is_returned_unchanged() {
        let src = grid(4, 4);
        let c = src.crop_square_top_left();
        assert_eq!(c, src, "正方形原样返回，不应有任何位移");
    }

    #[test]
    fn green_channel_proves_rows_are_not_shifted() {
        // 宽图裁掉右侧后，**每一行的来源行号**不能变；用 G 通道（存行号）验证。
        let src = grid(6, 2);
        let c = src.crop_square_top_left();
        assert_eq!((c.w, c.h), (2, 2));
        let green = |b: &RgbaBitmap, x: u32, y: u32| {
            let i = ((y * b.w + x) * 4) as usize;
            b.rgba[i + 1]
        };
        assert_eq!(green(&c, 0, 1), 1, "第二行必须来自原图第二行");
        assert_eq!(green(&c, 1, 0), 0);
    }

    // ─── 贴图右键菜单的几何变换（旋转 / 水平镜像 / 垂直镜像）───

    /// 顺时针 90°：宽高互换，且每个像素映射到正确的新位置。
    ///
    /// 用 `grid`（R 存列号、G 存行号）给每个像素一个独有坐标，旋转后逐点验"来源"。
    /// 判据：目标 `(x', y')` = 源 `(x = y', y = W-1-x')`。
    #[test]
    fn rotate_cw90_swaps_dims_and_maps_pixels() {
        let src = grid(2, 3); // 2 宽 × 3 高
        let r = src.rotate_cw90();
        assert_eq!((r.w, r.h), (3, 2), "宽高互换：2×3 → 3×2");

        // 目标 (x', y') 的像素 = 源 (x=y', y=W-1-x')。
        // 检查几个"坐标唯一"的样本点（R=x_src, G=y_src）。
        let at = |b: &RgbaBitmap, x: u32, y: u32| {
            let i = ((y * b.w + x) * 4) as usize;
            (b.rgba[i], b.rgba[i + 1]) // (R, G) = (源列, 源行)
        };
        // 用正向且直观的判据：原图 (sx,sy) → 目标 (h-1-sy, sx)。
        // 源 (0,0) → 目标 (3-1-0, 0)=(2,0)
        let r2 = src.rotate_cw90();
        let d00 = at(&r2, 2, 0);
        assert_eq!(d00, (0, 0), "源(0,0)→目标(2,0)");
        // 源 (1,0) → 目标 (3-1-0, 1)=(2,1)
        assert_eq!(at(&r2, 2, 1), (1, 0), "源(1,0)→目标(2,1)");
        // 源 (1,2) → 目标 (3-1-2, 1)=(0,1)
        assert_eq!(at(&r2, 0, 1), (1, 2), "源(1,2)→目标(0,1)");
        // 源 (0,2) → 目标 (3-1-2, 0)=(0,0)
        assert_eq!(at(&r2, 0, 0), (0, 2), "源(0,2)→目标(0,0)");
    }

    /// 旋转 4 次回到原图（90°×4 = 360°，几何上必须复原；顺带验证逐像素无损）。
    #[test]
    fn rotate_cw90_four_times_is_identity() {
        let src = grid(7, 5); // 非正方形才能看出"宽高互换回来"
        let r = src.rotate_cw90().rotate_cw90().rotate_cw90().rotate_cw90();
        assert_eq!(r, src, "4×90° 必须复原（含像素与尺寸）");
    }

    /// 水平镜像：`(x, y)` ← `(W-1-x, y)`，尺寸不变。
    #[test]
    fn flip_h_mirrors_columns() {
        let src = grid(4, 2);
        let f = src.flip_h();
        assert_eq!((f.w, f.h), (4, 2), "水平镜像不改尺寸");
        let at = |b: &RgbaBitmap, x: u32, y: u32| {
            let i = ((y * b.w + x) * 4) as usize;
            (b.rgba[i], b.rgba[i + 1])
        };
        // 第一行：源第 0 列 → 第 3 列
        assert_eq!(at(&f, 0, 0), (3, 0), "列镜像");
        assert_eq!(at(&f, 3, 0), (0, 0));
        assert_eq!(at(&f, 1, 1), (2, 1), "第二行也一起镜像，行号不变");
        assert_eq!(at(&f, 2, 1), (1, 1));
    }

    /// 垂直镜像：`(x, y)` ← `(x, H-1-y)`，尺寸不变。
    #[test]
    fn flip_v_mirrors_rows() {
        let src = grid(3, 4);
        let f = src.flip_v();
        assert_eq!((f.w, f.h), (3, 4), "垂直镜像不改尺寸");
        let at = |b: &RgbaBitmap, x: u32, y: u32| {
            let i = ((y * b.w + x) * 4) as usize;
            (b.rgba[i], b.rgba[i + 1])
        };
        assert_eq!(at(&f, 0, 0), (0, 3), "行镜像");
        assert_eq!(at(&f, 2, 3), (2, 0));
        assert_eq!(at(&f, 1, 2), (1, 1), "中间行保持列号");
    }

    /// 镜像两次 = 原图；镜像两次的组合（水平+垂直）可还原旋转后的方向差异，但这里只验证恒等。
    #[test]
    fn flip_twice_is_identity() {
        let src = grid(6, 6);
        assert_eq!(src.flip_h().flip_h(), src, "水平镜像两次复原");
        assert_eq!(src.flip_v().flip_v(), src, "垂直镜像两次复原");
    }

    /// 空图参与变换不 panic，且原样返回（退化路径）。
    #[test]
    fn transforms_tolerate_empty_bitmap() {
        let empty = RgbaBitmap {
            w: 0,
            h: 0,
            rgba: Vec::new(),
        };
        assert_eq!(empty.clone().rotate_cw90(), empty);
        assert_eq!(empty.clone().flip_h(), empty);
        assert_eq!(empty.clone().flip_v(), empty);
    }

    // ─── PNG 解码（全色型）回归 ───

    use std::path::PathBuf;

    /// 唯一临时目录（测试间不互踩）。
    fn tmp_dir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("curseen_img_{tag}_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 用 png::Encoder 造一张任意色型的真 PNG。
    #[allow(clippy::too_many_arguments)]
    fn write_png(
        dir: &Path,
        name: &str,
        w: u32,
        h: u32,
        color: png::ColorType,
        depth: png::BitDepth,
        data: &[u8],
        palette: Option<Vec<u8>>,
    ) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(color);
        enc.set_depth(depth);
        if let Some(pal) = palette {
            enc.set_palette(pal);
        }
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(data).unwrap();
        path
    }

    /// RGBA：像素原样保留（用正方形图避免裁剪干扰断言）。
    #[test]
    fn png_rgba_decodes() {
        let dir = tmp_dir("rgba");
        // 2x2：红、蓝 / 绿、黄
        let data = [
            255u8, 0, 0, 255, 0, 0, 255, 255, //
            0, 255, 0, 255, 255, 255, 0, 255, //
        ];
        let p = write_png(
            &dir,
            "rgba.png",
            2,
            2,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &data,
            None,
        );
        let bmp = load_png(&p).expect("RGBA 应可解码");
        assert_eq!((bmp.w, bmp.h), (2, 2));
        assert_eq!(bmp.rgba, data.to_vec());
    }

    /// ★ 非方图**不再被裁成正方形**：4×2 的图加载出来仍是 4×2（宽高比由绘制端保持）。
    #[test]
    fn png_keeps_its_own_aspect_ratio() {
        let dir = tmp_dir("wide");
        // 4 宽 2 高：R 通道=列号，若被裁正方形，宽度会退化成 2。
        let mut data = Vec::new();
        for _y in 0..2 {
            for x in 0..4u8 {
                data.extend_from_slice(&[x, 0, 0, 255]);
            }
        }
        let p = write_png(
            &dir,
            "wide.png",
            4,
            2,
            png::ColorType::Rgba,
            png::BitDepth::Eight,
            &data,
            None,
        );
        let bmp = load_png(&p).expect("4×2 应可解码");
        assert_eq!((bmp.w, bmp.h), (4, 2), "不该再裁正方形");
        assert_eq!(bmp.rgba.len(), (4 * 2 * 4) as usize);
        // 第一列 0、最后一列 3 → 整幅宽度都在。
        assert_eq!(bmp.rgba[0], 0);
        assert_eq!(bmp.rgba[12], 3);
    }

    /// RGB：补 255 alpha。
    #[test]
    fn png_rgb_gets_opaque_alpha() {
        let dir = tmp_dir("rgb");
        let data = [10u8, 20, 30, 200, 100, 50, 1, 2, 3, 4, 5, 6];
        let p = write_png(
            &dir,
            "rgb.png",
            2,
            2,
            png::ColorType::Rgb,
            png::BitDepth::Eight,
            &data,
            None,
        );
        let bmp = load_png(&p).expect("RGB 应可解码");
        assert_eq!(
            bmp.rgba,
            vec![10, 20, 30, 255, 200, 100, 50, 255, 1, 2, 3, 255, 4, 5, 6, 255]
        );
    }

    /// 调色板（用户实机文件就是这种）：索引正确展开为颜色。
    #[test]
    fn png_indexed_palette_decodes() {
        let dir = tmp_dir("pal");
        // 2x2，索引 0/1/1/0；调色板两色：洋红、青
        let data = [0u8, 1, 1, 0];
        let palette = vec![255, 0, 255, 0, 255, 255];
        let p = write_png(
            &dir,
            "pal.png",
            2,
            2,
            png::ColorType::Indexed,
            png::BitDepth::Eight,
            &data,
            Some(palette),
        );
        let bmp = load_png(&p).expect("调色板 PNG 应可解码");
        assert_eq!((bmp.w, bmp.h), (2, 2));
        // 索引 0 = 洋红（不透明）
        assert_eq!(&bmp.rgba[..4], &[255, 0, 255, 255]);
        // 索引 1 = 青
        assert_eq!(&bmp.rgba[4..8], &[0, 255, 255, 255]);
    }

    /// 16-bit RGBA：STRIP_16 后高位字节保留（数值 >> 8）。
    #[test]
    fn png_sixteen_bit_strips_to_eight() {
        let dir = tmp_dir("s16");
        // 1x1，RGBA16：R=0x1122 G=0x3344 B=0x5566 A=0x7788 → 8bit 后 0x11/0x33/0x55/0x77
        let data = [0x11u8, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
        let p = write_png(
            &dir,
            "deep.png",
            1,
            1,
            png::ColorType::Rgba,
            png::BitDepth::Sixteen,
            &data,
            None,
        );
        let bmp = load_png(&p).expect("16-bit 应可解码");
        assert_eq!(bmp.rgba, vec![0x11, 0x33, 0x55, 0x77]);
    }

    /// 损坏文件 → None（不 panic）。
    #[test]
    fn corrupted_file_returns_none() {
        let dir = tmp_dir("bad");
        let p = dir.join("bad.png");
        std::fs::write(
            &p,
            [0x89u8, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0],
        )
        .unwrap();
        assert!(load_png(&p).is_none());
        // 纯垃圾也一样
        let p2 = dir.join("junk.png");
        std::fs::write(&p2, b"not a png at all").unwrap();
        assert!(load_png(&p2).is_none());
    }

    /// 构造一张 8×8 的抓屏（BGRA）：底色 (30,20,10)，中心 4×4 是 (50,100,200)。
    ///
    /// 中心块让"圆心采到中心像素"可判定（8×8 的中心像素 4.5,4.5 落在块内）；
    /// 底色让"圆外/远端"可判定。
    fn center_block_shot() -> Shot {
        let mut bgra = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let center = (2..6).contains(&x) && (2..6).contains(&y);
                let (r, g, b) = if center { (50, 100, 200) } else { (30, 20, 10) };
                bgra.extend_from_slice(&[b, g, r, 255]);
            }
        }
        Shot { w: 8, h: 8, bgra }
    }

    fn px(bmp: &RgbaBitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * bmp.w + x) * 4) as usize;
        [
            bmp.rgba[i],
            bmp.rgba[i + 1],
            bmp.rgba[i + 2],
            bmp.rgba[i + 3],
        ]
    }

    /// 圆形蒙版：圆外必须全透明；圆心必须采到源图中心块的颜色。
    #[test]
    fn lens_circle_masks_outside_and_samples_center() {
        let shot = center_block_shot();
        let bmp =
            super::lens_circle(&shot, 4, LensMode::Magnify { zoom: 1.0 }).expect("应产出位图");
        assert_eq!((bmp.w, bmp.h), (8, 8));
        assert_eq!(px(&bmp, 0, 0)[3], 0, "左上角在圆外必须透明");
        assert_eq!(px(&bmp, 7, 7)[3], 0, "右下角在圆外必须透明");
        // 圆心像素（4,4）→ 源 (3.5,3.5) 落在中心块内。
        let c = px(&bmp, 4, 4);
        assert_eq!(c[3], 255, "圆心必须不透明");
        assert_eq!(&c[..3], &[50, 100, 200], "圆心应采到中心块颜色：{c:?}");
    }

    /// difference：逐通道 |源 − 差色|；圆外仍然透明。
    #[test]
    fn lens_circle_difference_inverts_channels() {
        let mut bgra = Vec::new();
        for _ in 0..8 {
            for _ in 0..8 {
                bgra.extend_from_slice(&[10, 20, 30, 255]); // B=10 G=20 R=30
            }
        }
        let shot = Shot { w: 8, h: 8, bgra };
        let bmp = super::lens_circle(
            &shot,
            4,
            LensMode::Difference {
                color: (200, 100, 50),
            },
        )
        .expect("应产出位图");
        let c = px(&bmp, 4, 4);
        assert_eq!(&c[..3], &[170, 80, 40], "R/G/B 应各自 |源−差色|");
        assert_eq!(px(&bmp, 0, 0)[3], 0);
    }

    /// 放大：同一 dest 像素在不同 zoom 下应采到源图不同位置。
    #[test]
    fn lens_circle_zoom_changes_source_window() {
        // 8×8 的横向红渐变：red = x * 30，其余通道 0。
        let mut bgra = Vec::new();
        for _y in 0..8 {
            for x in 0..8 {
                bgra.extend_from_slice(&[0, 0, (x * 30) as u8, 255]);
            }
        }
        let shot = Shot { w: 8, h: 8, bgra };
        let one = super::lens_circle(&shot, 4, LensMode::Magnify { zoom: 1.0 }).unwrap();
        let two = super::lens_circle(&shot, 4, LensMode::Magnify { zoom: 2.0 }).unwrap();
        // dest 像素 (6,4)：离圆心 +2.5px。zoom=1 采源 x=6.5，zoom=2 只到 x=5.25。
        let r1 = px(&one, 6, 4)[0];
        let r2 = px(&two, 6, 4)[0];
        assert!(
            r1 > r2,
            "zoom 越大，同一 dest 像素应采到越靠近中心（更暗）：{r1} vs {r2}"
        );
    }

    /// 退化输入不 panic：空图 / 半径 0 / 非法 zoom。
    #[test]
    fn lens_circle_handles_degenerate_inputs() {
        let empty = Shot::default();
        assert!(super::lens_circle(&empty, 4, LensMode::Magnify { zoom: 1.0 }).is_none());
        let shot = center_block_shot();
        assert!(super::lens_circle(&shot, 0, LensMode::Magnify { zoom: 1.0 }).is_none());
        let bmp = super::lens_circle(&shot, 2, LensMode::Magnify { zoom: f32::NAN })
            .expect("NaN zoom 应退回 1.0 而不是崩");
        assert_eq!((bmp.w, bmp.h), (4, 4));
    }
}
