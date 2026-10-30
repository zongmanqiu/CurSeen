-- CursorFX · f1car
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 光标替换类：赛车始终吸附在指针附近，转向由指针方向驱动；按住鼠标给一个
-- 额外推力，速度线、排气火焰和轮胎烟都会变强。原版 V10 引擎/8 速变速箱的
-- 音频合成与换挡音不在 CurSeen 的 API 范围内；随之舍弃的还有升挡喷焰爆发、
-- 减速回火等换挡视觉（f1car.js:162-164），常态排气焰发生率也低于原版
-- （24-80/s vs 60-180/s）。车体/车轮全部用旋转后的多边形顶点绘制，不依赖变换栈。

name = "f1car"
effect_name = "F1 赛车"
kind = "cursor"
description = "CursorFX f1car：随指针转向的 F1 赛车，按住加速，带轮胎烟与排气火焰。"
hide_cursor = true

local TAU = math.pi * 2
local MAX_FLAME = 70
local MAX_SMOKE = 50

local flames = {}
local smoke = {}
local flame_acc = 0
local smoke_acc = 0
local flame_side = 1
local smoke_side = 1
local placed = false

local car = {
  x = 0, y = 0, heading = 0, speed = 0, s = 0,
  steer = 0, turn = 0, screech = 0, scale = 1,
}

function schema()
  return {
    { key = "livery",      label = "车身色",   type = "color", alpha = true, default = "#D4161F" },
    { key = "accent",      label = "点缀色",   type = "color", alpha = true, default = "#F5F5F7" },
    { key = "scale",       label = "大小",     type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
    { key = "boostOnHold", label = "按住加速", type = "bool",  default = true },
    { key = "flames",      label = "排气火焰", type = "bool",  default = true },
    { key = "smoke",       label = "轮胎烟",   type = "bool",  default = true },
    { key = "speedLines",  label = "速度线",   type = "bool",  default = true },
  }
end

local function box(x, y, w, h)
  return { { x, y }, { x + w, y }, { x + w, y + h }, { x, y + h } }
end

local function oval(cx, cy, rx, ry, n)
  local out = {}
  for i = 0, n - 1 do
    local a = TAU * i / n
    out[#out + 1] = { cx + math.cos(a) * rx, cy + math.sin(a) * ry }
  end
  return out
end

local BODY = {
  { -31, -6 }, { -16, -12 }, { -2, -12 }, { 8, -6 }, { 33, -2.2 },
  { 33, 2.2 }, { 8, 6 }, { -2, 12 }, { -16, 12 }, { -31, 6 },
}
local TOP_HALF = {
  { -31, -6 }, { -16, -12 }, { -2, -12 }, { 8, -6 }, { 33, -2.2 },
  { 33, 0 }, { -31, 0 },
}
local SIDEPOD_L = { { -16, -12 }, { -2, -12 }, { 6, -7 }, { -16, -8 } }
local SIDEPOD_R = { { -16, 12 }, { -2, 12 }, { 6, 7 }, { -16, 8 } }
local ENGINE_FIN = { { -36, -2 }, { -20, -4 }, { -10, 0 }, { -20, 4 }, { -36, 2 } }
local WHEELS = {
  { x = -24, y = -13,   w = 15, h = 10, front = false },
  { x = -24, y = 13,    w = 15, h = 10, front = false },
  { x = 18,  y = -14.5, w = 13, h = 9,  front = true },
  { x = 18,  y = 14.5,  w = 13, h = 9,  front = true },
}

local function wheel_pts(wh, steer)
  local hw, hh = wh.w * 0.5, wh.h * 0.5
  local base = {
    { -hw + 2, -hh }, { -hw, -hh + 2 }, { hw - 2, -hh }, { hw, -hh + 2 },
    { hw, hh - 2 }, { hw - 2, hh }, { -hw + 2, hh }, { -hw, hh - 2 },
  }
  local c, s = math.cos(steer), math.sin(steer)
  local out = {}
  for i = 1, #base do
    local bx, by = base[i][1], base[i][2]
    out[i] = { wh.x + bx * c - by * s, wh.y + bx * s + by * c }
  end
  return out
end

function init()
  flames = {}
  smoke = {}
  flame_acc = 0
  smoke_acc = 0
  flame_side = 1
  smoke_side = 1
  placed = false
  car.x = mouse_x or 0
  car.y = mouse_y or 0
  car.heading = mouse_heading or 0
  car.speed = 0
  car.s = 0
  car.steer = 0
  car.turn = 0
  car.screech = 0
  car.scale = 1
end

local function wrap(a)
  return math.atan(math.sin(a), math.cos(a))
end

local function total_scale(params)
  local sw = screen_w or 1920
  local sh = screen_h or 1080
  return (params.scale or 1.0) * clamp(math.min(sw, sh) / 800.0, 0.8, 1.2)
end

local function emit_flame(boost)
  local ch, sh = math.cos(car.heading), math.sin(car.heading)
  local sc = car.scale
  flame_side = -flame_side
  local lx, ly = -37 * sc, 6 * flame_side * sc
  local wx = car.x + lx * ch - ly * sh
  local wy = car.y + lx * sh + ly * ch
  local sp = (2 + car.s * 5) * boost
  local life = rng(0.14, 0.30)
  flames[#flames + 1] = {
    x = wx, y = wy,
    vx = -ch * sp * 60 + rng(-45, 45),
    vy = -sh * sp * 60 + rng(-45, 45),
    size = rng(1.8, 3.4) * sc * boost,
    life = life, max = life,
  }
  if #flames > MAX_FLAME then table.remove(flames, 1) end
end

local function emit_smoke(strength)
  local ch, sh = math.cos(car.heading), math.sin(car.heading)
  local sc = car.scale
  smoke_side = -smoke_side
  local lx, ly = -24 * sc, 13 * smoke_side * sc
  local wx = car.x + lx * ch - ly * sh
  local wy = car.y + lx * sh + ly * ch
  local life = rng(0.55, 1.0)
  smoke[#smoke + 1] = {
    x = wx, y = wy,
    vx = (-ch * 0.6 + rng(-0.4, 0.4)) * 60,
    vy = (-sh * 0.6 + rng(-0.4, 0.4)) * 60,
    size = rng(2.0, 4.0) * sc * (0.8 + strength * 0.4),
    life = life, max = life,
  }
  if #smoke > MAX_SMOKE then table.remove(smoke, 1) end
end

local function update_particles(dt)
  local keep = {}
  for i = 1, #flames do
    local p = flames[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      local d = math.exp(-dt * 6.0)
      p.vx = p.vx * d
      p.vy = p.vy * d
      p.size = p.size * (1 - dt * 0.5)
      keep[#keep + 1] = p
    end
  end
  flames = keep

  keep = {}
  for i = 1, #smoke do
    local p = smoke[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      local d = math.exp(-dt * 4.2)
      p.vx = p.vx * d
      p.vy = p.vy * d
      p.size = p.size + 22 * dt
      keep[#keep + 1] = p
    end
  end
  smoke = keep
end

function step(dt, t, params)
  params = params or {}
  if not placed then
    car.x = mouse_x or car.x
    car.y = mouse_y or car.y
    car.heading = mouse_heading or car.heading
    placed = true
  end
  car.scale = total_scale(params)

  local mx = mouse_x or car.x
  local my = mouse_y or car.y
  local gx, gy = mx - car.x, my - car.y
  local gap = math.sqrt(gx * gx + gy * gy)
  local nx, ny = mx, my
  local max_step = (22 + car.s * 40) * dt * 60
  if gap > 1.5 then
    local stepd = gap * (1 - math.exp(-dt * 18))
    if stepd > max_step then stepd = max_step end
    if stepd < gap then
      nx = car.x + gx / gap * stepd
      ny = car.y + gy / gap * stepd
    end
  end

  local dx, dy = nx - car.x, ny - car.y
  local dist = math.sqrt(dx * dx + dy * dy)
  local old_heading = car.heading
  if gap > 0.4 then
    local target = math.atan(my - car.y, mx - car.x)
    local d = wrap(target - car.heading)
    car.heading = car.heading + d * (1 - math.exp(-dt * 20))
    local dh = wrap(car.heading - old_heading)
    local turn_frame = dh / math.max(dt, 1e-4) / 60
    car.turn = car.turn + (turn_frame - car.turn) * (1 - math.exp(-dt * 14))
  else
    car.turn = car.turn * math.exp(-dt * 10)
  end

  car.x = nx
  car.y = ny
  local target_speed = dist / math.max(dt, 1e-4) / 60
  car.speed = car.speed + (target_speed - car.speed) * (1 - math.exp(-dt * 12))

  local steer_target = clamp(car.turn * 6, -0.55, 0.55)
  car.steer = car.steer + (steer_target - car.steer) * (1 - math.exp(-dt * 14))

  local boosting = params.boostOnHold ~= false and mouse_down
    and (mouse_button == nil or mouse_button == "left")
  local st = clamp(car.speed / 26, 0, 1)
  if boosting then st = math.min(1, st + 0.35) end
  car.s = car.s + (st - car.s) * (1 - math.exp(-dt * 9))

  local lat = math.abs(car.turn) * car.speed
  local scr = clamp((lat - 0.9) / 2.5, 0, 1)
  car.screech = car.screech + (scr - car.screech) * (1 - math.exp(-dt * 12))

  if params.flames ~= false and car.s > 0.3 then
    local rate = boosting and 80 or (car.s > 0.6 and 46 or 24)
    flame_acc = flame_acc + rate * dt
    while flame_acc >= 1 do
      flame_acc = flame_acc - 1
      emit_flame(boosting and 1.5 or 1.0)
    end
  else
    flame_acc = 0
  end

  if params.smoke ~= false and car.screech > 0.08 then
    smoke_acc = smoke_acc + 34 * car.screech * dt
    while smoke_acc >= 1 do
      smoke_acc = smoke_acc - 1
      emit_smoke(car.screech)
    end
  else
    smoke_acc = 0
  end

  update_particles(dt)
end

function draw(ctx, params)
  params = params or {}
  local lr, lg, lb, la = hex_to_rgb(params.livery or "#D4161F")
  local ar, ag, ab, aa = hex_to_rgb(params.accent or "#F5F5F7")
  local s = car.scale
  local ch, sh = math.cos(car.heading), math.sin(car.heading)

  local function w(lx, ly)
    return car.x + (lx * ch - ly * sh) * s, car.y + (lx * sh + ly * ch) * s
  end

  local function poly(pts, r, g, b, a)
    local out = {}
    for i = 1, #pts do
      local x, y = w(pts[i][1], pts[i][2])
      out[i] = { x, y }
    end
    ctx:polygon(out, r, g, b, a)
  end

  local function line(x1, y1, x2, y2, width, r, g, b, a)
    local ax, ay = w(x1, y1)
    local bx, by = w(x2, y2)
    ctx:line(ax, ay, bx, by, math.max(0.6, width * s), r, g, b, a)
  end

  local function dot(lx, ly, radius, r, g, b, a)
    local x, y = w(lx, ly)
    ctx:circle(x, y, math.max(0.5, radius * s), r, g, b, a)
  end

  local boosting = params.boostOnHold ~= false and mouse_down
    and (mouse_button == nil or mouse_button == "left")

  poly(oval(-2, 3, 36, 18, 16), 0, 0, 0, 105)

  if boosting and params.flames ~= false then
    local gx, gy = w(-37, 0)
    ctx:radial_stops(gx, gy, 34 * s, {
      { offset = 0.0, r = 255, g = 150, b = 40, a = 60 },
      { offset = 0.5, r = 255, g = 90, b = 20, a = 28 },
      { offset = 1.0, r = 255, g = 60, b = 20, a = 0 },
    })
  end

  local motion = math.min(1, (mouse_speed or 0) / 26)
  local line_s = math.max(car.s, motion)
  if params.speedLines ~= false and line_s > 0.55 then
    local a = (line_s - 0.55) / 0.45
    for i = 0, 4 do
      local ly = -22 + i * 11 + math.sin((elapsed or 0) * 30 + i) * 3
      local len = 30 * a + i * 6
      line(-40, ly, -40 - len, ly, 1.2, 180, 220, 255, math.floor(90 * a))
    end
  end

  for i = 1, #smoke do
    local p = smoke[i]
    local k = math.max(0, p.life / p.max)
    local rz = p.size * (1.2 + (1 - k) * 1.5)
    ctx:radial_stops(p.x, p.y, rz, {
      { offset = 0.0, r = 175, g = 178, b = 190, a = math.floor(85 * k) },
      { offset = 0.65, r = 165, g = 168, b = 180, a = math.floor(42 * k) },
      { offset = 1.0, r = 160, g = 162, b = 175, a = 0 },
    })
  end

  for i = 1, #flames do
    local p = flames[i]
    local k = math.max(0, p.life / p.max)
    local a = math.floor(235 * k)
    local rz = p.size * (0.45 + k * 1.15)
    ctx:radial_stops(p.x, p.y, rz, {
      { offset = 0.0, r = 255, g = 246, b = 190, a = a },
      { offset = 0.42, r = 255, g = 150, b = 40, a = math.floor(a * 0.72) },
      { offset = 1.0, r = 255, g = 60, b = 15, a = 0 },
    })
  end

  poly(box(-39, -18, 8, 2.5), 230, 232, 238, 255)
  poly(box(-38, -16, 6, 32), 26, 27, 34, 255)
  line(-36.5, -13, -36.5, 13, 2.0, lr, lg, lb, la)
  poly(box(-39, 15.5, 8, 2.5), 230, 232, 238, 255)

  line(-24, -8, 18, -12, 1.2, 26, 27, 34, 150)
  line(-24, 8, 18, 12, 1.2, 26, 27, 34, 150)

  for i = 1, #WHEELS do
    local wh = WHEELS[i]
    local steer = wh.front and car.steer or 0
    poly(wheel_pts(wh, steer), 21, 22, 28, 255)
    local c, sn = math.cos(steer), math.sin(steer)
    local hw = wh.w * 0.5
    line(wh.x - hw * c, wh.y - hw * sn, wh.x + hw * c, wh.y + hw * sn,
      2.2, 58, 61, 72, 255)
    dot(wh.x, wh.y, 1.7, 58, 61, 72, 255)
  end

  poly(BODY, lr, lg, lb, la)
  poly(TOP_HALF, 255, 255, 255, 36)
  poly(SIDEPOD_L, 0, 0, 0, 70)
  poly(SIDEPOD_R, 0, 0, 0, 70)
  poly(ENGINE_FIN, 0, 0, 0, 40)
  line(-30, -1.5, -8, -1.5, 3.0, ar, ag, ab, aa)
  line(12, -1.2, 30, -1.2, 2.4, ar, ag, ab, aa)

  poly(box(27, -19, 6, 38), 230, 232, 238, 255)
  poly(box(29, -17, 2.5, 34), lr, lg, lb, la)
  poly(box(26, -20, 8, 2), lr, lg, lb, la)
  poly(box(26, 18, 8, 2), lr, lg, lb, la)

  poly(oval(-5, 0, 5, 3.6, 12), 11, 12, 16, 255)
  local hx, hy = w(-5, 0)
  ctx:ring(hx, hy, 6 * s, 2 * s, 42, 45, 54, 235)
  dot(-7, 0, 2.8, 255, 210, 63, 255)
  poly({ { -9.2, -2.2 }, { -5.2, -2.2 }, { -4.3, -1.2 }, { -9.2, -1.2 } },
    11, 12, 16, 255)

  dot(-37, -6, 1.7, 26, 27, 34, 255)
  dot(-37, 6, 1.7, 26, 27, 34, 255)
  if boosting then
    local ex, ey = w(-38, 0)
    ctx:plus_circle(ex, ey, 4 * s, 255, 150, 50, 90)
  end
end
