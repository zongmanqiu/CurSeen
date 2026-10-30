-- CursorFX · rocket / 复刻自 cursorfx-main/src/cursors/rocket.js（MIT） / 本文件为独立 Lua 重实现
--
-- 视觉：小火箭朝运动方向缓慢转头；速度越快尾焰越长，喷出的火星越多；按住加速。
-- 原版的合成音效（thruster）在 CurSeen Lua 里没有暴露音频接口，省略。
--
-- 实现：所有机身部件都是本地坐标多边形，绘制时按 heading 旋转 + 统一缩放。
-- 火星池上限 100：满速按住时喷 5 颗/帧、寿命 10–20 帧，实际存活约 50–100 颗，
-- 再高的上限永远碰不到；并把 60fps 的“每帧速度”换算到固定步长 dt。
--
-- 性能：全速按住时最多 100 个加色火星 + 5 个多边形 + 2 个圆，单帧 < 120 图元。

name = "rocket"
effect_name = "rocket"
kind = "cursor"
description = "CursorFX rocket：转向跟随的火箭，带尾焰与火星。"
hide_cursor = true

function schema()
  return {
    { key = "color",      label = "箭体颜色", type = "color", default = "#EEF1F6" },
    { key = "accent",     label = "装饰颜色", type = "color", default = "#FF5252" },
    { key = "scale",      label = "缩放",     type = "float", min = 0.5, max = 2.0, step = 0.05, default = 1.0 },
    { key = "sparks",     label = "火星",     type = "bool",  default = true },
    { key = "max_sparks", label = "火星上限", type = "int",   min = 30, max = 100, step = 5,  default = 100 },
  }
end

local r = { x = 0, y = 0, heading = -math.pi / 2.0, s = 0 }
local sparks = {}
local spawn_acc = 0
local started = false

local function clampf(v, lo, hi)
  if v < lo then return lo end
  if v > hi then return hi end
  return v
end

local function body_scale(params)
  local m = math.min(screen_w or 1920, screen_h or 1080) / 800.0
  return (params.scale or 1.0) * clampf(m, 0.8, 1.2)
end

function init()
  r.x, r.y, r.heading, r.s = 0, 0, -math.pi / 2.0, 0
  sparks = {}
  spawn_acc = 0
  started = false
end

function step(dt, t, params)
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    r.x, r.y = mx, my
    started = true
  end

  local f = clampf(dt * 60.0, 0.0, 2.0)
  local gx, gy = mx - r.x, my - r.y
  local gap = math.sqrt(gx * gx + gy * gy)
  local nx, ny = mx, my
  if gap > 1.5 then
    local max_step = (20.0 + r.s * 40.0) * f
    local step = gap * math.min(1.0, 0.8 * f)
    if step > max_step then step = max_step end
    if step < gap then
      nx = r.x + gx / gap * step
      ny = r.y + gy / gap * step
    end
  end

  local dx, dy = nx - r.x, ny - r.y
  local d = math.sqrt(dx * dx + dy * dy)
  r.x, r.y = nx, ny
  if f > 1e-4 and d / f > 0.4 then
    local a = math.atan(dy, dx) - r.heading
    a = math.atan(math.sin(a), math.cos(a))
    r.heading = r.heading + a * math.min(1.0, 0.25 * f)
  end
  if f > 1e-4 then
    r.s = r.s + (clampf(d / f / 24.0, 0.0, 1.0) - r.s) * math.min(1.0, 0.2 * f)
  end

  local boost = mouse_down and 1 or 0
  local cap = math.floor(clampf(params.max_sparks or 100, 30, 100))
  if params.sparks ~= false and (r.s > 0.05 or boost > 0) then
    spawn_acc = spawn_acc + (1.0 + math.floor(r.s * 2.0 + boost * 2.0 + 0.5)) * f
    local sc = body_scale(params)
    while spawn_acc >= 1.0 do
      spawn_acc = spawn_acc - 1.0
      while #sparks >= cap do
        table.remove(sparks, 1)
      end
      local ch, sh = math.cos(r.heading), math.sin(r.heading)
      local sp = 2.0 + r.s * 4.0 + boost * 2.0
      local life = rng(10.0, 20.0) / 60.0
      sparks[#sparks + 1] = {
        x = r.x - ch * 20.0 * sc,
        y = r.y - sh * 20.0 * sc,
        vx = -ch * sp + rng(-1.0, 1.0),
        vy = -sh * sp + rng(-1.0, 1.0),
        life = life,
        max = life,
        size = rng(2.0, 4.0) * sc,
      }
    end
  else
    spawn_acc = 0
  end

  local decay = 0.92 ^ f
  for i = #sparks, 1, -1 do
    local p = sparks[i]
    p.life = p.life - dt
    if p.life <= 0 then
      table.remove(sparks, i)
    else
      p.x = p.x + p.vx * f
      p.y = p.y + p.vy * f
      p.vx = p.vx * decay
      p.vy = p.vy * decay
    end
  end
end

function draw(ctx, params)
  if not started then return end

  for i = 1, #sparks do
    local p = sparks[i]
    local k = clampf(p.life / p.max, 0.0, 1.0)
    local cr, cg, cb
    if k > 0.6 then
      cr, cg, cb = 255, 240, 150
    elseif k > 0.3 then
      cr, cg, cb = 255, 150, 40
    else
      cr, cg, cb = 255, 60, 20
    end
    ctx:plus_circle(p.x, p.y, p.size * (0.4 + k), cr, cg, cb, math.floor(255.0 * k))
  end

  local sc = body_scale(params)
  local ch, sh = math.cos(r.heading), math.sin(r.heading)
  local bx, by = r.x, r.y

  local function wx(lx, ly)
    return bx + (lx * ch - ly * sh) * sc
  end
  local function wy(lx, ly)
    return by + (lx * sh + ly * ch) * sc
  end
  local function poly(points, cr, cg, cb, ca)
    local out = {}
    for i = 1, #points do
      local p = points[i]
      out[i] = { wx(p[1], p[2]), wy(p[1], p[2]) }
    end
    ctx:polygon(out, cr, cg, cb, ca)
  end

  local flame = 8.0 + r.s * 26.0 + ((mouse_down and 14.0) or 0.0)
  local flick = math.sin((elapsed or 0) * 40.0) * 3.0

  poly({
    { -18, -5 }, { -18 - flame - flick, 0 }, { -18, 5 },
  }, 255, 170, 60, 230)

  poly({
    { -18, -2.5 }, { -18 - flame * 0.55, 0 }, { -18, 2.5 },
  }, 255, 240, 180, 242)

  local fr, fg, fb, fa = hex_to_rgb(params.accent or "#FF5252")
  poly({
    { -14, -6 }, { -24, -13 }, { -18, 0 }, { -24, 13 }, { -14, 6 },
  }, fr, fg, fb, fa)

  local br, bg, bb, ba = hex_to_rgb(params.color or "#EEF1F6")
  poly({
    { -18, -7 }, { 10, -7 }, { 16, -6.6 }, { 21, -5 }, { 24.5, -2.7 },
    { 26, 0 }, { 24.5, 2.7 }, { 21, 5 }, { 16, 6.6 }, { 10, 7 }, { -18, 7 },
  }, br, bg, bb, ba)

  local ar, ag, ab, aa = hex_to_rgb(params.accent or "#FF5252")
  poly({
    { 12, -7 }, { 18, -6.2 }, { 23, -4 }, { 26, 0 },
    { 23, 4 }, { 18, 6.2 }, { 12, 7 },
  }, ar, ag, ab, aa)

  local gx, gy = wx(2, 0), wy(2, 0)
  ctx:circle(gx, gy, 3.8 * sc, 90, 209, 255, 255)
  ctx:ring(gx, gy, 3.8 * sc, 1.2 * sc, 0, 0, 0, 90)
end
