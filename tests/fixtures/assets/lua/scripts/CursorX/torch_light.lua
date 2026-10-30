-- CursorX · Torch Light
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：整屏压暗 + 暖色光孔，光孔像真火把一样抖动、滞后；指针停在交互提示上
-- 时抖动更快更亮，点击闪出一圈大光。暗罩留在 base 层（clear_circle 破坏性擦除）。

name = "Torch Light"
effect_name = "Torch Light"
kind = "cursor"
description = "CursorX Torch Light：会闪烁的暖光火把，照亮指针周围。"
hide_cursor = true

local tx, ty = 0, 0
local cx, cy = 0, 0
local flick_phase = 0
local click_t = -1
local radius = 140
local started = false

function schema()
  return {
    { key = "radius",      label = "光照半径", type = "int",   min = 60,  max = 300, step = 10,   default = 140 },
    { key = "darkness",    label = "暗罩浓度", type = "float", min = 0.5, max = 0.98,step = 0.01, default = 0.91 },
    { key = "flickerIntensity", label = "闪烁强度", type = "int", min = 0, max = 20, step = 1,   default = 9 },
    { key = "pointerAnim", label = "悬停快闪", type = "bool",  default = true },
    { key = "pointerFlickerIntensity", label = "悬停闪烁倍数", type = "float", min = 1.0, max = 5.0, step = 0.1, default = 2.5 },
    { key = "clickAnim",   label = "点击闪光", type = "bool",  default = true },
    { key = "clickFlashRadius", label = "闪光半径", type = "int", min = 50, max = 400, step = 10, default = 200 },
  }
end

function init()
  tx, ty = mouse_x or 0, mouse_y or 0
  cx, cy = tx, ty
  flick_phase = 0
  click_t = -1
  radius = 140
  started = true
end

function on_mouse_down(button, x, y, params)
  if button == "left" and params.clickAnim ~= false then click_t = 0 end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  tx, ty = mouse_x or tx, mouse_y or ty
  local k = math.min(1.0, 0.11 * f)
  cx = cx + (tx - cx) * k
  cy = cy + (ty - cy) * k

  local hover = cursor_hover and params.pointerAnim ~= false
  local intensity = (params.flickerIntensity or 9) * (hover and (params.pointerFlickerIntensity or 2.5) or 1.0)
  flick_phase = flick_phase + (hover and 0.28 or 0.09) * f
  local flick = math.sin(flick_phase) * intensity + math.sin(flick_phase * 2.4) * intensity * 0.44

  local flash = 0
  if click_t >= 0 and params.clickAnim ~= false then
    flash = math.sin(click_t * math.pi) * (params.clickFlashRadius or 200)
    click_t = click_t + dt * 3.6
    if click_t >= 1 then click_t = -1 end
  end
  radius = (params.radius or 140) + flick + flash
end

function draw(ctx, params)
  params = params or {}
  local darkness = clamp(params.darkness or 0.91, 0, 1)
  local r = math.max(radius, 2.0)
  local hover = cursor_hover and params.pointerAnim ~= false
  -- 火光抖动：只偏移绘制位置，不回写弹簧基准（否则每帧累加会漂走）。
  local x = cx + math.sin(flick_phase * 1.7) * 0.6
  local y = cy + math.cos(flick_phase * 1.3) * 0.4

  ctx:layer("base")
  ctx:rect(screen_x or 0, screen_y or 0, screen_w or 0, screen_h or 0, 0, 0, 0,
    math.floor(255 * darkness))
  ctx:clear_circle(x, y, r)
  -- 光孔内补一层暖色（越靠中心越亮），外沿比清洞略大，接缝藏进均匀暗罩。
  ctx:radial_stops(x, y, r + 2.0, {
    { offset = 0.0,  r = 255, g = 150, b = 40,  a = hover and 40 or 28 },
    { offset = 0.38, r = 255, g = 90,  b = 10,  a = 14 },
    { offset = 0.72, r = 0,   g = 0,   b = 0,   a = math.floor(255 * darkness * 0.92) },
    { offset = 1.0,  r = 0,   g = 0,   b = 0,   a = math.floor(255 * math.min(1, darkness + 0.06)) },
  })

  ctx:layer("normal")
  local glow = hover and 20 or 14
  ctx:glow(x, y, glow, 255, 176, 64, 220)
  ctx:circle(x, y, hover and 5.2 or 4.0, 255, 196, 96, 255)
  ctx:circle(x, y, 1.8, 255, 245, 220, 255)
end
