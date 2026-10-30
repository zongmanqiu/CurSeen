-- MFCMouseEffect · Signal Ring
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 强调红圈：更大柔光 + 外环 + 中环 + 白色核心。光标替换类，只隐藏箭头。

name = "Signal Ring"
effect_name = "Signal Ring"
kind = "cursor"
description = "MFCMouseEffect Signal Ring：大柔光强调红圈光标。"
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
  local mid = radius * 0.72
  local core = radius * 0.26
  local accent_a = math.floor(255 * alpha)

  ctx:radial_stops(mx, my, radius * 1.72, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.22) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })
  ctx:ring(mx, my, radius, math.max(2.2, radius * 0.18), r, g, b,
    math.floor(255 * alpha * 0.94))
  ctx:ring(mx, my, mid, math.max(1.3, radius * 0.09), r, g, b,
    math.floor(255 * alpha * 0.58))
  ctx:circle(mx, my, core, 255, 255, 255, math.min(255, accent_a + 36))
end
