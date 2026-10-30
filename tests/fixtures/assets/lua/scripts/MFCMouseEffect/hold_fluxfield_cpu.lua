-- MFCMouseEffect · FluxField HUD CPU
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 磁通场 HUD：6 层脉动同心环 + 3 组分段旋转弧 + 84 个轨道粒子（一半带中心连线）
-- + 中心光晕。按住 260ms 后出现，进度推进时半径扩大。为 CPU 软渲染做的裁剪：
-- 分段弧 3 带（8/10/12 段）共 30 段，上游 4 带（26/34/42/50）共 152 段（约 1/5，
-- 且缺最外带）；轨道粒子 84 个与上游相同未减（见 README）。

name = "FluxField HUD CPU"
effect_name = "FluxField HUD CPU"
kind = "hold"
description = "MFCMouseEffect FluxField HUD：按住时的磁通场环阵。"

local HOLD_DELAY = 260
local PARTICLES = 84
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0
local particles = {}

function schema()
  return {
    { key = "color",    label = "场颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "size",     label = "最大半径", type = "float", min = 30, max = 140, step = 2,   default = 70 },
    { key = "width",    label = "核心宽度", type = "float", min = 0.5,max = 6,   step = 0.1, default = 2.8 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
  }
end

local function make_particles(base)
  particles = {}
  for i = 1, PARTICLES do
    particles[i] = {
      radius = base * (0.18 + 0.78 * (((i * 37) % 100) / 100.0)),
      angle = (i * 29) * math.pi / 180.0,
      speed = 0.45 + 0.75 * (((i * 17) % 100) / 100.0),
      pulse = ((i * 11) % 100) / 100.0,
    }
  end
end

function init()
  press_at = nil
  vis = 0
  progress = 0
  hx, hy = mouse_x or 0, mouse_y or 0
  make_particles(70)
end

function on_mouse_down(button, x, y, params)
  press_at = wall_ms or 0
  hx, hy = x, y
end

function on_mouse_up(button, x, y, params)
  press_at = nil
end

function step(dt, t, params)
  local now = wall_ms or 0
  local k = math.min(1.0, (params.follow or 0.25) * dt * 60.0)
  hx = hx + ((mouse_x or hx) - hx) * k
  hy = hy + ((mouse_y or hy) - hy) * k
  if press_at and mouse_down then
    local held = now - press_at
    if held >= HOLD_DELAY then
      progress = clamp(held / math.max(300, params.duration or 900), 0, 1)
      vis = vis + (1 - vis) * math.min(1.0, dt * 10.0)
    end
  else
    vis = vis - vis * math.min(1.0, dt * 8.0)
    if vis < 0.01 then vis = 0 end
  end
end

local function arc(ctx, radius, start_deg, sweep_deg, width, r, g, b, a, steps)
  if sweep_deg <= 0.01 then return end
  steps = steps or 4
  local prev_x, prev_y
  for i = 0, steps do
    local ang = math.rad(start_deg + sweep_deg * i / steps)
    local x = hx + math.cos(ang) * radius
    local y = hy + math.sin(ang) * radius
    if i > 0 then ctx:line(prev_x, prev_y, x, y, width, r, g, b, a) end
    prev_x, prev_y = x, y
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local base = params.size or 70
  local width = params.width or 2.8
  local seconds = (wall_ms or 0) / 1000.0
  local radius = 12 + (base - 12) * progress

  -- Layer 1: 6 pulsing rings.
  for ring = 0, 5 do
    local frac = (ring + 1) / 6.0
    local rr = radius * (0.28 + frac * 0.78)
    local pulse = 0.55 + 0.45 * math.sin(seconds * (1.6 + frac * 0.7) + frac * 5.0)
    local alpha = math.floor(ca * (0.08 + 0.11 * frac) * pulse * vis)
    ctx:ring(hx, hy, rr, 1.3 + frac * 2.4, r, g, b, alpha)
  end

  -- Layer 2: 3 segmented arc bands (upstream 4 bands; reduced for CPU).
  for band = 0, 2 do
    local frac = (band + 1) / 4.0
    local arc_r = radius * (0.34 + frac * 0.62)
    local spin = seconds * (30.0 + 18.0 * frac) * ((band % 2 == 1) and -1 or 1)
    local segs = 8 + band * 2
    local sweep = 360.0 / segs
    for s = 0, segs - 1 do
      local phase = seconds * 2.4 + s * 0.45
      local alpha = math.floor(ca * (0.16 + 0.18 * math.abs(math.sin(phase))) * vis)
      arc(ctx, arc_r, spin + s * sweep, sweep * 0.62,
        1.2 + frac * 1.8, r, g, b, alpha, 3)
    end
  end

  -- Layer 3: orbit particles + center lines.
  local dot_alpha_base = ca * vis / 255.0
  for i = 1, #particles do
    local p = particles[i]
    local wobble = math.sin(seconds * 1.8 + p.pulse * 9.0) * radius * 0.045
    local pr = p.radius * (0.72 + progress * 0.42) + wobble
    local a = p.angle + seconds * p.speed * ((i % 2 == 1) and 1 or -1)
    local x = hx + math.cos(a) * pr
    local y = hy + math.sin(a * 1.15) * pr * 0.84
    local alpha = math.floor(255 * dot_alpha_base *
      (0.36 + 0.44 * (0.5 + 0.5 * math.sin(a + seconds))))
    local dot_r = 1.3 + 2.4 * (0.5 + 0.5 * math.sin(seconds * 2.0 + p.pulse * 7.0))
    ctx:circle(x, y, dot_r, r, g, b, alpha)
    if i % 2 == 0 then
      ctx:line(hx, hy, x, y, 1.0, r, g, b,
        math.floor(255 * dot_alpha_base * 0.12))
    end
  end

  -- Layer 4: center glow.
  local core_r = width * (2.6 + progress * 1.8)
  ctx:radial_stops(hx, hy, core_r, {
    { offset = 0.0, r = 255, g = 255, b = 255, a = math.floor(ca * 0.55 * vis) },
    { offset = 0.55, r = r, g = g, b = b, a = math.floor(ca * 0.18 * vis) },
    { offset = 1.0, r = r, g = g, b = b, a = 0 },
  })
end
