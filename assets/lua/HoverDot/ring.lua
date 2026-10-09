-- 圆环悬浮标：中心镂空的闭合圆环。
-- 参数三项：外径 / 弧宽（由外径向内收缩） / 颜色（含透明度）。
-- 渲染层把描边圆展开成自适应细分的闭合环带多边形（见 runtime/render.rs），
-- 任意外径下都 360° 严格闭合，无缺角。
--
-- ★ 透明度在**颜色里**给（`alpha = true` → `hex_to_rgb` 第四返回值），独立的
--   「不透明度」参数已删除。

effect_name = "圆环"
name = "圆环"
role = "effect"
function schema()
  return {
    { key = "outer", label = "外径", type = "int",   min = 4, max = 1000, step = 1, default = 50 },
    { key = "arc",   label = "弧宽", type = "int",   min = 1, max = 999,  step = 1, default = 3 },
    { key = "color", label = "颜色", type = "color", alpha = true, default = "#FF000040" },
  }
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#FF000040")
  local outer = params.outer or 50
  local arc = params.arc or 3
  -- ctx:ring 的语义：outer_r = 环的外半径，width = 从外径向内的环体厚度
  ctx:ring(mouse_x, mouse_y, outer / 2, arc, r, g, b, a)
end
