-- MouseEffects · Nebula
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 光标周围的一团星云：几层彩色径向渐变缓慢旋转，叠上细碎星点与中心辉光。
-- 上游是 4 层 GPU 噪声云；这里用错位旋转的渐变圆近似云团。

name = "Nebula"
effect_name = "Nebula"
kind = "cursor"
description = "MouseEffects Nebula：旋转的彩色星云与星点。"
hide_cursor = true

local blobs = {}
local stars = {}
local phase = 0
local cx, cy = 0, 0

function schema()
  return {
    { key = "radius",      label = "星云半径", type = "float", min = 160, max = 420, step = 10, default = 320 },
    { key = "layers",      label = "云层数",   type = "int",   min = 2,   max = 5,   step = 1,  default = 4 },
    { key = "swirl",       label = "旋转速度", type = "float", min = 0,   max = 2,   step = 0.1, default = 0.5 },
    { key = "starDensity", label = "星点密度", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.30 },
    { key = "glow",        label = "中心辉光", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.5 },
    { key = "alpha",       label = "不透明度", type = "float", min = 0.1, max = 0.9, step = 0.05, default = 0.70 },
    { key = "color1",      label = "主色",     type = "color", default = "#8B00FF" },
    { key = "color2",      label = "辅色",     type = "color", default = "#4068E0" },
    { key = "color3",      label = "点缀色",   type = "color", default = "#FF69B4" },
  }
end

function init()
  phase = 0
  cx, cy = mouse_x or 0, mouse_y or cy
  blobs = {}
  for i = 1, 9 do
    blobs[i] = {
      angle = rng(0, math.pi * 2),
      dist = rng(0.10, 0.72),
      size = rng(0.28, 0.55),
      speed = rng(-1, 1) * 0.9,
      tint = (i - 1) % 3 + 1,
    }
  end
  stars = {}
  for i = 1, 64 do
    stars[i] = {
      angle = rng(0, math.pi * 2),
      dist = rng(0.15, 1.05),
      size = rng(0.6, 1.9),
      speed = rng(-1, 1) * 0.22,
      tw = rng(0, math.pi * 2),
    }
  end
end

function step(dt, t, params)
  params = params or {}
  local f = math.min(2.0, dt * 60.0)
  local mx, my = mouse_x or cx, mouse_y or cy
  local k = math.min(1.0, 0.10 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k
  phase = phase + dt * (params.swirl or 0.5)
end

function draw(ctx, params)
  params = params or {}
  local radius = params.radius or 320
  local layers = math.max(2, math.min(5, math.floor(params.layers or 4)))
  local alpha0 = params.alpha or 0.70
  local c = {
    { hex_to_rgb(params.color1 or "#8B00FF") },
    { hex_to_rgb(params.color2 or "#4068E0") },
    { hex_to_rgb(params.color3 or "#FF69B4") },
  }

  local count = layers + 2
  for i = 1, count do
    local b = blobs[i]
    local a = b.angle + phase * b.speed
    local d = radius * b.dist
    local x = cx + math.cos(a) * d
    local y = cy + math.sin(a) * d * 0.72
    local br = radius * b.size * (0.85 + 0.15 * math.sin(phase * 1.7 + i))
    local col = c[b.tint]
    local am = col[4] / 255
    local a0 = math.floor(255 * alpha0 * 0.32 * am)
    local a1 = math.floor(255 * alpha0 * 0.10 * am)
    ctx:radial_stops(x, y, br, {
      { offset = 0.0, r = col[1], g = col[2], b = col[3], a = a0 },
      { offset = 0.45, r = col[1], g = col[2], b = col[3], a = a1 },
      { offset = 1.0, r = col[1], g = col[2], b = col[3], a = 0 },
    })
  end

  local glow = params.glow or 1.5
  local gr, gg, gb, ga = hex_to_rgb(params.color2 or "#4068E0")
  ctx:glow(cx, cy, 26 + 22 * glow, gr, gg, gb, math.floor(90 * math.min(1.0, alpha0 + 0.2) * ga / 255))

  local starN = math.floor(8 + 56 * (params.starDensity or 0.3))
  for i = 1, math.min(starN, #stars) do
    local s = stars[i]
    local a = s.angle + phase * s.speed
    local d = radius * s.dist
    local x = cx + math.cos(a) * d
    local y = cy + math.sin(a) * d * 0.8
    local tw = 0.55 + 0.45 * math.sin(phase * 3.0 + s.tw)
    ctx:plus_circle(x, y, s.size * (0.8 + 0.4 * tw), 255, 255, 255,
      math.floor(150 * tw * (0.5 + alpha0 * 0.5)))
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
