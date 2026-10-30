-- CursorX · Wind Stream
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：移动越快越密的彩色风丝，速度衰减后弯曲飘散；指针停在交互提示上时
-- 静止也会起微风、风丝更粗；点击炸出全色域的风爆。风丝用带状多边形绘制（一条指令），
-- 比逐段折线省一个量级的绘制指令。

name = "Wind Stream"
effect_name = "Wind Stream"
kind = "cursor"
description = "CursorX Wind Stream：随移动甩出的彩色风丝，点击风爆。"
hide_cursor = true

local MAX_STREAMS = 80
local MAX_POINTS = 20
local streams = {}
local frame = 0
local last_x, last_y = 0, 0

function schema()
  return {
    { key = "hue",             label = "风丝色相", type = "int",   min = 0,   max = 360, step = 5,    default = 200 },
    { key = "count",           label = "每段风丝", type = "int",   min = 1,   max = 5,   step = 1,    default = 2 },
    { key = "decay",           label = "速度衰减", type = "float", min = 0.8, max = 0.98,step = 0.01, default = 0.93 },
    { key = "minSpeed",        label = "触发速度", type = "float", min = 1,   max = 10,  step = 0.5,  default = 2.5 },
    { key = "pointerAnim",     label = "悬停微风", type = "bool",  default = true },
    { key = "pointerThickness",label = "悬停线宽", type = "float", min = 3.0, max = 12.0,step = 0.5,  default = 6.5 },
    { key = "clickAnim",       label = "点击风爆", type = "bool",  default = true },
  }
end

function init()
  streams = {}
  frame = 0
  last_x, last_y = mouse_x or 0, mouse_y or 0
end

local function new_stream(x, y, vx, vy, hue)
  if #streams >= MAX_STREAMS then table.remove(streams, 1) end
  streams[#streams + 1] = {
    pts = { { x = x, y = y } },
    vx = vx, vy = vy,
    alpha = 0.9,
    hue = hue,
  }
end

function on_move(x, y, t, params)
  params = params or {}
  local dx, dy = x - last_x, y - last_y
  last_x, last_y = x, y
  local spd = math.sqrt(dx * dx + dy * dy)
  if spd <= (params.minSpeed or 2.5) then return end
  local n = math.floor(params.count or 2) * ((cursor_hover and params.pointerAnim ~= false) and 2 or 1)
  for _ = 1, n do
    new_stream(x, y,
      dx * 0.28 + rng(-1.75, 1.75),
      dy * 0.28 + rng(-1.75, 1.75),
      (params.hue or 200) + rng(-22, 22))
  end
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  for i = 0, 17 do
    local a = i / 18 * math.pi * 2 + rng(-0.1, 0.1)
    local spd = rng(3.0, 8.0)
    new_stream(x, y, math.cos(a) * spd, math.sin(a) * spd, rng(0, 360))
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  frame = frame + 1

  local hover = cursor_hover and params.pointerAnim ~= false
  if hover and frame % 4 == 0 then
    local n = math.floor(params.count or 2)
    for _ = 1, n do
      new_stream(mouse_x or 0, mouse_y or 0,
        rng(-1.9, 1.9), rng(-1.9, 1.9) - 1.2,
        (params.hue or 200) + rng(-22, 22))
    end
  end

  local decay = params.decay or 0.93
  for i = #streams, 1, -1 do
    local s = streams[i]
    s.vx = s.vx * math.pow(decay, f)
    s.vy = s.vy * math.pow(decay, f) + 0.045 * f
    local last = s.pts[#s.pts]
    if #s.pts >= MAX_POINTS then table.remove(s.pts, 1) end
    s.pts[#s.pts + 1] = { x = last.x + s.vx * f, y = last.y + s.vy * f }
    s.alpha = s.alpha * math.pow(0.952, f)
    if s.alpha <= 0.02 then table.remove(streams, i) end
  end
end

local function stream_polygon(s, width)
  local pts = s.pts
  local n = #pts
  if n < 2 then return nil end
  local left, right = {}, {}
  for i = 1, n do
    local p = pts[i]
    local a = pts[math.max(1, i - 1)]
    local b = pts[math.min(n, i + 1)]
    local dx, dy = b.x - a.x, b.y - a.y
    local len = math.sqrt(dx * dx + dy * dy)
    if len < 1e-3 then dx, dy, len = 1, 0, 1 end
    local nx, ny = -dy / len, dx / len
    local t = i / n
    local w = width * (0.45 + 0.55 * t) * 0.5
    left[i] = { p.x + nx * w, p.y + ny * w }
    right[n - i + 1] = { p.x - nx * w, p.y - ny * w }
  end
  local poly = {}
  for i = 1, n do poly[#poly + 1] = left[i] end
  for i = 1, n do poly[#poly + 1] = right[i] end
  return poly
end

function draw(ctx, params)
  params = params or {}
  local hover = cursor_hover and params.pointerAnim ~= false
  local thick = hover and (params.pointerThickness or 6.5) or 3.5
  for i = 1, #streams do
    local s = streams[i]
    if #s.pts >= 2 then
      local r, g, b = hsv_to_rgb(s.hue % 360, 0.8, 0.7)
      local poly = stream_polygon(s, math.max(0.6, s.alpha * thick))
      if poly then
        ctx:polygon(poly, r, g, b, math.floor(255 * clamp(s.alpha, 0, 1)))
      end
    end
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  ctx:circle(mx, my, 3.0, 0, 0, 0, 130)
  ctx:circle(mx, my, 2.0, 255, 255, 255, 235)
end
