//! 暴露给 Lua 的绘图 API。Lua 脚本在 `draw(ctx)` 里通过 `ctx:circle(...)` 等调用绘图。
//!
//! ★ 生命周期修复：原先想用 `DrawListCtx<'a>` 包 `&mut DrawList` 注册成 userdata，但
//! mlua 的 `UserData` 要求 `Any + 'static`，带生命周期的引用类型没法实现该 trait，
//! 所以 `lua_effect::draw` 一直是个空壳（注释"生命周期限制，暂未实现"）。
//!
//! 现在改为：一个**无状态的 `ctx` userdata**（满足 'static）+ 一个线程局部的"当前绘制目标"
//! 裸指针。`set_draw_target(list)` 在每帧绘制前把指针指过去，`clear_draw_target()` 画完复位。
//! 整个程序单线程跑，指针在 `draw` 调用期间恒定有效，安全。

use std::cell::RefCell;

use mlua::{Lua, UserData, UserDataMethods};

use crate::core::color::Argb;
use crate::core::draw::{
    Blend, BlendMode, DrawCmd, DrawList, GradStop, Layer, Paint, PointRange, Shape,
};
use crate::core::geometry::Point;

thread_local! {
    /// 当前帧的绘制目标（裸指针，单线程下安全）。
    static TARGET: RefCell<Option<*mut DrawList>> = const { RefCell::new(None) };
    /// 当前帧可用的悬浮标位图（PNG 图片类型专用；由宿主在 call_draw 前设置）。
    static HOVER_IMAGE: RefCell<Option<std::rc::Rc<crate::core::image::RgbaBitmap>>> =
        const { RefCell::new(None) };
}

/// 设置本帧悬浮标脚本可用的位图（PNG 图片类型）。
pub fn set_hover_image(img: Option<std::rc::Rc<crate::core::image::RgbaBitmap>>) {
    HOVER_IMAGE.with(|i| *i.borrow_mut() = img);
}

/// 在调用 Lua `draw(ctx)` 前设置本帧的绘制目标；画完务必配对调用 [`clear_draw_target`]。
pub fn set_draw_target(list: &mut DrawList) {
    // ★ 每个脚本、每帧的 draw 前都把写入层复位到 Normal：脚本只在自己 draw 里用
    // ctx:layer 切换层，不把上一个脚本留下的层状态带到下一个。层状态是 DrawList 上的
    // 字段，不清这里的复位，前一个声明了 base 的脚本之后所有原生 push 都会错进 base 桶。
    list.set_layer(Layer::Normal);
    TARGET.with(|t| *t.borrow_mut() = Some(list as *mut DrawList));
}

/// 清空绘制目标（每帧 `draw` 结束后调用，避免悬垂指针残留），并把写入层复位为 Normal。
pub fn clear_draw_target() {
    TARGET.with(|t| {
        let p = t.borrow_mut().take();
        if let Some(p) = p {
            // SAFETY: 单线程；指针仍在 set/clear 的存活区间内（与 `with_list` 同源）。
            unsafe { (&mut *p).set_layer(Layer::Normal) };
        }
    });
}

/// 在持有 TARGET 期间对 `DrawList` 做可变操作。
///
/// 闭包内不得再触碰 `TARGET`（否则 `RefCell` 重入 panic）。所有绘图方法都只 push 指令，
/// 不会重入，安全。
///
/// ★ 返回 `Option` 而非 panic：绘图方法从 Lua `init`/`on_move`/`step` 里误调用时，
/// 本帧的绘制目标尚未设置（`set_draw_target` 只在 `draw(ctx)` 前调用），此时安全返回
/// `None`，由调用方静默丢弃——绝不能 `panic!` 把整个进程带崩（暴露给 Lua 的 API 不准 panic）。
fn with_list<R>(f: impl FnOnce(&mut DrawList) -> R) -> Option<R> {
    let p = TARGET.with(|t| *t.borrow_mut())?;
    // SAFETY: 单线程；指针在 set/clear 之间恒定指向同一份活着的 DrawList。
    let list = unsafe { &mut *p };
    Some(f(list))
}

/// 绘图上下文 userdata。本身无状态（满足 'static），所有状态走线程局部 `TARGET`。
pub struct DrawListCtx;

impl UserData for DrawListCtx {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method_mut(
            "circle",
            |_, _this, (x, y, radius, r, g, b, a): (f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                let center = Point::new(x, y);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle { center, radius },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::SourceOver,
                    });
                });
                Ok(())
            },
        );

        // 擦除圆：在已有图元上"挖洞"——把圆内像素清成完全透明（Blend::Clear）。
        // 聚光灯用它在大块暗罩上开圆形光圈：只光栅化一个小圆（包围盒 ≈ 2r×2r），
        // 而不是整屏多边形（旧实现 11ms/帧的卡顿主因）。颜色被 Clear 忽略，传 0 即可。
        methods.add_method_mut(
            "clear_circle",
            |_, _this, (x, y, radius): (f32, f32, f32)| {
                let center = Point::new(x, y);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle { center, radius },
                        paint: Paint::Solid(Argb::TRANSPARENT, BlendMode::Clear),
                        alpha: 1.0,
                        blend: Blend::Clear,
                    });
                });
                Ok(())
            },
        );

        // 绘制层：把本脚本**后续所有**绘制指令归入 "base"（垫底，先画）/"top"（顶层，后画）。
        // 未调用默认 "normal"。★ 聚光灯整屏暗罩用 `Clear` 挖洞——破坏性擦除，会把缓冲里
        // 已画的一切（含波纹/拖尾）清成透明——它必须垫底，挖洞才只清自己的暗罩、
        // 让叠在上面的特效保持可见。`ctx:layer("base")` 让聚光灯无论被当作悬浮标还是
        // 拖尾特效，暗罩始终先画，顺序正确且不依赖被加载到哪个目录。
        methods.add_method_mut("layer", |_, _this, name: String| {
            let l = match name.as_str() {
                "base" => Layer::Base,
                "top" => Layer::Top,
                "normal" => Layer::Normal,
                other => {
                    log::warn!("ctx:layer 收到未知层名 `{other}`，按 normal 处理");
                    Layer::Normal
                }
            };
            with_list(|list| list.set_layer(l));
            Ok(())
        });

        // 圆环：`outer_r` 是环的**外半径**，`width` 从外径向内收缩（用户语义）。
        // 内部转成 stroke_circle（中心线半径 = outer_r - width/2）；渲染层把描边圆
        // 展开成自适应细分的闭合环带多边形（见 render.rs，无缺角）。
        methods.add_method_mut(
            "ring",
            |_, _this, (x, y, outer_r, width, r, g, b, a): (f32, f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                let center = Point::new(x, y);
                let width = width.max(1.0);
                let outer_r = outer_r.max(width);
                with_list(|list| {
                    if width >= outer_r {
                        // 弧宽 ≥ 外径：环带内径归零，退化为实心圆。
                        list.push(DrawCmd::Fill {
                            shape: Shape::Circle {
                                center,
                                radius: outer_r,
                            },
                            paint: Paint::Solid(color, BlendMode::SourceOver),
                            alpha: 1.0,
                            blend: Blend::SourceOver,
                        });
                    } else {
                        list.stroke_circle(center, outer_r - width * 0.5, width, color);
                    }
                });
                Ok(())
            },
        );

        // 悬浮标图片：把宿主预加载的位图以 (x,y) 为**中心**贴出，尺寸 = 图片原始宽高 × `scale`。
        // ★ 2026-09-22 改：第三个参数从"边长"改成"缩放倍数"——早先按正方形铺，非方图会被
        //   拉变形（或被裁），现在宽高按原比例一起缩放，鼠标落在图片正中。
        // 位图由 `set_hover_image` 在每帧绘制前注入（未设置或无图时静默跳过）。
        methods.add_method_mut(
            "image",
            |_, _this, (x, y, scale, a): (f32, f32, f32, u8)| {
                let img = HOVER_IMAGE.with(|i| i.borrow().clone());
                if let Some(bmp) = img {
                    let k = scale.max(0.01);
                    let w = bmp.w as f32 * k;
                    let h = bmp.h as f32 * k;
                    let rect = crate::core::geometry::Rect::new(x - w * 0.5, y - h * 0.5, w, h);
                    with_list(|list| {
                        list.image(bmp, rect, f32::from(a) / 255.0);
                    });
                }
                Ok(())
            },
        );

        methods.add_method_mut(
            "rect",
            |_, _this, (x, y, w, h, r, g, b, a): (f32, f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Rect {
                            rect: crate::core::geometry::Rect::new(x, y, w, h),
                            radius: 0.0,
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::SourceOver,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "line",
            |_, _this, (x1, y1, x2, y2, width, r, g, b, a): (f32, f32, f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                with_list(|list| {
                    list.push(DrawCmd::Stroke {
                        shape: Shape::Line {
                            a: Point::new(x1, y1),
                            b: Point::new(x2, y2),
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        width,
                        cap: crate::core::draw::Cap::Round,
                        join: crate::core::draw::Join::Round,
                        blend: Blend::SourceOver,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "diamond",
            |_, _this, (x, y, size, r, g, b, a): (f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                let s = size;
                let p0 = Point::new(x, y - s);
                let p1 = Point::new(x + s, y);
                let p2 = Point::new(x, y + s);
                let p3 = Point::new(x - s, y);
                with_list(|list| {
                    let start = list.points_len() as u32;
                    list.push_point(p0);
                    list.push_point(p1);
                    list.push_point(p2);
                    list.push_point(p3);
                    list.push(DrawCmd::Fill {
                        shape: Shape::Polyline {
                            points: PointRange { start, len: 4 },
                            closed: true,
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::Plus,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "glow",
            |_, _this, (x, y, radius, r, g, b, a): (f32, f32, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                let center = Point::new(x, y);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle {
                            center,
                            radius: radius * 2.5,
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 0.12,
                        blend: Blend::Plus,
                    });
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle {
                            center,
                            radius: radius * 1.2,
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 0.30,
                        blend: Blend::Plus,
                    });
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle { center, radius },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::Plus,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "plus_circle",
            |_, _this, (x, y, radius, r, g, b, a): (f32, f32, f32, u8, u8, u8, u8)| {
                // 加色（Blend::Plus）实心圆：辉光层用。alpha 全部烘在颜色里（cmd alpha 1.0，
                // 避免"颜色与 cmd 双写=平方"的老坑）。
                let color = Argb::new(a, r, g, b);
                let center = Point::new(x, y);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle { center, radius },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::Plus,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "radial",
            |_, _this, (x, y, radius, r, g, b, a): (f32, f32, f32, u8, u8, u8, u8)| {
                // 径向渐变圆：中心 color → 边缘透明（SourceOver）。辉光圆影用。
                let color = Argb::new(a, r, g, b);
                let center = Point::new(x, y);
                with_list(|list| {
                    list.push(DrawCmd::Fill {
                        shape: Shape::Circle { center, radius },
                        paint: Paint::Radial {
                            center,
                            radius,
                            stops: vec![
                                GradStop { offset: 0.0, color },
                                GradStop {
                                    offset: 1.0,
                                    color: Argb::TRANSPARENT,
                                },
                            ],
                        },
                        alpha: 1.0,
                        blend: Blend::SourceOver,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "diamond_rot",
            |_, _this, (x, y, s, angle, r, g, b, a): (f32, f32, f32, f32, u8, u8, u8, u8)| {
                // 旋转菱形（Blend::Plus）：像素尘埃的粒子核心。
                let color = Argb::new(a, r, g, b);
                let (sin_a, cos_a) = angle.sin_cos();
                let p0 = Point::new(x - sin_a * s, y - cos_a * s);
                let p1 = Point::new(x + cos_a * s, y - sin_a * s);
                let p2 = Point::new(x + sin_a * s, y + cos_a * s);
                let p3 = Point::new(x - cos_a * s, y + sin_a * s);
                with_list(|list| {
                    let start = list.points_len() as u32;
                    list.push_point(p0);
                    list.push_point(p1);
                    list.push_point(p2);
                    list.push_point(p3);
                    list.push(DrawCmd::Fill {
                        shape: Shape::Polyline {
                            points: PointRange { start, len: 4 },
                            closed: true,
                        },
                        paint: Paint::Solid(color, BlendMode::SourceOver),
                        alpha: 1.0,
                        blend: Blend::Plus,
                    });
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "polygon",
            |_, _this, (points_lua, r, g, b, a): (mlua::Table, u8, u8, u8, u8)| {
                // 普通混合（SourceOver）填充多边形：飘带用（fill_polygon 是加色版）。
                let color = Argb::new(a, r, g, b);
                with_list(|list| {
                    let start = list.points_len() as u32;
                    let mut len = 0u32;
                    let table_len = points_lua.raw_len();
                    for i in 1..=table_len {
                        if let Ok(pt) = points_lua.get::<mlua::Table>(i) {
                            let x: f32 = pt.get::<f32>(1).unwrap_or(0.0);
                            let y: f32 = pt.get::<f32>(2).unwrap_or(0.0);
                            list.push_point(Point::new(x, y));
                            len += 1;
                        }
                    }
                    if len >= 3 {
                        list.push(DrawCmd::Fill {
                            shape: Shape::Polyline {
                                points: PointRange { start, len },
                                closed: true,
                            },
                            paint: Paint::Solid(color, BlendMode::SourceOver),
                            alpha: 1.0,
                            blend: Blend::SourceOver,
                        });
                    }
                });
                Ok(())
            },
        );

        methods.add_method_mut(
            "fill_polygon",
            |_, _this, (points_lua, r, g, b, a): (mlua::Table, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                with_list(|list| {
                    let start = list.points_len() as u32;
                    let mut len = 0u32;
                    let table_len = points_lua.raw_len();
                    for i in 1..=table_len {
                        if let Ok(pt) = points_lua.get::<mlua::Table>(i) {
                            let x: f32 = pt.get::<f32>(1).unwrap_or(0.0);
                            let y: f32 = pt.get::<f32>(2).unwrap_or(0.0);
                            list.push_point(Point::new(x, y));
                            len += 1;
                        }
                    }
                    if len >= 3 {
                        list.push(DrawCmd::Fill {
                            shape: Shape::Polyline {
                                points: PointRange { start, len },
                                closed: true,
                            },
                            paint: Paint::Solid(color, BlendMode::SourceOver),
                            alpha: 1.0,
                            blend: Blend::Plus,
                        });
                    }
                });
                Ok(())
            },
        );

        // ── 文字（2026-09-28 新增：供「随机文字」这类特效用）──
        //
        // ★ 两个入口的唯一区别是**锚点**：`text` 给的是排版盒左上角（与 `DrawCmd::Text`
        // 同义），`text_centered` 给的是**几何中心**（`DrawCmd::TextCentered`）。
        // 特效脚本要的是后者——"在点击点周围随机冒出文字"，给的坐标是文字**中心**；
        // 用左上角会让文字整体右下偏半个字宽/行高，而且模块层估不出真实行高
        // （拿不到字体），必须由渲染层按实测度量居中，这就是 `TextCentered` 存在的理由。
        methods.add_method_mut(
            "text",
            |_, _this, (x, y, s, size, r, g, b, a): (f32, f32, String, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                with_list(|list| list.text(x, y, &s, size, color, None));
                Ok(())
            },
        );
        methods.add_method_mut(
            "text_centered",
            |_, _this, (cx, cy, s, size, r, g, b, a): (f32, f32, String, f32, u8, u8, u8, u8)| {
                let color = Argb::new(a, r, g, b);
                with_list(|list| {
                    list.text_centered(Point::new(cx, cy), &s, size, color, None);
                });
                Ok(())
            },
        );
    }
}

/// 注册绘图相关全局：`ctx` userdata + `hsv_to_rgb` 助手。
pub fn register_draw_api(lua: &Lua) -> mlua::Result<()> {
    // 每个 VM 一个 'static 的 ctx userdata；它的"当前目标"由线程局部 TARGET 决定。
    lua.globals().set("ctx", DrawListCtx)?;

    lua.globals().set(
        "hsv_to_rgb",
        lua.create_function(
            |_, (h, s, v): (f32, f32, f32)| -> mlua::Result<(u8, u8, u8)> {
                let c = v * s;
                let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
                let m = v - c;
                let (r, g, b) = if h < 60.0 {
                    (c, x, 0.0)
                } else if h < 120.0 {
                    (x, c, 0.0)
                } else if h < 180.0 {
                    (0.0, c, x)
                } else if h < 240.0 {
                    (0.0, x, c)
                } else if h < 300.0 {
                    (c, 0.0, x)
                } else {
                    (x, 0.0, c)
                };
                Ok((
                    ((r + m) * 255.0) as u8,
                    ((g + m) * 255.0) as u8,
                    ((b + m) * 255.0) as u8,
                ))
            },
        )?,
    )?;

    Ok(())
}
