-- MouseEffects · Starfield Warp
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 以指针为中心的跃迁星场：星点从中心向外加速，拖出长短不一的条纹；
-- 近处星更亮更大。上游是 500 颗 GPU 实例；这里按软渲染压到 160 颗。

name = "Starfield Warp"
effect_name = "Starfield Warp"
kind = "cursor"
description = "MouseEffects Starfield Warp：从光标向外跃迁的星场。"
hide_cursor = true

local MAX_STARS = 260
local stars = {}
local cx, cy = 0, 0
local started = false

function schema()
  return {
    { key = "stars",      label = "星点数",   type = "int",   min = 80,  max = 260, step = 10, default = 160 },
    { key = "speed",      label = "跃迁速度", type = "float", min = 0.1, max = 3,   step = 0.1, default = 1.0 },
    { key = "radius",     label = "星场半径", type = "float", min = 200, max = 800, step = 20, default = 600 },
    { key = "streak",     label = "拖尾长度", type = "float", min = 0.1, max = 2,   step = 0.1, default = 0.5 },
    { key = "brightness", label = "亮度",     type = "float", min = 0.1, max = 3,   step = 0.1, default = 1.2 },
    { key = "color",      label = "星光颜色", type = "color", default = "#99CCFF" },
  }
end

function init()
  stars = {}
  for i = 1, MAX_STARS do
    stars[i] = {
      angle = rng(0, math.pi * 2),
      depth = rng(0.15, 1.0),
      dist = rng(0.02, 1.0),
      prev = 0,
      tw = rng(0, math.pi * 2),
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
  local k = math.min(1.0, 0.18 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local speed = params.speed or 1.0
  for i = 1, #stars do
    local s = stars[i]
    s.prev = s.dist
    s.dist = s.dist + (0.05 + s.depth * 0.22) * speed * dt * 3.2
    if s.dist > 1.05 then
      s.dist = rng(0.02, 0.10)
      s.prev = s.dist
      s.angle = rng(0, math.pi * 2)
      s.depth = rng(0.15, 1.0)
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_STARS, math.floor(params.stars or 160))
  local R = params.radius or 600
  local streak = params.streak or 0.5
  local bright = params.brightness or 1.2
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#99CCFF")
  local am = ca / 255

  for i = 1, n do
    local s = stars[i]
    local r0 = math.max(1.5, s.prev * R)
    local r1 = s.dist * R
    local x0 = cx + math.cos(s.angle) * r0
    local y0 = cy + math.sin(s.angle) * r0 * 0.92
    local x1 = cx + math.cos(s.angle) * r1
    local y1 = cy + math.sin(s.angle) * r1 * 0.92
    local tail = math.max(1.5, (r1 - r0) * (0.6 + streak * 1.8))
    local tx = x1 - math.cos(s.angle) * tail
    local ty = y1 - math.sin(s.angle) * tail * 0.92
    local a = math.floor(230 * math.min(1.0, bright) * (0.25 + s.depth * 0.75) * am)
    local w = 0.8 + s.depth * 2.2
    ctx:line(tx, ty, x1, y1, w, cr, cg, cb, a)
    if s.depth > 0.62 then
      local tw = 0.7 + 0.3 * math.sin((elapsed or 0) * 4.0 + s.tw)
      ctx:plus_circle(x1, y1, 1.0 + s.depth * 2.0 * tw, 255, 255, 255,
        math.floor(220 * s.depth * tw * am))
    end
  end

  -- 中心留一个暗核，强化"向远处跃迁"的隧道感。
  ctx:radial_stops(cx, cy, R * 0.16, {
    { offset = 0.0, r = 0, g = 0, b = 0, a = math.floor(80 * am) },
    { offset = 0.65, r = 0, g = 0, b = 0, a = math.floor(20 * am) },
    { offset = 1.0, r = 0, g = 0, b = 0, a = 0 },
  })

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
