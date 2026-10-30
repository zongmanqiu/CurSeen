-- MouseEffects · Lightning Storm
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 光标周围周期性劈下锯齿状闪电：主雷 + 分叉，落点有亮核，短促淡出。
-- 上游可按点击/移动触发；这里做成常驻风暴，保持"闪电随光标"的观感。

name = "Lightning Storm"
effect_name = "Lightning Storm"
kind = "cursor"
description = "MouseEffects Lightning Storm：围绕光标劈落的闪电。"
hide_cursor = true

local MAX_BOLTS = 10
local bolts = {}
local timer = 0

function schema()
  return {
    { key = "interval", label = "劈落间隔", type = "float", min = 0.15, max = 2,   step = 0.05, default = 0.5 },
    { key = "bolts",    label = "每次雷数", type = "int",   min = 1,    max = 8,   step = 1,    default = 3 },
    { key = "distance", label = "落点距离", type = "float", min = 80,   max = 400, step = 10,   default = 250 },
    { key = "width",    label = "雷弧宽度", type = "float", min = 0.5,  max = 5,   step = 0.5,  default = 2.5 },
    { key = "branches", label = "分叉数",   type = "int",   min = 0,    max = 8,   step = 1,    default = 3 },
    { key = "life",     label = "存在时间(秒)", type = "float", min = 0.08, max = 0.5, step = 0.02, default = 0.2 },
    { key = "color",    label = "闪电颜色", type = "color", default = "#66CCFF" },
  }
end

function init()
  bolts = {}
  timer = 0
end

local function make_bolt(x, y, angle, dist, params)
  local segs = {}
  local px, py = x, y
  local steps = 9
  local nx, ny = math.cos(angle), math.sin(angle)
  for i = 1, steps do
    local t = i / steps
    local tx = x + math.cos(angle) * dist * t
    local ty = y + math.sin(angle) * dist * t
    local jitter = (i == steps) and 0 or rng(-1, 1) * dist * 0.10
    tx = tx - ny * jitter
    ty = ty + nx * jitter
    segs[#segs + 1] = { px, py, tx, ty }
    px, py = tx, ty
  end
  local branchN = math.floor(params.branches or 3)
  for b = 1, branchN do
    local idx = math.max(1, math.floor(rng(2, steps - 1)))
    local s = segs[idx]
    local ba = angle + rng(-0.9, 0.9)
    local bx, by = s[3], s[4]
    local bl = dist * rng(0.12, 0.32)
    segs[#segs + 1] = { bx, by, bx + math.cos(ba) * bl, by + math.sin(ba) * bl }
  end
  bolts[#bolts + 1] = {
    segs = segs,
    age = 0,
    life = params.life or 0.2,
    width = params.width or 2.5,
    x = px, y = py,
  }
  while #bolts > MAX_BOLTS do table.remove(bolts, 1) end
end

function step(dt, t, params)
  params = params or {}
  timer = timer + dt
  if timer >= (params.interval or 0.5) then
    timer = timer - (params.interval or 0.5)
    local n = math.floor(params.bolts or 3)
    for i = 1, n do
      local a = rng(0, math.pi * 2)
      make_bolt(mouse_x or 0, mouse_y or 0, a, (params.distance or 250) * rng(0.5, 1.0), params)
    end
  end
  for i = #bolts, 1, -1 do
    local b = bolts[i]
    b.age = b.age + dt
    if b.age >= b.life then table.remove(bolts, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#66CCFF")
  local am = ca / 255
  for i = 1, #bolts do
    local b = bolts[i]
    local k = 1 - b.age / b.life
    local a = math.floor(240 * k * am)
    for s = 1, #b.segs do
      local g = b.segs[s]
      ctx:line(g[1], g[2], g[3], g[4], b.width * 2.6, cr, cg, cb, math.floor(a * 0.20))
      ctx:line(g[1], g[2], g[3], g[4], b.width, cr, cg, cb, a)
      ctx:line(g[1], g[2], g[3], g[4], b.width * 0.8, 255, 255, 255, a)
    end
    ctx:plus_circle(b.x, b.y, b.width * 2.2, 255, 255, 255, a)
    ctx:glow(b.x, b.y, 8, cr, cg, cb, math.floor(70 * k * am))
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
