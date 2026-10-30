-- MouseEffects · Dandelion Seeds
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针周围漂浮的蒲公英种子：一根细茎 + 三根冠毛 + 一点亮芯，随风上浮/侧移。
-- 上游 11 个参数；这里保留数量、尺寸、浮速、风力、寿命、辉光六项。

name = "Dandelion Seeds"
effect_name = "Dandelion Seeds"
kind = "cursor"
description = "MouseEffects Dandelion Seeds：围光标漂浮的蒲公英种子。"
hide_cursor = true

local MAX_SEEDS = 40
local seeds = {}
local cx, cy = 0, 0
local started = false

function schema()
  return {
    { key = "count", label = "种子数",   type = "int",   min = 8,  max = 40,  step = 2,   default = 20 },
    { key = "size",  label = "种子大小", type = "float", min = 8,  max = 30,  step = 1,   default = 16 },
    { key = "float", label = "上浮速度", type = "float", min = 10, max = 80,  step = 5,   default = 40 },
    { key = "wind",  label = "风力",     type = "float", min = 0,  max = 100, step = 5,   default = 50 },
    { key = "life",  label = "寿命(秒)", type = "float", min = 3,  max = 20,  step = 1,   default = 12 },
    { key = "glow",  label = "辉光强度", type = "float", min = 0,  max = 2,   step = 0.1, default = 0.5 },
    { key = "color", label = "种子颜色", type = "color", default = "#F5F0E1" },
  }
end

function init()
  seeds = {}
  for i = 1, MAX_SEEDS do
    seeds[i] = {
      dx = rng(-220, 220),
      dy = rng(-200, 220),
      phase = rng(0, math.pi * 2),
      rot = rng(0, math.pi * 2),
      size = rng(0.7, 1.3),
      age = rng(0, 6),
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
  local k = math.min(1.0, 0.09 * f)
  cx = cx + (mx - cx) * k
  cy = cy + (my - cy) * k

  local up = params.float or 40
  local wind = params.wind or 50
  local life = params.life or 12
  for i = 1, #seeds do
    local s = seeds[i]
    s.age = s.age + dt
    s.phase = s.phase + dt * 1.2
    s.rot = s.rot + dt * 0.5
    s.dy = s.dy - up * dt
    s.dx = s.dx + (wind * 0.35 + math.sin(s.phase) * 16) * dt
    if s.dy < -280 then
      -- 飞出上边界只换位置，不重置年龄：否则 life 会被"飞出去"抢先触发，
      -- 高值区间（12–20s）几乎看不到作用。
      s.dy = 280
      s.dx = rng(-220, 220)
    end
    if s.age >= life then
      s.dy = 280
      s.dx = rng(-220, 220)
      s.age = 0
    end
    if s.dx > 320 then s.dx = -300 elseif s.dx < -320 then s.dx = 300 end
  end
end

function draw(ctx, params)
  params = params or {}
  local n = math.min(MAX_SEEDS, math.floor(params.count or 20))
  local base = params.size or 16
  local glow = params.glow or 0.5
  local life = params.life or 12
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#F5F0E1")
  local am = ca / 255

  for i = 1, n do
    local s = seeds[i]
    local x = cx + s.dx
    local y = cy + s.dy
    local sz = base * s.size
    local ca, sa = math.cos(s.rot), math.sin(s.rot)
    local tx = x + math.sin(s.rot) * sz
    local ty = y - math.cos(s.rot) * sz
    local fade = math.min(1.0, math.max(0.0, (life - s.age) / 1.2))
    -- 细茎。
    ctx:line(x, y, tx, ty, 0.8, cr, cg, cb, math.floor(120 * fade * am))
    -- 三根冠毛。
    for k = -1, 1 do
      local a = s.rot + k * 0.55
      ctx:line(tx, ty, tx + math.sin(a) * sz * 0.55, ty - math.cos(a) * sz * 0.55,
        0.7, cr, cg, cb, math.floor(110 * fade * am))
    end
    -- 芯点。
    ctx:plus_circle(tx, ty, 1.2, 255, 255, 255, math.floor(200 * fade * am))
    if glow > 0.01 and i % 3 == 0 then
      ctx:glow(tx, ty, 3 + 4 * glow, cr, cg, cb, math.floor(40 * glow * fade * am))
    end
  end

  -- 光标定位点：黑外圈 + 白内芯，浅色/深色桌面都能看见（效果本身会隐藏系统指针）。
  local pmx, pmy = mouse_x or 0, mouse_y or 0
  ctx:circle(pmx, pmy, 6.2, 0, 0, 0, 230)
  ctx:circle(pmx, pmy, 3.4, 255, 255, 255, 255)

end
