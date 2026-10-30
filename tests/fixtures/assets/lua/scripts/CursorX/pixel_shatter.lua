-- CursorX · Pixel Shatter
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：移动时按位移量炸出旋转小方块，方块带重力下落并逐渐缩小；点击炸出
-- 一圈全色域碎片。指针停在交互提示上时静止也会持续掉落碎片。

name = "Pixel Shatter"
effect_name = "Pixel Shatter"
kind = "cursor"
description = "CursorX Pixel Shatter：移动迸出旋转像素碎片，点击爆散。"
hide_cursor = true

local MAX_SHARDS = 340
local shards = {}
local idle_frame = 0
local last_x, last_y

function schema()
  return {
    { key = "hue",        label = "基准色相", type = "int",   min = 0,   max = 360, step = 5,    default = 220 },
    { key = "count",      label = "每帧上限", type = "int",   min = 1,   max = 15,  step = 1,    default = 8 },
    { key = "gravity",    label = "重力",     type = "float", min = 0,   max = 0.5, step = 0.01, default = 0.18 },
    { key = "pointerAnim",label = "悬停掉落", type = "bool",  default = true },
    { key = "clickAnim",  label = "点击爆发", type = "bool",  default = true },
    { key = "burstCount", label = "爆发数量", type = "int",   min = 15,  max = 60,  step = 5,    default = 30 },
  }
end

local function spawn(x, y, params, burst)
  if #shards >= MAX_SHARDS then return end
  local a = rng(0, math.pi * 2)
  local spd = burst and rng(2, 9) or rng(2, 8)
  local hue = burst and rng(0, 360) or ((params.hue or 220) + rng(30, 90))
  shards[#shards + 1] = {
    x = x, y = y,
    vx = math.cos(a) * spd,
    vy = math.sin(a) * spd,
    size = rng(2, 8),
    rot = rng(0, math.pi * 2),
    spin = rng(-0.125, 0.125),
    life = 1.0,
    decay = burst and 0.016 or 0.028,
    hue = hue % 360,
    burst = burst,
  }
end

function init()
  shards = {}
  idle_frame = 0
  last_x, last_y = mouse_x or 0, mouse_y or 0
end

function on_move(x, y, t, params)
  local dx, dy = x - last_x, y - last_y
  local dist = math.sqrt(dx * dx + dy * dy)
  local n = math.min(math.floor(dist * 0.6), math.floor(params.count or 8))
  for _ = 1, n do spawn(x, y, params, false) end
  last_x, last_y = x, y
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  local n = math.floor(params.burstCount or 30)
  for _ = 1, n do spawn(x, y, params, true) end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)

  if cursor_hover and params.pointerAnim ~= false then
    idle_frame = idle_frame + 1
    if idle_frame % 4 == 0 then spawn(mouse_x or 0, mouse_y or 0, params, false) end
  else
    idle_frame = 0
  end

  local gravity = params.gravity or 0.18
  for i = #shards, 1, -1 do
    local s = shards[i]
    s.x = s.x + s.vx * f
    s.y = s.y + s.vy * f
    s.vy = s.vy + gravity * f
    s.vx = s.vx * math.pow(0.97, f)
    s.rot = s.rot + s.spin * f
    s.life = s.life - s.decay * f
    s.size = s.size * math.pow(0.975, f)
    if s.life <= 0 then table.remove(shards, i) end
  end
end

local function square(cx, cy, size, rot)
  local h = size * 0.5
  local ca, sa = math.cos(rot), math.sin(rot)
  local pts = {}
  local corners = { { -h, -h }, { h, -h }, { h, h }, { -h, h } }
  for i = 1, 4 do
    local x, y = corners[i][1], corners[i][2]
    pts[i] = { cx + x * ca - y * sa, cy + x * sa + y * ca }
  end
  return pts
end

function draw(ctx, params)
  for i = 1, #shards do
    local s = shards[i]
    local r, g, b = hsv_to_rgb(s.hue, 0.8, 0.7)
    ctx:polygon(square(s.x, s.y, s.size, s.rot), r, g, b,
      math.floor(255 * clamp(s.life, 0, 1)))
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  ctx:circle(mx, my, 3.0, 0, 0, 0, 140)
  ctx:circle(mx, my, 2.0, 255, 255, 255, 235)
end
