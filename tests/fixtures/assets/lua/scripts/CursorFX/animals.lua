-- CursorFX · animals
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：粒子从按下点爆散，再聚拢成一种动物轮廓，停留片刻后溶解。原版靠 emoji
-- 像素采样得到轮廓；CurSeen 不能读像素，这里改用 8 组程序化椭圆/折线轮廓（猫、鱼、
-- 蝴蝶、兔、鸟、龟、狐、蛙），保留“爆散 → 聚形 → 溶解”的叙事，粒子上限 260。

name = "animals"
effect_name = "animals"
kind = "click"
description = "CursorFX animals：粒子爆散后聚成动物轮廓的简化版。"

local TAU = math.pi * 2
local MAX_PARTICLES = 260
local BURST_TIME = 0.5
local DISSOLVE_TIME = 1.3
local REPEL_R = 95
local OUTLINE_R, OUTLINE_G, OUTLINE_B = 190, 205, 230

local particles = {}
local paths = {}
local targets = {}
local mode = "off"
local clock = 0
local hold = 0
local animal_idx = 0
local shape_cx, shape_cy = 0, 0

local function add_ellipse(out, cx, cy, rx, ry, rot, a0, a1, closed)
  rot = rot or 0
  a0 = a0 or 0
  if a1 == nil then
    a1 = TAU
  end
  local span = a1 - a0
  local per = math.pi * (3 * (rx + ry) - math.sqrt((3 * rx + ry) * (rx + 3 * ry)))
  local steps = math.floor(per * math.abs(span) / TAU / 7)
  if steps < 14 then
    steps = 14
  end
  if steps > 90 then
    steps = 90
  end
  local pts = {}
  local cr, sr = math.cos(rot), math.sin(rot)
  local count = closed and steps or (steps + 1)
  for i = 0, count - 1 do
    local a = a0 + span * i / steps
    local ex, ey = math.cos(a) * rx, math.sin(a) * ry
    pts[#pts + 1] = { cx + ex * cr - ey * sr, cy + ex * sr + ey * cr }
  end
  out[#out + 1] = { pts = pts, closed = closed }
end

local function add_poly(out, coords, closed)
  local n = math.floor(#coords / 2)
  local pts = {}
  local segs = closed and n or (n - 1)
  for i = 0, segs - 1 do
    local x1, y1 = coords[i * 2 + 1], coords[i * 2 + 2]
    local j = (i + 1) % n
    local x2, y2 = coords[j * 2 + 1], coords[j * 2 + 2]
    local dx, dy = x2 - x1, y2 - y1
    local len = math.sqrt(dx * dx + dy * dy)
    local steps = math.floor(len / 7)
    if steps < 2 then
      steps = 2
    end
    for s = 0, steps - 1 do
      local u = s / steps
      pts[#pts + 1] = { x1 + dx * u, y1 + dy * u }
    end
  end
  if not closed then
    pts[#pts + 1] = { coords[(n - 1) * 2 + 1], coords[(n - 1) * 2 + 2] }
  end
  out[#out + 1] = { pts = pts, closed = closed }
end

local function add_quad(out, x0, y0, cx, cy, x1, y1)
  local pts = {}
  local steps = 12
  for i = 0, steps do
    local u = i / steps
    local iu = 1 - u
    pts[#pts + 1] = {
      iu * iu * x0 + 2 * iu * u * cx + u * u * x1,
      iu * iu * y0 + 2 * iu * u * cy + u * u * y1,
    }
  end
  out[#out + 1] = { pts = pts, closed = false }
end

local ANIMALS = {
  {
    name = "cat",
    colors = { "#AAB2C8", "#F6D0DD" },
    build = function(p)
      add_ellipse(p, 0, 0, 94, 80, 0, 0, TAU, true)
      add_poly(p, { -76, -48, -112, -136, -24, -84 }, true)
      add_poly(p, { 76, -48, 112, -136, 24, -84 }, true)
      add_ellipse(p, -34, -8, 7, 10, 0, 0, TAU, true)
      add_ellipse(p, 34, -8, 7, 10, 0, 0, TAU, true)
      add_poly(p, { -10, 22, 0, 32, 10, 22 }, true)
    end,
  },
  {
    name = "fish",
    colors = { "#38BDF8", "#FDE68A" },
    build = function(p)
      add_ellipse(p, 0, 0, 128, 72, 0, 0, TAU, true)
      add_poly(p, { -108, 0, -178, -52, -178, 52 }, true)
      add_ellipse(p, 36, 0, 30, 54, 0, math.pi * 0.55, math.pi * 1.45, false)
      add_ellipse(p, 74, -18, 8, 10, 0, 0, TAU, true)
    end,
  },
  {
    name = "butterfly",
    colors = { "#C084FC", "#FDE047" },
    build = function(p)
      add_ellipse(p, -62, -32, 68, 46, -0.35, 0, TAU, true)
      add_ellipse(p, 62, -32, 68, 46, 0.35, 0, TAU, true)
      add_ellipse(p, -50, 46, 52, 36, 0.3, 0, TAU, true)
      add_ellipse(p, 50, 46, 52, 36, -0.3, 0, TAU, true)
      add_ellipse(p, 0, 4, 13, 56, 0, 0, TAU, true)
      add_quad(p, -4, -48, -34, -102, -48, -118)
      add_quad(p, 4, -48, 34, -102, 48, -118)
    end,
  },
  {
    name = "rabbit",
    colors = { "#F9A8D4", "#FFFFFF" },
    build = function(p)
      add_ellipse(p, 0, 14, 76, 66, 0, 0, TAU, true)
      add_ellipse(p, -36, -66, 20, 64, 0.12, 0, TAU, true)
      add_ellipse(p, 36, -66, 20, 64, -0.12, 0, TAU, true)
      add_ellipse(p, -36, -68, 8, 46, 0.12, 0, TAU, true)
      add_ellipse(p, 36, -68, 8, 46, -0.12, 0, TAU, true)
      add_ellipse(p, -25, 10, 7, 9, 0, 0, TAU, true)
      add_ellipse(p, 25, 10, 7, 9, 0, 0, TAU, true)
      add_poly(p, { -6, 30, 0, 40, 6, 30 }, true)
    end,
  },
  {
    name = "bird",
    colors = { "#7DD3FC", "#E0F2FE" },
    build = function(p)
      add_ellipse(p, 0, 4, 88, 54, 0, 0, TAU, true)
      add_ellipse(p, 66, -38, 34, 32, 0, 0, TAU, true)
      add_poly(p, { 96, -44, 142, -34, 98, -16 }, true)
      add_poly(p, { -82, 2, -156, -34, -156, 30 }, true)
      add_ellipse(p, -6, 4, 44, 26, -0.35, 0, TAU, true)
      add_ellipse(p, 68, -40, 5, 6, 0, 0, TAU, true)
    end,
  },
  {
    name = "turtle",
    colors = { "#4ADE80", "#FEF3C7" },
    build = function(p)
      add_ellipse(p, 0, -4, 108, 72, 0, 0, TAU, true)
      add_ellipse(p, 126, 14, 30, 26, 0, 0, TAU, true)
      add_ellipse(p, -76, 60, 26, 14, 0.25, 0, TAU, true)
      add_ellipse(p, 76, 60, 26, 14, -0.25, 0, TAU, true)
      add_ellipse(p, -76, -62, 26, 14, -0.25, 0, TAU, true)
      add_ellipse(p, 76, -62, 26, 14, 0.25, 0, TAU, true)
      add_poly(p, { -104, 4, -146, 4, -122, 24 }, true)
    end,
  },
  {
    name = "fox",
    colors = { "#FB923C", "#FFF7ED" },
    build = function(p)
      add_ellipse(p, 0, 0, 92, 78, 0, 0, TAU, true)
      add_poly(p, { -76, -44, -112, -132, -26, -82 }, true)
      add_poly(p, { 76, -44, 112, -132, 26, -82 }, true)
      add_poly(p, { -14, 24, 0, 50, 14, 24 }, true)
      add_ellipse(p, -32, -6, 7, 9, 0, 0, TAU, true)
      add_ellipse(p, 32, -6, 7, 9, 0, 0, TAU, true)
    end,
  },
  {
    name = "frog",
    colors = { "#86EFAC", "#FEF08A" },
    build = function(p)
      add_ellipse(p, 0, 0, 112, 68, 0, 0, TAU, true)
      add_ellipse(p, -58, -52, 26, 24, 0, 0, TAU, true)
      add_ellipse(p, 58, -52, 26, 24, 0, 0, TAU, true)
      add_ellipse(p, -58, -52, 10, 10, 0, 0, TAU, true)
      add_ellipse(p, 58, -52, 10, 10, 0, 0, TAU, true)
      add_ellipse(p, 0, 6, 52, 34, 0, 0.15 * math.pi, 0.85 * math.pi, false)
    end,
  },
}

local function build_shape(def)
  paths = {}
  def.build(paths)
  targets = {}
  local minx, miny, maxx, maxy = 1e9, 1e9, -1e9, -1e9
  for pi = 1, #paths do
    local pts = paths[pi].pts
    for i = 1, #pts do
      local pt = pts[i]
      if pt[1] < minx then minx = pt[1] end
      if pt[1] > maxx then maxx = pt[1] end
      if pt[2] < miny then miny = pt[2] end
      if pt[2] > maxy then maxy = pt[2] end
    end
  end
  local c1r, c1g, c1b = hex_to_rgb(def.colors[1])
  local c2r, c2g, c2b = hex_to_rgb(def.colors[2])
  local span = math.max(1, maxy - miny)
  for pi = 1, #paths do
    local pts = paths[pi].pts
    for i = 1, #pts do
      local pt = pts[i]
      local u = clamp((pt[2] - miny) / span, 0, 1)
      targets[#targets + 1] = {
        x = pt[1],
        y = pt[2],
        r = math.floor(lerp(c1r, c2r, u)),
        g = math.floor(lerp(c1g, c2g, u)),
        b = math.floor(lerp(c1b, c2b, u)),
      }
    end
  end
  return minx, miny, maxx, maxy
end

function schema()
  return {
    { key = "count", label = "粒子数",   type = "int",   min = 160, max = 260, step = 10, default = 240 },
    { key = "hold",  label = "停留(秒)", type = "float", min = 1.0, max = 3.8, step = 0.1, default = 2.6 },
  }
end

function init()
  particles = {}
  paths = {}
  targets = {}
  mode = "off"
  clock = 0
  hold = 0
  animal_idx = 0
end

function on_press(x, y, t, params)
  animal_idx = animal_idx % #ANIMALS + 1
  local def = ANIMALS[animal_idx]
  local minx, miny, maxx, maxy = build_shape(def)
  shape_cx = clamp(x, 14 - minx, math.max(14 - minx, screen_w - 14 - maxx))
  shape_cy = clamp(y, 14 - miny, math.max(14 - miny, screen_h - 14 - maxy))

  local count = clamp(math.floor(params.count or 240), 120, MAX_PARTICLES)
  local nt = #targets
  if nt == 0 then
    return
  end
  local hold_time = clamp(params.hold or 2.6, 1.0, 3.8)
  local life = BURST_TIME + hold_time + DISSOLVE_TIME

  particles = {}
  for i = 1, count do
    local a = rng(0, TAU)
    local sp = rng(220, 620)
    local j = math.floor((i - 1) * nt / count) + 1
    if j > nt then
      j = nt
    end
    local tg = targets[j]
    particles[#particles + 1] = {
      x = x + rng(-6, 6),
      y = y + rng(-6, 6),
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp,
      tx = shape_cx + tg.x + rng(-1.2, 1.2),
      ty = shape_cy + tg.y + rng(-1.2, 1.2),
      r = tg.r,
      g = tg.g,
      b = tg.b,
      size = rng(1.6, 2.6),
      alpha = 1,
      life = life,
      max = life,
    }
  end
  mode = "burst"
  clock = 0
  hold = 0
end

function step(dt, t, params)
  if mode == "off" or #particles == 0 then
    return
  end

  clock = clock + dt
  if mode == "burst" then
    if clock >= BURST_TIME then
      mode = "assemble"
      clock = 0
    end
  elseif mode == "assemble" then
    hold = hold + dt
    if hold >= clamp(params.hold or 2.6, 1.0, 3.8) then
      mode = "dissolve"
      clock = 0
    end
  elseif mode == "dissolve" then
    if clock >= DISSOLVE_TIME then
      mode = "off"
      particles = {}
      paths = {}
      targets = {}
      return
    end
  end

  local mx, my = mouse_x or 0, mouse_y or 0
  local damp_burst = math.exp(-1.1 * dt)
  local damp_asm = math.exp(-7.5 * dt)
  local damp_dissolve = math.exp(-3.0 * dt)
  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      if mode == "burst" then
        p.x = p.x + p.vx * dt
        p.y = p.y + p.vy * dt
        p.vx = p.vx * damp_burst
        p.vy = p.vy * damp_burst
      elseif mode == "assemble" then
        local dx = p.tx - p.x
        local dy = p.ty - p.y
        p.vx = p.vx + dx * 110 * dt
        p.vy = p.vy + dy * 110 * dt
        local rx, ry = p.x - mx, p.y - my
        local d2 = rx * rx + ry * ry
        if d2 < REPEL_R * REPEL_R then
          local d = math.sqrt(d2)
          if d < 0.001 then
            d = 0.001
          end
          local f = (REPEL_R - d) / REPEL_R * 900 * dt
          p.vx = p.vx + rx / d * f
          p.vy = p.vy + ry / d * f
        end
        p.vx = p.vx * damp_asm
        p.vy = p.vy * damp_asm
        p.x = p.x + p.vx * dt
        p.y = p.y + p.vy * dt
      else
        p.alpha = p.alpha - dt / DISSOLVE_TIME
        p.vy = p.vy - 26 * dt
        p.vx = p.vx * damp_dissolve
        p.vy = p.vy * damp_dissolve
        p.x = p.x + p.vx * dt * 0.35
        p.y = p.y + p.vy * dt * 0.35
      end
      keep[#keep + 1] = p
    end
  end
  particles = keep
  if #particles == 0 then
    mode = "off"
    paths = {}
    targets = {}
  end
end

function draw(ctx, params)
  if mode == "off" or #particles == 0 then
    return
  end

  if mode ~= "burst" then
    local oa = 22
    if mode == "assemble" then
      oa = math.floor(22 * math.min(1, hold / 0.35))
    elseif mode == "dissolve" then
      oa = math.floor(22 * math.max(0, 1 - clock / DISSOLVE_TIME))
    end
    if oa > 0 then
      for pi = 1, #paths do
        local pts = paths[pi].pts
        for i = 1, #pts - 1 do
          local a, b = pts[i], pts[i + 1]
          ctx:line(
            shape_cx + a[1], shape_cy + a[2],
            shape_cx + b[1], shape_cy + b[2],
            1, OUTLINE_R, OUTLINE_G, OUTLINE_B, oa
          )
        end
        if paths[pi].closed and #pts > 2 then
          local a, b = pts[#pts], pts[1]
          ctx:line(
            shape_cx + a[1], shape_cy + a[2],
            shape_cx + b[1], shape_cy + b[2],
            1, OUTLINE_R, OUTLINE_G, OUTLINE_B, oa
          )
        end
      end
    end
  end

  local breathe = 1
  if mode == "assemble" then
    breathe = 1 + 0.1 * math.sin((elapsed or 0) * 2.2)
  end
  for i = 1, #particles do
    local p = particles[i]
    local a = math.floor(255 * clamp(p.alpha, 0, 1))
    if a > 0 then
      ctx:circle(p.x, p.y, p.size * breathe, p.r, p.g, p.b, a)
    end
  end
end
