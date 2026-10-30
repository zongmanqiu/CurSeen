-- CursorFX · sparkles
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：指针移动时洒出四角星尘埃，星星缓慢下落、旋转并闪烁。四角星用 8 顶点
-- 星形多边形绘制（外/内半径交替），不用图片或文字旋转。

name = "sparkles"
effect_name = "sparkles"
kind = "trail"
description = "CursorFX sparkles：闪烁的四角星尘埃拖尾。"

local MAX_PARTICLES = 220
local TAU = math.pi * 2
local PALETTE = { "#FF7CE5", "#FFD166", "#7CF5FF", "#C3FF7C", "#FFFFFF" }

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

-- 复用的 8 顶点缓冲，避免每帧给每颗粒子新建表。
local star_pts = {}
for i = 1, 8 do
  star_pts[i] = { 0, 0 }
end

function schema()
  return {
    { key = "rate",  label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1,  default = 1.0 },
    { key = "size",  label = "星芒大小", type = "float", min = 2,   max = 20,  step = 0.5,  default = 7 },
    { key = "life",  label = "最长寿命(秒)", type = "float", min = 0.4, max = 1.8, step = 0.05, default = 1.15 },
    { key = "color", label = "主色",     type = "color", alpha = true, default = "#FFD166" },
    { key = "multi", label = "五彩",     type = "bool",  default = true },
  }
end

local function add_particle(x, y, params)
  local max_life = params.life or 1.15
  local col = params.color or "#FFD166"
  if params.multi ~= false then
    col = PALETTE[rng_int(1, #PALETTE)]
  end
  local cr, cg, cb, ca = hex_to_rgb(col)
  local size = rng(0.5, 1.2) * (params.size or 7)

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  particles[#particles + 1] = {
    x = x + rng(-4, 4),
    y = y + rng(-4, 4),
    vx = (rng(-0.8, 0.8) - (mouse_vx or 0) * 0.1) * 60,
    vy = rng(-0.6, 0.4) * 60,
    rot = rng(0, TAU),
    spin = rng(-0.1, 0.1) * 60,
    size = size,
    life = max_life * rng(0.5, 1.0),
    max = max_life,
    tw = rng(0, TAU),
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
    local rate = params.rate or 1
    local speed = math.max(mouse_speed or 0, 0.5)
    -- 原版每帧生成 (0.12 + speed*0.25)；按移动距离折算成每像素密度。
    acc = acc + dist * (0.25 + 0.12 / speed) * rate
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
  -- 指针静止时保留原版的少量“自洒”感觉；移动时的量由 on_move 按距离给。
  if seen and (mouse_speed or 0) < 1.5 then
    idle_acc = idle_acc + 0.12 * (params.rate or 1) * dt * 60
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
      p.vy = p.vy + 108 * dt -- 0.03 px/帧² → 108 px/s²
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
    local tw = 0.6 + 0.4 * math.sin(now * 12 + p.tw)
    local alpha = math.min(1, t * 1.5) * tw * p.a
    local a8 = math.floor(clamp(alpha, 0, 1) * 255)
    if a8 > 0 then
      local rad = p.size * (0.4 + t * 0.6)
      local inner = rad * 0.35
      for k = 0, 7 do
        local ang = k * (TAU / 8) + p.rot - math.pi / 2
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
