-- MouseEffects · Pixel Explosion
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 点击爆开的复古像素块：方形碎片带旋转、重力和阻力，颜色从街机调色板取。
-- 上游同时在移动时撒像素；CurSeen 的 click 通道只保留点击爆发（README 注明）。

name = "Pixel Explosion"
effect_name = "Pixel Explosion"
kind = "click"
description = "MouseEffects Pixel Explosion：点击爆开的像素方块。"

local MAX_PIXELS = 240
local pixels = {}

local PALETTE = {
  { 255, 64, 64 }, { 255, 160, 32 }, { 255, 230, 64 }, { 64, 220, 96 },
  { 64, 160, 255 }, { 180, 96, 255 }, { 255, 96, 200 },
}

function schema()
  return {
    { key = "count",   label = "像素数",   type = "int",   min = 20,  max = 80,  step = 5,   default = 40 },
    { key = "size",    label = "像素大小", type = "float", min = 3,   max = 12,  step = 0.5, default = 6 },
    { key = "force",   label = "爆散力",   type = "float", min = 150, max = 600, step = 10,  default = 350 },
    { key = "gravity", label = "重力",     type = "float", min = 100, max = 600, step = 10,  default = 250 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 0.8, max = 3,   step = 0.1, default = 1.8 },
    { key = "rainbow", label = "调色板",   type = "bool",  default = true },
    { key = "color",   label = "单色",     type = "color", default = "#FFFFFF" },
  }
end

function init()
  pixels = {}
end

function on_press(x, y, t, params)
  params = params or {}
  local n = math.floor(params.count or 40)
  local force = params.force or 350
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFFFFF")
  for i = 1, n do
    local a = rng(0, math.pi * 2)
    local sp = force * rng(0.3, 1.0)
    local c = PALETTE[(i - 1) % #PALETTE + 1]
    if params.rainbow == false then c = { cr, cg, cb } end
    pixels[#pixels + 1] = {
      x = x, y = y,
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp - force * 0.2,
      size = (params.size or 6) * rng(0.6, 1.3),
      rot = rng(0, math.pi * 2),
      spin = rng(-10, 10),
      age = 0,
      life = (params.life or 1.8) * rng(0.7, 1.2),
      color = c,
      alpha_mul = ca / 255,
    }
  end
  while #pixels > MAX_PIXELS do table.remove(pixels, 1) end
end

function step(dt, t, params)
  params = params or {}
  local gravity = params.gravity or 250
  for i = #pixels, 1, -1 do
    local p = pixels[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(pixels, i)
    else
      p.vy = p.vy + gravity * dt
      p.vx = p.vx * math.pow(0.92, dt * 60)
      p.vy = p.vy * math.pow(0.985, dt * 60)
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.rot = p.rot + p.spin * dt
    end
  end
end

function draw(ctx, params)
  params = params or {}
  for i = 1, #pixels do
    local p = pixels[i]
    local k = 1 - p.age / p.life
    local ca, sa = math.cos(p.rot), math.sin(p.rot)
    local s = p.size * (0.6 + 0.4 * k)
    local pts = {
      { p.x + (-s) * ca - (-s) * sa, p.y + (-s) * sa + (-s) * ca },
      { p.x + (s) * ca - (-s) * sa, p.y + (s) * sa + (-s) * ca },
      { p.x + (s) * ca - (s) * sa, p.y + (s) * sa + (s) * ca },
      { p.x + (-s) * ca - (s) * sa, p.y + (-s) * sa + (s) * ca },
    }
    ctx:polygon(pts, p.color[1], p.color[2], p.color[3], math.floor(235 * k * p.alpha_mul))
  end
end
