-- CursorX · Fire Trail
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：指针处持续冒火苗（青焰→橙焰→暗红按寿命过渡），点击炸出四散余烬。
-- 指针停在交互提示上时火苗变小变透、整体放大，给目标让路。

name = "Fire Trail"
effect_name = "Fire Trail"
kind = "cursor"
description = "CursorX Fire Trail：持续燃烧的火焰拖尾，点击迸出余烬。"
hide_cursor = true

local MAX_FLAMES = 320
local flames = {}
local spawn_acc = 0

function schema()
  return {
    { key = "count",       label = "每帧火苗", type = "int",   min = 1,   max = 10,  step = 1,    default = 5 },
    { key = "rise",        label = "上升速度", type = "float", min = 0.5, max = 4,   step = 0.25, default = 2 },
    { key = "size",        label = "火苗大小", type = "int",   min = 2,   max = 12,  step = 1,    default = 4 },
    { key = "pointerAnim", label = "悬停变透", type = "bool",  default = true },
    { key = "pointerAlpha",label = "悬停透明度", type = "float", min = 0.1, max = 0.8, step = 0.05, default = 0.35 },
    { key = "pointerSizeMult", label = "悬停放大", type = "float", min = 1.0, max = 3.0, step = 0.1, default = 1.5 },
    { key = "clickAnim",   label = "点击余烬", type = "bool",  default = true },
    { key = "clickEmberCount", label = "余烬数量", type = "int", min = 5, max = 50, step = 5,    default = 25 },
  }
end

function init()
  flames = {}
  spawn_acc = 0
end

local function flame(x, y, params, ember)
  if #flames >= MAX_FLAMES then return end
  local rise = params.rise or 2
  local size = params.size or 4
  flames[#flames + 1] = {
    x = x + rng(-4, 4), y = y,
    vx = rng(-0.9, 0.9),
    vy = -(rng(0, rise) + 1.2),
    life = 1.0,
    decay = ember and rng(0.015, 0.05) or rng(0.016, 0.044),
    size = ember and rng(3, 8) or rng(4, 4 + size),
    ember = ember,
  }
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  local n = math.floor(params.clickEmberCount or 25)
  for _ = 1, n do
    local a = rng(0, math.pi * 2)
    local spd = rng(2.0, 6.5)
    flame(x, y, params, true)
    local f = flames[#flames]
    if f then
      f.vx = math.cos(a) * spd
      f.vy = math.sin(a) * spd - 1.0
    end
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or 0, mouse_y or 0

  spawn_acc = spawn_acc + (params.count or 5) * f
  while spawn_acc >= 1 do
    flame(mx, my, params, false)
    spawn_acc = spawn_acc - 1
  end

  for i = #flames, 1, -1 do
    local p = flames[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    if p.ember then
      p.vy = p.vy + 0.06 * f
    else
      p.vx = p.vx + rng(-0.175, 0.175) * f
    end
    p.life = p.life - p.decay * f
    p.size = p.size * math.pow(0.968, f)
    if p.life <= 0 then table.remove(flames, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local hover = cursor_hover and params.pointerAnim ~= false
  local alpha_mul = hover and (params.pointerAlpha or 0.35) or 1.0
  local size_mul = hover and (params.pointerSizeMult or 1.5) or 1.0

  for i = 1, #flames do
    local p = flames[i]
    local hue, sat, lit
    if p.life > 0.55 then
      hue = 42 + (1 - p.life) * 22
      sat, lit = 100, 52 + p.life * 18
    elseif p.life > 0.2 then
      hue, sat, lit = 12, 100, 52 + p.life * 18
    else
      hue, sat, lit = 0, math.max(0, (p.life / 0.2) * 40), 35
    end
    local r, g, b = hsv_to_rgb(hue % 360, sat / 100, lit / 100)
    local a = math.floor(255 * clamp(p.life * 0.92 * alpha_mul, 0, 1))
    local size = math.max(0.4, p.size * size_mul)
    ctx:circle(p.x, p.y, size, r, g, b, a)
    ctx:plus_circle(p.x, p.y, size * 1.8, r, g, b, math.floor(a * 0.2))
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  ctx:circle(mx, my, 3.4, 40, 10, 0, 150)
  ctx:circle(mx, my, 2.4, 255, 190, 90, 245)
end
