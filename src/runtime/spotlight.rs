//! 聚光灯增量帧（补丁 032）。
//!
//! 整屏半透明聚光灯的瓶颈是**光栅面积**：4K 下每帧整屏重画会爆帧。
//! 这里的思路是「上一帧的暗罩长什么样」只需色 + 圆心 + 半径就能描述，
//! 于是「洞挪了个位置」等价于「补回旧洞 + 挖掉新洞」，只有这两块需要重画。

use super::*;

impl Runtime {
    /// 本帧是否走增量呈现、重画哪一块（世界坐标矩形）。
    pub fn partial_frame_dirty(&self) -> Option<Rect> {
        self.partial_dirty
    }

    /// 作废聚光灯增量缓存：下一帧强制整窗重画。
    ///
    /// 两个调用点：
    /// - overlay 发现 windui 后备缓冲代数变了（休眠释放 / 尺寸重建）——上一帧内容
    ///   已不存在，增量恢复的根基没了；
    /// - overlay 往缓冲上画过 draw_list 之外的东西（GIF 选框边框）——缓冲不再等于
    ///   "暗罩 + 光圈"，按增量恢复会把边框残留在屏上。
    pub fn invalidate_spotlight(&mut self) {
        self.spot_prev = None;
    }

    /// 识别「整屏实色矩形 + 圆形 Clear 挖洞」的聚光灯形态，返回挖洞参数。
    pub(crate) fn spotlight_pattern(list: &DrawList, screen: ScreenRect) -> Option<SpotlightKey> {
        let cmds = list.cmds();
        if cmds.len() != 2 || !list.images().is_empty() {
            return None;
        }
        let (
            DrawCmd::Fill {
                shape: Shape::Rect { rect, radius },
                paint: Paint::Solid(color, BlendMode::SourceOver),
                alpha: a1,
                blend: Blend::SourceOver,
            },
            DrawCmd::Fill {
                shape: Shape::Circle { center, radius: r },
                paint: Paint::Solid(hole_color, BlendMode::Clear),
                alpha: a2,
                blend: Blend::Clear,
            },
        ) = (&cmds[0], &cmds[1])
        else {
            return None;
        };
        if *radius != 0.0 || *a1 != 1.0 || *a2 != 1.0 || *r <= 0.0 || hole_color.a() != 0 {
            return None;
        }
        // 暗罩必须盖满覆盖层窗口：只盖一部分就增量恢复，会把没盖到的区域留在
        // 上一帧的内容上（那里的"恒定背景"假设不成立）。
        if rect.x > 0.0 || rect.y > 0.0 || rect.w < screen.w as f32 || rect.h < screen.h as f32 {
            return None;
        }
        Some(SpotlightKey {
            color: *color,
            cx: center.x,
            cy: center.y,
            r: *r,
        })
    }

    /// 聚光灯增量帧检测（**补丁 032**）。
    ///
    /// 聚光灯每帧的昂贵部分是"整屏暗罩"——但暗罩是**恒定**的，帧间唯一变化的是
    /// 光圈的位置。于是只要上一呈现帧也是聚光灯形态且暗罩同色，这一帧就退化为：
    ///
    /// 1. 把「旧洞 ∪ 新洞」的外接矩形（外扩 2px 容抗锯齿边）清成透明；
    /// 2. 在这块矩形上重填暗罩色（清过底，SourceOver 落在透明上 = 整窗路径同款）；
    /// 3. 在新位置挖洞。
    ///
    /// 成本从 O(整屏像素) 降到 O(洞面积)：4K 下 ~19ms/帧 → 亚毫秒。缓冲里脏区
    /// 之外的字节与上一呈现帧逐字节相同（它们就是上一帧的 BGRA），呈现也只交脏区
    /// （windui 补丁 032 的局部呈现）。
    ///
    /// 作废条件（自动落到整窗路径）：形态不符（任何其它模块出了指令）、暗罩变色、
    /// 缓冲被重建（代数对账，见 [`Self::invalidate_spotlight`]）。
    pub(crate) fn detect_spotlight_partial(&mut self) {
        self.partial_dirty = None;
        let pattern = Self::spotlight_pattern(&self.draw_list, self.screen);
        match (pattern, self.spot_prev) {
            (Some(new), Some(prev)) if new.color == prev.color => {
                // 脏区 = 旧洞 ∪ 新洞 的外接矩形，外扩 2px 盖住抗锯齿边缘，
                // 再裁进覆盖层窗口（光标可能拖到屏幕外）。
                const PAD: f32 = 2.0;
                let x0 = (prev.cx - prev.r).min(new.cx - new.r) - PAD;
                let y0 = (prev.cy - prev.r).min(new.cy - new.r) - PAD;
                let x1 = (prev.cx + prev.r).max(new.cx + new.r) + PAD;
                let y1 = (prev.cy + prev.r).max(new.cy + new.r) + PAD;
                let x0 = x0.max(0.0);
                let y0 = y0.max(0.0);
                let x1 = x1.min(self.screen.w as f32);
                let y1 = y1.min(self.screen.h as f32);
                if x1 <= x0 || y1 <= y0 {
                    // 完全出界（理论上到不了）：按整窗走，宁慢勿错。
                    self.spot_prev = Some(new);
                    return;
                }
                let rect = Rect {
                    x: x0,
                    y: y0,
                    w: x1 - x0,
                    h: y1 - y0,
                };
                let mut dl = DrawList::new();
                // ① 清脏区（Clear：抹成透明，不是"用透明色盖"）。
                dl.push(DrawCmd::Clear(rect));
                // ② 重填暗罩。清过底后 SourceOver 落在透明上，结果与整窗路径
                //    逐像素一致（整窗路径同样是"清屏 → 填暗罩"）。
                dl.push(DrawCmd::Fill {
                    shape: Shape::Rect { rect, radius: 0.0 },
                    paint: Paint::Solid(new.color, BlendMode::SourceOver),
                    alpha: 1.0,
                    blend: Blend::SourceOver,
                });
                // ③ 新洞。
                dl.push(DrawCmd::Fill {
                    shape: Shape::Circle {
                        center: Point::new(new.cx, new.cy),
                        radius: new.r,
                    },
                    paint: Paint::Solid(Argb::TRANSPARENT, BlendMode::Clear),
                    alpha: 1.0,
                    blend: Blend::Clear,
                });
                self.draw_list = dl;
                self.partial_dirty = Some(rect);
                self.spot_prev = Some(new);
            }
            // 首个聚光灯帧（或作废后重来）：整窗重画，同时种下缓存。
            (Some(new), _) => self.spot_prev = Some(new),
            // 不是聚光灯形态：缓存作废。
            (None, _) => self.spot_prev = None,
        }
    }
}

// ─── 聚光灯增量帧（补丁 032）───
#[cfg(test)]
mod spotlight_partial_tests {
    use super::*;
    use crate::core::draw::{Blend, BlendMode, DrawCmd, Paint, Shape};
    use windui::render::RenderTarget;

    /// 一份标准聚光灯指令表（整屏暗罩 + 圆形挖洞），即 spotlight.lua 每帧的产物。
    fn spotlight_list(dim: Argb, cx: f32, cy: f32, r: f32, sw: f32, sh: f32) -> DrawList {
        let mut dl = DrawList::new();
        dl.push(DrawCmd::Fill {
            shape: Shape::Rect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: sw,
                    h: sh,
                },
                radius: 0.0,
            },
            paint: Paint::Solid(dim, BlendMode::SourceOver),
            alpha: 1.0,
            blend: Blend::SourceOver,
        });
        dl.push(DrawCmd::Fill {
            shape: Shape::Circle {
                center: Point::new(cx, cy),
                radius: r,
            },
            paint: Paint::Solid(Argb::TRANSPARENT, BlendMode::Clear),
            alpha: 1.0,
            blend: Blend::Clear,
        });
        dl
    }

    /// 把 DrawList 光栅化到给定缓冲（**不清底**——增量帧的前提就是保留上一帧内容）。
    fn rasterize_onto(buf: &mut [u8], w: u32, h: u32, list: &DrawList) {
        let mut eng = windui::text::NullTextEngine;
        let mut target = windui::render::DibTarget::new(buf, w, h);
        let mut canvas = target.make_canvas(&mut eng, 1.0);
        crate::runtime::render::translate_offset(list, &mut *canvas, 1.0, 0.0, 0.0);
    }

    fn rt_240x180() -> Runtime {
        let mut rt = Runtime::new(Config::default());
        rt.screen = ScreenRect {
            x: 0,
            y: 0,
            w: 240,
            h: 180,
        };
        rt
    }

    /// ★ 核心等价性：增量帧（保留上一帧缓冲，只画脏区）必须与整窗重画逐字节一致。
    #[test]
    fn incremental_frame_matches_full_redraw_pixel_wise() {
        let (w, h) = (240u32, 180u32);
        let dim = Argb(0xAA000000);
        let r = 30.0;

        // 整窗路径：透明底 → 直接画"新洞"帧。
        let mut full_buf = vec![0_u8; (w * h * 4) as usize];
        rasterize_onto(
            &mut full_buf,
            w,
            h,
            &spotlight_list(dim, 150.0, 90.0, r, w as f32, h as f32),
        );

        // 增量路径：缓冲里是"旧洞 (60,90)"帧 → 生产同一套脏区计算 → 只画脏区。
        let mut inc_buf = vec![0_u8; (w * h * 4) as usize];
        rasterize_onto(
            &mut inc_buf,
            w,
            h,
            &spotlight_list(dim, 60.0, 90.0, r, w as f32, h as f32),
        );
        let mut rt = rt_240x180();
        rt.draw_list = spotlight_list(dim, 150.0, 90.0, r, w as f32, h as f32);
        rt.spot_prev = Some(SpotlightKey {
            color: dim,
            cx: 60.0,
            cy: 90.0,
            r,
        });
        rt.detect_spotlight_partial();
        assert!(rt.partial_dirty.is_some(), "同色聚光灯必须命中增量路径");
        assert_eq!(
            rt.draw_list.cmds().len(),
            3,
            "增量指令表 = 清脏区 + 重填暗罩 + 挖新洞"
        );
        rasterize_onto(&mut inc_buf, w, h, &rt.draw_list);

        assert_eq!(
            full_buf, inc_buf,
            "增量帧与整窗重画必须逐字节一致（含抗锯齿边）"
        );
    }

    /// 脏区必须同时盖住旧洞与新洞（外扩容差后），否则洞会留残影。
    #[test]
    fn dirty_rect_covers_both_holes() {
        let mut rt = rt_240x180();
        let dim = Argb(0xAA000000);
        rt.draw_list = spotlight_list(dim, 200.0, 40.0, 25.0, 240.0, 180.0);
        rt.spot_prev = Some(SpotlightKey {
            color: dim,
            cx: 40.0,
            cy: 140.0,
            r: 25.0,
        });
        rt.detect_spotlight_partial();
        let d = rt.partial_dirty.unwrap();
        // 旧洞外接盒
        assert!(d.x <= 40.0 - 25.0 && d.y <= 140.0 - 25.0);
        assert!(d.x + d.w >= 40.0 + 25.0 && d.y + d.h >= 140.0 + 25.0);
        // 新洞外接盒
        assert!(d.x <= 200.0 - 25.0 && d.y <= 40.0 - 25.0);
        assert!(d.x + d.w >= 200.0 + 25.0 && d.y + d.h >= 40.0 + 25.0);
    }

    /// 任何其它内容混进指令表 → 拒绝增量，整窗重画，缓存作废。
    #[test]
    fn rejects_when_other_content_present() {
        let mut rt = rt_240x180();
        let dim = Argb(0xAA000000);
        let mut list = spotlight_list(dim, 100.0, 90.0, 30.0, 240.0, 180.0);
        // 第三条指令（比如点击效果）。
        list.push(DrawCmd::Fill {
            shape: Shape::Circle {
                center: Point::new(30.0, 30.0),
                radius: 10.0,
            },
            paint: Paint::Solid(Argb(0x80FF0000), BlendMode::SourceOver),
            alpha: 1.0,
            blend: Blend::SourceOver,
        });
        rt.draw_list = list;
        rt.spot_prev = Some(SpotlightKey {
            color: dim,
            cx: 100.0,
            cy: 90.0,
            r: 30.0,
        });
        rt.detect_spotlight_partial();
        assert!(rt.partial_dirty.is_none());
        assert!(rt.spot_prev.is_none(), "形态不符必须作废缓存");
    }

    /// 首个聚光灯帧：整窗重画，但种下缓存——下一帧起才有增量。
    #[test]
    fn first_frame_is_full_but_seeds_cache() {
        let mut rt = rt_240x180();
        let dim = Argb(0xAA000000);
        rt.draw_list = spotlight_list(dim, 100.0, 90.0, 30.0, 240.0, 180.0);
        rt.detect_spotlight_partial();
        assert!(rt.partial_dirty.is_none());
        assert_eq!(
            rt.spot_prev,
            Some(SpotlightKey {
                color: dim,
                cx: 100.0,
                cy: 90.0,
                r: 30.0,
            })
        );
    }

    /// 暗罩变色（用户改了浓度）：立即回整窗，缓存跟进新色。
    #[test]
    fn dim_color_change_falls_back_to_full() {
        let mut rt = rt_240x180();
        rt.draw_list = spotlight_list(Argb(0xCC000000), 100.0, 90.0, 30.0, 240.0, 180.0);
        rt.spot_prev = Some(SpotlightKey {
            color: Argb(0xAA000000),
            cx: 100.0,
            cy: 90.0,
            r: 30.0,
        });
        rt.detect_spotlight_partial();
        assert!(rt.partial_dirty.is_none(), "变色帧必须整窗重画");
        assert_eq!(
            rt.spot_prev.map(|k| k.color),
            Some(Argb(0xCC000000)),
            "缓存更新为新色，下一帧起恢复增量"
        );
    }
}
