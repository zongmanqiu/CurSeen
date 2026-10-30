-- MouseEffects · Fireflies
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围的萤火虫：随机游走 + 向光标轻微聚拢，每只按自己的相位明灭；
-- 点击时虫群从点击点炸开（上游 FirefliesFactory.cs 强度 1500 的外冲量）。
-- 上游默认 49 只、GPU 光晕；这里按软渲染保留 120 只上限。

name = "Fireflies"
effect_name = "Fireflies"
kind = "cursor"
description = "MouseEffects Fireflies：围绕光标明灭游走的萤火虫。"
hide_cursor = true

local MAX_FIREFLIES = 120
local flies = {}
local cx, cy = 0, 0
local started = false

function schema()
  return {
    { key = "count",     label = "数量",     type = "int",   min = 12,  max = 120, step = 1,   default = 49 },
    { key = "size",      label = "光点大小", type = "float", min = 1,   max = 5,   step = 0.5, default = 2 },
    { key = "glow",      label = "辉光半径", type = "float", min = 5,   max = 30,  step = 1,   default = 15 },
    { key = "speed",     label = "游走速度", type = "float", min = 20,  max = 140, step = 5,   default = 60 },
    { key = "attract",   label = "聚拢强度", type = "float", min = 0,   max = 2,   step = 0.1, default = 1.1 },
    { key = "pulse",     label = "明灭速度", type = "float", min = 0.5, max = 6,   step = 0.1, default = 3.0 },
    { key = "color",     label = "萤光颜色", type = "color", default = "#CCFF4D" },
  }
end

function init()
  flies = {}
  for i = 1, MAX_FIREFLIES do
    local a = rng(0, math.pi * 2)
    local d = rng(40, 260)
    flies[i] = {
      x = (mouse_x or 0) + math.cos(a) * d,
      y = (mouse_y or 0) + math.sin(a) * d,
      vx = rng(-20, 20),
      vy = rng(-20, 20),
      bvx = 0, bvy = 0,
      phase = rng(0, math.pi * 2),
      pulse = rng(0.6, 1.4),
    }
  end
  cx, cy = mouse_x or 0, mouse_y or 0
  started = false
end

-- 点击炸开：给每只一个按距衰减的外冲量（上游 FirefliesFactory.cs:48-49 强度
-- 1500；FirefliesEffect.cs:169-172）。爆发分量单独存放、按指数衰减，不进
-- maxsp 钉制的游走速度，否则冲量会被立刻刭平、看不到爆炸。
function on_press(x, y, t, params)
  for i = 1, #flies do
    local p = flies[i]
    local dx, dy = p.x - x, p.y - y
    local d = math.max(1, math.sqrt(dx * dx + dy * dy))
    local imp = 1500 / (1 + d / 140)
    p.bvx = (p.bvx or 0) + (dx / d) * imp
    p.bvy = (p.bvy or 0) + (dy / d) * imp
  end
end

function step(dt, t, params)
  params = params or {}
  local mx, my = mouse_x or cx, mouse_y or cy
  if not started then
    cx, cy = mx, my
    started = true
  end
  local k = math.min(1.0, 0.10 * math.min(2.0, dt * 60.0))
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local speed = params.speed or 60
  local attract = params.attract or 1.1
  local pulse = params.pulse or 3.0
  for i = 1, #flies do
    local p = flies[i]
    p.phase = p.phase + dt * pulse * p.pulse
    local dx, dy = cx - p.x, cy - p.y
    local d = math.max(1, math.sqrt(dx * dx + dy * dy))
    local pull = 0.18 * attract / (1 + d / 120)
    p.vx = p.vx + (dx / d) * pull * 60 * dt + rng(-1, 1) * 40 * dt
    p.vy = p.vy + (dy / d) * pull * 60 * dt + rng(-1, 1) * 40 * dt
    local sp = math.sqrt(p.vx * p.vx + p.vy * p.vy)
    local maxsp = speed
    if sp > maxsp then
      p.vx = p.vx / sp * maxsp
      p.vy = p.vy / sp * maxsp
    end
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt
    -- 爆发分量：指数衰减（每帧 0.9^f），衰减完归零。
    if p.bvx ~= 0 or p.bvy ~= 0 then
      local damp = math.pow(0.9, math.min(2.0, dt * 60.0))
      p.x = p.x + p.bvx * dt
      p.y = p.y + p.bvy * dt
      p.bvx = p.bvx * damp
      p.bvy = p.bvy * damp
      if math.abs(p.bvx) < 2 and math.abs(p.bvy) < 2 then p.bvx, p.bvy = 0, 0 end
    end
    if d > 420 then
      p.x = cx + math.cos(p.phase) * 60
      p.y = cy + math.sin(p.phase) * 60
      p.vx, p.vy = 0, 0
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_FIREFLIES, math.floor(params.count or 49))
  local size = params.size or 2
  local glow = params.glow or 15
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#CCFF4D")
  local am = ca / 255

  for i = 1, n do
    local p = flies[i]
    local tw = 0.25 + 0.75 * (0.5 + 0.5 * math.sin(p.phase))
    if tw > 0.08 then
      local a = math.floor(225 * tw * am)
      ctx:radial_stops(p.x, p.y, glow * (0.55 + 0.45 * tw), {
        { offset = 0.0, r = cr, g = cg, b = cb, a = math.floor(70 * tw * am) },
        { offset = 0.45, r = cr, g = cg, b = cb, a = math.floor(24 * tw * am) },
        { offset = 1.0, r = cr, g = cg, b = cb, a = 0 },
      })
      ctx:plus_circle(p.x, p.y, size * (0.8 + 0.5 * tw), 255, 255, 210, a)
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
