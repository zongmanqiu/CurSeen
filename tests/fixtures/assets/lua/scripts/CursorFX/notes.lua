-- CursorFX · notes
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 点击类：音符从按下点上浮、左右轻摆并淡出。CurSeen 没有文字旋转，音符保持正立，
-- 用字号缩放和颜色区分代替原版的轻微倾斜；粒子上限 120。

name = "notes"
effect_name = "notes"
kind = "click"
description = "CursorFX notes：从按下点上浮的彩色音符。"

local MAX_PARTICLES = 200
local particles = {}
local SYMBOLS = { "♪", "♫", "♬", "♩" }
local COLORS = { "#FFD166", "#8ECAE6", "#FF8FAB", "#C77DFF", "#80ED99" }

function schema()
  return {
    { key = "count", label = "音符数", type = "int",   min = 2, max = 20, step = 1, default = 6 },
    { key = "size",  label = "字号",   type = "float", min = 14, max = 48, step = 1, default = 26 },
  }
end

function init()
  particles = {}
end

function on_press(x, y, t, params)
  local count = math.floor(params.count or 6)
  local size = params.size or 26
  for _ = 1, count do
    if #particles >= MAX_PARTICLES then
      break
    end
    local life = rng(1.2, 1.9)
    local er, eg, eb = hex_to_rgb(COLORS[rng_int(1, #COLORS)])
    particles[#particles + 1] = {
      x = x + rng(-8, 8),
      y = y + rng(-4, 4),
      vx = rng(-0.8, 0.8) * 60,
      vy = rng(-2.5, -1.0) * 60,
      sym = SYMBOLS[rng_int(1, #SYMBOLS)],
      r = er,
      g = eg,
      b = eb,
      ph = rng(0, 6),
      size = size * rng(0.85, 1.15),
      life = life,
      max = life,
    }
  end
end

function step(dt, t, params)
  if #particles == 0 then
    return
  end
  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + (p.vx + math.sin((t or 0) * 3 + p.ph) * 48) * dt
      p.y = p.y + p.vy * dt
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
    if a > 0 then
      local intro = math.min(1, (p.max - p.life) / 0.14)
      local scale = 0.75 + 0.25 * intro
      ctx:text_centered(p.x, p.y, p.sym, p.size * scale, p.r, p.g, p.b, a)
    end
  end
end
