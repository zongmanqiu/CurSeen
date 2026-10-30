-- CursorFX · emojiburst
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：emoji 从按下点向外抛出、受重力下落并逐渐淡出。CurSeen 不支持文字旋转，
-- 所有 emoji 保持正立，仅用缩放弹出近似原版的翻滚；粒子上限 200。

name = "emojiburst"
effect_name = "emojiburst"
kind = "click"
description = "CursorFX emojiburst：按下点弹出并下落的 emoji 粒子。"

local MAX_PARTICLES = 200
local particles = {}
local ITEMS = { "🎉", "✨", "💖", "🌈", "⭐", "🔥", "😍" }

function schema()
  return {
    { key = "count", label = "数量",   type = "int",   min = 4,  max = 36, step = 1, default = 14 },
    { key = "size",  label = "大小",   type = "float", min = 14, max = 56, step = 1, default = 26 },
    { key = "speed", label = "初速度", type = "float", min = 3,  max = 16, step = 0.5, default = 9 },
  }
end

function init()
  particles = {}
end

function on_press(x, y, t, params)
  local count = math.floor(params.count or 14)
  local speed = (params.speed or 9) * 60
  local size = params.size or 26
  for _ = 1, count do
    if #particles >= MAX_PARTICLES then
      break
    end
    local a = rng(0, math.pi * 2)
    local sp = rng(speed * 0.4, speed)
    local life = rng(0.85, 1.55)
    particles[#particles + 1] = {
      x = x,
      y = y,
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp - 3 * 60,
      scale = 0.25,
      target = rng(0.7, 1.2),
      item = ITEMS[rng_int(1, #ITEMS)],
      size = size,
      life = life,
      max = life,
    }
  end
end

function step(dt, t, params)
  if #particles == 0 then
    return
  end
  local damp = math.exp(-3.1 * dt)
  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.vy = p.vy + 900 * dt
      p.vx = p.vx * damp
      p.vy = p.vy * damp
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.scale = p.scale + (p.target - p.scale) * math.min(1, dt * 12)
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then
    return
  end
  for i = 1, #particles do
    local p = particles[i]
    local k = p.life / p.max
    local a = math.floor(255 * math.min(1, k * 3))
    local size = p.size * p.scale
    if a > 0 and size > 1 then
      ctx:text_centered(p.x, p.y, p.item, size, 255, 255, 255, a)
    end
  end
end
