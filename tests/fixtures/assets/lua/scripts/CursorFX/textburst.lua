-- CursorFX · textburst
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：漫画拟声词从按下点跳出、受重力落下。原版会轻微旋转，CurSeen 没有文字
-- 旋转 API，这里全部正立，用缩放弹出和深色描边维持漫画感；粒子上限 200。

name = "textburst"
effect_name = "textburst"
kind = "click"
description = "CursorFX textburst：按下点弹出的漫画拟声词。"

local MAX_PARTICLES = 200
local particles = {}
local WORDS = { "POW!", "BOOM!", "WOW!", "ZAP!", "BAM!", "YES!" }
local COLORS = { "#FFE600", "#FF3B3B", "#3BD1FF", "#7CFF5A", "#FF7CF0" }

function schema()
  return {
    { key = "count", label = "数量",   type = "int",   min = 2,  max = 14, step = 1, default = 5 },
    { key = "size",  label = "字号",   type = "float", min = 16, max = 56, step = 1, default = 28 },
    { key = "speed", label = "初速度", type = "float", min = 2,  max = 12, step = 0.5, default = 6 },
  }
end

function init()
  particles = {}
end

function on_press(x, y, t, params)
  local count = math.floor(params.count or 5)
  local speed = (params.speed or 6) * 60
  local size = params.size or 28
  for _ = 1, count do
    if #particles >= MAX_PARTICLES then
      break
    end
    local a = rng(0, math.pi * 2)
    local sp = rng(speed * 0.4, speed)
    local life = rng(0.9, 1.6)
    local cr, cg, cb = hex_to_rgb(COLORS[rng_int(1, #COLORS)])
    particles[#particles + 1] = {
      x = x,
      y = y,
      vx = math.cos(a) * sp,
      vy = math.sin(a) * sp - 4 * 60,
      scale = 0.2,
      target = rng(0.8, 1.15),
      word = WORDS[rng_int(1, #WORDS)],
      r = cr,
      g = cg,
      b = cb,
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
      p.vy = p.vy + 720 * dt
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
    local fade = math.min(1, k * 3)
    local a = math.floor(255 * fade)
    local size = p.size * p.scale
    if a > 0 and size > 1 then
      ctx:text_centered(p.x, p.y, p.word, size + 3, 18, 18, 24, math.floor(a * 0.8))
      ctx:text_centered(p.x, p.y, p.word, size, p.r, p.g, p.b, a)
    end
  end
end
