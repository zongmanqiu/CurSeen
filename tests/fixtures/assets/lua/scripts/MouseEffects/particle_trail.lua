-- MouseEffects · Particle Trail
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针移动时喷出的彩色粒子：初速沿运动反方向散开，受阻力/重力影响，
-- 颜色从起始色渐变到结束色。上游 1000 粒 GPU；这里按软渲染压到 220。

name = "Particle Trail"
effect_name = "Particle Trail"
kind = "trail"
description = "MouseEffects Particle Trail：跟随鼠标的渐变色粒子。"

local MAX_PARTICLES = 300
local particles = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",       label = "发射频率", type = "float", min = 20,  max = 200, step = 5,   default = 100 },
    { key = "life",       label = "寿命(秒)", type = "float", min = 0.5, max = 3,   step = 0.1, default = 1.5 },
    { key = "size",       label = "粒子大小", type = "float", min = 2,   max = 16,  step = 1,   default = 8 },
    { key = "spread",     label = "散布角",   type = "float", min = 0,   max = 1.57, step = 0.05, default = 0.5 },
    { key = "speed",      label = "初速度",   type = "float", min = 10,  max = 120, step = 5,   default = 50 },
    { key = "startColor", label = "起始颜色", type = "color", default = "#FF9933" },
    { key = "endColor",   label = "结束颜色", type = "color", default = "#FF33CC" },
  }
end

function init()
  particles = {}
  spawn_acc = 0
end

function step(dt, t, params)
  params = params or {}
  local speed = math.sqrt((mouse_vx or 0) ^ 2 + (mouse_vy or 0) ^ 2)
  if speed > 0.5 then
    spawn_acc = spawn_acc + (params.rate or 100) * dt
    local heading = mouse_heading or 0
    local spread = params.spread or 0.5
    while spawn_acc >= 1 do
      spawn_acc = spawn_acc - 1
      if #particles >= MAX_PARTICLES then table.remove(particles, 1) end
      local a = heading + math.pi + rng(-spread, spread)
      local sp = (params.speed or 50) * rng(0.5, 1.4)
      particles[#particles + 1] = {
        x = mouse_x or 0, y = mouse_y or 0,
        vx = math.cos(a) * sp,
        vy = math.sin(a) * sp,
        age = 0,
        life = (params.life or 1.5) * rng(0.7, 1.2),
        size = (params.size or 8) * rng(0.6, 1.2),
      }
    end
  end

  for i = #particles, 1, -1 do
    local p = particles[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(particles, i)
    else
      p.vx = p.vx * math.pow(0.94, dt * 60)
      p.vy = p.vy * math.pow(0.94, dt * 60) + 60 * dt
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local s1r, s1g, s1b, s1a = hex_to_rgb(params.startColor or "#FF9933")
  local e1r, e1g, e1b, e1a = hex_to_rgb(params.endColor or "#FF33CC")
  for i = 1, #particles do
    local p = particles[i]
    local k = 1 - p.age / p.life
    local r = math.floor(s1r + (e1r - s1r) * (1 - k))
    local g = math.floor(s1g + (e1g - s1g) * (1 - k))
    local b = math.floor(s1b + (e1b - s1b) * (1 - k))
    local am = (s1a + (e1a - s1a) * (1 - k)) / 255
    ctx:plus_circle(p.x, p.y, p.size * (0.4 + k * 0.6), r, g, b, math.floor(230 * k * am))
  end
end
