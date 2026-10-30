-- CursorFX · shockwave
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：按下点先闪一下白光，随即撑开一圈粗环，同时向四周甩出短促的火星。
-- 环、闪光与火星都按寿命归一化淡出；火星用“当前点 → 速度反方向”的短线。
-- 用 on_press 写，复制到 ClickFx/ 仍按点击宿主运行；放进包卡时由 Companion 的
-- on_mouse_down → on_press 兼容分支驱动。

name = "shockwave"
effect_name = "shockwave"
kind = "click"
description = "CursorFX shockwave：白光、冲击环与火星的爆炸冲击波。"

local MAX_RINGS = 8
local MAX_SPARKS = 220

local rings = {}
local sparks = {}

function schema()
  return {
    { key = "color",    label = "颜色",     type = "color", alpha = true, default = "#FFC878" },
    { key = "sparks",   label = "火星数",   type = "int",   min = 6,   max = 80,  step = 1,    default = 30 },
    { key = "radius",   label = "冲击半径", type = "float", min = 40,  max = 320, step = 5,    default = 160 },
    { key = "duration", label = "扩散时长", type = "float", min = 0.25, max = 1.5, step = 0.01, default = 0.67 },
  }
end

function init()
  rings = {}
  sparks = {}
end

local function a8(base, k)
  local v = math.floor(base * k + 0.5)
  if v < 0 then v = 0 elseif v > 255 then v = 255 end
  return v
end

function on_press(x, y, t, params)
  local dur = params.duration or 0.67
  rings[#rings + 1] = { x = x, y = y, age = 0, dur = dur }
  while #rings > MAX_RINGS do
    table.remove(rings, 1)
  end

  local n = math.floor(params.sparks or 30)
  if n < 1 then n = 1 elseif n > 80 then n = 80 end
  for i = 1, n do
    local ang = rng(0, 6.2831853)
    local sp = rng(180, 720)
    local life = rng(0.33, 0.75)
    sparks[#sparks + 1] = {
      x = x,
      y = y,
      vx = math.cos(ang) * sp,
      vy = math.sin(ang) * sp,
      life = life,
      max = life,
      len = rng(4, 12),
    }
  end
  while #sparks > MAX_SPARKS do
    table.remove(sparks, 1)
  end
end

function step(dt, t, params)
  local ds = dt or 0
  if ds > 0.064 then ds = 0.064 end
  if ds <= 0 then return end

  local rn = #rings
  local rw = 0
  for i = 1, rn do
    local r = rings[i]
    r.age = r.age + ds
    if r.age < r.dur then
      rw = rw + 1
      rings[rw] = r
    end
  end
  for i = #rings, rw + 1, -1 do
    rings[i] = nil
  end

  local damp = (0.9) ^ (ds * 60.0)
  local n = #sparks
  local w = 0
  for i = 1, n do
    local p = sparks[i]
    p.vx = p.vx * damp
    p.vy = p.vy * damp
    p.x = p.x + p.vx * ds
    p.y = p.y + p.vy * ds
    p.life = p.life - ds
    if p.life > 0 then
      w = w + 1
      sparks[w] = p
    end
  end
  for i = #sparks, w + 1, -1 do
    sparks[i] = nil
  end
end

function draw(ctx, params)
  if #rings == 0 and #sparks == 0 then return end

  local r, g, b, ca = hex_to_rgb(params.color or "#FFC878")
  local radius = params.radius or 160

  for i = 1, #rings do
    local ring = rings[i]
    local u = ring.age / ring.dur
    if u < 1.0 then
      local fade = 1.0 - u
      local rad = 10.0 + u * radius
      local width = 14.0 * fade + 1.0
      local al = a8(ca, fade)
      if al > 0 then
        ctx:ring(ring.x, ring.y, rad + width * 0.5, width, r, g, b, al)
      end
      if u < 0.3 then
        local fa = a8(ca, (0.3 - u) * 2.0)
        if fa > 0 then
          local flash = {
            { offset = 0.0,  r = 255, g = 255, b = 255, a = fa },
            { offset = 0.4,  r = r,   g = g,   b = b,   a = a8(ca, (0.3 - u) * 1.4) },
            { offset = 1.0,  r = r,   g = g,   b = b,   a = 0 },
          }
          ctx:radial_stops(ring.x, ring.y, 90, flash)
        end
      end
    end
  end

  for i = 1, #sparks do
    local p = sparks[i]
    local t = p.life / p.max
    local al = a8(ca, t)
    if al > 0 then
      local v = math.sqrt(p.vx * p.vx + p.vy * p.vy)
      if v < 0.001 then v = 1.0 end
      ctx:line(
        p.x, p.y,
        p.x - p.vx / v * p.len,
        p.y - p.vy / v * p.len,
        2.0 * t + 0.5,
        r, g, b, al
      )
      ctx:plus_circle(p.x, p.y, 1.2 * t + 0.2, r, g, b, a8(ca, t * 0.7))
    end
  end
end
