-- MouseEffects · Smoke
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针拖出的烟雾：软边烟团边升边膨胀边变淡，少量横向漂移。
-- 上游 100 个粒子；这里按软渲染同样压到 100 个（池大小与上游一致），
-- 每个一团径向渐变。

name = "Smoke"
effect_name = "Smoke"
kind = "trail"
description = "MouseEffects Smoke：跟随鼠标上升的烟雾。"

local MAX_PUFFS = 100
local puffs = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",    label = "生成频率", type = "float", min = 8,   max = 40,  step = 2,   default = 20 },
    { key = "size",    label = "烟团大小", type = "float", min = 8,   max = 30,  step = 1,   default = 18 },
    { key = "life",    label = "寿命(秒)", type = "float", min = 1,   max = 5,   step = 0.1, default = 2.5 },
    { key = "rise",    label = "上升速度", type = "float", min = 10,  max = 100, step = 5,   default = 45 },
    { key = "drift",   label = "横向漂移", type = "float", min = 0,   max = 60,  step = 5,   default = 20 },
    { key = "opacity", label = "不透明度", type = "float", min = 0.1, max = 0.7, step = 0.05, default = 0.35 },
    { key = "color",   label = "烟色",     type = "color", default = "#B0B0B8" },
  }
end

function init()
  puffs = {}
  spawn_acc = 0
end

function step(dt, t, params)
  params = params or {}
  local speed = math.sqrt((mouse_vx or 0) ^ 2 + (mouse_vy or 0) ^ 2)
  if speed > 0.5 then
    spawn_acc = spawn_acc + (params.rate or 20) * dt
    while spawn_acc >= 1 do
      spawn_acc = spawn_acc - 1
      if #puffs >= MAX_PUFFS then table.remove(puffs, 1) end
      puffs[#puffs + 1] = {
        x = (mouse_x or 0) + rng(-6, 6),
        y = (mouse_y or 0) + rng(-6, 6),
        vx = rng(-1, 1) * (params.drift or 20),
        vy = -(params.rise or 45) * rng(0.6, 1.3),
        size = (params.size or 18) * rng(0.6, 1.2),
        age = 0,
        life = (params.life or 2.5) * rng(0.7, 1.3),
        phase = rng(0, math.pi * 2),
      }
    end
  end

  for i = #puffs, 1, -1 do
    local p = puffs[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(puffs, i)
    else
      p.phase = p.phase + dt * 0.8
      p.size = p.size + 12 * dt
      p.x = p.x + (p.vx + math.sin(p.phase) * 6) * dt
      p.y = p.y + p.vy * dt
      p.vy = p.vy * 0.99
    end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#B0B0B8")
  local am = ca / 255
  local op = params.opacity or 0.35
  for i = 1, #puffs do
    local p = puffs[i]
    local k = 1 - p.age / p.life
    local a = math.floor(150 * op * k * am)
    ctx:radial_stops(p.x, p.y, p.size, {
      { offset = 0.0, r = cr, g = cg, b = cb, a = a },
      { offset = 0.55, r = cr, g = cg, b = cb, a = math.floor(a * 0.55) },
      { offset = 1.0, r = cr, g = cg, b = cb, a = 0 },
    })
  end
end
