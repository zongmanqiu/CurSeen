-- MouseEffects · Cherry Blossoms
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围飘落的樱花：每片花瓣是六点轮廓 + 一条叶脉，边落边摆边翻转。
-- 上游是 500 片 GPU 粒子；这里按软渲染压到 60 片，跟随指针整体缓慢移动。

name = "Cherry Blossoms"
effect_name = "Cherry Blossoms"
kind = "cursor"
description = "MouseEffects Cherry Blossoms：围绕光标飘落的樱花。"
hide_cursor = true

local MAX_PETALS = 60
local petals = {}
local cx, cy = 0, 0
local started = false

function schema()
  return {
    { key = "count",  label = "花瓣数",   type = "int",   min = 12,  max = 60,  step = 2,   default = 30 },
    { key = "size",   label = "花瓣大小", type = "float", min = 6,   max = 24,  step = 1,   default = 12 },
    { key = "fall",   label = "下落速度", type = "float", min = 20,  max = 120, step = 5,   default = 50 },
    { key = "sway",   label = "摆动幅度", type = "float", min = 10,  max = 80,  step = 5,   default = 40 },
    { key = "spin",   label = "翻转速度", type = "float", min = 0,   max = 3,   step = 0.1, default = 1.0 },
    { key = "glow",   label = "柔光",     type = "float", min = 0,   max = 1,   step = 0.05, default = 0.25 },
    { key = "color",  label = "花瓣颜色", type = "color", default = "#FFB7C5" },
  }
end

function init()
  petals = {}
  for i = 1, MAX_PETALS do
    petals[i] = {
      dx = rng(-260, 260),
      dy = rng(-260, 260),
      phase = rng(0, math.pi * 2),
      spin = rng(-1, 1),
      size = rng(0.7, 1.3),
    }
  end
  cx, cy = mouse_x or 0, mouse_y or 0
  started = false
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  if not started then
    cx, cy = mx, my
    started = true
  end
  local k = math.min(1.0, 0.08 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local fall = params.fall or 50
  local sway = params.sway or 40
  local spin = params.spin or 1.0
  for i = 1, #petals do
    local p = petals[i]
    p.phase = p.phase + dt * (0.5 + spin * 0.8)
    p.dy = p.dy + fall * dt
    p.dx = p.dx + math.sin(p.phase) * sway * dt
    p.spin = p.spin + spin * dt * 0.8
    if p.dy > 300 then
      p.dy = -280
      p.dx = rng(-260, 260)
    end
    if p.dx > 320 then p.dx = -300 elseif p.dx < -320 then p.dx = 300 end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_PETALS, math.floor(params.count or 30))
  local base = params.size or 12
  local glow = params.glow or 0.25
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFB7C5")
  local am = ca / 255

  for i = 1, n do
    local p = petals[i]
    local s = base * p.size
    local x = cx + p.dx
    local y = cy + p.dy
    -- 翻转：横向缩放随 spin 相位变化，模拟花瓣翻面。
    local flip = 0.25 + 0.75 * math.abs(math.cos(p.phase * 0.9))
    local ca, sa = math.cos(p.spin), math.sin(p.spin)
    local shape = {
      { 0, -s * 0.55 },
      { s * 0.55 * flip, -s * 0.18 },
      { s * 0.42 * flip, s * 0.50 },
      { 0, s * 0.28 },
      { -s * 0.42 * flip, s * 0.50 },
      { -s * 0.55 * flip, -s * 0.18 },
    }
    local pts = {}
    for k = 1, #shape do
      local px, py = shape[k][1], shape[k][2]
      pts[k] = { x + px * ca - py * sa, y + px * sa + py * ca }
    end
    ctx:polygon(pts, math.floor(cr * 0.98), math.floor(cg * 0.75), math.floor(cb * 0.82), math.floor(220 * am))
    ctx:line(x, y - s * 0.25, x, y + s * 0.25, 0.7,
      math.floor(cr * 0.8), math.floor(cg * 0.55), math.floor(cb * 0.65), math.floor(120 * am))
    if glow > 0.01 and i % 4 == 0 then
      ctx:plus_circle(x, y, s * 0.8, cr, cg, cb, math.floor(35 * glow * am))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
