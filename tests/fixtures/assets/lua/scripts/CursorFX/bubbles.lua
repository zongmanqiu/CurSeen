-- CursorFX · bubbles
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：肥皂泡从指针向上飘，左右摇摆、半径缓慢变大，寿命末段“啵”地炸开。
-- 泡体用半透明填充圆 + 白色圆环 + 左上角高光近似，不依赖贴图。

name = "bubbles"
effect_name = "bubbles"
kind = "trail"
description = "CursorFX bubbles：上浮、摇摆并破裂的肥皂泡拖尾。"

local MAX_PARTICLES = 200
local TAU = math.pi * 2

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

function schema()
  return {
    { key = "rate",    label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1,  default = 1.0 },
    { key = "size",    label = "气泡大小", type = "float", min = 2,   max = 16,  step = 0.5,  default = 6 },
    { key = "life",    label = "最长寿命(秒)", type = "float", min = 0.4, max = 4.0, step = 0.1,  default = 2.0 },
    { key = "color",   label = "气泡色",   type = "color", default = "#78C8FF" },
    { key = "opacity", label = "不透明度", type = "float", min = 0.1, max = 1.0, step = 0.05, default = 1.0 },
  }
end

local function add_bubble(x, y, params)
  local max_life = params.life or 2.0
  local scale = (params.size or 6) / 6

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  particles[#particles + 1] = {
    x = x,
    y = y,
    vx = rng(-0.5, 0.5) * 60,
    vy = rng(-1.4, -0.6) * 60,
    r = math.max(2.5, rng(3, 9) * scale),
    life = max_life * rng(0.5, 1.0),
    max = max_life,
    ph = rng(0, TAU),
  }
end

function init()
  particles = {}
  acc = 0
  idle_acc = 0
  last_x, last_y = nil, nil
  seen = false
end

function on_move(x, y, t, params)
  if not seen then
    seen = true
    acc = acc + 1
  end

  if last_x then
    local dx, dy = x - last_x, y - last_y
    local dist = math.sqrt(dx * dx + dy * dy)
    local speed = math.max(mouse_speed or 0, 0.5)
    -- 原版每帧 (0.05 + speed*0.08)，按距离折算。
    acc = acc + dist * (0.08 + 0.05 / speed) * (params.rate or 1)
  end
  last_x, last_y = x, y

  local budget = 5
  while acc >= 1 and budget > 0 do
    acc = acc - 1
    budget = budget - 1
    add_bubble(x, y, params)
  end
  if acc > 6 then acc = 6 end
end

function step(dt, t, params)
  if seen and (mouse_speed or 0) < 1.5 then
    idle_acc = idle_acc + 0.05 * (params.rate or 1) * dt * 60
    local budget = 3
    while idle_acc >= 1 and budget > 0 do
      idle_acc = idle_acc - 1
      budget = budget - 1
      add_bubble(mouse_x or 0, mouse_y or 0, params)
    end
  end

  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + (p.vx + math.sin((t or 0) * 3 + p.ph) * 24) * dt
      p.y = p.y + p.vy * dt
      p.r = p.r + 1.2 * dt
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then return end

  local cr, cg, cb, ca = hex_to_rgb(params.color or "#78C8FF")
  local op = params.opacity or 1.0

  for i = 1, #particles do
    local p = particles[i]
    local t = clamp(p.life / p.max, 0, 1)
    local a = math.min(1, t * 4)

    if t < 0.08 then
      -- 破裂：一圈迅速扩张的白环。
      local a8 = math.floor(clamp(t * 8, 0, 1) * op * 255 * ca / 255)
      if a8 > 0 then
        ctx:ring(p.x, p.y, p.r * (1 + (0.08 - t) * 10), 1, 255, 255, 255, a8)
      end
    else
      local fill_a = math.floor(0.18 * a * op * 255 * ca / 255)
      local stroke_a = math.floor(0.7 * a * op * 255 * ca / 255)
      local hi_a = math.floor(0.8 * a * op * 255 * ca / 255)
      if fill_a > 0 then
        ctx:circle(p.x, p.y, p.r, cr, cg, cb, fill_a)
      end
      if stroke_a > 0 then
        ctx:ring(p.x, p.y, p.r, 1, 255, 255, 255, stroke_a)
      end
      if hi_a > 0 then
        ctx:circle(p.x - p.r * 0.35, p.y - p.r * 0.35,
                   math.max(0.5, p.r * 0.2), 255, 255, 255, hi_a)
      end
    end
  end
end
