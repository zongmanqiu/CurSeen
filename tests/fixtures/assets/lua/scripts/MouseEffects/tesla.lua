-- MouseEffects · Tesla
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针处的特斯拉线圈：中心光球向外放出高频抖动的电弧，电弧落地有亮点。
-- 上游 94 个参数/多种触发；这里保留弧数、半径、宽度、寿命、分叉、辉光六项。

name = "Tesla"
effect_name = "Tesla"
kind = "cursor"
description = "MouseEffects Tesla：围绕光标放电的特斯拉电弧。"
hide_cursor = true

local MAX_ARCS = 18
local arcs = {}
local timer = 0

function schema()
  return {
    { key = "arcs",     label = "电弧数",   type = "int",   min = 3,   max = 12,  step = 1,   default = 8 },
    { key = "radius",   label = "放电半径", type = "float", min = 60,  max = 220, step = 5,   default = 130 },
    { key = "width",    label = "电弧宽度", type = "float", min = 0.5, max = 4,   step = 0.1, default = 1.6 },
    { key = "life",     label = "电弧寿命(秒)", type = "float", min = 0.06, max = 0.3, step = 0.02, default = 0.12 },
    { key = "branches", label = "分叉数",   type = "int",   min = 0,   max = 6,   step = 1,   default = 2 },
    { key = "glow",     label = "核心辉光", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.5 },
    { key = "color",    label = "电弧颜色", type = "color", default = "#66E0FF" },
  }
end

function init()
  arcs = {}
  timer = 0
end

local function make_arc(x, y, angle, dist, params)
  local segs = {}
  local px, py = x, y
  local nx, ny = math.cos(angle), math.sin(angle)
  local steps = 8
  for i = 1, steps do
    local t = i / steps
    local tx = x + math.cos(angle) * dist * t
    local ty = y + math.sin(angle) * dist * t
    local jitter = (i == steps) and 0 or rng(-1, 1) * 10
    tx = tx - ny * jitter
    ty = ty + nx * jitter
    segs[#segs + 1] = { px, py, tx, ty }
    px, py = tx, ty
  end
  local branches = math.floor(params.branches or 2)
  for b = 1, branches do
    local idx = math.max(1, math.floor(rng(2, steps - 1)))
    local s = segs[idx]
    local ba = angle + rng(-1.1, 1.1)
    local bl = dist * rng(0.10, 0.25)
    segs[#segs + 1] = { s[3], s[4], s[3] + math.cos(ba) * bl, s[4] + math.sin(ba) * bl }
  end
  arcs[#arcs + 1] = {
    segs = segs, age = 0, life = params.life or 0.12,
    width = params.width or 1.6, ex = px, ey = py,
  }
  while #arcs > MAX_ARCS do table.remove(arcs, 1) end
end

function step(dt, t, params)
  params = params or {}
  timer = timer + dt
  if timer >= (params.life or 0.12) * 1.4 then
    timer = 0
    local n = math.floor(params.arcs or 8)
    local R = params.radius or 130
    for i = 1, n do
      make_arc(mouse_x or 0, mouse_y or 0, rng(0, math.pi * 2), R * rng(0.65, 1.0), params)
    end
  end
  for i = #arcs, 1, -1 do
    local a = arcs[i]
    a.age = a.age + dt
    if a.age >= a.life then table.remove(arcs, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#66E0FF")
  local am = ca / 255
  local mx, my = mouse_x or 0, mouse_y or 0
  for i = 1, #arcs do
    local a = arcs[i]
    local k = 1 - a.age / a.life
    local alpha = math.floor(235 * k * am)
    for s = 1, #a.segs do
      local g = a.segs[s]
      ctx:line(g[1], g[2], g[3], g[4], a.width, cr, cg, cb, alpha)
      ctx:line(g[1], g[2], g[3], g[4], a.width * 2.8, cr, cg, cb, math.floor(alpha * 0.16))
    end
    ctx:plus_circle(a.ex, a.ey, a.width * 2.0, 255, 255, 255, alpha)
  end
  local glow = params.glow or 1.5
  ctx:glow(mx, my, 6 + 10 * glow, cr, cg, cb, math.floor(90 * math.min(1.0, glow) * am))
  ctx:plus_circle(mx, my, 3.0, 255, 255, 255, math.floor(235 * am))

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
