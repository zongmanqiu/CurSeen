-- MouseEffects · Snowfall
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
-- 指针周围飘落的雪花：六臂细线 + 亮芯，边落边摆；大雪花才画辉光。

name = "Snowfall"
effect_name = "Snowfall"
kind = "cursor"
description = "MouseEffects Snowfall：围绕光标飘落的六角雪花。"
hide_cursor = true

local MAX_FLAKES = 120
local flakes = {}
local cx, cy = 0, 0
local started = false

function schema()
  return {
    { key = "count", label = "雪花数",   type = "int",   min = 15,  max = 120, step = 5,   default = 50 },
    { key = "size",  label = "雪花大小", type = "float", min = 3,   max = 16,  step = 0.5, default = 8 },
    { key = "fall",  label = "下落速度", type = "float", min = 20,  max = 120, step = 5,   default = 60 },
    { key = "wind",  label = "风力",     type = "float", min = 0,   max = 80,  step = 5,   default = 30 },
    { key = "sway",  label = "摆动幅度", type = "float", min = 0,   max = 60,  step = 5,   default = 20 },
    { key = "glow",  label = "辉光强度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.5 },
    { key = "color", label = "雪花颜色", type = "color", default = "#EAF8FF" },
  }
end

function init()
  flakes = {}
  for i = 1, MAX_FLAKES do
    flakes[i] = {
      dx = rng(-300, 300),
      dy = rng(-300, 300),
      phase = rng(0, math.pi * 2),
      size = rng(0.5, 1.4),
      spin = rng(0, math.pi * 2),
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

  local fall = params.fall or 60
  local wind = params.wind or 30
  local sway = params.sway or 20
  for i = 1, #flakes do
    local s = flakes[i]
    s.phase = s.phase + dt * 1.4
    s.spin = s.spin + dt * 0.6
    s.dy = s.dy + fall * dt
    s.dx = s.dx + (wind * 0.35 + math.sin(s.phase) * sway) * dt
    if s.dy > 320 then
      s.dy = -300
      s.dx = rng(-300, 300)
    end
    if s.dx > 340 then s.dx = -320 elseif s.dx < -340 then s.dx = 320 end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_FLAKES, math.floor(params.count or 50))
  local base = params.size or 8
  local glow = params.glow or 0.5
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#EAF8FF")
  local am = ca / 255

  for i = 1, n do
    local s = flakes[i]
    local x = cx + s.dx
    local y = cy + s.dy
    local r = base * s.size
    local ca, sa = math.cos(s.spin), math.sin(s.spin)
    local a = math.floor((120 + 100 * (0.5 + 0.5 * math.sin(s.phase))) * am)
    for k = 0, 2 do
      local ang = s.spin + k * math.pi / 3
      ctx:line(x - math.cos(ang) * r, y - math.sin(ang) * r,
               x + math.cos(ang) * r, y + math.sin(ang) * r,
               0.9, cr, cg, cb, a)
    end
    ctx:plus_circle(x, y, 1.2, 255, 255, 255, math.floor(a * 0.9))
    if glow > 0.01 and r > base * 0.95 then
      ctx:glow(x, y, r * 0.8, cr, cg, cb, math.floor(35 * glow * am))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
