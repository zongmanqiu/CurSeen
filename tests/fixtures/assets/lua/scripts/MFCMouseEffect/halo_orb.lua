-- MFCMouseEffect · Halo Orb
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 光晕球：宽光晕 + 中球 + 细环 + 白核心。光标替换类，只隐藏箭头。

name = "Halo Orb"
effect_name = "Halo Orb"
kind = "cursor"
description = "MFCMouseEffect Halo Orb：宽带光晕与细环的球形光标。"
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

  ctx:radial_stops(mx, my, radius * 2.2, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.14) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })
  ctx:radial_stops(mx, my, radius * 1.12, {
    { offset = 0.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.58) },
    { offset = 1.0, r = r, g = g, b = b, a = math.floor(255 * alpha * 0.12) },
  })
  ctx:ring(mx, my, radius * 0.92, math.max(1.2, radius * 0.08), r, g, b,
    math.floor(255 * alpha * 0.52))
  ctx:circle(mx, my, radius * 0.22, 255, 255, 255,
    math.min(255, accent_a + 20))
end
