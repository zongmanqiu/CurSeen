-- MouseEffects · Gravity Well
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围的引力井：碎屑按牛顿引力（G/d²，带软化项）绕光标运行——可以坠入
-- 井心（坠入即重生到井缘）、可以被甩出（出屏重生）、撞屏幕边缘会反弹
-- （上游 GravityWellEffect.cs:54-55,544）。带短拖尾；上游 346 颗 GPU 粒子，
-- 这里按软渲染压到 220 颗，并给一半粒子画拖尾。

name = "Gravity Well"
effect_name = "Gravity Well"
kind = "cursor"
description = "MouseEffects Gravity Well：围绕光标的牛顿引力碎屑井。"
hide_cursor = true

local MAX_PARTICLES = 220
local particles = {}
local cx, cy = 0, 0
local started = false

-- 引力常数（px³/s²）：半径 300px 处圆轨道速度 ≈ sqrt(G/300)·orbit。
local G_CONST = 8.0e6

function schema()
  return {
    { key = "count",   label = "碎屑数量", type = "int",   min = 60,  max = 220, step = 10, default = 140 },
    { key = "radius",  label = "引力半径", type = "float", min = 120, max = 500, step = 10, default = 300 },
    { key = "orbit",   label = "轨道速度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.6 },
    { key = "gravity", label = "引力强度", type = "float", min = 0.2, max = 2,   step = 0.1, default = 1.0 },
    { key = "damping", label = "阻尼",     type = "float", min = 0.9, max = 1,   step = 0.01, default = 0.98 },
    { key = "size",    label = "碎屑大小", type = "float", min = 1,   max = 4,   step = 0.5, default = 2 },
    { key = "color",   label = "碎屑颜色", type = "color", default = "#33CCFF" },
  }
end

function init()
  particles = {}
  started = false
end

-- 播种：井缘内随机半径，初速取该半径的圆轨道速度（切向）×orbit，少量反向
-- 与随机扰动制造椭圆轨道——gravity/damping/orbit 三个滑条因此都真实生效。
local function seed(params, mx, my)
  particles = {}
  local R = params.radius or 300
  local orbit = params.orbit or 0.6
  local G = G_CONST * (params.gravity or 1.0)
  for i = 1, MAX_PARTICLES do
    local a = rng(0, math.pi * 2)
    local d = R * rng(0.25, 1.0)
    local v = math.sqrt(G / d) * orbit * rng(0.85, 1.15)
    local sgn = rng(0, 1) < 0.85 and 1 or -1
    local x = mx + math.cos(a) * d
    local y = my + math.sin(a) * d
    particles[i] = {
      x = x, y = y, px = x, py = y,
      vx = -math.sin(a) * v * sgn + rng(-12, 12),
      vy = math.cos(a) * v * sgn + rng(-12, 12),
      size = rng(0.6, 1.3),
    }
  end
end

-- 重生到井缘：新的圆轨道初速。
local function respawn(p, R, orbit, G)
  local ang = rng(0, math.pi * 2)
  p.x = cx + math.cos(ang) * R
  p.y = cy + math.sin(ang) * R
  local v = math.sqrt(G / R) * orbit
  p.vx = -math.sin(ang) * v
  p.vy = math.cos(ang) * v
  p.px, p.py = p.x, p.y
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  if not started then
    cx, cy = mx, my
    started = true
    seed(params, cx, cy)
  end
  local k = math.min(1.0, 0.12 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  dt = math.min(dt, 0.05)

  local G = G_CONST * (params.gravity or 1.0)
  local damp_f = math.pow(params.damping or 0.98, f)
  local R = params.radius or 300
  local orbit = params.orbit or 0.6
  local sx, sy = screen_x or 0, screen_y or 0
  local sw, sh = screen_w or 1920, screen_h or 1080

  for i = 1, #particles do
    local p = particles[i]
    p.px, p.py = p.x, p.y
    local dx, dy = cx - p.x, cy - p.y
    local d2 = dx * dx + dy * dy + 900
    local d = math.sqrt(d2)
    -- 牛顿引力，软化项 30px 防数值爆炸，加速度封顶。
    local acc = G / d2
    if acc > 2400 then acc = 2400 end
    p.vx = (p.vx + dx / d * acc * dt) * damp_f
    p.vy = (p.vy + dy / d * acc * dt) * damp_f
    local sp = math.sqrt(p.vx * p.vx + p.vy * p.vy)
    if sp > 900 then
      p.vx = p.vx / sp * 900
      p.vy = p.vy / sp * 900
    end
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt

    -- 坠入井心：重生到井缘。
    if d < 20 then
      respawn(p, R, orbit, G)
    else
      -- 屏幕边缘反弹。
      if p.x < sx then
        p.x = sx
        p.vx = math.abs(p.vx) * 0.7
      elseif p.x > sx + sw then
        p.x = sx + sw
        p.vx = -math.abs(p.vx) * 0.7
      end
      if p.y < sy then
        p.y = sy
        p.vy = math.abs(p.vy) * 0.7
      elseif p.y > sy + sh then
        p.y = sy + sh
        p.vy = -math.abs(p.vy) * 0.7
      end
      -- 逃逸出屏（含余量）：拉回井缘重置。
      if p.x < sx - 60 or p.x > sx + sw + 60 or p.y < sy - 60 or p.y > sy + sh + 60 then
        respawn(p, R, orbit, G)
      end
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_PARTICLES, math.floor(params.count or 140))
  local R = params.radius or 300
  local size = params.size or 2
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#33CCFF")
  local am = ca / 255

  -- 轨道参考环（很淡），说明"引力井"的范围。
  ctx:ring(cx, cy, R * 0.96, 1.2, cr, cg, cb, math.floor(60 * am))
  ctx:ring(cx, cy, R * 0.55, 1.0, cr, cg, cb, math.floor(38 * am))

  for i = 1, n do
    local p = particles[i]
    if i % 2 == 0 then
      ctx:line(p.px, p.py, p.x, p.y, 1.0, cr, cg, cb, math.floor(95 * am))
    end
    ctx:plus_circle(p.x, p.y, size * p.size, cr, cg, cb, math.floor(215 * am))
  end

  ctx:glow(cx, cy, 16 + 10 * (params.orbit or 0.6), cr, cg, cb, math.floor(70 * am))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
