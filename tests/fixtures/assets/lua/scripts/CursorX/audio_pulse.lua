-- CursorX · Audio Pulse
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：青蓝光点钉在指针上，一枚滞后圆环拖着走；悬停时两者放大、环内透出
-- 紫色内光；点击荡开最多 5 圈波纹。上游名叫 Audio Pulse，但实现并不读麦克风。

name = "Audio Pulse"
effect_name = "Audio Pulse"
kind = "cursor"
description = "CursorX Audio Pulse：青蓝光点 + 滞后脉冲环。"
hide_cursor = true

local MAX_RIPPLES = 5
local ripples = {}
local rx, ry = 0, 0
local dot_scale = 1.0
local ring_scale = 1.0

function schema()
  return {
    { key = "dotColor",       label = "光点颜色", type = "color", alpha = true, default = "#00E5FF" },
    { key = "ringColor",      label = "圆环颜色", type = "color", alpha = true, default = "#00E5FF" },
    { key = "glowColor",      label = "辉光颜色", type = "color", alpha = true, default = "#6366F1" },
    { key = "dotSize",        label = "光点大小", type = "int",   min = 4,   max = 16,  step = 1,    default = 8 },
    { key = "ringSize",       label = "圆环大小", type = "int",   min = 20,  max = 60,  step = 2,    default = 36 },
    { key = "ringLerp",       label = "圆环滞后", type = "float", min = 0.03,max = 0.3, step = 0.01, default = 0.09 },
    { key = "glowBlur",       label = "辉光半径", type = "int",   min = 4,   max = 30,  step = 1,    default = 12 },
    { key = "ringBorderWidth",label = "圆环粗细", type = "float", min = 1,   max = 4,   step = 0.5,  default = 1.5 },
    { key = "pointerAnim",    label = "悬停放大", type = "bool",  default = true },
    { key = "pointerRingScale", label = "环放大倍数", type = "float", min = 1.2, max = 3.0, step = 0.1, default = 1.8 },
    { key = "pointerDotScale",  label = "点放大倍数", type = "float", min = 1.0, max = 3.0, step = 0.1, default = 1.8 },
    { key = "clickAnim",      label = "点击波纹", type = "bool",  default = true },
    { key = "rippleMaxSize",  label = "波纹最大直径", type = "int", min = 40, max = 150, step = 5,  default = 90 },
  }
end

function init()
  ripples = {}
  rx, ry = mouse_x or 0, mouse_y or 0
  dot_scale = 1.0
  ring_scale = 1.0
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  ripples[#ripples + 1] = { x = x, y = y, start = wall_ms or 0 }
  while #ripples > MAX_RIPPLES do table.remove(ripples, 1) end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or rx, mouse_y or ry
  local hover = cursor_hover and params.pointerAnim ~= false

  local k = math.min(1.0, (params.ringLerp or 0.09) * f)
  rx = rx + (mx - rx) * k
  ry = ry + (my - ry) * k

  local target_dot = hover and (params.pointerDotScale or 1.8) or 1.0
  local target_ring = hover and (params.pointerRingScale or 1.8) or 1.0
  dot_scale = dot_scale + (target_dot - dot_scale) * math.min(1.0, 0.12 * f)
  ring_scale = ring_scale + (target_ring - ring_scale) * math.min(1.0, 0.12 * f)

  local now = wall_ms or 0
  for i = #ripples, 1, -1 do
    if now - ripples[i].start >= 600 then table.remove(ripples, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local dr, dg, db, da = hex_to_rgb(params.dotColor or "#00E5FF")
  local rr, rg, rb, ra = hex_to_rgb(params.ringColor or "#00E5FF")
  local gr, gg, gb, ga = hex_to_rgb(params.glowColor or "#6366F1")
  local mx, my = mouse_x or rx, mouse_y or ry
  local hover = cursor_hover and params.pointerAnim ~= false

  -- 滞后环
  local rs = (params.ringSize or 36) * ring_scale
  local border = params.ringBorderWidth or 1.5
  if hover then
    ctx:radial_stops(rx, ry, rs * 0.5, {
      { offset = 0.0, r = gr, g = gg, b = gb, a = math.floor(ga * 0.14) },
      { offset = 1.0, r = gr, g = gg, b = gb, a = 0 },
    })
  end
  ctx:ring(rx, ry, rs * 0.5 + border * 2.0, border * 4.0, rr, rg, rb, math.floor(ra * 0.10))
  ctx:ring(rx, ry, rs * 0.5, border, rr, rg, rb, math.floor(ra * (hover and 1.0 or 0.7)))

  -- 光点
  local ds = (params.dotSize or 8) * dot_scale
  local blur = params.glowBlur or 12
  ctx:glow(mx, my, ds * 0.5 + blur * 0.5, gr, gg, gb, math.floor(ga * 0.55))
  ctx:glow(mx, my, ds * 0.5 + 2.0, dr, dg, db, math.floor(da * 0.86))
  ctx:circle(mx, my, ds * 0.5, dr, dg, db, da)
  ctx:circle(mx, my, ds * 0.18, 255, 255, 255, math.floor(da * 0.92))

  -- 点击波纹：按墙钟推进，覆盖层休眠也不会卡在中间态。
  local now = wall_ms or 0
  for i = 1, #ripples do
    local rp = ripples[i]
    local t = math.min((now - rp.start) / 600.0, 1.0)
    local eased = 1.0 - (1.0 - t) * (1.0 - t)
    local radius = (10 + ((params.rippleMaxSize or 90) - 10) * eased) * 0.5
    local a = math.floor(255 * (1.0 - t))
    local cr, cg, cb = rr, rg, rb
    if t >= 0.5 then cr, cg, cb = gr, gg, gb end
    ctx:ring(rp.x, rp.y, radius, 2.0, cr, cg, cb, a)
  end
end
