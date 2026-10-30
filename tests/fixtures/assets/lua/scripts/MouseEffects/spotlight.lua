-- MouseEffects · Spotlight
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 全屏压暗、指针处留一个柔边光孔，光孔里漂着微小尘埃。
-- 上游用屏幕捕获做亮度提升；CurSeen 用 base 层暗罩 + clear_circle 挖孔，
-- 加一点尘埃与色温选择，得到"舞台追光"的观感。

name = "Spotlight"
effect_name = "Spotlight"
kind = "cursor"
description = "MouseEffects Spotlight：跟随指针的柔边聚光灯与尘埃。"
hide_cursor = true

local px, py = 0, 0
local vis = 0
local motes = {}
local started = false

function schema()
  return {
    { key = "radius",      label = "光圈半径", type = "float", min = 80,  max = 300, step = 5,   default = 180 },
    { key = "softness",    label = "边缘柔度", type = "float", min = 20,  max = 160, step = 5,   default = 80 },
    { key = "darkness",    label = "暗罩浓度", type = "float", min = 0.3, max = 0.95, step = 0.05, default = 0.7 },
    { key = "dust",        label = "尘埃密度", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.5 },
    { key = "temperature", label = "色温",     type = "choice", choices = { "warm", "neutral", "cool" },
      labels = { "暖光", "中性", "冷光" }, default = "warm" },
  }
end

function init()
  px, py = 0, 0
  vis = 0
  started = false
  motes = {}
  for i = 1, 36 do
    motes[i] = {
      angle = rng(0, math.pi * 2),
      dist = rng(0.05, 1.0),
      speed = rng(-0.3, 0.3),
      size = rng(0.6, 1.8),
      phase = rng(0, math.pi * 2),
    }
  end
end

function step(dt, t, params)
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  if not started then
    px, py = mx, my
    started = true
  end
  local f = math.min(2.0, dt * 60.0)
  local k = math.min(1.0, 0.30 * f)
  px = px + (mx - px) * k
  py = py + (my - py) * k
  vis = vis + (1.0 - vis) * math.min(1.0, 0.10 * f)
end

function draw(ctx, params)
  if not started or vis < 0.01 then return end
  params = params or {}
  local radius = params.radius or 180
  local soft = math.min(radius * 0.9, params.softness or 80)
  local dark = params.darkness or 0.7
  local dust = params.dust or 0.5
  local temp = params.temperature or "warm"
  local tr, tg, tb = 255, 236, 200
  if temp == "cool" then
    tr, tg, tb = 190, 215, 255
  elseif temp == "neutral" then
    tr, tg, tb = 255, 255, 255
  end

  local a = math.floor(255 * dark * vis)
  ctx:layer("base")
  ctx:rect(screen_x or 0, screen_y or 0, screen_w or 1920, screen_h or 1080, 0, 0, 0, a)
  ctx:clear_circle(px, py, radius)
  ctx:radial_stops(px, py, radius, {
    { offset = 0.0, r = 0, g = 0, b = 0, a = 0 },
    { offset = math.max(0.02, 1.0 - soft / radius), r = 0, g = 0, b = 0, a = 0 },
    { offset = 1.0, r = 0, g = 0, b = 0, a = a },
  })

  ctx:layer("top")
  if dust > 0.01 then
    local n = math.floor(6 + 30 * dust)
    local t = elapsed or 0
    for i = 1, n do
      local m = motes[i]
      local ang = m.angle + t * m.speed
      local d = radius * m.dist * (0.95 + 0.05 * math.sin(t + m.phase))
      local x = px + math.cos(ang) * d
      local y = py + math.sin(ang) * d
      local tw = 0.55 + 0.45 * math.sin(t * 3.0 + m.phase)
      ctx:plus_circle(x, y, m.size, tr, tg, tb, math.floor(150 * tw * dust))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
