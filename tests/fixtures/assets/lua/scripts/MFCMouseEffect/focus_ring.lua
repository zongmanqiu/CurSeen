-- MFCMouseEffect · Focus Ring
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 专注指示圈：外层软光 + 外环 + 内环 + 半透明核心。光标替换类，只隐藏箭头。

name = "Focus Ring"
effect_name = "Focus Ring"
kind = "cursor"
description = "MFCMouseEffect Focus Ring：带软光与外/内双环的专注光标。"
hide_cursor = true

function schema()
  return {
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#FF5A5A" },
    { key = "size",    label = "外环半径", type = "float", min = 6,  max = 72, step = 1,   default = 22 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.15, max = 1, step = 0.01, default = 0.82 },
  }
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#FF5A5A")
  local radius = params.size or 22
  local alpha = (params.opacity or 0.82) * ca / 255
  local mx, my = mouse_x or 0, mouse_y or 0
  local inner = radius * 0.45

  ctx:radial_stops(mx, my, radius * 1.45, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.16) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })
  ctx:ring(mx, my, radius, math.max(1.6, radius * 0.16), r, g, b,
    math.floor(255 * alpha * 0.82))
  ctx:ring(mx, my, inner, math.max(1.0, radius * 0.07), r, g, b,
    math.floor(255 * alpha * 0.36))
  ctx:circle(mx, my, inner * 0.58, r, g, b, math.floor(255 * alpha * 0.22))
end
