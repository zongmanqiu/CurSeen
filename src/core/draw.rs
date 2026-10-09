//! 绘制指令表。
//!
//! **纯数据、无 windui 依赖**：算法层（modules/*）产出它，渲染层（ui/paint.rs）消费它。
//! 这是"算法"与"渲染"之间唯一的契约。

use std::rc::Rc;

use super::color::Argb;
use super::geometry::{Point, Rect, ScreenRect};
use super::image::RgbaBitmap;

/// 绘制层：同窗绘制顺序的 Z 序桶（「统一层级模型」的维度 B·同窗内）。
///
/// 渲染按 `Base → Normal → Top` 依次光栅化到**同一块缓冲**：只重排桶的顺序，不引入
/// 任何中间合成层 / 额外缓冲，性能与现状完全一致。
///
/// 为什么必须垫底（`Base`）：`Blend::Clear` 是**破坏性擦除**，会把缓冲里已画的一切（含
/// 波纹/拖尾）清成透明。聚光灯这类「整屏暗罩 + 圆洞」的特效只有**先画**，它的挖洞才只
/// 清掉自己的暗罩、让叠在上面的特效保持可见。脚本经 `ctx:layer("base"|"normal"|"top")`
/// 自声明层，**未声明默认 `Normal`**——存量 40+ 个 `.lua` 无需迁移。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Layer {
    /// 垫底层：最先画（暗罩 / 挖洞特效）。
    Base,
    /// 普通层：大多数特效与原生模块（默认）。
    #[default]
    Normal,
    /// 顶层层：最后画（滤镜 / 叠加）。
    Top,
}

/// 一帧的绘制指令表。跨帧复用：clear() 保留容量。
#[derive(Default)]
pub struct DrawList {
    /// 底部层指令桶（`Layer::Base`）。
    pre: Vec<DrawCmd>,
    /// 普通层指令桶（`Layer::Normal`，默认）。
    cmds: Vec<DrawCmd>,
    /// 顶部层指令桶（`Layer::Top`）。
    post: Vec<DrawCmd>,
    /// 当前写入层：`push` 按其把指令定向到上面的桶。
    cur: Layer,
    points: Vec<Point>,
    /// 本帧引用的位图。`DrawCmd::Image` 只存下标，像素由这里按 `Rc` 共享
    /// （克隆一份指针，不拷像素）。
    images: Vec<Rc<RgbaBitmap>>,
}

impl DrawList {
    /// 把一张位图登记进本帧，并推送一条 `DrawCmd::Image`。
    ///
    /// `dest` 是世界坐标下的目标矩形（鼠标跟随层用「外接正方形」），`alpha` 是整体不透明度
    /// 乘子（悬浮标 PNG 用 1.0，透明度由图自身携带）。位图按 `Rc` 共享，不拷贝像素。
    ///
    /// 返回它在 `images` 里的下标——`DrawCmd::Image` 只记这个下标，渲染层再按它取回像素。
    pub fn image(&mut self, bitmap: Rc<RgbaBitmap>, dest: Rect, alpha: f32) -> usize {
        let idx = self.images.len();
        self.images.push(bitmap);
        self.push(DrawCmd::Image { idx, dest, alpha });
        idx
    }

    /// 本帧引用的位图切片（`DrawCmd::Image` 按下标在此取像素）。
    pub fn images(&self) -> &[Rc<RgbaBitmap>] {
        &self.images
    }
}

impl DrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cmds(&self) -> &[DrawCmd] {
        &self.cmds
    }

    pub fn push(&mut self, cmd: DrawCmd) {
        match self.cur {
            Layer::Base => self.pre.push(cmd),
            Layer::Normal => self.cmds.push(cmd),
            Layer::Top => self.post.push(cmd),
        }
    }

    /// 当前写入层。
    pub fn layer(&self) -> Layer {
        self.cur
    }

    /// 切换后续 `push` 的写入层。脚本经 `ctx:layer` 调用；原生层默认停在 `Normal`。
    pub fn set_layer(&mut self, l: Layer) {
        self.cur = l;
    }

    /// 全部三层指令总数。作为「本帧是否真的画了东西」的判据，**必须跨层统计**——
    /// 只查 `cmds()` 会把垫底/顶层脚本漏判成"没画"（见 `LuaEffect::draw` 的 active 标记）。
    pub fn len_all(&self) -> usize {
        self.pre.len() + self.cmds.len() + self.post.len()
    }

    /// 渲染顺序的指令迭代器：`Base → Normal → Top`，层内保持插入序（`push` 即调用序）。
    ///
    /// 渲染层用它在**同一块缓冲**里按序光栅化；外部只读遍历，零分配（三个切片的 `Chain`）。
    pub fn ordered(&self) -> impl Iterator<Item = &DrawCmd> {
        self.pre
            .iter()
            .chain(self.cmds.iter())
            .chain(self.post.iter())
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Argb) {
        self.push(DrawCmd::Fill {
            shape: Shape::Rect { rect, radius: 0.0 },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            blend: Blend::SourceOver,
        });
    }

    pub fn fill_round_rect(&mut self, rect: Rect, radius: f32, color: Argb) {
        self.fill_round_rect_alpha(rect, radius, color, 1.0);
    }

    /// 圆角矩形的**带乘子**填充。
    ///
    /// `alpha` 是透明度**乘子**（与颜色自带的 alpha 相乘，见
    /// `runtime::render::to_color_alpha`）。单独开一个构造器，是因为 [`fill_round_rect`]
    /// 的内部 alpha 恒为 1.0：拿它画"会淡出的底色"（按键弹窗背景）时，不透明度**只作用到
    /// 文字与边框**、底色始终实心，表现就是"不透明度调不动背景颜色"。
    ///
    /// [`fill_round_rect`]: Self::fill_round_rect
    pub fn fill_round_rect_alpha(&mut self, rect: Rect, radius: f32, color: Argb, alpha: f32) {
        self.push(DrawCmd::Fill {
            shape: Shape::Rect { rect, radius },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha,
            blend: Blend::SourceOver,
        });
    }

    pub fn fill_circle(&mut self, center: Point, radius: f32, color: Argb) {
        self.push(DrawCmd::Fill {
            shape: Shape::Circle { center, radius },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            blend: Blend::SourceOver,
        });
    }

    pub fn stroke_circle(&mut self, center: Point, radius: f32, width: f32, color: Argb) {
        self.push(DrawCmd::Stroke {
            shape: Shape::Circle { center, radius },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn draw_line(&mut self, a: Point, b: Point, width: f32, color: Argb) {
        self.push(DrawCmd::Stroke {
            shape: Shape::Line { a, b },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn draw_polyline(&mut self, pts: Vec<Point>, width: f32, color: Argb) {
        let start = self.points.len() as u32;
        let len = pts.len() as u32;
        self.points.extend(pts);
        self.push(DrawCmd::Stroke {
            shape: Shape::Polyline {
                points: PointRange { start, len },
                closed: false,
            },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn push_alpha(&mut self, alpha: f32) {
        self.push(DrawCmd::PushAlpha(alpha));
    }

    pub fn pop_alpha(&mut self) {
        self.push(DrawCmd::PopAlpha);
    }

    pub fn points_len(&self) -> usize {
        self.points.len()
    }

    /// 点池全量切片。
    ///
    /// 渲染层（`ui/paint.rs`）拿它按 [`PointRange`] 取折线顶点——`Shape::Polyline`
    /// 只存了一个 `PointRange`，没有这个访问器，翻译器连点都读不到，只能空转。
    pub fn points(&self) -> &[Point] {
        &self.points
    }

    /// 按 `PointRange` 取顶点切片。
    ///
    /// 越界/空区间返回空切片而不是 panic：算法层在极端参数下可能留下 stale range，
    /// 一帧画歪可以接受，进程崩掉不行。
    pub fn range(&self, r: PointRange) -> &[Point] {
        let s = r.start as usize;
        if s >= self.points.len() {
            return &[];
        }
        let e = s.saturating_add(r.len as usize).min(self.points.len());
        &self.points[s..e]
    }

    pub fn push_point(&mut self, p: Point) {
        self.points.push(p);
    }

    pub fn clear_rect(&mut self, rect: Rect) {
        self.push(DrawCmd::Clear(rect));
    }

    /// 清空整个指令表与复用点池（保留容量，避免每帧重分配）。
    ///
    /// 每帧渲染前调用：先 `clear()`，再让各 module 把本帧指令 push 进来。
    pub fn clear(&mut self) {
        self.pre.clear();
        self.cmds.clear();
        self.post.clear();
        self.cur = Layer::Normal;
        self.points.clear();
        // 位图随帧丢弃：下一帧重新 push（悬浮标 PNG 每张只产一次，且不跨帧复用）。
        self.images.clear();
    }

    pub fn stroke_rect(&mut self, rect: Rect, width: f32, color: Argb) {
        self.push(DrawCmd::Stroke {
            shape: Shape::Rect { rect, radius: 0.0 },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    /// 圆角矩形描边（**空心**，笔宽居中压在图元边界上）。
    ///
    /// 与 [`Self::fill_round_rect`] 成对使用即可画出"有边框的圆角块"：
    /// 描边 + 填色两条指令，而不是"先填一颗大矩形当边框、再压一颗内衬"——
    /// 后者在半透明时会露馅，内衬那层会把边框色混进键帽主体。
    pub fn stroke_round_rect(&mut self, rect: Rect, radius: f32, width: f32, color: Argb) {
        self.push(DrawCmd::Stroke {
            shape: Shape::Rect { rect, radius },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn stroke_ellipse(&mut self, rect: Rect, width: f32, color: Argb) {
        let center = rect.center();
        self.push(DrawCmd::Stroke {
            shape: Shape::Ellipse {
                center,
                radius_x: rect.w / 2.0,
                radius_y: rect.h / 2.0,
            },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn fill_polygon(&mut self, points: &[Point], color: Argb) {
        let start = self.points.len() as u32;
        let len = points.len() as u32;
        self.points.extend_from_slice(points);
        self.push(DrawCmd::Fill {
            shape: Shape::Polyline {
                points: PointRange { start, len },
                closed: true,
            },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            blend: Blend::SourceOver,
        });
    }

    pub fn draw_polyline_ref(&mut self, pts: &[Point], width: f32, color: Argb) {
        let start = self.points.len() as u32;
        let len = pts.len() as u32;
        self.points.extend_from_slice(pts);
        self.push(DrawCmd::Stroke {
            shape: Shape::Polyline {
                points: PointRange { start, len },
                closed: false,
            },
            paint: Paint::Solid(color, BlendMode::SourceOver),
            alpha: 1.0,
            width,
            cap: Cap::Round,
            join: Join::Round,
            blend: Blend::SourceOver,
        });
    }

    pub fn text(&mut self, x: f32, y: f32, text: &str, size: f32, color: Argb, alpha: Option<f32>) {
        self.push(DrawCmd::Text {
            x,
            y,
            text: text.to_string(),
            size,
            color,
            alpha,
        });
    }

    /// 以 `center` 为中心的文字（序号徽标用）。见 [`DrawCmd::TextCentered`]。
    pub fn text_centered(
        &mut self,
        center: Point,
        text: &str,
        size: f32,
        color: Argb,
        alpha: Option<f32>,
    ) {
        self.push(DrawCmd::TextCentered {
            center,
            text: text.to_string(),
            size,
            color,
            alpha,
        });
    }

    /// ★ 屏幕中央提示横幅（共享函数，2026-09-22）：半透明深底条 + 白字，
    ///   **底框与文字由同一 center 推导**——文字用 [`Self::text_centered`]（垂直居中
    ///   由渲染端保证），不再手算 baseline 魔法数。返回底框 Rect（脏区用）。
    ///
    /// 使用方：屏幕标注 / 截图 / 录屏的提示浮字（`modules::drawing::notify`）、
    /// GIF/MP4 录制倒计时、计时器（`runtime::draw_timer`）——同一函数，观感与对齐
    /// 自然一致。
    // ★ clippy::too_many_arguments：这九个参数是一次构图的全集（屏幕/文案/字号/内边距×2/
    //   配色×2/圆角），调用端一次性给齐最直观；打包成 struct 只会平添一层造值噪声。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_centered_banner(
        &mut self,
        screen: ScreenRect,
        text: &str,
        font_pt: f32,
        pad_x: f32,
        pad_y: f32,
        bg: Argb,
        fg: Argb,
        radius: f32,
    ) -> Rect {
        let center = Point::new(
            screen.x as f32 + screen.w as f32 / 2.0,
            screen.y as f32 + screen.h as f32 / 2.0,
        );
        self.draw_banner_at(center, text, font_pt, pad_x, pad_y, bg, fg, radius)
    }

    /// ★ 任意中心点的横幅（2026-09-25 自 [`Self::draw_centered_banner`] 泛化出）：
    ///   几何推导完全同款（底框 = 文字量得 + 内边距，文字以 `center` 居中），只是中心
    ///   不再钉死在屏幕中央——计时器按"锚点 + 偏移"算出自己的中心后走这里。
    ///   返回底框 Rect（脏区用）。
    // 参数与 [`Self::draw_centered_banner`] 一一对应（9 个）：横幅的几何参数本来就
    // 是一组，拆结构体反而让两处调用点不对称，豁免。
    #[allow(clippy::too_many_arguments)]
    pub fn draw_banner_at(
        &mut self,
        center: Point,
        text: &str,
        font_pt: f32,
        pad_x: f32,
        pad_y: f32,
        bg: Argb,
        fg: Argb,
        radius: f32,
    ) -> Rect {
        use crate::core::text_metrics;
        let text_w = text_metrics::estimate_width(text, font_pt);
        let text_h = text_metrics::line_height(font_pt);
        let w = text_w + pad_x * 2.0;
        let h = text_h + pad_y * 2.0;
        let box_ = Rect {
            x: center.x - w / 2.0,
            y: center.y - h / 2.0,
            w,
            h,
        };
        self.fill_round_rect_alpha(box_, radius, bg, 1.0);
        self.text_centered(center, text, font_pt, fg, None);
        box_
    }

    /// 把一段文字放进 `rect`：居中 + 放不下就缩字号。见 [`DrawCmd::TextInBox`]。
    pub fn text_in_box(
        &mut self,
        rect: Rect,
        text: &str,
        size: f32,
        color: Argb,
        alpha: Option<f32>,
    ) {
        self.push(DrawCmd::TextInBox {
            rect,
            text: text.to_string(),
            size,
            color,
            alpha,
        });
    }
}

/// 折线在点池里的引用。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointRange {
    pub start: u32,
    pub len: u32,
}

/// 形状。所有尺寸都是逻辑像素。
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Rect {
        rect: Rect,
        radius: f32,
    },
    Circle {
        center: Point,
        radius: f32,
    },
    Ellipse {
        center: Point,
        radius_x: f32,
        radius_y: f32,
    },
    Line {
        a: Point,
        b: Point,
    },
    Polyline {
        points: PointRange,
        closed: bool,
    },
}

/// 画笔。
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Argb, BlendMode),
    Linear {
        from: Point,
        to: Point,
        stops: Vec<GradStop>,
    },
    Radial {
        center: Point,
        radius: f32,
        stops: Vec<GradStop>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GradStop {
    pub offset: f32,
    pub color: Argb,
}

/// 线帽。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

/// 连接。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Miter,
    Round,
    Bevel,
}

/// 混合模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    SourceOver,
    Plus,
    Clear,
}

/// 混合模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    SourceOver,
    Plus,
    /// 擦除：把目标像素清成完全透明（tiny-skia `Clear` 混合，忽略源色）。
    /// 聚光灯用它在大块暗罩上"挖"出圆形光圈——只光栅化小圆，不碰整屏。
    Clear,
}

/// 绘制指令。
#[derive(Clone, Debug, PartialEq)]
pub enum DrawCmd {
    Fill {
        shape: Shape,
        paint: Paint,
        alpha: f32,
        blend: Blend,
    },
    Stroke {
        shape: Shape,
        paint: Paint,
        alpha: f32,
        width: f32,
        cap: Cap,
        join: Join,
        blend: Blend,
    },
    PushAlpha(f32),
    PopAlpha,
    Clear(Rect),
    /// 位图绘制：像素存在 `DrawList::images[idx]`，`dest` 是**世界坐标**目标矩形。
    /// 渲染层按 `View` 把矩形变换到画布本地坐标，再把位图 blit 到该矩形（见
    /// `runtime::render`）。用途：悬浮标的「PNG 图片」样式。
    Image {
        idx: usize,
        dest: Rect,
        alpha: f32,
    },
    /// 文字。`(x, y)` 是**排版盒的左上角**（不是基线！见 `TextCentered` 的说明）。
    ///
    /// 模块层拿不到字体，只能估算盒高；渲染层按**实测**尺寸把文字画在这个盒里
    /// （`runtime::render` 里 `draw_text` 在矩形内垂直居中 → 矩形高恰为自然行高时
    /// 退化为顶对齐，基线落在盒顶 + ascent）。
    Text {
        x: f32,
        y: f32,
        text: String,
        size: f32,
        color: Argb,
        alpha: Option<f32>,
    },
    /// **以 `center` 为几何中心**的文字（序号徽标用）。
    ///
    /// 存在的理由：`Text` 只能给一个"盒左上角"，而模块层算不出盒高——它没有字体，
    /// 只有 `text_metrics` 的估算。序号贴纸要让**数字墨迹**落在圆心，估算宽度/行高
    /// 带来的误差直接可见（实测数字偏移圆心 13px，用户报告"数字没有在圆的中心"）。
    ///
    /// 交给渲染层做，因为它手上有真东西：`measure_text` 给真实排版宽度、
    /// `text_line_metrics` 给真实 ascent/descent。渲染层据此：
    ///   · 水平——按真实排版宽度居中；
    ///   · 垂直——按"等宽数字的墨迹高 = 大写高 ≈ 0.7 em"把基线放在 `center.y + 0.35 em`，
    ///     再用实测 ascent 反推盒顶。这样数字的**墨迹**中心落在 `center`。
    TextCentered {
        center: Point,
        text: String,
        size: f32,
        color: Argb,
        alpha: Option<f32>,
    },
    /// **把一段文字放进一个矩形**：水平垂直都居中，**放不下就缩字号**。
    ///
    /// 与 [`TextCentered`] 的区别只有一条：它多了一个"宽度上限 = 矩形宽"，由渲染层按
    /// **实测**排版宽度决定要不要缩（缩到 55% 就不再缩 —— 再小就没法读了）。
    ///
    /// 为什么必须落在渲染层：模块层没有字体，只能用 `estimate_width` 估宽、用 1.25 em
    /// 凑盒高。虚拟键盘的键帽图例曾这么干，后果是
    ///   · 估宽偏大 → 文字整体左偏（用户报 "Ctrl 靠左"）；
    ///   · 凑的盒高 ≠ 真实行高 → 垂直方向也不居中。
    /// 而"这段字在键帽里放不放得下"同样只有拿到真实度量才答得准（`Home` / `PgUp` /
    /// `Num` / `Enter` 这类 4~5 字母的图例最容易溢出）。
    TextInBox {
        rect: Rect,
        text: String,
        size: f32,
        color: Argb,
        alpha: Option<f32>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drawlist_fill_rect() {
        let mut dl = DrawList::new();
        dl.fill_rect(Rect::new(0.0, 0.0, 100.0, 50.0), Argb(0xFFFF0000));
        assert_eq!(dl.cmds().len(), 1);
    }

    #[test]
    fn drawlist_clear_reuses() {
        let mut dl = DrawList::new();
        dl.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Argb(0xFF000000));
        dl.clear();
        dl.fill_rect(Rect::new(0.0, 0.0, 20.0, 20.0), Argb(0xFF00FF00));
        assert_eq!(dl.cmds().len(), 1);
    }

    /// 取 `ordered()` 里每条第 Fill 指令的实色（测试辅助）。
    fn fill_colors(dl: &DrawList) -> Vec<Argb> {
        dl.ordered()
            .filter_map(|c| match c {
                DrawCmd::Fill {
                    paint: Paint::Solid(c, _),
                    ..
                } => Some(*c),
                _ => None,
            })
            .collect()
    }

    /// ★ 层路由 + 排序：`ctx:layer` 把指令归入对应桶，`ordered()` 按 Base→Normal→Top
    /// 输出、层内保持插入序。默认未切层的指令进 Normal；`clear()` 后层状态复位为 Normal。
    #[test]
    fn layer_routes_and_orders_commands() {
        let red = Argb(0xFFFF0000);
        let blue = Argb(0xFF0000FF);
        let green = Argb(0xFF00FF00);
        let black = Argb(0xFF000000);

        let mut dl = DrawList::new();
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), red); // normalize
        dl.set_layer(Layer::Base);
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), blue); // base
        dl.set_layer(Layer::Top);
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), green); // top
        dl.set_layer(Layer::Normal);
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), black); // normal 再次

        // Base → Normal(插入序) → Top
        assert_eq!(fill_colors(&dl), vec![blue, red, black, green]);
        // 计数：cmds() 只回普通层，len_all() 跨层
        assert_eq!(dl.cmds().len(), 2);
        assert_eq!(dl.len_all(), 4);

        // clear() 复位全部层
        dl.clear();
        assert_eq!(dl.layer(), Layer::Normal);
        assert_eq!(dl.len_all(), 0);
    }

    /// `ctx:layer("top")` 让普通层指令垫在底下：Top 桶指令恒排在 Normal 之后。
    #[test]
    fn top_layer_sits_above_normal() {
        let a = Argb(0xFF111111);
        let b = Argb(0xFF222222);
        let mut dl = DrawList::new();
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), b); // normal
        dl.set_layer(Layer::Top);
        dl.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), a); // top
                                                        // top 的 a 画在 normal 的 b 之上
        assert_eq!(fill_colors(&dl).last(), Some(&a));
        assert_eq!(fill_colors(&dl).first(), Some(&b));
    }
}
