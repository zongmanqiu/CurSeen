-- 实心圆悬浮标：内部完全填实的圆盘。
-- 参数两项：直径 / 颜色（没有弧宽——那是圆环的参数）。
--
-- ★ 透明度在**颜色里**给：颜色参数声明 `alpha = true`，取色器长出透明度条，
--   脚本端 `hex_to_rgb` 的第四个返回值就是它（0–255）。独立的「不透明度」参数已删除
--   ——两套透明度并存会互相打架（详见 HoverDot/README.md）。

effect_name = "实心圆"
name = "实心圆"
role = "effect"
function schema()
  return {
    { key = "diameter", label = "直径", type = "int",   min = 4, max = 1000, step = 1, default = 50 },
    { key = "color",    label = "颜色", type = "color", alpha = true, default = "#FF000040" },
  }
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#FF000040")
  local d = params.diameter or 50
  ctx:circle(mouse_x, mouse_y, d / 2, r, g, b, a)
end
