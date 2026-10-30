-- CursorFX · fire
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：粒子从指针升起，速度越快喷得越密；静止时也持续燃烧（原版 acc 含
-- 常数项 0.9/帧，停住同样喷焰）；中心亮黄、边缘橙红，用径向渐变和加色叠加
-- 模拟火焰。粒子上限刻意压在 220，避免 CPU 软渲染下失控。

name = "fire"
effect_name = "fire"
kind = "trail"
description = "CursorFX fire：速度驱动的火焰拖尾。"

local MAX_PARTICLES = 220
local particles = {}
local spawn_acc = 0

function schema()
  return {
    { key = "rate",  label = "喷射频率", type = "float", min = 0.2, max = 4.0, step = 0.1, default = 1.0 },
    { key = "size",  label = "火焰大小", type = "float", min = 4,   max = 26,  step = 1,   default = 12 },
    { key = "life",  label = "寿命(秒)", type = "float", min = 0.15, max = 1.2, step = 0.01, default = 0.42 },
    { key = "color", label = "核心色",   type = "color", alpha = true, default = "#FFE08A" },
  }
end

function init()
  particles = {}
  spawn_acc = 0
end

-- 喷发累积放在 step 而不是 on_move：原版 fire.js:14 的 acc 每帧都加
-- (0.9 + speed*0.35)*rate，停住时也持续喷焰——之前只在 on_move 里累积，
-- 静止时整条火会熄灭，速度加成系数也误写成 0.03（原版 0.35）。
-- 上浮初速/摆动/收缩率同步对齐原版（-48~-132 px/s、30 px/s、0.95^帧）。
function step(dt, t, params)
  local f = math.min(2.0, math.max(0.0, (dt or 0.016) * 60.0))
  spawn_acc = spawn_acc + (0.9 + (mouse_speed or 0) * 0.35) * (params.rate or 1.0) * f
  local mx, my = cursor_x or 0, cursor_y or 0
  while spawn_acc >= 1 do
    spawn_acc = spawn_acc - 1
    if #particles >= MAX_PARTICLES then
      table.remove(particles, 1)
    end
    local life = (params.life or 0.42) * (0.75 + rng(0, 0.5))
    particles[#particles + 1] = {
      x = mx + rng(-6, 6),
      y = my + rng(-4, 4),
      vx = rng(-18, 18) - (mouse_vx or 0) * 0.03,
      vy = rng(-132, -48),
      size = (params.size or 12) * (0.6 + rng(0, 0.6)),
      life = life,
      max = life,
      ph = rng(0, 6.28),
    }
  end

  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.x = p.x + (p.vx + math.sin((t or 0) * 18 + p.ph) * 30) * dt
      p.y = p.y + p.vy * dt
      p.vy = p.vy + 18 * dt
      p.size = p.size * (1 - dt * 3.08)
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFE08A")
  for i = 1, #particles do
    local p = particles[i]
    local k = math.max(0, p.life / p.max)
    local a = math.floor(255 * k * (ca / 255.0))
    -- 外焰：橙红；内焰：核心色。两层径向渐变叠出白热中心。
    ctx:radial(p.x, p.y, p.size * (1.0 + (1.0 - k) * 0.8), 255, 80, 24, math.floor(a * 0.45))
    ctx:radial(p.x, p.y, p.size * 0.55, cr, cg, cb, a)
  end
end
