-- CursorFX · snow
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：雪花字符从指针缓缓飘落，横向正弦摇摆，越落越快一点点。CurSeen 没有
-- 文字旋转接口，这里用 text_centered 正立绘制雪花字符，不做逐粒自转。

name = "snow"
effect_name = "snow"
kind = "trail"
description = "CursorFX snow：正立雪花字符缓缓飘落。"

local MAX_PARTICLES = 200
local FLAKES = { "❄", "❅", "❆" }

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

function schema()
  return {
    { key = "rate",  label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1,  default = 1.0 },
    { key = "size",  label = "雪花字号", type = "float", min = 8,   max = 36,  step = 1,    default = 16 },
    { key = "life",  label = "最长寿命(秒)", type = "float", min = 1.0, max = 5.0, step = 0.1,  default = 2.5 },
    { key = "color", label = "雪花色",   type = "color", alpha = true, default = "#EAF8FF" },
  }
end

local function add_particle(x, y, params)
  local max_life = params.life or 2.5
  local scale = rng(0.7, 1.2)

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  particles[#particles + 1] = {
    x = x + rng(-3, 3),
    y = y,
    vx = (rng(-0.6, 0.6) + (mouse_vx or 0) * 0.15) * 60,
    vy = rng(0.4, 1.2) * 60,
    size = (params.size or 16) * scale,
    life = max_life * rng(0.6, 1.0),
    max = max_life,
    ch = FLAKES[rng_int(1, #FLAKES)],
    ph = rng(0, 6.2831853),
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
    -- 原版每帧 (0.05 + speed*0.05)。
    acc = acc + dist * (0.05 + 0.05 / speed) * (params.rate or 1)
  end
  last_x, last_y = x, y

  local budget = 4
  while acc >= 1 and budget > 0 do
    acc = acc - 1
    budget = budget - 1
    add_particle(x, y, params)
  end
  if acc > 6 then acc = 6 end
end

function step(dt, t, params)
  if seen and (mouse_speed or 0) < 1.5 then
    idle_acc = idle_acc + 0.05 * (params.rate or 1) * dt * 60
    local budget = 2
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
      p.vy = p.vy + 14.4 * dt -- 0.004 px/帧² → 14.4 px/s²
      p.x = p.x + (p.vx + math.sin((t or 0) * 2 + p.ph) * 30) * dt
      p.y = p.y + p.vy * dt
      p.vx = p.vx * (0.99 ^ (dt * 60))
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then return end

  local cr, cg, cb, ca = hex_to_rgb(params.color or "#EAF8FF")
  local base_a = ca / 255

  for i = 1, #particles do
    local p = particles[i]
    local t = clamp(p.life / p.max, 0, 1)
    local a8 = math.floor(clamp(math.min(1, t * 3) * base_a, 0, 1) * 255)
    if a8 > 0 then
      ctx:text_centered(p.x, p.y, p.ch, p.size, cr, cg, cb, a8)
    end
  end
end
