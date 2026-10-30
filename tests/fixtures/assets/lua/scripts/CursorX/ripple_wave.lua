-- CursorX · Ripple Wave
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：指针周围一圈白色细环；点击从点击点荡开多圈同色波纹。
-- 指针停在交互提示上时中心环按呼吸节奏放大。

name = "Ripple Wave"
effect_name = "Ripple Wave"
kind = "cursor"
description = "CursorX Ripple Wave：点击荡开彩色波纹，悬停时中心环呼吸。"
hide_cursor = true

local ripples = {}
local phase = 0

function schema()
  return {
    { key = "hue",              label = "波纹色相", type = "int",   min = 0,   max = 360, step = 5,    default = 220 },
    { key = "speed",            label = "扩散速度", type = "float", min = 1,   max = 8,   step = 0.1,  default = 3.2 },
    { key = "count",            label = "每次圈数", type = "int",   min = 1,   max = 5,   step = 1,    default = 3 },
    { key = "pointerAnim",      label = "悬停呼吸", type = "bool",  default = true },
    { key = "pointerBreathMult",label = "呼吸倍数", type = "float", min = 1.1, max = 2.5, step = 0.1,  default = 1.6 },
    { key = "clickAnim",        label = "点击波纹", type = "bool",  default = true },
  }
end

function init()
  ripples = {}
  phase = 0
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  local n = math.floor(params.count or 3)
  local hue = params.hue or 220
  for i = 1, n do
    ripples[#ripples + 1] = {
      x = x, y = y, r = 0,
      alpha = 0.82 - (i - 1) * 0.2,
      age = -(i - 1) * 5,
      hue = hue + rng(-15, 15),
    }
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  phase = phase + dt * 5.2

  for i = #ripples, 1, -1 do
    local rp = ripples[i]
    rp.age = rp.age + f
    if rp.age >= 0 then
      rp.r = rp.r + (params.speed or 3.2) * f
      rp.alpha = rp.alpha - 0.011 * f
    end
    if rp.alpha <= 0 then table.remove(ripples, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  local a = 230
  local radius = 4.5
  if cursor_hover and params.pointerAnim ~= false then
    local breath = 1.0 + (0.3 + math.sin(phase * 3.0) * 0.3) * ((params.pointerBreathMult or 1.6) - 1.0) / 0.6
    radius = radius * breath
    a = 255
  end
  ctx:ring(mx, my, radius + 1.2, 1.6, 0, 0, 0, 110)
  ctx:ring(mx, my, radius, 1.4, 255, 255, 255, a)

  for i = 1, #ripples do
    local rp = ripples[i]
    if rp.age >= 0 then
      local r, g, b = hsv_to_rgb(rp.hue % 360, 0.8, 0.7)
      local al = math.floor(255 * clamp(rp.alpha, 0, 1))
      ctx:ring(rp.x, rp.y, rp.r + 1.0, 3.0, r, g, b, math.floor(al * 0.18))
      ctx:ring(rp.x, rp.y, rp.r, 2.0, r, g, b, al)
    end
  end
end
