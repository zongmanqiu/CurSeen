-- 激光笔：隐藏系统指针，在指针位置画一个高亮实心圆。
--
-- ★ 本脚本声明 `hide_cursor = true`：运行时据此把系统**箭头**藏起来（全局只换
--   OCR_NORMAL；I 形/手形/缩放提示保留，见 `platform::hide_cursor`），退出时自动恢复。这正是激光笔的核心
--   ——真指针多余，观众只看这个发光的圆点。
-- 形态：单个实心圆，没有光晕/描边。颜色 / 半径可配，透明度在颜色里给
-- （`alpha = true` → `hex_to_rgb` 第四返回值），独立的「不透明度」参数已删除。

effect_name = "激光笔"
name = "激光笔"
role = "effect"
hide_cursor = true

function schema()
  return {
    { key = "radius", label = "半径", type = "int",   min = 2, max = 200, step = 1, default = 10 },
    { key = "color",  label = "颜色", type = "color", alpha = true, default = "#FF000040" },
  }
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#FF000040")
  local rad = params.radius or 10
  ctx:circle(mouse_x, mouse_y, rad, r, g, b, a)
end
