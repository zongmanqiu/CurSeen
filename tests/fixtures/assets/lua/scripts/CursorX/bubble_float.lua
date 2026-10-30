-- CursorX · Bubble Float
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：指针处不断冒出上浮的气泡（带高光与边缘），点击把所有气泡戳破成
-- 小水珠。指针停在交互提示上时冒得更快、气泡更大。

name = "Bubble Float"
effect_name = "Bubble Float"
kind = "cursor"
description = "CursorX Bubble Float：从指针升起的气泡，点击全部戳破。"
hide_cursor = true

local MAX_BUBBLES = 90
local bubbles = {}
local drops = {}
local frame = 0

function schema()
  return {
    { key = "hue",         label = "气泡色相", type = "int",   min = 0,   max = 360, step = 5,    default = 200 },
    { key = "spawnRate",   label = "生成间隔(帧)", type = "int",   min = 1,   max = 10,  step = 1,    default = 5 },
    { key = "maxSize",     label = "最大尺寸", type = "int",   min = 8,   max = 40,  step = 1,    default = 25 },
    { key = "riseSpeed",   label = "上浮速度", type = "float", min = 0.3, max = 3,   step = 0.1,  default = 1.6 },
    { key = "pointerAnim", label = "悬停加速", type = "bool",  default = true },
    { key = "pointerSizeMult", label = "悬停放大", type = "float", min = 1.0, max = 2.5, step = 0.05, default = 1.55 },
    { key = "clickAnim",   label = "点击戳破", type = "bool",  default = true },
  }
end

function init()
  bubbles = {}
  drops = {}
  frame = 0
end

local function bubble(x, y, params, hover)
  if #bubbles >= MAX_BUBBLES then return end
  local hue = params.hue or 200
  local mult = hover and (params.pointerSizeMult or 1.55) or 1.0
  local r = rng(7, math.max(8, params.maxSize or 25)) * mult
  bubbles[#bubbles + 1] = {
    x = x, y = y, r = r,
    vy = -(rng(0, params.riseSpeed or 1.6) + 0.6) * (hover and 1.25 or 1.0),
    vx = rng(-0.45, 0.45),
    wobble = rng(0, math.pi * 2),
    w_spd = rng(0.025, 0.08) * (hover and 1.75 or 1.0),
    alpha = 0.72,
    hue = hue + rng(-20, 20),
  }
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  for i = 1, #bubbles do
    local b = bubbles[i]
    for _ = 1, 5 do
      local a = rng(0, math.pi * 2)
      local spd = rng(1.2, 3.4)
      drops[#drops + 1] = {
        x = b.x, y = b.y,
        vx = math.cos(a) * spd,
        vy = math.sin(a) * spd,
        r = rng(1.0, 2.5),
        life = 1.0,
        hue = b.hue,
      }
    end
  end
  bubbles = {}
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  frame = frame + f

  local hover = cursor_hover and params.pointerAnim ~= false
  local rate = math.max(1, math.floor(params.spawnRate or 5))
  if hover then rate = math.max(1, math.floor(rate / 2)) end
  -- 用浮点累加避免掉帧时漏掉生成节拍。
  while frame >= rate do
    frame = frame - rate
    bubble(mouse_x or 0, mouse_y or 0, params, hover)
  end

  for i = #bubbles, 1, -1 do
    local b = bubbles[i]
    b.wobble = b.wobble + b.w_spd * f
    b.x = b.x + (b.vx + math.sin(b.wobble) * 0.65) * f
    b.y = b.y + b.vy * f
    b.alpha = b.alpha - 0.004 * f
    if b.alpha <= 0 then table.remove(bubbles, i) end
  end

  for i = #drops, 1, -1 do
    local p = drops[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    p.vy = p.vy + 0.05 * f
    p.life = p.life - 0.035 * f
    if p.life <= 0 then table.remove(drops, i) end
  end
end

function draw(ctx, params)
  for i = 1, #bubbles do
    local b = bubbles[i]
    local r, g, bl = hsv_to_rgb(b.hue % 360, 0.62, 0.72)
    local a = math.floor(255 * clamp(b.alpha, 0, 1))
    -- 主体：低透明度球 + 偏左上高光（径向渐变不可偏移中心，用两层圆近似）。
    ctx:circle(b.x, b.y, b.r, r, g, bl, math.floor(a * 0.22))
    ctx:radial_stops(b.x - b.r * 0.3, b.y - b.r * 0.3, b.r * 1.05, {
      { offset = 0.0,  r = 235, g = 248, b = 255, a = math.floor(a * 0.55) },
      { offset = 0.55, r = r,   g = g,   b = bl,  a = math.floor(a * 0.10) },
      { offset = 1.0,  r = r,   g = g,   b = bl,  a = 0 },
    })
    ctx:ring(b.x, b.y, b.r, 1.2, 255, 255, 255, math.floor(a * 0.55))
    ctx:circle(b.x - b.r * 0.32, b.y - b.r * 0.32, math.max(0.8, b.r * 0.18),
      255, 255, 255, math.floor(a * 0.75))
  end

  for i = 1, #drops do
    local p = drops[i]
    local r, g, b = hsv_to_rgb(p.hue % 360, 0.85, 0.78)
    ctx:circle(p.x, p.y, math.max(0.4, p.r * p.life), r, g, b,
      math.floor(255 * clamp(p.life, 0, 1)))
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  ctx:ring(mx, my, 7, 1.4, 255, 255, 255, 190)
  ctx:circle(mx, my, 2.0, 0, 0, 0, 120)
end
