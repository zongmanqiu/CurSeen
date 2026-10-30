-- MouseEffects · Fire Trail
-- 参考与致谢：MouseEffects（MIT，https://github.com/ltrudu/MouseEffects）。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制上游代码/素材。
--
-- 指针拖出的火焰：黄白核心 → 橙 → 暗红的粒子随寿命变暗上浮，叠加烟与余烬。
-- 上游是 GPU 火焰纹理；这里用加色圆粒子近似，粒子上限 220。

name = "Fire Trail"
effect_name = "Fire Trail"
kind = "trail"
description = "MouseEffects Fire Trail：跟随鼠标的火焰、烟与余烬。"

local MAX_PARTICLES = 220
local flames = {}
local smokes = {}
local embers = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",   label = "喷射频率", type = "float", min = 10,  max = 80,  step = 5,   default = 45 },
    { key = "size",   label = "火焰大小", type = "float", min = 4,   max = 20,  step = 1,   default = 10 },
    { key = "life",   label = "寿命(秒)", type = "float", min = 0.3, max = 1.5, step = 0.05, default = 0.8 },
    { key = "rise",   label = "上升速度", type = "float", min = 20,  max = 120, step = 5,   default = 60 },
    { key = "smoke",  label = "烟雾量",   type = "float", min = 0,   max = 1,   step = 0.05, default = 0.3 },
    { key = "ember",  label = "余烬量",   type = "float", min = 0,   max = 1,   step = 0.05, default = 0.2 },
    { key = "color",  label = "核心颜色", type = "color", default = "#FFE08A" },
  }
end

function init()
  flames, smokes, embers = {}, {}, {}
  spawn_acc = 0
end

local function mix(a, b, t) return a * (1 - t) + b * t end

function step(dt, t, params)
  params = params or {}
  local mx, my = mouse_x or 0, mouse_y or 0
  local vx, vy = mouse_vx or 0, mouse_vy or 0
  local speed = math.sqrt(vx * vx + vy * vy)
  spawn_acc = spawn_acc + (params.rate or 45) * dt * (0.35 + math.min(2.0, speed / 20))
  while spawn_acc >= 1 do
    spawn_acc = spawn_acc - 1
    if #flames < MAX_PARTICLES then
      flames[#flames + 1] = {
        x = mx + rng(-6, 6),
        y = my + rng(-5, 5),
        vx = rng(-18, 18) - vx * 1.2,
        vy = -(params.rise or 60) * rng(0.6, 1.2) - vy * 0.6,
        size = (params.size or 10) * rng(0.5, 1.2),
        age = 0,
        life = (params.life or 0.8) * rng(0.7, 1.3),
      }
    end
    if rng(0, 1) < (params.smoke or 0.3) * 0.35 and #smokes < 60 then
      smokes[#smokes + 1] = {
        x = mx + rng(-8, 8),
        y = my + rng(-8, 8),
        vx = rng(-12, 12),
        vy = -(params.rise or 60) * 0.35,
        size = (params.size or 10) * rng(0.8, 1.8),
        age = 0, life = rng(0.8, 1.8),
      }
    end
    if rng(0, 1) < (params.ember or 0.2) * 0.25 and #embers < 60 then
      embers[#embers + 1] = {
        x = mx + rng(-10, 10), y = my + rng(-6, 6),
        vx = rng(-40, 40), vy = rng(-70, -20),
        age = 0, life = rng(0.4, 1.0),
      }
    end
  end

  for i = #flames, 1, -1 do
    local p = flames[i]
    p.age = p.age + dt
    if p.age >= p.life then
      table.remove(flames, i)
    else
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.vx = p.vx * 0.96
      p.vy = p.vy * 0.98
    end
  end
  for i = #smokes, 1, -1 do
    local p = smokes[i]
    p.age = p.age + dt
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt
    p.size = p.size + 10 * dt
    if p.age >= p.life then table.remove(smokes, i) end
  end
  for i = #embers, 1, -1 do
    local p = embers[i]
    p.age = p.age + dt
    p.x = p.x + p.vx * dt
    p.y = p.y + p.vy * dt
    p.vy = p.vy + 60 * dt
    if p.age >= p.life then table.remove(embers, i) end
  end
end

function draw(ctx, params)
  params = params or {}
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFE08A")
  local am = ca / 255
  for i = 1, #smokes do
    local p = smokes[i]
    local a = math.floor(70 * (1 - p.age / p.life))
    ctx:circle(p.x, p.y, p.size, 70, 70, 78, a)
  end
  for i = 1, #flames do
    local p = flames[i]
    local k = 1 - p.age / p.life
    local r = math.floor(mix(120, cr, k))
    local g = math.floor(mix(20, cg, k * 0.9))
    local b = math.floor(mix(12, cb, k * 0.8))
    ctx:plus_circle(p.x, p.y, p.size * (0.5 + k * 0.7), r, g, b, math.floor(220 * k * am))
  end
  for i = 1, #embers do
    local p = embers[i]
    local k = 1 - p.age / p.life
    ctx:plus_circle(p.x, p.y, 1.2, 255, 180, 80, math.floor(230 * k))
  end
end
