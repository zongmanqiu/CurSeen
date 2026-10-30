-- CursorFX · petals
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：樱花花瓣从指针飘落，带轻微重力和横向摇曳。原版用 emoji 贴图；CurSeen
-- 这里改用 20 顶点花瓣多边形（5 个圆瓣的极坐标轮廓）+ 花心，避免字体差异，
-- 也不做逐粒旋转。

name = "petals"
effect_name = "petals"
kind = "trail"
description = "CursorFX petals：多边形樱花花瓣飘落拖尾。"

local TAU = math.pi * 2
local MAX_PARTICLES = 200
local LOBES = 5
local FLOWER_SEGMENTS = 20
local CENTER_R, CENTER_G, CENTER_B = 255, 224, 138

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

-- 复用的多边形顶点缓冲。
local flower_pts = {}
for i = 1, FLOWER_SEGMENTS do
  flower_pts[i] = { 0, 0 }
end

function schema()
  return {
    { key = "rate",  label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1,  default = 1.0 },
    { key = "size",  label = "花瓣大小", type = "float", min = 8,   max = 36,  step = 1,    default = 16 },
    { key = "life",  label = "最长寿命(秒)", type = "float", min = 1.0, max = 5.0, step = 0.01, default = 2.33 },
    { key = "color", label = "花瓣色",   type = "color", alpha = true, default = "#FFB7C5" },
  }
end

local function add_particle(x, y, params)
  local max_life = params.life or 2.33
  local scale = rng(0.7, 1.15)

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  particles[#particles + 1] = {
    x = x + rng(-4, 4),
    y = y,
    vx = (rng(-1.0, 1.0) + (mouse_vx or 0) * 0.15) * 60,
    vy = rng(0.3, 1.0) * 60,
    rad = (params.size or 16) * 0.7 * scale,
    life = max_life * rng(0.57, 1.0),
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
    -- 原版每帧 (0.04 + speed*0.05)。
    acc = acc + dist * (0.05 + 0.04 / speed) * (params.rate or 1)
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
    idle_acc = idle_acc + 0.04 * (params.rate or 1) * dt * 60
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
      p.vy = p.vy + 36 * dt -- 0.01 px/帧² → 36 px/s²
      p.x = p.x + (p.vx + math.sin((t or 0) * 2 + p.ph) * 42) * dt
      p.y = p.y + p.vy * dt
      p.vx = p.vx * (0.98 ^ (dt * 60))
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then return end

  local fr, fg, fb, fa = hex_to_rgb(params.color or "#FFB7C5")
  local base_a = fa / 255

  for i = 1, #particles do
    local p = particles[i]
    local t = clamp(p.life / p.max, 0, 1)
    local a8 = math.floor(clamp(math.min(1, t * 3) * base_a, 0, 1) * 255)
    if a8 > 0 then
      local rad = p.rad
      for k = 0, FLOWER_SEGMENTS - 1 do
        local ang = k * (TAU / FLOWER_SEGMENTS)
        -- 极坐标 5 瓣轮廓：半径在 0.24r..1r 之间起伏，形成圆花瓣。
        local rr = rad * (0.62 + 0.38 * math.cos(LOBES * ang))
        local pt = flower_pts[k + 1]
        pt[1] = p.x + math.cos(ang) * rr
        pt[2] = p.y + math.sin(ang) * rr
      end
      ctx:polygon(flower_pts, fr, fg, fb, a8)
      ctx:circle(p.x, p.y, math.max(0.5, rad * 0.22),
                 CENTER_R, CENTER_G, CENTER_B, math.floor(a8 * 0.9))
    end
  end
end
