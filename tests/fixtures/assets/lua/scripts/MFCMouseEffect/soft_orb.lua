-- MFCMouseEffect · Soft Orb
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 柔光球：宽柔光 + 柔光球 + 白核心。光标替换类，只隐藏箭头。

name = "Soft Orb"
effect_name = "Soft Orb"
kind = "cursor"
description = "MFCMouseEffect Soft Orb：带宽柔光的柔光球光标。"
hide_cursor = true

function schema()
  return {
    { key = "color",   label = "颜色",     type = "color", alpha = true, default = "#FF5A5A" },
    { key = "size",    label = "球半径",   type = "float", min = 6,  max = 72, step = 1,   default = 22 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.15, max = 1, step = 0.01, default = 0.82 },
  }
end

function draw(ctx, params)
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#FF5A5A")
  local radius = params.size or 22
  local alpha = (params.opacity or 0.82) * ca / 255
  local mx, my = mouse_x or 0, mouse_y or 0
  local accent_a = math.floor(255 * alpha)

  ctx:radial_stops(mx, my, radius * 1.8, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.18) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })
  ctx:radial_stops(mx, my, radius, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.86) },
    { offset = 0.55, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.55) },
    { offset = 1.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.22) },
  })
  ctx:circle(mx, my, radius * 0.34, 255, 255, 255,
    math.min(255, accent_a + 26))
end
