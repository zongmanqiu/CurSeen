-- CursorX · Particle Trail
-- 复刻自 CursorX — Interactive Cursor Effects（https://github.com/Felix-au/CursorX-Interactive-Cursor-Effects），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：指针移动时撒出闪烁粒子（色相围绕基准值抖动、带重力与收缩），
-- 点击炸开一团全色域粒子。指针停在交互提示上时，静止也会持续冒出小粒子。

name = "Particle Trail"
effect_name = "Particle Trail"
kind = "cursor"
description = "CursorX Particle Trail：闪烁粒子拖尾，点击炸开全色域粒子。"
hide_cursor = true

local MAX_PARTICLES = 380
local particles = {}
local idle_frame = 0

function schema()
  return {
    { key = "hue",        label = "基准色相", type = "int",   min = 0,   max = 360, step = 5,     default = 260 },
    { key = "count",      label = "每帧粒子", type = "int",   min = 1,   max = 10,  step = 1,     default = 4 },
    { key = "maxSize",    label = "最大尺寸", type = "float", min = 2,   max = 14,  step = 0.5,   default = 6 },
    { key = "fadeSpeed",  label = "消散速度", type = "float", min = 0.01,max = 0.08,step = 0.001, default = 0.022 },
    { key = "pointerAnim",label = "悬停冒粒子", type = "bool", default = true },
    { key = "clickAnim",  label = "点击爆发", type = "bool",  default = true },
    { key = "burstCount", label = "爆发数量", type = "int",   min = 15,  max = 80,  step = 5,     default = 45 },
  }
end

function init()
  particles = {}
  idle_frame = 0
end

local function spawn(x, y, params, burst)
  if #particles >= MAX_PARTICLES then return end
  local hue = params.hue or 260
  local max_size = params.maxSize or 6
  local a = rng(0, math.pi * 2)
  local spd = burst and rng(1.5, 6.5) or 0
  particles[#particles + 1] = {
    x = x, y = y,
    vx = burst and math.cos(a) * spd or rng(-1.25, 1.25),
    vy = burst and math.sin(a) * spd or (rng(-1.25, 1.25) - 0.8),
    life = 1.0,
    size = rng(1.0, 1.0 + max_size),
    hue = burst and rng(0, 360) or (hue + rng(-30, 30)),
    gravity = burst and 0.05 or 0.06,
    decay = burst and 0.018 or (params.fadeSpeed or 0.022),
    burst = burst,
  }
end

function on_move(x, y, t, params)
  local n = math.floor(params.count or 4)
  for _ = 1, n do spawn(x, y, params, false) end
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" or params.clickAnim == false then return end
  local n = math.floor(params.burstCount or 45)
  for _ = 1, n do spawn(x, y, params, true) end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or 0, mouse_y or 0

  if cursor_hover and params.pointerAnim ~= false then
    idle_frame = idle_frame + 1
    if idle_frame % 3 == 0 then spawn(mx, my, params, false) end
  else
    idle_frame = 0
  end

  for i = #particles, 1, -1 do
    local p = particles[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    p.vy = p.vy + p.gravity * f
    p.vx = p.vx * math.pow(0.97, f)
    p.life = p.life - p.decay * f
    p.size = p.size * math.pow(0.97, f)
    if p.life <= 0 then table.remove(particles, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  for i = 1, #particles do
    local p = particles[i]
    local r, g, b = hsv_to_rgb(p.hue % 360, 1.0, 0.72)
    local a = math.floor(255 * clamp(p.life, 0, 1))
    local size = math.max(0.4, p.size)
    ctx:circle(p.x, p.y, size, r, g, b, a)
    if not p.burst and size > 2.0 then
      ctx:plus_circle(p.x, p.y, size * 1.7, r, g, b, math.floor(a * 0.22))
    end
  end

  -- 粒子本身不提供稳定指针位置；给一个带暗边的小点（浅色背景也能找到）。
  local mx, my = mouse_x or 0, mouse_y or 0
  ctx:circle(mx, my, 3.0, 0, 0, 0, 140)
  ctx:circle(mx, my, 2.0, 255, 255, 255, 235)
end
