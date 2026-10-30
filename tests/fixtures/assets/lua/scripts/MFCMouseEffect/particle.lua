-- MFCMouseEffect · Particle
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 彩虹粒子：移动时按位移喷出小粒子（随机方向、阻力、寿命、尺寸），粒子受轻微
-- 重力并随寿命缩小淡出；可切换彩虹色。上游粒子发射数 = clamp(距离×0.18+2, 2, 12)。

name = "Particle"
effect_name = "Particle"
kind = "trail"
description = "MFCMouseEffect Particle：移动喷出的彩虹粒子拖尾。"

local MAX_PARTICLES = 300
local particles = {}
local hue = 0
local last_x, last_y = nil, nil

function schema()
  return {
    { key = "color",     label = "粒子颜色", type = "color", alpha = true, default = "#64FFDADC" },
    { key = "rainbow",   label = "彩虹色",   type = "bool",  default = false },
    { key = "size",      label = "粒子大小", type = "float", min = 3, max = 14, step = 0.5, default = 14 },
    { key = "rate",      label = "发射倍率", type = "float", min = 0.2, max = 3, step = 0.1, default = 1.0 },
    { key = "life",      label = "寿命(秒)", type = "float", min = 0.2, max = 2.0, step = 0.05, default = 0.9 },
  }
end

function init()
  particles = {}
  hue = 0
  last_x, last_y = nil, nil
end

local function spawn(x, y, params, speed)
  if #particles >= MAX_PARTICLES then return end
  local a = rng(0, math.pi * 2)
  local sp = rng(0.5, 3.8) * clamp(0.75 + speed / 24.0 * 0.6, 0.6, 1.8)
  local size = rng(2.0, 5.9) * clamp((params.size or 14) / 6.0, 0.45, 2.4)
  particles[#particles + 1] = {
    x = x, y = y,
    vx = math.cos(a) * sp,
    vy = math.sin(a) * sp,
    life = 1.0,
    decay = 1.0 / math.max(0.2, params.life or 0.9),
    size = size,
    hue = params.rainbow and rng(0, 360) or nil,
  }
end

function on_move(x, y, t, params)
  local dx, dy = 0, 0
  if last_x then dx, dy = x - last_x, y - last_y end
  last_x, last_y = x, y
  local dist = math.sqrt(dx * dx + dy * dy)
  if dist <= 0 then return end
  local n = math.floor(math.min(12, math.max(2, dist * 0.18 + 2)) *
    (params.rate or 1.0))
  if n < 1 then n = 1 end
  for _ = 1, n do spawn(x, y, params, dist) end
end

function step(dt, t, params)
  local f = math.min(2.0, dt * 60.0)
  hue = (hue + 5.0 * f) % 360
  for i = #particles, 1, -1 do
    local p = particles[i]
    p.x = p.x + p.vx * f
    p.y = p.y + p.vy * f
    p.vy = p.vy + 0.05 * f
    p.life = p.life - p.decay * dt
    if p.life <= 0 then table.remove(particles, i) end
  end
end

function draw(ctx, params)
  local r, g, b, a = hex_to_rgb(params.color or "#64FFDADC")
  for i = 1, #particles do
    local p = particles[i]
    local life = clamp(p.life, 0, 1)
    local radius = math.max(0.3, p.size * life * 0.5)
    local cr, cg, cb
    if p.hue then
      cr, cg, cb = hsv_to_rgb(p.hue, 0.8, 0.6)
    else
      cr, cg, cb = r, g, b
    end
    local alpha = math.floor(a * life)
    ctx:circle(p.x, p.y, radius, cr, cg, cb, alpha)
    -- 上游粒子无外圈光晕层，旧版多加的 plus_circle 已删。
  end
end
