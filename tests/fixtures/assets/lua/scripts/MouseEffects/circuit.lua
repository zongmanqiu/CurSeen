-- MouseEffects · Circuit
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针经过处生长出电路走线：每条线沿 0/90/180/270 度前进，随机 90 度拐弯与分叉，
-- 头部带小辉光。折点每 18px 落一个（不是每帧），长跑时总段数也不会膨胀。

name = "Circuit"
effect_name = "Circuit"
kind = "trail"
description = "MouseEffects Circuit：沿鼠标轨迹生长的电路走线。"

local MAX_TRACES = 18
local MAX_POINTS = 18
local traces = {}
local last_x, last_y = nil, nil

function schema()
  return {
    { key = "traces", label = "走线数",   type = "int",   min = 4,   max = 18,  step = 1,   default = 12 },
    { key = "speed",  label = "生长速度", type = "float", min = 50,  max = 300, step = 10,  default = 150 },
    { key = "length", label = "最大长度", type = "float", min = 50,  max = 300, step = 10,  default = 200 },
    { key = "branch", label = "分叉概率", type = "float", min = 0,   max = 1,   step = 0.05, default = 0.30 },
    { key = "width",  label = "线宽",     type = "float", min = 1,   max = 4,   step = 0.1, default = 1.8 },
    { key = "glow",   label = "辉光强度", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.0 },
    { key = "color",  label = "走线颜色", type = "color", default = "#00FF66" },
  }
end

function init()
  traces = {}
  last_x, last_y = nil, nil
end

local function new_trace(x, y, angle, params, generation)
  local dir = math.floor(angle / (math.pi / 2) + 0.5) % 4
  traces[#traces + 1] = {
    pts = { { x, y } },
    hx = x, hy = y,
    dir = dir,
    grow = 0,
    total = 0,
    width = (params.width or 1.8) * (generation > 0 and 0.7 or 1.0),
    life = 2.5 + rng(0, 2),
    age = 0,
    gen = generation or 0,
  }
  local cap = math.max(4, math.min(MAX_TRACES, math.floor(params.traces or 12)))
  while #traces > cap do table.remove(traces, 1) end
end

function on_move(x, y, t, params)
  params = params or {}
  if not last_x then
    last_x, last_y = x, y
    return
  end
  local dx, dy = x - last_x, y - last_y
  local dist = math.sqrt(dx * dx + dy * dy)
  if dist < (params.length or 200) * 0.22 then return end
  last_x, last_y = x, y
  new_trace(x, y, math.atan(dy, dx) + rng(-0.8, 0.8), params, 0)
end

function step(dt, t, params)
  params = params or {}
  local speed = params.speed or 150
  local maxLen = params.length or 200
  local branch = params.branch or 0.3

  for i = #traces, 1, -1 do
    local tr = traces[i]
    tr.age = tr.age + dt
    local move = speed * dt
    tr.total = tr.total + move
    tr.grow = tr.grow + move
    local a = tr.dir * math.pi / 2
    tr.hx = tr.hx + math.cos(a) * move
    tr.hy = tr.hy + math.sin(a) * move

    if tr.grow >= 18 then
      tr.grow = 0
      tr.pts[#tr.pts + 1] = { tr.hx, tr.hy }
      if #tr.pts > MAX_POINTS then table.remove(tr.pts, 1) end
      if rng(0, 1) < 0.55 then
        tr.dir = (tr.dir + (rng(0, 1) < 0.5 and 1 or 3)) % 4
      end
      if tr.gen < 2 and rng(0, 1) < branch and #traces < MAX_TRACES then
        new_trace(tr.hx, tr.hy,
          (tr.dir + (rng(0, 1) < 0.5 and 1 or 3)) * math.pi / 2, params, tr.gen + 1)
      end
    end

    if tr.total >= maxLen or tr.age >= tr.life then
      table.remove(traces, i)
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local width = params.width or 1.8
  local glow = params.glow or 1.0
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#00FF66")
  local am = ca / 255

  for i = 1, #traces do
    local tr = traces[i]
    local fade = math.min(1.0, math.max(0.0, (tr.life - tr.age) / 0.8))
    local a = math.floor(235 * fade * am)
    local pts = tr.pts
    for k = 2, #pts do
      local p0, p1 = pts[k - 1], pts[k]
      ctx:line(p0[1], p0[2], p1[1], p1[2], tr.width, cr, cg, cb, a)
    end
    if #pts > 0 then
      local last = pts[#pts]
      ctx:line(last[1], last[2], tr.hx, tr.hy, tr.width, cr, cg, cb, a)
    end
    ctx:plus_circle(tr.hx, tr.hy, width * 1.2, cr, cg, cb, a)
    if glow > 0.5 then
      ctx:glow(tr.hx, tr.hy, 2 + 2 * glow, cr, cg, cb,
        math.floor(35 * fade * math.min(1.0, glow) * am))
    end
  end
end
