-- MouseEffects · Crystal Growth
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击处向外生长的冰晶：主枝沿随机方向伸长，按概率分叉，枝端有亮核与闪光。
-- 上游 5 支/3 代；这里保持同样结构，段数封顶 160。

name = "Crystal Growth"
effect_name = "Crystal Growth"
kind = "click"
description = "MouseEffects Crystal Growth：从点击点分叉生长的冰晶。"

local MAX_SEGMENTS = 160
local MAX_POINTS = 30
local branches = {}

function schema()
  return {
    { key = "branches", label = "主枝数",   type = "int",   min = 3,   max = 10,  step = 1,   default = 5 },
    { key = "growth",   label = "生长速度", type = "float", min = 40,  max = 200, step = 10,  default = 120 },
    { key = "size",     label = "最大长度", type = "float", min = 40,  max = 160, step = 10,  default = 100 },
    { key = "branch",   label = "分叉概率", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.55 },
    { key = "width",    label = "枝宽",     type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.4 },
    { key = "life",     label = "寿命(秒)", type = "float", min = 0.8, max = 4,   step = 0.1, default = 2.2 },
    { key = "color",    label = "晶体颜色", type = "color", default = "#88CEEB" },
  }
end

function init()
  branches = {}
end

local function add_branch(x, y, angle, length, width, gen, life)
  if #branches >= MAX_SEGMENTS then return end
  branches[#branches + 1] = {
    pts = { { x, y } },
    hx = x, hy = y,
    angle = angle,
    remain = length,
    width = width,
    gen = gen or 0,
    age = 0,
    life = life,
    grow = 0,
    seg = 0,
  }
end

function on_press(x, y, t, params)
  params = params or {}
  branches = {}
  local n = math.floor(params.branches or 5)
  local size = params.size or 100
  local life = params.life or 2.2
  for i = 1, n do
    local a = i * math.pi * 2 / n + rng(-0.25, 0.25)
    add_branch(x, y, a, size * rng(0.7, 1.0), params.width or 1.4, 0, life * rng(0.8, 1.1))
  end
end

function step(dt, t, params)
  params = params or {}
  local speed = params.growth or 120
  local branchP = params.branch or 0.55
  local maxLen = params.size or 100
  for i = #branches, 1, -1 do
    local b = branches[i]
    b.age = b.age + dt
    local move = math.min(speed * dt, b.remain)
    b.remain = b.remain - move
    b.grow = b.grow + move
    b.angle = b.angle + math.sin((elapsed or 0) * 2.0 + i) * 0.02
    local nx = b.hx + math.cos(b.angle) * move
    local ny = b.hy + math.sin(b.angle) * move
    b.hx, b.hy = nx, ny
    b.seg = b.seg + move
    if b.seg >= 8 then
      b.seg = 0
      b.pts[#b.pts + 1] = { nx, ny }
      if #b.pts > MAX_POINTS then table.remove(b.pts, 1) end
    end

    if b.grow > 18 and b.gen < 2 then
      b.grow = 0
      if rng(0, 1) < branchP and #branches < MAX_SEGMENTS then
        local na = b.angle + (rng(0, 1) < 0.5 and 0.7 or -0.7) + rng(-0.2, 0.2)
        add_branch(nx, ny, na, maxLen * rng(0.35, 0.6), b.width * 0.65, b.gen + 1, b.life)
      end
    end

    if b.remain <= 0.01 or b.age >= b.life then
      table.remove(branches, i)
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#88CEEB")
  local am = ca / 255
  local width = params.width or 1.4
  for i = 1, #branches do
    local b = branches[i]
    local fade = math.min(1.0, math.max(0.0, (b.life - b.age) / 0.5))
    local a = math.floor(225 * fade * am)
    for k = 2, #b.pts do
      local p0, p1 = b.pts[k - 1], b.pts[k]
      ctx:line(p0[1], p0[2], p1[1], p1[2], b.width, cr, cg, cb, a)
    end
    local last = b.pts[#b.pts]
    ctx:line(last[1], last[2], b.hx, b.hy, b.width, cr, cg, cb, a)
    ctx:plus_circle(b.hx, b.hy, width * 1.8, 255, 255, 255, math.floor(a * 0.9))
    ctx:glow(b.hx, b.hy, 6 + 8 * fade, cr, cg, cb, math.floor(60 * fade * am))
  end
end
