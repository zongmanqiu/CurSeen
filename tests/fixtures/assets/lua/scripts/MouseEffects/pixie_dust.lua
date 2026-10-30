-- MouseEffects · Pixie Dust
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针移动时洒下的仙尘：小光点 + 四芒星 + 柔光，颜色可固定或彩虹循环。
-- 上游默认 10 粒/次、带点击爆发；这里保留移动撒尘，点击时多撒一小把。

name = "Pixie Dust"
effect_name = "Pixie Dust"
kind = "trail"
description = "MouseEffects Pixie Dust：跟随鼠标洒下的闪光仙尘。"

local MAX_DUST = 140
local dust = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",    label = "撒尘频率", type = "float", min = 5,   max = 40,  step = 1,   default = 26 },
    { key = "size",    label = "光点大小", type = "float", min = 4,   max = 24,  step = 1,   default = 14 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.8 },
    { key = "drift",   label = "飘移速度", type = "float", min = 10,  max = 80,  step = 5,   default = 30 },
    { key = "glow",    label = "辉光强度", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.2 },
    { key = "rainbow", label = "彩虹色",   type = "bool",  default = true },
    { key = "color",   label = "固定颜色", type = "color", default = "#FFCC33" },
  }
end

function init()
  dust = {}
  spawn_acc = 0
end

local function spawn(x, y, params)
  if #dust >= MAX_DUST then table.remove(dust, 1) end
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFCC33")
  local hue = rng(0, 360)
  if params.rainbow == false then
    dust[#dust + 1] = {
      x = x + rng(-8, 8), y = y + rng(-8, 8),
      vx = rng(-1, 1) * (params.drift or 30),
      vy = rng(-1, 1) * (params.drift or 30) - 10,
      age = 0, life = (params.life or 1.8) * rng(0.7, 1.3),
      size = (params.size or 14) * rng(0.5, 1.1),
      r = cr, g = cg, b = cb, alpha_mul = ca / 255,
      tw = rng(0, math.pi * 2),
    }
  else
    local r, g, b = hsv_to_rgb(hue, 1.0, 1.0)
    dust[#dust + 1] = {
      x = x + rng(-8, 8), y = y + rng(-8, 8),
      vx = rng(-1, 1) * (params.drift or 30),
      vy = rng(-1, 1) * (params.drift or 30) - 10,
      age = 0, life = (params.life or 1.8) * rng(0.7, 1.3),
      size = (params.size or 14) * rng(0.5, 1.1),
      r = r, g = g, b = b, alpha_mul = ca / 255,
      tw = rng(0, math.pi * 2),
    }
  end
end

function on_mouse_down(button, x, y, params)
  if button ~= "left" then return end
  for _ = 1, 8 do spawn(x, y, params or {}) end
end

function step(dt, t, params)
  params = params or {}
  local speed = math.sqrt((mouse_vx or 0) ^ 2 + (mouse_vy or 0) ^ 2)
  if speed > 0.5 then
    spawn_acc = spawn_acc + (params.rate or 26) * dt
    while spawn_acc >= 1 do
      spawn_acc = spawn_acc - 1
      spawn(mouse_x or 0, mouse_y or 0, params)
    end
  end
  for i = #dust, 1, -1 do
    local p = dust[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(dust, i)
    else
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.vx = p.vx * 0.97
      p.vy = p.vy * 0.97
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local glow = params.glow or 1.2
  for i = 1, #dust do
    local p = dust[i]
    local k = 1 - p.age / p.life
    local tw = 0.65 + 0.35 * math.sin((elapsed or 0) * 8.0 + p.tw)
    local a = math.floor(235 * k * tw * p.alpha_mul)
    ctx:plus_circle(p.x, p.y, p.size * 0.32 * (0.6 + 0.4 * k), p.r, p.g, p.b, a)
    if glow > 0.6 and i % 3 == 0 then
      ctx:glow(p.x, p.y, p.size * 0.6, p.r, p.g, p.b, math.floor(55 * k * math.min(1.0, glow) * p.alpha_mul))
      local s = p.size * 0.8 * k
      ctx:line(p.x - s, p.y, p.x + s, p.y, 0.8, p.r, p.g, p.b, math.floor(a * 0.7))
      ctx:line(p.x, p.y - s, p.x, p.y + s, 0.8, p.r, p.g, p.b, math.floor(a * 0.7))
    end
  end
end
