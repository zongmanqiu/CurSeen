-- MouseEffects · Confetti
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击爆开的彩色纸片：矩形/圆片/细带三种形状，受重力、阻力与飘摆影响。
-- 上游同时在移动时撒纸片；CurSeen 的 click 通道只保留点击爆发（README 注明）。

name = "Confetti"
effect_name = "Confetti"
kind = "click"
description = "MouseEffects Confetti：点击爆开的彩色纸片。"

local MAX_PIECES = 240
local pieces = {}

local PALETTE = {
  { 255, 0, 0 }, { 255, 140, 0 }, { 255, 214, 0 }, { 51, 204, 51 },
  { 31, 143, 255 }, { 153, 51, 204 }, { 255, 105, 181 }, { 0, 207, 209 },
}

function schema()
  return {
    { key = "count",   label = "纸片数",   type = "int",   min = 20,  max = 120, step = 5,   default = 60 },
    { key = "size",    label = "纸片大小", type = "float", min = 4,   max = 16,  step = 1,   default = 8 },
    { key = "force",   label = "爆发力",   type = "float", min = 150, max = 600, step = 10,  default = 350 },
    { key = "gravity", label = "重力",     type = "float", min = 100, max = 500, step = 10,  default = 220 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 1.5, max = 5,   step = 0.1, default = 3.0 },
    { key = "rainbow", label = "七彩",     type = "bool",  default = true },
    { key = "color",   label = "单色",     type = "color", default = "#FF595E" },
  }
end

function init()
  pieces = {}
end

function on_press(x, y, t, params)
  params = params or {}
  local n = math.floor(params.count or 60)
  local base = params.size or 8
  local force = params.force or 350
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FF595E")
  for i = 1, n do
    local a = rng(0, math.pi * 2)
    local sp = force * rng(0.35, 1.0)
    local c = PALETTE[(i - 1) % #PALETTE + 1]
    if params.rainbow == false then c = { cr, cg, cb } end
    pieces[#pieces + 1] = {
      x = x, y = y,
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp - force * 0.25,
      rot = rng(0, math.pi * 2),
      spin = rng(-8, 8),
      size = base * rng(0.6, 1.2),
      shape = (i % 3),
      color = c,
      age = 0,
      life = (params.life or 3.0) * rng(0.75, 1.15),
      phase = rng(0, math.pi * 2),
      alpha_mul = ca / 255,
    }
  end
  while #pieces > MAX_PIECES do table.remove(pieces, 1) end
end

function step(dt, t, params)
  params = params or {}
  local gravity = params.gravity or 220
  for i = #pieces, 1, -1 do
    local p = pieces[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(pieces, i)
    else
      p.vy = p.vy + gravity * dt
      p.vx = p.vx * math.pow(0.90, dt * 60)
      p.vy = p.vy * math.pow(0.985, dt * 60)
      p.x = p.x + p.vx * dt + math.sin(p.phase + p.age * 4) * 12 * dt
      p.y = p.y + p.vy * dt
      p.rot = p.rot + p.spin * dt
    end
  end
end

function draw(ctx, params)
  params = params or {}
  for i = 1, #pieces do
    local p = pieces[i]
    local fade = math.min(1.0, (p.life - p.age) / 0.4)
    local a = math.floor(230 * fade * p.alpha_mul)
    local c = p.color
    if p.shape == 1 then
      ctx:plus_circle(p.x, p.y, p.size * 0.5, c[1], c[2], c[3], a)
    elseif p.shape == 2 then
      local ca, sa = math.cos(p.rot), math.sin(p.rot)
      ctx:line(p.x - ca * p.size, p.y - sa * p.size,
               p.x + ca * p.size, p.y + sa * p.size,
               2.2, c[1], c[2], c[3], a)
    else
      local ca, sa = math.cos(p.rot), math.sin(p.rot)
      local w, h = p.size * 0.55, p.size * 0.35
      local pts = {
        { p.x + (-w) * ca - (-h) * sa, p.y + (-w) * sa + (-h) * ca },
        { p.x + (w) * ca - (-h) * sa, p.y + (w) * sa + (-h) * ca },
        { p.x + (w) * ca - (h) * sa, p.y + (w) * sa + (h) * ca },
        { p.x + (-w) * ca - (h) * sa, p.y + (-w) * sa + (h) * ca },
      }
      ctx:polygon(pts, c[1], c[2], c[3], a)
    end
  end
end
