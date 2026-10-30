-- CursorFX · emojirain
-- 复刻自 CursorFX（https://github.com/devkancheti4-design/cursorfx），MIT License。
-- 本文件为 CurSeen 的独立 Lua 重实现，未复制原项目代码/素材。
--
-- 拖尾类：随机 emoji 从指针连续落下、受重力加速并带空气阻力。CurSeen 没有文字
-- 旋转接口，这里用 text_centered 正立绘制，不做逐粒自转（原版 rot/spin 省略）。

name = "emojirain"
effect_name = "emojirain"
kind = "trail"
description = "CursorFX emoji rain：emoji 从指针落下并加速。"

local MAX_PARTICLES = 200
local EMOJI = { "😀", "😂", "😍", "🥳", "🔥", "💯" }

local particles = {}
local acc = 0
local idle_acc = 0
local last_x, last_y = nil, nil
local seen = false

function schema()
  return {
    { key = "rate",  label = "生成频率", type = "float", min = 0.2, max = 4.0, step = 0.1, default = 1.0 },
    { key = "size",  label = "字号",     type = "float", min = 10,  max = 40,  step = 1,   default = 22 },
    { key = "life",  label = "最长寿命(秒)", type = "float", min = 0.6, max = 2.5, step = 0.01, default = 1.33 },
    { key = "emoji", label = "自定义表情（留空随机）", type = "str", default = "" },
    { key = "color", label = "文字颜色", type = "color", default = "#FFFFFF" },
    { key = "rainbow", label = "彩虹色", type = "bool", default = true },
  }
end

local function add_particle(x, y, params)
  local max_life = params.life or 1.33
  local ch = params.emoji
  if ch == nil or ch == "" then
    ch = EMOJI[rng_int(1, #EMOJI)]
  end

  if #particles >= MAX_PARTICLES then
    table.remove(particles, 1)
  end

  local scale = rng(0.7, 1.2)
  particles[#particles + 1] = {
    x = x + rng(-2, 2),
    y = y,
    vx = (rng(-2.5, 2.5) + (mouse_vx or 0) * 0.15) * 60,
    vy = rng(-3, 0) * 60,
    size = (params.size or 22) * scale,
    life = max_life * rng(0.625, 1.0),
    max = max_life,
    ch = ch,
    ph = rng(0, 6.2831853),
    hue = rng(0, 360),
  }
end

function init()
  particles = {}
  acc = 0
  idle_acc = 0
  last_x, last_y = nil, nil
  seen = false
end

function on_move(x, y, t, params)
  if not seen then
    seen = true
    acc = acc + 1
  end

  if last_x then
    local dx, dy = x - last_x, y - last_y
    local dist = math.sqrt(dx * dx + dy * dy)
    local speed = math.max(mouse_speed or 0, 0.5)
    -- 原版每帧 (0.02 + speed*0.06)。
    acc = acc + dist * (0.06 + 0.02 / speed) * (params.rate or 1)
  end
  last_x, last_y = x, y

  local budget = 4
  while acc >= 1 and budget > 0 do
    acc = acc - 1
    budget = budget - 1
    add_particle(x, y, params)
  end
  if acc > 6 then acc = 6 end
end

function step(dt, t, params)
  if seen and (mouse_speed or 0) < 1.5 then
    idle_acc = idle_acc + 0.02 * (params.rate or 1) * dt * 60
    local budget = 2
    while idle_acc >= 1 and budget > 0 do
      idle_acc = idle_acc - 1
      budget = budget - 1
      add_particle(mouse_x or 0, mouse_y or 0, params)
    end
  end

  local keep = {}
  for i = 1, #particles do
    local p = particles[i]
    p.life = p.life - dt
    if p.life > 0 then
      p.vy = p.vy + 648 * dt -- 0.18 px/帧² → 648 px/s²
      p.x = p.x + p.vx * dt
      p.y = p.y + p.vy * dt
      p.vx = p.vx * (0.97 ^ (dt * 60))
      keep[#keep + 1] = p
    end
  end
  particles = keep
end

function draw(ctx, params)
  if #particles == 0 then return end

  local cr, cg, cb, ca = hex_to_rgb(params.color or "#FFFFFF")
  for i = 1, #particles do
    local p = particles[i]
    local t = clamp(p.life / p.max, 0, 1)
    local a8 = math.floor(clamp(math.min(1, t * 3), 0, 1) * 255 * ca / 255)
    if a8 > 0 then
      local r, g, b = cr, cg, cb
      if params.rainbow ~= false then
        r, g, b = hsv_to_rgb((p.hue + (elapsed or 0) * 50.0) % 360.0, 1.0, 1.0)
      end
      ctx:text_centered(p.x, p.y, p.ch, p.size, r, g, b, a8)
    end
  end
end
