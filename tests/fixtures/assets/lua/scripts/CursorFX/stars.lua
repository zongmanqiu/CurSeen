-- CursorFX · stars
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：金色五角星沿指针路径弹出，先放大、再自转、闪烁并淡出。五角星用
-- 10 顶点星形多边形（外/内半径交替，内径 0.45）绘制。

name = "stars"
effect_name = "stars"
kind = "trail"
description = "CursorFX stars：旋转闪烁的五角星拖尾。"

local MAX_PARTICLES = 220
local TAU = math.pi * 2
local PALETTE = { "#FFD700", "#FFF1A8", "#FFFFFF", "#FFB347" }

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

local star_pts = {}
for i = 1, 10 do
  star_pts[i] = { 0, 0 }
end

function schema()
  return {
    { key = "rate",  label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1,  default = 1.0 },
    { key = "size",  label = "五角星大小", type = "float", min = 2,  max = 24,  step = 0.5,  default = 9 },
    { key = "life",  label = "最长寿命(秒)", type = "float", min = 0.4, max = 1.8, step = 0.01, default = 1.17 },
    { key = "color", label = "主色",     type = "color", alpha = true, default = "#FFD700" },
    { key = "multi", label = "金色渐变", type = "bool",  default = true },
  }
end

local function add_particle(x, y, params)
  local max_life = params.life or 1.17
  local col = params.color or "#FFD700"
  if params.multi ~= false then
    col = PALETTE[rng_int(1, #PALETTE)]
  end
  local cr, cg, cb, ca = hex_to_rgb(col)

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  particles[#particles + 1] = {
    x = x + rng(-6, 6),
    y = y + rng(-6, 6),
    vx = rng(-0.4, 0.4) * 60,
    vy = rng(-0.4, 0.4) * 60,
    rot = rng(0, TAU),
    spin = rng(-0.08, 0.08) * 60,
    size = rng(0.6, 1.3) * (params.size or 9),
    life = max_life * rng(0.57, 1.0),
    max = max_life,
    r = cr,
    g = cg,
    b = cb,
    a = ca / 255,
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
    -- 原版每帧 (0.06 + speed*0.18)。
    acc = acc + dist * (0.18 + 0.06 / speed) * (params.rate or 1)
  end
  last_x, last_y = x, y

  local budget = 6
  while acc >= 1 and budget > 0 do
    acc = acc - 1
    budget = budget - 1
    add_particle(x, y, params)
  end
  if acc > 8 then acc = 8 end
end

function step(dt, t, params)
  if seen and (mouse_speed or 0) < 1.5 then
    idle_acc = idle_acc + 0.06 * (params.rate or 1) * dt * 60
    local budget = 3
    while idle_acc >= 1 and budget > 0 do
      idle_acc = idle_acc - 1
      budget = budget - 1
      add_particle(mouse_x or 0, mouse_y or 0, params)
    end
  end

  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.rot = p.rot + p.spin * dt
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then return end
  local now = elapsed or 0

  for i = 1, #particles do
    local p = particles[i]
    local t = clamp(p.life / p.max, 0, 1)
    local grow = 1
    if t > 0.85 then grow = (1 - t) / 0.15 end
    local alpha = math.min(1, t * 2) * (0.7 + 0.3 * math.sin(now * 10 + p.rot * 5)) * p.a
    local a8 = math.floor(clamp(alpha, 0, 1) * 255)
    local rad = p.size * grow * (0.5 + t * 0.5)
    if a8 > 0 and rad > 0.2 then
      local inner = rad * 0.45
      for k = 0, 9 do
        local ang = k * (TAU / 10) + p.rot - math.pi / 2
        local rr = rad
        if k % 2 == 1 then rr = inner end
        local pt = star_pts[k + 1]
        pt[1] = p.x + math.cos(ang) * rr
        pt[2] = p.y + math.sin(ang) * rr
      end
      ctx:polygon(star_pts, p.r, p.g, p.b, a8)
    end
  end
end
