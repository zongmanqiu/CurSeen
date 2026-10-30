-- MFCMouseEffect · Tech Ring (3D)
-- 复刻自 MFCMouseEffect（https://github.com/sqmw/MFCMouseEffect），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制项目代码/素材。
--
-- 科技圈（伪 3D）：3 个旋转/倾斜的陀螺环 + 外层扫描弧 + 核心光球 + 40 个球面粒子。
-- 按住 260ms 后出现，进度推进时外环扫描弧展开。

name = "Tech Ring (3D)"
effect_name = "Tech Ring (3D)"
kind = "hold"
description = "MFCMouseEffect Tech Ring：按住时的 3D 陀螺科技圈。"

local HOLD_DELAY = 260
local PARTICLES = 40
local press_at = nil
local vis = 0
local progress = 0
local hx, hy = 0, 0
local particles = {}

local function rot_x(v, a)
  local c, s = math.cos(a), math.sin(a)
  return { v[1], v[2] * c - v[3] * s, v[2] * s + v[3] * c }
end
local function rot_y(v, a)
  local c, s = math.cos(a), math.sin(a)
  return { v[1] * c + v[3] * s, v[2], -v[1] * s + v[3] * c }
end
local function rot_z(v, a)
  local c, s = math.cos(a), math.sin(a)
  return { v[1] * c - v[2] * s, v[1] * s + v[2] * c, v[3] }
end
local function project(v)
  local scale = 500.0 / (500.0 + v[3])
  return hx + v[1] * scale, hy + v[2] * scale
end

function schema()
  return {
    { key = "color",    label = "线条颜色", type = "color", alpha = true, default = "#4DF8FF" },
    { key = "fill",     label = "内环颜色", type = "color", alpha = true, default = "#00000000" },
    { key = "size",     label = "半径",     type = "float", min = 20, max = 160, step = 2,   default = 70 },
    { key = "width",    label = "线宽",     type = "float", min = 0.5,max = 6,   step = 0.1, default = 2.8 },
    { key = "duration", label = "进度满(ms)", type = "int", min = 300, max = 3000, step = 50, default = 900 },
    { key = "follow",   label = "跟随平滑", type = "float", min = 0.05, max = 1, step = 0.05, default = 0.25 },
  }
end

local function make_particles(radius)
  particles = {}
  for _ = 1, PARTICLES do
    local theta = rng(0, math.pi * 2)
    local phi = rng(0, math.pi)
    local r = radius * 0.5 * rng(0, 1)
    particles[#particles + 1] = {
      x = r * math.sin(phi) * math.cos(theta),
      y = r * math.sin(phi) * math.sin(theta),
      z = r * math.cos(phi),
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
  make_particles((params and params.size) or 70)
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

local function draw_ring(ctx, radius, spin, width, r, g, b, alpha,
  start_arc, sweep, axis_mode, axis_tilt, base_x, base_y)
  if radius <= 0.5 then return end
  local segs = 60
  local pts = {}
  for i = 0, segs do
    local ang = start_arc + sweep * i / segs
    local v = { radius * math.cos(ang), radius * math.sin(ang), 0 }
    if axis_mode == 0 then
      v = rot_z(v, spin)
    elseif axis_mode == 1 then
      v = rot_z(v, spin)
      v = rot_x(v, axis_tilt)
    else
      v = rot_z(v, spin)
      v = rot_y(v, axis_tilt)
    end
    v = rot_x(v, base_x)
    v = rot_y(v, base_y)
    pts[i + 1] = { project(v) }
  end
  for i = 1, #pts - 1 do
    local pa, pb = pts[i], pts[i + 1]
    ctx:line(pa[1], pa[2], pb[1], pb[2], width, r, g, b, alpha)
  end
end

function draw(ctx, params)
  if vis <= 0.01 then return end
  params = params or {}
  local r, g, b, ca = hex_to_rgb(params.color or "#4DF8FF")
  local fr, fg, fb, fa = hex_to_rgb(params.fill or "#00000000")
  local base = params.size or 70
  local width = params.width or 2.8
  local seconds = (wall_ms or 0) / 1000.0
  local radius = base * (0.8 + 0.2 * progress)
  local tilt_x = math.rad(20)
  local tilt_y = math.rad(10)

  -- 中心光晕：上游已明确移除（TechRingRenderer.h SetCenterColor(透明) + 注释
  -- "removed completely per user request"），旧版自加的淡蓝雾不画了。

  draw_ring(ctx, radius * 0.6, seconds * 1.5, 2.0, r, g, b,
    math.floor(ca * 0.5 * vis), 0, math.pi * 2, 1, math.rad(60), tilt_x, tilt_y)
  draw_ring(ctx, radius * 0.8, -seconds * 1.2, 2.0, fr, fg, fb,
    math.floor(fa * 0.4 * vis), 0, math.pi * 2, 2, math.rad(45), tilt_x, tilt_y)
  draw_ring(ctx, radius * 1.1, seconds * 0.8, 3.0 * width, r, g, b,
    math.floor(ca * 0.7 * vis), 0, math.pi * 2 * 0.75 * progress, 0, 0, tilt_x, tilt_y)
  draw_ring(ctx, radius * 1.15, -seconds * 1.2, 1.0, r, g, b,
    math.floor(ca * 0.7 * vis), 0, 2.0, 0, 0, tilt_x, tilt_y)

  for i = 1, #particles do
    local p = particles[i]
    local v = rot_y({ p.x, p.y, p.z }, seconds * 0.5)
    v = rot_x(v, tilt_x)
    local x, y = project(v)
    local scale = 500.0 / (500.0 + v[3])
    local size = 3.0 * scale
    ctx:circle(x, y, math.max(0.5, size), r, g, b, math.floor(120 * vis))
  end
end
